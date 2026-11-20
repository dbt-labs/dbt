# Experimental Jinja render trace

`dbt compile --feature write_render_trace` writes versioned metadata epochs at
`target/private/metadata/compile/render_traces/v1_<epoch>.parquet`. Each epoch
contains one row per rendered SQL node, including rendered dependencies, with the exact Jinja input,
the rendered SQL before ephemeral CTE injection, and ordered UTF-8 byte ranges
observed during the real render. Python models and disabled nodes do not produce
rows.

Epochs use the same append-and-compact lifecycle as other compile metadata.
The row with the greatest `ingested_at` timestamp for a `unique_id` is
authoritative. Equal timestamps prefer the later epoch, then the later row. Selected compiles retain traces for live, unselected nodes;
deletion pruning uses the complete `parse/alive.parquet` snapshot, not the
partially loaded resolver state. If liveness metadata is absent or metadata
writing is disabled, deletion pruning is skipped. Unreadable liveness metadata
leaves the trace epochs unchanged and emits a warning.
A selected node that enters traced Jinja rendering but
fails or produces no trace removes its prior row during compaction. Failures
before traced rendering begins do not update the epoch sequence. A selected Python
model invalidates its retained SQL trace. Unit-test override renders do not replace
canonical model traces.

For the public latest-row table, run:

```sh
dbt compile --generate-info-schema --feature write_render_trace
```

Consumers read `target/info_schema/v1/dbt.render_traces.parquet`.
`--target-path` relocates the target root; `--info-schema-dir` overrides the public
output directory. The feature alone records private epochs. Without the feature,
information-schema generation emits an empty render-traces table, even when
private epochs remain from an earlier invocation. Public snapshots are rewritten
as a whole; private epochs retain the incremental lifecycle.

This is an experimental protocol. Consumers must check `protocol_version`,
`invocation_id`, `unique_id`, `source`, and `render_stage` before using offsets.
Trace requests bypass the compiled-SQL cache because that cache does not retain
render events.

## Protocol version 2

Identity fields describe the producer and render:

- `dbt_version`, `invocation_id`, `ingested_at`, and `unique_id`
- `filename`, used for Jinja locations, and `source_path`, the rendered file
- `render_stage`, currently `jinja_before_ephemeral_injection`
- exact `source` and `rendered` strings

Each trace row has an ordered `events` list of structs:

```json
{
  "events": [
    {"kind":"literal","source_start":0,"source_end":7,"rendered_start":0,"rendered_end":7},
    {"kind":"variable","source_start":7,"source_end":16,"rendered_start":7,"rendered_end":7},
    {"kind":"templated","source_start":7,"source_end":16,"rendered_start":7,"rendered_end":8}
  ]
}
```

The Parquet type is `LIST<STRUCT<kind, source_start, source_end,
rendered_start, rendered_end>>`. Parquet stores those struct fields columnarly
without repeating JSON keys, while the list preserves execution order without a
separate sequence value. Source and rendered offsets form half-open UTF-8 byte
ranges.

Event kinds include `literal`, `templated`, `macro`, `block_start`,
`block_mid`, `block_end`, `block_standalone`, `variable`, `comment`,
`loop_iteration`, and `loop_skipped`. Layout events can have zero-width output
ranges. Loop execution can repeat source ranges and move source offsets
backwards; consumers must not sort or deduplicate event entries.

Foreign-template output has no editable range in the current source. Captured,
filtered, returned, included, inherited, and recursive output is represented by
its eventual emission rather than by intermediate buffers. Consumers should
validate every range and literal equality instead of inferring provenance by
matching output text.

## Covered Jinja patterns

The integration fixtures exercise common dbt model constructs together:

- multiline `config()` calls and `target` access;
- `ref()`, `source()`, `this`, `var()`, and `is_incremental()`;
- adapter dispatch and adapter quoting;
- macros with explicit returns;
- list-driven column generation, whitespace control, loop metadata, comments,
  and conditional SQL;
- ephemeral dependency injection after the traced render.

Renderer regressions separately cover if/elif/else paths, filtered and recursive
loops, break/continue, raw blocks, captures, named blocks, inheritance,
includes, `super()`, explicit returns, Unicode, and CRLF input.

## Semantic goldens

The readable `goldens/render_traces.json` fixture checks normalized source,
rendered SQL and event semantics. Rust schema tests and CLI integration tests
check the actual Parquet types separately. With this branch's Python wheel installed in the active virtual environment, regenerate it with:

```sh
uv run --no-project --active --with duckdb python crates/dbt-sa-python/tests/render_trace_goldens.py
```

The script compiles an isolated copy of the committed project through the real
exporter, normalizes invocation/version/path fields, and writes the JSON fixture.
Review the resulting diff before committing it.
