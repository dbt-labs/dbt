pub use crate::proto::v1::public::events::fusion::log::{ShowDataOutput, ShowDataOutputFormat};
use crate::serialize::arrow::ArrowAttributes;
use dbt_tracing::{
    AnyTelemetryEvent, ArrowSerializableTelemetryEvent, StaticTelemetryEvent,
    TelemetryEventRecType, TelemetryOutputFlags,
};
use prost::Name;
use std::borrow::Cow;

impl StaticTelemetryEvent for ShowDataOutput {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!(
            "Show Data Output ({})",
            self.unique_id.as_ref().unwrap_or(&self.node_name)
        )
    }

    fn has_sensitive_data(&self) -> bool {
        // Preview data may contain sensitive information
        true
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        let content = match self.output_format() {
            ShowDataOutputFormat::Tsv
            | ShowDataOutputFormat::Csv
            | ShowDataOutputFormat::Ndjson => self.content.clone(),
            ShowDataOutputFormat::Json | ShowDataOutputFormat::Yml => "{}".to_string(),
            ShowDataOutputFormat::Unspecified | ShowDataOutputFormat::Text => {
                "[REDACTED]".to_string()
            }
        };

        Some(Box::new(Self {
            content,
            columns: vec![],
            ..self.clone()
        }))
    }
}

/// Internal struct used for serializing/deserializing subset of
/// ShowDataOutput fields as JSON payload in ArrowAttributes.
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct ShowDataOutputJsonPayload<'a> {
    pub is_inline: bool,
    pub columns: Vec<Cow<'a, str>>,
}

impl ArrowSerializableTelemetryEvent for ShowDataOutput {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            dbt_core_event_code: Some(Cow::Borrowed(self.dbt_core_event_code.as_str())),
            name: Some(Cow::Borrowed(self.node_name.as_str())),
            unique_id: self.unique_id.as_deref().map(Cow::Borrowed),
            output_format: Some(Cow::Borrowed(self.output_format().as_str_name())),
            content: Some(Cow::Borrowed(self.content.as_str())),
            // The rest of the data is serialized as JSON payload
            json_payload: serde_json::to_string(&ShowDataOutputJsonPayload {
                is_inline: self.is_inline,
                columns: self.columns.iter().map(Cow::from).collect(),
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
        let json_payload: ShowDataOutputJsonPayload =
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

        let format_str = record.output_format.as_deref().ok_or_else(|| {
            format!(
                "Missing `output_format` for event type \"{}\"",
                Self::full_name()
            )
        })?;

        let output_format = ShowDataOutputFormat::from_str_name(format_str).ok_or_else(|| {
            format!(
                "Invalid `output_format` value '{}' for event type \"{}\"",
                format_str,
                Self::full_name()
            )
        })? as i32;

        Ok(Self {
            dbt_core_event_code: record
                .dbt_core_event_code
                .as_deref()
                .map(str::to_string)
                .unwrap_or_else(|| "Q041".to_string()),
            node_name: record.name.as_deref().map(str::to_string).ok_or_else(|| {
                format!("Missing `name` for event type \"{}\"", Self::full_name())
            })?,
            unique_id: record.unique_id.as_deref().map(str::to_string),
            output_format,
            content: record
                .content
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!("Missing `content` for event type \"{}\"", Self::full_name())
                })?,
            is_inline: json_payload.is_inline,
            columns: json_payload.columns.iter().map(|s| s.to_string()).collect(),
        })
    }
}
