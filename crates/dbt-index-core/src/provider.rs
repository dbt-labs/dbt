//! Generic provider trait + the one no-op stub used for every feature.
//!
//! A `Provider` represents one feature ("column lineage", "column impact",
//! "sample data", etc.) parameterised by its input and output types. The
//! default [`Provider::is_available`] returns `false`, so an unimplemented
//! stub reports unavailable without any extra code; host callers (CLI
//! dispatch, HTTP handlers) use that signal for PLG gating.
//!
//! The trait surface is intentionally a single generic shape rather than
//! one trait per feature: a single [`UnavailableProvider<Args, Output>`]
//! stub then covers every feature whose `Output` impls [`ProviderOutput`]
//! (e.g. every `Result<_, LineageError>` via the blanket impl in
//! [`super::column_lineage`]).

use std::marker::PhantomData;

/// Top-level provider trait. One implementation per feature.
///
/// `Send + Sync` is required so the impl can be held in
/// `Arc<dyn Provider<...>>` behind a shared application state.
pub trait Provider: Send + Sync {
    /// Inputs the provider operates on.
    type Args;
    /// What the provider returns. Typically a `Result<T, E>`.
    type Output;

    /// Whether this provider is available in the current distribution +
    /// data set. Defaults to `false`, so a no-op stub auto-reports
    /// unavailable.
    fn is_available(&self) -> bool {
        false
    }

    /// Execute the feature.
    fn run(&self, args: Self::Args) -> Self::Output;
}

/// Outputs that carry an "unavailable" signal. Required for any
/// `(Args, Output)` pair to be usable with [`UnavailableProvider`].
///
/// The blanket impl in [`super::column_lineage`] covers every
/// `Result<_, LineageError>`, which is what all feature outputs are
/// today — so feature authors don't normally write this impl by hand.
pub trait ProviderOutput {
    /// Construct the "feature not available" form of this output.
    fn unavailable() -> Self;
}

/// The single no-op [`Provider`] used for every feature that isn't
/// shipped in a given distribution. Parameterise it with the feature's
/// `(Args, Output)` pair:
///
/// ```ignore
/// type UnavailableColumnLineage =
///     UnavailableProvider<ColumnLineageArgs, ColumnLineageOutput>;
/// ```
///
/// Inherits `is_available -> false` from the trait default; `run`
/// returns `Output::unavailable()`.
pub struct UnavailableProvider<Args, Output>(PhantomData<fn(Args) -> Output>);

impl<Args, Output> UnavailableProvider<Args, Output> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<Args, Output> Default for UnavailableProvider<Args, Output> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Args, Output> Provider for UnavailableProvider<Args, Output>
where
    Args: Send + Sync,
    Output: ProviderOutput + Send + Sync,
{
    type Args = Args;
    type Output = Output;

    fn run(&self, _args: Args) -> Output {
        Output::unavailable()
    }
}
