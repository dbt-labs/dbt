use std::collections::BTreeMap;

use crate::{
    AnyTelemetryEvent, DebugValue, SpanStartInfo, SpanStatus, TelemetryAttributes,
    layers::data_layer::DLSpanStartInfo, shared::Recordable,
};

use tracing::Span;
use tracing_subscriber::{
    Registry,
    registry::{Extensions, ExtensionsMut, LookupSpan, SpanRef},
};

/// Object-safe abstraction over a span sufficient for our needs.
/// This erases the concrete registry type `R` and avoids generic blowup
pub(super) trait SpanAccess {
    fn extensions(&self) -> Extensions<'_>;
    fn extensions_mut(&self) -> ExtensionsMut<'_>;

    /// Iterates over ancestor spans starting from this span and going up to the root.
    /// The iterator includes this span as the first element.
    /// The callback should return true to continue iteration, false to stop.
    fn for_each_in_scope(&self, f: &mut dyn FnMut(&dyn SpanAccess) -> bool);
}

impl<'a, R> SpanAccess for SpanRef<'a, R>
where
    R: LookupSpan<'a>,
{
    fn extensions(&self) -> Extensions<'_> {
        SpanRef::extensions(self)
    }

    fn extensions_mut(&self) -> ExtensionsMut<'_> {
        SpanRef::extensions_mut(self)
    }

    fn for_each_in_scope(&self, f: &mut dyn FnMut(&dyn SpanAccess) -> bool) {
        for span_ref in self.scope() {
            if !f(&span_ref as &dyn SpanAccess) {
                break;
            }
        }
    }
}

/// Helper that extracts arbitrary captured fields into a map.
pub(super) fn get_span_debug_extra_attrs(values: Recordable<'_>) -> BTreeMap<String, DebugValue> {
    struct SpanEventAttributesVisitor(BTreeMap<String, DebugValue>);

    impl tracing::field::Visit for SpanEventAttributesVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .insert(field.name().to_string(), format!("{value:?}").into());
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_string(), value.into());
        }

        fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
            self.0.insert(field.name().to_string(), value.into());
        }

        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            self.0.insert(field.name().to_string(), value.into());
        }

        fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
            self.0.insert(field.name().to_string(), value.into());
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            self.0.insert(field.name().to_string(), value.into());
        }

        fn record_bytes(&mut self, field: &tracing::field::Field, value: &[u8]) {
            self.0.insert(field.name().to_string(), value.into());
        }
    }

    let mut visitor = SpanEventAttributesVisitor(BTreeMap::new());
    values.record(&mut visitor);

    visitor.0
}

/// Executes a closure with the current span reference allowing
/// direct access to the span's extensions.
///
/// # Returns
///
/// Should always return `Some(R)`. None means thread local subscriber missing,
/// which should not happen in our case.
pub(super) fn with_current_span<F, R>(mut f: F) -> Option<R>
where
    F: FnMut(SpanRef<Registry>) -> R,
{
    tracing::dispatcher::get_default(|dispatch| {
        // If the dispatcher is not a `Registry`, means tracing
        // wasn't initialized and so this is a no-op.
        let registry = dispatch.downcast_ref::<Registry>()?;

        let span_ref = registry
            // No current span? Silently ignore.
            .span(dispatch.current_span().id()?)
            .expect("Must be an existing span reference");

        Some(f(span_ref))
    })
}

/// Executes a closure with the span reference from the given span allowing
/// direct access to the span's extensions.
///
/// # Returns
///
/// Should always return `Some(R)`. None means thread local subscriber missing,
/// which should not happen in our case.
pub fn with_span<F, R>(span: &Span, f: F) -> Option<R>
where
    F: FnOnce(SpanRef<Registry>) -> R,
{
    span.with_subscriber(|(span_id, dispatch)| {
        // If the dispatcher is not a `Registry`, means tracing
        // wasn't initialized and so this is a no-op.
        let registry = dispatch.downcast_ref::<Registry>()?;

        let span_ref = registry
            // Disabled span? Silently ignore.
            .span(span_id)
            .expect("Must be an existing span reference");

        Some(f(span_ref))
    })
    .flatten()
}

pub fn get_root_span_ref(cur_span: SpanRef<Registry>) -> SpanRef<Registry> {
    cur_span.scope().from_root().next().unwrap_or(cur_span)
}

pub fn with_root_span<F, R>(mut f: F) -> Option<R>
where
    F: FnMut(SpanRef<Registry>) -> R,
{
    with_current_span(|cur_span| f(get_root_span_ref(cur_span)))
}

fn record_span_status_on_ref(span_ext_mut: &mut ExtensionsMut<'_>, error_message: Option<&str>) {
    span_ext_mut.replace(
        error_message
            .map(SpanStatus::failed)
            .unwrap_or_else(SpanStatus::succeeded),
    );
}

/// Records the status of a span. If `error_message` is `None`, the
/// status code will be set to `Ok`, otherwise it will be set to `Error`.
pub fn record_span_status(span: &Span, error_message: Option<&str>) {
    with_span(span, |span_ref| {
        record_span_status_on_ref(&mut span_ref.extensions_mut(), error_message)
    });
}

/// Extension trait to record span status on `Result` types, while
/// passing through the original result.
pub trait SpanStatusRecorder {
    /// Records the status of the given span based on the `Result`.
    /// If the result is `Ok`, the status code will be set to `Ok`,
    /// otherwise it will be set to `Error` with the error message.
    ///
    /// Returns the original result.
    fn record_status(self, span: &Span) -> Self;
}

impl<T, E> SpanStatusRecorder for Result<T, E>
where
    E: std::fmt::Display,
{
    fn record_status(self, span: &Span) -> Result<T, E> {
        match self {
            Ok(_) => record_span_status(span, None),
            Err(ref e) => record_span_status(span, Some(&e.to_string())),
        };

        self
    }
}

/// Updates attributes of the given span.
///
/// The `attrs_updater` closure receives a mutable reference to the current
/// attributes and should modify them in place.
///
/// If the span attirbutes type doesn't match the expected type `A`, this is a no-op.
///
/// # Panics
///
/// If telemetry hasn't been properly initialized.
pub fn update_span_attrs<F, A: AnyTelemetryEvent>(span: &Span, attrs_updater: F)
where
    F: FnOnce(&mut A),
{
    with_span(span, |span_ref| {
        let mut span_ext_mut = span_ref.extensions_mut();

        if let Some(attrs) = span_ext_mut
            // Get the current attributes
            .get_mut::<TelemetryAttributes>()
            .expect("Telemetry hasn't been properly initialized. Missing span event attributes")
            // Try downcasting to the expected type
            .downcast_mut::<A>()
        {
            // Found the expected attributes, call the updater and return
            attrs_updater(attrs);
        }
    });
}

/// Updates attributes for the closest span with the expected
/// `TelemetryAttributes` type starting from the current span and going up.
///
/// The `attrs_updater` closure receives a mutable reference to the current
/// attributes and should modify them in place.
///
/// If no such span is found, this is a no-op.
pub fn find_and_update_span_attrs<F, A: AnyTelemetryEvent>(mut attrs_updater: F)
where
    F: FnMut(&mut A),
{
    with_current_span(|span_ref| {
        // Find the closest span with the expected TelemetryAttributes type.
        // Scope iterator starts from the current span and goes up to the root.
        for span_ref in span_ref.scope() {
            let mut span_ext_mut = span_ref.extensions_mut();

            if let Some(attrs) = span_ext_mut
                // Get the current attributes
                .get_mut::<TelemetryAttributes>()
                .expect("Telemetry hasn't been properly initialized. Missing span event attributes")
                // Try downcasting to the expected type
                .downcast_mut::<A>()
            {
                // Found the expected attributes, call the updater and return
                attrs_updater(attrs);

                return;
            }
        }
    });
}

/// Records the status of a span for the closest span with the expected
/// `TelemetryAttributes` type starting from the current span and going up.
///
/// If no such span is found, this is a no-op.
pub fn find_and_record_span_status<A: AnyTelemetryEvent>(error_message: Option<&str>) {
    with_current_span(|span_ref| {
        // Find the closest span with the expected TelemetryAttributes type.
        // Scope iterator starts from the current span and goes up to the root.
        for span_ref in span_ref.scope() {
            let mut span_ext_mut = span_ref.extensions_mut();

            if span_ext_mut
                // Get the current attributes
                .get_mut::<TelemetryAttributes>()
                .expect("Telemetry hasn't been properly initialized. Missing span event attributes")
                // Try downcasting to the expected type
                .is::<A>()
            {
                // Found the expected attributes, call the updater and return
                record_span_status_on_ref(&mut span_ext_mut, error_message);
                return;
            }
        }
    });
}

/// Records the status and attributes of the given span.
///
/// If `error_message` is `None`, the status code will be set to `Ok`,
/// otherwise it will be set to `Error`.
///
/// The `attrs_updater` closure receives a mutable reference to the current
/// attributes and should modify them in place.
pub fn record_span_status_with_attrs<F>(span: &Span, attrs_updater: F, error_message: Option<&str>)
where
    F: FnOnce(&mut TelemetryAttributes),
{
    with_span(span, |span_ref| {
        let mut span_ext_mut = span_ref.extensions_mut();

        // Record the status of the span
        record_span_status_on_ref(&mut span_ext_mut, error_message);

        // Get the current attributes, and update or replace them
        let attrs = span_ext_mut
            .get_mut::<TelemetryAttributes>()
            .expect("Telemetry hasn't been properly initialized. Missing span event attributes");
        attrs_updater(attrs);
    });
}

/// Records the status and attributes for the closest span with the expected
/// `TelemetryAttributes` type starting from the current span and going up.
///
/// If `error_message` is `None`, the status code will be set to `Ok`,
/// otherwise it will be set to `Error`.
///
/// The `attrs_updater` closure receives a mutable reference to the current
/// attributes and should modify them in place.
///
/// If no such span is found, this is a no-op.
pub fn find_and_record_span_status_with_attrs<F, A: AnyTelemetryEvent>(
    mut attrs_updater: F,
    error_message: Option<&str>,
) where
    F: FnMut(&mut A),
{
    with_current_span(|span_ref| {
        // Find the closest span with the expected TelemetryAttributes type.
        // Scope iterator starts from the current span and goes up to the root.
        for span_ref in span_ref.scope() {
            let mut span_ext_mut = span_ref.extensions_mut();

            if let Some(attrs) = span_ext_mut
                // Get the current attributes
                .get_mut::<TelemetryAttributes>()
                .expect("Telemetry hasn't been properly initialized. Missing span event attributes")
                // Try downcasting to the expected type
                .downcast_mut::<A>()
            {
                // Found the expected attributes, call the updater and return
                attrs_updater(attrs);

                // Record the status of the span
                record_span_status_on_ref(&mut span_ext_mut, error_message);

                return;
            }
        }
    });
}

/// Records the status and attributes of the given span.
///
/// Uses event `get_span_status` method to determine the status. If the event
/// doesn't support inferring status, use `record_span_status_with_attrs` instead.
///
/// The `attrs_updater` closure receives a mutable reference to the current
/// attributes and should modify them in place.
pub fn record_span_status_from_attrs<F>(span: &Span, attrs_updater: F)
where
    F: FnOnce(&mut TelemetryAttributes),
{
    with_span(span, |span_ref| {
        let mut span_ext_mut = span_ref.extensions_mut();

        // Get the current attributes, and update or replace them
        let attrs = span_ext_mut
            .get_mut::<TelemetryAttributes>()
            .expect("Telemetry hasn't been properly initialized. Missing span event attributes");
        attrs_updater(attrs);

        // Record the status of the span from the attrs themselves
        if let Some(status) = attrs.get_span_status() {
            span_ext_mut.replace(status);
        }
    });
}

/// Records the status and attributes of the current span.
///
/// Uses event `get_span_status` method to determine the status. If the event
/// doesn't support inferring status, use `record_span_status_with_attrs` instead.
///
/// The `attrs_updater` closure receives a mutable reference to the current
/// attributes and should modify them in place.
pub fn record_current_span_status_from_attrs<F>(mut attrs_updater: F)
where
    F: FnMut(&mut TelemetryAttributes),
{
    with_current_span(|span_ref| {
        let mut span_ext_mut = span_ref.extensions_mut();

        // Get the current attributes, and update or replace them
        let attrs = span_ext_mut
            .get_mut::<TelemetryAttributes>()
            .expect("Telemetry hasn't been properly initialized. Missing span event attributes");
        attrs_updater(attrs);

        // Record the status of the span from the attrs themselves
        if let Some(status) = attrs.get_span_status() {
            span_ext_mut.replace(status);
        }
    });
}

/// Records the status and attributes for the closest span with the expected
/// `TelemetryAttributes` type starting from the current span and going up.
///
/// Uses event `get_span_status` method to determine the status. If the event
/// doesn't support inferring status, use `record_span_status_with_attrs` instead.
///
/// The `attrs_updater` closure receives a mutable reference to the current
/// attributes and should modify them in place.
///
/// If no such span is found, this is a no-op.
pub fn find_and_record_span_status_from_attrs<F, A: AnyTelemetryEvent>(attrs_updater: F)
where
    F: FnOnce(&mut A),
{
    let mut attrs_updater = Some(attrs_updater);
    with_current_span(move |span_ref| {
        // Find the closest span with the expected TelemetryAttributes type.
        // Scope iterator starts from the current span and goes up to the root.
        for span_ref in span_ref.scope() {
            let mut span_ext_mut = span_ref.extensions_mut();

            if let Some(attrs) = span_ext_mut
                // Get the current attributes
                .get_mut::<TelemetryAttributes>()
                .expect("Telemetry hasn't been properly initialized. Missing span event attributes")
                // Try downcasting to the expected type
                .downcast_mut::<A>()
            {
                // Found the expected attributes, call the updater and return
                attrs_updater
                    .take()
                    .expect("attrs_updater should only be called once")(attrs);

                // Record the status of the span from the attrs themselves
                if let Some(status) = attrs.get_span_status() {
                    span_ext_mut.replace(status);
                }

                return;
            }
        }
    });
}

/// Reads span start info from the given span with read-only access.
///
/// This provides immutable access to the span's start information including
/// trace_id, span_id, span_name, and other metadata.
///
/// Returns `None` if span is disabled.
///
/// # Panics
///
/// If the span doesn't have start info (i.e., the span wasn't
/// properly initialized or telemetry hasn't been set up).
///
/// # Example
///
/// ```ignore
/// read_span_start_info(&span, |info| {
///     println!("Trace ID: {:?}", info.trace_id);
///     println!("Span ID: {}", info.span_id);
///     info.span_id
/// })
/// ```
pub fn read_span_start_info<R>(span: &Span, reader: impl FnOnce(&SpanStartInfo) -> R) -> Option<R> {
    with_span(span, |span_ref| {
        let span_ext = span_ref.extensions();
        let info = span_ext
            .get::<DLSpanStartInfo>()
            .expect("Telemetry hasn't been properly initialized. Missing span start info");

        reader(info)
    })
}

/// Reads span start info from the current span with read-only access.
///
/// This provides immutable access to the current span's start information
/// including trace_id, span_id, span_name, and other metadata. The data layer
/// guarantees that this information cannot be modified through this API.
///
/// Returns `None` if there is no current span or the span doesn't have start
/// info (e.g., the span wasn't properly initialized or telemetry hasn't been
/// set up).
///
/// # Example
///
/// ```ignore
/// read_current_span_start_info(|info| {
///     println!("Trace ID: {:?}", info.trace_id);
///     println!("Span ID: {}", info.span_id);
///     info.span_id
/// })
/// ```
pub fn read_current_span_start_info<R>(mut reader: impl FnMut(&SpanStartInfo) -> R) -> Option<R> {
    with_current_span(|span_ref| {
        let span_ext = span_ref.extensions();
        let info = span_ext
            .get::<DLSpanStartInfo>()
            .expect("Telemetry hasn't been properly initialized. Missing span start info");

        reader(info)
    })
}

/// Reads attributes from a span with the expected `TelemetryAttributes` type.
///
/// This provides read-only access to span attributes that have been downcast
/// to the specific event type. Returns `None` if the span doesn't have the
/// expected attributes.
///
/// # Example
///
/// ```ignore
/// read_span_attrs::<MyEvent, _>(&span, |attrs| {
///     println!("Event name: {}", attrs.name);
///     attrs.some_field.clone()
/// })
/// ```
pub fn read_span_attrs<A: AnyTelemetryEvent, R>(
    span: &Span,
    reader: impl FnOnce(&A) -> R,
) -> Option<R> {
    with_span(span, |span_ref| {
        let span_ext = span_ref.extensions();

        span_ext
            .get::<TelemetryAttributes>()
            .expect("Telemetry hasn't been properly initialized. Missing span event attributes")
            .downcast_ref::<A>()
            .map(reader)
    })
    .flatten()
}
