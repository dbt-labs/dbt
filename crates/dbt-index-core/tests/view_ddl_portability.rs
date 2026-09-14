//! Pins the DDL-portability invariant from fs#13788: the generated view DDL
//! must behave identically on both DuckDB builds we ship against — the
//! extended driver (ADBC path: docs, MCP, wizard) and the vanilla driver
//! (the checks adapter). The whole shared-statements design rests on this
//! seam, so it is asserted, not assumed.
//!
//! The test writes an empty parquet file per canonical base table (real
//! schemas, via the same `IndexWriter` path production uses), runs the one
//! shared registration against each driver, and requires that *everything*
//! registers — every base view binds, every analytical view (including the
//! `QUALIFY`-using ones) compiles — with identical registration outcomes
//! across the two drivers.
//!
//! The information schema's own document gets the same treatment. It has a
//! second driver-portability question of its own: `dbt_internal.resources`
//! unions nine relations with `UNION ALL BY NAME`, which the vanilla driver
//! has to support as readily as the extended one.
//!
//! Drivers load via `SystemThenCdnCache`; if one is unavailable in this
//! environment the test skips that driver rather than failing, matching the
//! gating used by the info-schema COPY tests.

use std::path::Path;
use std::sync::Mutex;

use arrow_array::RecordBatch;
use dbt_adbc::Backend as DriverKind;
use dbt_index_core::backend::{
    Backend, BackendError, register_index_views, register_info_schema_views,
};
use dbt_index_core::db::Db;
use dbt_index_core::info_schema::project::out_schema;
use dbt_index_core::info_schema::schema::INFO_SCHEMA;
use dbt_index_core::parquet::IndexWriter;
use dbt_index_core::view_defs;

/// Executor over a [`Db`] opened on a caller-chosen driver.
struct DbExecutor {
    db: Mutex<Db>,
}

impl DbExecutor {
    fn open(kind: DriverKind) -> Option<Self> {
        Db::open_memory_on(kind)
            .ok()
            .map(|db| Self { db: Mutex::new(db) })
    }
}

impl Backend for DbExecutor {
    fn execute(&self, sql: &str) -> Result<(), BackendError> {
        self.db
            .lock()
            .unwrap()
            .execute_update(sql)
            .map(|_| ())
            .map_err(|e| BackendError::Query(e.to_string()))
    }

    fn query_arrow(&self, sql: &str) -> Result<Vec<RecordBatch>, BackendError> {
        self.db
            .lock()
            .unwrap()
            .execute_query(sql)
            .map_err(|e| BackendError::Query(e.to_string()))
    }
}

/// Register the full surface on one driver; return the sorted registered
/// names, or None if the driver is unavailable here.
fn register_on(kind: DriverKind, dir: &Path) -> Option<Vec<String>> {
    let exec = DbExecutor::open(kind)?;
    let report = register_index_views(&exec, dir).unwrap();

    assert!(
        report.skipped.is_empty(),
        "{kind:?}: nothing should be skipped with a complete index, got {:?}",
        report.skipped
    );

    // Smoke-query derived views through the same connection: creation alone
    // does not prove a view is usable.
    exec.query_arrow("SELECT * FROM dbt.nodes_enriched")
        .unwrap_or_else(|e| panic!("dbt.nodes_enriched unusable on {kind:?}: {e}"));
    exec.query_arrow("SELECT * FROM dbt_rt.run_results_latest")
        .unwrap_or_else(|e| panic!("dbt_rt.run_results_latest unusable on {kind:?}: {e}"));

    let mut registered = report.registered;
    registered.sort_unstable();
    Some(registered)
}

#[test]
fn generated_ddl_registers_identically_on_both_drivers() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut writer = IndexWriter::new(tmp.path()).unwrap();
    writer.ensure_dbt_tables().unwrap();
    writer.ensure_rt_tables().unwrap();

    let expected_count = view_defs::base_tables().count() + view_defs::DERIVED_VIEWS.len();

    let mut outcomes = Vec::new();
    for kind in [DriverKind::DuckDBExtended, DriverKind::DuckDB] {
        match register_on(kind, tmp.path()) {
            Some(registered) => {
                assert_eq!(
                    registered.len(),
                    expected_count,
                    "{kind:?}: every base and derived view must register"
                );
                outcomes.push((kind, registered));
            }
            None => eprintln!("skipping {kind:?}: driver unavailable in this environment"),
        }
    }

    if let [(_, a), (_, b)] = outcomes.as_slice() {
        assert_eq!(a, b, "registration outcomes differ between drivers");
    }
}

/// Write an empty parquet per information-schema table, with that table's real
/// output schema — the same shape `write_info_schema` produces for a project
/// with nothing in it, which it always writes so `views.sql` resolves.
fn write_empty_info_schema(dir: &Path) {
    use parquet::arrow::ArrowWriter;

    for spec in INFO_SCHEMA {
        let schema = out_schema(spec).expect("out_schema");
        let file = std::fs::File::create(dir.join(spec.file_name())).expect("create");
        ArrowWriter::try_new(file, schema, None)
            .expect("writer")
            .close()
            .expect("close");
    }
}

/// Register the information schema on one driver; return the sorted registered
/// names, or None if the driver is unavailable here.
fn register_info_schema_on(kind: DriverKind, dir: &Path) -> Option<Vec<String>> {
    let exec = DbExecutor::open(kind)?;
    let report = register_info_schema_views(&exec, dir).unwrap();

    assert!(
        report.skipped.is_empty(),
        "{kind:?}: nothing should be skipped with a complete information schema, got {:?}",
        report.skipped
    );

    // Creation alone does not prove a view is usable, and the union is the one
    // statement here whose syntax could plausibly differ between drivers.
    exec.query_arrow("SELECT * FROM dbt_internal.resources")
        .unwrap_or_else(|e| panic!("dbt_internal.resources unusable on {kind:?}: {e}"));
    exec.query_arrow("SELECT * FROM dbt_rt.run_results_latest")
        .unwrap_or_else(|e| panic!("dbt_rt.run_results_latest unusable on {kind:?}: {e}"));
    // Reading one column proves the union projects rather than just binds:
    // `UNION ALL BY NAME` has to have lined the branches up by name.
    exec.query_arrow("SELECT unique_id, resource_type FROM dbt_internal.resources")
        .unwrap_or_else(|e| panic!("dbt_internal.resources projection failed on {kind:?}: {e}"));

    let mut registered = report.registered;
    registered.sort_unstable();
    Some(registered)
}

#[test]
fn info_schema_ddl_registers_identically_on_both_drivers() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_empty_info_schema(tmp.path());

    // Every table, plus `dbt_rt.run_results_latest` and
    // `dbt_internal.resources`.
    let expected_count = INFO_SCHEMA.len() + 2;

    let mut outcomes = Vec::new();
    for kind in [DriverKind::DuckDBExtended, DriverKind::DuckDB] {
        match register_info_schema_on(kind, tmp.path()) {
            Some(registered) => {
                assert_eq!(
                    registered.len(),
                    expected_count,
                    "{kind:?}: every table and derived view must register"
                );
                outcomes.push((kind, registered));
            }
            None => eprintln!("skipping {kind:?}: driver unavailable in this environment"),
        }
    }

    if let [(_, a), (_, b)] = outcomes.as_slice() {
        assert_eq!(a, b, "registration outcomes differ between drivers");
    }
}
