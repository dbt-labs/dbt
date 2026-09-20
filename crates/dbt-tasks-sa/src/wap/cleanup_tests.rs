use dbt_common::stats::NodeStatus;
use dbt_common::{ErrorCode, fs_err};

use super::{WapCandidateState, run_failed_candidate_cleanup};

#[test]
fn failed_candidate_cleanup_defaults_to_removal_unless_retention_is_requested() {
    let statuses = [
        None,
        Some(NodeStatus::Errored),
        Some(NodeStatus::SkippedUpstreamFailed),
        Some(NodeStatus::Succeeded),
        Some(NodeStatus::SucceededWithWarning),
        Some(NodeStatus::TestPassed),
        Some(NodeStatus::TestWarned),
        Some(NodeStatus::StaticallyCheckedDataTest),
        Some(NodeStatus::NoOp),
        Some(NodeStatus::ReusedNoChanges(String::new())),
        Some(NodeStatus::ReusedStillFresh(String::new(), 0, 0)),
        Some(NodeStatus::ReusedStillFreshNoChanges(String::new())),
        Some(NodeStatus::ReusedCloned(None)),
    ];
    for retain_failed in [None, Some(false), Some(true)] {
        for state in [
            None,
            Some(WapCandidateState::Created),
            Some(WapCandidateState::PublicationSubmitted),
            Some(WapCandidateState::Published),
        ] {
            for status in &statuses {
                let expected = retain_failed != Some(true)
                    && matches!(
                        state,
                        Some(WapCandidateState::Created | WapCandidateState::Published)
                    )
                    && matches!(
                        status,
                        Some(NodeStatus::Errored | NodeStatus::SkippedUpstreamFailed)
                    );
                let mut calls = 0;
                let dropped =
                    run_failed_candidate_cleanup(retain_failed, state, status.clone(), || {
                        calls += 1;
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(
                    dropped, expected,
                    "retain_failed={retain_failed:?}, state={state:?}, status={status:?}"
                );
                assert_eq!(calls, usize::from(expected));
            }
        }
    }
}

#[test]
fn failed_claim_does_not_authorize_dropping_an_existing_candidate() {
    // A collision or a lost CREATE response does not establish ownership,
    // even when a candidate with the expected name exists in Snowflake.
    let mut candidate_exists = true;
    let dropped = run_failed_candidate_cleanup(None, None, Some(NodeStatus::Errored), || {
        candidate_exists = false;
        Ok(())
    })
    .unwrap();
    assert!(!dropped);
    assert!(candidate_exists);
}

#[test]
fn uncertain_publication_retains_the_candidate_even_after_the_graph_drains() {
    let dropped = run_failed_candidate_cleanup(
        None,
        Some(WapCandidateState::PublicationSubmitted),
        Some(NodeStatus::Errored),
        || panic!("a lost publication response must not authorize cleanup"),
    )
    .unwrap();
    assert!(!dropped);
}

#[test]
fn cleanup_error_is_reported_without_claiming_the_candidate_was_removed() {
    for state in [WapCandidateState::Created, WapCandidateState::Published] {
        let mut candidate_exists = true;
        let mut attempts = 0;
        let error =
            run_failed_candidate_cleanup(None, Some(state), Some(NodeStatus::Errored), || {
                attempts += 1;
                Err(fs_err!(
                    ErrorCode::ExecutionError,
                    "injected candidate drop failure"
                ))
            })
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("injected candidate drop failure")
        );
        assert_eq!(attempts, 1);
        assert!(candidate_exists);

        // The caller keeps ownership on failure, so a later confirmed cleanup
        // can still remove this exact candidate.
        let dropped =
            run_failed_candidate_cleanup(None, Some(state), Some(NodeStatus::Errored), || {
                attempts += 1;
                candidate_exists = false;
                Ok(())
            })
            .unwrap();
        assert!(dropped);
        assert_eq!(attempts, 2);
        assert!(!candidate_exists);
    }
}
