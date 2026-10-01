//! Snowflake compatibility tests for the MetricFlow semantic query compiler.
//!
//! Two modes:
//!
//! - **Replay** (default): loads pre-recorded results from
//!   `tests/data/snowflake_recordings.parquet`.  No Snowflake connection needed.
//!   ```sh
//!   cargo test -p dbt-metricflow --test snowflake_compat
//!   ```
//!
//! - **Record** (`#[ignore]`, requires `fusion_tests/snowflake` profile):
//!   executes compiled SQL on a live Snowflake cluster and writes results to
//!   the parquet tome so replay tests stay up to date.
//!   ```sh
//!   cargo test -p dbt-metricflow --test snowflake_compat -- --ignored --nocapture
//!   ```

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use arrow_array::RecordBatch;
use dbt_adbc::{Backend, LoadStrategy, connection, driver};
use dbt_auth::{AdapterConfig, auth_for_backend};
use dbt_metricflow::{Dialect, InMemoryMetricStore};
use dbt_profile::ProfileEnvironment;

// ═══════════════════════════════════════════════════════════════════════════
// Tome path
// ═══════════════════════════════════════════════════════════════════════════

fn tome_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("snowflake_recordings.parquet")
}

// ═══════════════════════════════════════════════════════════════════════════
// Snowflake connection wrapper
// ═══════════════════════════════════════════════════════════════════════════

const PROFILE: &str = "fusion_tests";
const TARGET: &str = "snowflake";

struct SnowflakeConn {
    _drv: Box<dyn dbt_adbc::Driver>,
    _database: Box<dyn dbt_adbc::Database>,
    conn: Box<dyn dbt_adbc::Connection>,
    schema: Option<String>,
}

impl Drop for SnowflakeConn {
    fn drop(&mut self) {
        if let Some(schema) = self.schema.take() {
            eprintln!("snowflake_compat: dropping schema {schema}");
            let _ = self.try_execute_update(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"));
        }
    }
}

impl SnowflakeConn {
    fn connect() -> Self {
        let home = dirs::home_dir().expect("home dir exists");
        let profile_path = home.join(".dbt").join("profiles.yml");
        let penv = ProfileEnvironment::new(BTreeMap::new());
        let resolved = dbt_profile::resolve_with_env(&penv, &profile_path, PROFILE, Some(TARGET))
            .expect("failed to resolve fusion_tests/snowflake profile");

        eprintln!(
            "snowflake_compat: connecting to {} ({}) target={}",
            resolved.adapter_type, resolved.profile_name, resolved.target_name,
        );

        assert_eq!(
            resolved.adapter_type, "snowflake",
            "profile must be snowflake, got: {}",
            resolved.adapter_type
        );

        let backend = Backend::Snowflake;
        let adapter_config = AdapterConfig::new(resolved.credentials);
        let auth = auth_for_backend(backend);
        let db_builder = auth
            .configure(&adapter_config)
            .expect("auth configuration failed");

        let mut drv = driver::Builder::new(backend, LoadStrategy::CdnCache)
            .try_load()
            .expect("failed to load Snowflake driver");

        let mut database = db_builder
            .build(&mut drv)
            .expect("failed to open Snowflake database");

        let conn = connection::Builder::default()
            .build(&mut database)
            .expect("failed to create Snowflake connection");

        SnowflakeConn {
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
// Record test — live Snowflake, writes parquet tome
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore = "requires fusion_tests/snowflake profile in ~/.dbt/profiles.yml"]
fn snowflake_compat_record() {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let schema = format!("MF_TEST_{ts}");
    eprintln!("snowflake_compat [record]: using schema {schema}");

    let mut sf = SnowflakeConn::connect();
    sf.execute_update(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"));
    sf.set_schema(schema.clone());
    eprintln!("snowflake_compat [record]: schema {schema} created");

    for stmt in common::snowflake_data_ddl(&schema) {
        sf.execute_update(&stmt);
    }
    eprintln!("snowflake_compat [record]: test data loaded");

    let db_batches = sf.execute_query("SELECT CURRENT_DATABASE()");
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
        Dialect::Snowflake,
        "Snowflake",
        &mut |name, sql| match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sf.execute_query(sql)
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
        "snowflake_compat [record]: tome written to {:?}",
        tome_path()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Replay test — reads parquet tome, no Snowflake needed
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn snowflake_compat_replay() {
    let path = tome_path();
    if !path.exists() {
        eprintln!(
            "snowflake_compat [replay]: no tome at {path:?} — \
             run snowflake_compat_record to generate it"
        );
        return;
    }

    let recordings = common::read_tome(&path);
    let mut store = common::setup_metric_store("RECORDED_DB", "RECORDED_SCHEMA");

    common::run_scorecard(
        &mut store,
        Dialect::Snowflake,
        "Snowflake",
        &mut |name, _sql| Ok(recordings.get(name).cloned()),
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Ported test record/replay — 266 ported MetricFlow tests on Snowflake
// ═══════════════════════════════════════════════════════════════════════════

fn ported_tome_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("snowflake_ported_recordings.parquet")
}

/// Build ported test stores from manifest JSON files, retargeted for Snowflake.
fn setup_ported_stores(database: &str, schema: &str) -> common::PortedStores {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("manifests");

    let model_names = [
        "SIMPLE_MODEL",
        "EXTENDED_DATE_MODEL",
        "UNPARTITIONED_MULTI_HOP_JOIN_MODEL",
        "PARTITIONED_MULTI_HOP_JOIN_MODEL",
        "SIMPLE_MODEL_NON_DS",
        "SCD_MODEL",
    ];

    let mut stores = common::PortedStores::new();
    for name in &model_names {
        let path = manifest_dir.join(format!("{name}.json"));
        if !path.exists() {
            eprintln!("  warning: manifest {path:?} not found — run export_ported_manifests first");
            continue;
        }
        let json_str = fs::read_to_string(&path).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        common::rewrite_manifest_relations(&mut manifest, database, schema);
        let store = InMemoryMetricStore::from_manifest(&manifest);
        stores.insert((*name).to_string(), store);
    }
    stores
}

#[test]
#[ignore = "requires fusion_tests/snowflake profile in ~/.dbt/profiles.yml"]
fn snowflake_ported_record() {
    use dbt_metricflow::{compile, parse_query_spec};

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let schema = format!("MF_PORTED_{ts}");
    eprintln!("snowflake_ported [record]: using schema {schema}");

    let mut sf = SnowflakeConn::connect();
    sf.execute_update(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"));
    sf.set_schema(schema.clone());
    eprintln!("snowflake_ported [record]: schema {schema} created");

    for stmt in common::snowflake_ported_data_ddl(&schema) {
        sf.execute_update(&stmt);
    }
    {
        // Load original simple model data, skipping tables that already exist
        // from ported data (they may have different column orders).
        let mut skip_table: Option<String> = None;
        for stmt in common::snowflake_data_ddl(&schema) {
            if stmt.contains("CREATE TABLE") {
                let table_name = stmt
                    .split("CREATE TABLE ")
                    .nth(1)
                    .and_then(|s| s.split_whitespace().next())
                    .unwrap_or("")
                    .to_string();
                if sf
                    .try_execute_update(&format!("SELECT 1 FROM {table_name} LIMIT 0"))
                    .is_ok()
                {
                    skip_table = Some(table_name);
                    continue;
                }
                skip_table = None;
            } else if let Some(ref skipped) = skip_table {
                if stmt.contains("INSERT INTO") && stmt.contains(skipped.as_str()) {
                    continue;
                }
            }
            sf.execute_update(&stmt);
        }
    }
    eprintln!("snowflake_ported [record]: test data loaded");

    let db_batches = sf.execute_query("SELECT CURRENT_DATABASE()");
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

    let mut stores = setup_ported_stores(&database, &schema);
    let test_cases = common::load_ported_test_cases();
    let mut tome = common::TomeWriter::new();

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut skip = 0u32;
    let mut results: Vec<(String, &str, String)> = Vec::new();

    for tc in &test_cases {
        let store = match stores.get_mut(tc.model.as_str()) {
            Some(s) => s,
            None => {
                skip += 1;
                continue;
            }
        };

        if let Some(ref reason) = tc.skip_reason {
            if !reason.is_empty() {
                skip += 1;
                continue;
            }
        }

        let spec_json = tc.spec.to_json();
        let spec = match parse_query_spec(&spec_json) {
            Ok(s) => s,
            Err(e) => {
                fail += 1;
                results.push((tc.name.clone(), "FAIL", format!("parse: {e}")));
                continue;
            }
        };
        let our_sql = match compile(store, &spec, Dialect::Snowflake) {
            Ok(s) => s,
            Err(e) => {
                fail += 1;
                results.push((tc.name.clone(), "FAIL", format!("compile: {e}")));
                continue;
            }
        };

        let our_sql = if tc.min_max_only {
            common::wrap_min_max_only(&our_sql, &tc.spec)
        } else {
            our_sql
        };

        // Execute our compiled SQL on Snowflake.
        let our_batches = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sf.execute_query(&our_sql)
        })) {
            Ok(b) => b,
            Err(_) => {
                fail += 1;
                results.push((
                    tc.name.clone(),
                    "FAIL",
                    format!("exec our SQL panicked\n  SQL: {our_sql}"),
                ));
                continue;
            }
        };

        // Execute the reference check_query rewritten for Snowflake.
        let ref_sql =
            common::rewrite_check_query_for_snowflake(&tc.check_query, &database, &schema);
        let ref_batches = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sf.execute_query(&ref_sql)
        })) {
            Ok(b) => b,
            Err(_) => {
                fail += 1;
                results.push((
                    tc.name.clone(),
                    "FAIL",
                    format!("exec check_query panicked\n  SQL: {ref_sql}"),
                ));
                continue;
            }
        };

        // Compare results; only record if they match.
        match common::compare_results(&our_batches, &ref_batches, tc.check_order) {
            Ok(()) => {
                tome.insert(&tc.name, &our_batches);
                tome.insert(&format!("{}__ref", tc.name), &ref_batches);
                let rows: usize = our_batches.iter().map(|b| b.num_rows()).sum();
                pass += 1;
                results.push((tc.name.clone(), "PASS", format!("{rows} rows")));
            }
            Err(diff) => {
                fail += 1;
                results.push((
                    tc.name.clone(),
                    "FAIL",
                    format!("compare: {diff}\n  OUR SQL: {our_sql}"),
                ));
            }
        }
    }

    tome.write(&ported_tome_path());

    let bar = "=".repeat(70);
    eprintln!("\n{bar}");
    eprintln!("Snowflake Ported Record Scorecard");
    eprintln!("{bar}");
    for (name, status, detail) in &results {
        if *status == "PASS" {
            continue;
        }
        eprintln!("  [{status}] {name}: {detail}");
    }
    eprintln!("{bar}");
    let total = pass + fail;
    eprintln!("  Total: {total}  Pass: {pass}  Fail: {fail}  Skip: {skip}");
    eprintln!("{bar}\n");

    eprintln!(
        "snowflake_ported [record]: tome written to {:?}",
        ported_tome_path()
    );

    if fail > 0 {
        eprintln!(
            "NOTE: {fail} Snowflake ported tests failed — \
             this scorecard tracks cross-dialect progress."
        );
    }
}

#[test]
fn snowflake_ported_replay() {
    let path = ported_tome_path();
    if !path.exists() {
        eprintln!(
            "snowflake_ported [replay]: no tome at {path:?} — \
             run snowflake_ported_record to generate it"
        );
        return;
    }

    let recordings = common::read_tome(&path);
    let test_cases = common::load_ported_test_cases();
    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut skip = 0u32;
    let mut results: Vec<(String, &str, String)> = Vec::new();

    for tc in &test_cases {
        if let Some(ref reason) = tc.skip_reason {
            if !reason.is_empty() {
                skip += 1;
                continue;
            }
        }

        let our_batches = match recordings.get(tc.name.as_str()) {
            Some(b) => b.clone(),
            None => {
                skip += 1;
                continue;
            }
        };
        let ref_key = format!("{}__ref", tc.name);
        let ref_batches = match recordings.get(ref_key.as_str()) {
            Some(b) => b.clone(),
            None => {
                skip += 1;
                continue;
            }
        };

        match common::compare_results(&our_batches, &ref_batches, tc.check_order) {
            Ok(()) => {
                let rows: usize = our_batches.iter().map(|b| b.num_rows()).sum();
                pass += 1;
                results.push((tc.name.clone(), "PASS", format!("{rows} rows")));
            }
            Err(diff) => {
                fail += 1;
                results.push((tc.name.clone(), "FAIL", format!("compare: {diff}")));
            }
        }
    }

    let bar = "=".repeat(70);
    eprintln!("\n{bar}");
    eprintln!("Snowflake Ported Replay Scorecard");
    eprintln!("{bar}");
    for (name, status, detail) in &results {
        if *status == "PASS" {
            continue;
        }
        eprintln!("  [{status}] {name}: {detail}");
    }
    eprintln!("{bar}");
    let total = pass + fail;
    eprintln!("  Total: {total}  Pass: {pass}  Fail: {fail}  Skip: {skip}");
    eprintln!("{bar}\n");

    assert_eq!(
        fail, 0,
        "{fail} of {total} Snowflake ported replay tests failed"
    );
}
