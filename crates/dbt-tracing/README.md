# dbt-tracing

`dbt-tracing` is a structured telemetry layer built on top of
[`tracing`](https://docs.rs/tracing). It is a generic library for typed span and
log attributes, telemetry record envelopes, middleware, consumers, filtering,
reload/shutdown handling, and serialization/export.

This crate is not anonymous product-usage telemetry and is not a product
analytics client. It does not define an application's event taxonomy; callers
provide concrete event types, Arrow schemas, and registry lookup behavior for
dynamic attributes.

## Architecture

Applications emit typed attributes through the span and event helpers in
`src/emit.rs`. `TelemetryDataLayer` receives native `tracing` spans and events,
turns them into `SpanStartInfo`, `SpanEndInfo`, and `LogRecordInfo`, passes them
through middleware, and then sends the processed records to consumers.

```text
┌────────────────────────────────────────────────────────────────┐
│                      Application Code                          │
│     tracing::instrument, create_info_span, emit_info_event      │
└────────────────────────┬───────────────────────────────────────┘
                         │
┌────────────────────────▼───────────────────────────────────────┐
│              TelemetryDataLayer (native tracing Layer)         │
│  - Generates globally unique span/event IDs                    │
│  - Correlates records with trace ID from root trace context    │
│  - Auto-injects code location (file, line, module)             │
│  - Converts spans/events to structured telemetry records       │
│  - Stores span context, root metrics, and root extensions      │
│  - Drives middleware and consumer layers                       │
│  - Tracks per-consumer span filtering decisions                │
└────────────────────────┬───────────────────────────────────────┘
                         │
┌────────────────────────▼───────────────────────────────────────┐
│                  Middleware Pipeline                           │
│  - Can modify or drop spans and log records                    │
│  - Can update root metrics and extensions through DataProvider │
│  - Runs before any consumer sees the processed records         │
└────────────────────────┬───────────────────────────────────────┘
                         │
┌────────────────────────▼───────────────────────────────────────┐
│                    Consumer Layers                             │
│  - Read-only consumers of processed telemetry data             │
│  - Can filter spans and logs independently                     │
│  - Can access metrics and extensions through DataProvider      │
│                                                                │
│  Generic examples:                                             │
│  - TelemetryJsonlWriterLayer: JSONL output                     │
│  - TelemetryParquetWriterLayer: Arrow/Parquet output           │
│  - OTLPExporterLayer: OpenTelemetry Protocol export            │
│  - TelemetryPrettyWriterLayer: human-readable rendering        │
│  - Application-specific consumers built on TelemetryConsumer   │
└────────────────────────────────────────────────────────────────┘
```

This crate still uses `tracing` as the runtime substrate. `TelemetryDataLayer`
is the single native `tracing_subscriber::Layer` that bridges from the tracing
registry into the structured telemetry pipeline. Middleware and consumers are
plain Rust traits called by that data layer after records have been materialized.

### Why Custom Abstractions?

Native `tracing` layers are powerful, but this library needs stricter behavior
than the raw layer API provides:

1. `tracing` has span extensions, but no equivalent thread-safe storage for log
   event data.
2. Span and event facades cannot carry arbitrary typed structured attributes
   without reducing them to primitive fields.
3. Per-layer filtering does not have reliable access to the fully materialized
   span or log record.
4. Runtime layer reloading does not work with filtered layers
   ([tokio-rs/tracing#1629](https://github.com/tokio-rs/tracing/issues/1629)).
5. Direct span-extension access is easy to misuse and can self-deadlock when a
   callback takes nested extension locks.

`TelemetryMiddleware`, `TelemetryConsumer`, and `DataProvider` provide:

- Clear write-before-read ordering: middleware can mutate or drop records before
  all consumers see them; consumers should be read-only.
- Access to structured telemetry attributes for filtering and output decisions.
- Thread-safe metric tracking on the root span.
- Controlled root/current/ancestor extension access that avoids long-lived
  extension locks.
- Reloadable filtered consumer stacks for tests and runtime reconfiguration.

## Core Concepts

- `AnyTelemetryEvent`, `StaticTelemetryEvent`,
  `ArrowSerializableTelemetryEvent`, `ArrowRegistryLookup`, and
  `JsonRegistryLookup` define the object-safe and typed event boundaries used
  by records and serializers.
- `TelemetryAttributes` wraps concrete typed events for transport through the
  tracing pipeline.
- `TelemetryContext` carries inherited context such as application-specific
  phase or operation identifiers.
- `TelemetryOutputFlags` lets event types declare which generic sinks should see
  them.
- `TelemetryDataLayerConfig` supplies callbacks for fallback attributes when a
  native `tracing` span or event was not created through the structured helpers,
  plus root trace-context extraction.
- `TelemetryMiddleware` can transform or drop records before all consumers see
  them.
- `TelemetryConsumer` is a read-only sink for processed records. Consumers can
  use `with_span_filter`, `with_log_filter`, or `with_filter` for scoped output.
- `DataProvider` gives middleware and consumers controlled access to root and
  current span metrics and extensions.

## Core Components

### TelemetryDataLayer

`TelemetryDataLayer` is the bridge between native `tracing` and this library's
structured record pipeline. It:

- Converts tracing spans and events into `SpanStartInfo`, `SpanEndInfo`, and
  `LogRecordInfo`.
- Generates globally unique span and event IDs across the process.
- Uses `TelemetryDataLayerConfig` callbacks to provide fallback attributes for
  unstructured tracing calls.
- Extracts root trace context from structured root-span attributes when the
  application provides it.
- Injects inherited `TelemetryContext` and callsite code location.
- Stores span data in tracing span extensions behind private wrapper types.
- Applies middleware before dispatching records to consumers.
- Stores per-consumer filter masks so a consumer that filtered out a span start
  will not receive that span end.

### TelemetryMiddleware

Middleware can transform telemetry data before consumers see it. Each callback
returns `Option<T>`: return `None` to drop the record for all consumers.

```rust
use dbt_tracing::{
    LogRecordInfo, SpanEndInfo, SpanStartInfo,
    data_provider::DataProvider,
    layer::TelemetryMiddleware,
    metrics::MetricKey,
};

struct SpanCounter;

impl TelemetryMiddleware for SpanCounter {
    fn on_span_start(
        &self,
        span: SpanStartInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanStartInfo> {
        data_provider.increment_metric(MetricKey::from_raw(1), 1);
        Some(span)
    }

    fn on_log_record(
        &self,
        record: LogRecordInfo,
        _data_provider: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        Some(record)
    }

    fn on_span_end(
        &self,
        span: SpanEndInfo,
        _data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanEndInfo> {
        Some(span)
    }
}
```

Use middleware for behavior that should affect every consumer: redaction,
normalization, aggregation, or record dropping.

### TelemetryConsumer

Consumers are sinks for processed telemetry records. They can filter spans and
logs independently and can read root-span metrics or extensions through
`DataProvider`.

```rust
use dbt_tracing::{
    LogRecordInfo, SeverityNumber, SpanEndInfo, SpanStartInfo, TelemetryOutputFlags,
    data_provider::DataProvider,
    layer::TelemetryConsumer,
};

struct DurationLogger;

impl TelemetryConsumer for DurationLogger {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        span.attributes
            .output_flags()
            .contains(TelemetryOutputFlags::OUTPUT_CONSOLE)
    }

    fn is_log_enabled(&self, log: &LogRecordInfo) -> bool {
        log.severity_number >= SeverityNumber::Warn
    }

    fn on_span_end(&self, span: &SpanEndInfo, _data_provider: &mut DataProvider<'_>) {
        if let Ok(duration) = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
        {
            println!("span {} took {}ms", span.span_name, duration.as_millis());
        }
    }
}
```

Existing generic consumers include:

- `TelemetryJsonlWriterLayer` in `src/layers/jsonl_writer.rs`.
- `TelemetryParquetWriterLayer` in `src/layers/parquet_writer.rs`.
- `OTLPExporterLayer` in `src/layers/otlp.rs`.
- `TelemetryPrettyWriterLayer` in `src/layers/pretty_writer.rs`.

### DataProvider

`DataProvider` wraps tracing span extensions and exposes safer access patterns:

- `increment_metric`, `get_metric`, and `get_all_metrics` for root-span
  counters.
- `init_root`, `with_root`, and `with_root_mut` for application-defined root
  extensions.
- `init_cur` for current-span extensions.
- `with_ancestor_attrs`, `with_ancestor_ext`, and mutable variants for
  structured ancestor lookup.

Methods that can take mutable extension locks require `&mut self`; this is
intentional and helps avoid self-deadlocks in middleware or consumer callbacks.

### Filters

Consumers can be wrapped with closure-based filters:

```rust
use dbt_tracing::{SeverityNumber, layer::TelemetryConsumer};

let consumer = DurationLogger
    .with_span_filter(|span| span.attributes.output_flags().intersects(output_mask))
    .with_log_filter(|log| log.severity_number >= SeverityNumber::Warn);
```

For level-based filtering, `tracing::level_filters::LevelFilter` implements the
same filter trait and can be passed to `with_filter`.

### Layer Assembly

A host application typically builds one `TelemetryDataLayer` and passes it a
middleware list plus the consumer layers it wants enabled:

```rust
use dbt_tracing::{
    layer::{ConsumerLayer, MiddlewareLayer},
    layers::{
        data_layer::{TelemetryDataLayer, TelemetryDataLayerConfig},
        jsonl_writer::build_jsonl_layer_with_background_writer,
        otlp::{build_otlp_layer, OtlpResourceConfig},
        parquet_writer::build_parquet_writer_layer,
    },
};
use tracing_subscriber::{Registry, layer::SubscriberExt as _};

let (jsonl, jsonl_shutdown) = build_jsonl_layer_with_background_writer(
    std::io::stdout(),
    tracing::level_filters::LevelFilter::INFO,
    None,
);
let (parquet, parquet_shutdown) =
    build_parquet_writer_layer::<_, MyArrowRegistry>(std::fs::File::create("trace.parquet")?)?;
let otlp = build_otlp_layer(OtlpResourceConfig::new("my-service", "dev"), None);

let mut consumers: Vec<ConsumerLayer> = vec![jsonl, parquet];
let mut shutdown_items = vec![jsonl_shutdown, parquet_shutdown];
if let Some((otlp_layer, mut otlp_shutdown)) = otlp {
    consumers.push(otlp_layer);
    shutdown_items.append(&mut otlp_shutdown);
}

let middlewares: Vec<MiddlewareLayer> = vec![Box::new(SpanCounter)];
let data_layer = TelemetryDataLayer::new(
    TelemetryDataLayerConfig::new(
        fallback_trace_id(),
        None,
        unstructured_span_attributes,
        unstructured_log_attributes,
        root_span_trace_context,
    ),
    cfg!(not(debug_assertions)),
    middlewares.into_iter(),
    consumers.into_iter(),
);

let subscriber = Registry::default().with(data_layer);
```

Applications own the concrete registry, fallback callbacks, formatters,
configuration surface, file locations, shutdown handling, and any
application-specific consumers.

## Using The Library

Downstream applications define concrete span and log attribute structs that
implement the telemetry event traits. Keep those structs domain-specific in the
application crate; keep this crate generic.

```rust
use dbt_tracing::emit::{create_info_span, emit_info_event};

#[derive(Debug, Clone)]
struct RequestStarted {
    route: String,
}

// In real code, RequestStarted implements the telemetry event traits and
// therefore Into<TelemetryAttributes>.

let span = create_info_span(
    RequestStarted {
        route: "/items".to_string(),
    }
);

let _guard = span.enter();
emit_info_event(RequestStarted { route: "/items".to_string() }, None);
```

For spans, use `create_root_info_span`, `create_info_span`,
`create_debug_span`, or the explicit-parent variants from `src/emit.rs`. For log
records, use `emit_error_event`, `emit_warn_event`, `emit_info_event`,
`emit_debug_event`, or lazy `emit_trace_event`.

When work crosses async, task, or thread boundaries, propagate the current span
explicitly. Prefer the crate helpers for spawned work:

```rust
use dbt_tracing::async_tracing::spawn_traced;
use tracing::Instrument as _;

async fn handle_request() {
    let span = dbt_tracing::emit::create_info_span(request_attributes());
    do_work().instrument(span).await;
}

fn spawn_work() {
    spawn_traced(async {
        run_async_work().await;
    });

    // IMPORTANT: dbt_runtime::spawn_blocking already propagates
    // the current span, so no extra instrumentation is needed.
    dbt_runtime::spawn_blocking(|| {
        run_blocking_work();
    });

    // Equivalent standard async form:
    // tokio::spawn(run_async_work().in_current_span());
    //
    // Equivalent standard blocking/thread form:
    // let span = tracing::Span::current();
    // std::thread::spawn(move || {
    //     let _guard = span.enter();
    //     run_blocking_work();
    // });
}
```

The helpers in `src/async_tracing.rs` cover common spawn patterns and keep
current-span propagation consistent across callsites.

## Serialization And Output

Generic output layers live in `src/layers/`:

- `jsonl_writer.rs` writes telemetry records as JSONL.
- `otlp.rs` exports records through OpenTelemetry Protocol.
- `parquet_writer.rs` writes Arrow/Parquet records.
- `pretty_writer.rs` renders generic human-readable records.

`TelemetryOutputFlags` controls which consumers should receive a given event
type:

- `EXPORT_JSONL`: machine-readable JSONL writers.
- `EXPORT_PARQUET`: Arrow/Parquet writers. Event types with this flag must
  implement Arrow serialization.
- `EXPORT_OTLP`: OpenTelemetry Protocol exporters.
- `OUTPUT_CONSOLE`: human-readable console or terminal output.
- `OUTPUT_LOG_FILE`: human-readable file-style output.
- `EXPORT_JSONL_AND_OTLP`: convenience alias for JSONL plus OTLP export.
- `EXPORT_ALL`: all machine-readable export flags.
- `OUTPUT_ALL`: all human-readable output flags.
- `ALL`: every export and output flag.

Flags are declared by each event type through `StaticTelemetryEvent::OUTPUT_FLAGS`
or `AnyTelemetryEvent::output_flags()`. Generic consumers use the flags in their
`is_span_enabled` and `is_log_enabled` implementations, so an event type can be
exported to a machine-readable sink without also being rendered for humans, or
vice versa.

Arrow support lives in `src/serialize/arrow.rs`. Use
`TelemetryArrowSchemas::new::<Registry>()`, `serialize_to_arrow`, and
`deserialize_from_arrow` with a caller-provided `ArrowRegistryLookup`
implementation. The registry owns the concrete Arrow attribute type and the
mapping from event type names to deserializers. `deserialize_from_arrow` returns
`Result<(Vec<TelemetryRecord>, Vec<IndexedTelemetryDeserializeError>), _>`:
schema and batch normalization failures remain outer errors, while row-level
envelope, unknown-event-type, and malformed-event failures are returned
alongside any successful records. Each row error includes a one-based Arrow row
index.

JSON deserialization support lives in `src/serialize/json.rs`. Use
`deserialize_from_json_str`, `deserialize_from_json_value`, or
`deserialize_json_lines` with a caller-provided `JsonRegistryLookup`
implementation. Serde parses the stable record envelope fields, while the
registry maps each flattened `event_type` plus nested `attributes` payload back
to a boxed `AnyTelemetryEvent`. The registry is expected to return attributes
that match the record category implied by the JSON envelope. Mismatches indicate
an incompatible registry or event schema and are checked with debug assertions.
`deserialize_json_lines` returns
`(Vec<TelemetryRecord>, Vec<IndexedTelemetryDeserializeError>)`,
allowing valid lines to be consumed even when other lines have invalid
envelopes, unknown event types, or malformed attributes. Each line error
includes a one-based JSONL line index.

OTLP support lives in `src/serialize/otlp.rs`. JSON envelope support lives in
`src/serialize/envelope.rs`.

## Best Practices

1. Use structured attributes for spans and logs that need downstream analysis.
2. Keep concrete event schemas and application-specific formatter behavior in
   downstream crates.
3. Prefer `#[tracing::instrument]` for ordinary developer diagnostics, and use
   typed span helpers when the span is part of the structured telemetry contract.
4. Use trace-level spans for high-volume developer debugging rather than
   user-facing debug logs.
5. Preserve span context across async, task, and thread boundaries with
   `spawn_traced`, `dbt_runtime::spawn_blocking`, or `tracing::Instrument`.
6. Implement filtering in `is_span_enabled`, `is_log_enabled`, or a
   `TelemetryFilter` wrapper to avoid unnecessary work in consumers.
7. Use middleware for global transforms, dropping, scrubbing, and metric
   aggregation; keep consumers focused on output/export.
8. Use `DataProvider` instead of direct span-extension access inside telemetry
   pipeline code.

## Testing

`src/test_support.rs` is available to tests and to dependents that enable the
`test-utils` feature. Existing crate tests under `src/tests/` use mock/generic
event types and should stay free of downstream application concepts.

Useful verification commands:

```bash
cargo xtask check-llm -p dbt-tracing
cargo xtask test --llm --no-external-deps -p dbt-tracing
```
