use crate::{attributes::DbtTelemetryContext, serialize::arrow::ArrowAttributes};
use dbt_tracing::{
    ArrowSerializableTelemetryEvent, SpanStatus, StaticTelemetryEvent, StatusCode,
    TelemetryContext, TelemetryEventRecType, TelemetryOutputFlags,
};
use prost::Name;

pub use crate::proto::v1::public::events::fusion::phase::{ExecutionPhase, PhaseExecuted};
use serde_with::skip_serializing_none;

impl StaticTelemetryEvent for PhaseExecuted {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        // Get the all caps phase name without the "EXECUTION_PHASE_" prefix
        let p = self
            .phase()
            .as_str_name()
            .trim_start_matches("EXECUTION_PHASE_");

        match self.node_count_total {
            Some(c) => format!("Phase: {p} (nodes: {c})"),
            None => format!("Phase: {p}"),
        }
    }

    fn get_span_status(&self) -> Option<SpanStatus> {
        let Some(err_count) = self.node_count_error else {
            // If we don't have node error count, we can't determine status
            // based on event data alone.
            return None;
        };

        let status = if err_count > 0 {
            SpanStatus {
                code: StatusCode::Error,
                message: Some(format!("{err_count} nodes errored")),
            }
        } else {
            SpanStatus::succeeded()
        };

        Some(status)
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn context(&self) -> Option<TelemetryContext> {
        Some(
            DbtTelemetryContext {
                phase: Some(self.phase()),
                unique_id: None,
            }
            .into(),
        )
    }
}

/// Internal struct used for serializing/deserializing subset of
/// PhaseExecuted fields as JSON payload in ArrowAttributes.
#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct PhaseExecutedJsonPayload {
    /// Optional count of total individual nodes within the phase (when applicable).
    pub node_count_total: Option<u64>,
    /// Optional count of skipped nodes within the phase (when applicable).
    /// Skipped means `node_outcome` was set to `NODE_OUTCOME_SKIPPED`.
    pub node_count_skipped: Option<u64>,
    /// Optional count of errored nodes within the phase (when applicable).
    /// Error means `node_outcome` was set to `NODE_OUTCOME_ERROR`.
    pub node_count_error: Option<u64>,
}

impl ArrowSerializableTelemetryEvent for PhaseExecuted {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            json_payload: serde_json::to_string(&PhaseExecutedJsonPayload {
                node_count_total: self.node_count_total,
                node_count_skipped: self.node_count_skipped,
                node_count_error: self.node_count_error,
            })
            .unwrap_or_else(|_| {
                panic!(
                    "Failed to serialize counts of event type \"{}\" to JSON",
                    Self::full_name()
                )
            })
            .into(),
            phase: self.phase().into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload: PhaseExecutedJsonPayload =
            serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
                format!(
                    "Missing json payload for event type \"{}\"",
                    Self::full_name()
                )
            })?)
            .map_err(|e| {
                format!(
                    "Failed to deserialize counts of event type \"{}\" from JSON payload: {}",
                    Self::full_name(),
                    e
                )
            })?;

        Ok(Self {
            phase: record.phase.map(|v| v as i32).ok_or_else(|| {
                format!("Missing `phase` for event type \"{}\"", Self::full_name())
            })?,
            node_count_total: json_payload.node_count_total,
            node_count_skipped: json_payload.node_count_skipped,
            node_count_error: json_payload.node_count_error,
        })
    }
}
