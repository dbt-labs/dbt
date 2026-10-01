use std::path::Path;

use dbt_tracing::{LogRecordInfo, SeverityNumber};

use super::super::{
    data_provider::DataProvider, fs_error_log::get_log_message, layer::TelemetryMiddleware,
};

pub struct TelemetryMarkdownLogFilter;

impl TelemetryMarkdownLogFilter {
    fn is_markdown_path(path: &str) -> bool {
        Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("md"))
            .unwrap_or(false)
    }
}

impl TelemetryMiddleware for TelemetryMarkdownLogFilter {
    fn on_log_record(
        &self,
        mut record: LogRecordInfo,
        _data_provider: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        if record.severity_number != SeverityNumber::Error {
            return Some(record);
        }

        let Some(log_message) = get_log_message(&record.attributes) else {
            return Some(record);
        };
        let path = log_message
            .relative_path
            .as_deref()
            .or(log_message.expanded_relative_path.as_deref());

        if path.is_some_and(Self::is_markdown_path) {
            record.severity_number = SeverityNumber::Warn;
            record.severity_text = SeverityNumber::Warn.as_str().to_string();
        }

        Some(record)
    }
}
