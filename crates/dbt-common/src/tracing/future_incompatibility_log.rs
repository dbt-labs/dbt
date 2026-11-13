use dbt_telemetry::LogMessage;
use dbt_tracing::{
    AnyTelemetryEvent, RecordCodeLocation, TelemetryContext, TelemetryEventRecType,
    TelemetryOutputFlags, serialize::traits::ArrowAttributesSerialize,
};

use super::dbt_metrics::CompatibilityProbe;

/// A future-incompatibility warning that keeps the probe naming its behavior.
///
/// Serialized and user-facing outputs see the same event identity and payload as
/// [`LogMessage`].
#[derive(Debug, Clone)]
pub(crate) struct FutureIncompatibilityLog {
    probe: CompatibilityProbe,
    log_message: LogMessage,
}

impl FutureIncompatibilityLog {
    pub(in crate::tracing) fn new(probe: CompatibilityProbe, log_message: LogMessage) -> Self {
        Self { probe, log_message }
    }

    pub(crate) fn probe(&self) -> CompatibilityProbe {
        self.probe
    }

    pub(crate) fn get_log_message(&self) -> &LogMessage {
        &self.log_message
    }

    pub(in crate::tracing) fn get_log_message_mut(&mut self) -> &mut LogMessage {
        &mut self.log_message
    }
}

impl AnyTelemetryEvent for FutureIncompatibilityLog {
    fn event_type(&self) -> &'static str {
        self.log_message.event_type()
    }

    fn event_display_name(&self) -> String {
        self.log_message.event_display_name()
    }

    fn record_category(&self) -> TelemetryEventRecType {
        self.log_message.record_category()
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        self.log_message.output_flags()
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| self.probe == other.probe && self.log_message == other.log_message)
    }

    fn code_location(&self) -> Option<RecordCodeLocation> {
        self.log_message.code_location()
    }

    fn with_code_location(&mut self, location: RecordCodeLocation) {
        self.log_message.with_code_location(location);
    }

    fn context(&self) -> Option<TelemetryContext> {
        self.log_message.context()
    }

    fn with_context(&mut self, context: &TelemetryContext) {
        self.log_message.with_context(context);
    }

    fn has_sensitive_data(&self) -> bool {
        self.log_message.has_sensitive_data()
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
        self.log_message.to_json()
    }

    fn to_arrow(&self) -> Option<Box<dyn ArrowAttributesSerialize + '_>> {
        self.log_message.to_arrow()
    }
}

#[cfg(test)]
mod tests {
    use dbt_error::ErrorCode;
    use dbt_telemetry::LogMessage;
    use dbt_tracing::AnyTelemetryEvent;

    use super::FutureIncompatibilityLog;
    use crate::tracing::dbt_metrics::NamedProbe;

    #[test]
    fn future_incompatibility_log_eq_by_probe_and_message() {
        let message = || {
            LogMessage::new_from_level_and_code(
                ErrorCode::FutureIncompatibility as u32,
                ErrorCode::FutureIncompatibility.name(),
                tracing::Level::WARN,
            )
        };
        let original = FutureIncompatibilityLog::new(NamedProbe::TestOnly.into(), message());
        let other_probe =
            FutureIncompatibilityLog::new(NamedProbe::TestOnlyOther.into(), message());

        assert!(original.event_eq(&original.clone()));
        assert!(!original.event_eq(&other_probe));
    }
}
