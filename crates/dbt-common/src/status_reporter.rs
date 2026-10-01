use std::sync::Arc;

use error::CodeLocationWithFile;

use crate::{
    constants::{FAILED, PASSED, REUSED, SKIPPED, SUCCEEDED, WARNED},
    io_utils::StatusReporter,
    stats::NodeStatus,
};

fn status_action(node_status: &NodeStatus) -> &'static str {
    match node_status {
        NodeStatus::Succeeded => SUCCEEDED,
        NodeStatus::SucceededWithWarning => SUCCEEDED,
        NodeStatus::TestPassed => PASSED,
        NodeStatus::StaticallyCheckedDataTest => PASSED,
        NodeStatus::TestWarned => WARNED,
        NodeStatus::Errored => FAILED,
        NodeStatus::SkippedUpstreamFailed => SKIPPED,
        NodeStatus::ReusedNoChanges(_)
        | NodeStatus::ReusedStillFresh(_, _, _)
        | NodeStatus::ReusedStillFreshNoChanges(_)
        | NodeStatus::ReusedCloned(_) => REUSED,
        NodeStatus::NoOp => "",
    }
}

pub fn report_completed(
    node_status: &NodeStatus,
    defined_at: Option<CodeLocationWithFile>,
    display_path: &str,
    with_cache: bool,
    status_reporter: Option<&Arc<dyn StatusReporter + 'static>>,
) {
    let Some(status_reporter) = status_reporter else {
        return;
    };

    if matches!(node_status, &NodeStatus::NoOp) {
        return;
    }

    let desc = if matches!(
        node_status,
        NodeStatus::Succeeded | NodeStatus::SucceededWithWarning
    ) {
        with_cache.then_some("New changes detected".to_string())
    } else if matches!(node_status, NodeStatus::TestWarned | NodeStatus::Errored)
        && let Some(location) = defined_at
    {
        Some(location.to_string())
    } else {
        node_status.get_message()
    };

    status_reporter.show_progress(status_action(node_status), display_path, desc.as_deref());
}
