# dbt-telemetry

`dbt-telemetry` owns the public dbt/Fusion structured telemetry event schemas.
It contains generated public protobuf event types, dbt event trait
implementations, `DbtTelemetryContext`, the concrete public `ArrowAttributes`
shape, and `TelemetryEventTypeRegistry`.

This crate is the canonical public source of truth for Fusion structured event
shape. Public protos here must remain backwards-compatible according to the
stated structured tracing guarantee because downstream consumers may compile
against or deserialize these event contracts.

This crate is not the tracing runtime, subscriber setup, or output layer
assembly. Those generic pieces live in `fs/sa/crates/dbt-tracing/`, and dbt CLI
assembly lives in `fs/sa/crates/dbt-common/src/tracing/`.

This crate is also not anonymous Vortex telemetry. Vortex-era product usage
telemetry is covered by `.agents/vortex-anonymous-telemetry.md`.

## Module Map

- `include/dbtlabs/proto/public/v1/events/fusion/`: source public Fusion
  telemetry protos owned in this repo.
- `src/gen/`: generated Rust protobuf types and descriptor data.
- `src/schemas/`: span/log trait implementations for generated event types.
- `src/impls/`: optional helper constructors and callsite conveniences.
- `src/attributes/`: dbt telemetry context and public event registry.
- `src/serialize/arrow.rs`: concrete dbt Arrow attribute storage shape and
  roundtrip tests.

## Adding Or Changing A Public Event

1. Edit the relevant public Fusion proto under
   `include/dbtlabs/proto/public/v1/events/fusion/`.
2. Run `cargo xtask protogen`.
3. Add or update the span or log schema implementation in `src/schemas/`.
4. Add helper impls in `src/impls/` only when they make callsites clearer.
5. Register first-class events in `TelemetryEventTypeRegistry`.
6. Update `ArrowAttributes` and registry Arrow fields only for intentionally
   well-known Parquet columns. Otherwise, keep event-specific data in the JSON
   payload.
7. Update registry coverage and Arrow roundtrip tests.

For schema expansions that affect Parquet compatibility, follow the regression
fixture guidance in `.agents/telemetry-tracing.md`.

## Testing

```bash
cargo xtask check-llm -p dbt-telemetry
cargo xtask test --llm --no-external-deps -p dbt-telemetry
```
