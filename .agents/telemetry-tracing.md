# Telemetry / Tracing

Paths in this file are relative to the directory containing `AGENTS.md`.

## Start Here

Read this file first for tracing or telemetry work. Then read the README or
AGENTS file for the layer you are changing.

## Decision Tree

- Generic tracing infrastructure: read
  `crates/dbt-tracing/AGENTS.md` and
  `crates/dbt-tracing/README.md`.
- dbt CLI output, log formatting, middleware behavior, layer assembly, or dbt
  fallback attributes: read
  `crates/dbt-common/src/tracing/README.md`.
- Structured event schemas, the event registry, or Arrow attributes:
  read `crates/dbt-telemetry/README.md`.
- Anonymous product-usage telemetry through Vortex: work in
  `crates/vortex-client/`, `crates/vortex-events/`, or
  `crates/proto-rust/` as appropriate. Do not treat Vortex work as
  `dbt-tracing` work.

## Architecture Boundary

- `dbt-tracing` is the generic structured tracing library. It owns the data
  layer, record envelopes, emit APIs, middleware/consumer traits, filters,
  generic output layers, Arrow serialization, and OTLP serialization.
- `dbt-telemetry` owns event schemas, helper impls, Arrow attribute shapes, and
  the event registry.
- `dbt-common::tracing` owns dbt runtime integration: `FsTraceConfig`,
  `init_tracing`, dbt fallback callbacks, user-facing output, dbt-only layers,
  middlewares, and formatters.
- Vortex anonymous telemetry is separate from this structured tracing/export
  stack.

## Common Workflows

### Add An Event

1. Edit a proto under
   `crates/dbt-telemetry/include/dbtlabs/proto/public/v1/events/fusion/`.
2. Regenerate the Rust bindings. Generated code is checked in; regeneration
   runs through the workspace proto codegen task.
3. Add or update schema implementations in
   `crates/dbt-telemetry/src/schemas/`.
4. Add optional helpers in `crates/dbt-telemetry/src/impls/` only when
   useful to callsites.
5. Register first-class events in
   `crates/dbt-telemetry/src/attributes/registry.rs`.
6. Update `crates/dbt-telemetry/src/serialize/arrow.rs` only for
   intentionally well-known Parquet columns.
7. Update registry coverage and Arrow roundtrip tests.
8. Run scoped checks/tests for `dbt-telemetry`.

### Change CLI Or User-Facing Formatting

Work in `crates/dbt-common/src/tracing/formatters/` and the relevant
dbt-only layer under `crates/dbt-common/src/tracing/layers/`. Middleware
behavior lives under `crates/dbt-common/src/tracing/middlewares/`.

Layer and middleware tests live under
`crates/dbt-common/src/tracing/tests/`.

### Add Or Change Generic Output Behavior

Work in `crates/dbt-tracing/src/layers/` for JSONL, Parquet, OTLP, and
pretty writer behavior. Work in `crates/dbt-tracing/src/serialize/` for
Arrow, JSON envelope, or OTLP serialization.

Keep generic layers independent of dbt event names, dbt CLI behavior, and
Vortex.

### Debug Local Traces With Jaeger

```bash
docker run -d --rm --name jaeger -p 4318:4318 -p 16686:16686 \
  cr.jaegertracing.io/jaegertracing/jaeger:2.10.0
OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4318" \
  cargo run -p dbt-sa-cli -- --export-to-otlp <your-dbt-commands>
docker stop jaeger
```

Open Jaeger at `http://localhost:16686`.

### Handle Async Or Thread Span Propagation

When work crosses async, task, or thread boundaries, preserve the current span
with `tracing::Instrument`, `Span::current().enter()`, or helpers in
`crates/dbt-tracing/src/async_tracing.rs`.

## Useful Paths

- Protos:
  `crates/dbt-telemetry/include/dbtlabs/proto/public/v1/events/fusion/`
- Generic emit helpers: `crates/dbt-tracing/src/emit.rs`
- dbt emit conveniences: `crates/dbt-common/src/tracing/dbt_emit.rs`
- Generic data layer: `crates/dbt-tracing/src/layers/data_layer.rs`
- Generic middleware/consumer traits: `crates/dbt-tracing/src/layer.rs`
- dbt layer assembly: `crates/dbt-common/src/tracing/config.rs`
- dbt integration tests: `crates/dbt-common/src/tracing/tests/`

## Telemetry Test Sweep

When a change may affect tracing infrastructure end to end, run the scoped
telemetry test sweep:

```bash
cargo nextest run \
  -E 'package(dbt-tracing) | package(dbt-common) | package(dbt-telemetry)'
```
