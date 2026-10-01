use dbt_telemetry::{NodeEvaluated, set_node_warning_outcome_warned};
use dbt_tracing::{LogRecordInfo, SeverityNumber};

use super::super::{
    data_provider::DataProvider, fs_error_log::get_log_message, layer::TelemetryMiddleware,
};

/// Middleware that automatically marks the current node span as having produced
/// warnings whenever a `Warn`-severity log record passes through the pipeline.
///
/// This must be placed **after** [`super::warn_error_options::TelemetryWarnErrorOptionsMiddleware`]
/// so that:
/// - Silenced warnings (dropped by that middleware) never reach here and don't set `WithWarnings`.
/// - Warnings upgraded to errors are seen at `Error` severity and are ignored here.
pub struct TelemetryNodeWarnOutcome;

impl TelemetryMiddleware for TelemetryNodeWarnOutcome {
    fn on_log_record(
        &self,
        log_record: LogRecordInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        if get_log_message(&log_record.attributes).is_some()
            && log_record.severity_number == SeverityNumber::Warn
        {
            data_provider.with_ancestor_attrs_mut::<NodeEvaluated>(|attrs| {
                set_node_warning_outcome_warned(attrs);
            });
        }
        Some(log_record)
    }
}
