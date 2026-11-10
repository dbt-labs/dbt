//! Redshift compatibility tests for the MetricFlow semantic query compiler.
//!
//! Two modes:
//!
//! - **Replay** (default): loads pre-recorded results from
//!   `tests/data/redshift_recordings.parquet`.  No Redshift connection needed.
//!   ```sh
//!   cargo test -p dbt-metricflow --test redshift_compat
//!   ```
//!
//! - **Record** (`#[ignore]`, requires `fusion_tests/redshift` profile):
//!   executes compiled SQL on a live Redshift cluster and writes results to
//!   the parquet tome so replay tests stay up to date.
//!   ```sh
//!   cargo test -p dbt-metricflow --test redshift_compat -- --ignored --nocapture
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
const TARGET: &str = "redshift";

// ═══════════════════════════════════════════════════════════════════════════
// Tome path
// ═══════════════════════════════════════════════════════════════════════════

fn tome_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("redshift_recordings.parquet")
}

// ═══════════════════════════════════════════════════════════════════════════
// Redshift connection wrapper
// ═══════════════════════════════════════════════════════════════════════════

struct RedshiftConn {
    _drv: Box<dyn dbt_adbc::Driver>,
    _database: Box<dyn dbt_adbc::Database>,
    conn: Box<dyn dbt_adbc::Connection>,
    schema: Option<String>,
}

impl Drop for RedshiftConn {
    fn drop(&mut self) {
        if let Some(schema) = self.schema.take() {
            eprintln!("redshift_compat: dropping schema {schema}");
            let _ = self.try_execute_update(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"));
        }
    }
}

impl RedshiftConn {
    fn connect() -> Self {
        let home = dirs::home_dir().expect("home dir exists");
        let profile_path = home.join(".dbt").join("profiles.yml");
        let penv = ProfileEnvironment::new(BTreeMap::new());
        let resolved = dbt_profile::resolve_with_env(&penv, &profile_path, PROFILE, Some(TARGET))
            .expect("failed to resolve fusion_tests/redshift profile");

        eprintln!(
            "redshift_compat: connecting to {} ({}) target={}",
            resolved.adapter_type, resolved.profile_name, resolved.target_name,
        );

        assert_eq!(
            resolved.adapter_type, "redshift",
            "profile must be redshift, got: {}",
            resolved.adapter_type
        );

        let backend = Backend::Redshift;
        let adapter_config = AdapterConfig::new(resolved.credentials);
        let auth = auth_for_backend(backend);
        let db_builder = auth
            .configure(&adapter_config)
            .expect("auth configuration failed");

        let mut drv = driver::Builder::new(backend, LoadStrategy::CdnCache)
            .try_load()
            .expect("failed to load Redshift driver");

        let mut database = db_builder
            .build(&mut drv)
            .expect("failed to open Redshift database");

        let conn = connection::Builder::default()
            .build(&mut database)
            .expect("failed to create Redshift connection");

        RedshiftConn {
            _drv: drv,
            _database: database,
            conn,
            schema: None,
        }
    }

    fn set_schema(&mut self, schema: String) {
        self.schema = Some(schema);
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
// Record test — live Redshift, writes parquet tome
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires fusion_tests/redshift profile in ~/.dbt/profiles.yml"]
fn redshift_compat_record() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let schema = format!("mf_test_{ts}");
    eprintln!("redshift_compat [record]: using schema {schema}");

    let mut rs = RedshiftConn::connect();
    rs.execute_update(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"));
    rs.set_schema(schema.clone());
    eprintln!("redshift_compat [record]: schema {schema} created");

    for stmt in common::redshift_data_ddl(&schema) {
        rs.execute_update(&stmt);
    }
    eprintln!("redshift_compat [record]: test data loaded");

    let db_batches = rs.execute_query("SELECT CURRENT_DATABASE()");
    let database = {
        use arrow_array::cast::AsArray;
        db_batches
            .first()
            .and_then(|b| {
                b.column(0)
                    .as_string_opt::<i32>()
                    .map(|a| a.value(0).to_string())
            })
            .expect("failed to get current database")
    };

    let mut store = common::setup_metric_store(&database, &schema);
    let mut tome = common::TomeWriter::new();

    common::run_scorecard(
        &mut store,
        Dialect::Redshift,
        "Redshift",
        &mut |name, sql| match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rs.execute_query(sql)
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
        "redshift_compat [record]: tome written to {:?}",
        tome_path()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Replay test — reads parquet tome, no Redshift needed
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn redshift_compat_replay() {
    let path = tome_path();
    if !path.exists() {
        eprintln!(
            "redshift_compat [replay]: no tome at {path:?} — \
             run redshift_compat_record to generate it"
        );
        return;
    }

    let recordings = common::read_tome(&path);
    let mut store = common::setup_metric_store("RECORDED_DB", "RECORDED_SCHEMA");

    common::run_scorecard(
        &mut store,
        Dialect::Redshift,
        "Redshift",
        &mut |name, _sql| Ok(recordings.get(name).cloned()),
    );
}
