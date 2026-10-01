//! Dbt-specific convenience helpers built on top of the generic tracing emit API.

use dbt_error::{ErrorCode, FsError};
use dbt_telemetry::{LogMessage, ProgressMessage};

use super::fs_error_log::FsErrorLog;

use dbt_tracing::emit::{
    emit_debug_event, emit_error_event, emit_info_event, emit_trace_event, emit_warn_event,
};

// Convenience shorthand's for common telemetry attributes

/// Emit a plain log message without error code at INFO level.
#[track_caller]
pub fn emit_info_log_message(message: impl AsRef<str>) {
    emit_info_event(
        LogMessage::new_from_level(tracing::Level::INFO),
        Some(message.as_ref()),
    )
}

/// Emit a plain log message without error code at DEBUG level.
#[track_caller]
pub fn emit_debug_log_message(message: impl AsRef<str>) {
    emit_debug_event(
        LogMessage::new_from_level(tracing::Level::DEBUG),
        Some(message.as_ref()),
    )
}

/// Emit a plain log message without error code at TRACE level.
///
/// NOTE: Trace level events are intended for fusion developer debugging and
/// turned off by default.
#[track_caller]
pub fn emit_trace_log_message(message: impl FnOnce() -> String) {
    emit_trace_event(|| {
        (
            LogMessage::new_from_level(tracing::Level::TRACE).into(),
            Some(message()),
        )
    })
}

/// Emit a log message event at ERROR level with the given code and message.
#[track_caller]
pub fn emit_error_log_message(code: ErrorCode, message: impl AsRef<str>) {
    emit_error_event(
        LogMessage::new_from_level_and_code(code as u32, code.name(), tracing::Level::ERROR),
        Some(message.as_ref()),
    );
}

/// Emit a package-scoped (coming from a dependency) error log message.
#[track_caller]
pub fn emit_error_log_message_package_scoped(
    code: ErrorCode,
    message: impl AsRef<str>,
    package_name: &str,
) {
    let mut log_message =
        LogMessage::new_from_level_and_code(code as u32, code.name(), tracing::Level::ERROR);
    log_message.package_name = Some(package_name.to_string());
    emit_error_event(log_message, Some(message.as_ref()));
}

/// Emit a log message event at ERROR level based on the given FsError.
#[track_caller]
pub fn emit_error_log_from_fs_error(error: FsError) {
    let message = error.message();
    emit_error_event(
        FsErrorLog::new(error, tracing::Level::ERROR),
        Some(message.as_str()),
    );
}

/// Emit a log message event at WARN level with the given code and message.
#[track_caller]
pub fn emit_warn_log_message(code: ErrorCode, message: impl AsRef<str>) {
    emit_warn_event(
        LogMessage::new_from_level_and_code(code as u32, code.name(), tracing::Level::WARN),
        Some(message.as_ref()),
    );
}

/// Emit a package-scoped (coming from a dependency) warning log message.
#[track_caller]
pub fn emit_warn_log_message_package_scoped(
    code: ErrorCode,
    message: impl AsRef<str>,
    package_name: &str,
) {
    let mut log_message =
        LogMessage::new_from_level_and_code(code as u32, code.name(), tracing::Level::WARN);
    log_message.package_name = Some(package_name.to_string());
    emit_warn_event(log_message, Some(message.as_ref()));
}

/// Emit a log message event at WARN level based on the given FsError.
#[track_caller]
pub fn emit_warn_log_from_fs_error(warning: FsError) {
    let message = warning.message();
    emit_warn_event(
        FsErrorLog::new(warning, tracing::Level::WARN),
        Some(message.as_str()),
    );
}

/// Emit a log message related to parsing error based on the given FsError.
///
#[track_caller]
pub fn emit_strict_parse_error(error: FsError, package_name: Option<impl AsRef<str>>) {
    let message = error.message();
    let package_name = package_name.as_ref().map(|name| name.as_ref().to_string());
    let event = FsErrorLog::new(error, tracing::Level::ERROR)
        .with_package_name(package_name)
        .with_parsing_error();
    emit_error_event(event, Some(message.as_str()));
}

// Progress messages
/// Emit a regular progress message at INFO level.
#[track_caller]
pub fn emit_info_progress_message(message: ProgressMessage) {
    emit_info_event(message, None)
}

/// Print a message on a separate line to stdout only. This should be used instead of `println!`.
#[track_caller]
pub fn println(message: impl AsRef<str>) {
    use super::private_events::print_event::StdoutMessage;

    emit_info_event(
        StdoutMessage,
        Some(format!("{}\n", message.as_ref()).as_str()),
    );
}

/// Print a message to stdout only. This should be used instead of `print!`.
#[track_caller]
pub fn print(message: impl AsRef<str>) {
    use super::private_events::print_event::StdoutMessage;

    emit_info_event(StdoutMessage, Some(message.as_ref()));
}

/// Print an error to stderr only. This should be used instead of `eprintln!`.
///
/// Takes a mandatory error code. The message will be formatted similarly
/// to how error logs are formatted: `[error] [Name (dbt####)]: <message>`,
/// error colored in red.
#[track_caller]
pub fn print_err(error_code: ErrorCode, message: impl AsRef<str>) {
    use super::private_events::print_event::StderrMessage;

    emit_error_event(StderrMessage::new(Some(error_code)), Some(message.as_ref()));
}

/// Print an error to stderr only. This should be used instead of `eprintln!`.
#[track_caller]
pub fn print_err_from_fs_error(error: &FsError) {
    print_err(error.code, error.message().as_str());
}
