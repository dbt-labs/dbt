use std::fmt;

use console::Style;
use dbt_telemetry::{GenericOpExecuted, GenericOpItemProcessed};
use dbt_tracing::{SpanStatus, StatusCode};

use super::{
    color::{GREEN, MaybeColored, RED},
    duration::FixedWidthDuration,
    layout::{CapitalizedFirst, RightAligned},
};

/// Displays a `GenericOpExecuted` event for the start of an operation
///
/// Writes the pattern:
/// `Started {display_action}` or `Started {display_action} ({total} items)` if total > 0
pub struct GenericOpStart<'a> {
    pub op: &'a GenericOpExecuted,
    pub colorize: bool,
}

impl fmt::Display for GenericOpStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total = self.op.item_count_total.unwrap_or_default();
        let action = MaybeColored::new(&GREEN, RightAligned("Started"), self.colorize);

        if total > 0 {
            write!(f, "{} {} ({} items)", action, self.op.display_action, total)
        } else {
            write!(f, "{} {}", action, self.op.display_action)
        }
    }
}

/// Displays a `GenericOpExecuted` event for the end of an operation
///
/// Writes the pattern:
/// `Finished {display_action} [duration]`
pub struct GenericOpEnd<'a> {
    pub op: &'a GenericOpExecuted,
    pub duration: std::time::Duration,
    pub status: Option<&'a SpanStatus>,
    pub colorize: bool,
}

impl fmt::Display for GenericOpEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let duration = FixedWidthDuration(self.duration);
        let total = self.op.item_count_total.unwrap_or_default();
        let display_action = &self.op.display_action;
        let (action, error_desc) = action_with_status_code("Finished", self.status, self.colorize);

        if let Some(error) = error_desc {
            write!(f, "{action} [{duration}] {display_action} (error: {error})")
        } else if total > 0 {
            write!(f, "{action} [{duration}] {display_action} ({total} items)")
        } else {
            write!(f, "{action} [{duration}] {display_action}")
        }
    }
}

/// Displays a `GenericOpItemProcessed` event for the start of an item
///
/// Writes the pattern:
/// `Started {display_action} {target}`
pub struct GenericOpItemStart<'a>(pub &'a GenericOpItemProcessed);

impl fmt::Display for GenericOpItemStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} {}",
            RightAligned("Started"),
            self.0.display_in_progress_action,
            self.0.target
        )
    }
}

/// Displays a `GenericOpItemProcessed` event for the end of an item
///
/// Writes the pattern:
/// `{target} [duration]`
pub struct GenericOpItemEnd<'a> {
    pub item: &'a GenericOpItemProcessed,
    pub duration: std::time::Duration,
    pub status: Option<&'a SpanStatus>,
    pub colorize: bool,
}

impl fmt::Display for GenericOpItemEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let duration = FixedWidthDuration(self.duration);
        let target = &self.item.target;
        let (action, error_desc) = action_with_status_code(
            self.item.display_on_success_action.as_str(),
            self.status,
            self.colorize,
        );

        if let Some(error) = error_desc {
            write!(f, "{action} [{duration}] {target} (error: {error})")
        } else {
            write!(f, "{action} [{duration}] {target}")
        }
    }
}

/// Resolves the action shown for the provided span status: `on_success_action`,
/// capitalized, unless the status is an error, in which case `Failed`.
///
/// Returns the colored and right-aligned action, and the error description if the
/// status indicates an error.
fn action_with_status_code<'a>(
    on_success_action: &'a str,
    status: Option<&'a SpanStatus>,
    colorize: bool,
) -> (
    MaybeColored<'static, RightAligned<CapitalizedFirst<'a>>>,
    Option<&'a str>,
) {
    let (color, action_text, error_desc): (&'static Style, _, _) = match status {
        Some(SpanStatus {
            message: _,
            code: StatusCode::Ok | StatusCode::Unset,
        })
        | None => (&GREEN, on_success_action, None),
        Some(SpanStatus {
            message,
            code: StatusCode::Error,
        }) => (&RED, "Failed", message.as_deref()),
    };

    (
        MaybeColored::new(color, RightAligned(CapitalizedFirst(action_text)), colorize),
        error_desc,
    )
}
