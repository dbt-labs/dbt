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

### Running dev commands

- Never run blanket `cargo` commands without `-p <crate>` unless explicitly asked.
- Avoid context pollution: don’t paste long logs; prefer LLM-friendly JSONL output and short excerpts.
- Avoid full builds and full test runs during iteration; keep commands scoped and fast.

## Where To Look Next (Task-Specific)

- Respect author preferences (optional/local): `AGENTS.local.md`
- When working on Adapters: `.agents/adapters.md`. Adapters consist of the following crates:
  - `dbt-adapter`
  - `dbt-adapter-core`
  - `dbt-adapter-sql`
  - `dbt-auth`
  - `dbt-sql-keywords`
  - `dbt-adbc`
- When working on telemetry/tracing: `.agents/telemetry-tracing.md`
- When working on dbt-docs-server (static site + DuckDB-WASM in the browser): `.agents/dbt-docs-server.md`
- Always check for nested `AGENTS.md` files in subdirectories you modify
- If context is not enough, try reading repo overview / architecture as last resort: `README.md`
