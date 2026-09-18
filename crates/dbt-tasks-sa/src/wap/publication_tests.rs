use super::{PublicationStep, run_publication_steps};
use dbt_common::stats::NodeStatus;
use dbt_common::{ErrorCode, FsResult, fs_err};

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
        assert!(message.contains("Working table retained: DB.SCHEMA.__DBT_WAP_TEST"));
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
