//! Async tracing helpers for propagating span context across task boundaries.

use tracing::Instrument as _;

/// Helper to spawn async tasks while preserving tracing span context.
/// This ensures all spawned work inherits the current span.
///
/// # Example
/// ```ignore
/// spawn_traced(async {
///     // This task inherits the current span
///     tracing::info!("This log will be in the parent span");
/// });
/// ```
pub fn spawn_traced<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    if cfg!(debug_assertions) {
        assert!(
            tokio::runtime::Handle::try_current().is_ok(),
            "spawn_traced called outside of a tokio runtime:\n{:?}",
            std::backtrace::Backtrace::force_capture()
        );
    }
    tokio::spawn(future.in_current_span())
}

/// Helper to spawn async tasks while preserving tracing span context.
/// This ensures all spawned work inherits the current span.
/// The task spawned will be blocked in place.
///
/// # Example
/// ```ignore
/// spawn_traced_block_in_place(async {
///     // This task inherits the current span
///     tracing::info!("This log will be in the parent span");
/// });
/// ```
pub fn spawn_traced_block_in_place<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let task = spawn_traced(future);
    tokio::task::block_in_place(move || tokio::runtime::Handle::current().block_on(task))
        .expect("expected to finish task")
}
