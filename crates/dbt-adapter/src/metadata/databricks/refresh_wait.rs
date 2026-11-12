//! Wait for a running materialized view / streaming table refresh before reading its config.
//!
//! dbt-databricks polls the DLT pipeline until it is IDLE
//! (https://github.com/databricks/dbt-databricks/blob/13686739eb59566c7a90ee3c357d12fe52ec02ea/dbt/adapters/databricks/impl.py#L908).
//! Instead of the pipelines REST API we poll the `Latest Refresh Status` row that
//! `DESCRIBE EXTENDED` already returns. Observed once (2026-10-29): `Succeeded` in the describe
//! while the pipelines REST API still reported RUNNING for a few seconds.
//!
//! For some relations (seen on ones owned by other principals) Databricks puts
//! [`UNAVAILABLE`] in both refresh rows. There is nothing to wait for then, so the wait ends at
//! once, as it does when the rows are absent. A wait that reaches its cap warns and reads the
//! config anyway; it never fails the command.

use std::time::{Duration, Instant};

use arrow_array::StringArray;
use dbt_agate::AgateTable;
use dbt_common::ErrorCode;
use dbt_common::cancellation::CancellationToken;
use dbt_common::tracing::dbt_emit::{emit_debug_log_message, emit_warn_log_message};

use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use dbt_adapter_engine::record_batch::RecordBatchExt;

const STATUS_ROW: &str = "Latest Refresh Status";
const LINK_ROW: &str = "Latest Refresh";
/// What Databricks puts in the refresh rows when it has no refresh information to show.
const UNAVAILABLE: &str = "WARNING: Refresh information is not available";

const RUNNING_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const UNKNOWN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Upper bound on replayed polls, for a recording whose status never leaves `Running`.
const REPLAY_MAX_POLLS: usize = 100;

/// Caps of the wait.
pub(super) struct RefreshWait {
    /// Cap for a refresh that stays `Running`.
    pub timeout: Duration,
    /// Cap for a status this code does not recognise.
    pub unknown_timeout: Duration,
}

impl RefreshWait {
    pub(super) const DEFAULT: Self = Self {
        timeout: RUNNING_TIMEOUT,
        unknown_timeout: UNKNOWN_TIMEOUT,
    };
}

/// What differs between a live run and a replay of its recording.
pub(super) trait WaitStrategy {
    /// Whether the wait must end now, given the polls so far and the time since it started.
    fn limit_reached(&self, polls: usize, elapsed: Duration, cap: Duration) -> bool;
    /// Reports that the wait ends at its limit; the last describe is returned either way.
    fn report_limit(&self, relation: &str, status: &str, link: &str, cap: Duration, polls: usize);
    /// Called between two polls.
    fn pause(&self, token: &CancellationToken, relation: &str) -> AdapterResult<()>;
    /// A describe made while polling failed.
    fn describe_failed(
        &self,
        relation: &str,
        err: AdapterError,
        describe_result: AgateTable,
    ) -> AdapterResult<AgateTable>;
    /// Fails with `Cancelled` once the command was cancelled; called wherever the wait can stop.
    fn check_cancelled(&self, token: &CancellationToken, relation: &str) -> AdapterResult<()> {
        if token.is_cancelled() {
            return Err(AdapterError::new(
                AdapterErrorKind::Cancelled,
                format!("Cancelled while waiting for the refresh of {relation} to finish"),
            ));
        }
        Ok(())
    }
}

/// The refresh runs for real: the elapsed time caps the wait, with a sleep between polls.
pub(super) struct Live {
    pub interval: Duration,
}

impl Live {
    pub(super) const DEFAULT: Self = Self {
        interval: POLL_INTERVAL,
    };
}

impl WaitStrategy for Live {
    fn limit_reached(&self, _polls: usize, elapsed: Duration, cap: Duration) -> bool {
        elapsed >= cap
    }

    fn report_limit(&self, relation: &str, status: &str, link: &str, cap: Duration, _polls: usize) {
        emit_warn_log_message(
            ErrorCode::DbRefreshWaitTimeoutWarning,
            format!(
                "Refresh of {relation} still has status `{status}` after {}s (pipeline refresh: {link}); reading its config anyway",
                cap.as_secs()
            ),
        );
    }

    fn pause(&self, token: &CancellationToken, relation: &str) -> AdapterResult<()> {
        let deadline = Instant::now() + self.interval;
        while Instant::now() < deadline {
            self.check_cancelled(token, relation)?;
            std::thread::sleep(Duration::from_millis(100).min(self.interval));
        }
        self.check_cancelled(token, relation)
    }

    fn describe_failed(
        &self,
        _relation: &str,
        err: AdapterError,
        _describe_result: AgateTable,
    ) -> AdapterResult<AgateTable> {
        Err(err)
    }
}

/// The same loop runs without sleeping, so it consumes the describes the live run recorded; it
/// stops when the recording runs out or after `REPLAY_MAX_POLLS`.
pub(super) struct Replay;

impl WaitStrategy for Replay {
    fn limit_reached(&self, polls: usize, _elapsed: Duration, _cap: Duration) -> bool {
        polls >= REPLAY_MAX_POLLS
    }

    fn report_limit(
        &self,
        relation: &str,
        status: &str,
        _link: &str,
        _cap: Duration,
        polls: usize,
    ) {
        emit_debug_log_message(format!(
            "Replay of {relation} still has refresh status `{status}` after {polls} polls; proceeding"
        ));
    }

    fn pause(&self, token: &CancellationToken, relation: &str) -> AdapterResult<()> {
        self.check_cancelled(token, relation)
    }

    /// A describe with no recording (the replay driver reports `NotFound`) ends the wait; every
    /// other error propagates. Recordings that end on `Running` (a live run that reached its cap)
    /// are not supported for replay: the extra poll consumes the next recorded slot.
    fn describe_failed(
        &self,
        relation: &str,
        err: AdapterError,
        describe_result: AgateTable,
    ) -> AdapterResult<AgateTable> {
        if err.kind() != AdapterErrorKind::NotFound {
            return Err(err);
        }
        emit_debug_log_message(format!(
            "Replay of {relation} has no further recorded describe ({err}); proceeding"
        ));
        Ok(describe_result)
    }
}

/// The `(Latest Refresh Status, Latest Refresh)` rows of a `DESCRIBE EXTENDED`. `None` when the
/// rows are absent or hold [`UNAVAILABLE`]: there is no refresh to wait for.
fn refresh_status(table: &AgateTable) -> AdapterResult<Option<(String, String)>> {
    let batch = table.original_record_batch();
    let col_names = batch.column_values::<StringArray>("col_name")?;
    let data_types = batch.column_values::<StringArray>("data_type")?;
    let row = |name: &str| {
        (0..batch.num_rows())
            .find(|&i| col_names.value(i).trim() == name)
            .map(|i| data_types.value(i).trim().to_string())
    };
    let Some(status) = row(STATUS_ROW).filter(|status| !status.starts_with(UNAVAILABLE)) else {
        return Ok(None);
    };
    Ok(Some((status, row(LINK_ROW).unwrap_or_default())))
}

enum Phase {
    Succeeded,
    /// `Failed`, `Canceled` or `Cancelled`: no refresh in flight.
    Failed,
    Running,
    Unknown,
}

impl From<&str> for Phase {
    fn from(status: &str) -> Self {
        let is = |s: &str| status.eq_ignore_ascii_case(s);
        if is("succeeded") {
            Phase::Succeeded
        } else if is("failed") || is("canceled") || is("cancelled") {
            Phase::Failed
        } else if is("running") {
            Phase::Running
        } else {
            Phase::Unknown
        }
    }
}

/// Re-runs `describe` until `Latest Refresh Status` is `Succeeded` and returns that result.
///
/// Without refresh information (see [`refresh_status`]) it returns at once. `Failed` and
/// `Canceled`/`Cancelled` warn and proceed: the config is still valid to read. A `Running` status
/// is polled for at most `timeout` and any other status for at most `unknown_timeout`; at the cap
/// the wait warns and proceeds.
pub(super) fn wait_for_refresh_to_finish(
    describe_result: AgateTable,
    relation: &str,
    token: &CancellationToken,
    wait: &RefreshWait,
    strategy: &dyn WaitStrategy,
    describe: impl FnMut() -> AdapterResult<AgateTable>,
) -> AdapterResult<AgateTable> {
    Poller {
        relation,
        token,
        wait,
        strategy,
        describe,
        start: Instant::now(),
    }
    .poll(describe_result, 0)
}

/// One wait in progress; [`Poller::poll`] handles a describe result and recurses on the next one.
struct Poller<'a, F> {
    relation: &'a str,
    token: &'a CancellationToken,
    wait: &'a RefreshWait,
    strategy: &'a dyn WaitStrategy,
    describe: F,
    start: Instant,
}

impl<F: FnMut() -> AdapterResult<AgateTable>> Poller<'_, F> {
    /// The recursion is at most one frame per poll: `RUNNING_TIMEOUT / POLL_INTERVAL` live and
    /// `REPLAY_MAX_POLLS` in replay.
    fn poll(&mut self, describe_result: AgateTable, polls: usize) -> AdapterResult<AgateTable> {
        let relation = self.relation;
        let Some((status, link)) = refresh_status(&describe_result)? else {
            emit_debug_log_message(format!(
                "No refresh information for {relation}; not waiting for a refresh"
            ));
            return Ok(describe_result);
        };
        emit_debug_log_message(format!(
            "Refresh status of {relation}: `{status}` after {:?}",
            self.start.elapsed()
        ));
        let cap = match Phase::from(status.as_str()) {
            Phase::Succeeded => return Ok(describe_result),
            Phase::Failed => {
                emit_warn_log_message(
                    ErrorCode::DbRefreshFailedWarning,
                    format!(
                        "Latest refresh of {relation} has status `{status}`; reading its config anyway"
                    ),
                );
                return Ok(describe_result);
            }
            Phase::Running => self.wait.timeout,
            Phase::Unknown => {
                emit_debug_log_message(format!(
                    "Unknown refresh status `{status}` for {relation}; polling"
                ));
                self.wait.unknown_timeout
            }
        };
        if self
            .strategy
            .limit_reached(polls, self.start.elapsed(), cap)
        {
            self.strategy
                .report_limit(relation, &status, &link, cap, polls);
            return Ok(describe_result);
        }
        self.strategy.pause(self.token, relation)?;
        match (self.describe)() {
            Ok(next) => self.poll(next, polls + 1),
            Err(err) => self
                .strategy
                .describe_failed(relation, err, describe_result),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::RecordBatch;
    use arrow_schema::{DataType, Field, Schema};
    use dbt_common::cancellation::{CancellationTokenSource, never_cancels};
    use std::cell::Cell;
    use std::sync::Arc;

    fn wait(timeout_ms: u64, unknown_ms: u64) -> RefreshWait {
        RefreshWait {
            timeout: Duration::from_millis(timeout_ms),
            unknown_timeout: Duration::from_millis(unknown_ms),
        }
    }

    fn live() -> Live {
        Live {
            interval: Duration::from_millis(1),
        }
    }

    fn describe_with(status: Option<&str>) -> AgateTable {
        let mut names = vec!["id", "Type"];
        let mut values = vec!["bigint", "MATERIALIZED_VIEW"];
        if let Some(status) = status {
            names.extend(["  Latest Refresh ", "Latest Refresh Status"]);
            values.extend(["https://host/pipelines/abc/updates/1", status]);
        }
        let schema = Arc::new(Schema::new(vec![
            Field::new("col_name", DataType::Utf8, false),
            Field::new("data_type", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(names)),
                Arc::new(StringArray::from(values)),
            ],
        )
        .unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    fn scripted(
        statuses: &[&'static str],
        calls: &Cell<usize>,
    ) -> impl FnMut() -> AdapterResult<AgateTable> {
        let statuses = statuses.to_vec();
        move || {
            let i = calls.get();
            calls.set(i + 1);
            Ok(describe_with(Some(statuses[i])))
        }
    }

    fn run(
        initial: Option<&str>,
        w: &RefreshWait,
        s: &dyn WaitStrategy,
        statuses: &[&'static str],
    ) -> usize {
        let calls = Cell::new(0);
        wait_for_refresh_to_finish(
            describe_with(initial),
            "mv",
            &never_cancels(),
            w,
            s,
            scripted(statuses, &calls),
        )
        .unwrap();
        calls.get()
    }

    #[test]
    fn waits_until_running_ends() {
        let calls = Cell::new(0);
        let out = wait_for_refresh_to_finish(
            describe_with(Some("Running")),
            "`c`.`s`.`mv`",
            &never_cancels(),
            &wait(5000, 5000),
            &live(),
            scripted(&["Running", "Succeeded"], &calls),
        )
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(refresh_status(&out).unwrap().unwrap().0, "Succeeded");
    }

    #[test]
    fn status_is_case_insensitive_and_trimmed() {
        assert_eq!(
            run(
                Some("  RUNNING \t"),
                &wait(5000, 5000),
                &live(),
                &["  succeeded "]
            ),
            1
        );
        assert_eq!(run(Some(" Succeeded "), &wait(5000, 5000), &live(), &[]), 0);
    }

    #[test]
    fn does_not_wait_without_refresh_information() {
        assert_eq!(run(None, &wait(5000, 5000), &live(), &[]), 0);
    }

    #[test]
    fn does_not_wait_when_refresh_information_is_unavailable() {
        for strategy in [&live() as &dyn WaitStrategy, &Replay] {
            let start = Instant::now();
            assert_eq!(run(Some(UNAVAILABLE), &wait(5000, 5000), strategy, &[]), 0);
            assert!(start.elapsed() < Duration::from_secs(1));
        }
    }

    #[test]
    fn refresh_information_turning_unavailable_stops_polling() {
        assert_eq!(
            run(Some("Running"), &wait(5000, 5000), &live(), &[UNAVAILABLE]),
            1
        );
    }

    #[test]
    fn refresh_status_reads_status_and_link() {
        let table = describe_with(Some("Succeeded"));
        assert_eq!(
            refresh_status(&table).unwrap(),
            Some((
                "Succeeded".to_string(),
                "https://host/pipelines/abc/updates/1".to_string()
            ))
        );
        assert_eq!(refresh_status(&describe_with(None)).unwrap(), None);
        assert_eq!(
            refresh_status(&describe_with(Some(UNAVAILABLE))).unwrap(),
            None
        );
    }

    #[test]
    fn failed_and_canceled_proceed() {
        for status in ["Failed", "Canceled", "Cancelled"] {
            assert_eq!(
                run(Some(status), &wait(5000, 5000), &live(), &[]),
                0,
                "{status}"
            );
        }
    }

    #[test]
    fn unknown_status_polls_then_proceeds_after_cap() {
        let calls = Cell::new(0);
        let out = wait_for_refresh_to_finish(
            describe_with(Some("Mystery")),
            "mv",
            &never_cancels(),
            &wait(60_000, 30),
            &live(),
            || {
                calls.set(calls.get() + 1);
                Ok(describe_with(Some("Mystery")))
            },
        )
        .unwrap();
        assert!(calls.get() > 0);
        assert_eq!(refresh_status(&out).unwrap().unwrap().0, "Mystery");
    }

    #[test]
    fn unknown_status_that_resolves_stops_polling() {
        assert_eq!(
            run(Some("Mystery"), &wait(5000, 5000), &live(), &["Succeeded"]),
            1
        );
    }

    fn replay_wait() -> RefreshWait {
        wait(5000, 5000)
    }

    #[test]
    fn replay_consumes_recorded_polls_without_sleeping() {
        let start = Instant::now();
        assert_eq!(
            run(
                Some("Running"),
                &replay_wait(),
                &Replay,
                &["Running", "Succeeded"]
            ),
            2
        );
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn replay_stops_when_recording_is_exhausted() {
        let calls = Cell::new(0);
        wait_for_refresh_to_finish(
            describe_with(Some("Running")),
            "mv",
            &never_cancels(),
            &replay_wait(),
            &Replay,
            || {
                calls.set(calls.get() + 1);
                Err(AdapterError::new(AdapterErrorKind::NotFound, "none"))
            },
        )
        .unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn replay_propagates_other_errors() {
        let err = wait_for_refresh_to_finish(
            describe_with(Some("Running")),
            "mv",
            &never_cancels(),
            &replay_wait(),
            &Replay,
            || Err(AdapterError::new(AdapterErrorKind::Driver, "mismatch")),
        )
        .unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Driver);
    }

    #[test]
    fn replay_stops_at_cap_when_status_never_changes() {
        let calls = Cell::new(0);
        let start = Instant::now();
        wait_for_refresh_to_finish(
            describe_with(Some("Running")),
            "mv",
            &never_cancels(),
            &replay_wait(),
            &Replay,
            || {
                calls.set(calls.get() + 1);
                Ok(describe_with(Some("Running")))
            },
        )
        .unwrap();
        assert_eq!(calls.get(), REPLAY_MAX_POLLS);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn strategies_differ_on_limit_and_errors() {
        let cap = Duration::from_secs(10);
        assert!(Live::DEFAULT.limit_reached(0, cap, cap));
        assert!(!Live::DEFAULT.limit_reached(1000, Duration::ZERO, cap));
        assert!(Replay.limit_reached(REPLAY_MAX_POLLS, Duration::ZERO, cap));
        assert!(!Replay.limit_reached(0, cap * 100, cap));
        let err = || AdapterError::new(AdapterErrorKind::NotFound, "none");
        assert!(
            Live::DEFAULT
                .describe_failed("mv", err(), describe_with(None))
                .is_err()
        );
        assert!(
            Replay
                .describe_failed("mv", err(), describe_with(None))
                .is_ok()
        );
    }

    #[test]
    fn describe_error_while_polling_propagates() {
        let err = wait_for_refresh_to_finish(
            describe_with(Some("Running")),
            "mv",
            &never_cancels(),
            &wait(5000, 5000),
            &live(),
            || Err(AdapterError::new(AdapterErrorKind::SqlExecution, "boom")),
        )
        .unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::SqlExecution);
    }

    #[test]
    fn running_past_its_cap_proceeds_with_the_last_describe() {
        let calls = Cell::new(0);
        let out = wait_for_refresh_to_finish(
            describe_with(Some("Running")),
            "`c`.`s`.`mv`",
            &never_cancels(),
            &wait(20, 60_000),
            &live(),
            || {
                calls.set(calls.get() + 1);
                Ok(describe_with(Some("Running")))
            },
        )
        .unwrap();
        assert!(calls.get() > 0);
        assert_eq!(refresh_status(&out).unwrap().unwrap().0, "Running");
    }

    #[test]
    fn stops_when_cancelled() {
        let source = CancellationTokenSource::new();
        let token = source.token();
        source.cancel();
        let calls = Cell::new(0);
        let err = wait_for_refresh_to_finish(
            describe_with(Some("Running")),
            "mv",
            &token,
            &wait(5000, 5000),
            &live(),
            scripted(&["Succeeded"], &calls),
        )
        .unwrap_err();
        assert_eq!(err.kind(), AdapterErrorKind::Cancelled);
        assert_eq!(calls.get(), 0);
    }
}
