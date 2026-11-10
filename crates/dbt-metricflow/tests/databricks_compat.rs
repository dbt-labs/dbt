//! Databricks compatibility tests for the MetricFlow semantic query compiler.
//!
//! Two modes:
//!
//! - **Replay** (default): loads pre-recorded results from
//!   `tests/data/databricks_recordings.parquet`.  No Databricks connection needed.
//!   ```sh
//!   cargo test -p dbt-metricflow --test databricks_compat
//!   ```
//!
//! - **Record** (`#[ignore]`, requires `fusion_tests/databricks` profile):
//!   executes compiled SQL on a live Databricks SQL warehouse and writes results
//!   to the parquet tome so replay tests stay up to date.
//!   ```sh
//!   cargo test -p dbt-metricflow --test databricks_compat -- --ignored --nocapture
//!   ```

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use arrow_array::RecordBatch;
use dbt_adbc::{Backend, LoadStrategy, connection, driver};
use dbt_auth::{AdapterConfig, auth_for_backend};
use dbt_metricflow::Dialect;
use dbt_profile::ProfileEnvironment;

const PROFILE: &str = "fusion_tests";
const TARGET: &str = "databricks";

// ═══════════════════════════════════════════════════════════════════════════
// Tome path
// ═══════════════════════════════════════════════════════════════════════════

fn tome_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("databricks_recordings.parquet")
}

// ═══════════════════════════════════════════════════════════════════════════
// Databricks connection wrapper
// ═══════════════════════════════════════════════════════════════════════════

struct DatabricksConn {
    _drv: Box<dyn dbt_adbc::Driver>,
    _database: Box<dyn dbt_adbc::Database>,
    conn: Box<dyn dbt_adbc::Connection>,
    schema: Option<(String, String)>,
}

impl Drop for DatabricksConn {
    fn drop(&mut self) {
        if let Some((catalog, schema)) = self.schema.take() {
            eprintln!("databricks_compat: dropping schema {catalog}.{schema}");
            let _ = self
                .try_execute_update(&format!("DROP SCHEMA IF EXISTS {catalog}.{schema} CASCADE"));
        }
    }
}

impl DatabricksConn {
    fn connect() -> Self {
        let home = dirs::home_dir().expect("home dir exists");
        let profile_path = home.join(".dbt").join("profiles.yml");
        let penv = ProfileEnvironment::new(BTreeMap::new());
        let resolved = dbt_profile::resolve_with_env(&penv, &profile_path, PROFILE, Some(TARGET))
            .expect("failed to resolve fusion_tests/databricks profile");

        eprintln!(
            "databricks_compat: connecting to {} ({}) target={}",
            resolved.adapter_type, resolved.profile_name, resolved.target_name,
        );

        assert_eq!(
            resolved.adapter_type, "databricks",
            "profile must be databricks, got: {}",
            resolved.adapter_type
        );

        let backend = Backend::Databricks;
        let adapter_config = AdapterConfig::new(resolved.credentials);
        let auth = auth_for_backend(backend);
        let db_builder = auth
            .configure(&adapter_config)
            .expect("auth configuration failed");

        let mut drv = driver::Builder::new(backend, LoadStrategy::CdnCache)
            .try_load()
            .expect("failed to load Databricks driver");

        let mut database = db_builder
            .build(&mut drv)
            .expect("failed to open Databricks database");

        let conn = connection::Builder::default()
            .build(&mut database)
            .expect("failed to create Databricks connection");

        DatabricksConn {
            _drv: drv,
            _database: database,
            conn,
            schema: None,
        }
    }

    fn set_schema(&mut self, catalog: String, schema: String) {
        self.schema = Some((catalog, schema));
    }

    fn try_execute_update(&mut self, sql: &str) -> Result<Option<i64>, String> {
        let mut stmt = self.conn.new_statement().map_err(|e| e.to_string())?;
        stmt.set_sql_query(sql).map_err(|e| e.to_string())?;
        stmt.execute_update().map_err(|e| e.to_string())
    }

    fn execute_update(&mut self, sql: &str) {
        let mut stmt = self
            .conn
            .new_statement()
            .expect("failed to create statement");
        stmt.set_sql_query(sql).expect("failed to set SQL");
        stmt.execute_update().unwrap_or_else(|e| {
            panic!("DDL/DML failed: {e}\n  SQL: {}", &sql[..sql.len().min(200)])
        });
    }

    fn execute_query(&mut self, sql: &str) -> Vec<RecordBatch> {
        let mut stmt = self
            .conn
            .new_statement()
            .expect("failed to create statement");
        stmt.set_sql_query(sql).expect("failed to set SQL");
        let reader = stmt
            .execute()
            .unwrap_or_else(|e| panic!("query failed: {e}\n  SQL: {}", &sql[..sql.len().min(200)]));
        reader
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("failed to read results")
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Record test — live Databricks, writes parquet tome
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires fusion_tests/databricks profile in ~/.dbt/profiles.yml"]
fn databricks_compat_record() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let catalog = "e2e";
    let schema = format!("mf_test_{ts}");
    eprintln!("databricks_compat [record]: using schema {catalog}.{schema}");

    let mut db = DatabricksConn::connect();
    db.execute_update(&format!("CREATE SCHEMA IF NOT EXISTS {catalog}.{schema}"));
    db.set_schema(catalog.to_string(), schema.clone());
    eprintln!("databricks_compat [record]: schema {catalog}.{schema} created");

    let qualified_schema = format!("{catalog}.{schema}");
    for stmt in common::databricks_data_ddl(&qualified_schema) {
        db.execute_update(&stmt);
    }
    eprintln!("databricks_compat [record]: test data loaded");

    let mut store = common::setup_metric_store(catalog, &schema);
    let mut tome = common::TomeWriter::new();

    common::run_scorecard(
        &mut store,
        Dialect::Databricks,
        "Databricks",
        &mut |name, sql| match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            db.execute_query(sql)
        })) {
            Ok(batches) => {
                tome.insert(name, &batches);
                Ok(Some(batches))
            }
            Err(_) => Err(format!("execution panicked\n  SQL: {sql}")),
        },
    );

    tome.write(&tome_path());
    eprintln!(
        "databricks_compat [record]: tome written to {:?}",
        tome_path()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Replay test — reads parquet tome, no Databricks needed
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn databricks_compat_replay() {
    let path = tome_path();
    if !path.exists() {
        eprintln!(
            "databricks_compat [replay]: no tome at {path:?} — \
             run databricks_compat_record to generate it"
        );
        return;
    }

    let recordings = common::read_tome(&path);
    let mut store = common::setup_metric_store("RECORDED_CATALOG", "RECORDED_SCHEMA");

    common::run_scorecard(
        &mut store,
        Dialect::Databricks,
        "Databricks",
        &mut |name, _sql| Ok(recordings.get(name).cloned()),
    );
}
