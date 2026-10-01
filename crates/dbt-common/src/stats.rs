use chrono::{DateTime, Local};
use dbt_telemetry::NodeOutcome;
use std::time::{Duration, SystemTime};
use strum_macros::EnumString;

// ------------------------------------------------------------------------------------------------
// Trivial Stats, foundation for run-results

#[derive(EnumString, PartialEq, Debug, Clone)]
pub enum NodeStatus {
    // the following states can be reported on the makefile
    Succeeded,
    /// Node built successfully in the warehouse but produced warnings (e.g.
    /// duplicate columns). `node_warning_outcome` on the span signals their presence.
    SucceededWithWarning,
    Errored,
    TestWarned,
    TestPassed,
    SkippedUpstreamFailed,
    ReusedNoChanges(String),
    ReusedStillFresh(String, u64, u64),
    ReusedStillFreshNoChanges(String),
    ReusedCloned(Option<u64>),
    StaticallyCheckedDataTest,
    NoOp,
}

impl NodeStatus {
    pub fn get_message(&self) -> Option<String> {
        match self {
            NodeStatus::ReusedNoChanges(message) => Some(message.clone()),
            NodeStatus::ReusedStillFresh(message, _, _) => Some(message.clone()),
            NodeStatus::ReusedStillFreshNoChanges(message) => Some(message.clone()),
            NodeStatus::ReusedCloned(_) => Some(self.default_message()),
            NodeStatus::StaticallyCheckedDataTest => Some(self.default_message()),
            _ => None,
        }
    }

    /// Returns a default message for the node status, used in run_results.json
    /// when no explicit message is provided.
    pub fn default_message(&self) -> String {
        match self {
            NodeStatus::Succeeded => "Succeeded".to_string(),
            NodeStatus::Errored => "Error".to_string(),
            NodeStatus::TestWarned => "Warn".to_string(),
            NodeStatus::TestPassed => "Pass".to_string(),
            NodeStatus::SkippedUpstreamFailed => "Skipped".to_string(),
            NodeStatus::ReusedNoChanges(msg) => msg.clone(),
            NodeStatus::ReusedStillFresh(msg, _, _) => msg.clone(),
            NodeStatus::ReusedStillFreshNoChanges(msg) => msg.clone(),
            NodeStatus::ReusedCloned(None) => "Cloned from other environment".to_string(),
            NodeStatus::ReusedCloned(Some(_)) => {
                "Cloned from other environment within tolerance".to_string()
            }
            NodeStatus::StaticallyCheckedDataTest => "Statically checked".to_string(),
            NodeStatus::SucceededWithWarning => "Warn".to_string(),
            NodeStatus::NoOp => "Skipped".to_string(),
        }
    }
}
impl From<NodeStatus> for NodeOutcome {
    fn from(status: NodeStatus) -> Self {
        match status {
            NodeStatus::Succeeded
            | NodeStatus::SucceededWithWarning
            | NodeStatus::TestWarned
            | NodeStatus::TestPassed => NodeOutcome::Success,
            NodeStatus::Errored => NodeOutcome::Error,
            NodeStatus::SkippedUpstreamFailed => NodeOutcome::Skipped,
            NodeStatus::ReusedNoChanges(_) => NodeOutcome::Skipped,
            NodeStatus::ReusedStillFresh(_, _, _) => NodeOutcome::Skipped,
            NodeStatus::ReusedStillFreshNoChanges(_) => NodeOutcome::Skipped,
            NodeStatus::ReusedCloned(_) => NodeOutcome::Skipped,
            NodeStatus::StaticallyCheckedDataTest => NodeOutcome::Success,
            NodeStatus::NoOp => NodeOutcome::Skipped,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Stat {
    pub unique_id: String,
    pub num_rows: Option<usize>,
    /// Rows affected by the warehouse DML (e.g. `CREATE TABLE AS SELECT`).
    /// Set from the NodeEvaluated OTel span after execution; `None` for views.
    pub rows_affected: Option<i64>,
    pub start_time: SystemTime,
    pub end_time: SystemTime,
    pub status: NodeStatus,
    pub thread_id: String,
    pub message: Option<String>,
}

impl Stat {
    pub fn new(
        unique_id: String,
        start_time: SystemTime,
        num_rows: Option<usize>,
        status: NodeStatus,
        message: Option<String>,
        thread_id: i32,
    ) -> Self {
        let end_time = SystemTime::now();
        let message = message.or_else(|| Some(status.default_message()));

        Stat {
            unique_id,
            num_rows,
            rows_affected: None,
            start_time,
            end_time,
            status,
            thread_id: format!("Thread-{} (worker)", thread_id),
            message,
        }
    }

    pub fn get_duration(&self) -> Duration {
        self.end_time
            .duration_since(self.start_time)
            .unwrap_or_default()
    }

    pub fn format_time(system_time: SystemTime) -> String {
        let datetime: DateTime<Local> = DateTime::from(system_time);
        datetime.format("%H:%M:%S").to_string()
    }
    pub fn status_string(&self) -> String {
        if self.status == NodeStatus::Succeeded
            && (self.unique_id.starts_with("test.") || self.unique_id.starts_with("unit_test."))
        {
            match self.num_rows {
                Some(0) => "Passed".to_string(),
                Some(_) => "Failed".to_string(),
                None => "Succeeded".to_string(),
            }
        } else if self.status == NodeStatus::SucceededWithWarning {
            "Warn".to_string()
        } else if self.status == NodeStatus::StaticallyCheckedDataTest {
            "Passed".to_string()
        } else {
            format!("{:?}", self.status)
        }
    }
    /// Nodes whose result is an *assertion* rather than a build: they report `pass`/`fail` instead
    /// of `success`/`error`, and the row count decides which. Checks belong here for the same reason
    /// tests do — "3 rows came back" is a failed assertion, not a failed build.
    fn is_assertion_node(&self) -> bool {
        self.unique_id.starts_with("test.")
            || self.unique_id.starts_with("unit_test.")
            || self.unique_id.starts_with("check.")
    }

    pub fn result_status_string(&self) -> String {
        match self.status {
            NodeStatus::Succeeded if self.is_assertion_node() => {
                match self.num_rows {
                    Some(0) => "pass".to_string(),
                    Some(_) => "fail".to_string(),
                    // Using "pass" as fallback, though tests should have pass/fail
                    None => "pass".to_string(),
                }
            }
            NodeStatus::Errored if self.is_assertion_node() => match self.num_rows {
                Some(0) => "error".to_string(),
                Some(_) => "fail".to_string(),
                None => "error".to_string(),
            },
            NodeStatus::Succeeded => "success".to_string(),
            NodeStatus::SucceededWithWarning => "warn".to_string(),
            NodeStatus::TestWarned => "warn".to_string(),
            NodeStatus::TestPassed => "pass".to_string(),
            NodeStatus::Errored => "error".to_string(),
            NodeStatus::SkippedUpstreamFailed => "skipped".to_string(),
            NodeStatus::ReusedNoChanges(_) => "reused".to_string(),
            NodeStatus::ReusedStillFresh(_, _, _) => "reused".to_string(),
            NodeStatus::ReusedStillFreshNoChanges(_) => "reused".to_string(),
            NodeStatus::ReusedCloned(_) => "reused".to_string(),
            NodeStatus::StaticallyCheckedDataTest => "pass".to_string(),
            NodeStatus::NoOp => "skipped".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thread_id_format() {
        let stat = Stat::new(
            "test.model".to_string(),
            SystemTime::now(),
            None,
            NodeStatus::Succeeded,
            None,
            1,
        );

        // Thread ID should be in format "Thread-<number> (worker)"
        assert!(
            stat.thread_id.starts_with("Thread-"),
            "thread_id should start with 'Thread-', got: {}",
            stat.thread_id
        );
        assert!(
            stat.thread_id.ends_with(" (worker)"),
            "thread_id should end with ' (worker)', got: {}",
            stat.thread_id
        );

        // Extract the number part and verify it's a valid number
        let number_part = stat
            .thread_id
            .trim_start_matches("Thread-")
            .trim_end_matches(" (worker)");
        assert!(
            number_part.parse::<u64>().is_ok(),
            "thread_id should have a number between 'Thread-' and ' (worker)', got: {}",
            stat.thread_id
        );
    }

    #[test]
    fn test_default_message_when_none_provided() {
        let stat = Stat::new(
            "model.my_project.my_model".to_string(),
            SystemTime::now(),
            None,
            NodeStatus::Succeeded,
            None,
            1,
        );

        assert_eq!(
            stat.message,
            Some("Succeeded".to_string()),
            "message should default to 'Succeeded' for successful nodes"
        );
    }

    #[test]
    fn test_explicit_message_preserved() {
        let stat = Stat::new(
            "model.my_project.my_model".to_string(),
            SystemTime::now(),
            None,
            NodeStatus::Errored,
            Some("Custom error message".to_string()),
            1,
        );

        assert_eq!(
            stat.message,
            Some("Custom error message".to_string()),
            "explicit message should be preserved"
        );
    }

    #[test]
    fn test_default_messages_for_all_statuses() {
        assert_eq!(NodeStatus::Succeeded.default_message(), "Succeeded");
        assert_eq!(NodeStatus::SucceededWithWarning.default_message(), "Warn");
        assert_eq!(NodeStatus::Errored.default_message(), "Error");
        assert_eq!(NodeStatus::TestWarned.default_message(), "Warn");
        assert_eq!(NodeStatus::TestPassed.default_message(), "Pass");
        assert_eq!(
            NodeStatus::SkippedUpstreamFailed.default_message(),
            "Skipped"
        );
        assert_eq!(
            NodeStatus::StaticallyCheckedDataTest.default_message(),
            "Statically checked"
        );
        assert_eq!(NodeStatus::NoOp.default_message(), "Skipped");
        assert_eq!(
            NodeStatus::ReusedNoChanges("Model reused".to_string()).default_message(),
            "Model reused"
        );
        assert_eq!(
            NodeStatus::ReusedCloned(None).default_message(),
            "Cloned from other environment"
        );
        assert_eq!(
            NodeStatus::ReusedCloned(Some(3600)).default_message(),
            "Cloned from other environment within tolerance"
        );
    }

    #[test]
    fn test_succeeded_with_warning_stat_strings() {
        let stat = Stat::new(
            "model.my_project.dup_col".to_string(),
            SystemTime::now(),
            None,
            NodeStatus::SucceededWithWarning,
            None,
            1,
        );
        assert_eq!(stat.status_string(), "Warn");
        assert_eq!(stat.result_status_string(), "warn");
    }
}
