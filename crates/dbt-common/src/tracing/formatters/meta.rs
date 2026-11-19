use dbt_tracing::SeverityNumber;

/// Severity level as fixed-width bracketed text: [info ], [warn ], [error], etc.
pub fn severity_fixed_width(severity: SeverityNumber) -> &'static str {
    match severity {
        SeverityNumber::Error => "[error]",
        SeverityNumber::Warn => "[warn ]",
        SeverityNumber::Info => "[info ]",
        SeverityNumber::Debug => "[debug]",
        SeverityNumber::Trace => "[trace]",
        SeverityNumber::Unspecified => "[info ]",
    }
}
