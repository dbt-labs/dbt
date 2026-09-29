use crate::{attributes::DbtTelemetryContext, serialize::arrow::ArrowAttributes};
use dbt_tracing::{
    AnyTelemetryEvent, ArrowSerializableTelemetryEvent, SpanStatus, StaticTelemetryEvent,
    TelemetryContext, TelemetryEventRecType, TelemetryOutputFlags,
};

use prost::Name;
use serde_with::skip_serializing_none;
use std::borrow::Cow;

pub use crate::impls::node::{
    AnyNodeOutcomeDetail, NodeEvent, get_cache_detail, get_freshness_detail,
    get_node_outcome_detail, get_test_batch_unique_id, get_test_outcome, has_node_warning,
    is_batched_test, is_statically_checked_test, set_node_warning_outcome_no_warnings,
    set_node_warning_outcome_warned, update_dbt_core_event_code_for_node_processed_end,
};
pub use crate::proto::v1::public::events::fusion::node::{
    NodeCacheDetail, NodeCacheReason, NodeCancelReason, NodeErrorType, NodeEvaluated,
    NodeEvaluationDetail, NodeMaterialization, NodeOutcome, NodeProcessed, NodeSkipReason,
    NodeSkipUpstreamDetail, NodeType, NodeWarningOutcome, SourceFreshnessDetail,
    SourceFreshnessOutcome, TestEvaluationDetail, TestOutcome, node_evaluated::NodeOutcomeDetail,
    node_processed,
};

impl StaticTelemetryEvent for NodeEvaluated {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("Node evaluated ({})", self.unique_id)
    }

    fn get_span_status(&self) -> Option<SpanStatus> {
        match self.node_outcome() {
            NodeOutcome::Success | NodeOutcome::Skipped => SpanStatus::succeeded().into(),
            NodeOutcome::Error => SpanStatus::failed("error").into(),
            NodeOutcome::Canceled => SpanStatus::failed("canceled").into(),
            NodeOutcome::Unspecified => None,
        }
    }

    fn has_sensitive_data(&self) -> bool {
        true
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        // TODO: theoretically we may want to use a consistent scrambling/hashing of
        // identifiers as some may consider this sensitive
        let new_outcome_detail = match self.node_outcome_detail.as_ref() {
            Some(NodeOutcomeDetail::NodeTestDetail(test_detail)) => {
                Some(NodeOutcomeDetail::NodeTestDetail(TestEvaluationDetail {
                    // Scrub diff_table as it may contain sensitive data
                    diff_table: None,
                    ..test_detail.clone()
                }))
            }
            _ => self.node_outcome_detail.clone(),
        };

        Some(Box::new(Self {
            node_outcome_detail: new_outcome_detail,
            ..self.clone()
        }))
    }

    fn context(&self) -> Option<TelemetryContext> {
        Some(
            DbtTelemetryContext {
                phase: Some(self.phase()),
                unique_id: Some(self.unique_id.clone()),
            }
            .into(),
        )
    }
}

/// Internal struct used for serializing/deserializing NodeEvaluated fields
/// stored in JSON payload.
#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct NodeEvaluatedJsonPayload {
    /// Node type specific outcome details.
    pub node_outcome_detail: Option<NodeOutcomeDetail>,
    /// Time the node evaluation spent idle.
    pub idle_time_ms: Option<u64>,
    /// dbt State decision id for the node's run-phase execution.
    pub state_decision_id: Option<String>,
}

/// Internal struct used for serializing/deserializing subset of
/// NodeProcessed fields as JSON payload in ArrowAttributes.
#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct NodeProcessedJsonPayload {
    /// Whether the node was in the selection set.
    pub in_selection: bool,
    /// Node type specific outcome details.
    pub node_outcome_detail: Option<node_processed::NodeOutcomeDetail>,
    /// Time the node spent idle across nested evaluations.
    pub idle_time_ms: Option<u64>,
    /// Source name for source nodes.
    pub source_name: Option<String>,
    /// 1-based position of this node within the invocation.
    pub node_index: Option<u32>,
    /// Total number of nodes in the selection set.
    pub node_count_total: Option<u32>,
}

fn deserialize_node_evaluated_json_payload(
    json_payload: Option<&String>,
) -> Result<NodeEvaluatedJsonPayload, String> {
    let Some(json_payload) = json_payload else {
        return Ok(NodeEvaluatedJsonPayload {
            node_outcome_detail: None,
            idle_time_ms: None,
            state_decision_id: None,
        });
    };

    let value = serde_json::from_str::<serde_json::Value>(json_payload).map_err(|e| {
        format!(
            "Failed to deserialize json payload for event type \"{}\" from JSON: {}",
            NodeEvaluated::full_name(),
            e
        )
    })?;

    let is_wrapped_payload = value.as_object().is_some_and(|obj| {
        obj.contains_key("node_outcome_detail")
            || obj.contains_key("idle_time_ms")
            || obj.contains_key("state_decision_id")
    });

    if is_wrapped_payload {
        serde_json::from_value::<NodeEvaluatedJsonPayload>(value).map_err(|e| {
            format!(
                "Failed to deserialize json payload for event type \"{}\" from JSON: {}",
                NodeEvaluated::full_name(),
                e
            )
        })
    } else {
        Ok(NodeEvaluatedJsonPayload {
            node_outcome_detail: Some(serde_json::from_value::<NodeOutcomeDetail>(value).map_err(
                |e| {
                    format!(
                        "Failed to deserialize legacy `node_outcome_detail` in event type \"{}\" from JSON: {}",
                        NodeEvaluated::full_name(),
                        e
                    )
                },
            )?),
            idle_time_ms: None,
            state_decision_id: None,
        })
    }
}

impl ArrowSerializableTelemetryEvent for NodeEvaluated {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            // Well-known fields for easier querying
            phase: Some(self.phase()),
            name: Some(Cow::Borrowed(self.name.as_str())),
            database: self.database.as_deref().map(Cow::Borrowed),
            schema: self.schema.as_deref().map(Cow::Borrowed),
            identifier: self.identifier.as_deref().map(Cow::Borrowed),
            unique_id: Some(Cow::Borrowed(self.unique_id.as_str())),
            materialization: self.materialization.map(|_| self.materialization()),
            custom_materialization: self.custom_materialization.as_deref().map(Cow::Borrowed),
            node_type: Some(self.node_type()),
            node_outcome: Some(self.node_outcome()),
            node_error_type: self.node_error_type.map(|_| self.node_error_type()),
            node_cancel_reason: self.node_cancel_reason.map(|_| self.node_cancel_reason()),
            node_skip_reason: self.node_skip_reason.map(|_| self.node_skip_reason()),
            sao_enabled: self.sao_enabled,
            dbt_core_event_code: self.dbt_core_event_code.as_deref().map(Cow::Borrowed),
            relative_path: Some(Cow::Borrowed(self.relative_path.as_str())),
            code_line: self.defined_at_line,
            code_column: self.defined_at_col,
            content_hash: Some(Cow::Borrowed(self.node_checksum.as_str())),
            // Serialize less frequently queried node fields into JSON as they may grow
            // with arbitrary data.
            json_payload: (self.node_outcome_detail.is_some()
                || self.idle_time_ms.is_some()
                || self.state_decision_id.is_some())
            .then(|| {
                serde_json::to_string(&NodeEvaluatedJsonPayload {
                    node_outcome_detail: self.node_outcome_detail.clone(),
                    idle_time_ms: self.idle_time_ms,
                    state_decision_id: self.state_decision_id.clone(),
                })
                .unwrap_or_else(|_| {
                    panic!(
                        "Failed to serialize json payload for event type \"{}\" to JSON",
                        Self::full_name()
                    )
                })
            }),
            rows_affected: self.rows_affected,
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload = deserialize_node_evaluated_json_payload(record.json_payload.as_ref())?;

        Ok(Self {
            phase: record.phase.map(|v| v as i32).ok_or_else(|| {
                format!("Missing `phase` for event type \"{}\"", Self::full_name())
            })?,
            name: record.name.as_deref().map(str::to_string).ok_or_else(|| {
                format!("Missing `name` for event type \"{}\"", Self::full_name())
            })?,
            database: record.database.as_deref().map(str::to_string),
            schema: record.schema.as_deref().map(str::to_string),
            identifier: record.identifier.as_deref().map(str::to_string),
            unique_id: record
                .unique_id
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `unique_id` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            materialization: record.materialization.map(|v| v as i32),
            custom_materialization: record.custom_materialization.as_deref().map(str::to_string),
            node_type: record.node_type.map(|v| v as i32).ok_or_else(|| {
                format!(
                    "Missing `node_type` for event type \"{}\"",
                    Self::full_name()
                )
            })?,
            node_outcome: record.node_outcome.map(|v| v as i32).ok_or_else(|| {
                format!(
                    "Missing `node_outcome` for event type \"{}\"",
                    Self::full_name()
                )
            })?,
            node_error_type: record.node_error_type.map(|v| v as i32),
            node_cancel_reason: record.node_cancel_reason.map(|v| v as i32),
            node_skip_reason: record.node_skip_reason.map(|v| v as i32),
            sao_enabled: record.sao_enabled,
            state_decision_id: json_payload.state_decision_id,
            dbt_core_event_code: record.dbt_core_event_code.as_deref().map(str::to_string),
            node_outcome_detail: json_payload.node_outcome_detail,
            relative_path: record
                .relative_path
                .as_deref()
                .map(str::to_string)
                // pre-preview.70 we haven't stored relative_path in arrow, so default to "unknown"
                .unwrap_or_else(|| "unknown".to_string()),
            defined_at_line: record.code_line,
            defined_at_col: record.code_column,
            node_checksum: record
                .content_hash
                .as_deref()
                .map(str::to_string)
                // pre-preview.70 we haven't stored node_checksum in arrow, so default to "<missing>"
                .unwrap_or_else(|| "<missing>".to_string()),
            rows_affected: record.rows_affected,
            idle_time_ms: json_payload.idle_time_ms,
        })
    }
}

impl StaticTelemetryEvent for NodeProcessed {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("Node processed ({})", self.unique_id)
    }

    fn get_span_status(&self) -> Option<SpanStatus> {
        match self.node_outcome() {
            NodeOutcome::Success | NodeOutcome::Skipped => SpanStatus::succeeded().into(),
            NodeOutcome::Error => SpanStatus::failed("error").into(),
            NodeOutcome::Canceled => SpanStatus::failed("canceled").into(),
            NodeOutcome::Unspecified => None,
        }
    }

    fn has_sensitive_data(&self) -> bool {
        true
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        // Similar to NodeEvaluated, scrub sensitive data from test details
        let new_outcome_detail = match self.node_outcome_detail.as_ref() {
            Some(node_processed::NodeOutcomeDetail::NodeTestDetail(test_detail)) => {
                Some(node_processed::NodeOutcomeDetail::NodeTestDetail(
                    TestEvaluationDetail {
                        // Scrub diff_table as it may contain sensitive data
                        diff_table: None,
                        ..test_detail.clone()
                    },
                ))
            }
            _ => self.node_outcome_detail.clone(),
        };

        Some(Box::new(Self {
            node_outcome_detail: new_outcome_detail,
            ..self.clone()
        }))
    }

    fn context(&self) -> Option<TelemetryContext> {
        Some(
            DbtTelemetryContext {
                // This span cuts across multiple phases, so we don't set a single phase here
                phase: None,
                unique_id: Some(self.unique_id.clone()),
            }
            .into(),
        )
    }
}

impl ArrowSerializableTelemetryEvent for NodeProcessed {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            // Well-known fields for easier querying
            // Use last_phase instead of phase
            phase: Some(self.last_phase()),
            name: Some(Cow::Borrowed(self.name.as_str())),
            database: self.database.as_deref().map(Cow::Borrowed),
            schema: self.schema.as_deref().map(Cow::Borrowed),
            identifier: self.identifier.as_deref().map(Cow::Borrowed),
            unique_id: Some(Cow::Borrowed(self.unique_id.as_str())),
            materialization: self.materialization.map(|_| self.materialization()),
            custom_materialization: self.custom_materialization.as_deref().map(Cow::Borrowed),
            node_type: Some(self.node_type()),
            node_outcome: Some(self.node_outcome()),
            node_error_type: self.node_error_type.map(|_| self.node_error_type()),
            node_cancel_reason: self.node_cancel_reason.map(|_| self.node_cancel_reason()),
            node_skip_reason: self.node_skip_reason.map(|_| self.node_skip_reason()),
            sao_enabled: self.sao_enabled,
            dbt_core_event_code: Some(Cow::Borrowed(self.dbt_core_event_code.as_str())),
            relative_path: Some(Cow::Borrowed(self.relative_path.as_str())),
            code_line: self.defined_at_line,
            code_column: self.defined_at_col,
            content_hash: Some(Cow::Borrowed(self.node_checksum.as_str())),
            // Serialize in_selection and node_outcome_detail into JSON payload
            json_payload: serde_json::to_string(&NodeProcessedJsonPayload {
                in_selection: self.in_selection,
                node_outcome_detail: self.node_outcome_detail.clone(),
                idle_time_ms: self.idle_time_ms,
                source_name: self.source_name.clone(),
                node_index: self.node_index,
                node_count_total: self.node_count_total,
            })
            .unwrap_or_else(|_| {
                panic!(
                    "Failed to serialize json payload for event type \"{}\" to JSON",
                    Self::full_name()
                )
            })
            .into(),
            duration_ms: self.duration_ms,
            rows_affected: self.rows_affected,
            group: self.group.as_deref().map(Cow::Borrowed),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload: NodeProcessedJsonPayload =
            serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
                format!(
                    "Missing json_payload for event type \"{}\"",
                    Self::full_name()
                )
            })?)
            .map_err(|e| {
                format!(
                    "Failed to deserialize json payload for event type \"{}\" from JSON: {}",
                    Self::full_name(),
                    e
                )
            })?;

        Ok(Self {
            // Use last_phase (stored in phase field of ArrowAttributes)
            last_phase: record.phase.map(|v| v as i32).ok_or_else(|| {
                format!("Missing `phase` for event type \"{}\"", Self::full_name())
            })?,
            name: record.name.as_deref().map(str::to_string).ok_or_else(|| {
                format!("Missing `name` for event type \"{}\"", Self::full_name())
            })?,
            database: record.database.as_deref().map(str::to_string),
            schema: record.schema.as_deref().map(str::to_string),
            identifier: record.identifier.as_deref().map(str::to_string),
            unique_id: record
                .unique_id
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `unique_id` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            materialization: record.materialization.map(|v| v as i32),
            custom_materialization: record.custom_materialization.as_deref().map(str::to_string),
            node_type: record.node_type.map(|v| v as i32).ok_or_else(|| {
                format!(
                    "Missing `node_type` for event type \"{}\"",
                    Self::full_name()
                )
            })?,
            node_outcome: record.node_outcome.map(|v| v as i32).ok_or_else(|| {
                format!(
                    "Missing `node_outcome` for event type \"{}\"",
                    Self::full_name()
                )
            })?,
            node_error_type: record.node_error_type.map(|v| v as i32),
            node_cancel_reason: record.node_cancel_reason.map(|v| v as i32),
            node_skip_reason: record.node_skip_reason.map(|v| v as i32),
            sao_enabled: record.sao_enabled,
            dbt_core_event_code: record
                .dbt_core_event_code
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `dbt_core_event_code` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            node_outcome_detail: json_payload.node_outcome_detail,
            source_name: json_payload.source_name,
            relative_path: record
                .relative_path
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `relative_path` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            defined_at_line: record.code_line,
            defined_at_col: record.code_column,
            node_checksum: record
                .content_hash
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `content_hash` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            duration_ms: record.duration_ms,
            in_selection: json_payload.in_selection,
            rows_affected: record.rows_affected,
            group: record.group.as_deref().map(str::to_string),
            idle_time_ms: json_payload.idle_time_ms,
            node_index: json_payload.node_index,
            node_count_total: json_payload.node_count_total,
        })
    }
}
