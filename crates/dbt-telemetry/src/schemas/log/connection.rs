pub use crate::proto::v1::public::events::fusion::query::{
    AdapterConnectionClose, AdapterConnectionOpen,
};
use crate::serialize::arrow::ArrowAttributes;
use dbt_tracing::{
    ArrowSerializableTelemetryEvent, StaticTelemetryEvent, TelemetryEventRecType,
    TelemetryOutputFlags,
};
use prost::Name;

impl StaticTelemetryEvent for AdapterConnectionOpen {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!(
            "Adapter connection open ({}, {})",
            self.adapter_type, self.adapter_backend
        )
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }
}

impl ArrowSerializableTelemetryEvent for AdapterConnectionOpen {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            json_payload: serde_json::to_string(self)
                .unwrap_or_else(|_| {
                    panic!(
                        "Failed to serialize event type \"{}\" to JSON",
                        Self::full_name()
                    )
                })
                .into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
            format!(
                "Missing json payload for event type \"{}\"",
                Self::full_name()
            )
        })?)
        .map_err(|e| {
            format!(
                "Failed to deserialize event type \"{}\" from JSON: {}",
                Self::full_name(),
                e
            )
        })
    }
}

impl StaticTelemetryEvent for AdapterConnectionClose {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        "Adapter connection close".to_string()
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }
}

impl ArrowSerializableTelemetryEvent for AdapterConnectionClose {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            json_payload: serde_json::to_string(self)
                .unwrap_or_else(|_| {
                    panic!(
                        "Failed to serialize event type \"{}\" to JSON",
                        Self::full_name()
                    )
                })
                .into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
            format!(
                "Missing json payload for event type \"{}\"",
                Self::full_name()
            )
        })?)
        .map_err(|e| {
            format!(
                "Failed to deserialize event type \"{}\" from JSON: {}",
                Self::full_name(),
                e
            )
        })
    }
}
