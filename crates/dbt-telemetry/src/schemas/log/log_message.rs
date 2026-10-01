pub use crate::proto::v1::public::events::fusion::log::{
    LogMessage, ProgressMessage, UserLogMessage,
};
use crate::{DbtTelemetryContext, serialize::arrow::ArrowAttributes};
use dbt_tracing::{
    ArrowSerializableTelemetryEvent, RecordCodeLocation, StaticTelemetryEvent, TelemetryContext,
    TelemetryEventRecType, TelemetryOutputFlags,
};
use prost::Name;
use serde_with::skip_serializing_none;
use std::borrow::Cow;

impl StaticTelemetryEvent for LogMessage {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("LogMessage ({})", self.code())
    }

    fn code_location(&self) -> Option<RecordCodeLocation> {
        Some(RecordCodeLocation {
            file: self.file.clone(),
            line: self.line,
            ..Default::default()
        })
    }

    fn with_code_location(&mut self, location: RecordCodeLocation) {
        // If we don't have a file yet, take it from the location.
        if let (None, Some(f)) = (self.file.clone(), location.file) {
            self.file = Some(f)
        }

        // If we don't have a line yet, take it from the location.
        if let (None, Some(l)) = (self.line, location.line) {
            self.line = Some(l)
        }
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn with_context(&mut self, context: &TelemetryContext) {
        let Some(context) = context.downcast_ref::<DbtTelemetryContext>() else {
            return;
        };

        // Inject unique_id if not set and provided by context
        if self.unique_id.is_none() {
            self.unique_id = context.unique_id.clone();
        }

        // Inject phase if not set and provided by context
        if self.phase.is_none()
            && let Some(p) = context.phase
        {
            self.phase = Some(p as i32);
        }
    }
}

/// Internal struct used for serializing/deserializing LogMessage fields as JSON payload.
#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug, Default)]
struct LogMessageJsonPayload {
    pub expanded_relative_path: Option<String>,
    pub expanded_line: Option<u32>,
    pub expanded_column: Option<u32>,
}

impl ArrowSerializableTelemetryEvent for LogMessage {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        let json_payload = LogMessageJsonPayload {
            expanded_relative_path: self.expanded_relative_path.clone(),
            expanded_line: self.expanded_line,
            expanded_column: self.expanded_column,
        };

        let json_payload = if json_payload.expanded_relative_path.is_none()
            && json_payload.expanded_line.is_none()
            && json_payload.expanded_column.is_none()
        {
            None
        } else {
            Some(serde_json::to_string(&json_payload).unwrap_or_else(|_| {
                panic!(
                    "Failed to serialize data in event type \"{}\" to JSON",
                    Self::full_name()
                )
            }))
        };

        ArrowAttributes {
            code: self.code,
            code_name: self.code_name.as_deref().map(Cow::Borrowed),
            dbt_core_event_code: self.dbt_core_event_code.as_deref().map(Cow::Borrowed),
            original_severity_number: Some(self.original_severity_number),
            original_severity_text: Some(Cow::Borrowed(self.original_severity_text.as_str())),
            unique_id: self.unique_id.as_deref().map(Cow::Borrowed),
            phase: self.phase.map(|_| self.phase()),
            file: self.file.as_deref().map(Cow::Borrowed),
            line: self.line,
            package_name: self.package_name.as_deref().map(Cow::Borrowed),
            relative_path: self.relative_path.as_deref().map(Cow::Borrowed),
            code_line: self.code_line,
            code_column: self.code_column,
            json_payload,
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload: Option<LogMessageJsonPayload> = match record.json_payload.as_ref() {
            Some(payload) => Some(serde_json::from_str(payload).map_err(|e| {
                format!(
                    "Failed to deserialize data of event type \"{}\" from JSON payload: {}",
                    Self::full_name(),
                    e
                )
            })?),
            None => None,
        };

        Ok(Self {
            code: record.code,
            code_name: record.code_name.as_deref().map(str::to_string),
            dbt_core_event_code: record.dbt_core_event_code.as_deref().map(str::to_string),
            original_severity_number: record.original_severity_number.ok_or_else(|| {
                format!(
                    "Missing severity number in event type \"{}\"",
                    Self::full_name()
                )
            })?,
            original_severity_text: record
                .original_severity_text
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing severity text in event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            unique_id: record.unique_id.as_deref().map(str::to_string),
            phase: record.phase.map(|v| v as i32),
            file: record.file.as_deref().map(str::to_string),
            line: record.line,
            package_name: record.package_name.as_deref().map(str::to_string),
            relative_path: record.relative_path.as_deref().map(str::to_string),
            code_line: record.code_line,
            code_column: record.code_column,
            expanded_relative_path: json_payload
                .as_ref()
                .and_then(|payload| payload.expanded_relative_path.clone()),
            expanded_line: json_payload
                .as_ref()
                .and_then(|payload| payload.expanded_line),
            expanded_column: json_payload
                .as_ref()
                .and_then(|payload| payload.expanded_column),
        })
    }
}

impl StaticTelemetryEvent for UserLogMessage {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!(
            "User LogMessage ({})",
            if self.is_print { "print" } else { "log" }
        )
    }

    fn has_sensitive_data(&self) -> bool {
        // None of the structured data is sensitive. Message itself can be,
        // but that's not part of this struct.
        false
    }

    fn with_context(&mut self, context: &TelemetryContext) {
        let Some(context) = context.downcast_ref::<DbtTelemetryContext>() else {
            return;
        };

        // Inject unique_id if not set and provided by context
        if self.unique_id.is_none() {
            self.unique_id = context.unique_id.clone();
        }

        // Inject phase if not set and provided by context
        if self.phase.is_none()
            && let Some(p) = context.phase
        {
            self.phase = Some(p as i32);
        }
    }
}

/// Internal struct used for serializing/deserializing subset of
/// UserLogMessage fields as JSON payload in ArrowAttributes.
#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct UserLogMessageJsonPayload {
    pub is_print: bool,
    // Legacy (pre preview.70) fields
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub relative_path: Option<String>,
}

impl ArrowSerializableTelemetryEvent for UserLogMessage {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            dbt_core_event_code: Some(Cow::Borrowed(self.dbt_core_event_code.as_str())),
            unique_id: self.unique_id.as_deref().map(Cow::Borrowed),
            phase: self.phase.map(|_| self.phase()),
            package_name: self.package_name.as_deref().map(Cow::Borrowed),
            relative_path: self.relative_path.as_deref().map(Cow::Borrowed),
            code_line: self.line,
            code_column: self.column,
            // The rest of the data is serialized as JSON payload
            json_payload: serde_json::to_string(&UserLogMessageJsonPayload {
                is_print: self.is_print,
                // Legacy (pre preview.70) fields
                line: None,
                column: None,
                relative_path: None,
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
        let json_payload: UserLogMessageJsonPayload =
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
            is_print: json_payload.is_print,
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
            unique_id: record.unique_id.as_deref().map(str::to_string),
            phase: record.phase.map(|v| v as i32),
            package_name: record.package_name.as_deref().map(str::to_string),
            line: record.code_line.or(json_payload.line),
            column: record.code_column.or(json_payload.column),
            relative_path: record
                .relative_path
                .as_deref()
                .map(str::to_string)
                .or(json_payload.relative_path),
        })
    }
}

impl StaticTelemetryEvent for ProgressMessage {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Log;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("ProgressMessage: {} {}", self.action, self.target)
    }

    fn with_code_location(&mut self, location: RecordCodeLocation) {
        // If we don't have a file yet, take it from the location.
        if let (None, Some(f)) = (self.file.clone(), location.file) {
            self.file = Some(f)
        }

        // If we don't have a line yet, take it from the location.
        if let (None, Some(l)) = (self.line, location.line) {
            self.line = Some(l)
        }
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn with_context(&mut self, context: &TelemetryContext) {
        let Some(context) = context.downcast_ref::<DbtTelemetryContext>() else {
            return;
        };

        // Inject unique_id if not set and provided by context
        if self.unique_id.is_none() {
            self.unique_id = context.unique_id.clone();
        }

        // Inject phase if not set and provided by context
        if self.phase.is_none()
            && let Some(p) = context.phase
        {
            self.phase = Some(p as i32);
        }
    }
}

/// Internal struct used for serializing/deserializing ProgressMessage fields as JSON payload.
#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct ProgressMessageJsonPayload<'a> {
    pub action: Cow<'a, str>,
    pub target: Cow<'a, str>,
    pub description: Option<Cow<'a, str>>,
}

impl ArrowSerializableTelemetryEvent for ProgressMessage {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            dbt_core_event_code: self.dbt_core_event_code.as_deref().map(Cow::Borrowed),
            unique_id: self.unique_id.as_deref().map(Cow::Borrowed),
            phase: self.phase.map(|_| self.phase()),
            file: self.file.as_deref().map(Cow::Borrowed),
            line: self.line,
            json_payload: serde_json::to_string(&ProgressMessageJsonPayload {
                action: self.action.as_str().into(),
                target: self.target.as_str().into(),
                description: self.description.as_deref().map(Cow::Borrowed),
            })
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to serialize data in event type \"{}\" to JSON: {e:?}",
                    Self::full_name()
                )
            })
            .into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload: ProgressMessageJsonPayload =
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
            dbt_core_event_code: record.dbt_core_event_code.as_deref().map(str::to_string),
            action: json_payload.action.into_owned(),
            target: json_payload.target.into_owned(),
            description: json_payload.description.as_deref().map(str::to_string),
            unique_id: record.unique_id.as_deref().map(str::to_string),
            file: record.file.as_deref().map(str::to_string),
            line: record.line,
            phase: record.phase.map(|v| v as i32),
        })
    }
}
