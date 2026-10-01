pub use crate::proto::v1::public::events::fusion::dev::{CallTrace, DebugValue, Unknown};
use crate::serialize::arrow::ArrowAttributes;
use dbt_tracing::{
    AnyTelemetryEvent, ArrowSerializableTelemetryEvent, RecordCodeLocation, StaticTelemetryEvent,
    TelemetryEventRecType, TelemetryOutputFlags,
};
use prost::Name;
use std::borrow::Cow;

impl StaticTelemetryEvent for CallTrace {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        if let Some(file) = &self.file {
            return format!(
                "Dev trace: {} ({}:{})",
                self.name,
                file,
                self.line.unwrap_or(0)
            );
        }
        format!("Dev trace: {}", self.name)
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
        true
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        // CallTrace is considered sensitive as it may carry arbitrary data in
        // the `extra` field. We strip it out here.
        Some(Box::new(CallTrace {
            name: self.name.clone(),
            file: self.file.clone(),
            line: self.line,
            extra: Default::default(),
        }))
    }
}

impl ArrowSerializableTelemetryEvent for CallTrace {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            dev_name: Some(Cow::Borrowed(self.name.as_str())),
            file: self.file.as_deref().map(Cow::Borrowed),
            line: self.line,
            json_payload: serde_json::to_string(&self.extra)
                .unwrap_or_else(|_| {
                    panic!(
                        "Failed to serialize `extra` field of event type \"{}\" to JSON",
                        Self::full_name()
                    )
                })
                .into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        Ok(Self {
            name: record
                .dev_name
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `dev_name` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            file: record.file.as_deref().map(str::to_string),
            line: record.line,
            extra: serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
                format!(
                    "Missing json payload for event type \"{}\"",
                    Self::full_name()
                )
            })?)
            .map_err(|e| {
                format!(
                    "Failed to deserialize `extra` field of event type \"{}\" from JSON: {}",
                    Self::full_name(),
                    e
                )
            })?,
        })
    }
}

impl StaticTelemetryEvent for Unknown {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("Unknown span: {} ({}:{})", self.name, self.file, self.line)
    }

    fn code_location(&self) -> Option<RecordCodeLocation> {
        Some(RecordCodeLocation {
            file: Some(self.file.clone()),
            line: Some(self.line),
            ..Default::default()
        })
    }

    fn with_code_location(&mut self, location: RecordCodeLocation) {
        if let Some(file) = location.file {
            self.file = file;
        }

        if let Some(line) = location.line {
            self.line = line;
        }
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }
}

impl ArrowSerializableTelemetryEvent for Unknown {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            dev_name: Some(Cow::Borrowed(self.name.as_str())),
            file: Some(Cow::Borrowed(self.file.as_str())),
            line: Some(self.line),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        Ok(Self {
            name: record
                .dev_name
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `dev_name` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            file: record
                .file
                .as_deref()
                .map(str::to_string)
                .unwrap_or_default(),
            line: record.line.unwrap_or_default(),
        })
    }
}
