//! Dbt-specific convenience helpers built on top of the generic tracing emit API.

use dbt_error::{ErrorCode, FsError};
use dbt_telemetry::{LogMessage, ProgressMessage};

use super::dbt_metrics::{CompatibilityProbe, FusionMetricKey};
use super::fs_error_log::FsErrorLog;
use super::future_incompatibility_log::FutureIncompatibilityLog;

use dbt_tracing::emit::{
    emit_debug_event, emit_error_event, emit_info_event, emit_trace_event, emit_warn_event,
};
use dbt_tracing::metrics::increment_metric;

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
    debug_assert_stable_code(code);
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
    debug_assert_stable_code(code);
    let mut log_message =
        LogMessage::new_from_level_and_code(code as u32, code.name(), tracing::Level::ERROR);
    log_message.package_name = Some(package_name.to_string());
    emit_error_event(log_message, Some(message.as_ref()));
}

/// Emit a log message event at ERROR level based on the given FsError.
#[track_caller]
pub fn emit_error_log_from_fs_error(error: FsError) {
    debug_assert_stable_code(error.code);
    let message = error.message();
    emit_error_event(
        FsErrorLog::new(error, tracing::Level::ERROR),
        Some(message.as_str()),
    );
}

/// Emit a log message event at WARN level with the given code and message.
#[track_caller]
pub fn emit_warn_log_message(code: ErrorCode, message: impl AsRef<str>) {
    debug_assert_stable_code(code);
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
    debug_assert_stable_code(code);
    let mut log_message =
        LogMessage::new_from_level_and_code(code as u32, code.name(), tracing::Level::WARN);
    log_message.package_name = Some(package_name.to_string());
    emit_warn_event(log_message, Some(message.as_ref()));
}

/// Emit a log message event at WARN level based on the given FsError.
#[track_caller]
pub fn emit_warn_log_from_fs_error(warning: FsError) {
    debug_assert_stable_code(warning.code);
    let message = warning.message();
    emit_warn_event(
        FsErrorLog::new(warning, tracing::Level::WARN),
        Some(message.as_str()),
    );
}

/// Warn about a behavior planned to change, and count it as `probe`.
///
/// For an [`ErrorCode`] registered in [`ErrorCode::future_error`], the warning carries that code
/// and says it will become an error. For a [`NamedProbe`](super::dbt_metrics::NamedProbe), it
/// carries [`ErrorCode::FutureIncompatibility`] and says the behavior may change. Either way the
/// tracking issue link is appended, and the probe is counted even when warn-error options
/// silence the warning.
#[track_caller]
pub fn emit_future_change(probe: impl Into<CompatibilityProbe>, message: impl AsRef<str>) {
    match probe.into() {
        CompatibilityProbe::ErrorCode(code) => {
            emit_future_change_from_fs_error(FsError::new_no_backtrace(code, message.as_ref()))
        }
        probe @ CompatibilityProbe::Named(named) => {
            increment_metric(FusionMetricKey::CompatibilityProbe(probe), 1);
            let issue = named.tracking_issue();
            debug_assert!(
                issue.is_some(),
                "register an issue for {} in NamedProbe::tracking_issue",
                named.name()
            );
            let message = match issue {
                Some(issue) => format!(
                    "{} This behavior may change in a future release. See {issue}",
                    message.as_ref()
                ),
                None => message.as_ref().to_string(),
            };
            let code = ErrorCode::FutureIncompatibility;
            let log_message =
                LogMessage::new_from_level_and_code(code as u32, code.name(), tracing::Level::WARN);
            emit_warn_event(
                FutureIncompatibilityLog::new(probe, log_message),
                Some(message.as_str()),
            )
        }
    }
}

/// [`emit_future_change`] for a warning that carries a location. Counted as its code.
///
/// Appends the notice to the warning's own message, so the LSP, which reads this error, shows it
/// too.
#[track_caller]
pub fn emit_future_change_from_fs_error(warning: FsError) {
    increment_metric(
        FusionMetricKey::CompatibilityProbe(CompatibilityProbe::ErrorCode(warning.code)),
        1,
    );
    debug_assert!(
        warning.code.future_error().is_some(),
        "register {} in ErrorCode::future_error",
        warning.code.name()
    );
    let warning = warning.with_future_error_notice();
    let message = warning.message();
    emit_warn_event(
        FsErrorLog::new(warning, tracing::Level::WARN),
        Some(message.as_str()),
    );
}

/// Codes planned to change are emitted with [`emit_future_change`].
fn debug_assert_stable_code(code: ErrorCode) {
    debug_assert!(
        code.future_error().is_none(),
        "emit {} with emit_future_change",
        code.name()
    );
    debug_assert!(
        code != ErrorCode::FutureIncompatibility,
        "emit FutureIncompatibility with emit_future_change and a NamedProbe"
    );
}

/// Count that execution reached a probed code path. Telemetry only; never shown to users.
/// Counted only under an invocation span.
#[track_caller]
pub fn emit_compatibility_probe(probe: impl Into<CompatibilityProbe>) {
    increment_metric(FusionMetricKey::CompatibilityProbe(probe.into()), 1);
}

/// Emit a log message related to parsing error based on the given FsError.
///
#[track_caller]
pub fn emit_strict_parse_error(error: FsError, package_name: Option<impl AsRef<str>>) {
    debug_assert_stable_code(error.code);
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
