use crate::{ExecutionPhase, attributes::DbtTelemetryContext, serialize::arrow::ArrowAttributes};
use dbt_tracing::{
    ArrowSerializableTelemetryEvent, SpanStatus, StaticTelemetryEvent, TelemetryContext,
    TelemetryEventRecType, TelemetryOutputFlags,
};
use prost::Name;
use serde_with::skip_serializing_none;

pub use crate::proto::v1::public::events::fusion::hook::{HookOutcome, HookProcessed, HookType};

impl StaticTelemetryEvent for HookProcessed {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("Hook processed ({})", self.unique_id)
    }

    fn get_span_status(&self) -> Option<SpanStatus> {
        match self.hook_outcome() {
            HookOutcome::Success => SpanStatus::succeeded().into(),
            HookOutcome::Error => SpanStatus::failed("error").into(),
            HookOutcome::Canceled => SpanStatus::failed("canceled").into(),
            HookOutcome::Unspecified => None,
        }
    }

    fn has_sensitive_data(&self) -> bool {
        false
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

    fn with_context(&mut self, context: &TelemetryContext) {
        let Some(context) = context.downcast_ref::<DbtTelemetryContext>() else {
            return;
        };

        // Inject phase from context if not set
        if let Some(ctx_phase) = context.phase
            && self.phase() == ExecutionPhase::Unspecified
        {
            self.set_phase(ctx_phase);
        }
    }
}

/// Internal struct used for serializing/deserializing subset of
/// HookProcessed fields as JSON payload in ArrowAttributes.
#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct HookProcessedJsonPayload {
    hook_type: HookType,
    hook_index: u32,
    hook_outcome: HookOutcome,
}

impl ArrowSerializableTelemetryEvent for HookProcessed {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        use std::borrow::Cow;

        ArrowAttributes {
            package_name: Some(self.package_name.as_str().into()),
            name: self.name.as_deref().map(Cow::Borrowed),
            unique_id: Some(Cow::Borrowed(self.unique_id.as_str())),
            dbt_core_event_code: Some(Cow::Borrowed(self.dbt_core_event_code.as_str())),
            phase: Some(self.phase()),
            json_payload: Some(
                serde_json::to_string(&HookProcessedJsonPayload {
                    hook_outcome: self.hook_outcome(),
                    hook_type: self.hook_type(),
                    hook_index: self.hook_index,
                })
                .unwrap_or_else(|_| {
                    panic!(
                        "Failed to serialize data in event type \"{}\" to JSON",
                        Self::full_name()
                    )
                }),
            ),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload: HookProcessedJsonPayload = record
            .json_payload
            .as_ref()
            .ok_or_else(|| {
                format!(
                    "Missing json payload for event type \"{}\"",
                    Self::full_name()
                )
            })
            .and_then(|s| {
                serde_json::from_str(s).map_err(|e| {
                    format!(
                        "Failed to deserialize event type \"{}\" from JSON: {e}",
                        Self::full_name(),
                    )
                })
            })?;

        Ok(Self {
            package_name: record
                .package_name
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `package_name` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            name: record.name.as_deref().map(str::to_string),
            hook_type: json_payload.hook_type as i32,
            hook_index: json_payload.hook_index,
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
            hook_outcome: json_payload.hook_outcome as i32,
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
            phase: record.phase.map(|v| v as i32).ok_or_else(|| {
                format!("Missing `phase` for event type \"{}\"", Self::full_name())
            })?,
        })
    }
}
