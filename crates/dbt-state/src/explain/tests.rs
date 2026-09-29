use super::log::{fallback_records_from_log, read_records_for_state_explain};
use super::paths::{resolve_log_file, state_explain_log_config_from_getter};
use super::render::render_explain_output;
use super::select::{filter_explain_records, sort_explain_records};
use super::service::{EXPLAIN_MAX_BATCH_SIZE, should_fetch_service_explain};
use super::types::STATE_EXPLAIN_RECORD_VERSION;
use super::{
    StateExplainDevClone, StateExplainLog, StateExplainLogRecord, StateExplainNode,
    StateExplainNodeInfo, StateExplainOptions, StateExplainRecord, StateExplainRunConfig,
    StateExplainRunStart, StateExplainStatus, append_state_explain_log_record,
    execution_decision_ids, new_state_explain_log_path, prune_state_explain_logs,
    read_explain_records, read_state_explain_log, render_explain_records,
    render_merged_explain_records, render_service_explain_response,
    run_cache_service_config_for_state_explain, service_explain_response_with_client,
};
use crate::proto::query_cache::{
    ExplainBadge, ExplainLine, ExplainMarker, ExplainMessageEntry, GetExplainMessagesRequest,
    GetExplainMessagesResponse, SubmitSqlResultType,
};
use crate::service_client::{RunCacheServiceClient, RunCacheServiceError};
use crate::service_config::{DEFAULT_LOG_PREFIX, RunCacheServiceConfig};
use std::path::PathBuf;
use std::sync::Mutex;

#[test]
fn read_explain_records_parses_jsonl() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("run-cache.jsonl");
    std::fs::write(
            &path,
            r#"{"version":1,"node_unique_id":"model.pkg.orders","execution_decision_id":"decision-1","status":"hit","reason":"fingerprint matched","details":["compiled SQL matched"]}
{"version":1,"node_unique_id":"model.pkg.customers","status":"miss","reason":"relation changed"}"#,
        )
        .unwrap();

    let records = read_explain_records(&path).unwrap();

    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].execution_decision_id.as_deref(),
        Some("decision-1")
    );
    assert_eq!(records[0].status, StateExplainStatus::Hit);
    assert_eq!(records[0].details, ["compiled SQL matched"]);
    assert!(records[1].execution_decision_id.is_none());
    assert_eq!(records[1].status, StateExplainStatus::Miss);
    assert!(records[1].details.is_empty());
}

#[test]
fn read_explain_records_reports_line_number_for_invalid_jsonl() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("run-cache.jsonl");
    std::fs::write(
        &path,
        r#"{"version":1,"node_unique_id":"model.pkg.orders","status":"hit","reason":"ok"}
not-json"#,
    )
    .unwrap();

    let err = read_explain_records(&path).unwrap_err().to_string();

    assert!(err.contains("run-cache.jsonl:2"));
}

#[test]
fn read_explain_records_rejects_unsupported_version() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("run-cache.jsonl");
    std::fs::write(
        &path,
        r#"{"version":2,"node_unique_id":"model.pkg.orders","status":"hit","reason":"ok"}"#,
    )
    .unwrap();

    let err = read_explain_records(&path).unwrap_err().to_string();

    assert!(err.contains("Unsupported dbt State explain record version 2"));
}

#[test]
fn render_explain_records_formats_plain_output() {
    let records = sample_records();

    let output = render_explain_records(&records, &StateExplainOptions::default());

    assert_eq!(
        output,
        "HIT model.pkg.orders - fingerprint matched\nMISS model.pkg.customers - relation changed"
    );
}

#[test]
fn render_explain_records_includes_details_when_verbose() {
    let records = sample_records();
    let options = StateExplainOptions {
        verbose: true,
        ..Default::default()
    };

    let output = render_explain_records(&records, &options);

    assert!(output.contains("  - compiled SQL matched"));
}

#[test]
fn render_explain_output_includes_structured_run_config_when_verbose() {
    let records = sample_records();
    let run_start = StateExplainRunStart {
        start_timestamp_utc: "2026-06-02T00:00:00Z".to_string(),
        run_config: StateExplainRunConfig {
            defer_to_target: "prod".to_string(),
            freshness_tolerance_seconds: 60,
            tolerate_nondeterminism: true,
            clone_incremental_in_dev: "IF_TABLE_MISSING".to_string(),
            metadata_cache_ttl_seconds: 300,
            profile_name: "jaffle_shop".to_string(),
            target_name: "dev".to_string(),
            select: vec!["orders".to_string()],
            ..Default::default()
        },
    };
    let options = StateExplainOptions {
        verbose: true,
        ..Default::default()
    };

    let output = render_explain_output(&records, None, Some(&run_start), &options);

    assert!(output.starts_with("Run configuration:\n"));
    assert!(output.contains("  - profile: jaffle_shop"));
    assert!(output.contains("  - target: dev"));
    assert!(output.contains("  - defer to target: prod"));
    assert!(output.contains("  - select: orders"));
    assert!(output.contains("HIT model.pkg.orders"));
}

#[test]
fn render_explain_records_reports_empty_input() {
    let output = render_explain_records(&[], &StateExplainOptions::default());

    assert_eq!(output, "No dbt State explain records found.");
}

#[test]
fn render_merged_explain_records_uses_matching_service_message() {
    let mut records = sample_records();
    records[0].execution_decision_id = Some("decision-1".to_string());
    let response = GetExplainMessagesResponse {
        messages: vec![service_message("decision-1", "ready")],
    };

    let output =
        render_merged_explain_records(&records, Some(&response), &StateExplainOptions::default());

    assert_eq!(
        output,
        "READY_TO_EXECUTE model.pkg.orders - ready\nMISS model.pkg.customers - relation changed"
    );
}

#[test]
fn render_merged_explain_records_hides_service_tree_unless_verbose() {
    let mut records = sample_records();
    records[0].execution_decision_id = Some("decision-1".to_string());
    let mut message = service_message("decision-1", "ready");
    message.explain_lines = vec![ExplainLine {
        text: "target table exists".to_string(),
        marker: Some(ExplainMarker::Success as i32),
        badge: None,
        children: Vec::new(),
    }];
    let response = GetExplainMessagesResponse {
        messages: vec![message],
    };

    let concise =
        render_merged_explain_records(&records, Some(&response), &StateExplainOptions::default());

    assert_eq!(
        concise,
        "READY_TO_EXECUTE model.pkg.orders - ready\nMISS model.pkg.customers - relation changed"
    );

    let verbose = render_merged_explain_records(
        &records,
        Some(&response),
        &StateExplainOptions {
            verbose: true,
            ..Default::default()
        },
    );

    assert_eq!(
        verbose,
        "READY_TO_EXECUTE model.pkg.orders - ready\n  - target table exists [SUCCESS]\nMISS model.pkg.customers - relation changed"
    );
}

#[test]
fn render_merged_explain_records_falls_back_for_missing_service_message() {
    let mut records = sample_records();
    records[0].execution_decision_id = Some("missing-decision".to_string());
    let response = GetExplainMessagesResponse {
        messages: vec![service_message("decision-1", "ready")],
    };

    let output =
        render_merged_explain_records(&records, Some(&response), &StateExplainOptions::default());

    assert_eq!(
        output,
        "HIT model.pkg.orders - fingerprint matched\nMISS model.pkg.customers - relation changed"
    );
}

#[test]
fn render_merged_explain_records_falls_back_without_service_response() {
    let records = sample_records();

    let output = render_merged_explain_records(&records, None, &StateExplainOptions::default());

    assert_eq!(
        output,
        "HIT model.pkg.orders - fingerprint matched\nMISS model.pkg.customers - relation changed"
    );
}

#[test]
fn execution_decision_ids_extracts_non_empty_ids() {
    let mut records = sample_records();
    records[0].execution_decision_id = Some("decision-1".to_string());
    records[1].execution_decision_id = Some(String::new());

    assert_eq!(execution_decision_ids(&records), ["decision-1"]);
}

#[test]
fn state_explain_log_record_round_trips_run_start_and_node() {
    let run_start = StateExplainLogRecord::RunStart(StateExplainRunStart {
        start_timestamp_utc: "2026-06-02T00:00:00Z".to_string(),
        run_config: StateExplainRunConfig {
            defer_to_target: "prod".to_string(),
            profile_name: "jaffle_shop".to_string(),
            target_name: "dev".to_string(),
            select: vec!["orders".to_string()],
            ..Default::default()
        },
    });
    let node = StateExplainLogRecord::Node(StateExplainNode {
        node_unique_id: "model.jaffle_shop.orders".to_string(),
        node_name: "orders".to_string(),
        node_info: StateExplainNodeInfo {
            fqn: "\"db\".\"schema\".\"orders\"".to_string(),
            node_resource_type: "model".to_string(),
            is_table: true,
            ..Default::default()
        },
        execution_decision_id: Some("decision-1".to_string()),
    });

    let run_start_json = serde_json::to_string(&run_start).unwrap();
    let node_json = serde_json::to_string(&node).unwrap();

    assert!(run_start_json.contains(r#""entry_type":"run_start""#));
    assert!(run_start_json.contains(r#""start_timestamp_utc":"#));
    assert!(node_json.contains(r#""entry_type":"node""#));
    assert!(node_json.contains(r#""is_table":true"#));
    assert_eq!(
        serde_json::from_str::<StateExplainLogRecord>(&run_start_json).unwrap(),
        run_start
    );
    assert_eq!(
        serde_json::from_str::<StateExplainLogRecord>(&node_json).unwrap(),
        node
    );
}

#[test]
fn read_state_explain_log_parses_run_start_and_nodes() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("state-explain.jsonl");
    std::fs::write(&path, sample_state_explain_log_jsonl()).unwrap();

    let log = read_state_explain_log(&path).unwrap();

    assert_eq!(log.run_start.unwrap().run_config.defer_to_target, "prod");
    assert_eq!(log.nodes.len(), 1);
    assert_eq!(log.nodes[0].node_name, "orders");
    assert_eq!(
        log.nodes[0].execution_decision_id.as_deref(),
        Some("decision-1")
    );
}

#[test]
fn read_state_explain_log_reports_physical_line_number() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("state-explain.jsonl");
    std::fs::write(&path, format!("\n{}\nnot-json", sample_run_start_json())).unwrap();

    let err = read_state_explain_log(&path).unwrap_err().to_string();

    assert!(err.contains("state-explain.jsonl:3"));
}

#[test]
fn append_state_explain_log_record_writes_jsonl() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("logs").join("state-explain.jsonl");

    append_state_explain_log_record(
        &path,
        &StateExplainLogRecord::RunStart(StateExplainRunStart {
            start_timestamp_utc: "2026-06-02T00:00:00Z".to_string(),
            run_config: StateExplainRunConfig::default(),
        }),
    )
    .unwrap();
    append_state_explain_log_record(
        &path,
        &StateExplainLogRecord::Node(StateExplainNode {
            node_unique_id: "model.pkg.orders".to_string(),
            node_name: "orders".to_string(),
            node_info: StateExplainNodeInfo::default(),
            execution_decision_id: Some("decision-1".to_string()),
        }),
    )
    .unwrap();

    let log = read_state_explain_log(&path).unwrap();

    assert!(log.run_start.is_some());
    assert_eq!(log.nodes.len(), 1);
    assert_eq!(
        log.nodes[0].execution_decision_id.as_deref(),
        Some("decision-1")
    );
}

#[test]
fn append_state_explain_log_record_serializes_concurrent_writes() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("state-explain.jsonl");
    let handles: Vec<_> = (0..32)
        .map(|idx| {
            let path = path.clone();
            std::thread::spawn(move || {
                append_state_explain_log_record(
                    &path,
                    &StateExplainLogRecord::Node(StateExplainNode {
                        node_unique_id: format!("model.pkg.node_{idx}"),
                        node_name: format!("node_{idx}"),
                        node_info: StateExplainNodeInfo::default(),
                        execution_decision_id: Some(format!("decision-{idx}")),
                    }),
                )
                .unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let log = read_state_explain_log(&path).unwrap();
    let ids: std::collections::HashSet<_> = log
        .nodes
        .iter()
        .filter_map(|node| node.execution_decision_id.as_deref())
        .collect();

    assert_eq!(log.nodes.len(), 32);
    assert_eq!(ids.len(), 32);
}

#[test]
fn new_state_explain_log_path_uses_discoverable_prefix() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut config = RunCacheServiceConfig::disabled();
    config.log_prefix = "fusion_".to_string();

    let path = new_state_explain_log_path(temp_dir.path(), None, &config);

    assert_eq!(path.parent().unwrap().file_name().unwrap(), "run_cache");
    assert!(
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("fusion_")
    );
    assert!(
        path.extension()
            .is_some_and(|extension| extension == "jsonl")
    );
}

#[test]
fn new_state_explain_log_path_uses_default_prefix_for_empty_config() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut config = RunCacheServiceConfig::disabled();
    config.log_prefix.clear();

    let path = new_state_explain_log_path(temp_dir.path(), None, &config);

    assert!(
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(DEFAULT_LOG_PREFIX)
    );
}

#[test]
fn prune_state_explain_logs_enforces_log_file_limit() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut config = RunCacheServiceConfig::disabled();
    config.log_file_limit = 2;
    let logs = write_explain_logs(temp_dir.path(), &config);
    let unrelated = temp_dir.path().join("other.jsonl");
    std::fs::write(&unrelated, "").unwrap();

    prune_state_explain_logs(logs.last().unwrap(), &config);

    assert!(!logs[0].exists());
    assert!(logs[1].exists());
    assert!(logs[2].exists());
    assert!(unrelated.exists());
}

#[test]
fn prune_state_explain_logs_keeps_all_logs_for_non_positive_limit() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut config = RunCacheServiceConfig::disabled();
    config.log_file_limit = 0;
    let logs = write_explain_logs(temp_dir.path(), &config);

    prune_state_explain_logs(logs.last().unwrap(), &config);
    config.log_file_limit = -1;
    prune_state_explain_logs(logs.last().unwrap(), &config);

    assert!(logs.iter().all(|path| path.exists()));
}

fn write_explain_logs(log_dir: &std::path::Path, config: &RunCacheServiceConfig) -> Vec<PathBuf> {
    ["20260101T000000Z", "20260102T000000Z", "20260103T000000Z"]
        .iter()
        .map(|timestamp| {
            let path = log_dir.join(format!("{}{timestamp}.jsonl", config.log_prefix));
            std::fs::write(&path, "").unwrap();
            path
        })
        .collect()
}

#[test]
fn state_explain_node_fallback_record_preserves_service_id() {
    let record = StateExplainRecord::fallback_from_node(StateExplainNode {
        node_unique_id: "model.jaffle_shop.orders".to_string(),
        node_name: "orders".to_string(),
        node_info: StateExplainNodeInfo::default(),
        execution_decision_id: Some("decision-1".to_string()),
    });

    assert_eq!(record.node_unique_id, "model.jaffle_shop.orders");
    assert_eq!(record.execution_decision_id.as_deref(), Some("decision-1"));
    assert_eq!(record.status, StateExplainStatus::Unknown);
    assert_eq!(record.reason, "dbt State explain details unavailable");
}

#[test]
fn structured_fallback_renders_local_details_only_when_verbose() {
    let record = StateExplainRecord::fallback_from_node(StateExplainNode {
        node_unique_id: "model.jaffle_shop.orders".to_string(),
        node_name: "orders".to_string(),
        node_info: StateExplainNodeInfo {
            fqn: "\"db\".\"analytics\".\"orders\"".to_string(),
            node_resource_type: "model".to_string(),
            is_view: false,
            is_table: true,
            is_ephemeral: false,
            is_incremental_or_snapshot: true,
            is_full_refresh: true,
            dev_clone: Some(StateExplainDevClone {
                source_table_fqn: "\"db\".\"prod\".\"orders\"".to_string(),
                target_table_fqn: "\"db\".\"analytics\".\"orders\"".to_string(),
            }),
            deferrals: std::collections::HashMap::from([
                ("upstream_b".to_string(), "prod_b".to_string()),
                ("upstream_a".to_string(), "prod_a".to_string()),
            ]),
        },
        execution_decision_id: None,
    });

    let normal = record.render(false);
    assert!(!normal.contains("resource type:"));
    assert!(!normal.contains("dev clone:"));

    let verbose = record.render(true);
    assert!(verbose.contains("  - name: orders"));
    assert!(verbose.contains(r#"  - relation: "db"."analytics"."orders""#));
    assert!(verbose.contains("  - resource type: model"));
    assert!(verbose.contains("  - materialization: incremental"));
    assert!(verbose.contains("  - full refresh: true"));
    assert!(
        verbose.contains(r#"  - dev clone: "db"."prod"."orders" -> "db"."analytics"."orders""#)
    );
    assert!(
        verbose.find("upstream_a -> prod_a").unwrap()
            < verbose.find("upstream_b -> prod_b").unwrap()
    );
}

#[test]
fn structured_fallback_labels_ephemeral_node_without_a_relation() {
    let record = StateExplainRecord::fallback_from_node(StateExplainNode {
        node_unique_id: "model.jaffle_shop.ephemeral_test".to_string(),
        node_name: "ephemeral_test".to_string(),
        node_info: StateExplainNodeInfo {
            node_resource_type: "model".to_string(),
            is_table: false,
            is_ephemeral: true,
            ..Default::default()
        },
        execution_decision_id: None,
    });

    let verbose = record.render(true);

    assert!(verbose.contains("  - materialization: ephemeral"));
    // Ephemeral models are inlined, so there is no relation to report.
    assert!(!verbose.contains("relation:"));
}

#[test]
fn sort_explain_records_orders_by_unique_id() {
    let mut records = vec![
        explain_record("model.jaffle_shop.stg_orders"),
        explain_record("model.jaffle_shop.customers"),
        explain_record("model.jaffle_shop.orders"),
    ];

    sort_explain_records(&mut records);

    let ordered: Vec<_> = records
        .iter()
        .map(|record| record.node_unique_id.as_str())
        .collect();
    assert_eq!(
        ordered,
        [
            "model.jaffle_shop.customers",
            "model.jaffle_shop.orders",
            "model.jaffle_shop.stg_orders"
        ]
    );
}

fn explain_record(node_unique_id: &str) -> StateExplainRecord {
    StateExplainRecord {
        version: STATE_EXPLAIN_RECORD_VERSION,
        node_unique_id: node_unique_id.to_string(),
        execution_decision_id: None,
        status: StateExplainStatus::Unknown,
        reason: "dbt State explain details unavailable".to_string(),
        details: Vec::new(),
    }
}

#[test]
fn fallback_records_from_log_keeps_node_order() {
    let log = StateExplainLog {
        run_start: Some(StateExplainRunStart {
            start_timestamp_utc: "2026-06-02T00:00:00Z".to_string(),
            run_config: StateExplainRunConfig::default(),
        }),
        nodes: vec![
            StateExplainNode {
                node_unique_id: "model.pkg.orders".to_string(),
                node_name: "orders".to_string(),
                node_info: StateExplainNodeInfo::default(),
                execution_decision_id: Some("decision-1".to_string()),
            },
            StateExplainNode {
                node_unique_id: "model.pkg.customers".to_string(),
                node_name: "customers".to_string(),
                node_info: StateExplainNodeInfo::default(),
                execution_decision_id: None,
            },
        ],
    };

    let records = fallback_records_from_log(log).unwrap();

    assert_eq!(records[0].node_unique_id, "model.pkg.orders");
    assert_eq!(records[1].node_unique_id, "model.pkg.customers");
    assert_eq!(execution_decision_ids(&records), ["decision-1"]);
}

#[test]
fn read_records_for_state_explain_requires_structured_run_start() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("state-explain.jsonl");
    std::fs::write(&path, sample_node_json()).unwrap();

    let err = read_records_for_state_explain(&path)
        .unwrap_err()
        .to_string();

    assert!(err.contains("Log file does not contain a run start entry"));
}

#[test]
fn read_records_for_state_explain_falls_back_to_flat_records() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("run-cache.jsonl");
    std::fs::write(
            &path,
            r#"{"version":1,"node_unique_id":"model.pkg.orders","execution_decision_id":"decision-1","status":"hit","reason":"fingerprint matched"}"#,
        )
        .unwrap();

    let records = read_records_for_state_explain(&path).unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, StateExplainStatus::Hit);
    assert_eq!(execution_decision_ids(&records), ["decision-1"]);
}

#[test]
fn read_records_for_state_explain_keeps_structured_errors() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("state-explain.jsonl");
    std::fs::write(
        &path,
        format!(
            "{}\n{}",
            sample_run_start_json(),
            r#"{"version":1,"node_unique_id":"model.pkg.orders","status":"hit","reason":"ok"}"#
        ),
    )
    .unwrap();

    let err = read_records_for_state_explain(&path)
        .unwrap_err()
        .to_string();

    assert!(err.contains("state-explain.jsonl:2"));
}

#[test]
fn filter_explain_records_matches_unique_id_name_and_wildcard() {
    let by_unique_id =
        filter_explain_records(sample_records(), &select_options("model.pkg.orders")).unwrap();
    let by_fqn = filter_explain_records(sample_records(), &select_options("pkg.orders")).unwrap();
    let by_name = filter_explain_records(sample_records(), &select_options("customers")).unwrap();
    let by_wildcard = filter_explain_records(sample_records(), &select_options("*ers")).unwrap();
    let by_fqn_wildcard =
        filter_explain_records(sample_records(), &select_options("pkg.*ers")).unwrap();

    assert_eq!(by_unique_id[0].node_unique_id, "model.pkg.orders");
    assert_eq!(by_fqn[0].node_unique_id, "model.pkg.orders");
    assert_eq!(by_name[0].node_unique_id, "model.pkg.customers");
    assert_eq!(by_wildcard.len(), 2);
    assert_eq!(by_fqn_wildcard.len(), 2);
}

#[test]
fn filter_explain_records_matches_fqn_method_selectors() {
    let by_unique_id =
        filter_explain_records(sample_records(), &select_options("fqn:model.pkg.orders")).unwrap();
    let by_fqn =
        filter_explain_records(sample_records(), &select_options("fqn:pkg.orders")).unwrap();
    let by_name =
        filter_explain_records(sample_records(), &select_options("fqn:customers")).unwrap();
    let by_wildcard =
        filter_explain_records(sample_records(), &select_options("fqn:*ers")).unwrap();
    let by_fqn_wildcard =
        filter_explain_records(sample_records(), &select_options("fqn:pkg.*ers")).unwrap();

    assert_eq!(by_unique_id[0].node_unique_id, "model.pkg.orders");
    assert_eq!(by_fqn[0].node_unique_id, "model.pkg.orders");
    assert_eq!(by_name[0].node_unique_id, "model.pkg.customers");
    assert_eq!(by_wildcard.len(), 2);
    assert_eq!(by_fqn_wildcard.len(), 2);
}

#[test]
fn filter_explain_records_errors_for_unsupported_select_methods() {
    let err = filter_explain_records(sample_records(), &select_options("tag:orders"))
        .unwrap_err()
        .to_string();

    assert!(err.contains("does not support select selector 'tag:orders'"));
}

#[test]
fn filter_explain_records_errors_for_unsupported_exclude_methods() {
    let options = StateExplainOptions {
        exclude: Some(vec!["tag:orders".to_string()]),
        ..Default::default()
    };
    let err = filter_explain_records(sample_records(), &options)
        .unwrap_err()
        .to_string();

    assert!(err.contains("does not support exclude selector 'tag:orders'"));
}

#[test]
fn filter_explain_records_honors_exclude_after_select() {
    let records = filter_explain_records(
        sample_records(),
        &select_exclude_options("*", "fqn:customers"),
    )
    .unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].node_unique_id, "model.pkg.orders");
}

#[test]
fn filter_explain_records_honors_exclude_without_select() {
    let options = StateExplainOptions {
        exclude: Some(vec!["customers".to_string()]),
        ..Default::default()
    };

    let records = filter_explain_records(sample_records(), &options).unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].node_unique_id, "model.pkg.orders");
}

#[test]
fn render_explain_records_reports_selected_empty_input() {
    let output = render_explain_records(&[], &select_options("missing"));

    assert_eq!(output, "No nodes found matching 'missing'");
}

#[test]
fn filter_explain_records_removes_unselected_decision_ids() {
    let mut records = sample_records();
    records[0].execution_decision_id = Some("decision-1".to_string());
    records[1].execution_decision_id = Some("decision-2".to_string());
    let records = filter_explain_records(records, &select_options("customers")).unwrap();

    assert_eq!(execution_decision_ids(&records), ["decision-2"]);
}

#[test]
fn resolve_log_file_returns_none_without_implicit_logs() {
    let temp_dir = tempfile::tempdir().unwrap();
    let options = StateExplainOptions {
        project_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    };

    let output = resolve_log_file(&options, &RunCacheServiceConfig::disabled()).unwrap();

    assert!(output.is_none());
}

#[test]
fn resolve_log_file_errors_for_missing_explicit_log() {
    let temp_dir = tempfile::tempdir().unwrap();
    let options = StateExplainOptions {
        project_dir: temp_dir.path().to_path_buf(),
        log_file: Some("missing.jsonl".into()),
        ..Default::default()
    };

    let err = resolve_log_file(&options, &RunCacheServiceConfig::disabled()).unwrap_err();

    assert!(err.to_string().contains("Log file not found"));
}

#[test]
fn state_explain_log_config_reads_only_log_env() {
    let config = state_explain_log_config_from_getter(|name| match name {
        "RUN_CACHE_LOG_DIR_OVERRIDE" => Some("/tmp/state-explain".to_string()),
        "RUN_CACHE_LOG_PREFIX" => Some("fusion_".to_string()),
        "RUN_CACHE_API_CLIENT_TIMEOUT" => Some("not-a-duration".to_string()),
        _ => None,
    });

    assert_eq!(
        config.log_dir_override.as_deref(),
        Some("/tmp/state-explain")
    );
    assert_eq!(config.log_prefix, "fusion_");
}

#[test]
fn render_service_explain_response_renders_tree() {
    let response = GetExplainMessagesResponse {
        messages: vec![ExplainMessageEntry {
            execution_decision_id: "decision-1".to_string(),
            decision: SubmitSqlResultType::ReadyToClone as i32,
            decision_description: "ready to clone".to_string(),
            explain_lines: vec![ExplainLine {
                text: "target table exists".to_string(),
                marker: Some(ExplainMarker::Success as i32),
                badge: Some(ExplainBadge::TargetTableExists as i32),
                children: vec![ExplainLine {
                    text: "upstream unchanged".to_string(),
                    marker: Some(ExplainMarker::Info as i32),
                    badge: None,
                    children: Vec::new(),
                }],
            }],
        }],
    };

    let output = render_service_explain_response(&response);

    assert_eq!(
        output,
        "READY_TO_CLONE decision-1 - ready to clone\n  - target table exists [SUCCESS, TARGET_TABLE_EXISTS]\n    - upstream unchanged [INFO]"
    );
}

#[test]
fn render_service_explain_response_drops_unspecified_line_metadata() {
    let response = GetExplainMessagesResponse {
        messages: vec![ExplainMessageEntry {
            execution_decision_id: "decision-1".to_string(),
            decision: SubmitSqlResultType::Unknown as i32,
            decision_description: "unknown decision type".to_string(),
            explain_lines: vec![ExplainLine {
                text: "no marker".to_string(),
                marker: Some(ExplainMarker::EmUnspecified as i32),
                badge: Some(ExplainBadge::EbUnspecified as i32),
                children: Vec::new(),
            }],
        }],
    };

    let output = render_service_explain_response(&response);

    assert_eq!(
        output,
        "UNKNOWN decision-1 - unknown decision type\n  - no marker"
    );
}

#[test]
fn render_service_explain_response_preserves_unknown_enum_values() {
    let response = GetExplainMessagesResponse {
        messages: vec![ExplainMessageEntry {
            execution_decision_id: "decision-1".to_string(),
            decision: i32::MAX,
            decision_description: "future decision".to_string(),
            explain_lines: vec![ExplainLine {
                text: "future line metadata".to_string(),
                marker: Some(i32::MAX),
                badge: Some(i32::MAX),
                children: Vec::new(),
            }],
        }],
    };

    let output = render_service_explain_response(&response);

    assert_eq!(
        output,
        "UNKNOWN_DECISION(2147483647) decision-1 - future decision\n  - future line metadata [UNKNOWN_MARKER(2147483647), UNKNOWN_BADGE(2147483647)]"
    );
}

#[dbt_runtime::test]
async fn service_explain_response_with_client_fetches_decision_ids() {
    let mut records = sample_records();
    records[0].execution_decision_id = Some("decision-1".to_string());
    records[1].execution_decision_id = Some("decision-2".to_string());
    let client = MockExplainClient {
        response: GetExplainMessagesResponse {
            messages: vec![ExplainMessageEntry {
                execution_decision_id: "decision-1".to_string(),
                decision: SubmitSqlResultType::ReadyToExecute as i32,
                decision_description: "ready".to_string(),
                explain_lines: Vec::new(),
            }],
        },
        requests: Mutex::new(Vec::new()),
    };

    let response = service_explain_response_with_client(&client, &records)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        client.requests.into_inner().unwrap(),
        [["decision-1", "decision-2"]]
    );
    assert_eq!(response.messages.len(), 1);
    assert_eq!(response.messages[0].execution_decision_id, "decision-1");
}

#[dbt_runtime::test]
async fn state_explain_service_path_fetches_selected_records_and_renders_response() {
    let mut records = sample_records();
    records[0].execution_decision_id = Some("decision-1".to_string());
    records[1].execution_decision_id = Some("decision-2".to_string());
    let options = StateExplainOptions {
        manage_state: true,
        ..select_options("orders")
    };
    let records = filter_explain_records(records, &options).unwrap();
    assert!(should_fetch_service_explain(
        &execution_decision_ids(&records),
        &options
    ));
    let client = MockExplainClient {
        response: GetExplainMessagesResponse {
            messages: vec![service_message("decision-1", "service ready")],
        },
        requests: Mutex::new(Vec::new()),
    };

    let service_response = service_explain_response_with_client(&client, &records)
        .await
        .unwrap();
    let output = render_merged_explain_records(&records, service_response.as_ref(), &options);

    assert_eq!(client.requests.into_inner().unwrap(), [["decision-1"]]);
    assert_eq!(output, "READY_TO_EXECUTE model.pkg.orders - service ready");
}

#[test]
fn state_explain_service_path_respects_manage_state_gate() {
    let mut records = sample_records();
    records[0].execution_decision_id = Some("decision-1".to_string());
    let options = StateExplainOptions {
        manage_state: false,
        ..Default::default()
    };

    assert!(!should_fetch_service_explain(
        &execution_decision_ids(&records),
        &options
    ));
}

#[test]
fn state_explain_service_path_ignores_invalid_service_config() {
    let service_config = run_cache_service_config_for_state_explain(|name| match name {
        "RUN_CACHE_API_CLIENT_TIMEOUT" => Some("not-a-duration".to_string()),
        _ => None,
    });

    assert!(service_config.is_none());
}

#[dbt_runtime::test]
async fn service_explain_response_with_client_skips_empty_decision_ids() {
    let records = sample_records();
    let client = MockExplainClient {
        response: GetExplainMessagesResponse {
            messages: Vec::new(),
        },
        requests: Mutex::new(Vec::new()),
    };

    let output = service_explain_response_with_client(&client, &records)
        .await
        .unwrap();

    assert!(output.is_none());
    assert!(client.requests.into_inner().unwrap().is_empty());
}

#[dbt_runtime::test]
async fn service_explain_response_with_client_batches_decision_ids() {
    let records: Vec<_> = (0..=EXPLAIN_MAX_BATCH_SIZE)
        .map(|idx| StateExplainRecord {
            version: STATE_EXPLAIN_RECORD_VERSION,
            node_unique_id: format!("model.pkg.model_{idx}"),
            execution_decision_id: Some(format!("decision-{idx}")),
            status: StateExplainStatus::Hit,
            reason: "ok".to_string(),
            details: Vec::new(),
        })
        .collect();
    let client = MockExplainClient {
        response: GetExplainMessagesResponse {
            messages: Vec::new(),
        },
        requests: Mutex::new(Vec::new()),
    };

    service_explain_response_with_client(&client, &records)
        .await
        .unwrap();
    let requests = client.requests.into_inner().unwrap();

    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].len(), EXPLAIN_MAX_BATCH_SIZE);
    assert_eq!(requests[1], ["decision-1000"]);
}

fn sample_records() -> Vec<StateExplainRecord> {
    vec![
        StateExplainRecord {
            version: STATE_EXPLAIN_RECORD_VERSION,
            node_unique_id: "model.pkg.orders".to_string(),
            execution_decision_id: None,
            status: StateExplainStatus::Hit,
            reason: "fingerprint matched".to_string(),
            details: vec!["compiled SQL matched".to_string()],
        },
        StateExplainRecord {
            version: STATE_EXPLAIN_RECORD_VERSION,
            node_unique_id: "model.pkg.customers".to_string(),
            execution_decision_id: None,
            status: StateExplainStatus::Miss,
            reason: "relation changed".to_string(),
            details: Vec::new(),
        },
    ]
}

fn select_options(selector: &str) -> StateExplainOptions {
    StateExplainOptions {
        select: Some(vec![selector.to_string()]),
        ..Default::default()
    }
}

fn select_exclude_options(selector: &str, exclude: &str) -> StateExplainOptions {
    StateExplainOptions {
        select: Some(vec![selector.to_string()]),
        exclude: Some(vec![exclude.to_string()]),
        ..Default::default()
    }
}

fn sample_state_explain_log_jsonl() -> String {
    format!("{}\n{}", sample_run_start_json(), sample_node_json())
}

fn sample_run_start_json() -> String {
    serde_json::to_string(&StateExplainLogRecord::RunStart(StateExplainRunStart {
        start_timestamp_utc: "2026-06-02T00:00:00Z".to_string(),
        run_config: StateExplainRunConfig {
            defer_to_target: "prod".to_string(),
            profile_name: "jaffle_shop".to_string(),
            target_name: "dev".to_string(),
            ..Default::default()
        },
    }))
    .unwrap()
}

fn sample_node_json() -> String {
    serde_json::to_string(&StateExplainLogRecord::Node(StateExplainNode {
        node_unique_id: "model.jaffle_shop.orders".to_string(),
        node_name: "orders".to_string(),
        node_info: StateExplainNodeInfo::default(),
        execution_decision_id: Some("decision-1".to_string()),
    }))
    .unwrap()
}

fn service_message(id: &str, description: &str) -> ExplainMessageEntry {
    ExplainMessageEntry {
        execution_decision_id: id.to_string(),
        decision: SubmitSqlResultType::ReadyToExecute as i32,
        decision_description: description.to_string(),
        explain_lines: Vec::new(),
    }
}

struct MockExplainClient {
    response: GetExplainMessagesResponse,
    requests: Mutex<Vec<Vec<String>>>,
}

#[async_trait::async_trait]
impl RunCacheServiceClient for MockExplainClient {
    async fn validate_client_version(
        &self,
    ) -> Result<crate::service_client::ClientVersionStatus, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }

    async fn submit_enriched_sql(
        &self,
        _request: crate::proto::query_cache::SubmitEnrichedSqlRequest,
    ) -> Result<crate::proto::query_cache::SubmitSqlResponse, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }

    async fn submit_values(
        &self,
        _request: crate::proto::query_cache::SubmitValuesRequest,
    ) -> Result<crate::proto::query_cache::SubmitSqlResponse, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }

    async fn confirm_execution(
        &self,
        _request: crate::proto::query_cache::ConfirmExecutionRequest,
    ) -> Result<crate::proto::query_cache::ConfirmExecutionResponse, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }

    async fn get_explain_messages(
        &self,
        request: GetExplainMessagesRequest,
    ) -> Result<GetExplainMessagesResponse, RunCacheServiceError> {
        self.requests
            .lock()
            .unwrap()
            .push(request.execution_decision_ids);
        Ok(self.response.clone())
    }
}
