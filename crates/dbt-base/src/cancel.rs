use std::{
    error::Error,
    fmt,
    sync::{
        Arc, LazyLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

static NEVER_CANCELS_CST: LazyLock<CancellationTokenSource> =
    LazyLock::new(CancellationTokenSource::new);

pub struct CancelledError;

/// Turns any error type `E` into a cancellable error type.
///
/// A function that returns `Result<T, Cancellable<E>>` can return
/// `Err(Cancellable::Cancelled)` to indicate that the operation was
/// cancelled.
///
/// If `E` can represent the cancellation state, the user can provide
/// an implementation of `From<CancelledError>` for `E` to convert
/// `Cancellable::Cancelled` into `E` automatically.
#[derive(Debug)]
pub enum Cancellable<E> {
    Cancelled,
    Error(E),
}

impl<E: From<CancelledError>> Cancellable<E> {
    /// Flatten `Cancellable<E>` into `E`.
    ///
    /// This relies on the `From<CancelledError>` implementation for `E`.
    pub fn flatten(self) -> E {
        match self {
            Cancellable::Cancelled => E::from(CancelledError),
            Cancellable::Error(e) => e,
        }
    }
}

impl<E> From<CancelledError> for Cancellable<E> {
    fn from(_: CancelledError) -> Self {
        Cancellable::Cancelled
    }
}

impl<E: Error> From<E> for Cancellable<E> {
    fn from(err: E) -> Self {
        Cancellable::Error(err)
    }
}

/// Inner structure for [CancellationTokenSource].
#[derive(Default, Debug)]
struct InnerCST {
    request_id: AtomicU64,
}

/// A source of cancellation tokens that can be used to signal cancellation requests.
///
/// [CancellationToken]s can be issued with `.token()`. `.cancel()` can be called to
/// cancel all tokens issued so far. Additionally, if the [CancellationTokenSource]
/// is dropped, all tokens issued from it will be considered cancelled.
///
/// ```rust
/// # use dbt_base::cancel::{
/// #  CancellationTokenSource, CancellationToken, CancelledError
/// # };
/// # struct Handler {
/// #  cts: CancellationTokenSource,
/// # }
///
/// impl Handler {
///   pub async fn handle_request(&self, n: u64) -> Result<(), CancelledError> {
///     self.cts.cancel(); // cancel work for previous requests
///     let token = self.cts.token(); // issue a fresh cancellation token
///     self.process_request(n, &token).await
///   }
///
///   async fn process_request(
///     &self,
///     n: u64,
///     token: &CancellationToken,
///   ) -> Result<(), CancelledError> {
///     for i in 0..n {
///       self.work(i);
///       token.check_cancellation()?; // check before doing more work
///     }
///
///     self.non_cancellable_async_work().await;
///
///     // check_cancellation() uses token.is_cancelled() internally
///     token.check_cancellation()?; // check before doing more work
///     self.cancellable_async_work(token).await;
///
///     token.check_cancellation()?; // check before doing more work
///     self.cancellable_async_work(token).await
///   }
///
///   // This function could return:
///   // - `Result<_, E>` for any E that implements From<CancelledError>
///   // - `Result<_, Cancellable<E>>` for any E
///   // - `Result<_, CancelledError>`
///   async fn cancellable_async_work(
///     &self,
///     token: &CancellationToken,
///   ) -> Result<(), CancelledError> {
///     self.work(42);
///     token.check_cancellation()?; // check before continuing
///     self.work(42);
///     Ok(())
///   }
///
///   async fn non_cancellable_async_work(&self) {
///     // ...async function that does not take a cancellation token...
/// #   unimplemented!();
///   }
///
/// # fn work(&self, i: u64) { }
/// }
/// ```
#[derive(Clone, Default, Debug)]
pub struct CancellationTokenSource {
    inner: Arc<InnerCST>,
}

impl CancellationTokenSource {
    pub fn new() -> Self {
        let inner = Arc::new(InnerCST {
            request_id: AtomicU64::new(0),
        });
        CancellationTokenSource { inner }
    }

    /// Issues a fresh cancellation token from this source.
    pub fn token(&self) -> CancellationToken {
        let request_id = self.inner.request_id.load(Ordering::Acquire);
        CancellationToken::new(Arc::downgrade(&self.inner), request_id)
    }

    /// Cancels all tokens created from this source so far.
    #[inline]
    pub fn cancel(&self) {
        self.inner.request_id.fetch_add(1, Ordering::AcqRel);
    }
}

/// Internal trait for cancellation token implementations.
trait CancellationTokenLike: fmt::Debug + Send + Sync + 'static {
    fn is_cancelled(&self) -> bool;
    fn clone_box(&self) -> Box<dyn CancellationTokenLike>;
}

#[derive(Clone, Debug)]
struct CancellationTokenImpl {
    source: Weak<InnerCST>,
    request_id: u64,
}

impl CancellationTokenLike for CancellationTokenImpl {
    fn is_cancelled(&self) -> bool {
        if let Some(source) = self.source.upgrade() {
            return source.request_id.load(Ordering::Acquire) > self.request_id;
        }
        true // CancellationTokenSource has been dropped
    }

    fn clone_box(&self) -> Box<dyn CancellationTokenLike> {
        Box::new(self.clone())
    }
}

/// Implementation used by [CancellationToken::combine_with_flag()].
#[derive(Debug)]
struct CancellationTokenWithFlagImpl {
    inner: Box<dyn CancellationTokenLike>,
    should_cancel_flag: Arc<AtomicBool>,
}

impl Clone for CancellationTokenWithFlagImpl {
    fn clone(&self) -> Self {
        CancellationTokenWithFlagImpl {
            inner: self.inner.clone_box(),
            should_cancel_flag: Arc::clone(&self.should_cancel_flag),
        }
    }
}

impl CancellationTokenLike for CancellationTokenWithFlagImpl {
    fn is_cancelled(&self) -> bool {
        if self.should_cancel_flag.load(Ordering::Acquire) {
            return true;
        }
        self.inner.is_cancelled()
    }

    fn clone_box(&self) -> Box<dyn CancellationTokenLike> {
        Box::new(self.clone())
    }
}

/// A cancellation token that can be used to check for cancellation requests.
///
/// A token is created from a [CancellationTokenSource] and can be used to
/// check if the operation using it has been cancelled. If the token is
/// cancelled, it will return `true` for `is_cancelled()`. Tasks should
/// check the cancellation state periodically using `check_cancellation()`
/// or `is_cancelled()` directly.
#[derive(Debug)]
pub struct CancellationToken {
    inner: Box<dyn CancellationTokenLike>,
}

impl Clone for CancellationToken {
    fn clone(&self) -> Self {
        CancellationToken {
            inner: self.inner.clone_box(),
        }
    }
}

impl CancellationToken {
    /// Private constructor for [CancellationToken].
    ///
    /// Use [CancellationTokenSource::token()] or, for tests,
    /// [CancellationToken::never_cancels()], to create instances.
    fn new(source: Weak<InnerCST>, request_id: u64) -> Self {
        let inner = Box::new(CancellationTokenImpl { source, request_id });
        Self { inner }
    }

    /// Creates a cancellation token that never cancels.
    ///
    /// Useful in tests and as a escape hatch when cancellation
    /// tokens haven't been passed through call chains yet.
    pub fn never_cancels() -> Self {
        NEVER_CANCELS_CST.token()
    }

    /// Checks if the token has been cancelled.
    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    /// Checks if the token has been cancelled and returns a `Result`.
    ///
    /// `token.check_cancellation()?;` can be used in functions that return:
    ///
    /// - `Result<T, E>` for any `E` that implements `From<CancelledError>`
    /// - `Result<T, Cancellable<E>>` for any `E`
    /// - `Result<T, CancelledError>`
    #[inline]
    pub fn check_cancellation(&self) -> Result<(), CancelledError> {
        if self.is_cancelled() {
            Err(CancelledError)
        } else {
            Ok(())
        }
    }

    /// Combines this cancellation token with a flag that can be set to cancel.
    ///
    /// This allows for additional cancellation logic based on an external flag.
    /// This cancellation token will be cancelled if either the original token
    /// is cancelled or the shared `should_cancel_flag` is set to `true`.
    pub fn combine_with_flag(self, should_cancel_flag: Arc<AtomicBool>) -> CancellationToken {
        let inner = Box::new(CancellationTokenWithFlagImpl {
            inner: self.inner,
            should_cancel_flag,
        });
        CancellationToken { inner }
    }
}

/// Creates a cancellation token that never cancels.
///
/// Useful in tests and as a escape hatch when cancellation
/// tokens haven't been passed through call chains yet.
pub fn never_cancels() -> CancellationToken {
    CancellationToken::never_cancels()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use std::{fmt, thread};

    #[derive(Debug)]
    struct MyError {
        cancelled: bool,
    }

    impl MyError {
        fn new() -> Self {
            MyError { cancelled: false }
        }
    }
    impl Error for MyError {}

    impl fmt::Display for MyError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "MyError: cancelled={}", self.cancelled)
        }
    }

    impl From<Cancellable<MyError>> for MyError {
        fn from(value: Cancellable<MyError>) -> Self {
            match value {
                Cancellable::Cancelled => MyError::from(CancelledError),
                Cancellable::Error(e) => e,
            }
        }
    }

    impl From<CancelledError> for MyError {
        fn from(_: CancelledError) -> Self {
            MyError { cancelled: true }
        }
    }

    #[test]
    fn test_cancellation_token_source() {
        let cts = CancellationTokenSource::new();
        let token = cts.token();

        assert!(!token.is_cancelled());

        cts.cancel();
        assert!(token.is_cancelled());

        let new_token = cts.token();
        assert!(!new_token.is_cancelled());

        drop(cts);
        assert!(new_token.is_cancelled());
    }

    #[test]
    fn test_combine_with_flag() {
        let cts = CancellationTokenSource::new();
        let token = cts.token();
        let should_cancel_flag = Arc::new(AtomicBool::new(false));

        let token = token.combine_with_flag(Arc::clone(&should_cancel_flag));
        assert!(!token.is_cancelled());

        cts.cancel();
        assert!(token.is_cancelled());

        let new_token = cts.token();
        assert!(!new_token.is_cancelled());

        let new_token = new_token.combine_with_flag(Arc::clone(&should_cancel_flag));
        assert!(!new_token.is_cancelled());

        should_cancel_flag.store(true, Ordering::Release);
        assert!(new_token.is_cancelled());

        should_cancel_flag.store(false, Ordering::Release);
        assert!(!new_token.is_cancelled());

        cts.cancel();
        assert!(new_token.is_cancelled());
    }

    #[test]
    fn test_thread_safety() {
        let cts = CancellationTokenSource::new();
        let token = cts.token();

        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cts.cancel();
        });
        assert!(!token.is_cancelled());
        while !token.is_cancelled() {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(token.is_cancelled());

        handle.join().unwrap();
    }

    fn fail() -> Result<(), MyError> {
        Err(MyError::new())
    }

    fn cancellable_fail() -> Result<(), Cancellable<MyError>> {
        fail()?;
        Err(MyError::new().into()) // .into() converts MyError to Cancellable<MyError>
    }

    fn nocancel() -> Result<(), Cancellable<MyError>> {
        Ok(())
    }

    fn cancel() -> Result<(), Cancellable<MyError>> {
        Err(Cancellable::Cancelled)
    }

    fn fail_because_cancelled() -> Result<(), MyError> {
        nocancel()?;
        cancel()?; // Cancelled converts to MyError via From<CancelledError> for MyError
        Ok(())
    }

    fn fail_because_fail() -> Result<(), Cancellable<MyError>> {
        fail()?; // MyError converts to Cancellable<MyError> via From<E> for Cancellable<E>
        Ok(())
    }

    #[test]
    fn test_cancellable_conversion() {
        assert_eq!(
            format!("{:?}", cancellable_fail().unwrap_err()),
            "Error(MyError { cancelled: false })"
        );
        assert_eq!(format!("{:?}", cancel().unwrap_err()), "Cancelled");
        assert_eq!(
            format!("{}", fail_because_cancelled().unwrap_err()),
            "MyError: cancelled=true"
        );
        assert_eq!(
            format!("{:?}", fail_because_fail().unwrap_err()),
            "Error(MyError { cancelled: false })"
        );
    }

    fn long_cancellable_task(token: &CancellationToken) -> Result<(), Cancellable<MyError>> {
        // ...some work...
        token.check_cancellation()?; // check before proceeding
        // ...more work...
        Ok(())
    }

    fn long_task(token: &CancellationToken) -> Result<(), MyError> {
        // ...some work...
        token.check_cancellation()?; // Cancelled converts to MyError via From<CancelledError> for MyError
        // ...more work...
        Ok(())
    }

    #[test]
    fn test_token_check() {
        let cts = CancellationTokenSource::new();

        let token = cts.token();
        assert_eq!(format!("{:?}", long_cancellable_task(&token)), "Ok(())");
        cts.cancel();
        assert_eq!(
            format!("{:?}", long_cancellable_task(&token)),
            "Err(Cancelled)"
        );

        let token = cts.token();
        assert_eq!(format!("{:?}", long_task(&token)), "Ok(())");
        cts.cancel();
        assert_eq!(
            format!("{:?}", long_task(&token)),
            "Err(MyError { cancelled: true })"
        );
    }
}
