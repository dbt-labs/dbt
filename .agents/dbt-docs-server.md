# dbt-docs-server — Agent context

Paths in this file are relative to the directory containing `AGENTS.md`.

## What it is

The successor to Core v1's `dbt docs generate` + `dbt docs serve` for the Rust dbt v2
runtime. Apache 2.0 crate at `crates/dbt-docs-server/`.

**`dbt docs generate` writes a static site.** There is no server-side query engine and
no HTTP API. The crate exports parquet next to a React SPA; the browser loads
DuckDB-WASM from a CDN and runs every query itself. The output directory works on any
plain file host — GitLab Pages, GitHub Pages, S3 — with no process and no state.
`dbt docs serve` is a static file host for local preview, and generates first when the
site is missing or older than the index.

**Critical:** it does NOT read `manifest.json` — not on the Rust side and not in the
browser. Data comes from the **dbt information schema** at `<target>/info_schema/v<n>/`:
`dbt.*.parquet` plus the generated `views.sql`. The exporter reads it through
`dbt-index-core::Backend` (`DuckDbInfoSchemaBackend`); the browser fetches the same
files over HTTP.

**The browser does not author view SQL.** It fetches `views.sql` and executes the
statements it needs (`web/src/shared/data-sources/duckdb/viewsSql.ts`), so the
relations the page queries are the ones generated for the parquet beside it. Writing
`CREATE VIEW` client-side is what made this app a second definition of the schema —
the cross-language half of fs#13788. Do not reintroduce it.

**`dbt docs generate` compiles, every time.** It synthesizes a `compile --write-index`
invocation and hands it to the ordinary phase pipeline (`build_index_for_docs`,
`fs/sa/crates/dbt-main/src/dbt_lib.rs`), then builds the information schema from the
metadata (`build_info_schema_for_docs`) — so a fresh checkout needs one command rather
than two. The information schema is built *after* the compile and its metadata ingest
rather than by the compile's own `--generate-info-schema`, because the invocation
record is written after that point; built earlier it would miss it and the site's
timings and status surfaces would come up empty. `--no-compile` opts out and reduces the command to the pure exporter it used to be,
where a missing index is an error. `--compile` is accepted as a hidden no-op for v1 script
compatibility.

There is deliberately **no artifact-presence check** deciding whether to compile. An
earlier version compiled only when `index_dir/` held no artifacts; that made the second
`docs generate` of the day publish the first one's output, which reads as a caching bug no
matter how it is documented. `--no-compile` already says "use what is on disk", so nothing
has to be inferred. This matches v1, which compiles unconditionally.

The synthesized invocation carries `EvalArgs::command` = `Compile`, because the phase
pipeline branches on it in roughly two dozen places and anything else silently produces
a near-empty index, and `command_entrypoint` = `Docs`, so the origin stays visible. That
second field is what suppresses the `--write-index` static-analysis advisories, which
would otherwise tell the user to add flags to a command they never ran.

## Key decisions (see `crates/dbt-docs-server/API-CONTRACTS.md` for full rationale)

The ADRs predate the static rearchitecture and are written in terms of REST endpoints.
The *decisions* still bind — they are about what data the UI gets and in what shape —
so read "endpoint" as "the query behind that surface".

| Decision | Rule | Revisit when |
|---|---|---|
| Type-specific detail queries | One projection per resource type. No generic "any node" detail lookup. | MCP is added to dbt-docs-server |
| `execution_info` placement | Nested on the resource detail, null when run results are absent. | Run history (last N runs) is required |
| Shared base projection | Typed detail queries compose the common node fields rather than restating them. | Never remove; only extend |
| Field naming | `snake_case` for every column a mapper reads (CC-1). | Never |
| Nested objects | Preserve nested objects from the Discovery API shape; do not flatten (CC-2). | Only flatten singleton wrappers |
| Capability gating | Nullable fields gated by a capability flag, never by query variants (CC-3). | Never |
| Tests fold together | Test detail serves both `test.*` and `unit_test.*` as a discriminated union on `resource_type`. Both render on the same page (ADR-3). | Unit tests and generic tests need separate detail pages |
| `execution_info` field naming | Bare names only: `status`, `completed_at`, `error`. No `last_run_*` or phase-scoped prefixes — this is a snapshot, not a history. (ADR-4) | Multi-run history is required |
| Browser-direct telemetry | Events are encoded client-side and POSTed straight to Vortex; no relay (ADR-10, supersedes ADR-9). | Consent needs a server-side authority again |

## Data flow

```
dbt project
    │  dbt compile [--static-analysis strict]
    │  …or nothing: `docs generate` runs that compile itself unless `--no-compile`
    ▼
<target>/private/metadata/**    ← epoch files
    │  write_info_schema (COPY from the epoch views; Arrow fallback)
    ▼
<target>/info_schema/v<n>/      ← dbt.*.parquet + views.sql (NOT manifest.json)
    │  dbt docs generate — writes the embedded SPA beside it, nothing else
    ▼
<target>/index.html + assets/   ← the site; the data stays in info_schema/v<n>/
    │  views.sql fetched once, then artifacts fetched whole on demand
    ▼
DuckDB-WASM from jsDelivr, in the page
    ▼
React SPA (hash routing, relative asset base)
```

`--static-analysis strict` on the compile is what produces column lineage. Without it
the export writes no `dbt.column_lineage.parquet`, which is the normal case, not an
edge case — and it stays the normal case for the auto-compile, which deliberately runs a
plain `--write-index` rather than forcing strict. Column lineage therefore remains
opt-in through an explicit compile.

Typical local loop:

```
dbtd docs generate                  # compiles, then writes into target/
cd target && python3 -m http.server # a host with no SPA rewrite and no ranges
```

Compile separately when you want column lineage, or to iterate on the exporter without
paying for a compile each time:

```
dbtd compile --generate-info-schema --static-analysis strict
dbtd docs generate --no-compile
```

That last step is the load-bearing check: if it works under `http.server` it works on
GitLab Pages.

## Capability gating

`dbt-docs-server` depends only on `dbt-index-core`'s public traits. Which parquet exists
varies by how the index was produced, so features are detected rather than assumed —
but the detection now happens in the browser:

- `has_column_lineage` ⟸ `dbt.column_lineage` **has rows**. Presence is no longer a
  signal: the information schema writes every table even at zero rows, precisely so
  `views.sql` always resolves, so the file is always there. A populated and an empty
  one even measured the same 1552 bytes on a real project, so size is not a signal
  either. Both the exporter's progress message and the browser ask the same rows
  question, so they cannot disagree.
- A gated surface renders the upsell/fallback state. No 412, no error. Gate *before*
  the query, never by catching one: mapping a thrown query to "gated" renders every
  mistake in the SQL as an upgrade card.

Two things follow, and both have bitten:

1. **Absence must look deliberate.** The degraded path is the default path, so a gated
   surface rendering as an empty list next to a non-zero sidebar count reads as data
   loss. Use `isSupported` on `useSourceQuery` and the unsupported-surface message.
2. **The information schema writes every table, schema-only when empty**, so client
   SQL can assume every relation exists and skip "view might be missing" variants.
   There is no client-side empty-relation DDL any more, and nothing to keep in step
   with it.

## Tech stack

**Frontend** — paths relative to `crates/dbt-docs-server/web/`:
- React 19 + TypeScript + Vite, standalone pnpm project (not part of any workspace)
- Routing: `react-router-dom` **`HashRouter`**, Vite `base: './'`, `LinkPrefixProvider
  prefix="/"` (the router owns the `#`). All three are required for subpath-agnostic
  static hosting: a plain file host rewrites nothing, so real paths would 404 on a
  deep-link reload. The hash also never reaches the server, which keeps
  `document.baseURI` stable for resolving the data directory.
- Data: `@tanstack/react-query` over the `MetadataDataSource` adapter in `src/shared/`.
  `createDuckDbDataSource` is the only production implementation.
- Styling: Tailwind (configured in `tailwind.config.cjs`, with the sourdough and
  dbt-dag presets) **plus** hand-written CSS in `src/app.css` keyed by BEM-style
  class prefixes (e.g. `locate-pane__`). Both are in use — check `app.css` before
  adding a utility-class-only solution, and prefer sourdough CSS custom properties
  (`var(--fgMain)`, `var(--bgMainHover)`, …) over literal colors.
- Icons: `@dbt-labs/sourdough` (Ryecon) and `@dbt-labs/dbt-dag` (`DbtResourceIcon`)
- `src/shared/` is a **fork** of the docs-v2 slice of `@dbt-labs/metadata-shared`
  (dbt-ui). It mirrors upstream's layout and barrel so the two can be diffed. Treat
  it as vendored: prefer additive local changes, and expect upstream drift.
- Private deps (`@dbt-labs/sourdough`, `dbt-dag`, `biga`) come from GitHub Packages,
  so `pnpm install` needs `GITHUB_TOKEN` with `read:packages`. `cargo build` does
  not — `web/dist/` is committed.
- **After any `web/` source edit, run `pnpm build` and commit `web/dist/` in the same
  change.** Nothing enforces this — no CI job, no git hook — so a source-only commit
  silently ships a stale UI.
- `pnpm dev` needs a generated site to read: point `DBT_DOCS_DEV_SITE` at one and the
  `devSite` plugin injects a dev bootstrap and serves its `data/`.

**Backend** — paths relative to `crates/dbt-docs-server/`:
- Rust: `axum` + `tokio` + `arrow-array` + `rust-embed`
- `src/export/` — the whole product: artifact selection, `COPY … TO`, bootstrap
  injection
- `src/server.rs` — a router with **no routes**, only a static fallback
- `src/state.rs` — `AppState`: index dir + provider trait objects

## Key files

All paths relative to `crates/dbt-docs-server/`.

| File | Role |
|---|---|
| `src/export/mod.rs` | The exporter. `data_dir()`, refuses to write a site with zero resources, decides lineage from rows rather than from the file |
| `src/export/bootstrap.rs` | `window.__DBT_DOCS__` injection, with `<`/`>`/`&` escaped so the payload cannot steer the HTML tokenizer |
| `src/server.rs` | Static host. No routes |
| `src/state.rs` | AppState: data dir + providers |
| `web/src/main.tsx` | The only production data-source construction site; throws if the bootstrap is missing |
| `web/src/types.ts` | Shared wire vocabulary that outlived the API (`NodeSummary`, the telemetry event union) |
| `web/src/lib/siteBootstrap.ts` | Reads and version-checks `window.__DBT_DOCS__`; resolves `data/` against `document.baseURI` |
| `web/src/lib/vortexSink.ts` | Browser Vortex producer. `enabled: false` under denied consent, so no call site can leak through |
| `web/src/shared/data-sources/duckdb/` | `viewsSql.ts` (fetch + parse the shipped `views.sql`), `engine.ts` (CDN load, `registerFileBuffer`, on-demand registration), `bootstrap.ts` (hyparquet first paint), `sql.ts`, `lists.ts`, `details.ts`, `search.ts` |
| `web/src/shared/data-sources/duckdb/realArtifacts.test.ts` | Every query in the app, bound against a real information schema. Opt-in: `DBT_DOCS_REAL_ARTIFACTS=<target>/info_schema/v1 pnpm test`. This is the mechanical form of the habit below, and it has caught column-name bugs no other test can |
| `web/src/shared/data-sources/mappers/fromWire.ts` | The single mapping layer, 46 tests. Column names in every SQL projection match what these mappers read — that is the guard against drift |
| `web/src/shared/data-sources/conformance.test.ts` | Protocol-agnostic suite over `MetadataDataSource`; holds the fake and the DuckDB source to one contract |
| `web/src/shared/typings/domain/` | Domain types (`Asset`, `ModelSummary`, `Capabilities`, …) |
| `web/src/lib/resourceType.tsx` | Resource type order, labels, icons, badge colors — single source of truth |
| `web/src/components/LocatePane.tsx` | Sidebar: AssetMode, TreeMode, FilterMode |
| `web/src/app.css` | Hand-written CSS (alongside Tailwind utilities in the TSX) |
| `web/dist/` | Committed build embedded by `rust-embed`; regenerate with `pnpm build` |

## The artifact set

The dbt information schema, defined in `crates/dbt-index-core/src/info_schema/schema.rs`
and rendered as views by `info_schema/views.rs`. **There is no exported artifact set.**
The site reads `<target>/info_schema/v<n>/` as it was written — no projection, no split,
no copy. `window.__DBT_DOCS__.data_dir` carries the directory (version and all, because
the version belongs to the writer), and `--output-dir` copies it verbatim, `views.sql`
included, so a standalone site reads the same files.

Names to know, because they are *not* the index's:

| Index | Information schema |
|---|---|
| `dbt.nodes` | one table per resource type (`dbt.models`, `dbt.seeds`, …), unioned by `dbt_internal.resources` |
| `dbt.docs` / `block_contents` | `dbt.docs_blocks` / `content` |
| `dbt.test_metadata` | folded into `dbt.data_tests` |
| `dbt.source_freshness` | `dbt_rt.freshness` |
| `dbt.catalog_tables` + EAV `dbt.catalog_stats` | `dbt_rt.relations`, one typed row per relation |
| `dbt.generation` | `dbt.project.last_full_parse_at` |
| `access_level`, `group_name`, `patch_path`, `declared_type`, `from_*`/`to_*` | `access`, `group`, `properties_yml_file_path`, `data_type_declared`, `parent_*`/`child_*` |

Three consequences worth knowing:

1. **`dbt_internal.resources` reads every resource artifact.** Use the typed table when
   the resource type is known — lists and details do. The union is for the surfaces
   that genuinely span types: counts, the file tree, search, lineage metadata.
   `dbt.dag_nodes` is *not* a substitute: three columns, enabled rows only, DAG types
   only.
2. **First paint cannot use `views.sql`.** There is no DuckDB yet, so `bootstrap.ts`
   reads each resource artifact with hyparquet and concatenates. That list is the only
   place left that names information-schema tables in TypeScript; a resource type
   missing from it is a silently empty sidebar section.
3. **Every table is written, schema-only when empty.** An absent artifact is a broken
   site, not a signal — the opposite of the index, and why there is no client-side
   empty-relation DDL any more.

**Not available:** UDF/function resources — count is always 0 until dbt v2 writes UDF
parquet. Catalog numbers (`row_count`, `bytes`, `last_modified`) need a `dbt run` or
`dbt build`: the catalog fetch is gated on those commands, and `docs generate`
synthesizes a *compile*. DuckDB reports no catalog stats at all, so verifying that
column end to end needs Snowflake or BigQuery.

## Adding UI or queries

Contracts + ADRs: `crates/dbt-docs-server/API-CONTRACTS.md`

1. Read `API-CONTRACTS.md` — check existing ADRs before proposing any design; do not
   re-litigate closed ADRs.
2. **Run the SQL against real parquet before wiring it up.** This is the single
   highest-leverage habit in this crate. Nine bugs in the original port were column
   names that do not exist (`n.language`), columns that are JSON strings rather than
   structs (`meta`), or names that differ from the obvious guess (`dbt.project` stores
   `project_name`, not `name`); the migration onto the information schema added two
   more. None would have been caught by types or by tests against fixtures.

   `realArtifacts.test.ts` does this mechanically for every query in the app:

   ```
   dbtd compile --generate-info-schema --static-analysis strict
   DBT_DOCS_REAL_ARTIFACTS=<project>/target/info_schema/v1 pnpm test
   ```
3. Keep projection column names identical to what `fromWire.ts` reads. The mappers are
   the contract; renaming a column silently nulls a field.
4. Gate nullable data on artifact presence, never on a build flag or a query variant.
5. Icons: use `DbtResourceIcon resource={type}` (from `@dbt-labs/dbt-dag`) for all
   resource types including group — no special cases needed.
6. CSS: use sourdough CSS vars, follow existing BEM class prefixes in `app.css`.

## Icon reference

`DbtResourceIcon` from `@dbt-labs/dbt-dag` is the canonical icon component for all resource types.
Its `resourceIconMap` maps each type to a Ryecon icon from `@dbt-labs/sourdough`.

| Resource type | `resourceIconMap` entry | Notes |
|---|---|---|
| model | `RyeconModel` | |
| source | `RyeconDatabase` | |
| test, unit_test | `RyeconClipboardSuccess` | unit_test folds into test in the UI |
| exposure | `RyeconMeter` | |
| group | `RyeconGroup` | |
| metric | `RyeconChartColumn` | |
| semantic_model | `RyeconGraphNodes` | |
| seed | `RyeconSeed` | |
| macro | `RyeconFile` | |
| snapshot | `RyeconCamera` | |
| saved_query | `RyeconSave` | |
| function | `RyeconFunction` | count is 0 until dbt v2 writes UDF parquet |
| analysis | `RyeconCrosshair` | kept for older project compat |

`DbtResourceIcon`'s `resource` prop expects a specific union type. Cast with `resource={t as any}`
if TypeScript complains; verify with `cargo xtask check-llm -p dbt-docs-server`.

## Telemetry

Events go from the browser straight to the Vortex collector — `@dbt-labs/vortex` +
`@dbt-labs/proto` + `@bufbuild/protobuf`, encoded client-side. ADR-10 in
`API-CONTRACTS.md` has the reasoning and supersedes ADR-9's server-side relay.

- **Consent** rides in `window.__DBT_DOCS__.telemetry.enabled`, resolved at export time:
  only the machine running `dbt docs generate` can read the project and the profile.
  It fails closed — an unreadable bootstrap denies consent.
- **Hydration** that the relay used to do authoritatively (`dbt_version`,
  `distribution`, `is_logged_in`, the `dbt_cloud_*` context) is now baked into the
  bootstrap at export time from `DistInfoProvider::telemetry_hydration()`.
- `@dbt-labs/vortex` does `await import("node:fs")` in a dev-mode branch behind
  `PLATFORM === "nodejs"`. It is browser-safe, but Vite will warn about the
  externalized `node:fs` on every build. Do not "fix" this by vendoring the producer.
- The event schema itself is inherited from the relay and is reviewed separately. Do not
  add fields to it as a side effect of a UI change.
