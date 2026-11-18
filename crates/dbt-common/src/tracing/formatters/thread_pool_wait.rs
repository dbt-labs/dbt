use std::fmt;

use super::duration::FixedWidthDuration;

pub const THREAD_POOL_WAIT_START: &str = "Started thread pool wait";

/// Displays the end of a thread pool wait.
pub struct ThreadPoolWaitEnd(pub std::time::Duration);

impl fmt::Display for ThreadPoolWaitEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Finished thread pool wait [{}]",
            FixedWidthDuration(self.0)
        )
    }
}
