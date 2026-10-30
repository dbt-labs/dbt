# Experimental model render provenance

The internal `write_render_trace` feature exports observations from an actual
SQL model Jinja render for development of external source-mapping consumers:

```shell
dbt compile --select path:models/example.sql --feature write_render_trace
```

This is an experimental interface for evaluating an upstream contribution, not
a stable public artifact contract. It is disabled by default and uses the flag
registry's invocation-level CLI/environment handling. It is not a project
behavior flag and is not read from a project's `flags:` block.

Each rendered SQL node writes a `*.render_trace.json` sidecar next to its
compiled SQL, under the normal target directory. It contains:

- `protocol_version`: `1` for this experimental format.
- `dbt_version`, `invocation_id`, and `unique_id`: producer and model identity.
- `filename`: the path used for Jinja source locations.
- `source_path`: the actual file used as render input. This may differ from
  `filename`, particularly for generated snapshot SQL.
- `render_stage`: `jinja_before_ephemeral_injection`.
- `source`, `rendered`: the exact strings supplied to and returned by Jinja.
- `events`: observations in execution order, each with a `kind`,
  `source_start`, `source_end`, `rendered_start`, and `rendered_end`.

Offsets are zero-based UTF-8 bytes and end offsets are exclusive. Layout and
loop events may have zero-width rendered ranges. Repeated loop iterations may
refer to the same source span with different output spans. Macro boundary events
can enclose other events; the event list is not a partition of either string.
Consumers must validate ranges and must not infer missing provenance by matching
identical output strings. The initial event kinds are `literal`, `templated`,
`macro`, `block_start`, `block_mid`, `block_end`, `block_standalone`, `variable`,
`comment`, `loop_iteration`, and `loop_skipped`.

Layout events describe source tags, not synthetic AST nodes. An if/elif chain
emits one block_end for its shared endif per execution. Internal macro-span
events can still enclose synthetic conditional scopes; they do not introduce
additional Jinja layout blocks.

Events can legitimately repeat when a source tag executes again in a loop,
including with identical source and rendered offsets if the body emits no text.
Consumers must not globally deduplicate events by their byte ranges.

Raw-block literal ranges cover the post-whitespace-control payload, not the
surrounding stripped whitespace. Loop repetitions describe executions of the
source body; a filtered loop's internal selection pass emits no repetitions.
A for/else closes either each executed body or the executed else, never both.

Named blocks render into temporary buffers. Their events are rebased when the
buffer is appended directly, and the enclosing tracker is restored even on an
error. Captured or transformed block values do not publish intermediate literal
spans; their final expression output remains generated. Buffer callbacks are
internal observer hooks and do not add event kinds or change protocol version 1.

The `rendered` string is deliberately the Jinja result before ephemeral CTE
injection. These offsets must not be applied directly to the final compiled SQL
artifact. Generated snapshot inputs do not yet have a mapping back to the
original snapshot definition; `source_path` identifies that distinction. Python
models and alternate/unexecuted branch renders are outside this first version.

Trace requests bypass the compiled-SQL render cache because it does not store
events. Normal renders preserve the existing cache path and do not create a
trace observer. Tracing shares the VM's chosen output tracker, so it observes
the active factory's output positions without replacing that factory or taking
ownership of its tracker. The callback has a default no-op implementation for
existing listeners.

Consumers should only use traces from a successful invocation, check the
invocation/model identity, and compare the source with the file being linted.
Old sidecars can remain in a reused target directory when tracing is disabled;
the existence of a file alone does not establish freshness.

Disabled nodes are not rendered and emit no trace or stub. After a successful
compile, consumers can find them in the same invocation's manifest.disabled
mapping: each value is a list of node definitions with original_file_path and
config.enabled set to false. Match the full source path, not just a basename.
Only that explicit disabled status justifies skipping an absent trace; missing
output for enabled or unknown nodes remains an error. A failed invocation is
not evidence that a model is disabled.

A future cache integration can persist provenance with the corresponding
compilation result.

The observer's unit tests compare normal/traced SQL and existing macro spans,
and exercise ambiguous branches, repeated/empty loops, captures, macros,
Unicode, newline handling, whitespace control and a non-default tracker setup.
Before treating this as stable, agree the schema, validate warehouse-dependent
macros and both shipped distributions, measure overhead, and extend the cache
and source-rewriting cases as needed.

## Executable format example

See the six-line [SQL/Jinja input](../../crates/dbt-tasks-sa/tests/fixtures/render_trace/example.sql)
and its [complete JSON trace](../../crates/dbt-tasks-sa/tests/fixtures/render_trace/example.render_trace.json).
They demonstrate UTF-8 source offsets, literal and generated output, an inline
macro, repeated loop spans, a conditional separator and a skipped loop. The
macro-generated values are mapped to the calling `{{ doubled(n) }}` expressions
as `templated` output.

The Rust `render_trace_example_matches_fixture` test renders that input and
compares the entire artifact with the JSON fixture using the production artifact
type. Producer version, invocation ID and paths are fixed example values so the
fixture stays stable across releases and machines; execution events are captured
from the real renderer. The local attributes keep fixture newlines as LF because
the byte offsets describe those exact input bytes.

## Local prototype validation

Checked on Windows x86-64, Python 3.13.5 and Rust stable 1.98.1:

- After rebasing onto upstream main (`4b4701c77`), all 16 `dbt-flags` and
  all 158 `dbt-jinja-utils` library tests passed, including the four new
  provenance tests. The executable artifact example test also passed.

Before the rebase, the following integration checks passed on the same
trace implementation:

- Built and installed the `dbt-sa-python` ABI3 wheel and ran both tests in
  `tests/integration/test_render_trace.py` successfully. These exercise a real
  project, dbt `return()`, an ephemeral parent, output equality with tracing off,
  repeated source spans, macro-change freshness, and failed rendering.
- The installed console entry point emitted traces that drove SQLFluff source
  fixes and a successful, idempotent recompile. This used a temporary metadata
  adapter for the separate proof-of-concept consumer's old SHA-pinned format;
  it did not modify execution events or the committed consumer.
- Scoped Clippy completed for `dbt-flags`, `dbt-jinja-utils` and `dbt-tasks-sa`.
  Its warnings were in existing code, not the new observer or artifact writer.
- Rust formatting, Python test lint/format and diff whitespace checks passed.

This debug build required `RUST_LOG=debug` and `DBT_LOG_LEVEL=debug` to keep
upstream tracing spans enabled. Without them, the ordinary untraced Python
invocation also panicked during tracing initialization. The local test results
use that environment workaround; release-build behaviour was not tested.

These checks do not establish complete dbt/Core templater parity. Live warehouse
introspection, product-specific factories, cross-platform releases, alternate
branches and source-rewritten snapshots still need validation. The flag and
schema remain an upstream design proposal.
