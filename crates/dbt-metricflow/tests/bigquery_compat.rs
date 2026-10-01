//! BigQuery compatibility tests for the MetricFlow semantic query compiler.
//!
//! Two modes:
//!
//! - **Replay** (default): loads pre-recorded results from
//!   `tests/data/bigquery_recordings.parquet`.  No BigQuery connection needed.
//!   ```sh
//!   cargo test -p dbt-metricflow --test bigquery_compat
//!   ```
//!
//! - **Record** (`#[ignore]`, requires `fusion_tests/bigquery` profile):
//!   executes compiled SQL on a live BigQuery instance and writes results to
//!   the parquet tome so replay tests stay up to date.
//!   ```sh
//!   cargo test -p dbt-metricflow --test bigquery_compat -- --ignored --nocapture
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
const TARGET: &str = "bigquery";

// ═══════════════════════════════════════════════════════════════════════════
// Tome path
// ═══════════════════════════════════════════════════════════════════════════

fn tome_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("bigquery_recordings.parquet")
}

// ═══════════════════════════════════════════════════════════════════════════
// BigQuery connection wrapper
// ═══════════════════════════════════════════════════════════════════════════

struct BigQueryConn {
    _drv: Box<dyn dbt_adbc::Driver>,
    _database: Box<dyn dbt_adbc::Database>,
    conn: Box<dyn dbt_adbc::Connection>,
    dataset: Option<String>,
}

impl Drop for BigQueryConn {
    fn drop(&mut self) {
        if let Some(dataset) = self.dataset.take() {
            eprintln!("bigquery_compat: dropping dataset {dataset}");
            let _ = self.try_execute_update(&format!("DROP SCHEMA IF EXISTS {dataset} CASCADE"));
        }
    }
}

impl BigQueryConn {
    fn connect() -> Self {
        let home = dirs::home_dir().expect("home dir exists");
        let profile_path = home.join(".dbt").join("profiles.yml");
        let penv = ProfileEnvironment::new(BTreeMap::new());
        let resolved = dbt_profile::resolve_with_env(&penv, &profile_path, PROFILE, Some(TARGET))
            .expect("failed to resolve fusion_tests/bigquery profile");

        eprintln!(
            "bigquery_compat: connecting to {} ({}) target={}",
            resolved.adapter_type, resolved.profile_name, resolved.target_name,
        );

        assert_eq!(
            resolved.adapter_type, "bigquery",
            "profile must be bigquery, got: {}",
            resolved.adapter_type
        );

        let backend = Backend::BigQuery;
        let adapter_config = AdapterConfig::new(resolved.credentials);
        let auth = auth_for_backend(backend);
        let db_builder = auth
            .configure(&adapter_config)
            .expect("auth configuration failed");

        let mut drv = driver::Builder::new(backend, LoadStrategy::CdnCache)
            .try_load()
            .expect("failed to load BigQuery driver");

        let mut database = db_builder
            .build(&mut drv)
            .expect("failed to open BigQuery database");

        let conn = connection::Builder::default()
            .build(&mut database)
            .expect("failed to create BigQuery connection");

        BigQueryConn {
            _drv: drv,
            _database: database,
            conn,
            dataset: None,
        }
    }

    fn set_dataset(&mut self, dataset: String) {
        self.dataset = Some(dataset);
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
// Record test — live BigQuery, writes parquet tome
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires fusion_tests/bigquery profile in ~/.dbt/profiles.yml"]
fn bigquery_compat_record() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let dataset = format!("mf_test_{ts}");
    eprintln!("bigquery_compat [record]: using dataset {dataset}");

    let mut bq = BigQueryConn::connect();
    bq.execute_update(&format!("CREATE SCHEMA IF NOT EXISTS {dataset}"));
    bq.set_dataset(dataset.clone());
    let database = "dbt-test-env";
    // BigQuery schema creation is eventually consistent — poll until queryable.
    for attempt in 0..30 {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bq.execute_query(&format!(
                "SELECT 1 FROM `{database}`.`{dataset}`.INFORMATION_SCHEMA.TABLES LIMIT 0"
            ))
        })) {
            Ok(_) => break,
            Err(_) if attempt < 29 => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(_) => panic!("dataset {dataset} not available after 6 s"),
        }
    }
    eprintln!("bigquery_compat [record]: dataset {dataset} created");

    for stmt in common::bigquery_data_ddl(&dataset) {
        bq.execute_update(&stmt);
    }
    eprintln!("bigquery_compat [record]: test data loaded");

    let mut store = common::setup_metric_store(database, &dataset);
    let mut tome = common::TomeWriter::new();

    common::run_scorecard(
        &mut store,
        Dialect::BigQuery,
        "BigQuery",
        &mut |name, sql| match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bq.execute_query(sql)
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
        "bigquery_compat [record]: tome written to {:?}",
        tome_path()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Replay test — reads parquet tome, no BigQuery needed
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn bigquery_compat_replay() {
    let path = tome_path();
    if !path.exists() {
        eprintln!(
            "bigquery_compat [replay]: no tome at {path:?} — \
             run bigquery_compat_record to generate it"
        );
        return;
    }

    let recordings = common::read_tome(&path);
    let mut store = common::setup_metric_store("RECORDED_DB", "RECORDED_DATASET");

    common::run_scorecard(
        &mut store,
        Dialect::BigQuery,
        "BigQuery",
        &mut |name, _sql| Ok(recordings.get(name).cloned()),
    );
}
