use dbt_telemetry::{HookOutcome, HookProcessed};
use std::{borrow::Cow, time::Duration};

use super::{
    color::{BLUE, GREEN, PLAIN, RED, YELLOW, maybe_apply_color},
    duration::format_duration_fixed_width,
    layout::right_align_static_action,
};

/// Format hook name with optional coloring
fn format_hook_name(hook_name: &str, colorize: bool) -> Cow<'_, str> {
    if colorize {
        Cow::Owned(BLUE.apply_to(hook_name).to_string())
    } else {
        Cow::Borrowed(hook_name)
    }
}

pub fn format_hook_processed_start(hook: &HookProcessed, colorize: bool) -> String {
    let hook_type = hook.hook_type().as_static_str();

    if let Some(hook_name) = hook.name.as_deref() {
        let hook_name = format_hook_name(hook_name, colorize);
        format!("Started {hook_type} {hook_name}")
    } else {
        format!("Started {hook_type}")
    }
}

pub fn format_hook_outcome_as_status(hook_outcome: HookOutcome, colorize: bool) -> String {
    let (status, color) = match hook_outcome {
        HookOutcome::Success => ("success", &GREEN),
        HookOutcome::Error => ("error", &RED),
        HookOutcome::Canceled => ("cancelled", &YELLOW),
        HookOutcome::Unspecified => ("unknown", &YELLOW),
    };

    if colorize {
        color.apply_to(status).to_string()
    } else {
        status.to_string()
    }
}

/// Format a HookProcessed event into a single output line.
///
/// Returns formatted string in the pattern:
/// `{action} [{duration}] hook {qualifier}.{name}`
pub fn format_hook_processed_end(
    hook: &HookProcessed,
    duration: Duration,
    colorize: bool,
) -> String {
    // Determine status/action based on outcome
    let (action, color) = match hook.hook_outcome() {
        HookOutcome::Success => ("Finished", &GREEN),
        HookOutcome::Error => ("Failed", &RED),
        HookOutcome::Canceled => ("Cancelled", &YELLOW),
        HookOutcome::Unspecified => ("Finished", &PLAIN),
    };

    // Format action with color and right alignment
    let action_formatted = right_align_static_action(action);
    let action_formatted = maybe_apply_color(color, &action_formatted, colorize);

    // Format duration
    let duration_formatted = format_duration_fixed_width(duration);

    // Build the target name with colors (similar to how nodes format qualifier.alias)
    let hook_name = format_hook_name(hook.name.as_deref().unwrap_or(""), colorize);

    // Format: "hook" as the type (like "seed", "model", "test")
    let hook_type = maybe_apply_color(&PLAIN, "hook ", colorize);

    format!("{action_formatted} [{duration_formatted}] {hook_type} {hook_name}",)
}
