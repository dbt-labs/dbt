//! Injected tables: tables that a feature injects into the information schema while its flag
//! is on.
//!
//! [`INFO_SCHEMA`] is the catalog of every build. A build can declare more tables. dbt
//! publishes a declared table while its flag is on. It never publishes a table because files
//! for it exist.
//!
//! The feature defines the table ([`InjectedTable`]): its flag, its name, its columns, and
//! the query that gives its rows from the metadata directory. How those rows get there is the
//! feature's own: it writes its files, in its own crate. dbt reads the table one way, as it
//! reads the catalog's tables: a view over the metadata directory, which `dbt show --info`
//! queries and the information schema's writer copies to `<ns>.<name>.parquet`.

use std::path::Path;

use arrow_schema::SchemaRef;
use dbt_flags::{FlagDef, ProjectFlags};

use crate::IndexError;

use super::schema::INFO_SCHEMA;
use super::spec::Ns;
use super::{copy, epoch_views, views, write_parquet};

/// A table that a feature injects into the information schema while its flag is on.
pub trait InjectedTable: Send + Sync {
    /// The flag that turns the table on. While it is off the table is not published and
    /// `dbt show --info` does not know it.
    fn flag(&self) -> &'static FlagDef;

    /// The namespace of the table.
    fn ns(&self) -> Ns;

    /// The name of the table in its namespace: a lowercase identifier.
    fn name(&self) -> &'static str;

    /// The columns of the table. dbt needs them before any row exists: to publish the table
    /// with no rows, and for `dbt show --info <name>` to know its columns.
    fn schema(&self) -> SchemaRef;

    /// The query that gives the rows of the table from the metadata directory: a `SELECT`
    /// over the files that the feature wrote there, or over metadata that dbt has. `None`
    /// while there is nothing to read.
    ///
    /// A column of [`Self::schema`] takes the query's column of the same name, cast to its
    /// type, and is null where the query has none. A column the schema does not have is not
    /// published.
    fn select(&self, metadata_dir: &Path) -> Result<Option<String>, IndexError>;

    /// `dbt_rt.widget_runs`
    fn qualified_name(&self) -> String {
        format!("{}.{}", self.ns().prefix(), self.name())
    }

    /// `dbt_rt.widget_runs.parquet`
    fn file_name(&self) -> String {
        format!("{}.parquet", self.qualified_name())
    }
}

/// Checks that `table` can be published beside the catalog: its name is a lowercase
/// identifier that is not a table or a derived view of the catalog, and it has columns, of
/// types that a view can declare.
pub fn check(table: &dyn InjectedTable) -> Result<(), IndexError> {
    let qualified = table.qualified_name();
    let invalid = |why: &str| IndexError::Other(format!("injected table `{qualified}`: {why}"));
    let name = table.name();
    let identifier = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !identifier {
        return Err(invalid("its name is not a lowercase identifier"));
    }
    if INFO_SCHEMA
        .iter()
        .any(|table| table.qualified_name() == qualified)
        || views::derived_statements()
            .iter()
            .any(|(view, _)| *view == qualified)
    {
        return Err(invalid("the information schema has a table of that name"));
    }
    if table.schema().fields().is_empty() {
        return Err(invalid("it has no columns"));
    }
    empty_select(&table.schema())
        .map(|_| ())
        .map_err(|e| invalid(&e.to_string()))
}

/// `SELECT` of the columns of `schema`, with their types and no rows.
fn empty_select(schema: &SchemaRef) -> Result<String, IndexError> {
    let columns = schema
        .fields()
        .iter()
        .map(|field| {
            let ty = epoch_views::duckdb_type(field.data_type())?;
            Ok(format!("CAST(NULL AS {ty}) AS \"{}\"", field.name()))
        })
        .collect::<Result<Vec<_>, IndexError>>()?;
    Ok(format!("SELECT {} WHERE FALSE", columns.join(", ")))
}

/// The view of `table` over the metadata directory: its query in its declared columns, or
/// its columns and no rows while there is nothing to read.
fn view_statement(table: &dyn InjectedTable, metadata_dir: &Path) -> Result<String, IndexError> {
    let schema = table.schema();
    let empty = empty_select(&schema)?;
    let body = match table.select(metadata_dir)? {
        None => empty,
        Some(select) => {
            let columns = schema
                .fields()
                .iter()
                .map(|field| {
                    let ty = epoch_views::duckdb_type(field.data_type())?;
                    let name = field.name();
                    Ok(format!("CAST(\"{name}\" AS {ty}) AS \"{name}\""))
                })
                .collect::<Result<Vec<_>, IndexError>>()?;
            // The declared columns first, by name, so that a column the query lacks is
            // there, with nulls.
            format!(
                "SELECT {} FROM ({empty} UNION ALL BY NAME SELECT * FROM ({select}))",
                columns.join(", ")
            )
        }
    };
    Ok(format!(
        "CREATE OR REPLACE VIEW {} AS {body}",
        table.qualified_name()
    ))
}

/// One declared injected table, with whether its flag is on for this invocation.
pub struct ResolvedInjectedTable<'a> {
    pub table: &'a dyn InjectedTable,
    pub on: bool,
}

impl ResolvedInjectedTable<'_> {
    /// Whether users can name the table now: its flag is on, and it is not a table of
    /// `dbt_internal`.
    pub fn is_public(&self) -> bool {
        self.on && self.table.ns() != Ns::DbtInternal
    }
}

/// Each of `declared`, with whether its flag is on. `project` gives the root project's
/// `flags:`, as for the flag of a kind of node.
pub fn resolve<'a>(
    declared: &'a [&'a dyn InjectedTable],
    project: &dyn ProjectFlags,
) -> Vec<ResolvedInjectedTable<'a>> {
    declared
        .iter()
        .map(|table| ResolvedInjectedTable {
            table: *table,
            on: table.flag().resolve_with_project(project).value,
        })
        .collect()
}

/// A view for each injected table that is on, over the metadata directory. A table that is
/// off gets none, so its name does not bind.
pub fn views(
    tables: &[ResolvedInjectedTable<'_>],
    metadata_dir: &Path,
) -> Result<Vec<String>, IndexError> {
    tables
        .iter()
        .filter(|resolved| resolved.on)
        .map(|resolved| view_statement(resolved.table, metadata_dir))
        .collect()
}

/// Publishes the injected tables in `dir`, the version's directory of an information schema
/// that was just written: each table that is on is its view, copied to its file, with a view
/// of that file in `views.sql`. The file of a table that is off is removed.
///
/// A directory without `views.sql` is not a complete information schema, and gets nothing.
/// Without a DuckDB driver the views can't be read: a file that is there stays as it is, and
/// a table without one is published with no rows.
///
/// Returns the number of tables published.
pub fn publish(
    tables: &[ResolvedInjectedTable<'_>],
    metadata_dir: &Path,
    dir: &Path,
) -> Result<usize, IndexError> {
    let views_sql = dir.join(views::VIEWS_SQL_FILE_NAME);
    let Ok(mut sql) = std::fs::read_to_string(&views_sql) else {
        return Ok(0);
    };
    let before = sql.len();
    for resolved in tables.iter().filter(|resolved| !resolved.on) {
        match std::fs::remove_file(dir.join(resolved.table.file_name())) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let on: Vec<&dyn InjectedTable> = tables
        .iter()
        .filter(|resolved| resolved.on)
        .map(|resolved| resolved.table)
        .collect();
    if on.is_empty() {
        return Ok(0);
    }

    let mut db = copy::open_copy_db().ok();
    if let Some(db) = &mut db {
        for ns in Ns::ALL {
            db.execute_update(&format!("CREATE SCHEMA IF NOT EXISTS {}", ns.prefix()))?;
        }
    }
    for table in &on {
        let file = std::path::absolute(dir.join(table.file_name()))?;
        match &mut db {
            Some(db) => {
                db.execute_update(&view_statement(*table, metadata_dir)?)?;
                copy::copy_view(db, &table.qualified_name(), &file)?;
            }
            None if file.exists() => {}
            None => write_parquet(&file, &table.schema(), &[])?,
        }
        if let Some(statement) = views::statement_for_file(&table.file_name())
            && !sql.contains(&statement)
        {
            sql.push_str(&format!("{statement};\n"));
        }
    }
    if sql.len() != before {
        std::fs::write(&views_sql, sql)?;
    }
    Ok(on.len())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use arrow_array::builder::{Int64Builder, ListBuilder, StringBuilder, StructBuilder};
    use arrow_array::{Array, ArrayRef, BooleanArray, Int64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Fields, Schema};
    use dbt_flags::{NoProjectFlags, Remote, Stage};

    use super::*;
    use crate::db::Db;
    use crate::epoch_layers::{append_epoch, select_epochs};

    static WIDGET_RUNS_FLAG: FlagDef = FlagDef {
        name: "test_widget_runs",
        description: "The runs of widgets, for testing injected tables.",
        stage: Stage::Beta,
        default: false,
        remote: Remote::Never,
        aliases: &[],
        legacy_env: &[],
        tracking: None,
        remove_by: None,
        docs_url: None,
    };

    /// Where the tests' feature writes the runs of widgets, under the metadata directory.
    fn runs_dir(metadata_dir: &Path) -> PathBuf {
        metadata_dir.join("run").join("widget_runs")
    }

    /// A table of the runs of widgets, as a feature defines one: its columns, and the query
    /// over the files that the feature writes.
    struct WidgetRuns {
        ns: Ns,
        name: &'static str,
        fields: Vec<Field>,
        /// A query that reads no file of the feature's, if the table has one.
        derived: Option<&'static str>,
    }

    impl WidgetRuns {
        fn named(ns: Ns, name: &'static str) -> Self {
            Self {
                ns,
                name,
                fields: vec![
                    Field::new("widget", DataType::Utf8, true),
                    Field::new("rows", DataType::Int64, true),
                    Field::new("passed", DataType::Boolean, true),
                ],
                derived: None,
            }
        }
    }

    impl InjectedTable for WidgetRuns {
        fn flag(&self) -> &'static FlagDef {
            &WIDGET_RUNS_FLAG
        }
        fn ns(&self) -> Ns {
            self.ns
        }
        fn name(&self) -> &'static str {
            self.name
        }
        fn schema(&self) -> SchemaRef {
            Arc::new(Schema::new(self.fields.clone()))
        }
        fn select(&self, metadata_dir: &Path) -> Result<Option<String>, IndexError> {
            match self.derived {
                Some(select) => Ok(Some(select.to_string())),
                None => select_epochs(&runs_dir(metadata_dir)),
            }
        }
    }

    fn widget_runs() -> WidgetRuns {
        WidgetRuns::named(Ns::DbtRt, "widget_runs")
    }

    fn resolved(table: &WidgetRuns, on: bool) -> Vec<ResolvedInjectedTable<'_>> {
        vec![ResolvedInjectedTable { table, on }]
    }

    /// One run, as the feature writes it. An older run has no `passed`, and every run has a
    /// column that the table does not have.
    fn run(widget: &str, rows: i64, passed: Option<bool>) -> RecordBatch {
        let mut fields = vec![
            Field::new("widget", DataType::Utf8, true),
            Field::new("rows", DataType::Int64, true),
            Field::new("note", DataType::Utf8, true),
        ];
        let mut columns: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(vec![widget])),
            Arc::new(Int64Array::from(vec![rows])),
            Arc::new(StringArray::from(vec!["not a column of the table"])),
        ];
        if let Some(passed) = passed {
            fields.push(Field::new("passed", DataType::Boolean, true));
            columns.push(Arc::new(BooleanArray::from(vec![passed])));
        }
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
    }

    /// A version's directory with a `views.sql`, as a written information schema has.
    fn written_info_schema() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        views::write_views_sql(dir.path()).unwrap();
        dir
    }

    fn views_sql(dir: &Path) -> String {
        std::fs::read_to_string(dir.join(views::VIEWS_SQL_FILE_NAME)).unwrap()
    }

    /// A database with the views of `tables` over `metadata_dir`, as `dbt show --info` has.
    fn live(tables: &[ResolvedInjectedTable<'_>], metadata_dir: &Path) -> Db {
        let mut db = Db::open_memory().unwrap();
        db.execute_update("CREATE SCHEMA IF NOT EXISTS dbt_rt")
            .unwrap();
        for statement in views(tables, metadata_dir).unwrap() {
            db.execute_update(&statement).unwrap();
        }
        db
    }

    fn column_names(db: &mut Db, from: &str) -> String {
        db.query_count(&format!(
            "SELECT string_agg(column_name, ',') FROM (DESCRIBE SELECT * FROM {from})"
        ))
    }

    /// The statement for a file is the one `views.sql` has for the table in it.
    #[test]
    fn the_statement_for_a_file_is_the_tables_view() {
        for table in INFO_SCHEMA {
            assert_eq!(
                views::statement_for_file(&table.file_name()),
                Some(views::base_view_statement(table, None))
            );
        }
        assert_eq!(
            views::statement_for_file(&widget_runs().file_name()).unwrap(),
            "CREATE OR REPLACE VIEW dbt_rt.widget_runs AS SELECT * FROM \
             read_parquet('dbt_rt.widget_runs.parquet')"
        );
        assert_eq!(views::statement_for_file("views.sql"), None);
        assert_eq!(views::statement_for_file("other.widget_runs.parquet"), None);
    }

    #[test]
    fn the_flag_decides_whether_an_injected_table_is_on() {
        // Off by default, and nothing here turns it on.
        let table = widget_runs();
        let declared: [&dyn InjectedTable; 1] = [&table];
        let resolved = resolve(&declared, &NoProjectFlags);
        assert_eq!(resolved.len(), 1);
        assert!(!resolved[0].on);
        assert!(!resolved[0].is_public());
    }

    #[test]
    fn an_injected_table_is_checked_against_the_catalog() {
        assert!(check(&widget_runs()).is_ok());
        let error = |table: WidgetRuns| check(&table).unwrap_err().to_string();
        assert!(
            error(WidgetRuns::named(Ns::DbtRt, "run_results")).contains("has a table of that name"),
            "a table of the catalog"
        );
        assert!(
            error(WidgetRuns::named(Ns::DbtRt, "run_results_latest"))
                .contains("has a table of that name"),
            "a derived view"
        );
        assert!(
            error(WidgetRuns::named(Ns::DbtRt, "Widget Runs")).contains("lowercase identifier")
        );
        let mut no_columns = widget_runs();
        no_columns.fields.clear();
        assert!(error(no_columns).contains("it has no columns"));
        let mut odd_type = widget_runs();
        odd_type.fields = vec![Field::new("size", DataType::Float16, true)];
        assert!(error(odd_type).contains("no DuckDB type"));
    }

    /// An injected table that is on is published as a table of the catalog is: a file with
    /// its columns, and a view of the file in `views.sql`, once.
    #[test]
    fn an_injected_table_that_is_on_is_published() {
        let metadata = tempfile::tempdir().unwrap();
        append_epoch(&runs_dir(metadata.path()), &[run("a", 3, None)]).unwrap();
        append_epoch(&runs_dir(metadata.path()), &[run("b", 1, Some(false))]).unwrap();
        let dir = written_info_schema();
        let table = widget_runs();
        let on = resolved(&table, true);
        assert_eq!(publish(&on, metadata.path(), dir.path()).unwrap(), 1);
        assert_eq!(publish(&on, metadata.path(), dir.path()).unwrap(), 1);

        let statement = "CREATE OR REPLACE VIEW dbt_rt.widget_runs AS SELECT * FROM \
                         read_parquet('dbt_rt.widget_runs.parquet');";
        assert_eq!(views_sql(dir.path()).matches(statement).count(), 1);

        let mut db = Db::open_memory().unwrap();
        let file = dir.path().join("dbt_rt.widget_runs.parquet");
        let from = format!("read_parquet('{}')", file.display());
        assert_eq!(db.query_count(&format!("SELECT count(*) FROM {from}")), "2");
        // The table's columns only, with nulls where an older run has none.
        assert_eq!(column_names(&mut db, &from), "widget,rows,passed");
        assert_eq!(
            db.query_count(&format!("SELECT count(*) FROM {from} WHERE passed IS NULL")),
            "1"
        );
    }

    /// With nothing to read yet, the table is published with its columns and no rows.
    #[test]
    fn an_injected_table_without_rows_is_published_empty() {
        let metadata = tempfile::tempdir().unwrap();
        let dir = written_info_schema();
        let table = widget_runs();
        assert_eq!(
            publish(&resolved(&table, true), metadata.path(), dir.path()).unwrap(),
            1
        );
        let mut db = Db::open_memory().unwrap();
        let file = dir.path().join("dbt_rt.widget_runs.parquet");
        let from = format!("read_parquet('{}')", file.display());
        assert_eq!(db.query_count(&format!("SELECT count(*) FROM {from}")), "0");
        assert_eq!(column_names(&mut db, &from), "widget,rows,passed");
    }

    /// An injected table that is off is not published, though the feature's files are
    /// there, and a file from an invocation that had it on is removed.
    #[test]
    fn an_injected_table_that_is_off_is_not_published() {
        let metadata = tempfile::tempdir().unwrap();
        append_epoch(&runs_dir(metadata.path()), &[run("a", 3, Some(true))]).unwrap();
        let dir = written_info_schema();
        let file = dir.path().join("dbt_rt.widget_runs.parquet");
        let table = widget_runs();

        let off = resolved(&table, false);
        assert_eq!(publish(&off, metadata.path(), dir.path()).unwrap(), 0);
        assert!(!file.exists());
        assert!(!views_sql(dir.path()).contains("widget_runs"));

        publish(&resolved(&table, true), metadata.path(), dir.path()).unwrap();
        assert!(file.exists());
        // The next write of the information schema writes `views.sql` anew.
        views::write_views_sql(dir.path()).unwrap();
        assert_eq!(publish(&off, metadata.path(), dir.path()).unwrap(), 0);
        assert!(!file.exists());
        assert!(!views_sql(dir.path()).contains("widget_runs"));
    }

    /// A directory that is not a written information schema gets nothing.
    #[test]
    fn nothing_is_published_without_an_information_schema() {
        let metadata = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let table = widget_runs();
        assert_eq!(
            publish(&resolved(&table, true), metadata.path(), dir.path()).unwrap(),
            0
        );
        assert!(!dir.path().join("dbt_rt.widget_runs.parquet").exists());
    }

    /// A value that is not of the column's type is an error, not a table of another shape.
    #[test]
    fn a_value_of_another_type_is_an_error() {
        let metadata = tempfile::tempdir().unwrap();
        let schema = Schema::new(vec![Field::new("rows", DataType::Utf8, true)]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![Arc::new(StringArray::from(vec!["three"]))],
        )
        .unwrap();
        append_epoch(&runs_dir(metadata.path()), &[batch]).unwrap();
        let dir = written_info_schema();
        let table = widget_runs();
        let error = publish(&resolved(&table, true), metadata.path(), dir.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("three"), "{error}");
    }

    /// The views that `dbt show --info` reads: the table's columns over the feature's rows
    /// while it is on, its columns and no rows while there is nothing to read, and no view
    /// while it is off.
    #[test]
    fn injected_tables_are_queryable_live_while_on() {
        let metadata = tempfile::tempdir().unwrap();
        let table = widget_runs();
        assert!(
            views(&resolved(&table, false), metadata.path())
                .unwrap()
                .is_empty()
        );

        let on = resolved(&table, true);
        let mut db = live(&on, metadata.path());
        assert_eq!(
            db.query_count("SELECT count(*) FROM dbt_rt.widget_runs"),
            "0"
        );
        assert_eq!(
            column_names(&mut db, "dbt_rt.widget_runs"),
            "widget,rows,passed"
        );

        append_epoch(&runs_dir(metadata.path()), &[run("a", 3, None)]).unwrap();
        append_epoch(&runs_dir(metadata.path()), &[run("b", 1, Some(false))]).unwrap();
        let mut db = live(&on, metadata.path());
        assert_eq!(
            db.query_count("SELECT count(*) FROM dbt_rt.widget_runs"),
            "2"
        );
        assert_eq!(
            column_names(&mut db, "dbt_rt.widget_runs"),
            "widget,rows,passed"
        );
        assert_eq!(
            db.query_count("SELECT count(*) FROM dbt_rt.widget_runs WHERE passed IS NULL"),
            "1"
        );
    }

    /// A table needs no files of its own: its query can be over anything the database can
    /// read. A column that the query lacks is null.
    #[test]
    fn a_table_can_be_a_query_with_no_files_of_its_own() {
        let metadata = tempfile::tempdir().unwrap();
        let table = WidgetRuns {
            derived: Some("SELECT 'a' AS widget, 3 AS rows UNION ALL SELECT 'b', 1"),
            ..widget_runs()
        };
        let mut db = live(&resolved(&table, true), metadata.path());
        assert_eq!(
            db.query_count("SELECT sum(rows) FROM dbt_rt.widget_runs"),
            "4"
        );
        assert_eq!(
            db.query_count("SELECT count(*) FROM dbt_rt.widget_runs WHERE passed IS NULL"),
            "2"
        );
    }

    /// The parts of a widget, as a list of structs. `with_size` adds the field `size`.
    fn parts(widget: &str, with_size: bool) -> RecordBatch {
        let mut fields = vec![Field::new("name", DataType::Utf8, true)];
        if with_size {
            fields.push(Field::new("size", DataType::Int64, true));
        }
        let fields = Fields::from(fields);
        let mut list = ListBuilder::new(StructBuilder::from_fields(fields, 1));
        let part = list.values();
        part.field_builder::<StringBuilder>(0)
            .unwrap()
            .append_value("bolt");
        if with_size {
            part.field_builder::<Int64Builder>(1)
                .unwrap()
                .append_value(3);
        }
        part.append(true);
        list.append(true);
        let list = list.finish();
        let schema = Schema::new(vec![
            Field::new("widget", DataType::Utf8, true),
            Field::new("parts", list.data_type().clone(), true),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![Arc::new(StringArray::from(vec![widget])), Arc::new(list)],
        )
        .unwrap()
    }

    /// A column can be a list of structs, and a field added to the struct later is null in
    /// the rows of the files that lack it.
    #[test]
    fn a_field_added_to_a_list_of_structs_is_null_in_older_rows() {
        let metadata = tempfile::tempdir().unwrap();
        append_epoch(&runs_dir(metadata.path()), &[parts("a", false)]).unwrap();
        append_epoch(&runs_dir(metadata.path()), &[parts("b", true)]).unwrap();
        let part = DataType::Struct(Fields::from(vec![
            Field::new("name", DataType::Utf8, true),
            Field::new("size", DataType::Int64, true),
        ]));
        let table = WidgetRuns {
            fields: vec![
                Field::new("widget", DataType::Utf8, true),
                Field::new(
                    "parts",
                    DataType::List(Arc::new(Field::new("item", part, true))),
                    true,
                ),
            ],
            ..widget_runs()
        };
        assert!(check(&table).is_ok());
        let mut db = live(&resolved(&table, true), metadata.path());
        assert_eq!(
            db.query_count(
                "SELECT count(*) FROM dbt_rt.widget_runs, unnest(parts) AS t(part) \
                 WHERE part.name = 'bolt'"
            ),
            "2"
        );
        assert_eq!(
            db.query_count(
                "SELECT count(*) FROM dbt_rt.widget_runs, unnest(parts) AS t(part) \
                 WHERE part.size IS NULL"
            ),
            "1"
        );
    }
}
