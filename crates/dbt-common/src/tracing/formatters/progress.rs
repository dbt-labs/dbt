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

        let parenthetical = match progress.description.as_deref() {
            Some(desc) if !desc.is_empty() => Some(desc.to_string()),
            _ => progress
                .blocking_run_id
                .as_deref()
                .map(|id| format!("blocked by run {id}")),
        };

        match parenthetical {
            Some(text) => write!(f, " {} ({})", progress.target, text),
            None => write!(f, " {}", progress.target),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(progress: &ProgressMessage) -> String {
        ProgressMessageLine {
            progress,
            severity: SeverityNumber::Info,
            pad_action: false,
            colorize: false,
        }
        .to_string()
    }

    #[test]
    fn renders_plain_target_with_no_description_or_blocking_run_id() {
        let progress = ProgressMessage {
            action: "Waiting".to_string(),
            target: "model.a".to_string(),
            ..Default::default()
        };
        assert_eq!(render(&progress), "Waiting model.a");
    }

    #[test]
    fn renders_description_when_set() {
        let progress = ProgressMessage {
            action: "Loading".to_string(),
            target: "project".to_string(),
            description: Some("from cache".to_string()),
            ..Default::default()
        };
        assert_eq!(render(&progress), "Loading project (from cache)");
    }

    #[test]
    fn derives_parenthetical_from_blocking_run_id_when_description_is_unset() {
        let progress = ProgressMessage {
            action: "Waiting".to_string(),
            target: "model.a".to_string(),
            blocking_run_id: Some("run-123".to_string()),
            ..Default::default()
        };
        assert_eq!(
            render(&progress),
            "Waiting model.a (blocked by run run-123)"
        );
    }

    #[test]
    fn prefers_description_over_blocking_run_id_when_both_are_set() {
        let progress = ProgressMessage {
            action: "Waiting".to_string(),
            target: "model.a".to_string(),
            description: Some("custom note".to_string()),
            blocking_run_id: Some("run-123".to_string()),
            ..Default::default()
        };
        assert_eq!(render(&progress), "Waiting model.a (custom note)");
    }
}
