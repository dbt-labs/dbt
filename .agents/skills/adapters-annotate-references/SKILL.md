---
name: adapter-skills-annotate-references
description: Use when annotating fs adapter implementations with upstream dbt-adapters Python reference links. Triggers whenever you need to add or audit GitHub permalink comments in adapter_impl.rs, trace an fs adapter method back to its Python origin in dbt-adapters, or when working on adapter divergence and want to know where a method came from. Also use when asked to "annotate references", "add upstream links", or "link adapter methods".
---

# Adapter Skills: Annotate References

Walks the Python `@available` method chain (`BaseAdapter → SQLAdapter → [Platform]Adapter`) and
annotates the corresponding Rust `pub fn` methods in `adapter_impl.rs` with permanent upstream
GitHub links. Methods present in Python but absent in Rust are written to a gap report.

## Background

`fs` implements adapter methods in Rust, drawing from the Python inheritance chain:

```
BaseAdapter  (dbt-adapters/src/dbt/adapters/base/impl.py)
  └── SQLAdapter  (dbt-adapters/src/dbt/adapters/sql/impl.py)
        └── PlatformAdapter  (dbt-{bigquery,snowflake,redshift,spark}/src/dbt/adapters/{platform}/impl.py)
```

The dbt-adapters repo is a **monorepo** (Snowflake, BigQuery, Redshift, Spark, Postgres, Athena,
plus the base and SQL layers). Databricks lives in a **separate** repo.

`@available` (and variants `@available.parse_list`, `@available.parse_none`, `@available.parse(…)`)
marks methods exposed to the Jinja environment. Starting from Python guarantees full coverage,
including platform-specific methods that only exist in one adapter's `impl.py`.

---

## Step 0 — Gather configuration

Ask the user two questions:

1. **Local path to the dbt-adapters checkout** (the dbt-labs monorepo):
   > What is the local path to your dbt-adapters checkout? (e.g. `~/code/dbt-adapters`)

2. **Any other adapter repos to include?** (comma-separated, optional):
   > e.g. `~/code/dbt-databricks` — leave blank if none

The gap report defaults to `missing_adapter_methods.md` at the repo root.

---

## Step 1 — Run the reverse annotation script

```bash
python3 .agents/adapter-skills-annotate-references/scripts/annotate.py \
    --adapter-impl <path/to/adapter_impl.rs> \
    --dbt-adapters <dbt-adapters-path> \
    --all-adapters \
    [--extra-repos <path1>,<path2>,...] \
    [--missing-out missing_adapter_methods.md] \
    [--dry-run]
```

Always pass `--all-adapters` to scan the full monorepo. Use `--extra-repos` for repos outside
it (e.g. dbt-databricks). Use `--dry-run` to preview without writing.

The script:
1. Enumerates all `@available` methods across every `*impl.py` in the provided repos.
2. Scans `adapter_impl.rs` for `pub fn` methods.
3. For methods **found in Rust, not yet annotated** → inserts `/// ClassName <url>`.
4. For methods **not found in Rust** → records them in the gap report.
5. Prints a JSON summary to stdout: `{annotated, already_annotated, missing_from_rust, …}`.

---

## Step 2 — Review the results

**Annotations:** spot-check a few inserted lines:
```rust
/// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/<sha>/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L42
pub fn load_dataframes(
```

**Gap report** (`missing_adapter_methods.md`): rendered in canonical adapter order — Snowflake,
BigQuery, Redshift, Spark, Postgres, Athena, then any extra repos (e.g. Databricks), then
Base / SQL Adapter. Adapters with full coverage show ✅. Share the report with the user and
decide whether any missing methods need follow-up porting work.

---

## Annotation format

```
/// <ClassName> <github-permalink>
```

**Placement** (handled automatically):
- No existing `///` block → insert above the first `#[…]` attribute (or above `pub fn` directly).
- Existing `///` block → append after its last line, with a blank `///` separator if needed.

```rust
// No prior doc comment:
/// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/<sha>/dbt-adapters/src/dbt/adapters/base/impl.py#L862
#[tracing::instrument(…)]
pub fn get_missing_columns(

// Existing doc comment:
/// Returns columns present in source but absent in target.
///
/// BaseAdapter https://…/base/impl.py#L862
pub fn get_missing_columns(
```

The class name prefix (`BaseAdapter`, `SQLAdapter`, `SnowflakeAdapter`, …) tells readers at a
glance which level of the hierarchy the method comes from.

---

## Step 3 — Verify

```bash
cargo check -p dbt-adapter
cargo nextest run -p dbt-adapter
```

---

## Per-method lookup

To investigate a single method's upstream origin:

```bash
python3 .agents/adapter-skills-annotate-references/scripts/find_upstream.py \
    <method_name> <dbt-adapters-path> \
    [--adapter-type <type>] \
    [--extra-repos <path>]
```

Returns JSON with `found`, `best` (lowest-priority = most specific match), `all_matches`, and
`non_available_matches`. Only `@available`-decorated methods appear in `all_matches`; the rest
are in `non_available_matches` for debugging. When `found: false`, skip — no annotation needed.

---

## Notes

- `///` doc comment → method-level upstream link (what this skill adds)
- `//` inline comment → within-body line notes (leave as-is)
- Scripts use the current git SHA for stable permalinks; never use `main`-based URLs.
- Monorepo package URLs are repo-root-relative: `dbt-bigquery/src/…`, `dbt-adapters/src/…`
