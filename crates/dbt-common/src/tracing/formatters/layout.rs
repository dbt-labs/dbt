use std::borrow::Cow;

use super::{
    color::{DIM, maybe_apply_color},
    constants::{ACTION_WIDTH, DEFAULT_TERMINAL_WIDTH},
};

/// Formats a centered delimiter line with '=' padding characters and optional DIM color.
///
/// This is used to create section headers in terminal output, such as
/// "Execution Summary", "Test Failures", and "Errors and Warnings".
///
/// # Arguments
/// * `text` - The text to center within the delimiter
/// * `width` - Optional terminal width. Uses DEFAULT_TERMINAL_WIDTH if None.
/// * `colorize` - Whether to apply DIM color styling to the delimiter
///
/// # Example
/// ```
/// let header = format_delimiter(" Execution Summary ", Some(80), true);
/// // Returns: DIM-colored "========================= Execution Summary ========================="
/// ```
pub fn format_delimiter(text: &str, width: Option<usize>, colorize: bool) -> String {
    let width = width.unwrap_or(DEFAULT_TERMINAL_WIDTH);
    let raw = format!("{:=^width$}", text, width = width);
    maybe_apply_color(&DIM, &raw, colorize)
}

/// Right aligns the given const (i.e. known at compile time) action text
/// to a fixed width defined by ACTION_WIDTH.
///
/// Use it for the first column of progress messages.
///
/// # Arguments
/// * `action` - The action text to right align
///
/// # Returns
/// A right-aligned string padded to ACTION_WIDTH characters
///
/// # Panics
/// Panics in debug builds if the action text exceeds ACTION_WIDTH characters.
pub fn right_align_static_action(action: &'static str) -> String {
    // Right align and pad to ACTION_WIDTH characters
    debug_assert!(
        action.len() <= ACTION_WIDTH,
        "Action text too long for padding"
    );

    format!("{:>width$}", action, width = ACTION_WIDTH)
}

/// Right aligns the given action text to a fixed width defined by ACTION_WIDTH.
///
/// If the action text is known at compile time, prefer using `right_align_static_action`.
///
/// # Arguments
/// * `action` - The action text to right align (as a Cow)
///
/// # Returns
/// A right-aligned string padded to ACTION_WIDTH characters, or the original
/// string if it exceeds ACTION_WIDTH.
pub fn right_align_action(action: Cow<'_, str>) -> Cow<'_, str> {
    if action.len() >= ACTION_WIDTH {
        action
    } else {
        Cow::Owned(format!("{:>width$}", action, width = ACTION_WIDTH))
    }
}
