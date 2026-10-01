pub use crate::proto::v1::public::events::fusion::log::{ListItemOutput, ListOutputFormat};
use crate::serialize::arrow::ArrowAttributes;
use dbt_tracing::{
    ArrowSerializableTelemetryEvent, StaticTelemetryEvent, TelemetryEventRecType,
    TelemetryOutputFlags,
};
use prost::Name;
use std::borrow::Cow;

impl StaticTelemetryEvent for ListItemOutput {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        // Convert enum for display
        let format_name = self
            .output_format()
            .as_str_name()
            .trim_start_matches("LIST_OUTPUT_FORMAT_");

        format!("List Item Output ({})", format_name)
    }

    fn has_sensitive_data(&self) -> bool {
        // As of today, we assume node info is never sensitive.
        false
    }
}

impl ArrowSerializableTelemetryEvent for ListItemOutput {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            dbt_core_event_code: Some(Cow::Borrowed("Z052")),
            output_format: Some(Cow::Borrowed(self.output_format().as_str_name())),
            content: Some(Cow::Borrowed(self.content.as_str())),
            unique_id: self.unique_id.as_deref().map(Cow::Borrowed),
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

        let output_format = ListOutputFormat::from_str_name(format_str).ok_or_else(|| {
            format!(
                "Invalid `output_format` value '{}' for event type \"{}\"",
                format_str,
                Self::full_name()
            )
        })? as i32;

        Ok(Self {
            output_format,
            content: record
                .content
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!("Missing `content` for event type \"{}\"", Self::full_name())
                })?,
            unique_id: record.unique_id.as_deref().map(str::to_string),
        })
    }
}
