# dbt-tracing Agent Guide

## Scope

This crate must stay generic and reusable. Do not add dbt event schemas, CLI
configuration, Fusion formatters, dbt fallback event types, or anonymous
product-usage telemetry here.

## Where Work Belongs

- Generic data layer, record envelopes, emit APIs, consumers, generic output
  layers, Arrow serialization, registry-backed JSON deserialization, and OTLP
  serialization belong in this crate.
- dbt-specific config, layer assembly, formatters, middlewares, and fallback
  event selection belong in `dbt-common::tracing`.
- Public and private dbt/Fusion event schemas, helper constructors, Arrow
  attribute shapes, and event registries belong in the telemetry schema crates.
- Anonymous telemetry and Vortex work belongs in `.agents/vortex-anonymous-telemetry.md`
  and the Vortex-related crates.

## Implementation Rules

- Prefer callbacks or traits at the boundary instead of hardcoding downstream
  event types.
- Keep output layers generic. Downstream crates provide registries, concrete
  Arrow schemas, JSON event payload deserializers, and optional log
  preprocessors.
- Keep examples and tests based on mock or generic event types.
- Preserve the distinction between structured tracing/export and anonymous
  product analytics.

## Validation

```bash
cargo xtask check-llm -p dbt-tracing
cargo xtask test --llm --no-external-deps -p dbt-tracing
```
