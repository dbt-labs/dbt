//! The shared in-memory DuckDB that seed CSVs are read through, so the rows can
//! be sent to the target over the connection (Arrow bulk ingest) instead of the
//! target running `COPY ... FROM <local path>`. A remote DuckDB such as
//! MotherDuck with `motherduck_saas_mode` cannot read local files at all.

use std::sync::{Condvar, Mutex, OnceLock, PoisonError};

use adbc_core::options::{OptionStatement, OptionValue};
use arrow_schema::Schema;
use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::ident::{escape_string_literal, quote_identifier};
use dbt_adapter_sql::types::SqlType;
use dbt_adbc::driver::Builder as DriverBuilder;
use dbt_adbc::{Backend, Connection, Database, LoadStrategy, QueryCtx, database};
use dbt_common::stdfs;

use crate::AdapterResult;
use crate::errors::{AdapterError, AdapterErrorKind, adbc_error_to_adapter_error};
use crate::statement::DBT_NODE_ID;

/// Most seeds in flight at once through the shared writer. See [`DuckDbSeedWriter`].
pub(crate) const MAX_CONCURRENT_SEED_WRITES: usize = 5;

/// The one in-memory DuckDB every seed is read through, bound to
/// [`MAX_CONCURRENT_SEED_WRITES`] concurrent seeds.
///
/// Terms:
/// - **Instance**: the in-memory DuckDB database. It is created on the first
///   seed by [`get_duckdb_seed_writer`] and lives until the process exits.
///   Idle, it holds roughly 15-40 MB.
/// - **Connection (cursor)**: opened per seed from the instance and dropped
///   when that seed finishes, which gives that seed's memory back.
/// - **Permit**: a slot in the concurrency bound. A seed holds one for as long
///   as it reads and ingests, so at most `MAX_CONCURRENT_SEED_WRITES` seeds are
///   in flight and the rest wait.
/// - **Allowed directories**: every loaded package's seed directories. DuckDB
///   cannot widen them once external access is off, which is why they are set
///   once, at creation, from the registry in `dbt-schemas`.
///
/// Why one instance and a bound: the memory of reading seeds is driven by how
/// many are read at once, not by how many instances exist. Measured on macOS
/// with 15 seeds of 10 MB (peak RSS of a standalone reader, not the adapter):
///
/// | concurrent seeds | peak RSS | speed vs 1 |
/// |---|---|---|
/// | 1 | ~80 MB | 1x |
/// | 3 | ~150 MB | 3x |
/// | 5 | ~220 MB | 4.6x |
/// | 15 | ~460 MB | 5.7x |
///
/// dbt Cloud pods have 600 MiB for the whole run, so 5 keeps most of the speed
/// at about half the peak of running all seeds at once.
///
/// Tuning: lower `MAX_CONCURRENT_SEED_WRITES` if seeds hit the memory limit,
/// raise it if seed loading is slow and memory is spare. DuckDB's own `threads`
/// and `memory_limit` are not set here and can be added to the `SET` statements
/// in [`get_duckdb_seed_writer`] to cap per-instance CPU and memory further.
///
/// How it interacts with the runtime: seeds run on the `dbt_runtime` blocking
/// pool, whose thread cap is lowered to the `threads` setting at startup
/// (default cap 48, `--no-parallel` pins it to 1). The pool can never run more
/// than `threads` seeds at once, so:
/// - with `threads <= MAX_CONCURRENT_SEED_WRITES` the permits are never
///   exhausted, nothing ever waits, and `threads` is the effective bound;
/// - with `threads > MAX_CONCURRENT_SEED_WRITES`, up to `threads - MAX` seeds
///   sleep in [`DuckDbSeedWriter::acquire`], each holding a pool thread while it
///   waits. A waiting seed does not observe cancellation until it has a permit.
///
/// **Rule: nothing that runs while a permit is held may wait on another task on
/// the same pool** (no `spawn_blocking` followed by a wait on the result, no
/// `block_on` on pool work). The holders already have their own pool threads,
/// so they finish and release permits as long as they never need another one.
/// If one did, and sleeping seeds had filled the pool to its cap, the holder
/// would wait for a thread, the sleepers would wait for its permit, and nothing
/// would move. The read, the ingest and the ADBC calls under them are
/// synchronous today; keep them that way.
pub(crate) struct DuckDbSeedWriter {
    database: Box<dyn Database>,
    in_flight: Mutex<usize>,
    freed: Condvar,
}

pub(crate) struct SeedWritePermit<'a>(&'a DuckDbSeedWriter);

impl Drop for SeedWritePermit<'_> {
    fn drop(&mut self) {
        *self
            .0
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner) -= 1;
        self.0.freed.notify_one();
    }
}

impl DuckDbSeedWriter {
    /// Blocks until fewer than `MAX_CONCURRENT_SEED_WRITES` seeds are in flight.
    pub(crate) fn acquire(&self) -> SeedWritePermit<'_> {
        let in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut in_flight = self
            .freed
            .wait_while(in_flight, |n| *n >= MAX_CONCURRENT_SEED_WRITES)
            .unwrap_or_else(PoisonError::into_inner);
        *in_flight += 1;
        SeedWritePermit(self)
    }

    /// A new connection to the shared instance. Drop it when the seed is done.
    pub(crate) fn new_connection(&self) -> AdapterResult<Box<dyn Connection>> {
        self.database
            .new_connection()
            .map_err(adbc_error_to_adapter_error)
    }
}

static DUCKDB_SEED_WRITER: OnceLock<AdapterResult<DuckDbSeedWriter>> = OnceLock::new();

/// Gets the shared seed writer, creating it on the first call.
pub(crate) fn get_duckdb_seed_writer() -> AdapterResult<&'static DuckDbSeedWriter> {
    fn init() -> AdapterResult<DuckDbSeedWriter> {
        let directories = dbt_schemas::state::canonical_seed_dirs()
            .iter()
            .map(|dir| format!("'{}'", escape_string_literal(dir, AdapterType::DuckDB)))
            .collect::<Vec<_>>()
            .join(", ");

        let mut driver = DriverBuilder::new(Backend::DuckDB, LoadStrategy::SystemThenCdnCache)
            .try_load()
            .map_err(adbc_error_to_adapter_error)?;
        let mut builder = database::Builder::new(Backend::DuckDB);
        builder
            .with_named_option("path", ":memory:")
            .map_err(adbc_error_to_adapter_error)?;
        let database = builder
            .build(&mut driver)
            .map_err(adbc_error_to_adapter_error)?;

        let mut conn = database
            .new_connection()
            .map_err(adbc_error_to_adapter_error)?;
        for sql in [
            "SET autoinstall_known_extensions = false".to_string(),
            "SET autoload_known_extensions = false".to_string(),
            format!("SET allowed_directories = [{directories}]"),
            "SET enable_external_access = false".to_string(),
            "SET lock_configuration = true".to_string(),
        ] {
            let mut stmt = conn.new_statement().map_err(adbc_error_to_adapter_error)?;
            stmt.set_sql_query(&sql)
                .map_err(adbc_error_to_adapter_error)?;
            stmt.execute_update().map_err(adbc_error_to_adapter_error)?;
        }
        Ok(DuckDbSeedWriter {
            database,
            in_flight: Mutex::new(0),
            freed: Condvar::new(),
        })
    }

    DUCKDB_SEED_WRITER
        .get_or_init(init)
        .as_ref()
        .map_err(Clone::clone)
}

/// Loads the seed CSV at `path` into the existing table
/// `database.schema.table_name` with a `COPY` run by the target, which reads
/// the file itself. A target that cannot read local files, such as MotherDuck
/// with `motherduck_saas_mode`, needs [`ingest_seed_csv`] instead.
pub(crate) fn copy_seed_csv(
    conn: &mut dyn Connection,
    database: &str,
    schema: &str,
    table_name: &str,
    path: &str,
    delimiter: &str,
) -> AdapterResult<()> {
    let duckdb = AdapterType::DuckDB;
    let mut stmt = conn.new_statement().map_err(adbc_error_to_adapter_error)?;
    stmt.set_sql_query(&format!(
        "COPY {}.{}.{} FROM '{}' (FORMAT CSV, HEADER TRUE, DELIMITER '{}')",
        quote_identifier(database, duckdb),
        quote_identifier(schema, duckdb),
        quote_identifier(table_name, duckdb),
        escape_string_literal(path, duckdb),
        escape_string_literal(delimiter, duckdb),
    ))
    .map_err(adbc_error_to_adapter_error)?;
    stmt.execute_update().map_err(adbc_error_to_adapter_error)?;
    Ok(())
}

/// Loads the seed CSV at `path` into the existing table
/// `database.schema.table_name` on `conn`, the target.
///
/// The CSV is read with DuckDB's CSV reader on a new connection to the shared
/// writer (see [`copy_from_csv_read_sql`]), and each Arrow batch is
/// bulk-ingested into the target in append mode. Every column is read as the
/// target's own type, so DuckDB sniffs dates and timestamps
/// (`31/12/2024`, `12/31/2024 01:45:00 PM`, ...) and casts values as `COPY`
/// does. A TIMESTAMPTZ value without an offset is read in the reader's
/// `TimeZone`, where `COPY` would use the target session's.
pub(crate) fn ingest_seed_csv(
    ctx: &QueryCtx,
    conn: &mut dyn Connection,
    database: &str,
    schema: &str,
    table_name: &str,
    path: &str,
    delimiter: &str,
) -> AdapterResult<()> {
    let mut ingest = conn.new_statement().map_err(adbc_error_to_adapter_error)?;
    if let Some(node_id) = ctx.node_id() {
        ingest
            .set_option(
                OptionStatement::Other(DBT_NODE_ID.to_string()),
                OptionValue::from(node_id.as_str()),
            )
            .map_err(adbc_error_to_adapter_error)?;
    }
    ingest
        .set_option(OptionStatement::TargetCatalog, OptionValue::from(database))
        .map_err(adbc_error_to_adapter_error)?;
    ingest
        .set_option(OptionStatement::TargetDbSchema, OptionValue::from(schema))
        .map_err(adbc_error_to_adapter_error)?;
    ingest
        .set_option(OptionStatement::TargetTable, OptionValue::from(table_name))
        .map_err(adbc_error_to_adapter_error)?;
    // precondition: destination table already exists, likely created by create_csv_table
    ingest
        .set_option(
            OptionStatement::IngestMode,
            OptionValue::from("adbc.ingest.mode.append"),
        )
        .map_err(adbc_error_to_adapter_error)?;
    let target: Schema = conn
        .get_table_schema(Some(database), Some(schema), table_name)
        .map_err(adbc_error_to_adapter_error)?;

    let duckdb = AdapterType::DuckDB;
    let read_error = |e: &dyn std::fmt::Display| {
        AdapterError::new(
            AdapterErrorKind::Io,
            format!("failed to read seed file {path}: {e}"),
        )
    };
    // DuckDB matches the allowed directories against real paths.
    let real_path = stdfs::canonicalize(path).map_err(|e| read_error(&e))?;

    // Every column is read as the target's own type, as `COPY` reads it.
    let columns: Vec<(String, String)> = target
        .fields()
        .iter()
        .map(|field| {
            (
                field.name().to_string(),
                SqlType::from_arrow_type(duckdb, field.data_type()).to_string(duckdb),
            )
        })
        .collect();

    let writer = get_duckdb_seed_writer()?;
    let _permit = writer.acquire();
    let mut reader_conn = writer.new_connection()?;
    let mut read = reader_conn
        .new_statement()
        .map_err(adbc_error_to_adapter_error)?;
    read.set_sql_query(&copy_from_csv_read_sql(
        &real_path.to_string_lossy(),
        delimiter,
        &columns,
    ))
    .map_err(|e| read_error(&e))?;
    for batch in read.execute().map_err(|e| read_error(&e))? {
        ingest
            .bind(batch.map_err(|e| read_error(&e))?)
            .map_err(adbc_error_to_adapter_error)?;
        ingest
            .execute_update()
            .map_err(adbc_error_to_adapter_error)?;
    }
    Ok(())
}

/// Mirrors `MultiFileBindCopy` ([`MultiFileBindCopy`]); it is a C++ internal
/// that neither the C API nor ADBC exposes, so this is a copy of its behavior.
/// If DuckDB changes how `COPY FROM` binds a CSV, a port here will be needed.
///
/// [`ParseCopyOption`]: https://github.com/duckdb/duckdb/blob/08e34c447bae34eaee3723cac61f2878b6bdf787/src/execution/operator/csv_scanner/table_function/csv_multi_file_info.cpp#L20-L26
/// [`FinalizeCopyBind`]: https://github.com/duckdb/duckdb/blob/08e34c447bae34eaee3723cac61f2878b6bdf787/src/execution/operator/csv_scanner/table_function/csv_multi_file_info.cpp#L35-L46
/// [`MultiFileBindCopy`]: https://github.com/duckdb/duckdb/blob/08e34c447bae34eaee3723cac61f2878b6bdf787/src/include/duckdb/common/multi_file/multi_file_function.hpp#L195-L221
fn copy_from_csv_read_sql(path: &str, delimiter: &str, columns: &[(String, String)]) -> String {
    let duckdb = AdapterType::DuckDB;
    let columns = columns
        .iter()
        .map(|(name, data_type)| {
            format!(
                "'{}': '{}'",
                escape_string_literal(name, duckdb),
                escape_string_literal(data_type, duckdb)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT * FROM read_csv('{}', header=true, delim='{}', columns={{{columns}}})",
        escape_string_literal(path, duckdb),
        escape_string_literal(delimiter, duckdb),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect()
    }

    #[test]
    fn copy_from_csv_read_sql_gives_every_column_its_type() {
        let sql = copy_from_csv_read_sql(
            "/seeds/orders.csv",
            ",",
            &columns(&[("id", "VARCHAR"), ("ordered_on", "DATE")]),
        );
        assert_eq!(
            sql,
            "SELECT * FROM read_csv('/seeds/orders.csv', header=true, delim=',', \
             columns={'id': 'VARCHAR', 'ordered_on': 'DATE'})"
        );
    }

    #[test]
    fn copy_from_csv_read_sql_escapes_quotes_in_path_delimiter_and_names() {
        let sql = copy_from_csv_read_sql("/p/it's.csv", "'", &columns(&[("a'b", "VARCHAR")]));
        assert_eq!(
            sql,
            "SELECT * FROM read_csv('/p/it''s.csv', header=true, delim='''', \
             columns={'a''b': 'VARCHAR'})"
        );
    }

    // End-to-end tests of the seed load, against a real in-memory DuckDB with file
    // access disabled (as MotherDuck's `saas_mode` does) as the target.

    use arrow::array::StringArray;
    use dbt_schemas::state::{DbtRuntimeConfig, register_global_runtime_config};

    /// A project directory shared by every test in the process, because the seed
    /// writer's allowed directories are fixed when it is first created.
    fn seeds_dir() -> &'static std::path::Path {
        static PROJECT: OnceLock<tempfile::TempDir> = OnceLock::new();
        PROJECT
            .get_or_init(|| {
                let project = tempfile::tempdir().unwrap();
                std::fs::create_dir(project.path().join("seeds")).unwrap();
                let mut config = DbtRuntimeConfig::default();
                config.inner.project_root = project.path().to_path_buf();
                config.inner.seed_paths = vec!["seeds".to_string()];
                register_global_runtime_config(
                    "test_project".to_string(),
                    std::sync::Arc::new(config),
                );
                project
            })
            .path()
    }

    fn execute(conn: &mut dyn Connection, sql: &str) {
        let mut stmt = conn.new_statement().unwrap();
        stmt.set_sql_query(sql).unwrap();
        stmt.execute_update().unwrap();
    }

    /// The first column of every row of `sql`, as text.
    fn values(conn: &mut dyn Connection, sql: &str) -> Vec<String> {
        let mut stmt = conn.new_statement().unwrap();
        stmt.set_sql_query(sql).unwrap();
        stmt.execute()
            .unwrap()
            .flat_map(|batch| {
                let batch = batch.unwrap();
                let column = batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                column
                    .iter()
                    .map(|v| v.unwrap().to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// A target DuckDB with file access disabled, holding the `ddl` table `raw`, and the
    /// seed `contents` written as `<seeds dir>/<name>`. Loads the seed into `raw`.
    fn load_seed(
        name: &str,
        contents: &str,
        ddl: &str,
        delimiter: &str,
    ) -> (Box<dyn Database>, Box<dyn Connection>, AdapterResult<()>) {
        let path = seeds_dir().join("seeds").join(name);
        std::fs::write(&path, contents).unwrap();

        let mut driver = DriverBuilder::new(Backend::DuckDB, LoadStrategy::SystemThenCdnCache)
            .try_load()
            .unwrap();
        let mut builder = database::Builder::new(Backend::DuckDB);
        builder.with_named_option("path", ":memory:").unwrap();
        let database = builder.build(&mut driver).unwrap();
        let mut conn = database.new_connection().unwrap();
        for sql in [
            "SET enable_external_access = false",
            "SET TimeZone = 'UTC'",
            &format!("CREATE TABLE raw ({ddl})"),
        ] {
            execute(conn.as_mut(), sql);
        }
        let result = ingest_seed_csv(
            &QueryCtx::default(),
            conn.as_mut(),
            "memory",
            "main",
            "raw",
            path.to_str().unwrap(),
            delimiter,
        );
        (database, conn, result)
    }

    /// `COPY` from the seed file fails on this target, while the seed load produces the
    /// rows `COPY` would, including non-ISO DATE/TIMESTAMP formats and the casts to the
    /// target's types.
    #[test]
    fn seed_load_matches_copy_without_file_access() {
        let csv = "d,ts,tz,amount,flag,label\n\
                   31/12/2024,31/12/2024 13:45:00,2024-12-31 13:45:00+00,1.005,yes,NULL\n\
                   30/11/2024,30/11/2024 01:02:03,2024-11-30T01:02:03Z,2,f,\"a, b\"\n\
                   ,,,,,\n";
        let (_database, mut conn, result) = load_seed(
            "all_types.csv",
            csv,
            "d DATE, ts TIMESTAMP, tz TIMESTAMPTZ, amount DECIMAL(10,2), flag BOOLEAN, label VARCHAR",
            ",",
        );
        result.unwrap();

        let path = seeds_dir().join("seeds/all_types.csv");
        let mut stmt = conn.new_statement().unwrap();
        let err = stmt
            .set_sql_query(&format!(
                "COPY raw FROM '{}' (FORMAT CSV, HEADER TRUE, DELIMITER ',')",
                path.display()
            ))
            .and_then(|_| stmt.execute_update())
            .unwrap_err();
        assert!(err.message.contains("Permission Error"), "{err:?}");

        assert_eq!(
            values(
                conn.as_mut(),
                "SELECT concat_ws('|', d, ts, tz, amount, flag, coalesce(label, '<null>')) \
                 FROM raw ORDER BY d NULLS LAST",
            ),
            [
                "2024-11-30|2024-11-30 01:02:03|2024-11-30 01:02:03+00|2.00|false|a, b",
                "2024-12-31|2024-12-31 13:45:00|2024-12-31 13:45:00+00|1.01|true|NULL",
                "<null>",
            ]
        );
    }

    /// Only empty values are NULL: the text `NULL`/`null` and padding survive, and a
    /// custom delimiter is honored.
    #[test]
    fn seed_load_only_nulls_empty_values_and_honors_the_delimiter() {
        let csv = "id|label\n1|NULL\n2|null\n3|\n4|\"\"\n5|\"a, b\"\n6|  padded  \n";
        let (_database, mut conn, result) =
            load_seed("nulls.csv", csv, "id VARCHAR, label VARCHAR", "|");
        result.unwrap();

        assert_eq!(
            values(
                conn.as_mut(),
                "SELECT id || ':' || coalesce(label, '<null>') FROM raw ORDER BY id",
            ),
            [
                "1:NULL",
                "2:null",
                "3:<null>",
                "4:<null>",
                "5:a, b",
                "6:  padded  "
            ]
        );
    }

    #[test]
    fn seed_load_reports_a_row_that_does_not_parse() {
        let (_database, _conn, result) = load_seed("bad.csv", "d\nnot a date\n", "d DATE", ",");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("failed to read seed file"),
            "{err}"
        );
    }
}
