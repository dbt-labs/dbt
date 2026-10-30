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
Loop `break` and `continue` emit closing boundaries for the exited conditionals
and loop body before jumping, preserving enclosing scopes and genuine repeats.
The VM records whether an item was visited separately from its zero-based index,
so a break during the first item is not misreported as an empty loop or routed
through its else body.

Template identity follows the executing instructions through inheritance and
nested evaluations. Output from a different template is `templated` with an
empty source range at byte zero; foreign layout and loop events are omitted.
Overridden blocks belonging to this source retain their exact literal ranges.
Foreign bytes are never offered as editable literals in the child file.

Named blocks render into temporary buffers. Their events are rebased when the
buffer is appended directly, and the enclosing tracker is restored even on an
error. Captured or transformed block values do not publish intermediate literal
spans; their final expression output remains generated. Buffer callbacks are
internal observer hooks and do not add event kinds or change protocol version 1.
An explicit return replaces that evaluation's output: all events for the
discarded buffer are removed, and the returned string is generated (`templated`
with an empty source range at byte zero). Enclosing output remains mapped;
ordinary macro returns continue to be covered by their calling expression.

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

## Regression coverage

The [Python artifact regression matrix](../../crates/dbt-sa-python/tests/integration/test_render_trace_regressions.py)
compiles 28 independent examples both normally and with trace export enabled.
Every case checks identical compiled SQL, exact input/output, invocation/model
identity, valid UTF-8 boundaries, literal equality, correctly nested layout
closures, and complete rendered-output coverage. Individual cases also pin
closing-event and loop-repeat counts or the exact text eligible for literal
source mapping.

The cases cover trimmed raw blocks, Unicode/CRLF, if/elif/else paths, nested
conditionals, empty/single/multiple-iteration for/else, partially and entirely
filtered loops, genuine empty-output iterations, buffered/captured/transformed
blocks, break/continue through nested scopes, first-item breaks, top-level and
block returns, empty returned values, and ordinary macro returns.

Renderer unit tests additionally cover the short `PCQ` inheritance example,
multiple inheritance levels with child overrides, both tracker configurations,
observer-only explicit returns, and restoration after buffered-render errors.
The shared unit-test assertions check ranges, literal equality, balanced layout
and output coverage across the existing renderer regression examples.

The existing project integrations continue to check pre-injection offsets for
an ephemeral dependency, fresh provenance after a macro changes, disabled-node
manifest identity and absence of successful artifacts after rendering fails.
The executable JSON fixture above remains the complete format example.

## Local prototype validation

Checked on Windows x86-64, Python 3.13.5 and Rust stable 1.98.1:

- All 177 `dbt-jinja-utils` library tests passed.
- All 31 Python exporter integrations passed: the 28-case regression matrix
  and the three existing project/artifact tests. These use the ABI3 runtime
  built from `b57b0f671`; this test expansion changes no production renderer code.
- All 106 SQLFluff consumer and plugin-generation tests passed against that
  runtime, including real CLI source fixes and parallel-worker integration.
- The complete golden JSON example comparison passed.
- Rust formatting, Python test lint/format and diff whitespace checks passed.

This debug build required `RUST_LOG=debug` and `DBT_LOG_LEVEL=debug` to keep
upstream tracing spans enabled. Without them, the ordinary untraced Python
invocation also panicked during tracing initialization. The local test results
use that environment workaround; release-build behaviour was not tested.

These checks do not establish complete dbt/Core templater parity. Live warehouse
introspection, product-specific factories, cross-platform releases, alternate
branches and source-rewritten snapshots still need validation. The flag and
schema remain an upstream design proposal.
