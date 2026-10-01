use std::sync::{Arc, OnceLock};

use dbt_error::FsError;
use dbt_telemetry::LogMessage;
use dbt_tracing::{
    AnyTelemetryEvent, RecordCodeLocation, TelemetryAttributes, TelemetryContext,
    TelemetryEventRecType, TelemetryOutputFlags, serialize::traits::ArrowAttributesSerialize,
};

/// A log event that retains its source error for in-process consumers.
///
/// Serialized and user-facing outputs see the same event identity and payload as
/// [`LogMessage`]. The retained error is intentionally available only through a
/// borrowed accessor.
#[derive(Debug)]
pub struct FsErrorLog {
    fs_error: Arc<FsError>,
    log_message: OnceLock<LogMessage>,
    level: tracing::Level,
    package_name: Option<String>,
    parsing_error: bool,
}

impl FsErrorLog {
    pub fn new(error: FsError, level: tracing::Level) -> Self {
        Self {
            fs_error: Arc::new(error),
            log_message: OnceLock::new(),
            level,
            package_name: None,
            parsing_error: false,
        }
    }

    pub fn with_package_name(mut self, package_name: Option<String>) -> Self {
        self.package_name = package_name;
        self
    }

    pub(in crate::tracing) fn with_parsing_error(mut self) -> Self {
        self.parsing_error = true;
        self
    }

    pub(in crate::tracing) fn is_parsing_error(&self) -> bool {
        self.parsing_error
    }

    pub fn get_fs_error(&self) -> &FsError {
        self.fs_error.as_ref()
    }

    pub fn get_log_message(&self) -> &LogMessage {
        self.log_message.get_or_init(|| {
            let error = self.get_fs_error();
            let mut log_message = LogMessage::new_from_level_and_code(
                error.code as u32,
                error.code.name(),
                self.level,
            );
            log_message.package_name.clone_from(&self.package_name);

            if let Some(location) = error.location.as_ref() {
                log_message.relative_path =
                    Some(location.relative_path().to_string_lossy().to_string());
                log_message.code_line = location.line_opt();
                log_message.code_column = location.col_opt();

                if let Some(expanded) = location.expanded() {
                    log_message.expanded_relative_path =
                        Some(expanded.relative_path().to_string_lossy().to_string());
                    log_message.expanded_line = expanded.line_opt();
                    log_message.expanded_column = expanded.col_opt();
                }
            }

            log_message
        })
    }

    pub(in crate::tracing) fn get_log_message_mut(&mut self) -> &mut LogMessage {
        let _ = self.get_log_message();
        self.log_message
            .get_mut()
            .expect("log message was initialized above")
    }
}

impl Clone for FsErrorLog {
    fn clone(&self) -> Self {
        Self {
            fs_error: Arc::clone(&self.fs_error),
            log_message: self.log_message.clone(),
            level: self.level,
            package_name: self.package_name.clone(),
            parsing_error: self.parsing_error,
        }
    }
}

impl AnyTelemetryEvent for FsErrorLog {
    fn event_type(&self) -> &'static str {
        self.get_log_message().event_type()
    }

    fn event_display_name(&self) -> String {
        self.get_log_message().event_display_name()
    }

    fn record_category(&self) -> TelemetryEventRecType {
        self.get_log_message().record_category()
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        self.get_log_message().output_flags()
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| Arc::ptr_eq(&self.fs_error, &other.fs_error))
    }

    fn code_location(&self) -> Option<RecordCodeLocation> {
        self.get_log_message().code_location()
    }

    fn with_code_location(&mut self, location: RecordCodeLocation) {
        self.get_log_message_mut().with_code_location(location);
    }

    fn context(&self) -> Option<TelemetryContext> {
        self.get_log_message().context()
    }

    fn with_context(&mut self, context: &TelemetryContext) {
        self.get_log_message_mut().with_context(context);
    }

    fn has_sensitive_data(&self) -> bool {
        self.get_log_message().has_sensitive_data()
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
        self.get_log_message().to_json()
    }

    fn to_arrow(&self) -> Option<Box<dyn ArrowAttributesSerialize + '_>> {
        self.get_log_message().to_arrow()
    }
}

pub fn get_log_message(attributes: &TelemetryAttributes) -> Option<&LogMessage> {
    attributes.downcast_ref::<LogMessage>().or_else(|| {
        attributes
            .downcast_ref::<FsErrorLog>()
            .map(FsErrorLog::get_log_message)
    })
}

pub(in crate::tracing) fn get_log_message_mut(
    attributes: &mut TelemetryAttributes,
) -> Option<&mut LogMessage> {
    if attributes.is::<LogMessage>() {
        attributes.downcast_mut::<LogMessage>()
    } else {
        attributes
            .downcast_mut::<FsErrorLog>()
            .map(FsErrorLog::get_log_message_mut)
    }
}
