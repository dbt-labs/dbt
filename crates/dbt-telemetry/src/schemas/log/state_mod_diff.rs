pub use crate::proto::v1::public::events::fusion::log::StateModifiedDiff;
use crate::{DbtTelemetryContext, serialize::arrow::ArrowAttributes};
use dbt_tracing::{
    ArrowSerializableTelemetryEvent, StaticTelemetryEvent, TelemetryContext, TelemetryEventRecType,
    TelemetryOutputFlags,
};
use prost::Name;
use serde_with::skip_serializing_none;
use std::borrow::Cow;

impl StaticTelemetryEvent for StateModifiedDiff {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!(
            "State Modified Diff ({})",
            self.unique_id.as_deref().unwrap_or("unknown")
        )
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn with_context(&mut self, context: &TelemetryContext) {
        let Some(context) = context.downcast_ref::<DbtTelemetryContext>() else {
            return;
        };

        if self.unique_id.is_none() {
            self.unique_id = context.unique_id.clone();
        }
    }
}

#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct StateModifiedDiffJsonPayload {
    pub node_type_or_category: String,
    pub check: String,
    pub self_value: Option<String>,
    pub other_value: Option<String>,
}

impl ArrowSerializableTelemetryEvent for StateModifiedDiff {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            unique_id: self.unique_id.as_deref().map(Cow::Borrowed),
            json_payload: serde_json::to_string(&StateModifiedDiffJsonPayload {
                node_type_or_category: self.node_type_or_category.clone(),
                check: self.check.clone(),
                self_value: self.self_value.clone(),
                other_value: self.other_value.clone(),
            })
            .unwrap_or_else(|_| {
                panic!(
                    "Failed to serialize data in event type \"{}\" to JSON",
                    Self::full_name()
                )
            })
            .into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload: StateModifiedDiffJsonPayload =
            serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
                format!(
                    "Missing json payload for event type \"{}\"",
                    Self::full_name()
                )
            })?)
            .map_err(|e| {
                format!(
                    "Failed to deserialize data of event type \"{}\" from JSON payload: {}",
                    Self::full_name(),
                    e
                )
            })?;

        Ok(Self {
            unique_id: record.unique_id.as_deref().map(str::to_string),
            node_type_or_category: json_payload.node_type_or_category,
            check: json_payload.check,
            self_value: json_payload.self_value,
            other_value: json_payload.other_value,
        })
    }
}
