use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, slice};

use adbc_core::{
    error::{Error, Result, Status},
    options::{AdbcVersion, ObjectDepth, OptionStatement, OptionValue},
};
use arrow_array::RecordBatch;
use arrow_schema::{Schema, SchemaRef};
use dbt_pretty_table::pretty_data_table;
use dialoguer::{BasicHistory, Input, theme::ColorfulTheme};

use crate::{Backend, Connection, Database, Driver, connection, database, driver};

struct Profile {
    backend: Backend,
    path: PathBuf,
    options: Vec<(String, String)>,
}

fn parse_backend(s: &str) -> Result<Backend> {
    match s.to_lowercase().as_str() {
        "snowflake" => Ok(Backend::Snowflake),
        "bigquery" => Ok(Backend::BigQuery),
        "postgres" | "postgresql" => Ok(Backend::Postgres),
        "databricks" => Ok(Backend::Databricks),
        "redshift" => Ok(Backend::Redshift),
        "spark" => Ok(Backend::Spark),
        "salesforce" => Ok(Backend::Salesforce),
        "duckdb" => Ok(Backend::DuckDB),
        "sqlserver" | "mssql" => Ok(Backend::SQLServer),
        "athena" => Ok(Backend::Athena),
        "clickhouse" => Ok(Backend::ClickHouse),
        "exasol" => Ok(Backend::Exasol),
        _ => Err(Error::with_message_and_status(
            format!("Unsupported backend: {s}"),
            Status::InvalidArguments,
        )),
    }
}

fn load_profile(config_path: &Path, profile_name: &str) -> Result<Profile> {
    let text = fs::read_to_string(config_path).map_err(|e| {
        Error::with_message_and_status(
            format!("failed to read config file {}: {e}", config_path.display()),
            Status::IO,
        )
    })?;
    let mut table: toml::Table = toml::from_str(&text).map_err(|e| {
        Error::with_message_and_status(
            format!("failed to parse TOML config {}: {e}", config_path.display()),
            Status::InvalidArguments,
        )
    })?;

    let profile_value = table.remove(profile_name).ok_or_else(|| {
        Error::with_message_and_status(
            format!(
                "profile '{profile_name}' not found in {}",
                config_path.display()
            ),
            Status::InvalidArguments,
        )
    })?;

    let mut profile_table = match profile_value {
        toml::Value::Table(t) => t,
        _ => {
            return Err(Error::with_message_and_status(
                format!("profile '{profile_name}' must be a TOML table"),
                Status::InvalidArguments,
            ));
        }
    };

    let backend_value = profile_table.remove("backend").ok_or_else(|| {
        Error::with_message_and_status(
            format!("profile '{profile_name}' is missing required key 'backend'"),
            Status::InvalidArguments,
        )
    })?;
    let backend_str = match backend_value {
        toml::Value::String(s) => s,
        _ => {
            return Err(Error::with_message_and_status(
                format!("profile '{profile_name}' key 'backend' must be a string"),
                Status::InvalidArguments,
            ));
        }
    };
    let backend = parse_backend(&backend_str)?;

    let path_value = profile_table.remove("path").ok_or_else(|| {
        Error::with_message_and_status(
            format!("profile '{profile_name}' is missing required key 'path'"),
            Status::InvalidArguments,
        )
    })?;
    let path = match path_value {
        toml::Value::String(s) => PathBuf::from(s),
        _ => {
            return Err(Error::with_message_and_status(
                format!("profile '{profile_name}' key 'path' must be a string"),
                Status::InvalidArguments,
            ));
        }
    };

    let mut options = Vec::with_capacity(profile_table.len());
    for (key, value) in profile_table {
        let value_str = match value {
            toml::Value::String(s) => s,
            other => {
                return Err(Error::with_message_and_status(
                    format!(
                        "profile '{profile_name}' option '{key}' must be a string, got {}",
                        other.type_str()
                    ),
                    Status::InvalidArguments,
                ));
            }
        };
        options.push((key, value_str));
    }

    Ok(Profile {
        backend,
        path,
        options,
    })
}

pub struct ReplState {
    _driver: Box<dyn Driver>,
    _database: Box<dyn Database>,
    connection: Box<dyn Connection>,
    // TODO(jasonlin45): figure out the lifetime restriction here so we can directly store RecordBatchReader
    current_schema: Option<SchemaRef>,
    current_batches: Vec<RecordBatch>,
    current_batch_idx: usize,
    pending_options: Vec<(String, String)>,
}

impl ReplState {
    fn from_profile(profile: Profile) -> Result<Self> {
        let path_str = profile
            .path
            .to_str()
            .ok_or_else(|| {
                Error::with_message_and_status(
                    format!("driver path is not valid UTF-8: {}", profile.path.display()),
                    Status::InvalidArguments,
                )
            })?
            .to_string();

        let mut driver = driver::Builder::new(
            profile.backend,
            driver::LoadStrategy::System(Some(path_str)),
        )
        .with_adbc_version(AdbcVersion::V110)
        .try_load()?;

        let mut database_builder = database::Builder::new(profile.backend);
        for (key, value) in profile.options {
            match key.as_str() {
                "uri" => {
                    database_builder.with_parse_uri(value)?;
                }
                "username" => {
                    database_builder.with_username(value);
                }
                "password" => {
                    database_builder.with_password(value);
                }
                _ => {
                    database_builder.with_named_option(&key, value)?;
                }
            }
        }

        let mut database = database_builder.build(&mut driver)?;
        let connection = connection::Builder::default().build(&mut database)?;

        Ok(Self {
            _driver: driver,
            _database: database,
            connection,
            current_schema: None,
            current_batches: Vec::new(),
            current_batch_idx: 0,
            pending_options: Vec::new(),
        })
    }

    pub fn set_statement_option(&mut self, key: String, value: String) {
        if let Some(existing) = self.pending_options.iter_mut().find(|(k, _)| *k == key) {
            existing.1 = value;
        } else {
            self.pending_options.push((key, value));
        }
    }

    pub fn pending_options(&self) -> &[(String, String)] {
        &self.pending_options
    }

    pub fn execute_query(&mut self, query: &str) -> Result<(usize, usize)> {
        if query.trim().is_empty() {
            return Ok((0, 0));
        }

        let conn = self.connection.as_mut();
        let mut stmt = conn.new_statement()?;
        for (key, value) in std::mem::take(&mut self.pending_options) {
            stmt.set_option(
                OptionStatement::from(key.as_str()),
                OptionValue::String(value),
            )?;
        }
        stmt.set_sql_query(query)?;
        let reader = stmt.execute()?;

        let num_cols = reader.schema().fields().len();
        self.current_schema = Some(reader.schema());

        // grab all the batches
        self.current_batches = reader
            .map(|r| r.map_err(|e| Error::with_message_and_status(e.to_string(), Status::IO)))
            .collect::<Result<Vec<_>>>()?;
        let num_batches = self.current_batches.len();
        self.current_batch_idx = 0;

        Ok((num_batches, num_cols))
    }

    pub fn show_schema(&self) -> Result<Option<SchemaRef>> {
        Ok(self.current_schema.clone())
    }

    pub fn show_batch(&self) -> Result<Option<RecordBatch>> {
        if self.current_batches.is_empty() {
            Ok(None)
        } else {
            Ok(Some(self.current_batches[self.current_batch_idx].clone()))
        }
    }

    pub fn move_pointer(&mut self, delta: isize) -> Result<()> {
        let new_idx = if delta < 0 {
            self.current_batch_idx.checked_sub(delta.unsigned_abs())
        } else {
            self.current_batch_idx.checked_add(delta as usize)
        };

        if let Some(idx) = new_idx {
            if idx >= self.current_batches.len() {
                Err(Error::with_message_and_status(
                    format!("Out of range {idx}"),
                    Status::InvalidArguments,
                ))
            } else {
                self.current_batch_idx = idx;
                Ok(())
            }
        } else {
            Err(Error::with_message_and_status(
                "Index overflow".to_string(),
                Status::InvalidArguments,
            ))
        }
    }
}

enum Command {
    Query { query: String },
    // move current pointer in batch by some amount
    Move { delta: isize },
    ReloadDriver,
    ShowSchema,
    ShowBatch,
    GetObjects { identifier: Option<String> },
    GetSchema { identifier: String },
    SetOption { key: String, value: String },
    ShowOptions,
    Help,
    Quit,
    Invalid,
}

fn parse_command(line: &str) -> Option<Command> {
    let line = if let Some(rest) = line.strip_prefix(':') {
        rest.trim()
    } else {
        return Some(Command::Query {
            query: line.to_string(),
        });
    };

    let (cmd, rest) = match line.split_once(char::is_whitespace) {
        Some((c, r)) => (c, r.trim()),
        None => (line, ""),
    };

    match cmd {
        "q" | "exit" | "quit" => Some(Command::Quit),
        "h" | "help" => Some(Command::Help),
        "r" | "reload" => Some(Command::ReloadDriver),
        "ss" | "show-schema" => Some(Command::ShowSchema),
        "sb" | "show-batch" => Some(Command::ShowBatch),
        "p" | "prev" => Some(Command::Move { delta: -1 }),
        "n" | "next" => Some(Command::Move { delta: 1 }),
        "m" | "move" => {
            if let Ok(delta) = rest.parse::<isize>() {
                Some(Command::Move { delta })
            } else {
                Some(Command::Invalid)
            }
        }
        "go" | "get-objects" => {
            let identifier = if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            };
            Some(Command::GetObjects { identifier })
        }
        "so" | "set-option" => {
            if rest.is_empty() {
                return Some(Command::ShowOptions);
            }
            let (key, value) = match rest.split_once(['=', ' ', '\t']) {
                Some((k, v)) => (k.trim(), v.trim_start().trim_start_matches('=').trim()),
                None => return Some(Command::Invalid),
            };
            if key.is_empty() {
                Some(Command::Invalid)
            } else {
                Some(Command::SetOption {
                    key: key.to_string(),
                    value: value.to_string(),
                })
            }
        }
        "gs" | "get-schema" => {
            if rest.is_empty() {
                Some(Command::Invalid)
            } else {
                Some(Command::GetSchema {
                    identifier: rest.to_string(),
                })
            }
        }
        _ => Some(Command::Invalid),
    }
}

fn parse_table_identifier(s: &str) -> (Option<&str>, Option<&str>, &str) {
    let parts: Vec<&str> = s.split('.').collect();
    match parts.len() {
        1 => (None, None, parts[0]),
        2 => (None, Some(parts[0]), parts[1]),
        _ => (Some(parts[0]), Some(parts[1]), parts[2]),
    }
}

// Prints a visualization of a schema to stdout
fn visualize_schema(schema: Arc<Schema>) {
    println!("Schema");
    println!("├─ Fields");

    for field in schema.fields.iter() {
        println!("│  ├─ {}", field.name());
        println!("│  │  ├─ Type: {:?}", field.data_type());
        println!("│  │  ├─ Nullable: {}", field.is_nullable());

        if !field.metadata().is_empty() {
            println!("│  │  └─ Metadata");
            let entries: Vec<_> = field.metadata().iter().collect();
            for (i, (key, value)) in entries.iter().enumerate() {
                let prefix = if i == entries.len() - 1 {
                    "└─"
                } else {
                    "├─"
                };
                println!("│  │     {prefix} {key}: {value}");
            }
        }
    }

    // todo: break this out to a function with indent levels
    if !&schema.metadata.is_empty() {
        println!("└─ Metadata");
        for (key, value) in &schema.metadata {
            println!("   └─ {key}: {value}");
        }
    }
}

fn print_batches(title: &str, batches: &[RecordBatch]) {
    if batches.is_empty() {
        println!("(no rows)");
        return;
    }
    let column_names: Vec<String> = batches[0]
        .schema()
        .fields()
        .iter()
        .map(|field| field.name().to_string())
        .collect();
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    match pretty_data_table(
        title,
        "",
        &column_names,
        batches,
        dbt_pretty_table::DisplayFormat::Table,
        Some(50),
        true,
        Some(total_rows),
    ) {
        Ok(table) => println!("{table}"),
        Err(_) => {
            eprintln!("Failed to pretty print as table.");
            for batch in batches {
                println!("{batch:#?}");
            }
        }
    }
}

pub async fn run_repl(config_path: &Path, profile_name: &str) -> Result<()> {
    let profile = load_profile(config_path, profile_name)?;
    let backend = profile.backend;
    let driver_path = profile.path.clone();

    let mut state = ReplState::from_profile(profile)?;

    let mut history = BasicHistory::new().max_entries(8).no_duplicates(true);
    let theme = ColorfulTheme::default();

    println!("Welcome to dbt-adbc REPL!");
    println!(
        "Loaded profile '{profile_name}' from {} ({backend}, driver: {})",
        config_path.display(),
        driver_path.display(),
    );
    println!("Type :help for available commands");
    println!("Type :quit to exit");

    let prompt = format!("dbt-adbc | {backend}>");

    loop {
        let input: String = Input::with_theme(&theme)
            .with_prompt(&prompt)
            .history_with(&mut history)
            .interact_text()
            .map_err(|e| Error::with_message_and_status(e.to_string(), Status::IO))?;

        match parse_command(&input) {
            Some(Command::Query { query }) => {
                println!("Executing query...");
                match state.execute_query(&query) {
                    Ok((batches, cols)) => {
                        println!("Successfully executed query.");
                        println!("{batches} batches with {cols} columns returned.");
                        println!("  :show-schema    - Show schema");
                        println!("  :show-batch     - Show current batch");
                    }
                    Err(e) => {
                        eprintln!("Error executing query: {e}");
                        continue;
                    }
                }
            }
            Some(Command::Move { delta }) => {
                if let Err(e) = state.move_pointer(delta) {
                    eprintln!("Error moving pointer: {e}");
                    continue;
                }
            }
            Some(Command::Help) => {
                println!("Available commands:");
                println!("  :help, :h                      - Show this help message");
                println!("  <query>                        - Execute SQL query");
                println!("  :show-schema, :ss              - Show current schema");
                println!("  :show-batch, :sb               - Show current batch");
                println!(
                    "  :move, :m <int>                - Move current batch pointer. Negative values move backwards, positive values move forwards."
                );
                println!("  :prev, :p                      - Move to previous batch");
                println!("  :next, :n                      - Advance to next batch");
                println!(
                    "  :get-objects, :go [<a.b.c>]    - List objects (catalogs/schemas/tables) optionally filtered by an identifier"
                );
                println!(
                    "  :get-schema, :gs <a.b.c>       - Show the Arrow schema of the table identified by <catalog.schema.table>"
                );
                println!(
                    "  :set-option, :so <key> <value> - Set a statement option applied to the next executed statement (also accepts <key>=<value>); with no arguments, list pending options"
                );
                println!(
                    "  :reload, :r                    - Reload the adbc driver from the config file"
                );
                println!("  :quit, :q                      - Exit the REPL");
            }
            Some(Command::ShowSchema) => {
                if let Some(schema) = state.show_schema()? {
                    visualize_schema(schema);
                } else {
                    println!("No schema found");
                }
            }
            Some(Command::ShowBatch) => {
                if let Ok(Some(batch)) = state.show_batch() {
                    let column_names: Vec<String> = batch
                        .schema()
                        .fields()
                        .iter()
                        .map(|field| field.name().to_string())
                        .collect();

                    if let Ok(table) = pretty_data_table(
                        "Query Results",
                        "",
                        &column_names,
                        slice::from_ref(&batch),
                        dbt_pretty_table::DisplayFormat::Table,
                        Some(10),
                        true,
                        Some(batch.num_rows()),
                    ) {
                        println!("{table}");
                    } else {
                        eprintln!("Failed to pretty print as table.");
                        // fallback: dump as a debug print
                        println!("{batch:#?}");
                    }
                } else {
                    println!("No batch found!");
                }
            }
            Some(Command::GetObjects { identifier }) => {
                let (catalog, db_schema, table_name) = match identifier {
                    Some(ref id) => {
                        let (c, s, t) = parse_table_identifier(id);
                        (c, s, Some(t))
                    }
                    None => (None, None, None),
                };
                match state.connection.get_objects(
                    ObjectDepth::All,
                    catalog,
                    db_schema,
                    table_name,
                    None,
                    None,
                ) {
                    Ok(reader) => {
                        let collected: std::result::Result<Vec<_>, _> = reader.collect();
                        match collected {
                            Ok(batches) => print_batches("Objects", &batches),
                            Err(e) => eprintln!("Error reading objects: {e}"),
                        }
                    }
                    Err(e) => eprintln!("Error getting objects: {e}"),
                }
            }
            Some(Command::GetSchema { identifier }) => {
                let (catalog, db_schema, table_name) = parse_table_identifier(&identifier);
                match state
                    .connection
                    .get_table_schema(catalog, db_schema, table_name)
                {
                    Ok(schema) => visualize_schema(Arc::new(schema)),
                    Err(e) => eprintln!("Error getting table schema: {e}"),
                }
            }
            Some(Command::SetOption { key, value }) => {
                state.set_statement_option(key.clone(), value.clone());
                println!("Option '{key}' = '{value}' will be set on the next statement");
            }
            Some(Command::ShowOptions) => {
                let options = state.pending_options();
                if options.is_empty() {
                    println!("No pending statement options");
                } else {
                    println!("Options for the next statement:");
                    for (key, value) in options {
                        println!("  {key} = {value}");
                    }
                }
            }
            Some(Command::ReloadDriver) => {
                // TODO(jasonlin45) the actual binary ends up cached in driver.rs
                println!("Reloading driver...");
                let profile = match load_profile(config_path, profile_name) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Failed to reload profile: {e}");
                        continue;
                    }
                };
                match ReplState::from_profile(profile) {
                    Ok(new_state) => {
                        state = new_state;
                        println!("Driver reloaded successfully");
                    }
                    Err(e) => {
                        eprintln!("Failed to rebuild connection: {e}");
                    }
                }
            }
            Some(Command::Quit) => break,
            Some(Command::Invalid) => {
                eprintln!("Invalid command. Type :help for available commands");
            }
            None => {}
        }
    }

    Ok(())
}
