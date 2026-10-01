use dbt_telemetry::{GenericOpExecuted, GenericOpItemProcessed};
use dbt_tracing::{SpanStatus, StatusCode};

use crate::tracing::formatters::layout::{right_align_action, right_align_static_action};

use super::{
    color::{GREEN, RED, maybe_apply_color},
    duration::format_duration_fixed_width,
};

pub fn capitalize_first_letter(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

/// Format a `GenericOpExecuted` event for the start of an operation
///
/// Returns formatted string in the pattern:
/// `Started {display_action}` or `Started {display_action} ({total} items)` if total > 0
pub fn format_generic_op_start(op: &GenericOpExecuted, colorize: bool) -> String {
    let total = op.item_count_total.unwrap_or_default();
    let action = maybe_apply_color(
        &GREEN,
        right_align_static_action("Started").as_str(),
        colorize,
    );

    if total > 0 {
        format!("{} {} ({} items)", action, op.display_action, total)
    } else {
        format!("{} {}", action, op.display_action)
    }
}

/// Format a `GenericOpExecuted` event for the end of an operation
///
/// Returns formatted string in the pattern:
/// `Finished {display_action} [duration]`
pub fn format_generic_op_end(
    op: &GenericOpExecuted,
    duration: std::time::Duration,
    status: Option<&SpanStatus>,
    colorize: bool,
) -> String {
    let duration_formatted = format_duration_fixed_width(duration);

    let total = op.item_count_total.unwrap_or_default();

    let (action, error_desc) = format_action_with_status_code("Finished", status, colorize);

    if let Some(error) = error_desc {
        format!(
            "{} [{}] {} (error: {})",
            action, duration_formatted, op.display_action, error
        )
    } else if total > 0 {
        format!(
            "{} [{}] {} ({} items)",
            action, duration_formatted, op.display_action, total
        )
    } else {
        format!("{} [{}] {}", action, duration_formatted, op.display_action)
    }
}

/// Format a `GenericOpItemProcessed` event for the start of an item
///
/// Returns formatted string in the pattern:
/// `Started {display_action} {target}`
pub fn format_generic_op_item_start(item: &GenericOpItemProcessed) -> String {
    format!(
        "{} {} {}",
        right_align_static_action("Started"),
        item.display_in_progress_action,
        item.target
    )
}

/// Format a `GenericOpItemProcessed` event for the end of an item
///
/// Returns formatted string in the pattern:
/// `{target} [duration]`
pub fn format_generic_op_item_end(
    item: &GenericOpItemProcessed,
    duration: std::time::Duration,
    status: Option<&SpanStatus>,
    colorize: bool,
) -> String {
    let duration_formatted = format_duration_fixed_width(duration);
    let (action, error_desc) =
        format_action_with_status_code(item.display_on_success_action.as_str(), status, colorize);

    if let Some(error) = error_desc {
        format!(
            "{} [{}] {} (error: {})",
            action, duration_formatted, item.target, error
        )
    } else {
        format!("{} [{}] {}", action, duration_formatted, item.target)
    }
}

/// Formats an action from the status code for coloring and text
/// Returns a colored and right-aligned action string based on the provided status
///
/// # Arguments
/// * `on_success_action` - The action text to display on success
/// * `status` - The optional SpanStatus indicating success or failure
/// * `colorize` - Whether to apply color to the action text
///
/// # Returns
/// A tuple containing:
/// - The formatted action string with appropriate color and alignment
/// - An optional error description if the status indicates an error
fn format_action_with_status_code<'a>(
    on_success_action: &str,
    status: Option<&'a SpanStatus>,
    colorize: bool,
) -> (String, Option<&'a str>) {
    let (color, action_text, error_desc) = match status {
        Some(SpanStatus {
            message: _,
            code: StatusCode::Ok | StatusCode::Unset,
        })
        | None => (
            &GREEN,
            right_align_action(capitalize_first_letter(on_success_action).into()).to_string(),
            None,
        ),
        Some(SpanStatus {
            message,
            code: StatusCode::Error,
        }) => (
            &RED,
            right_align_static_action("Failed"),
            message.as_deref(),
        ),
    };

    (
        maybe_apply_color(color, action_text.as_str(), colorize),
        error_desc,
    )
}
