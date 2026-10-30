use std::fmt;

use dbt_telemetry::{HookOutcome, HookProcessed};

use super::{
    color::{BLUE, GREEN, MaybeColored, PLAIN, RED, YELLOW},
    duration::FixedWidthDuration,
    layout::RightAligned,
};

/// Displays a hook's start.
pub struct HookProcessedStart<'a> {
    pub hook: &'a HookProcessed,
    pub colorize: bool,
}

impl fmt::Display for HookProcessedStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hook_type = self.hook.hook_type().as_static_str();

        if let Some(hook_name) = self.hook.name.as_deref() {
            let hook_name = MaybeColored::new(&BLUE, hook_name, self.colorize);
            write!(f, "Started {hook_type} {hook_name}")
        } else {
            write!(f, "Started {hook_type}")
        }
    }
}

/// Displays a hook outcome as a status word, optionally colorized.
pub struct HookOutcomeStatus {
    pub outcome: HookOutcome,
    pub colorize: bool,
}

impl fmt::Display for HookOutcomeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (status, color) = match self.outcome {
            HookOutcome::Success => ("success", &GREEN),
            HookOutcome::Error => ("error", &RED),
            HookOutcome::Canceled => ("cancelled", &YELLOW),
            HookOutcome::Unspecified => ("unknown", &YELLOW),
        };

        write!(f, "{}", MaybeColored::new(color, status, self.colorize))
    }
}

/// Displays a HookProcessed event as a single output line.
///
/// Writes the pattern:
/// `{action} [{duration}] hook {qualifier}.{name}`
pub struct HookProcessedEnd<'a> {
    pub hook: &'a HookProcessed,
    pub duration: std::time::Duration,
    pub colorize: bool,
}

impl fmt::Display for HookProcessedEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let colorize = self.colorize;
        // Determine status/action based on outcome
        let (action, color) = match self.hook.hook_outcome() {
            HookOutcome::Success => ("Finished", &GREEN),
            HookOutcome::Error => ("Failed", &RED),
            HookOutcome::Canceled => ("Cancelled", &YELLOW),
            HookOutcome::Unspecified => ("Finished", &PLAIN),
        };

        write!(
            f,
            "{} [{}] {} {}",
            MaybeColored::new(color, RightAligned(action), colorize),
            FixedWidthDuration(self.duration),
            // "hook" as the type (like "seed", "model", "test")
            MaybeColored::new(&PLAIN, "hook ", colorize),
            // The target name with colors (similar to how nodes format qualifier.alias)
            MaybeColored::new(&BLUE, self.hook.name.as_deref().unwrap_or(""), colorize),
        )
    }
}
