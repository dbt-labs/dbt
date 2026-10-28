use std::fmt;

use console::Style;
use dbt_telemetry::ProgressMessage;
use dbt_tracing::SeverityNumber;

use super::{
    color::{GREEN, MaybeColored, PLAIN, RED, YELLOW},
    layout::RightAligned,
};

/// Map severity number to corresponding color style.
///
/// Unlike with log messages, progress messages use green for info severity.
pub fn severity_to_color_style(severity_number: SeverityNumber) -> &'static Style {
    match severity_number {
        SeverityNumber::Info => &GREEN,
        SeverityNumber::Error => &RED,
        SeverityNumber::Warn => &YELLOW,
        SeverityNumber::Unspecified | SeverityNumber::Trace | SeverityNumber::Debug => &PLAIN,
    }
}

/// Displays a progress message: its action, optionally right aligned, then its target
/// and description.
pub struct ProgressMessageLine<'a> {
    pub progress: &'a ProgressMessage,
    pub severity: SeverityNumber,
    pub pad_action: bool,
    pub colorize: bool,
}

impl fmt::Display for ProgressMessageLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let progress = self.progress;
        let style = severity_to_color_style(self.severity);
        let action = progress.action.as_str();
        if self.pad_action {
            write!(
                f,
                "{}",
                MaybeColored::new(style, RightAligned(action), self.colorize)
            )?;
        } else {
            write!(f, "{}", MaybeColored::new(style, action, self.colorize))?;
        }

        match progress.description.as_ref() {
            Some(desc) if !desc.is_empty() => {
                write!(f, " {} ({})", progress.target, desc)
            }
            _ => write!(f, " {}", progress.target),
        }
    }
}
