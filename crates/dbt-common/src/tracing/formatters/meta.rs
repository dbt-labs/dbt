use dbt_tracing::SeverityNumber;

/// Format severity level as fixed-width bracketed string: [info ], [warn ], [error], etc.
pub fn format_severity_fixed_width(severity: SeverityNumber) -> String {
    let level_str = match severity {
        SeverityNumber::Error => "error",
        SeverityNumber::Warn => "warn ",
        SeverityNumber::Info => "info ",
        SeverityNumber::Debug => "debug",
        SeverityNumber::Trace => "trace",
        SeverityNumber::Unspecified => "info ",
    };
    format!("[{}]", level_str)
}
