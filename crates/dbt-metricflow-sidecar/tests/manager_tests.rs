//! Integration tests for `MetricflowSidecarManager`, run against the
//! `fake_mf_entry` fixture binary (see `tests/fixtures/fake_mf_entry.rs`) —
//! no dependency on Python or a real Nuitka-compiled `mf_entry` binary.

use std::path::PathBuf;

use dbt_metricflow_sidecar::manager::MetricflowSidecarManager;
use dbt_metricflow_sidecar::messages::ExplainParams;

fn fixture_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_mf_entry"))
}

fn explain_params(manifest_path: &str) -> ExplainParams {
    ExplainParams {
        manifest_path: manifest_path.to_string(),
        metric_names: Some(vec!["bookings".to_string()]),
        group_by_names: Some(vec!["metric_time".to_string()]),
        where_constraints: None,
        order_by_names: None,
        limit: None,
        sql_engine: "DUCKDB".to_string(),
    }
}

#[tokio::test]
async fn spawn_succeeds_and_ping_round_trips() {
    let manager = MetricflowSidecarManager::spawn(&fixture_binary_path())
        .await
        .expect("spawn should succeed");
    manager.ping().await.expect("ping should succeed");
}

#[tokio::test]
async fn explain_returns_sql() {
    let manager = MetricflowSidecarManager::spawn(&fixture_binary_path())
        .await
        .expect("spawn should succeed");
    let sql = manager
        .explain(explain_params("/some/manifest"))
        .await
        .expect("explain should succeed");
    assert_eq!(sql, "SELECT 1");
}

#[tokio::test]
async fn explain_surfaces_structured_error() {
    let manager = MetricflowSidecarManager::spawn(&fixture_binary_path())
        .await
        .expect("spawn should succeed");
    let err = manager
        .explain(explain_params("FORCE_ERROR"))
        .await
        .expect_err("explain should fail");
    let message = err.to_string();
    assert!(
        message.contains("TestError"),
        "expected TestError in: {message}"
    );
    assert!(
        message.contains("forced error for testing"),
        "expected error detail in: {message}"
    );
}

#[tokio::test]
async fn concurrent_explain_requests_are_each_answered_correctly() {
    let manager = MetricflowSidecarManager::spawn(&fixture_binary_path())
        .await
        .expect("spawn should succeed");

    let mut handles = Vec::new();
    for i in 0..8 {
        let manager = manager.clone();
        handles.push(tokio::spawn(async move {
            let params = explain_params(&format!("/manifest-{i}"));
            manager.explain(params).await
        }));
    }

    for handle in handles {
        let sql = handle
            .await
            .expect("task should not panic")
            .expect("explain should succeed");
        assert_eq!(sql, "SELECT 1");
    }
}

#[tokio::test]
async fn shutdown_round_trips() {
    let manager = MetricflowSidecarManager::spawn(&fixture_binary_path())
        .await
        .expect("spawn should succeed");
    manager.shutdown().await.expect("shutdown should succeed");
}
