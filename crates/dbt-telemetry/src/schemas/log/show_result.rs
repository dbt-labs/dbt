pub use crate::proto::v1::public::events::fusion::log::{ShowResult, ShowResultOutputFormat};
use crate::serialize::arrow::ArrowAttributes;
use dbt_tracing::{
    AnyTelemetryEvent, ArrowSerializableTelemetryEvent, StaticTelemetryEvent,
    TelemetryEventRecType, TelemetryOutputFlags,
};
use prost::Name;
use std::borrow::Cow;

/// Internal struct used for serializing/deserializing subset of
/// ShowResult fields as JSON payload in ArrowAttributes.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct ShowResultJsonPayload<'a> {
    pub title: Cow<'a, str>,
}

impl StaticTelemetryEvent for ShowResult {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("Show Result ({})", self.result_type)
    }

    fn has_sensitive_data(&self) -> bool {
        // Content may contain sensitive information depending on result_type
        true
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        Some(Box::new(Self {
            content: "[REDACTED]".to_string(),
            ..self.clone()
        }))
    }
}

impl ArrowSerializableTelemetryEvent for ShowResult {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            name: Some(Cow::Borrowed(self.result_type.as_str())),
            unique_id: self.unique_id.as_deref().map(Cow::Borrowed),
            output_format: Some(Cow::Borrowed(self.output_format().as_str_name())),
            content: Some(Cow::Borrowed(self.content.as_str())),
            // Store title in json_payload
            json_payload: serde_json::to_string(&ShowResultJsonPayload {
                title: Cow::Borrowed(self.title.as_str()),
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
        let format_str = record.output_format.as_deref().ok_or_else(|| {
            format!(
                "Missing `output_format` for event type \"{}\"",
                Self::full_name()
            )
        })?;

        let output_format = ShowResultOutputFormat::from_str_name(format_str).ok_or_else(|| {
            format!(
                "Invalid `output_format` value '{}' for event type \"{}\"",
                format_str,
                Self::full_name()
            )
        })? as i32;

        let json_payload: ShowResultJsonPayload =
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
            output_format,
            content: record
                .content
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!("Missing `content` for event type \"{}\"", Self::full_name())
                })?,
            result_type: record.name.as_deref().map(str::to_string).ok_or_else(|| {
                format!(
                    "Missing `name` (result_type) for event type \"{}\"",
                    Self::full_name()
                )
            })?,
            title: json_payload.title.into_owned(),
            unique_id: record.unique_id.as_deref().map(str::to_string),
        })
    }
}
