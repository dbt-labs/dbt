use super::{PublicationStep, audit_publication_status, run_publication_steps};
use crate::runnable::test::{TestExecutionStatus, status_with_warn_error_overrides};
use dbt_common::stats::NodeStatus;
use dbt_common::warn_error_options::{
    SupportedLegacyWarnError, WarnErrorOptionValue, WarnErrorOptions,
};
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::DbtTest;
use dbt_schemas::schemas::common::Severity;
use dbt_tasks_core::run_cache::run_cache_service::CachedTestExecutionResult;

use PublicationStep::*;

/// Simulate warehouse effects at the production publication boundary. A failed
/// statement leaves its previous state intact; tests below check what can run next.
struct Warehouse {
    steps: Vec<PublicationStep>,
    failures: Vec<PublicationStep>,
    public_revision: u8,
    candidate_exists: bool,
    query_tag_set: bool,
    overrides_set: bool,
}

impl Warehouse {
    fn new(failures: Vec<PublicationStep>) -> Self {
        Self {
            steps: Vec::new(),
            failures,
            public_revision: 1,
            candidate_exists: true,
            query_tag_set: false,
            overrides_set: false,
        }
    }

    fn execute(&mut self, step: PublicationStep) -> FsResult<()> {
        self.steps.push(step);
        if self.failures.contains(&step) {
            return Err(fs_err!(
                ErrorCode::ExecutionError,
                "injected {step:?} failure"
            ));
        }
        match step {
            Prepare => self.overrides_set = true,
            SetQueryTag => self.query_tag_set = true,
            Clone => self.public_revision = 2,
            Finalize => {}
            ResetQueryTag => self.query_tag_set = false,
            ResetOverrides => self.overrides_set = false,
            DropCandidate => self.candidate_exists = false,
        }
        Ok(())
    }

    fn publish(&mut self) -> FsResult<()> {
        run_publication_steps(
            [("audit", Some(NodeStatus::TestPassed))],
            "DB.SCHEMA.ORDERS",
            "DB.SCHEMA.__DBT_WAP_TEST",
            |step| self.execute(step),
        )
    }
}

#[test]
fn publication_does_not_touch_warehouse_without_every_audit_passing() {
    for status in [
        None,
        Some(NodeStatus::Errored),
        Some(NodeStatus::TestWarned),
        Some(NodeStatus::SkippedUpstreamFailed),
        Some(NodeStatus::StaticallyCheckedDataTest),
        Some(NodeStatus::ReusedNoChanges("cached".to_owned())),
    ] {
        let mut warehouse = Warehouse::new(vec![]);
        let result = run_publication_steps(
            [
                ("passed", Some(NodeStatus::TestPassed)),
                ("blocking", status),
            ],
            "ORDERS",
            "WORK",
            |step| warehouse.execute(step),
        );
        assert!(result.is_err());
        assert!(warehouse.steps.is_empty());
        assert_eq!(warehouse.public_revision, 1);
        assert!(warehouse.candidate_exists);
    }
    assert!(
        run_publication_steps([], "ORDERS", "WORK", |_| {
            panic!("no publication step may run without an audit")
        })
        .is_err()
    );
}

#[test]
fn publication_rejects_silenced_audit_warnings() {
    let mut test = DbtTest::default();
    test.deprecated_config.severity = Some(Severity::Warn);
    let raw_result = CachedTestExecutionResult {
        failures: 1,
        should_warn: true,
        should_error: true,
    };
    let options = WarnErrorOptions {
        silence: vec![WarnErrorOptionValue::SupportedLegacy(
            SupportedLegacyWarnError::LogTestResult,
        )],
        ..Default::default()
    };
    let reported_status =
        status_with_warn_error_overrides(TestExecutionStatus::Warned, &options).node_status();
    assert_eq!(reported_status, NodeStatus::TestPassed);

    let status = audit_publication_status(Some(reported_status), Some(&test), Some(raw_result));
    let mut warehouse = Warehouse::new(vec![]);
    let error = run_publication_steps([("audit", status)], "ORDERS", "WORK", |step| {
        warehouse.execute(step)
    })
    .unwrap_err();
    assert!(error.to_string().contains("did not PASS"));
    assert!(warehouse.steps.is_empty());
    assert_eq!(warehouse.public_revision, 1);
    assert!(warehouse.candidate_exists);
}

#[test]
fn publication_requires_the_audit_node_and_raw_result() {
    let test = DbtTest::default();
    let raw_result = CachedTestExecutionResult {
        failures: 0,
        should_warn: false,
        should_error: false,
    };
    for (test, raw_result) in [(Some(&test), None), (None, Some(raw_result))] {
        let status = audit_publication_status(Some(NodeStatus::TestPassed), test, raw_result);
        let mut warehouse = Warehouse::new(vec![]);
        let result = run_publication_steps([("audit", status)], "ORDERS", "WORK", |step| {
            warehouse.execute(step)
        });
        assert!(result.is_err());
        assert!(warehouse.steps.is_empty());
        assert_eq!(warehouse.public_revision, 1);
        assert!(warehouse.candidate_exists);
    }
}

#[test]
fn publication_rejects_invalid_raw_failure_counts() {
    let test = DbtTest::default();
    let status = audit_publication_status(
        Some(NodeStatus::TestPassed),
        Some(&test),
        Some(CachedTestExecutionResult {
            failures: -1,
            should_warn: false,
            should_error: false,
        }),
    );
    let mut warehouse = Warehouse::new(vec![]);
    assert!(
        run_publication_steps([("audit", status)], "ORDERS", "WORK", |step| {
            warehouse.execute(step)
        })
        .is_err()
    );
    assert!(warehouse.steps.is_empty());
    assert_eq!(warehouse.public_revision, 1);
    assert!(warehouse.candidate_exists);
}

#[test]
fn publication_raw_results_do_not_override_nonpassing_statuses() {
    let test = DbtTest::default();
    let raw_result = CachedTestExecutionResult {
        failures: 0,
        should_warn: false,
        should_error: false,
    };
    for reported_status in [
        None,
        Some(NodeStatus::Errored),
        Some(NodeStatus::TestWarned),
        Some(NodeStatus::SkippedUpstreamFailed),
        Some(NodeStatus::StaticallyCheckedDataTest),
        Some(NodeStatus::ReusedNoChanges("cached".to_owned())),
    ] {
        let status =
            audit_publication_status(reported_status.clone(), Some(&test), Some(raw_result));
        assert_eq!(status, reported_status);
        let mut warehouse = Warehouse::new(vec![]);
        let result = run_publication_steps([("audit", status)], "ORDERS", "WORK", |step| {
            warehouse.execute(step)
        });
        assert!(result.is_err());
        assert!(warehouse.steps.is_empty());
    }
}

#[test]
fn publication_uses_configured_thresholds_and_severity_for_raw_results() {
    for (severity, failures, should_warn, should_error, passes) in [
        (None, 0, false, false, true),
        (Some(Severity::Error), 3, false, false, true),
        (Some(Severity::Warn), 3, false, true, true),
        (Some(Severity::Error), 3, false, true, false),
        (Some(Severity::Warn), 3, true, false, false),
    ] {
        let mut test = DbtTest::default();
        test.deprecated_config.severity = severity;
        let status = audit_publication_status(
            Some(NodeStatus::TestPassed),
            Some(&test),
            Some(CachedTestExecutionResult {
                failures,
                should_warn,
                should_error,
            }),
        );
        let mut warehouse = Warehouse::new(vec![]);
        let result = run_publication_steps([("audit", status)], "ORDERS", "WORK", |step| {
            warehouse.execute(step)
        });
        assert_eq!(result.is_ok(), passes);
        if passes {
            assert_eq!(warehouse.public_revision, 2);
            assert!(!warehouse.candidate_exists);
        } else {
            assert!(warehouse.steps.is_empty());
            assert_eq!(warehouse.public_revision, 1);
            assert!(warehouse.candidate_exists);
        }
    }
}

#[test]
fn publication_cleans_up_only_after_finalization_and_session_restoration() {
    let mut warehouse = Warehouse::new(vec![]);
    warehouse.publish().unwrap();
    assert_eq!(warehouse.public_revision, 2);
    assert!(!warehouse.candidate_exists);
    assert!(!warehouse.query_tag_set);
    assert!(!warehouse.overrides_set);
    assert_eq!(
        warehouse.steps,
        [
            Prepare,
            SetQueryTag,
            Clone,
            Finalize,
            ResetQueryTag,
            ResetOverrides,
            DropCandidate
        ]
    );
}

#[test]
fn publication_failures_before_clone_confirmation_retain_candidate_and_restore_session() {
    for failure in [Prepare, SetQueryTag, Clone] {
        let mut warehouse = Warehouse::new(vec![failure]);
        let error = warehouse.publish().unwrap_err();
        assert!(error.to_string().contains(&format!("injected {failure:?}")));
        assert!(!error.to_string().contains("was published"));
        assert_eq!(warehouse.public_revision, 1);
        assert!(warehouse.candidate_exists);
        assert!(!warehouse.query_tag_set);
        assert!(!warehouse.overrides_set);
        assert!(!warehouse.steps.contains(&Finalize));
        assert!(!warehouse.steps.contains(&DropCandidate));
        assert_eq!(warehouse.steps.last(), Some(&ResetOverrides));
        assert_eq!(warehouse.steps.contains(&ResetQueryTag), failure == Clone);
    }
}

#[test]
fn publication_retains_candidate_when_clone_outcome_is_unknown() {
    let mut warehouse = Warehouse::new(vec![]);
    let error = run_publication_steps(
        [("audit", Some(NodeStatus::TestPassed))],
        "ORDERS",
        "WORK",
        |step| {
            warehouse.execute(step)?;
            if step == Clone {
                // Snowflake committed, but the client lost its response. The
                // orchestration must not claim either confirmed success or rollback.
                return Err(fs_err!(
                    ErrorCode::ExecutionError,
                    "Publication may have committed; inspect Snowflake query history"
                ));
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("may have committed"));
    assert!(!error.to_string().contains("was published"));
    assert_eq!(warehouse.public_revision, 2);
    assert!(warehouse.candidate_exists);
    assert!(!warehouse.steps.contains(&Finalize));
    assert!(!warehouse.steps.contains(&DropCandidate));
    assert!(!warehouse.query_tag_set);
    assert!(!warehouse.overrides_set);
}

#[test]
fn publication_finalization_failures_report_published_and_retain_candidate() {
    for failure in [Finalize, ResetQueryTag, ResetOverrides] {
        let mut warehouse = Warehouse::new(vec![failure]);
        let message = warehouse.publish().unwrap_err().to_string();
        assert!(message.contains("DB.SCHEMA.ORDERS was published"));
        assert!(message.contains(&format!("injected {failure:?}")));
        assert!(message.contains("Working table at failure: DB.SCHEMA.__DBT_WAP_TEST"));
        assert_eq!(warehouse.public_revision, 2);
        assert!(warehouse.candidate_exists);
        assert!(warehouse.steps.contains(&ResetQueryTag));
        assert!(warehouse.steps.contains(&ResetOverrides));
        assert!(!warehouse.steps.contains(&DropCandidate));
    }
}

#[test]
fn publication_preserves_primary_error_when_session_restoration_also_fails() {
    for failure in [Clone, Finalize] {
        let mut warehouse = Warehouse::new(vec![failure, ResetQueryTag, ResetOverrides]);
        let message = warehouse.publish().unwrap_err().to_string();
        assert!(message.contains(&format!("injected {failure:?}")));
        assert!(!message.contains("injected Reset"));
        assert!(warehouse.steps.contains(&ResetQueryTag));
        assert!(warehouse.steps.contains(&ResetOverrides));
        assert!(warehouse.candidate_exists);
    }
}

#[test]
fn publication_remains_successful_when_candidate_cleanup_fails() {
    let mut warehouse = Warehouse::new(vec![DropCandidate]);
    warehouse.publish().unwrap();
    assert_eq!(warehouse.public_revision, 2);
    assert!(warehouse.candidate_exists);
    assert!(!warehouse.query_tag_set);
    assert!(!warehouse.overrides_set);
}
