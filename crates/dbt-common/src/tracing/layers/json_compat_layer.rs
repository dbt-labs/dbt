use std::time::SystemTime;

use chrono::Utc;
use dbt_error::ErrorCode;
use dbt_telemetry::{
    ArtifactType, ArtifactWritten, CompiledCode, CompiledCodeInline, DepsAddPackage,
    DepsPackageInstalled, ExecutionPhase, HookProcessed, HookType, Invocation, ListItemOutput,
    LogMessage, NodeEvaluated, NodeEvent, NodeOutcome, NodeProcessed, NodeSkipReason, NodeType,
    ProgressMessage, QueryExecuted, ShowDataOutput, ShowResult, TestOutcome, UserLogMessage,
    get_freshness_detail, get_test_outcome, has_node_warning,
};

use serde_json::json;
use tracing::level_filters::LevelFilter;

use dbt_tracing::{
    LogRecordInfo, SeverityNumber, SpanEndInfo, SpanStartInfo, StatusCode, TelemetryOutputFlags,
    background_writer::BackgroundWriter,
    data_provider::DataProvider,
    layer::{ConsumerLayer, TelemetryConsumer},
    shared_writer::SharedWriter,
    shutdown::TelemetryShutdownItem,
};

use super::super::{
    dbt_metrics::{FusionMetricKey, InvocationMetricKey},
    event_classifiers::is_exit_with_status_log,
    formatters::{
        deps::{format_package_installed_end, format_package_installed_start, format_package_spec},
        duration::format_timestamp_utc_zulu,
        hook::format_hook_outcome_as_status,
        invocation::format_invocation_summary,
        log_message::format_log_message,
        node::{
            format_compiled_inline_code, format_node_evaluated_start,
            format_node_outcome_as_status, format_node_processed_end, format_node_processed_start,
            get_num_failures,
        },
        progress::format_progress_message,
        query_log::format_query_log,
    },
    fs_error_log::get_log_message,
};

use crate::io_args::FsCommand;
use proto_rust::v1::public::fields::core_types::CoreEventInfo;

/// Build a JSON compatibility layer. This will write directly to the writer.
/// If you want to write to slow IO sink, prefer `build_json_compat_layer_with_background_writer`
pub fn build_json_compat_layer<W: SharedWriter + 'static>(
    writer: W,
    max_log_verbosity: LevelFilter,
    invocation_id: uuid::Uuid,
    command: FsCommand,
    command_name: &'static str,
) -> ConsumerLayer {
    Box::new(
        JsonCompatLayer::new(writer, invocation_id, command, command_name)
            .with_filter(max_log_verbosity),
    )
}

/// Build a JSON compatibility layer with a background writer. This is preferred for writing to
/// slow IO sinks like files.
pub fn build_json_compat_layer_with_background_writer<W: std::io::Write + Send + 'static>(
    writer: W,
    max_log_verbosity: LevelFilter,
    invocation_id: uuid::Uuid,
    command: FsCommand,
    command_name: &'static str,
) -> (ConsumerLayer, TelemetryShutdownItem) {
    let (writer, handle) = BackgroundWriter::new(writer);

    (
        build_json_compat_layer(
            writer,
            max_log_verbosity,
            invocation_id,
            command,
            command_name,
        ),
        Box::new(handle),
    )
}

/// Build a NodeInfo JSON object from common node fields
fn build_node_info_json(
    node: NodeEvent,
    node_status: &str,
    start_time_unix_nano: SystemTime,
    end_time_unix_nano: Option<SystemTime>,
) -> serde_json::Value {
    let (
        unique_id,
        name,
        relative_path,
        node_type,
        materialization,
        node_checksum,
        database,
        schema,
        identifier,
    ) = match node {
        NodeEvent::Evaluated(n) => (
            &n.unique_id,
            &n.name,
            &n.relative_path,
            n.node_type(),
            if n.materialization.is_some() {
                let mat = n.materialization();
                if mat == dbt_telemetry::NodeMaterialization::Custom {
                    n.custom_materialization.as_deref()
                } else {
                    Some(mat.as_static_ref())
                }
            } else {
                None
            },
            &n.node_checksum,
            n.database.as_deref(),
            n.schema.as_deref(),
            n.identifier.as_deref(),
        ),
        NodeEvent::Processed(n) => (
            &n.unique_id,
            &n.name,
            &n.relative_path,
            n.node_type(),
            if n.materialization.is_some() {
                let mat = n.materialization();
                if mat == dbt_telemetry::NodeMaterialization::Custom {
                    n.custom_materialization.as_deref()
                } else {
                    Some(mat.as_static_ref())
                }
            } else {
                None
            },
            &n.node_checksum,
            n.database.as_deref(),
            n.schema.as_deref(),
            n.identifier.as_deref(),
        ),
    };

    // Build relation_name from database, schema, and identifier (if available)
    let relation_name = match (database, schema, identifier) {
        (Some(db), Some(sch), Some(ident)) => format!("{}.{}.{}", db, sch, ident),
        _ => String::new(),
    };

    // Format timestamps as ISO 8601
    let started_at_str = format_timestamp_utc_zulu(start_time_unix_nano);
    let finished_at_str = end_time_unix_nano
        .map(format_timestamp_utc_zulu)
        .unwrap_or_default();

    json!({
        "node_path": relative_path,
        "node_name": name,
        "unique_id": unique_id,
        "resource_type": node_type.as_static_ref(),
        "materialized": materialization.unwrap_or_default(),
        "node_status": node_status,
        "node_started_at": started_at_str,
        "node_finished_at": finished_at_str,
        "node_relation": {
            "database": database.unwrap_or_default(),
            "schema": schema.unwrap_or_default(),
            "alias": identifier.unwrap_or_default(),
            "relation_name": relation_name
        },
        "node_checksum": node_checksum
    })
}

/// A telemetry consumer that writes out events in a format compatible with dbt-core structured json logs.
///
/// It is only writing a small subset of event types and not 100% matching dbt-core schema and left only
/// for backward compatibility with what has been implemented in fusion by May 2025 launch.
struct JsonCompatLayer {
    writer: Box<dyn SharedWriter>,
    invocation_id: uuid::Uuid,
    filter_flag: TelemetryOutputFlags,
    command: FsCommand,
    custom_envs: std::collections::BTreeMap<String, String>,
    command_name: &'static str,
}

impl JsonCompatLayer {
    pub fn new<W: SharedWriter + 'static>(
        writer: W,
        invocation_id: uuid::Uuid,
        command: FsCommand,
        command_name: &'static str,
    ) -> Self {
        let is_tty = writer.is_terminal();
        let custom_envs = crate::constants::collect_dbt_custom_envs();

        Self {
            writer: Box::new(writer),
            invocation_id,
            filter_flag: if is_tty {
                TelemetryOutputFlags::OUTPUT_CONSOLE
            } else {
                TelemetryOutputFlags::OUTPUT_LOG_FILE
            },
            command,
            custom_envs,
            command_name,
        }
    }

    fn build_core_event_info(
        &self,
        code: Option<&str>,
        name: Option<&str>,
        level: &str,
        msg: String,
    ) -> CoreEventInfo {
        CoreEventInfo {
            category: "".to_string(),
            code: code.unwrap_or("").to_string(),
            invocation_id: self.invocation_id.to_string(),
            name: name.unwrap_or("Generic").to_string(),
            pid: std::process::id() as i32,
            thread: std::thread::current().name().unwrap_or("main").to_string(),
            // drop the timezone offset and format as microseconds to conform to python logging timestamp parsing
            ts: Some(Utc::now().into()),
            msg,
            level: level.to_lowercase(),
            #[allow(clippy::disallowed_types)]
            extra: self
                .custom_envs
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    fn emit_main_report_version(&self) {
        let version = env!("CARGO_PKG_VERSION");
        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("A001"),
            Some("MainReportVersion"),
            "info",
            format!("Running with {}={}", self.command_name, version),
        ))
        .expect("Failed to serialize core event info to JSON");

        let value = json!({
            "info": info_json,
            "data": {
                "log_version": 3,
                "version": format!("={}", version),
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    fn emit_main_report_args(&self, invocation: &Invocation) {
        let mut args = serde_json::Map::new();

        // Add invocation_command from raw_command
        args.insert(
            "invocation_command".to_string(),
            json!(invocation.raw_command),
        );

        // Add fields from eval_args if present
        let msg = if let Some(eval_args) = &invocation.eval_args {
            // Add debug
            if let Some(debug) = eval_args.debug {
                args.insert("debug".to_string(), json!(debug.to_string()));
            }

            // Add indirect_selection
            if let Some(indirect_selection) = &eval_args.indirect_selection {
                args.insert("indirect_selection".to_string(), json!(indirect_selection));
            }

            // Add log_format
            if let Some(log_format) = &eval_args.log_format {
                args.insert("log_format".to_string(), json!(log_format));
            }

            // Add log_path
            if let Some(log_path) = &eval_args.log_path {
                args.insert("log_path".to_string(), json!(log_path));
            }

            // Add profiles_dir
            if let Some(profiles_dir) = &eval_args.profiles_dir {
                args.insert("profiles_dir".to_string(), json!(profiles_dir));
            }

            // Add quiet
            if let Some(quiet) = eval_args.quiet {
                args.insert("quiet".to_string(), json!(quiet.to_string()));
            }

            // Add target_path
            if let Some(target_path) = &eval_args.target_path {
                args.insert("target_path".to_string(), json!(target_path));
            }

            // Add write_json
            if let Some(write_json) = eval_args.write_json {
                args.insert("write_json".to_string(), json!(write_json.to_string()));
            }

            format!(
                "running {} with arguments {}",
                self.command_name,
                serde_json::to_string(eval_args).unwrap_or_default()
            )
        } else {
            format!("running {}", self.command_name)
        };

        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("A002"),
            Some("MainReportArgs"),
            "debug",
            msg,
        ))
        .expect("Failed to serialize core event info to JSON");

        let value = json!({
            "info": info_json,
            "data": {
                "args": args,
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    fn emit_query_executed(&self, query_data: &QueryExecuted, span: &SpanEndInfo) {
        // Format the query log using the shared formatter
        let formatted_query = format_query_log(
            query_data,
            span.start_time_unix_nano,
            span.end_time_unix_nano,
        );

        let info_json = serde_json::to_value(self.build_core_event_info(
            query_data.dbt_core_event_code.as_str().into(),
            Some("SQLQuery"),
            &span.severity_text,
            formatted_query,
        ))
        .expect("Failed to serialize core event info to JSON");

        let value = if let Some(unique_id) = query_data.unique_id.as_deref() {
            json!({
                "info": info_json,
                "data": {
                    "sql": query_data.sql,
                    "node_info": {
                        "unique_id": unique_id
                    }
                }
            })
        } else {
            json!({
                "info": info_json,
                "data": {
                    "sql": query_data.sql,
                }
            })
        };

        self.writer.writeln(value.to_string().as_str());
    }

    fn emit_invocation_completed(
        &self,
        invocation: &Invocation,
        span: &SpanEndInfo,
        data_provider: &mut DataProvider<'_>,
    ) {
        let formatted = format_invocation_summary(span, invocation, data_provider, false, None);

        if let Some(line) = formatted.autofix_line() {
            let info_json = serde_json::to_value(self.build_core_event_info(
                None,
                None,
                &span.severity_text,
                line.to_string(),
            ))
            .expect("Failed to serialize core event info to JSON");

            let value = json!({
                "info": info_json,
                "data": {}
            })
            .to_string();

            self.writer.writeln(value.as_str());
        }

        let message = formatted
            .summary_lines()
            .map(|summary_lines| summary_lines.join("\n"))
            .unwrap_or_default();

        let elapsed_secs = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default()
            .as_secs_f32();

        let completed_at_str = format_timestamp_utc_zulu(span.end_time_unix_nano);
        let success = data_provider.get_metric(FusionMetricKey::InvocationMetric(
            InvocationMetricKey::TotalErrors,
        )) == 0;

        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("Q039"),
            Some("CommandCompleted"),
            &span.severity_text,
            message,
        ))
        .expect("Failed to serialize core event info to JSON");

        // We only use structured type for info, because proto defined data
        // types are not matching what fusion emitted for compatibility by the
        // original launch date.
        let value = json!({
            "info": info_json,
            "data": {
                "completed_at": completed_at_str,
                "elapsed": elapsed_secs,
                "success": success
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    fn emit_log_message(&self, log_msg: &LogMessage, log_record: &LogRecordInfo) {
        // Format the message
        let formatted_message = format_log_message(
            log_msg
                .code
                .and_then(|c| u16::try_from(c).ok())
                .and_then(|c| ErrorCode::try_from(c).ok()),
            // Unfortunately, we do not currently enforce log body to not contain ANSI codes,
            // so we need to make sure to strip them
            console::strip_ansi_codes(log_record.body.as_str()),
            log_record.severity_number,
            false,
            true,
        );

        let code = log_msg
            .dbt_core_event_code
            .clone()
            .or_else(|| log_msg.code.map(|c| c.to_string()));

        let info_json = serde_json::to_value(self.build_core_event_info(
            code.as_deref(),
            log_msg.code_name.as_deref(),
            &log_record.severity_text,
            formatted_message,
        ))
        .expect("Failed to serialize core event info to JSON");

        let mut data_obj = json!({});

        if let Some(unique_id) = log_msg.unique_id.as_deref() {
            data_obj.as_object_mut().unwrap().insert(
                "node_info".to_string(),
                json!({
                    "unique_id": unique_id
                }),
            );
        }

        let value = json!({
            "info": info_json,
            "data": data_obj
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }
    /// Handle UserLogMessage events (from Jinja print() and log() functions)
    /// These map to dbt-core's JinjaLogInfo (I062), JinjaDebugInfo (I063) and PrintEvent ("Z052") events
    fn emit_user_log_message(&self, user_log_msg: &UserLogMessage, log_record: &LogRecordInfo) {
        let (event_name, event_code) = match (user_log_msg.is_print, log_record.severity_number) {
            (true, _) => {
                // print() maps to PrintEvent (Z052)
                ("PrintEvent", "Z052")
            }
            (false, SeverityNumber::Info) => {
                // log(..., info=true) maps to JinjaLogInfo (I062)
                ("JinjaLogInfo", "I062")
            }
            (false, SeverityNumber::Debug) => {
                // log(..., info=false) maps to JinjaDebugInfo (I063)
                ("JinjaDebugInfo", "I063")
            }
            _ => {
                // Unknown combination, skip
                return;
            }
        };

        let info_json = serde_json::to_value(self.build_core_event_info(
            Some(event_code),
            Some(event_name),
            &log_record.severity_text,
            log_record.body.clone(),
        ))
        .expect("Failed to serialize core event info to JSON");

        let mut data_obj = json!({
            "msg": log_record.body, // Yes, duplicate the message here as well
        });

        if let Some(unique_id) = user_log_msg.unique_id.as_deref() {
            data_obj.as_object_mut().unwrap().insert(
                "node_info".to_string(),
                json!({
                    "unique_id": unique_id
                }),
            );
        }

        let value = json!({
            "info": info_json,
            "data": data_obj
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle ListItemOutput events (from dbt list command)
    /// These map to PrintEvent (Z052) for backward compatibility
    fn emit_list_item_output(&self, list_item: &ListItemOutput, log_record: &LogRecordInfo) {
        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("Z052"),
            Some("PrintEvent"),
            &log_record.severity_text,
            list_item.content.clone(),
        ))
        .expect("Failed to serialize core event info to JSON");

        let value = json!({
            "info": info_json,
            "data": {
                "msg": &list_item.content, // Use content from event for backward compatibility
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle ShowDataOutput events (from dbt show command)
    /// These map to ShowNode event (Q041) for backward compatibility
    fn emit_show_data_output(&self, inline_data: &ShowDataOutput, log_record: &LogRecordInfo) {
        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("Q041"),
            Some("ShowNode"),
            &log_record.severity_text,
            inline_data.content.clone(),
        ))
        .expect("Failed to serialize core event info to JSON");

        let data_obj = json!({
            "node_name": &inline_data.node_name,
            "preview": &inline_data.content,
            "is_inline": inline_data.is_inline,
            "output_format": inline_data.output_format().as_static_str(),
            "unique_id": inline_data.unique_id.as_deref().unwrap_or("sql_operation.inline_query"),
            "columns": &inline_data.columns,
        });

        let value = json!({
            "info": info_json,
            "data": data_obj,
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle ShowResult events (verbose/diagnostic output controlled by --show flag)
    fn emit_show_result(&self, show_result: &ShowResult, log_record: &LogRecordInfo) {
        let info_json = serde_json::to_value(self.build_core_event_info(
            None,
            None,
            &log_record.severity_text,
            format!(
                "{}\n{}",
                show_result.title,
                console::strip_ansi_codes(show_result.content.as_str())
            ),
        ))
        .expect("Failed to serialize core event info to JSON");

        let mut data_obj = json!({
            "result_type": &show_result.result_type,
            "content": &show_result.content,
            "output_format": show_result.output_format().as_static_str(),
        });

        if let Some(unique_id) = &show_result.unique_id {
            data_obj
                .as_object_mut()
                .unwrap()
                .insert("unique_id".to_string(), json!(unique_id));
        }

        let value = json!({
            "info": info_json,
            "data": data_obj,
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle ArtifactWritten events
    /// Maps to ArtifactWritten (P001) for backward compatibility with dbt-core
    fn emit_artifact_written(&self, artifact_data: &ArtifactWritten, span: &SpanEndInfo) {
        // Convert artifact_type i32 to enum and then to string
        let artifact_type_str = ArtifactType::try_from(artifact_data.artifact_type)
            .map(|at| match at {
                ArtifactType::Manifest => "WritableManifest".to_string(),
                _ => at.as_str_name().to_string(),
            })
            .unwrap_or_else(|_| "UNKNOWN".to_string());

        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("P001"),
            Some("ArtifactWritten"),
            &span.severity_text,
            format!("Artifact written to {}", artifact_data.relative_path),
        ))
        .expect("Failed to serialize core event info to JSON");

        let value = json!({
            "info": info_json,
            "data": {
                "artifact_path": &artifact_data.relative_path,
                "artifact_type": artifact_type_str,
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle HookProcessed span start events
    /// Generates LogHookStartLine (Q032)
    fn emit_hook_processed_start(&self, hook: &HookProcessed, _span: &SpanStartInfo) {
        if matches!(hook.hook_type(), HookType::PreHook | HookType::PostHook) {
            // Dbt core doesn't emit json events for node hooks
            return;
        }

        let msg = format!("START hook: {}", hook.unique_id);

        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("Q032"),
            Some("LogHookStartLine"),
            "info",
            msg,
        ))
        .expect("Failed to serialize core event info to JSON");

        let node_info = json!({
            "unique_id": hook.unique_id,
            "resource_type": "operation",
            "node_status": "started",
            "node_name": hook.name.as_deref().unwrap_or(""),
        });

        let value = json!({
            "info": info_json,
            "data": {
                "node_info": node_info,
                "index": hook.hook_index,
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle HookProcessed span end events
    /// Generates LogHookEndLine (Q033)
    fn emit_hook_processed_end(&self, hook: &HookProcessed, span: &SpanEndInfo) {
        if matches!(hook.hook_type(), HookType::PreHook | HookType::PostHook) {
            // Dbt core doesn't emit json events for node hooks
            return;
        }

        let hook_outcome = hook.hook_outcome();
        let status = format_hook_outcome_as_status(hook_outcome, false);

        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();

        let msg_status = if hook_outcome == dbt_telemetry::HookOutcome::Success {
            "OK"
        } else {
            "ERROR"
        };
        let msg = format!("{msg_status} hook: {}", hook.unique_id,);

        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("Q033"),
            Some("LogHookEndLine"),
            "info",
            msg,
        ))
        .expect("Failed to serialize core event info to JSON");

        let node_info = json!({
            "unique_id": hook.unique_id,
            "resource_type": "operation",
            "node_status": status,
            "node_name": hook.name.as_deref().unwrap_or(""),
        });

        let value = json!({
            "info": info_json,
            "data": {
                "node_info": node_info,
                "index": hook.hook_index,
                "status": status,
                "execution_time": duration.as_secs_f32(),
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle NodeEvaluated span start events
    /// Generates NodeCompiling (Q030) for Render phase or NodeExecuting (Q031) for Run phase
    fn emit_node_evaluated_start(&self, node: &NodeEvaluated, span: &SpanStartInfo) {
        // Only emit for Render (NodeCompiling) or Run (NodeExecuting) phases
        let (event_name, event_code, node_status) = match node.phase() {
            ExecutionPhase::Render => ("NodeCompiling", "Q030", "compiling"),
            ExecutionPhase::Run => ("NodeExecuting", "Q031", "executing"),
            _ => return, // Skip Analyze and other phases
        };

        let msg = format_node_evaluated_start(node, false);

        let node_info =
            build_node_info_json(node.into(), node_status, span.start_time_unix_nano, None);

        let info_json = serde_json::to_value(self.build_core_event_info(
            Some(event_code),
            Some(event_name),
            "debug",
            msg,
        ))
        .expect("Failed to serialize core event info to JSON");

        let value = json!({
            "info": info_json,
            "data": {
                "node_info": node_info
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle NodeProcessed span start events
    /// Generates NodeStart (Q024) and LogStartLine (Q011)
    fn emit_node_processed_start(&self, node: &NodeProcessed, span: &SpanStartInfo) {
        // Do not emit for non-selected nodes
        if !node.in_selection {
            return;
        }

        let node_info =
            build_node_info_json(node.into(), "started", span.start_time_unix_nano, None);

        // Emit NodeStart (Q024)
        let info_json_start = serde_json::to_value(self.build_core_event_info(
            Some("Q024"),
            Some("NodeStart"),
            "debug",
            format!("Began running node {}", node.unique_id),
        ))
        .expect("Failed to serialize core event info to JSON");

        let value_start = json!({
            "info": info_json_start,
            "data": {
                "node_info": node_info
            }
        })
        .to_string();

        self.writer.writeln(value_start.as_str());

        // Emit LogStartLine (Q011)
        let msg = format_node_processed_start(node, false);

        let info_json_log = serde_json::to_value(self.build_core_event_info(
            Some("Q011"),
            Some("LogStartLine"),
            "info",
            msg,
        ))
        .expect("Failed to serialize core event info to JSON");

        // dbt core always emits these as integers, using 0 for the ephemeral models that
        // never take an index, so mirror that rather than omitting the keys.
        let value_log = json!({
            "info": info_json_log,
            "data": {
                "node_info": node_info,
                "index": node.node_index.unwrap_or(0),
                "total": node.node_count_total.unwrap_or(0)
            }
        })
        .to_string();

        self.writer.writeln(value_log.as_str());
    }

    /// Handle MarkSkippedChildren event
    /// Emitted when a node fails or is skipped, indicating children will be skipped
    fn emit_mark_skipped_children(
        &self,
        node: &NodeProcessed,
        status: &str,
        run_result: &serde_json::Value,
    ) {
        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("Z033"),
            Some("MarkSkippedChildren"),
            "debug",
            format!("Marking children of {} as skipped", node.unique_id),
        ))
        .expect("Failed to serialize core event info to JSON");

        let value = json!({
            "info": info_json,
            "data": {
                "unique_id": node.unique_id,
                "status": status,
                "run_result": run_result
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle NodeProcessed span end events
    /// Generates Log[NODE TYPE]Result based on node type/phase & NodeFinished (Q025)
    fn emit_node_processed_end(&self, node: &NodeProcessed, span: &SpanEndInfo) {
        // Do not emit for non-selected nodes
        if !node.in_selection {
            return;
        }

        let source_name = node.source_name.as_deref().unwrap_or("");
        let table_name = node.identifier.as_deref().unwrap_or(node.name.as_str());

        let node_outcome = node.node_outcome();
        let node_type = node.node_type();
        let node_skip_reason = node.node_skip_reason.map(|_| node.node_skip_reason());

        // Get both test and freshness details (mutually exclusive - oneof in proto)
        let test_outcome = get_test_outcome(node.into());
        let freshness_detail = get_freshness_detail(node.into());
        let freshness_outcome = freshness_detail
            .as_ref()
            .map(|d| d.node_freshness_outcome());

        // Every node measured by the freshness phase reports a freshness result, models with
        // a freshness SLA included — not just sources. The phase is enough on its own: the
        // freshness SAO runs during a build create no node spans at all, so an extended model
        // can never reach here with this phase.
        let is_freshness = node.last_phase() == ExecutionPhase::FreshnessAnalysis;

        // Determine `Log[NODE TYPE]Result` event name and code
        let (event_name, event_code) = if is_freshness {
            // Freshness phase: emit LogFreshnessResult (Q018)
            ("LogFreshnessResult", "Q018")
        } else if node_outcome == NodeOutcome::Skipped
            && matches!(node_skip_reason, Some(NodeSkipReason::NoOp))
        {
            // Special case for no-op result
            ("LogNodeNoOpResult", "Q019")
        } else {
            match node_type {
                NodeType::Test | NodeType::UnitTest => ("LogTestResult", "Q007"),
                NodeType::Model => ("LogModelResult", "Q012"),
                NodeType::Snapshot => ("LogSnapshotResult", "Q015"),
                NodeType::Seed => ("LogSeedResult", "Q016"),
                NodeType::Function => ("LogFunctionResult", "Q047"),
                _ => ("LogNodeResult", "Q008"),
            }
        };

        // Build status string using shared formatter (handles both test and freshness outcomes)
        let status = format_node_outcome_as_status(
            node_outcome,
            node_skip_reason,
            test_outcome,
            freshness_outcome,
            has_node_warning(node.into()),
            false,
        );

        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();

        // Format message - for freshness, use special format
        let msg = if is_freshness {
            // dbt-core qualifies with the source name; a model has none, so fall back to its
            // schema rather than emitting a bare leading dot.
            let qualifier = if source_name.is_empty() {
                node.schema.as_deref().unwrap_or("")
            } else {
                source_name
            };
            if qualifier.is_empty() {
                format!("Freshness of {}: {}", table_name, status)
            } else {
                format!("Freshness of {}.{}: {}", qualifier, table_name, status)
            }
        } else {
            format_node_processed_end(node, duration, false)
        };

        let node_info = build_node_info_json(
            node.into(),
            &status,
            span.start_time_unix_nano,
            Some(span.end_time_unix_nano),
        );

        let info_json = serde_json::to_value(self.build_core_event_info(
            Some(event_code),
            Some(event_name),
            "info",
            msg,
        ))
        .expect("Failed to serialize core event info to JSON");

        // Extract num_failures for tests
        let num_failures = get_num_failures(node.into());

        // Build data object
        let mut data = json!({
            "node_info": node_info,
            "status": status,
            "execution_time": duration.as_secs_f32(),
            "index": node.node_index.unwrap_or(0),
            "total": node.node_count_total.unwrap_or(0)
        });

        if is_freshness {
            let as_map = data.as_object_mut().unwrap();
            as_map.insert("source_name".to_string(), source_name.into());
            as_map.insert("table_name".to_string(), table_name.into());
        }

        if let Some(num_failures_val) = num_failures {
            data.as_object_mut()
                .unwrap()
                .insert("num_failures".to_string(), json!(num_failures_val));
        }

        let value = json!({
            "info": info_json,
            "data": data
        })
        .to_string();

        self.writer.writeln(value.as_str());

        // NodeFinished (Q025) - emit for ALL nodes including freshness
        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("Q025"),
            Some("NodeFinished"),
            "debug",
            format!("Finished running node {}", node.unique_id),
        ))
        .expect("Failed to serialize core event info to JSON");

        // Build run_result with num_failures if available
        let mut run_result = json!({
            "status": status,
            "execution_time": duration.as_secs_f32(),
        });

        if let Some(num_failures_val) = num_failures {
            run_result
                .as_object_mut()
                .unwrap()
                .insert("num_failures".to_string(), json!(num_failures_val));
        }

        // Emit MarkSkippedChildren if node outcome would cause children to be skipped
        // This happens when node fails, is skipped, canceled, or for tests with failures
        // Note: Freshness nodes don't have children, so we skip this check for them
        let should_mark_skipped = if is_freshness {
            false // Freshness sources don't have downstream children in the execution graph
        } else {
            match (node_outcome, test_outcome) {
                (NodeOutcome::Success, None) => false,
                // Tests report as success, we need to check test outcome for failures
                (NodeOutcome::Success, Some(to)) => to == TestOutcome::Failed,
                // Any error outcome causes children to be skipped
                (NodeOutcome::Error, _) => true,
                // Canceled doesn't mean skipped, just operation was aborted
                (NodeOutcome::Canceled, _) => false,
                // Skipped because of error causes children to be skipped as well
                (NodeOutcome::Skipped, _) => {
                    matches!(node_skip_reason, Some(NodeSkipReason::Upstream))
                }
                // should not happen, treat as not skipping children
                (NodeOutcome::Unspecified, _) => false,
            }
        };

        if should_mark_skipped {
            self.emit_mark_skipped_children(node, &status, &run_result);
        }

        let value = json!({
            "info": info_json,
            "data": {
                "node_info": node_info,
                "run_result": run_result
            }
        })
        .to_string();

        // Now write NodeFinished event
        self.writer.writeln(value.as_str());
    }

    /// Handle ProgressMessage events (debug command progress, etc.)
    /// If dbt_core_event_code is present, map to appropriate event, otherwise emit as generic
    fn emit_progress_message(&self, progress_msg: &ProgressMessage, log_record: &LogRecordInfo) {
        let msg = format_progress_message(progress_msg, log_record.severity_number, false, false);

        // Determine event code and name based on dbt_core_event_code
        let (event_code, event_name) = match progress_msg.dbt_core_event_code.as_deref() {
            Some("Z047") => (Some("Z047"), Some("DebugCmdOut")),
            Some("Z048") => (Some("Z048"), Some("DebugCmdResult")),
            Some(_) => {
                // In debug build panic for unknown codes to catch missing mappings, in prod just skip
                #[cfg(debug_assertions)]
                panic!(
                    "Unhandled dbt_core_event_code '{}' in ProgressMessage. Add mapping in JsonCompatLayer `emit_progress_message` function.",
                    progress_msg.dbt_core_event_code.as_deref().unwrap()
                );
                #[cfg(not(debug_assertions))]
                return;
            }
            None => (None, None), // Fall back to generic handling
        };

        let info_json = serde_json::to_value(self.build_core_event_info(
            event_code,
            event_name,
            &log_record.severity_text,
            msg.clone(),
        ))
        .expect("Failed to serialize core event info to JSON");

        let mut data_obj = json!({
            "msg": msg,
        });

        // Include unique_id in node_info if available
        if let Some(unique_id) = progress_msg.unique_id.as_deref() {
            data_obj.as_object_mut().unwrap().insert(
                "node_info".to_string(),
                json!({
                    "unique_id": unique_id
                }),
            );
        }

        let value = json!({
            "info": info_json,
            "data": data_obj
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle DepsPackageInstalled span start events
    fn emit_dep_installed_start(&self, pkg: &DepsPackageInstalled, span: &SpanStartInfo) {
        // Format with shared formatter
        let formatted_message = format_package_installed_start(pkg, false);

        // Use dbt-core event code M014 and name DepsStartPackageInstall
        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("M014"),
            Some("DepsStartPackageInstall"),
            &span.severity_text,
            formatted_message,
        ))
        .expect("Failed to serialize core event info to JSON");

        // dbt-core's DepsStartPackageInstall only has package_name field
        let package_name = pkg
            .package_name
            .as_deref()
            .or(pkg.package_url_or_path.as_deref())
            .unwrap_or("unknown");

        let value = json!({
            "info": info_json,
            "data": {
                "package_name": package_name,
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle DepsPackageInstalled span end events
    fn emit_dep_installed_end(&self, pkg: &DepsPackageInstalled, span: &SpanEndInfo) {
        // Only emit if the span ended successfully
        let status = span.status.as_ref().map_or(StatusCode::Unset, |s| s.code);
        if status == StatusCode::Error {
            return;
        }

        // Format with shared formatter
        let formatted_message = format_package_installed_end(pkg, status, false);

        // Use dbt-core event code M015 and name DepsInstallInfo
        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("M015"),
            Some("DepsInstallInfo"),
            &span.severity_text,
            formatted_message,
        ))
        .expect("Failed to serialize core event info to JSON");

        // dbt-core's DepsInstallInfo only has version_name field
        let value = json!({
            "info": info_json,
            "data": {
                "version_name": pkg.package_version.as_deref().unwrap_or_default(),
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle DepsAddPackage span end events - only emits on successful completion
    fn emit_package_add_end(&self, pkg: &DepsAddPackage, span: &SpanEndInfo) {
        // Only emit if the span ended successfully
        let status = span.status.as_ref().map_or(StatusCode::Unset, |s| s.code);
        if status == StatusCode::Error {
            return;
        }

        // Format package spec (name@version or just name)
        let package_spec = format_package_spec(&pkg.package_name, pkg.package_version.as_deref());

        // Message format matching dbt-core: "Added new package <spec>"
        let formatted_message = format!("Added new package {}", package_spec);

        // Use dbt-core event code M032 and name DepsAddPackage
        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("M032"),
            Some("DepsAddPackage"),
            &span.severity_text,
            formatted_message,
        ))
        .expect("Failed to serialize core event info to JSON");

        let value = json!({
            "info": info_json,
            "data": {
                "package_name": &pkg.package_name,
                "version": pkg.package_version.as_deref().unwrap_or_default(),
            }
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle CompiledCode events (from compile command with selected node)
    /// Maps to CompiledNode (Q042) for dbt-core compatibility
    fn emit_compiled_code(&self, compiled_code: &CompiledCode, _log_record: &LogRecordInfo) {
        if self.command != FsCommand::Compile {
            return;
        }

        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("Q042"),
            Some("CompiledNode"),
            "info",
            compiled_code.sql.clone(),
        ))
        .expect("Failed to serialize core event info to JSON");

        let data_obj = json!({
            "compiled": &compiled_code.sql,
            "is_inline": false,
            "node_name": &compiled_code.node_name,
            "output_format": "text",
            "unique_id": &compiled_code.unique_id,
        });

        let value = json!({
            "info": info_json,
            "data": data_obj,
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }

    /// Handle CompiledCodeInline events (from compile command with inline query)
    /// Maps to CompiledNode (Q042) for dbt-core compatibility
    fn emit_compiled_code_inline(
        &self,
        compiled_code: &CompiledCodeInline,
        log_record: &LogRecordInfo,
    ) {
        let msg = format_compiled_inline_code(compiled_code, false);

        let info_json = serde_json::to_value(self.build_core_event_info(
            Some("Q042"),
            Some("CompiledNode"),
            &log_record.severity_text,
            msg,
        ))
        .expect("Failed to serialize core event info to JSON");

        let data_obj = json!({
            "compiled": &compiled_code.sql,
            "is_inline": true,
            "node_name": "inline_query",
            "output_format": "text",
            "unique_id": "sql_operation.inline_query",
        });

        let value = json!({
            "info": info_json,
            "data": data_obj,
        })
        .to_string();

        self.writer.writeln(value.as_str());
    }
}

impl TelemetryConsumer for JsonCompatLayer {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        span.attributes.output_flags().contains(self.filter_flag)
    }

    fn is_log_enabled(&self, log_record: &LogRecordInfo) -> bool {
        log_record
            .attributes
            .output_flags()
            .contains(self.filter_flag)
            // ExitWithStatus is a pseudo error used only to short-circuit execution, so we
            // filter it from dbt-facing output
            && !is_exit_with_status_log(log_record)
    }

    fn on_span_start(&self, span: &SpanStartInfo, _data_provider: &mut DataProvider<'_>) {
        // Emit MainReportVersion and MainReportArgs events once when invocation span is matched
        if let Some(invocation) = span.attributes.downcast_ref::<Invocation>() {
            self.emit_main_report_version();
            self.emit_main_report_args(invocation);
            return;
        }

        // Dispatch to NodeEvaluated handler
        if let Some(node_evaluated) = span.attributes.downcast_ref::<NodeEvaluated>() {
            self.emit_node_evaluated_start(node_evaluated, span);
            return;
        }

        // Dispatch to NodeProcessed handler
        if let Some(node_processed) = span.attributes.downcast_ref::<NodeProcessed>() {
            self.emit_node_processed_start(node_processed, span);
            return;
        }

        // Dispatch to HookProcessed handler
        if let Some(hook_processed) = span.attributes.downcast_ref::<HookProcessed>() {
            self.emit_hook_processed_start(hook_processed, span);
            return;
        }

        // Dispatch to DepsPackageInstalled handler
        if let Some(pkg) = span.attributes.downcast_ref::<DepsPackageInstalled>() {
            self.emit_dep_installed_start(pkg, span);
        }
    }

    fn on_span_end(&self, span: &SpanEndInfo, data_provider: &mut DataProvider<'_>) {
        // Dispatch to QueryExecuted handler
        if let Some(query_data) = span.attributes.downcast_ref::<QueryExecuted>() {
            self.emit_query_executed(query_data, span);
            return;
        }

        // Dispatch to ArtifactWritten handler
        if let Some(artifact_data) = span.attributes.downcast_ref::<ArtifactWritten>() {
            self.emit_artifact_written(artifact_data, span);
            return;
        }

        // Dispatch to NodeProcessed handler
        if let Some(node_processed) = span.attributes.downcast_ref::<NodeProcessed>() {
            self.emit_node_processed_end(node_processed, span);
            return;
        }

        // Dispatch to HookProcessed handler
        if let Some(hook_processed) = span.attributes.downcast_ref::<HookProcessed>() {
            self.emit_hook_processed_end(hook_processed, span);
            return;
        }

        // Dispatch to Invocation handler
        if let Some(invocation) = span.attributes.downcast_ref::<Invocation>() {
            self.emit_invocation_completed(invocation, span, data_provider);
            return;
        }

        // Dispatch to DepsPackageInstalled handler
        if let Some(pkg) = span.attributes.downcast_ref::<DepsPackageInstalled>() {
            self.emit_dep_installed_end(pkg, span);
            return;
        }

        // Dispatch to DepsAddPackage handler
        if let Some(pkg) = span.attributes.downcast_ref::<DepsAddPackage>() {
            self.emit_package_add_end(pkg, span);
        }
    }

    fn on_log_record(&self, log_record: &LogRecordInfo, _data_provider: &mut DataProvider<'_>) {
        // Dispatch to LogMessage handler
        if let Some(log_msg) = get_log_message(&log_record.attributes) {
            self.emit_log_message(log_msg, log_record);
            return;
        }

        // Dispatch to ProgressMessage handler
        if let Some(progress_msg) = log_record.attributes.downcast_ref::<ProgressMessage>() {
            self.emit_progress_message(progress_msg, log_record);
            return;
        }

        // Dispatch to UserLogMessage handler
        if let Some(user_log_msg) = log_record.attributes.downcast_ref::<UserLogMessage>() {
            self.emit_user_log_message(user_log_msg, log_record);
            return;
        }

        // Dispatch to ListItemOutput handler
        if let Some(list_item) = log_record.attributes.downcast_ref::<ListItemOutput>() {
            self.emit_list_item_output(list_item, log_record);
            return;
        }

        // Dispatch to ShowDataOutput handler
        if let Some(inline_data) = log_record.attributes.downcast_ref::<ShowDataOutput>() {
            self.emit_show_data_output(inline_data, log_record);
            return;
        }

        // Dispatch to ShowResult handler
        if let Some(show_result) = log_record.attributes.downcast_ref::<ShowResult>() {
            self.emit_show_result(show_result, log_record);
            return;
        }

        // Dispatch to CompiledCode handler
        if let Some(compiled_code) = log_record.attributes.downcast_ref::<CompiledCode>() {
            self.emit_compiled_code(compiled_code, log_record);
            return;
        }

        // Dispatch to CompiledCodeInline handler
        if let Some(compiled_code) = log_record.attributes.downcast_ref::<CompiledCodeInline>() {
            self.emit_compiled_code_inline(compiled_code, log_record);
        }
    }
}
