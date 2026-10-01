use std::io::Write;
use std::process::Command;

fn dbt_metricflow() -> Command {
    let bin = env!("CARGO_BIN_EXE_dbt-metricflow");
    Command::new(bin)
}

fn example_manifest() -> tempfile::NamedTempFile {
    let manifest = serde_json::json!({
        "semantic_models": [{
            "name": "orders",
            "node_relation": {
                "alias": "orders",
                "schema_name": "public",
                "relation_name": "\"public\".\"orders\""
            },
            "defaults": { "agg_time_dimension": "ordered_at" },
            "primary_entity": "order",
            "entities": [
                { "name": "order", "type": "primary", "expr": "order_id" }
            ],
            "measures": [
                { "name": "order_count", "agg": "sum", "expr": "1" },
                { "name": "revenue", "agg": "sum", "expr": "amount" }
            ],
            "dimensions": [
                { "name": "ordered_at", "type": "time", "expr": "ordered_at",
                  "type_params": { "time_granularity": "day" } },
                { "name": "status", "type": "categorical", "expr": "status" }
            ]
        }],
        "metrics": [
            {
                "name": "order_count",
                "type": "simple",
                "type_params": {
                    "metric_aggregation_params": {
                        "semantic_model": "orders", "agg": "sum",
                        "expr": "1", "agg_time_dimension": "ordered_at"
                    }
                }
            },
            {
                "name": "revenue",
                "type": "simple",
                "type_params": {
                    "metric_aggregation_params": {
                        "semantic_model": "orders", "agg": "sum",
                        "expr": "amount", "agg_time_dimension": "ordered_at"
                    }
                }
            }
        ],
        "project_configuration": { "time_spines": [] }
    });
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(manifest.to_string().as_bytes()).unwrap();
    f
}

#[test]
fn cli_help() {
    let out = dbt_metricflow().arg("--help").output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("dbt-metricflow"),
        "help should mention binary name"
    );
    assert!(
        stderr.contains("compile"),
        "help should mention compile command"
    );
}

#[test]
fn cli_example() {
    let out = dbt_metricflow().arg("example").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("semantic_models"));
    assert!(stdout.contains("dbt-metricflow compile"));
}

#[test]
fn cli_compile_simple() {
    let manifest = example_manifest();
    let out = dbt_metricflow()
        .args([
            "compile",
            "-m",
            manifest.path().to_str().unwrap(),
            "--metrics",
            "order_count",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sql = String::from_utf8_lossy(&out.stdout);
    assert!(sql.contains("SUM(1) AS order_count"));
    assert!(sql.contains("\"public\".\"orders\""));
}

#[test]
fn cli_compile_with_group_by() {
    let manifest = example_manifest();
    let out = dbt_metricflow()
        .args([
            "compile",
            "-m",
            manifest.path().to_str().unwrap(),
            "--metrics",
            "revenue",
            "--group-by",
            "metric_time:day,status",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sql = String::from_utf8_lossy(&out.stdout);
    assert!(sql.contains("metric_time"));
    assert!(sql.contains("status"));
    assert!(sql.contains("GROUP BY"));
}

#[test]
fn cli_compile_with_where() {
    let manifest = example_manifest();
    let out = dbt_metricflow()
        .args([
            "compile",
            "-m",
            manifest.path().to_str().unwrap(),
            "--metrics",
            "order_count",
            "--where",
            "status=completed",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sql = String::from_utf8_lossy(&out.stdout);
    assert!(sql.contains("WHERE"));
    assert!(sql.contains("'completed'"));
}

#[test]
fn cli_compile_snowflake_dialect() {
    let manifest = example_manifest();
    let out = dbt_metricflow()
        .args([
            "compile",
            "-m",
            manifest.path().to_str().unwrap(),
            "--metrics",
            "revenue",
            "--group-by",
            "metric_time:day",
            "--dialect",
            "snowflake",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sql = String::from_utf8_lossy(&out.stdout);
    assert!(sql.contains("DATE_TRUNC"));
}

#[test]
fn cli_compile_multi_metric() {
    let manifest = example_manifest();
    let out = dbt_metricflow()
        .args([
            "compile",
            "-m",
            manifest.path().to_str().unwrap(),
            "--metrics",
            "order_count,revenue",
            "--group-by",
            "metric_time:day",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sql = String::from_utf8_lossy(&out.stdout);
    assert!(sql.contains("order_count"));
    assert!(sql.contains("revenue"));
}

#[test]
fn cli_error_missing_manifest() {
    let out = dbt_metricflow()
        .args(["compile", "--metrics", "foo"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--manifest"));
}

#[test]
fn cli_error_bad_metric() {
    let manifest = example_manifest();
    let out = dbt_metricflow()
        .args([
            "compile",
            "-m",
            manifest.path().to_str().unwrap(),
            "--metrics",
            "nonexistent_metric",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("compile error"));
}

#[test]
fn cli_compile_redshift_dialect() {
    let manifest = example_manifest();
    let out = dbt_metricflow()
        .args([
            "compile",
            "-m",
            manifest.path().to_str().unwrap(),
            "--metrics",
            "revenue",
            "--group-by",
            "metric_time:day",
            "--dialect",
            "redshift",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sql = String::from_utf8_lossy(&out.stdout);
    assert!(sql.contains("DATE_TRUNC"));
    assert!(
        sql.contains("::DATE"),
        "Redshift should use Postgres-style casts"
    );
}

#[test]
fn cli_compile_bigquery_dialect() {
    let manifest = example_manifest();
    let out = dbt_metricflow()
        .args([
            "compile",
            "-m",
            manifest.path().to_str().unwrap(),
            "--metrics",
            "revenue",
            "--group-by",
            "metric_time:day",
            "--dialect",
            "bigquery",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sql = String::from_utf8_lossy(&out.stdout);
    assert!(
        sql.contains("CAST(DATE_TRUNC("),
        "BigQuery reverses DATE_TRUNC args"
    );
    assert!(!sql.contains("::DATE"), "BigQuery should not use :: casts");
}

#[test]
fn cli_compile_databricks_dialect() {
    let manifest = example_manifest();
    let out = dbt_metricflow()
        .args([
            "compile",
            "-m",
            manifest.path().to_str().unwrap(),
            "--metrics",
            "revenue",
            "--group-by",
            "metric_time:day",
            "--dialect",
            "databricks",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sql = String::from_utf8_lossy(&out.stdout);
    assert!(sql.contains("CAST("));
    assert!(
        !sql.contains("::DATE"),
        "Databricks should not use :: casts"
    );
}
