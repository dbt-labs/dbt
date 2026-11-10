use std::fs;
use std::path::{Path, PathBuf};

use arrow_array::{Decimal128Array, Float64Array, Int32Array, Int64Array, RecordBatch};
use dbt_adbc::{Backend, Connection, Database, database, driver};
use dbt_metricflow::{Dialect, InMemoryMetricStore, compile, parse_query_spec};

struct DuckDb {
    #[allow(dead_code)]
    database: Box<dyn Database>,
    connection: Box<dyn Connection>,
}

impl DuckDb {
    fn open_memory() -> Self {
        let mut drv = driver::Builder::new(
            Backend::DuckDBExtended,
            driver::LoadStrategy::SystemThenCdnCache,
        )
        .try_load()
        .expect("failed to load DuckDB driver");
        let mut db_builder = database::Builder::new(Backend::DuckDBExtended);
        db_builder
            .with_named_option("path", ":memory:")
            .expect("failed to set DuckDB path");
        let database = db_builder
            .build(&mut drv)
            .expect("failed to create DuckDB database");
        let connection = database
            .new_connection()
            .expect("failed to create DuckDB connection");
        Self {
            database,
            connection,
        }
    }

    fn execute_update(&mut self, sql: &str) -> Result<Option<i64>, String> {
        let mut stmt = self.connection.new_statement().map_err(|e| e.to_string())?;
        stmt.set_sql_query(sql).map_err(|e| e.to_string())?;
        stmt.execute_update().map_err(|e| e.to_string())
    }

    fn execute_query(&mut self, sql: &str) -> Result<Vec<RecordBatch>, String> {
        let mut stmt = self.connection.new_statement().map_err(|e| e.to_string())?;
        stmt.set_sql_query(sql).map_err(|e| e.to_string())?;
        let reader = stmt.execute().map_err(|e| e.to_string())?;
        let mut batches = Vec::new();
        for batch in reader {
            batches.push(batch.map_err(|e| e.to_string())?);
        }
        Ok(batches)
    }
}

fn docs_examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("examples")
}

fn docs_sql_dir() -> PathBuf {
    docs_examples_dir().join("sql")
}

fn load_manifest_store() -> InMemoryMetricStore {
    let manifest_path = docs_examples_dir().join("duckdb_orders_manifest.json");
    let manifest_text = fs::read_to_string(&manifest_path).expect("failed to read manifest");
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_text).expect("failed to parse manifest");
    InMemoryMetricStore::from_manifest(&manifest)
}

fn setup_duckdb() -> DuckDb {
    let sql_path = docs_examples_dir().join("duckdb_orders.sql");
    let sql = fs::read_to_string(&sql_path).expect("failed to read DuckDB setup SQL");
    let mut db = DuckDb::open_memory();
    for stmt in sql
        .split(';')
        .map(str::trim)
        .filter(|stmt| !stmt.is_empty())
    {
        db.execute_update(stmt)
            .unwrap_or_else(|e| panic!("failed to execute docs example SQL: {e}\nSQL: {stmt}"));
    }
    db
}

fn compile_and_execute(query_file: &str) -> (String, Vec<RecordBatch>) {
    let query_path = docs_examples_dir().join("queries").join(query_file);
    let query_text = fs::read_to_string(&query_path).expect("failed to read query spec");
    let spec = parse_query_spec(&query_text).expect("failed to parse query spec");
    let mut store = load_manifest_store();
    let sql = compile(&mut store, &spec, Dialect::DuckDB).expect("failed to compile query");
    let mut db = setup_duckdb();
    let batches = db
        .execute_query(&sql)
        .unwrap_or_else(|e| panic!("failed to execute compiled SQL: {e}\nSQL: {sql}"));
    (sql, batches)
}

fn compile_query(query_file: &str) -> String {
    let query_path = docs_examples_dir().join("queries").join(query_file);
    let query_text = fs::read_to_string(&query_path).expect("failed to read query spec");
    let spec = parse_query_spec(&query_text).expect("failed to parse query spec");
    let mut store = load_manifest_store();
    compile(&mut store, &spec, Dialect::DuckDB).expect("failed to compile query")
}

fn row_count(batches: &[RecordBatch]) -> usize {
    batches.iter().map(|batch| batch.num_rows()).sum()
}

fn first_scalar_f64(batches: &[RecordBatch]) -> Option<f64> {
    let batch = batches.first()?;
    if batch.num_rows() == 0 || batch.num_columns() == 0 {
        return None;
    }
    let col = batch.column(batch.num_columns() - 1);
    if let Some(arr) = col.as_any().downcast_ref::<Float64Array>() {
        Some(arr.value(0))
    } else if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
        Some(arr.value(0) as f64)
    } else if let Some(arr) = col.as_any().downcast_ref::<Int32Array>() {
        Some(arr.value(0) as f64)
    } else if let Some(arr) = col.as_any().downcast_ref::<Decimal128Array>() {
        let scale = arr.scale() as f64;
        Some(arr.value(0) as f64 / 10f64.powf(scale))
    } else {
        None
    }
}

fn approx_eq(left: f64, right: f64) -> bool {
    (left - right).abs() < 1e-9
}

#[test]
fn docs_examples_compile_and_execute_in_duckdb() {
    let cases = [
        ("order_count_by_status.json", 2usize),
        ("revenue_by_day.json", 3usize),
        ("completion_rate_by_day.json", 3usize),
        // Cumulative metrics use a bounded spine-backed range and can emit
        // trailing buckets beyond the raw source dates.
        ("trailing_2_day_revenue_by_day.json", 4usize),
    ];

    for (query_file, expected_rows) in cases {
        let (_sql, batches) = compile_and_execute(query_file);
        assert_eq!(
            row_count(&batches),
            expected_rows,
            "unexpected row count for {query_file}"
        );
    }
}

#[test]
fn docs_average_order_value_matches_expected_result() {
    let (_sql, batches) = compile_and_execute("average_order_value.json");
    assert_eq!(row_count(&batches), 1);
    let value = first_scalar_f64(&batches).expect("expected scalar metric result");
    assert!(
        approx_eq(value, 87.5),
        "expected 87.5 for average_order_value, got {value}"
    );
}

#[test]
fn docs_example_sql_matches_checked_in_snapshots() {
    let cases = [
        ("order_count_total.json", "order_count_total.sql"),
        ("revenue_by_day.json", "revenue_by_day.sql"),
        ("order_count_by_status.json", "order_count_by_status.sql"),
        (
            "order_count_by_customer.json",
            "order_count_by_customer.sql",
        ),
        (
            "order_count_where_completed.json",
            "order_count_where_completed.sql",
        ),
        ("multi_metric_by_day.json", "multi_metric_by_day.sql"),
        (
            "revenue_by_day_desc_limit_2.json",
            "revenue_by_day_desc_limit_2.sql",
        ),
        (
            "revenue_by_day_time_constraint.json",
            "revenue_by_day_time_constraint.sql",
        ),
        (
            "trailing_2_day_revenue_by_day.json",
            "trailing_2_day_revenue_by_day.sql",
        ),
    ];

    for (query_file, sql_file) in cases {
        let actual = compile_query(query_file);
        let expected = fs::read_to_string(docs_sql_dir().join(sql_file))
            .unwrap_or_else(|e| panic!("failed to read SQL snapshot {sql_file}: {e}"));
        assert_eq!(
            actual.trim_end(),
            expected.trim_end(),
            "generated SQL drifted for {query_file}"
        );
    }
}
