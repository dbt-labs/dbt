use std::fmt;

use dbt_telemetry::{ExecutionPhase, PhaseExecuted};

use super::{duration::FixedWidthDuration, layout::RightAligned};
/// Get the phase action text for a given `ExecutionPhase` (lowercase, no padding)
pub fn get_phase_action(phase: ExecutionPhase) -> &'static str {
    match phase {
        ExecutionPhase::Analyze => "analyzing",
        ExecutionPhase::Clean => "cleaning",
        ExecutionPhase::Compare => "comparing",
        ExecutionPhase::Debug => "debugging",
        ExecutionPhase::DeferHydration => "deferring state",
        ExecutionPhase::FreshnessAnalysis => "analyzing freshness",
        ExecutionPhase::InitAdapter => "initializing adapter",
        ExecutionPhase::Lineage => "generating lineage",
        ExecutionPhase::LoadProject => "loading project",
        ExecutionPhase::NodeCacheHydration => "hydrating",
        ExecutionPhase::OnRunStart => "on-run-start hooks",
        ExecutionPhase::OnRunEnd => "on-run-end hooks",
        ExecutionPhase::Parse => "parsing",
        ExecutionPhase::Render => "rendering",
        ExecutionPhase::Run => "running",
        ExecutionPhase::Schedule => "scheduling",
        ExecutionPhase::SchemaHydration => "downloading schemas",
        ExecutionPhase::TaskGraphBuild => "building task graph",
        ExecutionPhase::Unspecified => "unknown phase",
    }
}

/// Get the phase action text for the progress bar or spinner.
/// Returns `None` for some phases as of now.
pub fn get_phase_progress_text(phase: ExecutionPhase) -> Option<RightAligned<&'static str>> {
    let action = match phase {
        ExecutionPhase::Analyze => "Analyzing",
        ExecutionPhase::Clean => "Cleaning",
        ExecutionPhase::Compare => "Comparing",
        ExecutionPhase::Debug => "Debugging",
        ExecutionPhase::DeferHydration => "Deferring",
        // Longer than the action column, so displayed without padding
        ExecutionPhase::FreshnessAnalysis => "Analyzing freshness",
        ExecutionPhase::InitAdapter => return None,
        ExecutionPhase::Lineage => "Generating",
        ExecutionPhase::LoadProject => "Loading",
        ExecutionPhase::NodeCacheHydration => "Hydrating",
        // Longer than the action column, so displayed without padding
        ExecutionPhase::OnRunStart => "Running on-run-start",
        ExecutionPhase::OnRunEnd => "Running on-run-end",
        ExecutionPhase::Parse => "Parsing",
        ExecutionPhase::Render => "Rendering",
        ExecutionPhase::Run => "Running",
        ExecutionPhase::Schedule => "Scheduling",
        // Longer than the action column, so displayed without padding
        ExecutionPhase::SchemaHydration => "Downloading Schemas",
        ExecutionPhase::TaskGraphBuild => "Building",
        ExecutionPhase::Unspecified => return None,
    };

    Some(RightAligned(action))
}

/// Displays a `PhaseExecuted` event for the start of a phase
///
/// Writes the pattern:
/// `Started {phase_action}` or `Started {phase_action} ({total} nodes)` if total > 0
pub struct PhaseExecutedStart<'a>(pub &'a PhaseExecuted);

impl fmt::Display for PhaseExecutedStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phase = self.0;
        let phase_action = get_phase_action(phase.phase());
        let total = phase.node_count_total.unwrap_or_default();

        if total > 0 {
            write!(f, "Started {} ({} nodes)", phase_action, total)
        } else {
            write!(f, "Started {}", phase_action)
        }
    }
}

/// Displays a PhaseExecuted event for the end of a phase
///
/// Writes the pattern:
/// `Finished {phase_action} [duration]` or
/// `Finished {phase_action} [duration] ({total} nodes, {skipped} skipped, {errors} errors)` if total > 0
pub struct PhaseExecutedEnd<'a> {
    pub phase: &'a PhaseExecuted,
    pub duration: std::time::Duration,
}

impl fmt::Display for PhaseExecutedEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phase = self.phase;
        let phase_action = get_phase_action(phase.phase());
        let duration_formatted = FixedWidthDuration(self.duration);
        let total = phase.node_count_total.unwrap_or_default();

        if total > 0 {
            let skipped = phase.node_count_skipped.unwrap_or_default();
            let errors = phase.node_count_error.unwrap_or_default();
            write!(
                f,
                "Finished {} [{}] ({} nodes, {} skipped, {} errors)",
                phase_action, duration_formatted, total, skipped, errors
            )
        } else {
            write!(f, "Finished {} [{}]", phase_action, duration_formatted)
        }
    }
}
