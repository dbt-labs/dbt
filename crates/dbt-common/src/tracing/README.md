# dbt-common::tracing

This module integrates the generic `dbt-tracing` library with dbt/Fusion
runtime behavior. It owns `FsTraceConfig`, dbt tracing initialization, dbt fallback
attributes, process/root span attributes, CLI layer assembly, user-facing
formatters, dbt-specific middlewares, and convenience emit helpers.

Read `fs/sa/crates/dbt-tracing/README.md` first for the full generic
architecture: records, data layer, middleware/consumer traits, filters,
serialization, and generic output layers. This README focuses on the dbt/Fusion
integration built on top of that crate.

Generic telemetry records, middleware/consumer traits, filters, data providers,
JSONL/OTLP/Parquet/pretty layers, and Arrow/OTLP serialization live in
`fs/sa/crates/dbt-tracing/`.

Structured dbt event schemas and the public registry live in
`fs/sa/crates/dbt-telemetry/`. Private event schemas and the private-aware
registry live in `crates/dbt-telemetry-private/`.

## Architecture Boundary

```text
dbt code
  dbt_common::{create_info_span, create_root_info_span}
  dbt_common::tracing::dbt_emit::*
        |
        v
dbt-common::tracing
  - FsTraceConfig and shared dbt tracing assembly
  - dbt_data_layer_config callbacks
  - dbt-specific middlewares and user-facing layers
  - formatter families for console, file, JSON compat, and query logs
        |
        v
dbt-tracing
  - TelemetryDataLayer
  - generic records, middleware/consumer traits, filters, DataProvider
  - JSONL, Parquet, OTLP, and pretty writer layers
        |
        v
dbt-telemetry / dbt-telemetry-private
  - concrete event schemas, registries, and Arrow attributes
```

## dbt Data Layer Configuration

`dbt_data_layer_config` in `dbt_data_layer.rs` configures
`TelemetryDataLayer` with dbt-specific callbacks:

- Unstructured TRACE spans become `CallTrace` when debug attributes are
  available.
- Other unstructured spans become `Unknown`.
- Unstructured logs become `LogMessage`.
- Root trace context is extracted from `Invocation`.
- Process span attributes come from `create_process_event_data` through
  `dbt_process_span_attributes`.

`FsTraceConfig::init` assembles the CLI layers and passes their explicit inputs
to the config-independent initializer in `dbt_init.rs`. The initializer builds
`TelemetryDataLayer`, opens the process span, and returns a `TelemetryHandle`
for graceful shutdown. Other applications can reuse the middleware and
file-output assembly functions without constructing `FsTraceConfig`.

## Layer Assembly

`FsTraceConfig::build_layers` assembles generic `dbt-tracing` consumers with
dbt-specific consumers.

Generic layers from `dbt-tracing`:

- JSONL file and stdout output from `src/layers/jsonl_writer.rs`.
- Parquet output from `src/layers/parquet_writer.rs`.
- OTLP export from `src/layers/otlp.rs`.

dbt-only layers in this module:

- `layers/tui_layer.rs` for default/text terminal output.
- `layers/file_log_layer.rs` for unstructured `dbt.log`.
- `layers/json_compat_layer.rs` for legacy-compatible JSON logs.
- `layers/query_log.rs` for `query_log.sql`.

JSONL and OTLP outputs use `dbt_log_preprocessor_hook` from `config.rs` to strip
ANSI sequences from `LogMessage` bodies before structured export. The generic
writers accept that hook but do not know what a dbt `LogMessage` is.

## Middlewares

Middleware order is intentional and is defined in `FsTraceConfig::build_layers`:

1. `TelemetryMarkdownLogFilter` downgrades markdown file errors first.
2. `TelemetryParsingErrorFilter` filters repeated parsing/deprecation errors.
3. `TelemetryWarnErrorOptionsMiddleware` applies warn/error options, including
   warning upgrades or silencing.
4. `TelemetryNodeWarnOutcome` marks node spans with warning outcomes after warn
   transforms have settled.
5. `TelemetryMetricAggregator` runs last so invocation metrics see the final
   severity and outcome state.

## Formatters

User-facing CLI and file rendering belongs in `formatters/`, not in
`dbt-tracing`. Formatter modules cover log messages, nodes, phases, progress,
hooks, dependencies, assets, test results, query logs, layout, color, duration,
and other dbt presentation details.

Add formatting behavior here when the output is intended for dbt users. Add
generic rendering behavior to `dbt-tracing` only when it can stay independent of
dbt event schemas and CLI conventions.

## Emitting From dbt Code

Use the re-exported structured span helpers from `dbt_common`:

```rust
use dbt_common::{create_info_span, create_root_info_span};
use dbt_telemetry::{Invocation, PhaseExecuted};

let root = create_root_info_span(Invocation {
    invocation_id: invocation_id.to_string(),
    parent_span_id: None,
    ..Default::default()
});

let _root_guard = root.enter();

let phase = create_info_span(PhaseExecuted::start_general(phase)).entered();
```

Use `dbt_common::tracing::dbt_emit` for common dbt log helpers:

```rust
use dbt_common::tracing::dbt_emit::{
    emit_error_log_from_fs_error, emit_info_log_message, emit_warn_log_message,
};

emit_info_log_message("Parsing project");
emit_warn_log_message(code, "Deprecated config");
emit_error_log_from_fs_error(error);
```

`dbt_emit.rs` also contains helpers for package-scoped messages, strict parse
errors, progress messages, and stdout/stderr-oriented messages. Prefer these
helpers when emitting user-facing dbt logs so locations, error codes, and
middleware expectations stay consistent. Error and warning helpers that receive
an `FsError` retain a borrowed-only error view for in-process consumers while
delegating serialized and user-facing output to the corresponding `LogMessage`.

For direct structured events, use the generic helpers re-exported through
`dbt_common::tracing::emit` only when there is no dbt-specific convenience
helper.

## Local Debugging And Exporters

Start local Jaeger for trace visualization:

```bash
cargo xtask telemetry
OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4318" cargo run -p dbt-cli -- --export-to-otlp <your-dbt-commands>
cargo xtask telemetry --stop
```

Open Jaeger at `http://localhost:16686`.

CLI and file outputs are controlled by `IoArgs` and resolved into
`FsTraceConfig`:

- Default interactive output: `--log-format default`, rendered by `tui_layer`.
- Non-interactive text output: `--log-format text`, rendered by `tui_layer`.
- Legacy-compatible JSON output: `--log-format json`, rendered by
  `json_compat_layer`.
- OTEL JSONL stdout: `--log-format otel`, rendered by the generic JSONL writer
  with dbt log preprocessing.
- JSONL file export: `--otel-file-name`, written under the resolved log path.
- Parquet file export: `--otel-parquet-file-name`, written under
  `{target_path}/metadata/`.
- OTLP export: `--export-to-otlp`, sent to the endpoint configured by
  `OTEL_EXPORTER_OTLP_ENDPOINT`.
- Unstructured file log: `dbt.log`, enabled by file log verbosity and written
  under the resolved log path.
- Query log: `query_log.sql`, enabled by query logging and written under the
  resolved log path.

Use `--log-level trace` for developer trace spans. In debug builds, native
TRACE spans without explicit structured attributes can become `CallTrace`
records with captured debug fields. `RUST_LOG` module filtering is only useful
in debug builds; prefer `--log-level` for release builds.

## Changing Or Testing This Module

Layer and middleware tests live under
`fs/sa/crates/dbt-common/src/tracing/tests/`.

The CLI telemetry snapshot test lives at
`crates/dbt-cli/tests/otel/telemetry_snapshot.rs`. Do not add new telemetry
snapshot tests unless explicitly asked.

Useful verification command:

```bash
cargo xtask check-llm -p dbt-common
```
