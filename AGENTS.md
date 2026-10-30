# Project Instructions (AI Agents)

These instructions are for coding agents (Codex CLI, Claude Code, etc).

Paths in this file and in `.agents/` are relative to the directory containing this file.

## Hard Rules

### Code Style

- Follow existing code style and patterns in the given crate/module.
- Keep changes minimal and focused to the task at hand.
- Prefer simple, readable code over clever or complex solutions.

### Changelog

- Never manually create or write changelog YAML files. Always use `changie new` to generate entries.
- Run `changie new` from the repository root with flags: `changie new --kind "<kind>" --body "<description>" --custom "project=<project>" --custom "issue=<number>" --custom "author=<handle>" --interactive=false`
- For optional empty fields (`issue`, `author`), prompt the user if running locally or pass a space (e.g., `--custom "issue= "`) then edit the generated file to replace `' '` with `""`.

### Logging and printing

- Never call `tracing::{error,warn,info,debug,trace}!` or `tracing::event!`, and never use `println!`/`eprintln!`/`print!`/`eprint!`. This includes debug-only and internal code.
- Emit a structured event if one exists (`LogMessage`, `ProgressMessage`, ...). Otherwise use the level helper in `dbt_common::tracing::dbt_emit`: `emit_error_log_message`, `emit_warn_log_message`, `emit_info_log_message`, `emit_debug_log_message`, `emit_trace_log_message`.
- Only for output strictly to the CLI with no trace: `dbt_emit::print_err` instead of `eprintln!`, `dbt_emit::print`/`println` instead of `print!`/`println!`.
- Why: the emit helpers use `#[track_caller]` to record the call site and pass telemetry attributes through thread-local storage, and their events are exported to OTEL. The macros bypass both.
- Create spans with `create_info_span`/`create_debug_span` (`dbt_tracing::emit`), not `tracing::span!`. `#[tracing::instrument]`, `tracing::Instrument` and `Span::current()` are fine.
- Exceptions to the print ban: `build.rs` (`println!("cargo:...")` directives), tests, and `xtask`/dev tooling (`crates/xtask`, `dbt-ci`, `examples/`, `benches/`).
- Details: `.agents/telemetry-tracing.md`.

### Error codes

- Never use `ErrorCode::Generic` in new code. It carries no signal about what went wrong.
- Use `ErrorCode::Unexpected` only for a true internal inconsistent state, meaning an invariant the code itself broke. Never use it for errors caused by user input, config, project files, or the warehouse.
- Otherwise use the most specific existing `ErrorCode`, or add a new one in `crates/dbt-error/src/codes.rs` (CLI codes `[1000, 8999]`, internal `[9000, 9899]`; frontend codes come from `dbt_frontend_common::error::ErrorCode`, don't add them here).
- Why: codes `>= 9000` are reported as Fusion bugs (`ErrorCode::is_bug`), so misusing `Unexpected` inflates bug counts and hides the real cause. User-facing codes (`dbtNNNN`) tell users what to fix, and `warn_error_options` targets codes, so a shared code makes users promote or silence unrelated warnings. Give each new warning its own code.
- Existing `Generic`/`Unexpected` sites are legacy; don't copy them.

### Running dev commands

- Never run blanket `cargo` commands without `-p <crate>` unless explicitly asked.
- Avoid context pollution: don’t paste long logs; prefer LLM-friendly JSONL output and short excerpts.
- Avoid full builds and full test runs during iteration; keep commands scoped and fast.

## Where To Look Next (Task-Specific)

- Respect author preferences (optional/local): `AGENTS.local.md`
- When writing, running or recording adapter record/replay tests (end-to-end tests against a warehouse): `crates/dbt-sa-adapter-tests/README.md`
- When working on Adapters: `.agents/adapters.md`. Adapters consist of the following crates:
  - `dbt-adapter`
  - `dbt-adapter-core`
  - `dbt-adapter-sql`
  - `dbt-auth`
  - `dbt-sql-keywords`
  - `dbt-adbc`
- When working on telemetry/tracing: `.agents/telemetry-tracing.md`
- When working on dbt-docs-server (static site + DuckDB-WASM in the browser): `.agents/dbt-docs-server.md`
- When adding, graduating, or removing a feature flag or behavior flag, or reading one at a call site: declare it in `crates/dbt-flags/src/catalog.rs`; precedence and stages are documented in `crates/dbt-flags/src/lib.rs`. Do not add new `std::env::var` toggles or `project_flags_get_value` lookups for this.
- Always check for nested `AGENTS.md` files in subdirectories you modify
- If context is not enough, try reading repo overview / architecture as last resort: `README.md`
