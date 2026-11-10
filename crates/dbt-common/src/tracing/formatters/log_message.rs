//! Formatter for log messages.
//!
//! This module provides formatters that add error code prefixes and optional
//! color formatting to messages.

use std::fmt;

use dbt_error::ErrorCode;
use dbt_tracing::SeverityNumber;

use crate::{constants, pretty_string::ColoredQuotes};

use super::color::{MaybeColored, severity_to_color_style};

pub fn severity_to_prefix(severity_number: SeverityNumber) -> Option<&'static str> {
    match severity_number {
        SeverityNumber::Error => Some(constants::ERROR),
        SeverityNumber::Warn => Some(constants::WARNING),
        SeverityNumber::Unspecified
        | SeverityNumber::Trace
        | SeverityNumber::Debug
        | SeverityNumber::Info => None,
    }
}

/// Displays a message with "[severity] [name (dbt{code})]: " prefix and optional
/// colorization, e.g. "[warning] [Generic (dbt1000)]: message text".
pub struct LogMessageLine<'a> {
    /// Error code to include in the prefix
    pub error_code: Option<ErrorCode>,
    /// The message text
    pub message: &'a str,
    /// The severity level of the message
    pub severity: SeverityNumber,
    /// Applies the severity color to the level prefix and highlights quoted text
    pub colorize: bool,
    /// Includes the severity level prefix (ERROR, WARNING, etc.)
    pub include_level_prefix: bool,
}

impl fmt::Display for LogMessageLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level_prefix = self
            .include_level_prefix
            .then(|| severity_to_prefix(self.severity))
            .flatten();

        if let Some(prefix_text) = level_prefix {
            let style = severity_to_color_style(self.severity);
            write!(
                f,
                "{}",
                MaybeColored::new(style, Bracketed(prefix_text), self.colorize)
            )?;
        }
        if let Some(code) = self.error_code {
            if level_prefix.is_some() {
                f.write_str(" ")?;
            }
            write!(f, "[{} (dbt{code})]", code.name())?;
        }
        if level_prefix.is_some() || self.error_code.is_some() {
            f.write_str(": ")?;
        }

        if self.colorize {
            ColoredQuotes(self.message).fmt_forced(f)
        } else {
            f.write_str(self.message)
        }
    }
}

/// `text` in square brackets.
struct Bracketed<'a>(&'a str);

impl fmt::Display for Bracketed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.0)
    }
}
