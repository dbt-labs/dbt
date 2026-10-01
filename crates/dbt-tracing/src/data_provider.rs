use crate::{
    AnyTelemetryEvent, TelemetryAttributes,
    metrics::{
        MetricKey, get_all_metrics_from_span_extension, get_metric_from_span_extension,
        increment_metric_on_span,
    },
    span_info::SpanAccess,
};
use std::any::TypeId;
use tracing_subscriber::registry::{LookupSpan, SpanRef};

/// A data provider allowing safe access to metrics and efficient, thread-safe
/// storage of arbitrary data on a per-invocation basis, that can be used
/// by consumer and middleware layers. E.g. to delay exporting of events.
///
/// Technical note:
/// This is a wrapper around tracing lib's span extensions that provides a safer
/// API (prevents deadlocks) and implements on-demand access to root span extensions
/// that avoids long living locks on span extensions. Root span is shared among all threads, so we use
/// this to avoid contention on invocation level data.
///
/// Even though this provider uses interior mutability through the span extension system,
/// some write operations require `&mut self` receiver to prevent self-deadlocks.
pub struct DataProvider<'a> {
    root_span: Option<&'a dyn SpanAccess>,
    current_span: Option<&'a dyn SpanAccess>,
}

impl<'a> DataProvider<'a> {
    /// Creates a new data provider from the span.
    pub(super) fn new<'sp, R>(
        root_span: &'a SpanRef<'sp, R>,
        current_span: &'a SpanRef<'sp, R>,
    ) -> Self
    where
        R: LookupSpan<'sp>,
    {
        Self {
            root_span: Some(root_span as &dyn SpanAccess),
            current_span: Some(current_span as &dyn SpanAccess),
        }
    }

    pub(super) fn none() -> Self {
        Self {
            root_span: None,
            current_span: None,
        }
    }

    /// Initializes an extension value on the root span.
    ///
    /// Note, that it will replace any existing value of the same type.
    ///
    /// # Returns
    ///
    /// `Some(T)` if the root span exists and a previous value was replaced.
    pub fn init_root<T>(&self, value: T) -> Option<T>
    where
        T: Send + Sync + 'static,
    {
        self.root_span.and_then(|root_span| {
            let mut mut_extensions = root_span.extensions_mut();
            mut_extensions.replace(value)
        })
    }

    /// Accesses an extension value on the root span for reading.
    pub fn with_root<T>(&self, f: impl FnOnce(&T))
    where
        T: Send + Sync + 'static,
    {
        if let Some(root_span) = self.root_span
            && let Some(ext) = root_span.extensions().get::<T>()
        {
            f(ext)
        };
    }

    /// Accesses an extension value on the root span for mutation.
    ///
    /// The closure is called with a mutable reference to the extension value if it exists.
    ///
    /// We require `&mut self` to avoid pitfalls of self-locking since this
    /// function will acquire a write lock on span extensions and closure may
    /// try using the same data provider reference again.
    pub fn with_root_mut<T>(&mut self, f: impl FnOnce(&mut T))
    where
        T: Send + Sync + 'static,
    {
        if let Some(root_span) = self.root_span
            && let Some(ext) = root_span.extensions_mut().get_mut::<T>()
        {
            f(ext)
        };
    }

    /// Initializes an extension value on the current span. Use in conjunction
    /// with `with_ancestor_ext` to store data on intermediate spans in the span tree
    /// for later access by descendant spans.
    ///
    /// Note, that it will replace any existing value of the same type.
    ///
    /// # Returns
    ///
    /// `Some(T)` if the root span exists and a previous value was replaced.
    pub fn init_cur<T>(&self, value: T) -> Option<T>
    where
        T: Send + Sync + 'static,
    {
        self.current_span.and_then(|root_span| {
            let mut mut_extensions = root_span.extensions_mut();
            mut_extensions.replace(value)
        })
    }

    /// Accesses an extension value on the closest ancestor span whose TelemetryAttributes
    /// match the given type A, starting from the current span and going up to the root.
    ///
    /// NOTE:
    /// - On span start callbacks, the current span doesn't have access to it's own attributes,
    ///   so the search will effectively from the parent span.
    /// - On span end and for logs, the current span is included in the search.
    ///
    /// The closure is called with a reference to the attributes and the extension value.
    ///
    /// Use `with_ancestor_attrs` to access the ancestor telemetry attributes themselves.
    ///
    /// This function will be no-op in the following cases:
    /// - There is no current span (only possible for a log outside of any span)
    /// - No ancestor span has TelemetryAttributes of type A
    /// - The found ancestor span does not have an extension of type T
    ///   (meaning `init_cur` was not called for T on that span)
    pub fn with_ancestor_ext<A, T>(&self, f: impl FnOnce(&A, &T))
    where
        A: AnyTelemetryEvent,
        T: Send + Sync + 'static,
    {
        debug_assert!(
            TypeId::of::<T>() != TypeId::of::<TelemetryAttributes>(),
            "use with_ancestor_attrs for telemetry attributes access"
        );

        let Some(current_span) = self.current_span else {
            return;
        };

        // Wrap closure to make it FnMut for for_each_in_scope, although
        // logically it can only be called once.
        let mut f = Some(f);

        current_span.for_each_in_scope(&mut |span| {
            let extensions = span.extensions();

            // Check if this span has TelemetryAttributes of type A
            if let Some(attrs) = extensions
                .get::<TelemetryAttributes>()
                .and_then(|attrs| attrs.downcast_ref::<A>())
            {
                // Found matching span, access the extension
                if let Some(ext) = extensions.get::<T>()
                    && let Some(f) = f.take()
                {
                    f(attrs, ext);
                }

                return false; // Stop iteration
            }

            true // Continue iteration
        });
    }

    /// Accesses the closest ancestor span's telemetry attributes matching type A,
    /// starting from the current span and going up to the root.
    ///
    /// NOTE:
    /// - On span start callbacks, the current span doesn't have access to it's own attributes,
    ///   so the search will effectively from the parent span.
    /// - On span end and for logs, the current span is included in the search.
    ///
    /// This function will be no-op in the following cases:
    /// - There is no current span (only possible for a log outside of any span)
    /// - No ancestor span has TelemetryAttributes of type A
    pub fn with_ancestor_attrs<A>(&self, f: impl FnOnce(&A))
    where
        A: AnyTelemetryEvent,
    {
        let Some(current_span) = self.current_span else {
            return;
        };

        // Wrap closure to make it FnMut for for_each_in_scope, although
        // logically it can only be called once.
        let mut f = Some(f);

        current_span.for_each_in_scope(&mut |span| {
            let extensions = span.extensions();

            if let Some(attrs) = extensions
                .get::<TelemetryAttributes>()
                .and_then(|attrs| attrs.downcast_ref::<A>())
            {
                if let Some(f) = f.take() {
                    f(attrs);
                }

                return false; // Stop iteration
            }

            true // Continue iteration
        });
    }

    /// Accesses an extension value for mutation on the closest ancestor span whose
    /// TelemetryAttributes match the given type A, starting from the current span and going up to the root.
    ///
    /// NOTE:
    /// - On span start callbacks, the current span doesn't have access to it's own attributes,
    ///   so the search will effectively from the parent span.
    /// - On span end and for logs, the current span is included in the search.
    ///
    /// The closure is called with a mutable reference to the extension value.
    ///
    /// Use `with_ancestor_attrs_mut` to mutate the ancestor telemetry attributes themselves.
    ///
    /// This function will be no-op in the following cases:
    /// - There is no current span (only possible for a log outside of any span)
    /// - No ancestor span has TelemetryAttributes of type A
    /// - The found ancestor span does not have an extension of type T
    ///   (meaning `init_cur` was not called for T on that span)
    ///
    /// We require `&mut self` to avoid pitfalls of self-locking since this
    /// function will acquire a write lock on span extensions and closure may
    /// try using the same data provider reference again.
    pub fn with_ancestor_ext_mut<A, T>(&mut self, f: impl FnOnce(&mut T))
    where
        A: AnyTelemetryEvent,
        T: Send + Sync + 'static,
    {
        debug_assert!(
            TypeId::of::<T>() != TypeId::of::<TelemetryAttributes>(),
            "use with_ancestor_attrs_mut for telemetry attributes access"
        );

        let Some(current_span) = self.current_span else {
            return;
        };

        // Wrap closure to make it FnMut for for_each_in_scope, although
        // logically it can only be called once.
        let mut f = Some(f);

        current_span.for_each_in_scope(&mut |span| {
            // Check if this span has TelemetryAttributes of type A
            if span
                .extensions()
                .get::<TelemetryAttributes>()
                .map(|attrs| attrs.is::<A>())
                .is_none_or(|is_a| !is_a)
            {
                return true; // Continue iteration
            }

            // Found matching span, access the extension
            if let Some(ext) = span.extensions_mut().get_mut::<T>()
                && let Some(f) = f.take()
            {
                f(ext);

                return false; // Stop iteration
            }

            true // Continue iteration
        });
    }

    /// Mutates the closest ancestor span's telemetry attributes matching type A,
    /// starting from the current span and going up to the root.
    ///
    /// NOTE:
    /// - On span start callbacks, the current span doesn't have access to it's own attributes,
    ///   so the search will effectively from the parent span.
    /// - On span end and for logs, the current span is included in the search.
    ///
    /// This function will be no-op in the following cases:
    /// - There is no current span (only possible for a log outside of any span)
    /// - No ancestor span has TelemetryAttributes of type A
    ///
    /// We require `&mut self` to avoid pitfalls of self-locking since this
    /// function will acquire a write lock on span extensions and closure may
    /// try using the same data provider reference again.
    pub fn with_ancestor_attrs_mut<A>(&mut self, f: impl FnOnce(&mut A))
    where
        A: AnyTelemetryEvent,
    {
        let Some(current_span) = self.current_span else {
            return;
        };

        // Wrap closure to make it FnMut for for_each_in_scope, although
        // logically it can only be called once.
        let mut f = Some(f);

        current_span.for_each_in_scope(&mut |span| {
            let has_matching_attrs = {
                let extensions = span.extensions();
                extensions
                    .get::<TelemetryAttributes>()
                    .is_some_and(|attrs| attrs.is::<A>())
            };

            if !has_matching_attrs {
                return true; // Continue iteration
            }

            if let Some(attrs) = span
                .extensions_mut()
                .get_mut::<TelemetryAttributes>()
                .and_then(|attrs| attrs.downcast_mut::<A>())
                && let Some(f) = f.take()
            {
                f(attrs);
            }

            false // Stop iteration
        });
    }

    /// Gets a specific per-invocation metric (stored in the root invocation span).
    pub fn get_metric(&self, key: impl Into<MetricKey>) -> u64 {
        // Keep the public API generic while routing through a non-generic helper to
        // avoid extra monomorphized copies at call sites.
        self.get_metric_inner(key.into())
    }

    fn get_metric_inner(&self, key: MetricKey) -> u64 {
        self.root_span
            .map(|root_span| get_metric_from_span_extension(&root_span.extensions(), key))
            .unwrap_or_default()
    }

    /// Gets all per-invocation metrics (stored in the root invocation span).
    pub fn get_all_metrics(&self) -> Vec<(MetricKey, u64)> {
        self.root_span
            .map(|root_span| get_all_metrics_from_span_extension(&root_span.extensions()))
            .unwrap_or_default()
    }

    /// Increments a per-invocation metric counter on the invocation span extensions directly.
    pub fn increment_metric(&self, key: impl Into<MetricKey>, value: u64) {
        // Keep the public API generic while routing through a non-generic helper to
        // avoid extra monomorphized copies at call sites.
        self.increment_metric_inner(key.into(), value);
    }

    fn increment_metric_inner(&self, key: MetricKey, value: u64) {
        if let Some(root_span) = self.root_span {
            increment_metric_on_span(root_span, key, value);
        }
    }
}
