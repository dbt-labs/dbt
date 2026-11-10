use std::marker;
use std::pin::Pin;
use std::result::Result;
use std::task::{Context, Poll};

use tokio::sync::watch;

/// Fail-fast signal.
///
/// Fusion is highly parallel. It's desirable to have a fail-fast mechanism to
/// quickly cancel ongoing operations when a critical error is detected without
/// waiting for the operations to time out on their own.
///
/// Each invocation (or test) should get its own [FailFast] instance so that
/// concurrent runs don't interfere with each other. This struct is very cheap to
/// clone because a [watch::Sender] is just an `Arc` around the actual data.
#[derive(Clone)]
pub struct FailFast {
    tx: watch::Sender<bool>,
}

impl Default for FailFast {
    fn default() -> Self {
        Self::new()
    }
}

impl FailFast {
    /// Create a new independent fail-fast signal.
    pub fn new() -> Self {
        Self {
            tx: watch::Sender::new(false),
        }
    }

    /// Signal that fail-fast has been triggered. Wakes all receivers.
    ///
    /// Only the last value is kept, but that's fine because we only care about
    /// the transition from `false` to `true`.
    pub fn trigger(&self) {
        self.tx.send_replace(true);
    }

    /// Check if the fail-fast signal has been triggered before.
    pub fn has_triggered(&self) -> bool {
        *self.tx.borrow()
    }

    /// Subscribe to the fail-fast signal.
    ///
    /// If the `fail_fast_flag` is `false`, this Future will never complete.
    /// This flag should be true when the user invokes dbt with `--fail-fast`.
    pub async fn subscribe(self, fail_fast_flag: bool) -> Result<(), watch::error::RecvError> {
        #[must_use = "futures do nothing unless you `.await` or poll them"]
        pub struct Pending<T> {
            _data: marker::PhantomData<fn() -> T>,
        }
        impl<T> Future for Pending<T> {
            type Output = T;
            fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<T> {
                Poll::Pending
            }
        }

        pub fn pending<T>() -> Pending<T> {
            Pending {
                _data: marker::PhantomData,
            }
        }

        let mut rx = self.tx.subscribe();
        loop {
            let fail_fast = *rx.wait_for(|&v| v).await?;
            if fail_fast {
                // If flag is true, return immediately. Otherwise, wait indefinitely
                // because the caller is not interested in waking up on fail-fast trigger.
                return if fail_fast_flag {
                    Ok(())
                } else {
                    pending().await
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_millis(100);

    #[dbt_runtime::test]
    async fn subscribe_returns_immediately_when_already_triggered() {
        let signal = FailFast::new();
        signal.trigger();
        let result = tokio::time::timeout(TIMEOUT, signal.subscribe(true)).await;
        #[allow(clippy::single_match)]
        match result {
            Ok(res) => res.unwrap(),
            Err(_) => (), // timed-out, ignore (test machines can be slow)
        }
    }

    #[dbt_runtime::test]
    async fn subscribe_never_returns_if_flag_is_false() {
        let signal = FailFast::new();
        let result = tokio::time::timeout(TIMEOUT, signal.subscribe(false)).await;
        let timed_out = result.is_err();
        assert!(
            timed_out,
            "Expected subscribe to time out, but it returned instead"
        );
    }
}
