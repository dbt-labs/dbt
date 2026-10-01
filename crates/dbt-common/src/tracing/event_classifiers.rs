use dbt_error::ErrorCode;
use dbt_tracing::LogRecordInfo;

use super::fs_error_log::get_log_message;

/// Checks if this log record is a pseudo error that has already been reported
/// elsewhere and should be treated as control flow by dbt-facing sinks.
///
/// This covers:
/// - ExitWithStatus: signals main() to exit with a specific status code.
/// - JinjaWarnUpgradedToError: a warn() upgraded to error; the warning was
///   already emitted by emit_warn_log_from_fs_error.
pub fn is_exit_with_status_log(log_record: &LogRecordInfo) -> bool {
    matches!(
        get_log_message(&log_record.attributes)
            .and_then(|message| message.code)
            .and_then(|code| u16::try_from(code).ok())
            .and_then(|code| ErrorCode::try_from(code).ok()),
        Some(ErrorCode::ExitWithStatus | ErrorCode::JinjaWarnUpgradedToError)
    )
}
