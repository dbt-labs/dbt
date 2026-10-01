use crate::serialize::arrow::ArrowAttributes;
use dbt_tracing::{
    AnyTelemetryEvent, ArrowSerializableTelemetryEvent, StaticTelemetryEvent,
    TelemetryEventRecType, TelemetryOutputFlags,
};
use prost::Name;
use std::borrow::Cow;

pub use crate::proto::v1::public::events::fusion::log::{CompiledCode, CompiledCodeInline};

impl StaticTelemetryEvent for CompiledCode {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("Compiled SQL ({})", self.unique_id)
    }

    fn has_sensitive_data(&self) -> bool {
        true
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        Some(Box::new(Self {
            sql: "[REDACTED]".to_string(),
            ..self.clone()
        }))
    }
}

impl ArrowSerializableTelemetryEvent for CompiledCode {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            unique_id: Some(Cow::Borrowed(self.unique_id.as_str())),
            name: Some(Cow::Borrowed(self.node_name.as_str())),
            relative_path: Some(Cow::Borrowed(self.relative_path.as_str())),
            content: Some(Cow::Borrowed(self.sql.as_str())),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        if let (Some(unique_id), Some(node_name), Some(relative_path), Some(sql)) = (
            record.unique_id.as_deref(),
            record.name.as_deref(),
            record.relative_path.as_deref(),
            record.content.as_deref(),
        ) {
            return Ok(Self {
                unique_id: unique_id.to_string(),
                node_name: node_name.to_string(),
                relative_path: relative_path.to_string(),
                sql: sql.to_string(),
            });
        }

        // Backward compatibility fallback for older rows that stored everything in json_payload.
        serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
            format!(
                "Missing required fields (`unique_id`, `name`, `relative_path`, `content`) and json payload for event type \"{}\"",
                Self::full_name()
            )
        })?)
        .map_err(|e| {
            format!(
                "Failed to deserialize legacy {} from JSON payload: {}",
                Self::full_name(),
                e
            )
        })
    }
}

impl StaticTelemetryEvent for CompiledCodeInline {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        "Compiled SQL (inline)".to_string()
    }

    fn has_sensitive_data(&self) -> bool {
        true
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        Some(Box::new(Self {
            sql: "[REDACTED]".to_string(),
        }))
    }
}

impl ArrowSerializableTelemetryEvent for CompiledCodeInline {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            content: Some(Cow::Borrowed(self.sql.as_str())),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        if let Some(sql) = record.content.as_deref() {
            return Ok(Self {
                sql: sql.to_string(),
            });
        }

        // Backward compatibility fallback for older rows that stored everything in json_payload.
        serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
            format!(
                "Missing required field (`content`) and json payload for event type \"{}\"",
                Self::full_name()
            )
        })?)
        .map_err(|e| {
            format!(
                "Failed to deserialize legacy {} from JSON payload: {}",
                Self::full_name(),
                e
            )
        })
    }
}
