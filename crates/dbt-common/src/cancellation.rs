use std::error::Error;
use std::panic;
use std::time::Duration;
use tokio::task::JoinError;

// re-export symbols that used to live here, but are now in dbt-cancel
pub use dbt_base::cancel::{
    Cancellable, CancellationToken, CancellationTokenSource, CancelledError, never_cancels,
};

/// The max timeout duration for waiting for running operations to
/// finish after Ctrl+C (the "final wait").
pub const TIMEOUT_AFTER_CTRL_C: Duration = Duration::from_secs(3);

/// Information about the cancellation process after Ctrl+C (or fail-fast).
#[derive(Debug)]
pub struct CancellationReport {
    /// Whether cancellation was triggered by a fail-fast.
    pub fail_fast_triggered: bool,
    /// The max timeout duration for waiting for running operations to
    /// finish after Ctrl+C (the "final wait").
    pub timeout: Duration,
    /// The duration of the final wait after Ctrl+C.
    pub final_wait_duration: Duration,
    /// Whether the final wait after Ctrl+C timed out and a forceful cancellation of the
    /// [Future] was needed.
    pub timed_out: bool,
    /// How much time was spent canceling statements after Ctrl+C and final wait.
    ///
    /// Cancellation of statements starts in parallel with the final wait, but if in
    /// that final wait, more statements start executing, the cancellation will spend
    /// some extra time until all statements are cancelled.
    pub stmt_cancel_duration: Duration,
    /// How many SQL statements were cancelled.
    pub stmt_cancel_count: usize,
    /// How many SQL statements failed to cancel.
    pub stmt_cancel_fail_count: usize,
}

pub fn cancellable_from_join_error<T: Error>(err: JoinError) -> Cancellable<T> {
    if err.is_cancelled() {
        Cancellable::Cancelled
    } else if err.is_panic() {
        panic::resume_unwind(err.into_panic());
    } else {
        unreachable!("JoinError's are either due to cancellation or panic");
    }
}
