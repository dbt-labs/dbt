use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;
use crate::{
    constants::DBT_TARGET_DIR_NAME,
    io_args::FsCommand,
    tracing::{
        emit::{
            create_debug_span, create_info_span, create_root_info_span, emit_debug_event,
            emit_error_event, emit_info_event,
        },
        layers::json_compat_layer::build_json_compat_layer,
        span_info::{record_span_status_from_attrs, update_span_attrs},
    },
};
use dbt_telemetry::{
    ArtifactType, ArtifactWritten, CompiledCode, CompiledCodeInline, DepsAddPackage,
    DepsPackageInstalled, ExecutionPhase, HookOutcome, HookProcessed, HookType, Invocation,
    InvocationEvalArgs, ListItemOutput, ListOutputFormat, LogMessage, NodeEvaluated,
    NodeMaterialization, NodeOutcome, NodeOutcomeDetail, NodeProcessed, NodeSkipReason,
    NodeSkipUpstreamDetail, NodeType, PackageType, ProgressMessage, QueryExecuted, QueryOutcome,
    ShowDataOutput, ShowDataOutputFormat, ShowResult, ShowResultOutputFormat,
    SourceFreshnessDetail, SourceFreshnessOutcome, UserLogMessage, node_processed,
    update_dbt_core_event_code_for_node_processed_end,
};
use dbt_tracing::test_support::mocks::TestWriter;
use dbt_tracing::test_support::mocks::{MockDynSpanEvent, test_data_layer};
use dbt_tracing::{SeverityNumber, TelemetryOutputFlags};
use serde_json::{Value, json};
use tracing::level_filters::LevelFilter;
use uuid::Uuid;

const TEST_DATABASE: &str = "SOME_DB";
const TEST_SCHEMA: &str = "SOME_SCHEMA";
const TEST_ALIAS: &str = "my_model";
const TEST_RELATION_NAME: &str = "SOME_DB.SOME_SCHEMA.my_model";
const TEST_REL_PATH: &str = "models/my_model.sql";
const TEST_NODE_CHECKSUM: &str = "abcdef1234567890";
const TEST_NODE_INDEX: u32 = 3;
const TEST_NODE_COUNT_TOTAL: u32 = 7;

/// Helper function to setup tracing with json compat layer and execute a closure
/// that produces events. Returns the collected JSON output lines.
/// Creates a dummy root span and executes test_fn within its scope.
fn with_json_compat_layer<F>(invocation_id: Uuid, command: FsCommand, test_fn: F) -> Vec<Value>
where
    F: FnOnce(),
{
    let trace_id = rand::random::<u128>();
    let writer = TestWriter::non_terminal();

    let json_layer = build_json_compat_layer(
        writer.clone(),
        LevelFilter::TRACE,
        invocation_id,
        command,
        "dbt-fusion",
    );

    let subscriber = create_tracing_subcriber_with_layer(
        LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(json_layer),
        ),
    );

    tracing::subscriber::with_default(subscriber, || {
        // Create root Invocation span and enter its scope
        let root_span = create_root_info_span(MockDynSpanEvent {
            name: "not used".to_string(),
            // this will ensure some root is present
            flags: TelemetryOutputFlags::OUTPUT_ALL,
            ..Default::default()
        });
        let _guard = root_span.enter();

        // Execute test function within the root span scope
        test_fn();
    });

    // Parse JSON lines
    writer
        .get_lines()
        .into_iter()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            serde_json::from_str(trimmed).ok()
        })
        .collect()
}

/// Scrubs unstable fields (pid, thread, ts) from JSON output for comparison
fn scrub_json(mut value: Value, scrub_data_keys: &[&str]) -> Value {
    if let Some(info) = value.get_mut("info") {
        if let Some(obj) = info.as_object_mut() {
            obj.remove("pid");
            obj.remove("thread");
            obj.remove("ts");
        }
    }

    // Also scrub keys in data if present, supporting nested paths like "key.subkey.subsubkey"
    if let Some(data) = value.get_mut("data") {
        for &key in scrub_data_keys {
            let parts: Vec<&str> = key.split('.').collect();

            // Navigate to the nested value
            let mut current = &mut *data;

            for (i, &part) in parts.iter().enumerate() {
                if i == parts.len() - 1 {
                    // Last part - this is what we want to scrub
                    if let Some(v) = current.get_mut(part) {
                        *v = Value::String(format!("<redacted::{}>", part));
                    }
                } else {
                    // Intermediate part - navigate deeper
                    if let Some(next) = current.get_mut(part) {
                        current = next;
                    } else {
                        break;
                    }
                }
            }
        }
    }

    value
}

/// Scrubs a list of JSON values
fn scrub_json_list(values: Vec<Value>, scrub_data_keys: &[&str]) -> Vec<Value> {
    values
        .into_iter()
        .map(|v| scrub_json(v, scrub_data_keys))
        .collect()
}

/// Helper to test events and compare against expected JSON outputs
fn test_events<F>(
    test_name: &str,
    invocation_id: Uuid,
    event_fn: F,
    scrub_data_keys: &[&str],
    expected: Vec<Value>,
) where
    F: FnOnce(),
{
    test_events_with_msg_fields_to_ignore_for_command(
        test_name,
        invocation_id,
        FsCommand::Build,
        event_fn,
        scrub_data_keys,
        expected,
        &[],
    )
}

/// Helper to test events and compare against expected JSON outputs, scrubbing all msg fields
fn test_events_scrub_all_msg<F>(
    test_name: &str,
    invocation_id: Uuid,
    event_fn: F,
    scrub_data_keys: &[&str],
    expected: Vec<Value>,
) where
    F: FnOnce(),
{
    let events_len = expected.len();

    test_events_with_msg_fields_to_ignore_for_command(
        test_name,
        invocation_id,
        FsCommand::Build,
        event_fn,
        scrub_data_keys,
        expected,
        &(0..events_len).collect::<Vec<usize>>(), // Ignore all msg fields
    )
}

/// Helper to test events and compare against expected JSON, optionally ignoring msg fields at specific indices
fn test_events_with_msg_fields_to_ignore<F>(
    test_name: &str,
    invocation_id: Uuid,
    event_fn: F,
    scrub_data_keys: &[&str],
    expected: Vec<Value>,
    ignore_msg_at_indices: &[usize],
) where
    F: FnOnce(),
{
    test_events_with_msg_fields_to_ignore_for_command(
        test_name,
        invocation_id,
        FsCommand::Build,
        event_fn,
        scrub_data_keys,
        expected,
        ignore_msg_at_indices,
    )
}

/// Helper to test events and compare against expected JSON, optionally ignoring msg fields at
/// specific indices, using an explicit invocation command for json compat layer configuration.
fn test_events_with_msg_fields_to_ignore_for_command<F>(
    test_name: &str,
    invocation_id: Uuid,
    command: FsCommand,
    event_fn: F,
    scrub_data_keys: &[&str],
    mut expected: Vec<Value>,
    ignore_msg_at_indices: &[usize],
) where
    F: FnOnce(),
{
    let mut actual_outputs = with_json_compat_layer(invocation_id, command, event_fn);

    assert_eq!(
        actual_outputs.len(),
        expected.len(),
        "[{}] Expected {} outputs, got {}",
        test_name,
        expected.len(),
        actual_outputs.len()
    );

    // Sort both by name to ensure consistent order
    let sort_by_name = |a: &Value, b: &Value| {
        let a_name = a
            .get("info")
            .and_then(|i| i.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("");
        let b_name = b
            .get("info")
            .and_then(|i| i.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("");
        a_name.cmp(b_name)
    };
    actual_outputs.sort_by(sort_by_name);
    expected.sort_by(sort_by_name);

    let mut actual_scrubbed = scrub_json_list(actual_outputs, scrub_data_keys);
    let mut expected_scrubbed = scrub_json_list(expected, scrub_data_keys);

    // Remove msg fields at specified indices
    for &idx in ignore_msg_at_indices {
        if idx < actual_scrubbed.len() {
            if let Some(info) = actual_scrubbed[idx].get_mut("info") {
                if let Some(obj) = info.as_object_mut() {
                    obj.remove("msg");
                }
            }
        }
        if idx < expected_scrubbed.len() {
            if let Some(info) = expected_scrubbed[idx].get_mut("info") {
                if let Some(obj) = info.as_object_mut() {
                    obj.remove("msg");
                }
            }
        }
    }

    assert_eq!(
        actual_scrubbed, expected_scrubbed,
        "[{}] JSON outputs mismatch",
        test_name
    );
}

#[test]
fn test_query_executed() {
    let invocation_id = Uuid::new_v4();

    test_events_scrub_all_msg(
        "QueryExecuted",
        invocation_id,
        || {
            // With unique_id
            let _span = create_info_span(QueryExecuted {
                sql: "SELECT 1".to_string(),
                query_outcome: QueryOutcome::Success as i32,
                unique_id: Some("model.my_project.my_model".to_string()),
                dbt_core_event_code: "Q001".to_string(),
                ..Default::default()
            });
            // Without unique_id
            let _span = create_info_span(QueryExecuted {
                sql: "SELECT 2".to_string(),
                query_outcome: QueryOutcome::Success as i32,
                unique_id: None,
                dbt_core_event_code: "Q002".to_string(),
                ..Default::default()
            });
        },
        &[],
        vec![
            // Reverse order due to span closing order
            json!({
                "info": {
                    "category": "",
                    "code": "Q002",
                    "invocation_id": invocation_id.to_string(),
                    "name": "SQLQuery",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "sql": "SELECT 2"
                }
            }),
            json!({
                "info": {
                    "category": "",
                    "code": "Q001",
                    "invocation_id": invocation_id.to_string(),
                    "name": "SQLQuery",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "sql": "SELECT 1",
                    "node_info": {
                        "unique_id": "model.my_project.my_model"
                    }
                }
            }),
        ],
    );
}

#[test]
fn test_invocation_span() {
    let invocation_id = Uuid::new_v4();

    test_events_with_msg_fields_to_ignore(
        "Ivocation (MainReportVersion, MainReportArgs, CommandCompleted)",
        invocation_id,
        || {
            let _span = create_info_span(Invocation {
                invocation_id: invocation_id.to_string(),
                raw_command: "dbt build --log-format json".to_string(),
                eval_args: Some(InvocationEvalArgs {
                    command: "build".to_string(),
                    debug: Some(false),
                    indirect_selection: Some("eager".to_string()),
                    log_format: Some("json".to_string()),
                    log_path: Some("/path/to/logs".to_string()),
                    profiles_dir: Some("/path/to/profiles".to_string()),
                    quiet: Some(false),
                    target_path: Some(DBT_TARGET_DIR_NAME.to_string()),
                    write_json: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            });
        },
        &["elapsed", "completed_at"],
        vec![
            // CommandCompleted
            json!({
                "info": {
                    "category": "",
                    "code": "Q039",
                    "invocation_id": invocation_id.to_string(),
                    "name": "CommandCompleted",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "elapsed": "<redacted::elapsed>",
                    "success": true,
                    "completed_at": "<redacted::completed_at>"
                }
            }),
            // MainReportArgs
            json!({
                "info": {
                    "category": "",
                    "code": "A002",
                    "invocation_id": invocation_id.to_string(),
                    "name": "MainReportArgs",
                    "level": "debug",
                    "msg": "",
                    "extra": {}
                },
                "data": {
                    "args": {
                        "debug": "false",
                        "indirect_selection": "eager",
                        "invocation_command": "dbt build --log-format json",
                        "log_format": "json",
                        "log_path": "/path/to/logs",
                        "profiles_dir": "/path/to/profiles",
                        "quiet": "false",
                        "target_path": "target",
                        "write_json": "true"
                    }
                }
            }),
            // MainReportVersion
            json!({
                "info": {
                    "category": "",
                    "code": "A001",
                    "invocation_id": invocation_id.to_string(),
                    "name": "MainReportVersion",
                    "level": "info",
                    "msg": format!("Running with dbt-fusion={}", env!("CARGO_PKG_VERSION")),
                    "extra": {}
                },
                "data": {
                    "log_version": 3,
                    "version": format!("={}", env!("CARGO_PKG_VERSION"))
                }
            }),
        ],
        // do not check msg field for CommandCompleted, has duration
        // and MainReportArgs which is log and not critical here
        &[0, 1],
    );
}

#[test]
fn test_artifact_written() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "ArtifactWritten",
        invocation_id,
        || {
            // Test Manifest type (should map to "WritableManifest")
            let _span = create_info_span(ArtifactWritten {
                relative_path: "target/manifest.json".to_string(),
                artifact_type: ArtifactType::Manifest as i32,
            });

            // Test other artifact types (should use as_str_name())
            let _span = create_info_span(ArtifactWritten {
                relative_path: "target/catalog.json".to_string(),
                artifact_type: ArtifactType::Catalog as i32,
            });
        },
        &[],
        vec![
            json!({
                "info": {
                    "category": "",
                    "code": "P001",
                    "invocation_id": invocation_id.to_string(),
                    "name": "ArtifactWritten",
                    "level": "info",
                    "msg": "Artifact written to target/catalog.json",
                    "extra": {}
                },
                "data": {
                    "artifact_path": "target/catalog.json",
                    "artifact_type": "ARTIFACT_TYPE_CATALOG"
                }
            }),
            json!({
                "info": {
                    "category": "",
                    "code": "P001",
                    "invocation_id": invocation_id.to_string(),
                    "name": "ArtifactWritten",
                    "level": "info",
                    "msg": "Artifact written to target/manifest.json",
                    "extra": {}
                },
                "data": {
                    "artifact_path": "target/manifest.json",
                    "artifact_type": "WritableManifest"
                }
            }),
        ],
    );
}

#[test]
fn test_log_message() {
    let invocation_id = Uuid::new_v4();

    test_events_with_msg_fields_to_ignore(
        "LogMessage",
        invocation_id,
        || {
            emit_error_event(
                LogMessage {
                    code: Some(1234),
                    original_severity_number: SeverityNumber::Error as i32,
                    original_severity_text: "ERROR".to_string(),
                    ..Default::default()
                },
                Some("Test error message"),
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "1234",
                "invocation_id": invocation_id.to_string(),
                "name": "Generic",
                "level": "error",
                "extra": {},
                "msg": "[error]: Test error message"
            },
            "data": {}
        })],
        &[],
    );
}

#[test]
fn test_log_message_with_code_name() {
    let invocation_id = Uuid::new_v4();

    test_events_with_msg_fields_to_ignore(
        "LogMessage",
        invocation_id,
        || {
            emit_error_event(
                LogMessage {
                    code: Some(1060),
                    code_name: Some("UnusedConfigKey".to_string()),
                    original_severity_number: SeverityNumber::Error as i32,
                    original_severity_text: "ERROR".to_string(),
                    ..Default::default()
                },
                Some("Test error message"),
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "1060",
                "invocation_id": invocation_id.to_string(),
                "name": "UnusedConfigKey",
                "level": "error",
                "extra": {},
                "msg": "[error] [UnusedConfigKey (dbt1060)]: Test error message"
            },
            "data": {}
        })],
        &[],
    );
}

#[test]
fn test_log_message_with_dbt_core_event_code() {
    let invocation_id = Uuid::new_v4();

    test_events_with_msg_fields_to_ignore(
        "LogMessage",
        invocation_id,
        || {
            emit_error_event(
                LogMessage {
                    code: Some(1060),
                    code_name: Some("UnusedConfigKey".to_string()),
                    dbt_core_event_code: Some("Z048".to_string()),
                    original_severity_number: SeverityNumber::Error as i32,
                    original_severity_text: "ERROR".to_string(),
                    ..Default::default()
                },
                Some("Test error message"),
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "Z048",
                "invocation_id": invocation_id.to_string(),
                "name": "UnusedConfigKey",
                "level": "error",
                "extra": {},
                "msg": "[error] [UnusedConfigKey (dbt1060)]: Test error message"
            },
            "data": {}
        })],
        &[],
    );
}

#[test]
fn test_user_log_message_print() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "UserLogMessage (print)",
        invocation_id,
        || {
            emit_info_event(
                UserLogMessage {
                    is_print: true,
                    ..Default::default()
                },
                Some("Hello from print()"),
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "Z052",
                "invocation_id": invocation_id.to_string(),
                "name": "PrintEvent",
                "msg": "Hello from print()",
                "level": "info",
                "extra": {}
            },
            "data": {
                "msg": "Hello from print()"
            }
        })],
    );
}

#[test]
fn test_user_log_message_jinja_log_info() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "UserLogMessage (log with info=true)",
        invocation_id,
        || {
            emit_info_event(
                UserLogMessage {
                    is_print: false,
                    ..Default::default()
                },
                Some("Log info message"),
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "I062",
                "invocation_id": invocation_id.to_string(),
                "name": "JinjaLogInfo",
                "msg": "Log info message",
                "level": "info",
                "extra": {}
            },
            "data": {
                "msg": "Log info message"
            }
        })],
    );
}

#[test]
fn test_user_log_message_jinja_debug_info() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "UserLogMessage (log with info=false)",
        invocation_id,
        || {
            emit_debug_event(
                UserLogMessage {
                    is_print: false,
                    ..Default::default()
                },
                Some("Log debug message"),
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "I063",
                "invocation_id": invocation_id.to_string(),
                "name": "JinjaDebugInfo",
                "msg": "Log debug message",
                "level": "debug",
                "extra": {}
            },
            "data": {
                "msg": "Log debug message"
            }
        })],
    );
}

#[test]
fn test_list_item_output() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "ListItemOutput",
        invocation_id,
        || {
            emit_info_event(
                ListItemOutput {
                    content: "model.my_project.my_model".to_string(),
                    output_format: ListOutputFormat::Name as i32,
                    unique_id: Some("model.my_project.my_model".to_string()),
                },
                Some("model.my_project.my_model"),
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "Z052",
                "invocation_id": invocation_id.to_string(),
                "name": "PrintEvent",
                "msg": "model.my_project.my_model",
                "level": "info",
                "extra": {}
            },
            "data": {
                "msg": "model.my_project.my_model"
            }
        })],
    );
}

#[test]
fn test_show_data_output() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "ShowDataOutput",
        invocation_id,
        || {
            emit_info_event(
                ShowDataOutput {
                    node_name: TEST_ALIAS.to_string(),
                    content: "col1\tcol2\nval1\tval2".to_string(),
                    is_inline: false,
                    output_format: ShowDataOutputFormat::Tsv as i32,
                    unique_id: Some("model.my_project.my_model".to_string()),
                    columns: vec!["col1".to_string(), "col2".to_string()],
                    dbt_core_event_code: "Q041".to_string(),
                },
                Some("col1\tcol2\nval1\tval2"),
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "Q041",
                "invocation_id": invocation_id.to_string(),
                "name": "ShowNode",
                "msg": "col1\tcol2\nval1\tval2",
                "level": "info",
                "extra": {}
            },
            "data": {
                "node_name": TEST_ALIAS,
                "preview": "col1\tcol2\nval1\tval2",
                "is_inline": false,
                "output_format": "tsv",
                "unique_id": "model.my_project.my_model",
                "columns": ["col1", "col2"]
            }
        })],
    );
}

#[test]
fn test_show_data_output_inline_without_unique_id() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "ShowDataOutput (inline without unique_id)",
        invocation_id,
        || {
            emit_info_event(
                ShowDataOutput {
                    node_name: "inline_query".to_string(),
                    content: "result".to_string(),
                    is_inline: true,
                    output_format: ShowDataOutputFormat::Text as i32,
                    unique_id: None,
                    columns: vec![],
                    dbt_core_event_code: "Q041".to_string(),
                },
                Some("result"),
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "Q041",
                "invocation_id": invocation_id.to_string(),
                "name": "ShowNode",
                "msg": "result",
                "level": "info",
                "extra": {}
            },
            "data": {
                "node_name": "inline_query",
                "preview": "result",
                "is_inline": true,
                "output_format": "text",
                "unique_id": "sql_operation.inline_query",
                "columns": []
            }
        })],
    );
}

#[test]
fn test_node_events() {
    let invocation_id = Uuid::new_v4();
    let test_node_type = NodeType::Test;
    let test_unique_id = format!(
        "{}.my_project.{}",
        test_node_type.as_static_ref(),
        TEST_ALIAS
    );
    let test_upstream_id = format!(
        "{}.my_project.upstream_model",
        NodeType::Model.as_static_ref()
    );
    let test_custom_mat = "test_mat".to_string();

    let create_test_node_evaluated_span = |phase: ExecutionPhase| -> tracing::Span {
        create_debug_span(NodeEvaluated::start(
            test_unique_id.clone(),
            TEST_ALIAS.to_string(),
            Some(TEST_DATABASE.to_string()),
            Some(TEST_SCHEMA.to_string()),
            Some(TEST_ALIAS.to_string()),
            Some(NodeMaterialization::Custom),
            Some(test_custom_mat.clone()),
            test_node_type,
            phase,
            TEST_REL_PATH.to_string(),
            Some(1),
            Some(0),
            TEST_NODE_CHECKSUM.to_string(),
        ))
    };

    let setup = || {
        let mut np_event = NodeProcessed::start(
            test_unique_id.clone(),
            TEST_ALIAS.to_string(),
            Some(TEST_DATABASE.to_string()),
            Some(TEST_SCHEMA.to_string()),
            Some(TEST_ALIAS.to_string()),
            Some(NodeMaterialization::Custom),
            Some(test_custom_mat.clone()),
            test_node_type,
            Some(ExecutionPhase::Run),
            TEST_REL_PATH.to_string(),
            Some(1),
            Some(0),
            TEST_NODE_CHECKSUM.to_string(),
            true,
            None,
        );
        // The span builder normally stamps these on; set them here so the emitted
        // `index`/`total` are asserted against real values rather than the 0 fallback.
        np_event.node_index = Some(TEST_NODE_INDEX);
        np_event.node_count_total = Some(TEST_NODE_COUNT_TOTAL);

        let np_span = create_info_span(np_event);

        // create NodeEvaluated spans for different phases that follow from NodeProcessed
        let render_ne_span = create_test_node_evaluated_span(ExecutionPhase::Render);
        render_ne_span.follows_from(np_span.clone());

        // Record success on render
        record_span_status_from_attrs(&render_ne_span, |attrs| {
            if let Some(ev) = attrs.downcast_mut::<NodeEvaluated>() {
                ev.set_node_outcome(NodeOutcome::Success);
            }
        });

        drop(render_ne_span);

        // Now analyze phase
        let analyze_ne_span = create_test_node_evaluated_span(ExecutionPhase::Analyze);
        analyze_ne_span.follows_from(np_span.clone());

        // Record success on analyze
        record_span_status_from_attrs(&analyze_ne_span, |attrs| {
            if let Some(ev) = attrs.downcast_mut::<NodeEvaluated>() {
                ev.set_node_outcome(NodeOutcome::Success);
            }
        });
        drop(analyze_ne_span);

        // Finally run phase with skip result
        let run_ne_span = create_test_node_evaluated_span(ExecutionPhase::Run);
        run_ne_span.follows_from(np_span.clone());

        // Record skip on run
        record_span_status_from_attrs(&run_ne_span, |attrs| {
            if let Some(ev) = attrs.downcast_mut::<NodeEvaluated>() {
                ev.set_node_outcome(NodeOutcome::Skipped);
                ev.set_node_skip_reason(NodeSkipReason::Upstream);
                ev.node_outcome_detail = Some(NodeOutcomeDetail::NodeSkipUpstreamDetail(
                    NodeSkipUpstreamDetail::new(test_upstream_id.clone()),
                ));
            }
        });
        drop(run_ne_span);

        // Record the same skip outcome on NodeProcessed
        record_span_status_from_attrs(&np_span, |attrs| {
            if let Some(ev) = attrs.downcast_mut::<NodeProcessed>() {
                ev.set_node_outcome(NodeOutcome::Skipped);
                ev.set_node_skip_reason(NodeSkipReason::Upstream);
                ev.node_outcome_detail =
                    Some(node_processed::NodeOutcomeDetail::NodeSkipUpstreamDetail(
                        NodeSkipUpstreamDetail::new(test_upstream_id.clone()),
                    ));
            }
        });
    };

    let expected_node_info = |node_status: &str, include_finished: bool| {
        json!({
            "node_path": TEST_REL_PATH,
            "node_name": TEST_ALIAS,
            "unique_id": test_unique_id.clone(),
            "resource_type": test_node_type.as_static_ref(),
            "materialized": test_custom_mat.clone(),
            "node_status": node_status,
            "node_started_at": "<redacted::node_started_at>",
            "node_finished_at": if include_finished { "<redacted::node_finished_at>" } else { "" },
            "node_relation": {
                "database": TEST_DATABASE,
                "schema": TEST_SCHEMA,
                "alias": TEST_ALIAS,
                "relation_name": TEST_RELATION_NAME
            },
            "node_checksum": TEST_NODE_CHECKSUM
        })
    };

    let expected_node_compiling = json!({
        "info": {
            "category": "",
            "code": "Q030",
            "invocation_id": invocation_id.to_string(),
            "name": "NodeCompiling",
            "level": "debug",
            "extra": {},
            "msg": format!("Started rendering test {}.{}", TEST_SCHEMA, TEST_ALIAS),
        },
        "data": {
            "node_info": expected_node_info("compiling", false)
        }
    });

    let expected_node_executing = json!({
        "info": {
            "category": "",
            "code": "Q031",
            "invocation_id": invocation_id.to_string(),
            "name": "NodeExecuting",
            "level": "debug",
            "extra": {},
            "msg": format!("Started running test {}.{}", TEST_SCHEMA, TEST_ALIAS),
        },
        "data": {
            "node_info": expected_node_info("executing", false)
        }
    });

    let expected_log_start_line = json!({
        "info": {
            "category": "",
            "code": "Q011",
            "invocation_id": invocation_id.to_string(),
            "name": "LogStartLine",
            "level": "info",
            "extra": {},
            "msg": format!("Started test {}.{}", TEST_SCHEMA, TEST_ALIAS),
        },
        "data": {
            "node_info": expected_node_info("started", false),
            "index": TEST_NODE_INDEX,
            "total": TEST_NODE_COUNT_TOTAL
        }
    });

    let expected_node_start = json!({
        "info": {
            "category": "",
            "code": "Q024",
            "invocation_id": invocation_id.to_string(),
            "name": "NodeStart",
            "level": "debug",
            "msg": format!("Began running node {}", test_unique_id.clone()),
            "extra": {}
        },
        "data": {
            "node_info": expected_node_info("started", false)
        }
    });

    let expected_log_result = json!({
        "info": {
            "category": "",
            "code": "Q007",
            "invocation_id": invocation_id.to_string(),
            "name": "LogTestResult",
            "level": "info",
            "extra": {},
            // Counter trails the line; a skipped node has no elapsed time, so the trailing
            // field carries the counter alone.
            "msg": format!(
                "   Skipped test  {} ({} - {}:1:0) [{} of {}]",
                TEST_ALIAS, test_custom_mat.clone(), TEST_REL_PATH, TEST_NODE_INDEX, TEST_NODE_COUNT_TOTAL
            )
        },
        "data": {
            "node_info": expected_node_info("skipped", true),
            "status": "skipped",
            "execution_time": "<redacted::execution_time>",
            "index": TEST_NODE_INDEX,
            "total": TEST_NODE_COUNT_TOTAL,
        }
    });

    let expected_mark_skipped = json!({
        "info": {
            "category": "",
            "code": "Z033",
            "invocation_id": invocation_id.to_string(),
            "name": "MarkSkippedChildren",
            "level": "debug",
            "extra": {},
            "msg": format!("Marking children of {} as skipped", test_unique_id.clone())
        },
        "data": {
            "unique_id": test_unique_id.clone(),
            "status": "skipped",
            "run_result": {
                "status": "skipped",
                "execution_time": "<redacted::execution_time>",
            }
        }
    });

    let expected_node_finished = json!({
        "info": {
            "category": "",
            "code": "Q025",
            "invocation_id": invocation_id.to_string(),
            "name": "NodeFinished",
            "level": "debug",
            "extra": {},
            "msg": format!("Finished running node {}", test_unique_id.clone()),
        },
        "data": {
            "node_info": expected_node_info("skipped", true),
            "run_result": {
                "status": "skipped",
                "execution_time": "<redacted::execution_time>",
            }
        }
    });

    test_events(
        "Node events (skipped test)",
        invocation_id,
        setup,
        &[
            "node_info.node_started_at",
            "node_info.node_finished_at",
            "execution_time",
            "run_result.execution_time",
        ],
        vec![
            expected_node_compiling,
            expected_node_executing,
            expected_log_start_line,
            expected_node_start,
            expected_log_result,
            expected_mark_skipped,
            expected_node_finished,
        ],
    );
}

#[test]
fn test_progress_message_debug_cmd_out() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "ProgressMessage (DebugCmdOut Z047)",
        invocation_id,
        || {
            emit_info_event(
                ProgressMessage::new_with_code(
                    "Debugging".to_string(),
                    "profile: my_profile".to_string(),
                    None,
                    "Z047".to_string(),
                ),
                None,
            );
            emit_info_event(
                ProgressMessage::new_with_code(
                    "Debugged".to_string(),
                    "All checks passed!".to_string(),
                    None,
                    "Z048".to_string(),
                ),
                None,
            );
        },
        &[],
        vec![
            json!({
                "info": {
                    "category": "",
                    "code": "Z047",
                    "invocation_id": invocation_id.to_string(),
                    "name": "DebugCmdOut",
                    "msg": "Debugging profile: my_profile",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "msg": "Debugging profile: my_profile"
                }
            }),
            json!({
                "info": {
                    "category": "",
                    "code": "Z048",
                    "invocation_id": invocation_id.to_string(),
                    "name": "DebugCmdResult",
                    "msg": "Debugged All checks passed!",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "msg": "Debugged All checks passed!"
                }
            }),
        ],
    );
}

#[test]
fn test_progress_message_generic_fallback() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "ProgressMessage (generic fallback without dbt_core_event_code)",
        invocation_id,
        || {
            // Progress message without dbt_core_event_code and without unique_id
            emit_info_event(
                ProgressMessage::new_with_description(
                    "Loading".to_string(),
                    "dependencies".to_string(),
                    "Resolving package versions".to_string(),
                ),
                None,
            );
            // Progress message without dbt_core_event_code but with unique_id
            emit_info_event(
                ProgressMessage::new_with_description_and_unique_id(
                    "Compiling".to_string(),
                    "model".to_string(),
                    "Rendering Jinja templates".to_string(),
                    "model.my_project.my_model".to_string(),
                ),
                None,
            );
        },
        &[],
        vec![
            // Generic event without unique_id
            json!({
                "info": {
                    "category": "",
                    "code": "",
                    "invocation_id": invocation_id.to_string(),
                    "name": "Generic",
                    "msg": "Loading dependencies (Resolving package versions)",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "msg": "Loading dependencies (Resolving package versions)"
                }
            }),
            // Generic event with unique_id in node_info
            json!({
                "info": {
                    "category": "",
                    "code": "",
                    "invocation_id": invocation_id.to_string(),
                    "name": "Generic",
                    "msg": "Compiling model (Rendering Jinja templates)",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "msg": "Compiling model (Rendering Jinja templates)",
                    "node_info": {
                        "unique_id": "model.my_project.my_model"
                    }
                }
            }),
        ],
    );
}

#[test]
fn test_compiled_code() {
    let invocation_id = Uuid::new_v4();

    test_events_with_msg_fields_to_ignore_for_command(
        "CompiledCode",
        invocation_id,
        FsCommand::Compile,
        || {
            emit_debug_event(
                CompiledCode {
                    relative_path: "target/compiled/models/my_model.sql".to_string(),
                    sql: "SELECT 1 AS value".to_string(),
                    unique_id: "model.my_project.my_model".to_string(),
                    node_name: "my_model".to_string(),
                },
                None,
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "Q042",
                "invocation_id": invocation_id.to_string(),
                "name": "CompiledNode",
                "msg": "SELECT 1 AS value",
                "level": "info",
                "extra": {}
            },
            "data": {
                "compiled": "SELECT 1 AS value",
                "is_inline": false,
                "node_name": "my_model",
                "output_format": "text",
                "unique_id": "model.my_project.my_model"
            }
        })],
        &[],
    );
}

#[test]
fn test_compiled_code_inline() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "CompiledCodeInline",
        invocation_id,
        || {
            emit_info_event(
                CompiledCodeInline {
                    sql: "SELECT 1 AS value".to_string(),
                },
                None,
            );
        },
        &[],
        vec![json!({
            "info": {
                "category": "",
                "code": "Q042",
                "invocation_id": invocation_id.to_string(),
                "name": "CompiledNode",
                "msg": "Compiled inline node is:\nSELECT 1 AS value",
                "level": "info",
                "extra": {}
            },
            "data": {
                "compiled": "SELECT 1 AS value",
                "is_inline": true,
                "node_name": "inline_query",
                "output_format": "text",
                "unique_id": "sql_operation.inline_query"
            }
        })],
    );
}

#[test]
fn test_deps_package_installed() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "DepsPackageInstalled (DepsStartPackageInstall M014, DepsInstallInfo M015)",
        invocation_id,
        || {
            // Create a span for a hub package installation
            let span = create_info_span(DepsPackageInstalled {
                package_name: Some("dbt-utils".to_string()),
                package_type: PackageType::Hub as i32,
                package_version: None,
                package_url_or_path: None,
                dbt_core_event_code: "M014".to_string(),
            });

            update_span_attrs(&span, |ev: &mut DepsPackageInstalled| {
                ev.package_version = Some("1.2.0".to_string());
                ev.dbt_core_event_code = "M015".to_string();
            });
        },
        &[],
        vec![
            // DepsStartPackageInstall (M014) - emitted on span start
            json!({
                "info": {
                    "category": "",
                    "code": "M014",
                    "invocation_id": invocation_id.to_string(),
                    "msg": "Installing dbt-utils",
                    "name": "DepsStartPackageInstall",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "package_name": "dbt-utils"
                }
            }),
            // DepsInstallInfo (M015) - emitted on span end
            json!({
                "info": {
                    "category": "",
                    "code": "M015",
                    "invocation_id": invocation_id.to_string(),
                    "msg": " Installed dbt-utils: 1.2.0",
                    "name": "DepsInstallInfo",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "version_name": "1.2.0"
                }
            }),
        ],
    );
}

#[test]
fn test_deps_add_package() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "DepsAddPackage (M032 DepsAddPackage)",
        invocation_id,
        || {
            // Create a span for a hub package addition
            let _span = create_info_span(DepsAddPackage {
                package_name: "dbt-utils".to_string(),
                package_type: PackageType::Hub as i32,
                package_version: Some("1.3.0".to_string()),
                dbt_core_event_code: "M032".to_string(),
            });
        },
        &[],
        vec![
            // DepsAddPackage (M032) - emitted only on successful span end
            // Message format matches dbt-core: "Added new package <name>@<version>"
            json!({
                "info": {
                    "category": "",
                    "code": "M032",
                    "invocation_id": invocation_id.to_string(),
                    "msg": "Added new package dbt-utils@1.3.0",
                    "name": "DepsAddPackage",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "package_name": "dbt-utils",
                    "version": "1.3.0"
                }
            }),
        ],
    );
}

#[test]
fn test_show_result() {
    let invocation_id = Uuid::new_v4();

    test_events(
        "ShowResult",
        invocation_id,
        || {
            emit_info_event(
                ShowResult {
                    output_format: ShowResultOutputFormat::Text as i32,
                    content: "key: value\nother: data".to_string(),
                    result_type: "config".to_string(),
                    title: "Node Configuration".to_string(),
                    unique_id: Some("model.my_project.my_model".to_string()),
                },
                Some("Node Configuration\nkey: value\nother: data"),
            );
            emit_info_event(
                ShowResult {
                    output_format: ShowResultOutputFormat::Text as i32,
                    content: "manifest content here".to_string(),
                    result_type: "manifest".to_string(),
                    title: "Manifest".to_string(),
                    unique_id: None,
                },
                Some("Manifest\nmanifest content here"),
            );
        },
        &[],
        vec![
            json!({
                "info": {
                    "category": "",
                    "code": "",
                    "invocation_id": invocation_id.to_string(),
                    "name": "Generic",
                    "msg": "Node Configuration\nkey: value\nother: data",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "result_type": "config",
                    "content": "key: value\nother: data",
                    "output_format": "text",
                    "unique_id": "model.my_project.my_model"
                }
            }),
            json!({
                "info": {
                    "category": "",
                    "code": "",
                    "invocation_id": invocation_id.to_string(),
                    "name": "Generic",
                    "msg": "Manifest\nmanifest content here",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "result_type": "manifest",
                    "content": "manifest content here",
                    "output_format": "text"
                }
            }),
        ],
    );
}

#[test]
fn test_freshness_result() {
    let invocation_id = Uuid::new_v4();
    let source_name = "my_source";
    let source_schema = "my_schema";
    let table_name = "my_table";
    let test_unique_id = format!("source.my_project.{}.{}", source_name, table_name);

    // Helper to build node_info JSON - note: node_finished_at is redacted for all events
    // because the test framework collects outputs after span close
    let expected_node_info = |node_status: &str| {
        json!({
            "node_path": "models/sources.yml",
            "node_name": table_name,
            "unique_id": test_unique_id.clone(),
            "resource_type": "source",
            "materialized": "",
            "node_status": node_status,
            "node_started_at": "<redacted::node_started_at>",
            "node_finished_at": "<redacted::node_finished_at>",
            "node_relation": {
                "database": "MY_DB",
                "schema": source_schema,
                "alias": table_name,
                "relation_name": format!("MY_DB.{}.{}", source_schema, table_name)
            },
            "node_checksum": TEST_NODE_CHECKSUM
        })
    };

    test_events(
        "Freshness: NodeStart, LogStartLine, LogFreshnessResult, NodeFinished",
        invocation_id,
        || {
            // Create NodeProcessed span for freshness
            let mut node_processed_event = NodeProcessed::start(
                test_unique_id.clone(),
                table_name.to_string(),
                Some("MY_DB".to_string()),
                Some(source_schema.to_string()),
                Some(table_name.to_string()),
                None,
                None,
                NodeType::Source,
                Some(ExecutionPhase::FreshnessAnalysis),
                "models/sources.yml".to_string(),
                None,
                None,
                TEST_NODE_CHECKSUM.to_string(),
                true,
                None,
            );
            node_processed_event.source_name = Some(source_name.to_string());
            // Normally stamped on by the span builder; set here so the emitted
            // `index`/`total` are asserted against real values.
            node_processed_event.node_index = Some(TEST_NODE_INDEX);
            node_processed_event.node_count_total = Some(TEST_NODE_COUNT_TOTAL);

            // Set freshness outcome (warn) with age
            node_processed_event.node_outcome = NodeOutcome::Success as i32;
            node_processed_event.node_outcome_detail = Some(
                node_processed::NodeOutcomeDetail::NodeFreshnessOutcome(SourceFreshnessDetail {
                    node_freshness_outcome: SourceFreshnessOutcome::OutcomeWarned as i32,
                    age_seconds: Some(7200), // 2 hours
                }),
            );
            update_dbt_core_event_code_for_node_processed_end(&mut node_processed_event);

            let _span = create_info_span(node_processed_event);
        },
        &[
            "node_info.node_started_at",
            "node_info.node_finished_at",
            "execution_time",
            "run_result.execution_time",
        ],
        vec![
            // LogFreshnessResult (Q018)
            json!({
                "info": {
                    "category": "",
                    "code": "Q018",
                    "invocation_id": invocation_id.to_string(),
                    "name": "LogFreshnessResult",
                    "level": "info",
                    "extra": {},
                    "msg": format!("Freshness of {}.{}: warn", source_name, table_name)
                },
                "data": {
                    "node_info": expected_node_info("warn"),
                    "status": "warn",
                    "execution_time": "<redacted::execution_time>",
                    "source_name": source_name,
                    "table_name": table_name,
                    "index": TEST_NODE_INDEX,
                    "total": TEST_NODE_COUNT_TOTAL
                }
            }),
            // LogStartLine (Q011)
            json!({
                "info": {
                    "category": "",
                    "code": "Q011",
                    "invocation_id": invocation_id.to_string(),
                    "name": "LogStartLine",
                    "level": "info",
                    "extra": {},
                    "msg": format!("Started source {}.{}", source_name, table_name)
                },
                "data": {
                    "node_info": expected_node_info("started"),
                    "index": TEST_NODE_INDEX,
                    "total": TEST_NODE_COUNT_TOTAL
                }
            }),
            // NodeFinished (Q025) - must also be emitted for freshness nodes
            json!({
                "info": {
                    "category": "",
                    "code": "Q025",
                    "invocation_id": invocation_id.to_string(),
                    "name": "NodeFinished",
                    "level": "debug",
                    "extra": {},
                    "msg": format!("Finished running node {}", test_unique_id.clone())
                },
                "data": {
                    "node_info": expected_node_info("warn"),
                    "run_result": {
                        "status": "warn",
                        "execution_time": "<redacted::execution_time>"
                    }
                }
            }),
            // NodeStart (Q024)
            json!({
                "info": {
                    "category": "",
                    "code": "Q024",
                    "invocation_id": invocation_id.to_string(),
                    "name": "NodeStart",
                    "level": "debug",
                    "extra": {},
                    "msg": format!("Began running node {}", test_unique_id.clone())
                },
                "data": {
                    "node_info": expected_node_info("started")
                }
            }),
        ],
    );
}

#[test]
fn test_hook_processed() {
    let invocation_id = Uuid::new_v4();

    test_events_with_msg_fields_to_ignore(
        "HookProcessed (on-run-start with success)",
        invocation_id,
        || {
            // Create a HookProcessed span for on-run-start hook
            let _span = create_info_span(HookProcessed {
                package_name: "my_project".to_string(),
                name: Some("my_project-on-run-start.1".to_string()),
                hook_type: HookType::OnRunStart as i32,
                hook_outcome: HookOutcome::Error as i32,
                unique_id: "operation.my_project.my_project-on-run-start.1".to_string(),
                phase: ExecutionPhase::OnRunStart as i32,
                dbt_core_event_code: "Q032".to_string(),
                hook_index: 1,
            });
        },
        &["execution_time"],
        vec![
            // LogHookEndLine (Q033) - emitted on span end
            json!({
                "info": {
                    "category": "",
                    "code": "Q033",
                    "invocation_id": invocation_id.to_string(),
                    "name": "LogHookEndLine",
                    "level": "info",
                    "msg": "ERROR hook: operation.my_project.my_project-on-run-start.1",
                    "extra": {}
                },
                "data": {
                    "node_info": {
                        "node_name": "my_project-on-run-start.1",
                        "unique_id": "operation.my_project.my_project-on-run-start.1",
                        "resource_type": "operation",
                        "node_status": "error"
                    },
                    "index": 1,
                    "status": "error",
                    "execution_time": "<redacted::execution_time>"
                }
            }),
            // LogHookStartLine (Q032) - emitted on span start
            json!({
                "info": {
                    "category": "",
                    "code": "Q032",
                    "invocation_id": invocation_id.to_string(),
                    "name": "LogHookStartLine",
                    "msg": "START hook: operation.my_project.my_project-on-run-start.1",
                    "level": "info",
                    "extra": {}
                },
                "data": {
                    "node_info": {
                        "node_name": "my_project-on-run-start.1",
                        "unique_id": "operation.my_project.my_project-on-run-start.1",
                        "resource_type": "operation",
                        "node_status": "started"
                    },
                    "index": 1
                }
            }),
        ],
        &[],
    );
}
