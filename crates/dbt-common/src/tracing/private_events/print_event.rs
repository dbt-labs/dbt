use dbt_error::ErrorCode;
use dbt_tracing::{AnyTelemetryEvent, TelemetryEventRecType, TelemetryOutputFlags};

/// Private type to wrap messages intended for stdout printing only (essentially alternative to `println!`).
#[derive(Debug)]
pub(in crate::tracing) struct StdoutMessage;

impl AnyTelemetryEvent for StdoutMessage {
    fn event_type(&self) -> &'static str {
        "v1.internal.events.fusion.log.StdoutMessage"
    }

    fn event_display_name(&self) -> String {
        "Stdout Message".to_string()
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Log
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        TelemetryOutputFlags::OUTPUT_CONSOLE
    }

    fn event_eq(&self, _: &dyn AnyTelemetryEvent) -> bool {
        false
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(Self)
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        Err("Unexpected attempt to serialize internal event".to_string())
    }
}

/// Private type to wrap messages intended for stderr printing only (essentially alternative to `eprintln!`).
#[derive(Debug, Clone)]
pub(in crate::tracing) struct StderrMessage {
    error_code: Option<ErrorCode>,
}

impl StderrMessage {
    pub(in crate::tracing) fn new(error_code: Option<ErrorCode>) -> Self {
        Self { error_code }
    }

    pub(in crate::tracing) fn error_code(&self) -> Option<ErrorCode> {
        self.error_code
    }
}

impl AnyTelemetryEvent for StderrMessage {
    fn event_type(&self) -> &'static str {
        "v1.internal.events.fusion.log.StderrMessage"
    }

    fn event_display_name(&self) -> String {
        self.error_code
            .map(|code| format!("Stderr Message ({})", code))
            .unwrap_or_else(|| "Stderr Message".to_string())
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Log
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        TelemetryOutputFlags::OUTPUT_CONSOLE
    }

    fn event_eq(&self, _: &dyn AnyTelemetryEvent) -> bool {
        false
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        Err("Unexpected attempt to serialize internal event".to_string())
    }
}
