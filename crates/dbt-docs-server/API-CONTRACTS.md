# dbt-docs-server API contracts

## Contents

- [dbt-docs-server API contracts](#dbt-docs-server-api-contracts)
  - [Contents](#contents)
  - [How to use this document](#how-to-use-this-document)
  - [Conventions](#conventions)
  - [ADR-1: Single-resource detail endpoint structure](#adr-1-single-resource-detail-endpoint-structure)
    - [Options considered](#options-considered)
    - [Backend prerequisite](#backend-prerequisite)
  - [ADR-2: `execution_info` placement](#adr-2-execution_info-placement)
    - [Options considered](#options-considered-1)
  - [Backend conventions](#backend-conventions)
    - [`NodeBase` struct](#nodebase-struct)
    - [Capability flags](#capability-flags)
  - [`GET /api/v1/models/:id`](#get-apiv1modelsid)
    - [Example response](#example-response)
    - [Field reference](#field-reference)
    - [Type definition](#type-definition)
    - [Risk register](#risk-register)
  - [ADR-3: `GET /api/v1/tests/:id` response shape](#adr-3-get-apiv1testsid-response-shape-for-test-vs-unit_test)
  - [`GET /api/v1/nodes/:id` (deferred)](#get-apiv1nodesid-deferred)
  - [ADR-4: `execution_info` field naming — single-run semantics](#adr-4-execution_info-field-naming--single-run-semantics)
  - [ADR-5: `execution_info` omission for definition-only and Semantic Layer resources](#adr-5-execution_info-omission-for-definition-only-and-semantic-layer-resources)
  - [ADR-7: Capabilities are distribution-gated; parquet absence emits JSON `null`](#adr-7-capabilities-are-distribution-gated-parquet-absence-emits-json-null)
  - [`GET /api/v1/sources/:id`](#get-apiv1sourcesid)
  - [`GET /api/v1/seeds/:id`](#get-apiv1seedsid)
  - [`GET /api/v1/snapshots/:id`](#get-apiv1snapshotsid)
  - [`GET /api/v1/tests/:id`](#get-apiv1testsid)
  - [Design notes — `GET /api/v1/exposures/:id`](#design-notes--get-apiv1exposuresid)
  - [`GET /api/v1/exposures/:id`](#get-apiv1exposuresid)
  - [Design notes — `GET /api/v1/groups/:id`](#design-notes--get-apiv1groupsid)
  - [`GET /api/v1/groups/:id`](#get-apiv1groupsid)
  - [Design notes — `GET /api/v1/macros/:id`](#design-notes--get-apiv1macrosid)
  - [`GET /api/v1/macros/:id`](#get-apiv1macrosid)
  - [Design notes — `GET /api/v1/metrics/:id`](#design-notes--get-apiv1metricsid)
  - [`GET /api/v1/metrics/:id`](#get-apiv1metricsid)
  - [Design notes — `GET /api/v1/saved_queries/:id`](#design-notes--get-apiv1saved_queriesid)
  - [`GET /api/v1/saved_queries/:id`](#get-apiv1saved_queriesid)
  - [Design notes — `GET /api/v1/semantic_models/:id`](#design-notes--get-apiv1semantic_modelsid)
  - [`GET /api/v1/semantic_models/:id`](#get-apiv1semantic_modelsid)
  - [ADR-6: List + facets endpoint envelope](#adr-6-list--facets-endpoint-envelope)
  - [`GET /api/v1/sources`](#get-apiv1sources)
  - [`GET /api/v1/sources/facets`](#get-apiv1sourcesfacets)
  - [`GET /api/v1/seeds`](#get-apiv1seeds)
  - [`GET /api/v1/seeds/facets`](#get-apiv1seedsfacets)
  - [`GET /api/v1/snapshots`](#get-apiv1snapshots)
  - [`GET /api/v1/snapshots/facets`](#get-apiv1snapshotsfacets)
  - [`GET /api/v1/tests`](#get-apiv1tests)
  - [`GET /api/v1/tests/facets`](#get-apiv1testsfacets)
  - [`GET /api/v1/exposures`](#get-apiv1exposures)
  - [`GET /api/v1/exposures/facets`](#get-apiv1exposuresfacets)
  - [`GET /api/v1/groups`](#get-apiv1groups)
  - [`GET /api/v1/groups/facets`](#get-apiv1groupsfacets)
  - [`GET /api/v1/macros`](#get-apiv1macros)
  - [`GET /api/v1/macros/facets`](#get-apiv1macrosfacets)
  - [`GET /api/v1/metrics`](#get-apiv1metrics)
  - [`GET /api/v1/metrics/facets`](#get-apiv1metricsfacets)
  - [`GET /api/v1/saved_queries`](#get-apiv1saved_queries)
  - [`GET /api/v1/saved_queries/facets`](#get-apiv1saved_queriesfacets)
  - [`GET /api/v1/semantic_models`](#get-apiv1semantic_models)
  - [`GET /api/v1/semantic_models/facets`](#get-apiv1semantic_modelsfacets)
  - [ADR-8: Unified `GET /api/v1/search` endpoint as the documented exception to ADR-1](#adr-8-unified-get-apiv1search-endpoint-as-the-documented-exception-to-adr-1)
  - [`GET /api/v1/search`](#get-apiv1search)
  - [ADR-9: Server-side analytics relay as the documented exception to the read-only GET pattern](#adr-9-server-side-analytics-relay-as-the-documented-exception-to-the-read-only-get-pattern)
  - [`POST /api/v1/analytics/events`](#post-apiv1analyticsevents)
  - [ADR-10: Browser-to-Vortex analytics for static sites (supersedes ADR-9)](#adr-10-browser-to-vortex-analytics-for-static-sites-supersedes-adr-9)

---

## How to use this document

All architectural decisions about the dbt-docs-server REST API are recorded here as ADRs.
When an ADR status is "Decided", the decision is **closed** — do not re-litigate it in
PRs or planning. To change a closed decision, add a new ADR that supersedes the old one.

Every new endpoint contract must be appended here before implementation begins. A PR that
implements an endpoint without a corresponding contract entry should be rejected in review.

Use `claude/prompts/dbt-docs-parity.md` (local, untracked) to run a parity analysis
that produces the next contract entry.

---

## Conventions

Field naming, data classification, and pagination follow the methodology in
`/Users/eddowh/codaz/poc-dbt-index-docs/FEATURE-TO-ENDPOINT-MAPPING.md`
(outside this repo). Key cross-cutting rules:

| Code | Rule |
|---|---|
| CC-1 | `snake_case` for all JSON field names and REST path segments |
| CC-2 | Preserve nested objects from Discovery API shape; do not flatten (exception: singleton wrappers may be flattened) |
| CC-3 | Nullable fields gated by `Capabilities` flags; no query variants |
| CC-4 | Cursor-based pagination (`?first=&after=`) for **all** list endpoints. Response envelope includes `page_info: { end_cursor, has_next_page }`. No `total` field — counts are O(N) and defeat cursor pagination's constant-time-per-page guarantee; if a real consumer needs a count, expose it as an opt-in (`?include_total=true`) additively. ADR-6 captures the full envelope. |
| CC-5 | Three classes of "internal" data: **A** = Discovery-internal but parquet-backed → promote to public REST; **B** = no parquet path → exclude; **C** = public in Discovery but CodexDB-only → stub 412 with `upgrade_path` |
| CC-6 | Inline edge arrays (`depends_on`, `referenced_by`, `models[]` member lists) are capped server-side at 500 entries and signal truncation with `truncated: true` on the response. Clients cannot tune the cap — the legacy `?first=<n>` parameter was withdrawn to free the `first` name for LIST cursor pagination (ADR-6, CC-4). If a real consumer ever needs more than 500 inline edges, the path is a dedicated sub-resource (e.g., `GET /api/v1/<r>/:id/edges`) with its own cursor pagination, not retrofitting a client knob. Applies to every typed detail endpoint that exposes inline arrays. |
| CC-7 | JSON-string parquet columns (`meta`, `config`, `type_params`, `query_params`, `exports`, `arguments`, `agg_params`, `validity_params`, `metric_filter`, `non_additive_dimension`, etc.) are deserialized handler-side via a shared `json_parse_or_null` helper in `src/handlers/json.rs`. Failed parse → emit `null` and `tracing::warn`; never bubble the error to the client and never leak escaped JSON strings to the response. |

---

## ADR-1: Single-resource detail endpoint structure

**Status:** Decided — type-specific endpoints chosen for v0. Generic dispatcher deferred.
**Trigger to revisit:** MCP is added to dbt-docs-server.

### Options considered

**Generic endpoint only: `GET /api/v1/nodes/:id`**

Returns all resource types as a discriminated union keyed by `resource_type`.

- Pro: single endpoint; no surface proliferation; aligns with `GET /api/v1/nodes` list.
- Pro: FEATURE-TO-ENDPOINT-MAPPING.md Appendix B3 recommended this initially.
- **Con:** TypeScript union types are a UX tax — on a dedicated Model detail page the
  frontend already knows it's rendering a model; narrowing adds overhead at every call
  site on every dedicated detail page.
- **Con:** OpenAPI `oneOf` + discriminator codegen ergonomics depend on toolchain;
  `openapi-typescript` handles it, `swagger-codegen` does not. Adding a codegen pipeline
  as a prerequisite for basic productivity is the wrong tradeoff for v0.
- **Con:** Diverges from Discovery API's per-type operation structure; FE engineers
  familiar with that API face an impedance mismatch.

→ **Rejected.** Union overhead at every call site; benefit accrues only to server-side
maintainers.

---

**Type-specific endpoints: `GET /api/v1/models/:id`, `GET /api/v1/sources/:id`, etc.**

Each resource type has its own endpoint returning a standalone TypeScript type.

- Pro: `ModelDetail`, `SourceDetail`, `TestDetail` are clean standalone types.
- Pro: Mirrors Discovery API's per-type operation structure.
- Pro: FE engineers explicitly requested this.
- Pro: Each endpoint is independently testable and evolvable.
- Con: N endpoints to maintain as resource types grow.
- Con: No generic "give me any resource by `unique_id`" — but this use case does not
  exist in v0 UI: detail pages know the type from routing; lineage components know from
  `resource_type` already present in `NodeSummary`.
- Con: Common fields repeated across Rust structs unless a `NodeBase` is factored out —
  mitigated by the backend prerequisite below.

→ **Chosen for v0.**

---

**Type-specific + generic dispatcher**

Type-specific endpoints exist as above. `GET /api/v1/nodes/:id` also exists as a thin
router that parses the `unique_id` prefix (`model.`, `source.`, etc.) and delegates to
the appropriate typed handler.

- Pro: Adds back the "I have a `unique_id` and don't know the type" escape hatch —
  useful for MCP tools and AI agents.
- Pro: Additive over the chosen option; no rework required.
- Con: No identified v0 UI use case.
- Con: Unnecessary surface invites misuse and undermines the clean-type story.

→ **Deferred, not rejected.** Trigger: MCP lands in dbt-docs-server. At that point a
one-afternoon addition — provided `NodeBase` is already factored out.

### Backend prerequisite

**All typed detail handlers must compose a shared `NodeBase` Rust struct.** Without it,
adding the generic dispatcher later requires duplicating SQL queries across N handlers.
With it, the dispatcher is a string split plus a match expression.

---

## ADR-2: `execution_info` placement

**Status:** Decided — inline in each resource detail response, null when no row exists in `dbt_rt.run_results` for the resource. (See ADR-7 — the original "null-gated by capability" wording was misleading; parquet absence is not a capability.)
**Trigger to revisit:** Run history (last N runs) becomes a product requirement.

### Options considered

**Inline in model detail response**

`execution_info` is a nested object in `ModelDetail`. `null` when `dbt build` hasn't
run — that is, when `dbt_rt.run_results` has no row for this resource.

- Pro: One request for everything the page needs.
- Pro: Consistent with how `columns[]` is already inlined.
- Pro: Recommended by FEATURE-TO-ENDPOINT-MAPPING.md POC analysis for v0.
- Con: If run history (last N runs) is added later, the inline field forces a breaking
  schema change rather than an additive one.

**Separate sub-resource: `GET /api/v1/models/:id/run-results`**

- Pro: Run history can be added later without breaking the model detail contract.
- Con: Two round trips per page render; more client complexity.
- Con: Over-engineered for v0 where only latest run is needed.

→ **Inline chosen.** If run history is added later, promote to a sub-resource — that
change requires a deprecation period or version bump.

---

## Backend conventions

### `NodeBase` struct

All typed detail handlers compose this struct for fields shared across all resource types.

```rust
// Fields common to every resource type — all typed handlers compose this.
// Precondition for ADR-1's deferred generic dispatcher to remain cheap to add.
struct NodeBase {
    unique_id: String,
    name: String,
    resource_type: String,
    package_name: Option<String>,
    description: Option<String>,
    original_file_path: Option<String>,
    tags: Vec<String>,    // dbt.nodes — not yet queried in existing handler; add to SELECT
    fqn: Vec<String>,     // dbt.nodes — not yet queried in existing handler; add to SELECT
}
```

### Capability flags

`Capabilities` is solely distribution-gated per ADR-7. The only flag is `has_column_lineage` (proprietary `dbt-index` exposes it; the SA distribution wires `UnavailableColumnLineage` and reports `false`).

Optional sub-objects on detail responses (`execution_info`, `catalog`, `freshness`) are emitted as JSON `null` when the relevant parquet view has no row — handlers do not gate them through `Capabilities`. See ADR-7 for the rationale.

---

## `GET /api/v1/models/:id`

Powers: `ModelView` / `ResourceDetailsPage` in dbt-ui.

dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/ModelView.tsx`

GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/discovery/model.ts`

### Example response

Fields marked `// conditional` are `null` when their capability gate is absent.
Fields marked `// 🔧` are not yet returned — they require a backend change.

```json
{
  "unique_id": "model.jaffle_shop.orders",
  "name": "orders",
  "resource_type": "model",
  "package_name": "jaffle_shop",
  "materialized": "table",
  "description": "Final orders model combining payments and order status.",
  "database_name": "prod",
  "schema_name": "dbt_prod",
  "relation_name": "prod.dbt_prod.orders",
  "identifier": "orders",
  "original_file_path": "models/orders.sql",
  "file_path": "models/orders.sql",
  "access_level": "public",
  "group_name": "finance",
  "raw_code": "select order_id, ...\nfrom {{ ref('stg_orders') }}",
  "compiled_code": "select order_id, ...\nfrom prod.dbt_prod.stg_orders",
  "contract_enforced": true,
  "tags": ["finance", "core"],
  "fqn": ["jaffle_shop", "orders"],
  "columns": [
    {
      "name": "order_id",
      "index": 0,
      "data_type": "integer",
      "declared_type": "int",
      "inferred_type": null,
      "catalog_type": "INT64",
      "description": "Unique order identifier.",
      "label": null,
      "granularity": null
    }
  ],
  "depends_on": [
    { "unique_id": "model.jaffle_shop.stg_orders", "edge_type": "model" },
    { "unique_id": "model.jaffle_shop.stg_payments", "edge_type": "model" }
  ],
  "referenced_by": [
    { "unique_id": "exposure.jaffle_shop.revenue_dashboard", "edge_type": "exposure" }
  ],
  "execution_info": {
    "status": "success",
    "execution_time": 4.2,
    "completed_at": "2026-05-15T10:32:11Z"
  },
  "catalog": {
    "type": "table",
    "owner": "dbt_runner",
    "bytes_stat": null,
    "row_count_stat": null
  }
}
```

`execution_info` is `null` when `dbt_rt.run_results` has no rows for this model (i.e., `dbt build` has not run or produced no result for this node).
`catalog` is `null` when `dbt.catalog_tables` has no rows for this model (i.e., `dbt docs generate` has not run).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | ✅ | — | e.g., `"model.pkg.name"` |
| `name` | `string` | Core | ✅ | — | |
| `resource_type` | `"model"` | Core | ✅ | — | Always `"model"` for this endpoint |
| `package_name` | `string \| null` | Core | ✅ | — | |
| `description` | `string \| null` | Core | ✅ | — | |
| `original_file_path` | `string \| null` | Core | ✅ | — | Relative to project root |
| `file_path` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.file_path`; for models equals `original_file_path` (compiled SQL is in a separate compile target, not surfaced here) |
| `tags` | `string[]` | Core | ✅ | — | `List(Utf8)` column in `dbt.nodes` |
| `fqn` | `string[]` | Core | ✅ | — | `List(Utf8)` column in `dbt.nodes` |
| `materialized` | `string \| null` | Core | ✅ | — | `"table"` · `"view"` · `"incremental"` · `"ephemeral"` |
| `database_name` | `string \| null` | Core | ✅ | — | |
| `schema_name` | `string \| null` | Core | ✅ | — | |
| `relation_name` | `string \| null` | Core | ✅ | — | Fully qualified: `db.schema.name` |
| `identifier` | `string \| null` | Core | ✅ | — | |
| `access_level` | `string \| null` | Core | ✅ | — | `"public"` · `"protected"` · `"private"` — see Risk #6 |
| `group_name` | `string \| null` | Core | ✅ | — | |
| `contract_enforced` | `boolean \| null` | Core | ✅ | — | |
| `raw_code` | `string \| null` | Core | ✅ | — | |
| `compiled_code` | `string \| null` | Core | ✅ | — | Confirmed present in `dbt.nodes`; matches `raw_code` for SQL models without macros |
| `columns` | `ModelColumn[]` | Core | ✅ | — | Empty array if no columns declared |
| `columns[*].name` | `string` | Core | ✅ | — | |
| `columns[*].index` | `number \| null` | Core | ✅ | — | Column order |
| `columns[*].data_type` | `string \| null` | Core | ✅ | — | Declared in YAML |
| `columns[*].declared_type` | `string \| null` | Core | ✅ | — | |
| `columns[*].inferred_type` | `string \| null` | Proprietary | ✅ | — | `null` in Core; populated by dbt v2 static analysis |
| `columns[*].catalog_type` | `string \| null` | Core-conditional | ✅ | `null` when catalog absent | Warehouse-verified type; `null` unless `dbt docs generate` ran |
| `columns[*].description` | `string \| null` | Core | ✅ | — | |
| `columns[*].label` | `string \| null` | Core | ✅ | — | |
| `columns[*].granularity` | `string \| null` | Core | ✅ | — | Semantic layer use |
| `depends_on` | `EdgeRef[]` | Core | ✅ | — | 1-hop upstream; see Risk #5 re: lineage bounding decision |
| `depends_on[*].unique_id` | `string` | Core | ✅ | — | |
| `depends_on[*].edge_type` | `string` | Core | ✅ | — | |
| `referenced_by` | `EdgeRef[]` | Core | ✅ | — | 1-hop downstream; see Risk #5 re: lineage bounding decision |
| `referenced_by[*].unique_id` | `string` | Core | ✅ | — | |
| `referenced_by[*].edge_type` | `string` | Core | ✅ | — | |
| `execution_info` | `ExecutionInfo \| null` | Core-conditional | ✅ | `null` when run results absent | `null` when `dbt_rt.run_results` has no rows for this model |
| `execution_info.status` | `string \| null` | Core-conditional | ✅ | — | `"success"` · `"error"` · `"skipped"` |
| `execution_info.completed_at` | `string \| null` | Core-conditional | ✅ | — | Derived from `created_at`; space-separated local-timezone format (see Risk #1) |
| `execution_info.execution_time` | `number \| null` | Core-conditional | ✅ | — | Seconds (float) |
| `catalog` | `CatalogInfo \| null` | Core-conditional | ✅ | `null` when catalog absent | `null` when `dbt.catalog_tables` has no rows for this model |
| `catalog.type` | `string \| null` | Core-conditional | ✅ | — | `"table"` · `"view"` · `"materialized view"`; maps from `table_type` column |
| `catalog.owner` | `string \| null` | Core-conditional | ✅ | — | Warehouse role; maps from `table_owner` column |
| `catalog.bytes_stat` | `number \| null` | Core-conditional | 🔧 | — | Always `null`; lives in `dbt.catalog_stats` with adapter-specific `stat_id` (see Risk #4) |
| `catalog.row_count_stat` | `number \| null` | Core-conditional | 🔧 | — | Always `null`; same as above (see Risk #4) |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; Discovery-API-internal — see Risk #7 |
| `usage_query_count` | *(absent)* | — | ❌ | — | Class B: no parquet path; Discovery-API-internal |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface ModelDetail {
  unique_id: string;
  name: string;
  resource_type: "model";
  package_name: string | null;
  description: string | null;
  original_file_path: string | null;
  file_path: string | null;
  tags: string[];
  fqn: string[];
  materialized: string | null;
  database_name: string | null;
  schema_name: string | null;
  relation_name: string | null;
  identifier: string | null;
  access_level: string | null;
  group_name: string | null;
  contract_enforced: boolean | null;
  raw_code: string | null;
  compiled_code: string | null;
  columns: ModelColumn[];
  depends_on: EdgeRef[];
  referenced_by: EdgeRef[];
  execution_info: ExecutionInfo | null;
  catalog: CatalogInfo | null;
}

interface ModelColumn {
  name: string;
  index: number | null;
  data_type: string | null;
  declared_type: string | null;
  inferred_type: string | null;
  catalog_type: string | null;
  description: string | null;
  label: string | null;
  granularity: string | null;
}

interface ExecutionInfo {
  status: string | null;
  completed_at: string | null;
  execution_time: number | null;
}

interface CatalogInfo {
  type: string | null;
  owner: string | null;
  bytes_stat: number | null;
  row_count_stat: number | null;
}

interface EdgeRef {
  unique_id: string;
  edge_type: string;
}
```

### Risk register

1. **`execution_info` implemented; `completed_at` format is approximate.** *(2026-05-18)*
   Implemented as a query against `dbt_rt.run_results` (not `run_results_latest` — no
   such pre-aggregated view found). Verified column names: `status` (Utf8), `execution_time`
   (Float64), `created_at` (timestamptz). `completed_at` is derived from `created_at` via
   `CAST(... AS VARCHAR)`, producing e.g. `"2026-05-14 17:41:56.652026-07"` (space separator,
   local timezone). The `timing` column holds a JSON array with per-phase UTC timestamps;
   extracting the execute-phase `completed_at` would give a cleaner ISO 8601 UTC value but
   adds DuckDB JSON path complexity. Deferred; acceptable for v0.

2. **`tags`, `fqn`, `contract_enforced` verified and implemented.** *(2026-05-18)*
   Confirmed against a real index: `tags` and `fqn` are native `List(Utf8)` columns —
   arrow_json serializes them as JSON arrays correctly. `contract_enforced` is a Boolean
   column. All three added to the handler SELECT.

3. **`compiled_code` confirmed present and implemented.** *(2026-05-18)*
   `compiled_code` is a `VARCHAR` column in `dbt.nodes` parquet. Enabled in the handler
   SELECT as `n.compiled_code`. For SQL models without macros, `compiled_code` equals
   `raw_code` (no template expansion needed). For models with `{{ ref(...) }}` calls,
   `compiled_code` contains the fully qualified SQL.

4. **Catalog column names corrected; `bytes_stat`/`row_count_stat` still open.** *(2026-05-18)*
   Verified `dbt.catalog_tables` schema: actual column names are `table_type` and
   `table_owner` (not `type`/`owner` as initially assumed — those would have caused 500s
   with real catalog data). Corrected in the handler. `bytes_stat` and `row_count_stat`
   do not exist in `catalog_tables`; they live in `dbt.catalog_stats` keyed by adapter-
   specific `stat_id` strings (e.g., `"bytes"`, `"num_bytes"` vary by adapter). Both
   are stubbed as `NULL::BIGINT` until a populated catalog index is available to confirm
   the stat IDs. The `catalog` object in the response will always have `null` for these
   two fields until that work is done.

5. **`depends_on`/`referenced_by` are intentionally unbounded.** *(Decision: 2026-05-18)*
   A `?first=` cap on a nested field is an API smell: every paginated request re-fetches
   all base fields as fixed overhead, and cursor state doesn't compose cleanly with a
   single-resource endpoint. The correct bounded path, if fan-out at scale requires it, is
   a dedicated lineage sub-resource (additive, backwards compatible). One caveat: silently
   **truncating** the inline arrays would be backwards incompatible by output even if the
   field name is preserved — clients that iterate `depends_on`/`referenced_by` would
   render incomplete graphs with no schema error to surface the problem. Therefore: keep
   unbounded until a sub-resource exists; never truncate the inline arrays without
   deprecating them first.

6. **`access_level` enum values need verification.** dbt-ui uses `AccessLevel`
   (`public | protected | private`). The current field is `string | null`. Confirm the
   string values match before the FE renders access badges to avoid silent mismatches.

7. **`health_issues` is Class B — no parquet path.** It is `subGraphs: ['internal']` in
   codex-api AND absent from all 34 parquet tables. The FE must render a graceful null
   state; do not add the field. Document explicitly so FE engineers don't chase it.

8. **Per-node test list has no handler coverage.** Requires a join across
   `dbt.test_metadata` and `dbt_rt.run_results`. **Open question:** inline a test summary
   array in `ModelDetail`, or introduce `GET /api/v1/models/:id/tests`? Resolve before
   implementing.

---

## ADR-3: `GET /api/v1/tests/:id` response shape for `test` vs `unit_test`

**Status:** Decided — single endpoint, discriminated union on `resource_type`.
**Trigger to revisit:** Unit tests and generic tests diverge enough to require a
dedicated UI page (currently both render via `TestView.tsx`).

### Context

`GET /api/v1/tests/:id` must serve two structurally distinct resource types:

| `resource_type` | `unique_id` prefix | Distinctive fields |
|---|---|---|
| `test` | `test.` | `column_name`, `test_metadata.kwargs`, `status`, `error` |
| `unit_test` | `unit_test.` | `given` rows, `expect` rows |

Both fold into the same "Tests" tab in dbt-ui (`TestView.tsx` handles both).
ADR-1 chose type-specific endpoints over a generic discriminated union across all
resource types — but `GET /api/v1/tests/:id` is itself a union of two sub-types.

### Options considered

**Single endpoint, discriminated union on `resource_type`**

`TestDetail` is `GenericTestDetail | UnitTestDetail`, narrowed by `resource_type`.
The two types share a common base and extend it with type-specific fields.

- Pro: one endpoint, one fetch per test detail page.
- Pro: both types are conceptually "test results" rendered on the same page — the
  distinction is an implementation detail, not a product-level one.
- Pro: consistent with how `GET /api/v1/nodes/:id` (deferred) would dispatch.
- Con: the response type is a union; FE must narrow on `resource_type`.

**Two endpoints: `/tests/:id` and `/unit_tests/:id`**

- Pro: fully separate types, no narrowing needed.
- Pro: strictest alignment with ADR-1's "one type per endpoint" principle.
- Con: FE must inspect `resource_type` (or parse the `unique_id` prefix) before
  routing to the right endpoint — the branching just moves to the call site.
- Con: tests and unit tests share a page and a concept; splitting endpoints
  diverges from the user's mental model.

→ **Single endpoint chosen.** There is no world where tests and unit tests belong
on separate detail pages. The union is an exception to ADR-1's general principle,
justified by the fact that `test` and `unit_test` are the same concept (test
coverage of a model) rendered identically in the UI.

---

## `GET /api/v1/nodes/:id` (deferred)

**Status:** Deferred — no v0 UI use case identified.
**Trigger to add:** MCP is added to dbt-docs-server.

When added: parse the `unique_id` prefix (`model.` → models handler, `source.` →
sources handler, etc.) and delegate to the appropriate typed handler. No logic
duplication. OpenAPI response type: `oneOf [ModelDetail, SourceDetail, ...]` with
`resource_type` as discriminator.

**Precondition:** `NodeBase` struct must already exist (ADR-1 backend prerequisite).

---

## ADR-4: `execution_info` field naming — single-run semantics

**Status:** Decided — bare field names without `last_run_*` or phase-scoped prefixes.
**Trigger to revisit:** Multi-run history (last N runs) becomes a product requirement.

dbt-docs-server is a **snapshot server**, not a history server. Every query reflects a
single indexed state: the output of one `dbt build` / `dbt seed` / `dbt snapshot` run
captured in parquet files at `<target>/index/`. There is no run timeline, no "previous
run" to contrast with a "last run." The word "last" implies a sequence; dbt-docs-server
exposes only the current snapshot.

Discovery API fields like `lastRunStatus`, `executeCompletedAt`, and `lastRunError` carry
prefixes because CodexDB has access to full run history. Importing those prefixes into
dbt-docs-server would be semantically misleading — `last_run_status` implies there could
be a `second_to_last_run_status`. The `execute_` prefix on `executeCompletedAt` is an
internal timing-phase name (compile vs. execute) that has no relevance to API consumers.

`lastKnownResult` (Discovery API for tests) tracks whether a test passed *before* a schema
change invalidated it — a concept that requires run history to be meaningful. In
dbt-docs-server the index is always one coherent snapshot: either the test ran and `status`
reflects the result, or the test hasn't run and `execution_info` is `null`. The "known vs.
actual" distinction does not exist and the field is **dropped**.

### Decision

`execution_info` fields use bare names:

| Discovery API field | dbt-docs-server field | Reason |
|---|---|---|
| `lastRunStatus` | `status` | No "last" — only one indexed run |
| `executeCompletedAt` | `completed_at` | `execute_` is an internal timing phase name |
| `lastRunError` | `error` | Same prefix problem |
| `lastKnownResult` | *(dropped)* | Requires run history; meaningless in snapshot world |

### If multi-run history is ever required

The path is a **new `runs[]` sub-resource**, not retrofitting `last_*` prefixes onto
existing fields. For example: `GET /api/v1/models/:id/runs` returns `Run[]` where each
`Run` has `status`, `completed_at`, `error`, `execution_time`. The inline `execution_info`
on the detail response becomes a convenience shortcut for the most-recent entry. This is
an additive change with no breaking impact on existing contracts.

---

## ADR-5: `execution_info` omission for definition-only and Semantic Layer resources

**Status:** Decided — `execution_info` is **omitted entirely** (not null-gated) on detail responses for resource types that never produce a `dbt_rt.run_results` row.
**Trigger to revisit:** dbt-mantle's Semantic Layer service exposes per-resource SL query history that can be backfilled into `dbt_rt.run_results` for `metric.*` / `saved_query.*` / `semantic_model.*` unique_ids.

### Why a new ADR

ADR-2 settled the *placement* of `execution_info` (inline, null when no `dbt_rt.run_results` row exists for the resource — see ADR-7). ADR-2 implicitly assumed every resource type runs — true for models, sources (as test parents), seeds, snapshots, tests. False for the six resource types contracted in this PR.

A strict reading of ADR-2 would force `execution_info: null` onto every exposure / group / macro / metric / saved_query / semantic_model response in perpetuity, with no project state that would ever flip it to non-null. That's a "this field is dead by construction" surface — the FE has to render the null branch, the TypeScript type has to carry the optional, and reviewers wonder why a runnable's empty-state shape appears on a resource that never runs.

The honest contract is to omit the field entirely from the type and the wire response, the way `SourceDetail` omits `depends_on` (sources have no upstream) and `SeedDetail` omits `materialized` (seeds aren't materialized in the dbt sense). This ADR codifies that.

### Decision

The following resource types **do not carry `execution_info` on their detail response** and do not have a `Core-conditional` row for run state in their field reference:

| Resource type | Why no `execution_info` |
|---|---|
| `exposure` | Not executed by dbt; declarative YAML pointing at downstream consumers (BI tools, ML jobs). No `dbt_rt.run_results` row. |
| `group` | Definition-only metadata. `dbt build` emits run results for member models, not for the group itself. |
| `macro` | Template; never materialized. The macro's *invocations* run; the macro doesn't. |
| `metric` | Semantic Layer definition. Executed at SL-query time against `dbt-mantle`, not by `dbt build`. |
| `saved_query` | Semantic Layer definition. Same execution model as `metric`. |
| `semantic_model` | Spec-only. Declares entities/dimensions/measures on top of an existing model; not itself executed. |

The "last updated" header timestamp these resources would otherwise want from `execution_info.completed_at` is sourced from `created_at` (epoch seconds, from each table's `dbt.<table>.created_at` column). Groups fall back to `ingested_at` (no `created_at` column on `dbt.groups`).

### Backend prerequisite — `NodeBase` split

ADR-1 required that all typed detail handlers compose a shared `NodeBase` struct. Now that ADR-5 carves out a class of resources that don't carry `execution_info` (and groups don't even carry `fqn`), `NodeBase` is split into three: a shared `NodeBase` for fields every resource type has, a `RunnableNodeBase` that adds `tags`, `fqn`, and `execution_info` for resource types that `dbt build` actually runs, and a `DefinitionNodeBase` that adds `tags`, `fqn`, and `created_at` for definition-only resources (`GroupDetail` composes `NodeBase` directly because `dbt.groups` has no `fqn`).

```rust
// Common to every resource type.
struct NodeBase {
    unique_id: String,
    name: String,
    resource_type: String,
    package_name: Option<String>,
    description: Option<String>,
    original_file_path: Option<String>,
}

// Composed by ModelDetail, SourceDetail, SeedDetail, SnapshotDetail, TestDetail.
// Carries the fields that only apply to resources dbt actually runs.
struct RunnableNodeBase {
    #[serde(flatten)] base: NodeBase,
    tags: Vec<String>,
    fqn: Vec<String>,
    execution_info: Option<ExecutionInfo>,   // null when dbt_rt.run_results has no row
}

// Composed by ExposureDetail, MacroDetail, MetricDetail, SavedQueryDetail,
// SemanticModelDetail. GroupDetail composes NodeBase directly (no fqn either).
struct DefinitionNodeBase {
    #[serde(flatten)] base: NodeBase,
    tags: Vec<String>,           // sourced from `config` JSON for some resources
    fqn: Vec<String>,            // empty for groups
    created_at: Option<f64>,     // epoch seconds; groups fall back to ingested_at
}
```

### What this changes in the per-endpoint contracts

Each ADR-5–scoped endpoint's field reference table:
- **may** include an `❌ absent` row for `execution_info` documenting *why* it's not in the response shape (preserves the audit trail; the field is still absent from `DefinitionNodeBase`, the example response, and the TypeScript type). Existing contracts use this pattern — keep it.
- **must** include a `created_at: number | null` row (Core 🔧) sourced from the resource's parquet `created_at` column. **Exception:** `dbt.groups` has no `created_at` column — the groups contract surfaces `ingested_at` instead, with a row note documenting the fallback.

The per-endpoint Design notes for each of the six resource types that flag "no execution_info because definition-only" are now redundant with this ADR; they remain in place as supporting context but reviewers should treat ADR-5 as the source of truth.

---

## ADR-6: List + facets endpoint envelope

**Status:** Decided — Relay-style cursor pagination (`first` / `after` + `page_info`); `/facets` returns distinct filter values.
**Trigger to revisit:** A consumer requires backward navigation (`before` / `start_cursor`); a consumer needs a total-row count badly enough to justify a separate count query.

### Why cursor and not offset

CC-4 has documented cursor as the standard direction since the inception of this doc. An earlier draft of this ADR proposed offset/limit for parity with the existing `list_models` handler — that was parity-convenience, not a principled choice. Three reasons cursor is correct here:

1. **dbt-ui already speaks cursor.** Discovery API + the `usePaginatedQuery` reducer consume `{first, after}` and `{pageInfo: {endCursor, hasNextPage}}`. Offset pagination forces the FE to use two pagination shapes side-by-side.
2. **Read-only parquet snapshot makes stable cursors free.** Data is loaded once at server boot into in-memory DuckDB; there are no inserts/deletes during a session. The hardest property of cursor pagination on a live store — stability across writes — does not need to be defended here.
3. **Every resource has a stable tie-breaker.** All rows carry `unique_id`. The composite `(sort_value, unique_id)` cursor is unambiguous; DuckDB supports tuple comparison natively (`WHERE (a, b) > (?, ?)`).

The existing `list_models` handler is migrated in a separate PR off `main` (`meta-7432/models-list-cursor-pagination`); ADR-6 and the handler land in lockstep.

### Response envelope (LIST)

```json
{
  "data": [ /* item summaries */ ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoibW9kZWwuYWxwaGEiLCJpIjoibW9kZWwuanMuYWxwaGEifQ",
    "end_cursor": "eyJzIjoibW9kZWwub3JkZXJzIiwiaSI6Im1vZGVsLmpzLm9yZGVycyJ9",
    "has_next_page": true
  }
}
```

The `page_info` shape mirrors GraphQL Relay's `PageInfo` connection spec (snake_case per CC-1) plus the commonly-added `totalCount` extension:

- **`total_count`** (`number`): total row count under the current filter set, ignoring `first` and `after`. Computed via a separate `COUNT(*)` query per request. Adds a constant-time cost (DuckDB over parquet column statistics is fast); accepted as the price of consumer convenience. If profiling ever shows this materially impacts page latency, move it behind a `?include_total=true` opt-in additively.
- **`start_cursor`** (`string | null`): opaque base64 cursor pointing to the position of the FIRST row of the current page. `null` when `data` is empty. Useful for "anchor here" UX or for clients that cache pages keyed by their start cursor. Symmetric with `end_cursor`.
- **`end_cursor`** (`string | null`): opaque base64 cursor pointing to the position of the LAST row of the current page. `null` when `data` is empty OR when `has_next_page` is `false`. Clients pass it back via `?after=...` unmodified to fetch the next page; the server reserves the right to change the internal shape at any time.
- **`has_next_page`** (`boolean`): `true` when there is at least one more row after the returned page. Implementation: query `LIMIT first + 1` and trim the last row if present; emit `has_next_page = (returned_rows == first + 1)`.

Top-level array is keyed `data` for every resource — generic so one FE / MCP client can consume any list endpoint without a per-resource discriminator.

`has_previous_page` is intentionally omitted from v0 — only forward navigation is supported, and the dbt-ui `usePaginatedQuery` reducer only paginates forward. Including `start_cursor` already gives consumers what they need to build "anchor"/back-to-top behavior client-side; full backward pagination (via `?before=<start_cursor>`) would be additive when a real consumer needs it.

### Cursor encoding

Cursors are opaque to the client. Server-side, the cursor is base64-encoded JSON with two fields: the value of the primary sort column for the last row of the previous page, and that row's `unique_id` as the tie-breaker. For a sort spec `<col>:<dir>`:

```sql
-- First page (no ?after):
SELECT … ORDER BY <col> <dir> NULLS LAST, unique_id ASC LIMIT first+1

-- Subsequent page (after cursor decodes to (sort_val, uid)):
SELECT … WHERE (<col>, unique_id) > (?sort_val, ?uid)   -- semantics depend on sort direction & NULLS LAST
        ORDER BY <col> <dir> NULLS LAST, unique_id ASC LIMIT first+1
```

For `:desc` sorts and nullable sort columns the predicate uses an OR-of-conjunctions form to handle NULL bucketing correctly; see the shared `src/handlers/pagination.rs` helper (added by the companion PR).

### Query parameters (LIST)

| Param | Type | Default | Notes |
|---|---|---|---|
| `first` | u32 | 100 (max 1000) | Server clamps. The earlier offset envelope had `limit=1000/max=5000`; cursor pagination implies clients page progressively, so the per-page default is smaller. Resources are free to override the default and the max per-resource in their contract section. |
| `after` | string (opaque base64) | — | The `end_cursor` returned by the previous page. Omit for the first page. Tampering yields 400 `invalid cursor`. |
| `sort` | string | per-resource (default `name:asc` unless the resource section says otherwise) | `<column>:<asc\|desc>`; columns are an allowlist per resource. The sort columns must be deterministic enough that `(sort_val, unique_id)` uniquely orders every row. |
| `<filter>` | string (CSV) | none | Comma-separated values OR'd within a filter. Filter names per resource. |

`?limit` and `?offset` from the offset envelope are NOT accepted. Passing them returns 400 with a body that points at this ADR to help client migration.

### Response envelope (FACETS)

```json
{
  "<filter_name>": [ { "value": "Marts", "count": null } ]
}
```

- One key per documented filter for the resource. Order is alphabetical by filter name.
- `count` is `null` today — reserved for a future enhancement that counts rows per facet.
- If a resource has no filters in dbt-ui, the facets endpoint still exists and returns `{}`.
- Facets are unpaginated; the filter universe per resource is bounded (single-digit databases, schemas, owners, etc.).

### Shared TypeScript types

`PageInfo` is the canonical envelope-suffix type used by every LIST endpoint's response. It is declared **once here** rather than repeated in each LIST contract's Type definition block — each LIST contract's TypeScript fragment refers to `PageInfo` by name and trusts this declaration.

```typescript
// Shared envelope type used by every LIST endpoint (ADR-6).
interface PageInfo {
  total_count: number;
  start_cursor: string | null;
  end_cursor: string | null;
  has_next_page: boolean;
}
```

When a per-resource LIST contract's TypeScript references `page_info: PageInfo;`, it points at this single definition. Updating the shape (additive fields, new types) happens here; every contract picks it up by reference.

### Why one ADR, not ten

Lifting cursor pagination into doctrine prevents the ten new resource contracts from re-deriving pagination param names, cursor formats, and sort grammar inconsistently. Test-suite parity, FE client generation, and docs all benefit from one canonical envelope.

### Risks specific to cursor pagination

These apply to every LIST handler; resource-specific contracts may add more.

1. **Cursor stability is server-snapshot-scoped.** Cursors are valid for the lifetime of the current `dbt docs serve` process. Restarting the server invalidates outstanding cursors — clients see a 400 `invalid cursor` and must restart pagination. This matches the read-only-snapshot model. If `dbt docs serve` reloads parquet at runtime (out of scope today), this guarantee must be re-evaluated.
2. **Tampered cursors must fail closed.** The cursor payload is opaque but not signed. A malformed or out-of-domain cursor (decode error, type mismatch, sort_val type drift) must return 400; never silently re-interpret as a fresh first-page request — that masks client bugs.
3. **`(sort_val, unique_id)` must be a deterministic order.** If the sort allowlist for a resource permits a non-deterministic column (e.g., a freely-typed string that may have ties on duplicate values), the `unique_id` tie-breaker preserves total order. Resources whose primary sort column is itself unique (e.g., `unique_id` directly) can elide the tie-breaker SQL but must still emit it in the cursor.
4. **Null sort values require ordered NULL semantics.** Use `NULLS LAST` for ASC and `NULLS LAST` for DESC by convention; encode the cursor with an explicit null marker so the cursor predicate puts post-null rows correctly. Implementation lives in `src/handlers/pagination.rs`.

---

## ADR-8: Unified `GET /api/v1/search` endpoint as the documented exception to ADR-1

**Status:** Decided — one cross-resource search endpoint; per-type search variants rejected.
**Trigger to revisit:** A second cross-resource surface (e.g., global "ask anything" lookup, AI agent ingest) needs a different shape, OR per-type relevance tuning becomes a product requirement that cannot be expressed against a uniform pipeline.

### Context

ADR-1 mandated type-specific endpoints (`/api/v1/models/:id`, `/api/v1/sources/:id`, etc.) for the detail surface, rejecting a generic dispatcher. That decision was anchored on the detail-page use case, where the FE always knows the resource type from routing and per-type TypeScript narrowing imposes a non-trivial UX tax at every call site.

Project search has the opposite shape:

- The user types one query into one search box (`/proj/search/?search=<term>`).
- The UI renders **one mixed-type result list** — `SearchResultsList.tsx` iterates a single `SearchResultDisplayData[]`, with the hit's `resource_type` driving only a per-row `ResourceChip` and the optional "View lineage" link.
- `SearchResultsContents.tsx` calls **one** GraphQL hook (`useSearchResults` / `GetAppliedSearchResults`) that returns interleaved `models | sources | tests | seeds | snapshots | exposures | metrics | semantic_models | saved_queries | macros` edges in a single `SearchResult` envelope (`appliedSearch.ts`).
- Pagination, the result-count badge, and the filter pills are all global, not per-type.

Applying ADR-1 literally would require N per-type search endpoints (`/api/v1/models/search`, `/api/v1/sources/search`, …). The SPA would fan out N requests, reassemble the interleaved list client-side, and re-implement total-count and pagination across N independent cursors. That is the wrong primitive for this UI — the cross-resource result stream is the product.

### Options considered

**Per-type search endpoints: `/api/v1/models/search`, `/api/v1/sources/search`, etc.**

Each existing detail-endpoint family gets a sibling `/search` route returning that type's hits.

- Pro: strict consistency with ADR-1.
- Pro: each endpoint's response type is a clean, non-union shape.
- **Con:** the SPA must fan out N requests per keystroke and merge results — direct violation of the "one search box → one result list" UX.
- **Con:** total-count, cursor pagination, and ranking become client-reassembly problems with no single source of truth.
- **Con:** filter overlap with the existing `ResourceFilterPanel` (which already targets a global mixed list) is awkward.

→ **Rejected.** The cost is paid at every keystroke; the benefit (ADR-1 purity) accrues only to docs-server maintainers.

**Per-type search endpoints + client-side fan-out helper**

Same as above, with a shared client utility that fans out and merges.

- Pro: keeps server endpoints per-type and uniform.
- Con: pushes the cross-resource pagination/ranking design into the client, which is exactly the design we need to settle once on the server. The "helper" becomes a load-bearing piece of FE code with no test ownership at the server boundary.

→ **Rejected.** Pushes the hard problem (mixed-type ordering and cursor consistency) out of the typed contract layer.

**Unified endpoint: `GET /api/v1/search`**

One endpoint, one envelope, polymorphic `hit` shape discriminated on `resource_type`.

- Pro: matches the UI's "one box → one list" mental model and Discovery API's `AppliedSearch` shape.
- Pro: total-count, cursor pagination (CC-4), and ranking are decided once on the server.
- Pro: the filter taxonomy (`?type=`, `?package=`) is a single contract, not N parallel ones.
- Con: the response carries a polymorphic `hit` type — but the existing `SearchResultHit` TypeScript shape in `SearchResultsList.tsx` is already polymorphic-by-`resourceType`, so this matches the UI's narrowing pattern rather than adding new tax.
- Con: cursor pagination across a UNION of parquet tables requires a stable total ordering decision (see Q-E9 in the endpoint section).

→ **Chosen for v0.** Cross-resource is the product; encoding it as a single typed contract is honest. ADR-1 stands for every other endpoint family.

### Decision

`GET /api/v1/search` is the single REST surface for free-text project search. It is the **only documented exception** to ADR-1's type-specific-endpoint rule, and only because the UI consumes a cross-resource result stream as a first-class primitive. No other endpoint added under this exception without a new ADR.

The endpoint:

- Returns a `data[]` array of `{ matched_field, highlight, hit }` envelopes (CC-2: highlight metadata is a sibling of `hit`, not flattened into it).
- Paginates per CC-4 (`?first=&after=`, `page_info.end_cursor`, `page_info.has_next_page`).
- Discriminates `hit` on `resource_type`, with a shared base shape and additive type-specific fields (mirrors ADR-3's union pattern for tests).
- Inherits all cross-cutting conventions (CC-1 snake_case, CC-3 capability gating, CC-5 field classification).

The endpoint contract is at [`GET /api/v1/search`](#get-apiv1search) below.

### Trigger conditions for revisiting

- A second cross-resource surface (e.g., AI agent ingest, global account-wide lookup) needs a materially different envelope shape — at that point the "unified" abstraction has more than one consumer and the precedent should be re-examined.
- Per-type relevance tuning (`/models/search` ranks `name` higher than `/sources/search` does) becomes a product requirement that cannot be expressed against a uniform ranking pipeline.
- Body-search expansion (Q-E3 option b) becomes load-bearing and the resulting query cost asymmetry between types makes per-type endpoints cheaper to reason about.

None of these triggers apply to v0. Default for the foreseeable future: this single endpoint, in this shape.

---

## `GET /api/v1/sources/:id`

Powers: `SourceView` / `ResourceDetailsPage` in dbt-ui.
dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/SourceView.tsx`
GraphQL hooks: `packages/metadata/dbt-explorer/src/hooks/dbtStrategy/useSource.ts` → `src/hooks/discovery/source.ts` (`GetSourceByUniqueId`)

**No new ADR needed.** This endpoint follows ADR-1 (type-specific) and ADR-2 (conditional
data inlined, null-gated by capability) without exception. `freshness` replaces
`execution_info` as the Core-conditional surface for sources.

### Example response

`freshness` is `null` when `dbt.source_freshness` has no row for this source (i.e., `dbt source freshness` has not run for it).
`catalog` is `null` when `dbt.catalog_tables` has no row for this source (i.e., `dbt docs generate` has not run).
Fields marked `// 🔧` are not yet returned — they require a backend change.

```json
{
  "unique_id": "source.jaffle_shop.raw_jaffle.orders",
  "name": "orders",
  "resource_type": "source",
  "package_name": "jaffle_shop",
  "description": "Raw orders table from the production Postgres database.",
  "original_file_path": "models/staging/sources.yml",
  "file_path": "models/staging/sources.yml",
  "tags": ["raw", "jaffle"],
  "fqn": ["jaffle_shop", "raw_jaffle", "orders"],
  "database_name": "raw",
  "schema_name": "jaffle_shop",
  "identifier": "orders",
  "source_name": "raw_jaffle",
  "source_description": "Raw tables synced from the Jaffle Shop production database.",
  "loader": "fivetran",
  "meta": { "owner": "data-eng" },
  "referenced_by": [
    { "unique_id": "model.jaffle_shop.stg_orders", "edge_type": "model" }
  ],
  "columns": [
    {
      "name": "id",
      "index": 0,
      "data_type": "integer",
      "declared_type": "int",
      "inferred_type": null,
      "catalog_type": "INT64",
      "description": "Unique order identifier.",
      "label": null,
      "granularity": null
    }
  ],
  "freshness": {
    "status": "pass",
    "snapshotted_at": "2026-05-15T10:00:00Z",
    "max_loaded_at": "2026-05-15T09:45:00Z",
    "max_loaded_at_time_ago": 900.0,
    "criteria": {
      "error_after": { "count": 24, "period": "hour" },
      "warn_after": { "count": 12, "period": "hour" }
    }
  },
  "catalog": {
    "type": "table",
    "owner": "fivetran",
    "comment": "Raw orders synced from production PostgreSQL.",
    "primary_key": ["id"],
    "row_count_stat": 50000,
    "bytes_stat": 2097152,
    "stats": [
      {
        "id": "has_stats",
        "label": "Has Stats?",
        "value": "true",
        "description": "Indicates whether there are statistics for this table",
        "include": false
      }
    ]
  }
}
```

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | ✅ | — | e.g., `"source.pkg.source_name.table_name"` — 4-part unique_id |
| `name` | `string` | Core | ✅ | — | Table name within the source block |
| `resource_type` | `"source"` | Core | ✅ | — | Always `"source"` for this endpoint |
| `package_name` | `string \| null` | Core | ✅ | — | |
| `description` | `string \| null` | Core | ✅ | — | Per-table description from YAML |
| `original_file_path` | `string \| null` | Core | ✅ | — | Path to the `.yml` file containing the source definition |
| `file_path` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.file_path`; for sources equals `original_file_path` (the same `.yml`, since sources are YAML-only) |
| `tags` | `string[]` | Core | 🔧 | — | In `dbt.nodes` parquet; add to handler SELECT |
| `fqn` | `string[]` | Core | 🔧 | — | In `dbt.nodes` parquet; 3-part for sources: `[pkg, source_name, table]` |
| `database_name` | `string \| null` | Core | ✅ | — | |
| `schema_name` | `string \| null` | Core | ✅ | — | |
| `identifier` | `string \| null` | Core | ✅ | — | Overrides table name if set; falls back to `name` |
| `source_name` | `string \| null` | Core | 🔧 | — | dbt source block name (e.g., `"raw_jaffle"`) — in `dbt.nodes` parquet |
| `source_description` | `string \| null` | Core | 🔧 | — | Block-level description from YAML — in `dbt.nodes` parquet |
| `loader` | `string \| null` | Core | 🔧 | — | e.g., `"fivetran"`, `"airbyte"` — in `dbt.nodes` parquet |
| `meta` | `Record<string, unknown> \| null` | Core | 🔍 | — | JSONB blob — confirm `dbt.nodes` parquet includes a `meta` column |
| `referenced_by` | `EdgeRef[]` | Core | ✅ | — | Downstream models; sources have **no** `depends_on` |
| `referenced_by[*].unique_id` | `string` | Core | ✅ | — | |
| `referenced_by[*].edge_type` | `string` | Core | ✅ | — | |
| `columns` | `SourceColumn[]` | Core | ✅ | — | Identical shape to `ModelColumn[]` |
| `columns[*].name` | `string` | Core | ✅ | — | |
| `columns[*].index` | `number \| null` | Core | ✅ | — | |
| `columns[*].data_type` | `string \| null` | Core | ✅ | — | Declared in YAML |
| `columns[*].declared_type` | `string \| null` | Core | ✅ | — | |
| `columns[*].inferred_type` | `string \| null` | Proprietary | ✅ | — | `null` in Core; populated by dbt v2 static analysis |
| `columns[*].catalog_type` | `string \| null` | Core-conditional | ✅ | — | Warehouse-verified type |
| `columns[*].description` | `string \| null` | Core | ✅ | — | |
| `columns[*].label` | `string \| null` | Core | ✅ | — | |
| `columns[*].granularity` | `string \| null` | Core | ✅ | — | |
| `freshness` | `FreshnessInfo \| null` | Core-conditional | 🔧 | — | `null` if `dbt source freshness` hasn't run — see Risk #2 |
| `freshness.status` | `string` | Core-conditional | 🔧 | — | `"pass"` · `"warn"` · `"error"` · `"runtime error"` |
| `freshness.snapshotted_at` | `string \| null` | Core-conditional | 🔧 | — | ISO 8601; when freshness was last checked |
| `freshness.max_loaded_at` | `string \| null` | Core-conditional | 🔧 | — | ISO 8601; most recent row timestamp from the source table |
| `freshness.max_loaded_at_time_ago` | `number \| null` | Core-conditional | 🔧 | — | Seconds elapsed since `max_loaded_at` |
| `freshness.criteria.error_after.count` | `number \| null` | Core-conditional | 🔧 | — | |
| `freshness.criteria.error_after.period` | `string \| null` | Core-conditional | 🔧 | — | `"minute"` · `"hour"` · `"day"` |
| `freshness.criteria.warn_after.count` | `number \| null` | Core-conditional | 🔧 | — | |
| `freshness.criteria.warn_after.period` | `string \| null` | Core-conditional | 🔧 | — | `"minute"` · `"hour"` · `"day"` |
| `catalog` | `SourceCatalogInfo \| null` | Core-conditional | 🔧 | — | Superset of model `CatalogInfo` — adds `comment`, `primary_key`, `stats[]` |
| `catalog.type` | `string \| null` | Core-conditional | 🔧 | — | |
| `catalog.owner` | `string \| null` | Core-conditional | 🔧 | — | |
| `catalog.comment` | `string \| null` | Core-conditional | 🔧 | — | Warehouse table comment — source-only field |
| `catalog.primary_key` | `string[]` | Core-conditional | 🔧 | — | Column names constituting the PK; empty array if none. Sourced from `dbt.nodes.primary_key` (a `List<String>` column) — not from `dbt.catalog_tables`, which has no `primary_key` column — source-only field |
| `catalog.row_count_stat` | `number \| null` | Core-conditional | 🔧 | — | |
| `catalog.bytes_stat` | `number \| null` | Core-conditional | 🔧 | — | |
| `catalog.stats` | `CatalogStat[]` | Core-conditional | 🔧 | — | Arbitrary warehouse statistics — source-only field |
| `catalog.stats[*].id` | `string` | Core-conditional | 🔧 | — | Stat identifier |
| `catalog.stats[*].label` | `string` | Core-conditional | 🔧 | — | Human-readable label |
| `catalog.stats[*].value` | `string` | Core-conditional | 🔧 | — | Always a string; parse as number if needed |
| `catalog.stats[*].description` | `string` | Core-conditional | 🔧 | — | |
| `catalog.stats[*].include` | `boolean` | Core-conditional | 🔧 | — | Whether the stat should be displayed in the UI |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api |
| `patch_path` | *(absent)* | — | ❌ | — | Class B: YAML-only resource — `original_file_path` IS the `.yml` file containing the source definition; the patch concept does not apply (a "patch" is a separate YAML that augments a non-YAML primary definition, e.g. `.sql` + `schema.yml`). Discovery's `patchPath` would be null or duplicate `originalFilePath` for this resource. |

**Fields absent from `SourceDetail` that exist on `ModelDetail`:**
Sources have no SQL, no materialization strategy, no dbt-managed relation, and no run
execution. The following fields from `ModelDetail` are intentionally omitted:
`materialized`, `relation_name`, `access_level`, `group_name`, `contract_enforced`,
`raw_code`, `compiled_code`, `depends_on`, `execution_info`.

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface SourceDetail {
  unique_id: string;
  name: string;
  resource_type: "source";
  package_name: string | null;
  description: string | null;
  original_file_path: string | null;
  file_path: string | null;
  tags: string[];
  fqn: string[];
  database_name: string | null;
  schema_name: string | null;
  identifier: string | null;
  source_name: string | null;
  source_description: string | null;
  loader: string | null;
  meta: Record<string, unknown> | null;
  referenced_by: EdgeRef[];
  columns: SourceColumn[];
  freshness: FreshnessInfo | null;
  catalog: SourceCatalogInfo | null;
}

// SourceColumn is identical in shape to ModelColumn
interface SourceColumn {
  name: string;
  index: number | null;
  data_type: string | null;
  declared_type: string | null;
  inferred_type: string | null;
  catalog_type: string | null;
  description: string | null;
  label: string | null;
  granularity: string | null;
}

interface FreshnessInfo {
  status: string;
  snapshotted_at: string | null;
  max_loaded_at: string | null;
  max_loaded_at_time_ago: number | null;
  criteria: {
    error_after: { count: number | null; period: string | null } | null;
    warn_after: { count: number | null; period: string | null } | null;
  } | null;
}

// SourceCatalogInfo extends model CatalogInfo with source-specific fields
interface SourceCatalogInfo {
  type: string | null;
  owner: string | null;
  comment: string | null;
  primary_key: string[];
  row_count_stat: number | null;
  bytes_stat: number | null;
  stats: CatalogStat[];
}

interface CatalogStat {
  id: string;
  label: string;
  value: string;
  description: string;
  include: boolean;
}

// EdgeRef is shared with ModelDetail
interface EdgeRef {
  unique_id: string;
  edge_type: string;
}
```

### Risk register

1. **`source_name`, `source_description`, `loader` not yet queried.** These are
   source-specific fields in `dbt.nodes` parquet that aren't in the current handler
   SELECT. Add them alongside `tags`, `fqn`, and `contract_enforced` in the same
   handler change.

2. **Freshness parquet coexistence is unverified.** `dbt.source_freshness.parquet`
   contains all the fields needed, but it's written by a separate command (`dbt source
   freshness`) from the one that writes `dbt.nodes.parquet` (`dbt build` / `dbt parse`).
   The question from FEATURE-TO-ENDPOINT-MAPPING.md Appendix B1: does `dbt --use-index
   source freshness` use MergePrune semantics that preserve `nodes.parquet`, or does it
   overwrite the index directory? If it overwrites: freshness is not available in
   stateless docs and must be treated as Platform-tier. **Verify with the dbt-index team
   before relying on `dbt.source_freshness` as the freshness data source.**

3. **`meta` JSONB presence in parquet is unverified.** The `meta` field is a JSONB
   object in codex-api's Prisma schema. Confirm it's serialized into `dbt.nodes.parquet`
   as a JSON string column before adding it to the SELECT.

4. **`SourceCatalogInfo` is a superset of `CatalogInfo`.** The model catalog type
   (`CatalogInfo`) does not include `comment`, `primary_key`, or `stats[]`. The Rust
   handler will need a separate response struct for source catalog data, or `CatalogInfo`
   must be extended. Decide before implementing to avoid a breaking change to `ModelDetail`.

5. **`catalog.stats[]` schema is warehouse-dependent.** The stat entries (e.g.,
   `has_stats`, `row_count`, `bytes`) vary by adapter. The `value` field is always a
   string regardless of the underlying type — document this explicitly so FE engineers
   don't attempt numeric parsing without a string-to-number conversion.

6. **Sources have no `depends_on`.** The current `NodeDetail` handler always returns
   both `depends_on` and `referenced_by`. The `SourceDetail` handler must omit
   `depends_on` entirely (not return an empty array) to avoid FE engineers
   misinterpreting an empty array as "no upstream sources found."

## `GET /api/v1/seeds/:id`

Powers: `SeedView` / `ResourceDetailsPage` in dbt-ui.
dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/SeedView.tsx`
GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/discovery/seed.ts` (`GetSeedByUniqueId`)

Seeds are CSV files loaded into the warehouse by `dbt seed` / `dbt build`. They share
the `dbt.nodes` parquet row structure with models and snapshots, but have no SQL body
(`raw_code` and `compiled_code` absent), no materialization strategy (`materialized`
absent), and no upstream dependencies (`depends_on` omitted — not an empty array).
Seeds DO have `execution_info`, `columns`, and `catalog`. `identifier` maps to
`dbt.nodes.alias` for seeds (the field that overrides the CSV filename to set the
warehouse table name). The per-seed `tests[]` inline array is deferred to a future
`GET /api/v1/seeds/:id/tests` sub-resource — mirrors the open question from
`ModelDetail` Risk #8.

### Example response

Fields marked `// conditional` are `null` when their capability gate is absent.
Fields marked `// 🔧` are not yet returned — they require a backend change.
Fields marked `// 🔍` are parquet-unverified — confirm schema before implementing.

```json
{
  "unique_id": "seed.jaffle_shop.raw_customers",
  "name": "raw_customers",
  "resource_type": "seed",
  "package_name": "jaffle_shop",
  "description": "Raw customer seed file loaded from CSV.",
  "original_file_path": "seeds/raw_customers.csv",
  "file_path": "raw_customers.csv",
  "patch_path": "seeds/_schema.yml",
  "tags": ["raw", "seed"],
  "fqn": ["jaffle_shop", "raw_customers"],
  "database_name": "prod",
  "schema_name": "dbt_prod",
  "identifier": "raw_customers",
  "meta": { "owner": "data-eng" },
  "columns": [
    {
      "name": "id",
      "index": 0,
      "data_type": "integer",
      "declared_type": "int",
      "inferred_type": null,
      "catalog_type": "INT64",
      "description": "Unique customer identifier.",
      "label": null,
      "granularity": null
    }
  ],
  "referenced_by": [
    { "unique_id": "model.jaffle_shop.stg_customers", "edge_type": "ref" }
  ],
  "execution_info": {
    "status": "success",
    "completed_at": "2026-05-15T10:28:03Z",
    "execution_time": 1.8
  },
  "catalog": {
    "type": "table",
    "owner": "dbt_runner",
    "row_count_stat": 935,
    "bytes_stat": 49152,
    "stats": [
      {
        "id": "has_stats",
        "label": "Has Stats?",
        "value": "true",
        "description": "Indicates whether there are statistics for this table",
        "include": false
      }
    ]
  }
}
```

`execution_info` is `null` when `dbt_rt.run_results` has no row for this seed (i.e., `dbt seed` / `dbt build` has not run).
`catalog` is `null` when `dbt.catalog_tables` has no row for this seed (i.e., `dbt docs generate` has not run).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | ✅ | — | e.g., `"seed.pkg.name"` |
| `name` | `string` | Core | ✅ | — | |
| `resource_type` | `"seed"` | Core | ✅ | — | Always `"seed"` for this endpoint |
| `package_name` | `string \| null` | Core | ✅ | — | |
| `description` | `string \| null` | Core | ✅ | — | |
| `original_file_path` | `string \| null` | Core | ✅ | — | Path to the CSV file relative to project root |
| `file_path` | `string \| null` | Core | 🔧 | — | Relative path from `dbt.nodes.file_path`; used by UI "Files" list |
| `patch_path` | `string \| null` | Core | 🔧 | — | Path to the YAML schema patch file, project-relative (no `<package>://` prefix — `dbt.nodes.patch_path` stores the bare path) — in `dbt.nodes` parquet |
| `tags` | `string[]` | Core | 🔧 | — | In `dbt.nodes` parquet; add to handler SELECT |
| `fqn` | `string[]` | Core | 🔧 | — | In `dbt.nodes` parquet; add to handler SELECT |
| `database_name` | `string \| null` | Core | ✅ | — | |
| `schema_name` | `string \| null` | Core | ✅ | — | |
| `identifier` | `string \| null` | Core | ✅ | — | Maps to `dbt.nodes.alias` for seeds; warehouse table name |
| `meta` | `Record<string, unknown> \| null` | Core | 🔍 | — | JSONB blob — confirm `dbt.nodes` parquet includes a `meta` column |
| `columns` | `SeedColumn[]` | Core | ✅ | — | Empty array if `dbt docs generate` has not run |
| `columns[*].name` | `string` | Core | ✅ | — | |
| `columns[*].index` | `number \| null` | Core | ✅ | — | Column order |
| `columns[*].data_type` | `string \| null` | Core | ✅ | — | Declared in YAML patch |
| `columns[*].declared_type` | `string \| null` | Core | ✅ | — | |
| `columns[*].inferred_type` | `string \| null` | Proprietary | ✅ | — | `null` in Core; populated by dbt v2 static analysis |
| `columns[*].catalog_type` | `string \| null` | Core-conditional | ✅ | — | Warehouse-verified type; `null` unless `dbt docs generate` ran |
| `columns[*].description` | `string \| null` | Core | ✅ | — | |
| `columns[*].label` | `string \| null` | Core | ✅ | — | |
| `columns[*].granularity` | `string \| null` | Core | ✅ | — | |
| `referenced_by` | `EdgeRef[]` | Core | ✅ | — | Downstream models; seeds have **no** `depends_on` |
| `referenced_by[*].unique_id` | `string` | Core | ✅ | — | |
| `referenced_by[*].edge_type` | `string` | Core | ✅ | — | |
| `execution_info` | `ExecutionInfo \| null` | Core-conditional | 🔧 | — | `null` when `dbt seed` / `dbt build` hasn't run |
| `execution_info.status` | `string` | Core-conditional | 🔧 | — | `"success"` · `"error"` · `"skipped"` |
| `execution_info.completed_at` | `string \| null` | Core-conditional | 🔍 | — | ISO 8601; extracted from `timing` JSON column — requires `json_extract_string` over the `timing` array |
| `execution_info.execution_time` | `number \| null` | Core-conditional | 🔧 | — | Seconds (float); from `dbt_rt.run_results.execution_time` |
| `catalog` | `SeedCatalogInfo \| null` | Core-conditional | 🔧 | — | `null` when `dbt docs generate` hasn't run |
| `catalog.type` | `string \| null` | Core-conditional | 🔧 | — | Warehouse object type; seeds are always `"table"` |
| `catalog.owner` | `string \| null` | Core-conditional | 🔧 | — | Warehouse role that owns the relation |
| `catalog.row_count_stat` | `number \| null` | Core-conditional | 🔍 | — | Approximate row count; from `dbt.catalog_stats` — confirm stat key |
| `catalog.bytes_stat` | `number \| null` | Core-conditional | 🔍 | — | Bytes; from `dbt.catalog_stats` — confirm stat key |
| `catalog.stats` | `CatalogStat[]` | Core-conditional | 🔧 | — | Arbitrary warehouse statistics |
| `catalog.stats[*].id` | `string` | Core-conditional | 🔧 | — | Stat identifier |
| `catalog.stats[*].label` | `string` | Core-conditional | 🔧 | — | Human-readable label |
| `catalog.stats[*].value` | `string` | Core-conditional | 🔧 | — | Always a string; parse as number if needed |
| `catalog.stats[*].description` | `string` | Core-conditional | 🔧 | — | |
| `catalog.stats[*].include` | `boolean` | Core-conditional | 🔧 | — | Whether the stat should be displayed in the UI |
| `project_id` | *(absent)* | — | ❌ | — | Class B: Cloud concept; not in parquet |
| `last_run_id` | *(absent)* | — | ❌ | — | Class B: Cloud run ID; not in local parquet |
| `last_job_definition_id` | *(absent)* | — | ❌ | — | Class B: Cloud scheduler concept; not in parquet |
| `raw_code` | *(absent)* | — | ❌ | — | Seeds have no SQL body |
| `compiled_code` | *(absent)* | — | ❌ | — | Seeds have no SQL body |
| `materialized` | *(absent)* | — | ❌ | — | Seeds are always a table; no strategy field |
| `access_level` | *(absent)* | — | ❌ | — | Model-access feature; not applicable to seeds |
| `group_name` | *(absent)* | — | ❌ | — | Not applicable to seeds |
| `contract_enforced` | *(absent)* | — | ❌ | — | Not applicable to seeds |
| `relation_name` | *(absent)* | — | ❌ | — | Not emitted by dbt for seeds; `identifier` covers the use case |
| `depends_on` | *(absent)* | — | ❌ | — | Seeds have no upstream dependencies; omit entirely (not empty array) |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api |

`SeedCatalogInfo` omits `comment` and `primary_key` (source-only fields) and is
structurally identical to the base `CatalogInfo` from `ModelDetail`, extended with `stats[]`.

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface SeedDetail {
  unique_id: string;
  name: string;
  resource_type: "seed";
  package_name: string | null;
  description: string | null;
  original_file_path: string | null;
  file_path: string | null;
  patch_path: string | null;
  tags: string[];
  fqn: string[];
  database_name: string | null;
  schema_name: string | null;
  identifier: string | null;
  meta: Record<string, unknown> | null;
  columns: SeedColumn[];
  referenced_by: EdgeRef[];
  execution_info: ExecutionInfo | null;
  catalog: SeedCatalogInfo | null;
}

// SeedColumn is identical in shape to ModelColumn and SourceColumn
interface SeedColumn {
  name: string;
  index: number | null;
  data_type: string | null;
  declared_type: string | null;
  inferred_type: string | null;
  catalog_type: string | null;
  description: string | null;
  label: string | null;
  granularity: string | null;
}

// SeedCatalogInfo extends base CatalogInfo with stats[]
// Does NOT include comment or primary_key (those are SourceCatalogInfo-only)
interface SeedCatalogInfo {
  type: string | null;
  owner: string | null;
  row_count_stat: number | null;
  bytes_stat: number | null;
  stats: CatalogStat[];
}

// ExecutionInfo, CatalogStat, EdgeRef are shared with ModelDetail
```

### Risk register

1. **`file_path` and `patch_path` are not queried by the existing handler.** Both columns
   exist in `dbt.nodes` parquet (confirmed in `upsert_node`). Add them to the seed-specific
   handler SELECT alongside `tags`, `fqn`, and `alias`.

2. **`completed_at` requires JSON extraction from `timing`.** The `dbt_rt.run_results`
   table stores timing data as a JSON array in the `timing` column. Extraction requires a
   DuckDB JSON path expression, not a simple column alias. Confirm the exact syntax against a
   real index before implementing. If the execute phase is missing, return `null`.

3. **`meta` JSONB presence in parquet is unverified.** Same risk as documented in the source
   contract (Risk #3). Confirm the `meta` column is queryable in `dbt.nodes.parquet` before
   adding it to the SELECT.

4. **Per-seed test list is deferred.** The GraphQL query fetches `tests[]` inline, powering
   the `useSetMissingTests` warning banner in `SeedView`. Defer to a future
   `GET /api/v1/seeds/:id/tests` sub-resource. FE must render a graceful null state until
   that endpoint exists.

5. **`catalog.row_count_stat` and `catalog.bytes_stat` stat key names are unverified.**
   These values live in `dbt.catalog_stats` keyed by `stat_id`. Canonical stat IDs vary by
   adapter. Confirm the exact keys used by dbt-index catalog ingestion before mapping to
   top-level response fields. The raw `stats[]` array is the safe fallback.

6. **`depends_on` must be omitted, not empty.** Seeds have no upstream SQL dependencies.
   The handler must NOT return `depends_on: []` — omit the field entirely. Consistent with
   `SourceDetail` precedent.

7. **`SeedCatalogInfo` catalog struct alignment.** Three distinct catalog shapes now exist:
   `CatalogInfo` (models), `SourceCatalogInfo` (adds `comment`, `primary_key`, `stats[]`),
   `SeedCatalogInfo` (adds `stats[]` only). Decide before implementation whether to define
   separate Rust structs or unify into a single struct with nullable extension fields.

---

## `GET /api/v1/snapshots/:id`

Powers: `SnapshotView` / `ResourceDetailsPage` in dbt-ui.
dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/SnapshotView.tsx`
GraphQL hooks: `packages/metadata/dbt-explorer/src/hooks/discovery/snapshot.ts` (`GetSnapshotByUniqueId`) and `src/hooks/dbtStrategy/useSnapshot.ts`

**No new ADR needed.** This endpoint follows ADR-1 (type-specific) and ADR-2 (conditional
data inlined, null-gated by capability) without exception. `execution_info` applies to
snapshots exactly as it does to models (`dbt build` and `dbt snapshot` both produce run
results). Snapshots share the model execution surface (`execution_info`, `catalog`,
`columns`, `depends_on`, `referenced_by`, `raw_code`, `compiled_code`) but add
`patch_path` (the `.yml` patch file, separate from `original_file_path`) and omit
model-only governance fields (`access_level`, `group_name`, `contract_enforced`). The
per-snapshot `tests[]` inline array is deferred — same open question as `ModelDetail`
Risk #8.

### Example response

Fields marked `// conditional` are `null` when their capability gate is absent.
Fields marked `// 🔧` are not yet returned — they require a backend change.
Fields marked `// 🔍` are parquet presence unverified.

```json
{
  "unique_id": "snapshot.jaffle_shop.orders_snapshot",
  "name": "orders_snapshot",
  "resource_type": "snapshot",
  "package_name": "jaffle_shop",
  "description": "Snapshot of the orders table tracking row-level changes over time.",
  "original_file_path": "snapshots/orders_snapshot.sql",
  "patch_path": "snapshots/schema.yml",
  "tags": ["finance", "snapshot"],
  "fqn": ["jaffle_shop", "orders_snapshot"],
  "database_name": "prod",
  "schema_name": "dbt_prod",
  "identifier": "orders_snapshot",
  "relation_name": "prod.dbt_prod.orders_snapshot",
  "materialized": "snapshot",
  "raw_code": "{%- snapshot orders_snapshot -%}\n  ...\n{%- endsnapshot -%}",
  "compiled_code": null,
  "meta": { "owner": "data-eng" },
  "depends_on": [
    { "unique_id": "model.jaffle_shop.orders", "edge_type": "model" }
  ],
  "referenced_by": [],
  "columns": [
    {
      "name": "order_id",
      "index": 0,
      "data_type": "integer",
      "declared_type": "int",
      "inferred_type": null,
      "catalog_type": "INT64",
      "description": "Unique order identifier.",
      "label": null,
      "granularity": null
    }
  ],
  "execution_info": {
    "status": "success",
    "completed_at": "2026-05-15T10:32:11Z",
    "execution_time": 12.7
  },
  "catalog": {
    "type": "table",
    "owner": "dbt_runner",
    "primary_key": ["order_id"],
    "row_count_stat": 42000,
    "bytes_stat": 3145728,
    "stats": [
      {
        "id": "has_stats",
        "label": "Has Stats?",
        "value": "true",
        "description": "Indicates whether there are statistics for this table",
        "include": false
      }
    ]
  }
}
```

`execution_info` is `null` when `dbt_rt.run_results` has no row for this snapshot (i.e., `dbt build` has not run).
`catalog` is `null` when `dbt.catalog_tables` has no row for this snapshot (i.e., `dbt docs generate` has not run).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | ✅ | — | e.g., `"snapshot.pkg.name"` |
| `name` | `string` | Core | ✅ | — | |
| `resource_type` | `"snapshot"` | Core | ✅ | — | Always `"snapshot"` for this endpoint |
| `package_name` | `string \| null` | Core | ✅ | — | |
| `description` | `string \| null` | Core | ✅ | — | |
| `original_file_path` | `string \| null` | Core | ✅ | — | Path to the `.sql` file; maps from `filePath` in GraphQL |
| `patch_path` | `string \| null` | Core | 🔍 | — | Path to the `.yml` patch file; in manifest but unverified in `dbt.nodes` parquet — see Risk #1 |
| `tags` | `string[]` | Core | 🔧 | — | In `dbt.nodes` parquet; add to handler SELECT |
| `fqn` | `string[]` | Core | 🔧 | — | In `dbt.nodes` parquet; add to handler SELECT |
| `database_name` | `string \| null` | Core | ✅ | — | |
| `schema_name` | `string \| null` | Core | ✅ | — | |
| `identifier` | `string \| null` | Core | ✅ | — | Maps from `alias` in GraphQL; overrides `name` if set |
| `relation_name` | `string \| null` | Core | ✅ | — | Fully qualified: `db.schema.name` |
| `materialized` | `"snapshot"` | Core | ✅ | — | Always `"snapshot"` for this resource type |
| `raw_code` | `string \| null` | Core | ✅ | — | The `{%- snapshot -%}` block source |
| `compiled_code` | `string \| null` | Core | 🔍 | — | Likely in `dbt.nodes` parquet — confirm schema before implementing; see Risk #2 |
| `meta` | `Record<string, unknown> \| null` | Core | 🔍 | — | JSONB blob — confirm `dbt.nodes` parquet includes a `meta` column; see Risk #3 |
| `depends_on` | `EdgeRef[]` | Core | ✅ | — | 1-hop upstream; see Risk #4 re: pagination |
| `depends_on[*].unique_id` | `string` | Core | ✅ | — | |
| `depends_on[*].edge_type` | `string` | Core | ✅ | — | |
| `referenced_by` | `EdgeRef[]` | Core | ✅ | — | 1-hop downstream; see Risk #4 re: pagination |
| `referenced_by[*].unique_id` | `string` | Core | ✅ | — | |
| `referenced_by[*].edge_type` | `string` | Core | ✅ | — | |
| `columns` | `SnapshotColumn[]` | Core | ✅ | — | Identical shape to `ModelColumn[]`; empty array if none declared |
| `columns[*].name` | `string` | Core | ✅ | — | |
| `columns[*].index` | `number \| null` | Core | ✅ | — | Column order |
| `columns[*].data_type` | `string \| null` | Core | ✅ | — | Declared in YAML |
| `columns[*].declared_type` | `string \| null` | Core | ✅ | — | |
| `columns[*].inferred_type` | `string \| null` | Proprietary | ✅ | — | `null` in Core; populated by dbt v2 static analysis |
| `columns[*].catalog_type` | `string \| null` | Core-conditional | ✅ | — | Warehouse-verified type; `null` unless `dbt docs generate` ran |
| `columns[*].description` | `string \| null` | Core | ✅ | — | |
| `columns[*].label` | `string \| null` | Core | ✅ | — | |
| `columns[*].granularity` | `string \| null` | Core | ✅ | — | |
| `execution_info` | `ExecutionInfo \| null` | Core-conditional | 🔧 | — | `null` when `dbt build` hasn't run |
| `execution_info.status` | `string` | Core-conditional | 🔧 | — | `"success"` · `"error"` · `"skipped"` |
| `execution_info.completed_at` | `string \| null` | Core-conditional | 🔧 | — | ISO 8601 timestamp |
| `execution_info.execution_time` | `number \| null` | Core-conditional | 🔧 | — | Seconds (float) |
| `catalog` | `SnapshotCatalogInfo \| null` | Core-conditional | 🔧 | — | `null` when `dbt docs generate` hasn't run; adds `primary_key` and `stats[]` over base `CatalogInfo` |
| `catalog.type` | `string \| null` | Core-conditional | 🔧 | — | `"table"` · `"view"` · `"materialized view"` |
| `catalog.owner` | `string \| null` | Core-conditional | 🔧 | — | Warehouse role that owns the relation |
| `catalog.primary_key` | `string[]` | Core-conditional | 🔧 | — | Column names constituting the PK; empty array if none. Sourced from `dbt.nodes.primary_key` (a `List<String>` column, populated from the snapshot's `unique_key` config) — not from `dbt.catalog_tables`, which has no `primary_key` column |
| `catalog.row_count_stat` | `number \| null` | Core-conditional | 🔧 | — | Approximate row count |
| `catalog.bytes_stat` | `number \| null` | Core-conditional | 🔧 | — | Bytes; warehouse-specific |
| `catalog.stats` | `CatalogStat[]` | Core-conditional | 🔧 | — | Arbitrary warehouse statistics; same shape as `SourceCatalogInfo.stats[]` |
| `catalog.stats[*].id` | `string` | Core-conditional | 🔧 | — | |
| `catalog.stats[*].label` | `string` | Core-conditional | 🔧 | — | |
| `catalog.stats[*].value` | `string` | Core-conditional | 🔧 | — | Always string; parse as number if needed |
| `catalog.stats[*].description` | `string` | Core-conditional | 🔧 | — | |
| `catalog.stats[*].include` | `boolean` | Core-conditional | 🔧 | — | Whether to display in UI |
| `tests` | *(absent)* | — | ❌ | — | Deferred for v0; same open question as `ModelDetail` Risk #8 — defer until model contract resolves |
| `access_level` | *(absent)* | — | ❌ | — | Model-only governance field; not applicable to snapshots |
| `group_name` | *(absent)* | — | ❌ | — | Model-only governance field; not applicable to snapshots |
| `contract_enforced` | *(absent)* | — | ❌ | — | Model-only governance field; not applicable to snapshots |
| `last_run_id` | *(absent)* | — | ❌ | — | Class B: Cloud-specific run ID; no parquet path |
| `last_job_definition_id` | *(absent)* | — | ❌ | — | Class B: Cloud-specific job ID; no parquet path |
| `project_id` | *(absent)* | — | ❌ | — | Class B: Cloud-specific; no parquet path |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; Discovery-API-internal |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface SnapshotDetail {
  unique_id: string;
  name: string;
  resource_type: "snapshot";
  package_name: string | null;
  description: string | null;
  original_file_path: string | null;
  patch_path: string | null;
  tags: string[];
  fqn: string[];
  database_name: string | null;
  schema_name: string | null;
  identifier: string | null;
  relation_name: string | null;
  materialized: "snapshot";
  raw_code: string | null;
  compiled_code: string | null;
  meta: Record<string, unknown> | null;
  depends_on: EdgeRef[];
  referenced_by: EdgeRef[];
  columns: SnapshotColumn[];
  execution_info: ExecutionInfo | null;
  catalog: SnapshotCatalogInfo | null;
}

// SnapshotColumn is identical in shape to ModelColumn
interface SnapshotColumn {
  name: string;
  index: number | null;
  data_type: string | null;
  declared_type: string | null;
  inferred_type: string | null;
  catalog_type: string | null;
  description: string | null;
  label: string | null;
  granularity: string | null;
}

// SnapshotCatalogInfo adds primary_key and stats[] over model CatalogInfo
// (matches SourceCatalogInfo minus the comment field)
interface SnapshotCatalogInfo {
  type: string | null;
  owner: string | null;
  primary_key: string[];
  row_count_stat: number | null;
  bytes_stat: number | null;
  stats: CatalogStat[];
}

// ExecutionInfo, CatalogStat, EdgeRef are shared with ModelDetail
```

### Risk register

1. **`patch_path` presence in parquet is unverified.** `SnapshotView.tsx` reads
   `snapshot.applied.patchPath` to populate the file link in the resource header. The field
   exists in `manifest.json` and the GraphQL applied-state layer, but whether dbt-index
   writes it into `dbt.nodes.parquet` is unconfirmed. Verify before adding to the handler
   SELECT. If absent from parquet, the field must be omitted or gated on a new capability.

2. **`compiled_code` presence in parquet is unverified.** Snapshots use `{%- snapshot -%}`
   blocks and dbt does compile them; the compiled form is likely present but needs
   confirmation. Mark as TODO and omit if absent.

3. **`meta` JSONB presence in parquet is unverified.** Same risk as `SourceDetail` Risk #3.
   Confirm the column is serialized as a queryable JSON string in `dbt.nodes.parquet` before
   adding to the SELECT.

4. **`depends_on`/`referenced_by` have no pagination cap.** Identical risk to `ModelDetail`
   Risk #5. Add a `?first=` cap with `truncated: true` for v0.

5. **`SnapshotCatalogInfo` vs. `CatalogInfo` struct proliferation.** Snapshots and sources
   both extend base `CatalogInfo` with `primary_key` and `stats[]`; sources also add
   `comment`. Three distinct catalog structs now exist. Decide at implementation time whether
   to unify into a single struct with nullable extension fields, or keep separate Rust structs.
   The decision affects all three existing contracts.

6. **`tests[]` inline deferred — surface may never be added.** Block on the model-level
   resolution of `ModelDetail` Risk #8 to keep contracts consistent.

7. **`execution_info` absent from current handler.** The existing generic `get_node` handler
   does not query `dbt_rt.run_results_latest`. The snapshot-specific handler will need this
   query added; the resulting `execution_info` is `null` when no row exists.

---

## `GET /api/v1/tests/:id`

Powers: `TestView` / `ResourceDetailsPage` in dbt-ui — header card (type, last run status,
target column), Code tab (raw + compiled SQL for data tests; given/expect YAML for unit
tests), General tab (description, metadata, dependencies).

dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/TestView.tsx`
GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/dbtStrategy/useTest.ts` → `src/hooks/discovery/test.ts` (`GetTestByUniqueId`)

This endpoint covers **both** `test.*` and `unit_test.*` unique_ids. The response shape is
a discriminated union on `resource_type` as decided in ADR-3.

### Example response (data test)

`execution_info` is `null` when `dbt_rt.run_results` has no row for this test (i.e., `dbt build` has not run).
Fields marked `// 🔧` are not yet returned — they require a backend change.
Fields marked `// 🔍` are in parquet but the exact column name is unconfirmed.

```json
{
  "unique_id": "test.jaffle_shop.not_null_orders_order_id",
  "name": "not_null_orders_order_id",
  "resource_type": "test",
  "package_name": "jaffle_shop",
  "description": "Asserts that order_id is never null.",
  "original_file_path": "models/schema.yml",
  "tags": ["data-quality"],
  "fqn": ["jaffle_shop", "not_null_orders_order_id"],
  "column_name": "order_id",
  "test_type": "generic",
  "severity": "ERROR",
  "test_metadata": {
    "name": "not_null",
    "kwargs": { "column_name": "order_id", "model": "ref('orders')" }
  },
  "raw_code": "select order_id from {{ model }} where order_id is null",
  "compiled_code": "select order_id from prod.dbt_prod.orders where order_id is null",
  "file_path": "models/schema.yml",
  "patch_path": null,
  "meta": {},
  "depends_on": [
    { "unique_id": "model.jaffle_shop.orders", "edge_type": "model" }
  ],
  "execution_info": {
    "status": "pass",
    "error": null,
    "completed_at": "2026-05-15T10:32:11Z",
    "execution_time": 1.4
  }
}
```

### Example response (unit test)

`execution_info` is `null` when `dbt_rt.run_results` has no row for this test.

```json
{
  "unique_id": "unit_test.jaffle_shop.test_orders_completed_status",
  "name": "test_orders_completed_status",
  "resource_type": "unit_test",
  "package_name": "jaffle_shop",
  "description": "Checks that completed orders always have a non-null amount.",
  "original_file_path": "models/schema.yml",
  "tags": [],
  "fqn": ["jaffle_shop", "test_orders_completed_status"],
  "model": "ref('orders')",
  "given": [
    {
      "input": "ref('stg_orders')",
      "rows": [
        { "order_id": 1, "status": "completed" },
        { "order_id": 2, "status": "pending" }
      ]
    }
  ],
  "expect": {
    "rows": [
      { "order_id": 1, "amount": 25.00 }
    ]
  },
  "num_given": 1,
  "num_given_rows": 2,
  "num_expect_rows": 1,
  "file_path": "models/schema.yml",
  "patch_path": null,
  "meta": {},
  "depends_on": [
    { "unique_id": "model.jaffle_shop.orders", "edge_type": "model" },
    { "unique_id": "model.jaffle_shop.stg_orders", "edge_type": "model" }
  ],
  "execution_info": {
    "status": "pass",
    "error": null,
    "completed_at": "2026-05-15T10:32:15Z",
    "execution_time": 0.8
  }
}
```

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

Fields that appear in only one variant are noted in the Notes column.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"test.pkg.name"` or `"unit_test.pkg.name"` |
| `name` | `string` | Core | 🔧 | — | |
| `resource_type` | `"test" \| "unit_test"` | Core | 🔧 | — | Discriminator — determines which variant shape is returned |
| `package_name` | `string \| null` | Core | 🔧 | — | |
| `description` | `string \| null` | Core | 🔧 | — | |
| `original_file_path` | `string \| null` | Core | 🔧 | — | Path to the `.yml` file defining the test |
| `tags` | `string[]` | Core | 🔧 | — | In `dbt.nodes` parquet |
| `fqn` | `string[]` | Core | 🔧 | — | In `dbt.nodes` parquet |
| `file_path` | `string \| null` | Core | 🔧 | — | Rendered in TestView header via `applied.filePath`; in `dbt.nodes` parquet |
| `patch_path` | `string \| null` | Core | 🔧 | — | Rendered in TestView header via `applied.patchPath`; in `dbt.nodes` parquet |
| `meta` | `Record<string, unknown> \| null` | Core | 🔍 | — | JSONB blob — confirm `dbt.nodes` parquet includes a `meta` column |
| `depends_on` | `EdgeRef[]` | Core | 🔧 | — | 1-hop upstream from `dbt.edges` parquet; maps to `parents` in GraphQL |
| `depends_on[*].unique_id` | `string` | Core | 🔧 | — | |
| `depends_on[*].edge_type` | `string` | Core | 🔧 | — | |
| `execution_info` | `TestExecutionInfo \| null` | Core-conditional | 🔧 | — | `null` when `dbt build` hasn't run; present on both variants |
| `execution_info.status` | `string \| null` | Core-conditional | 🔧 | — | `"pass"` · `"fail"` · `"error"` · `"warn"` · `"skipped"` · `"reused"` |
| `execution_info.error` | `string \| null` | Core-conditional | 🔧 | — | Error message when status is `"error"`; `null` otherwise |
| `execution_info.completed_at` | `string \| null` | Core-conditional | 🔧 | — | ISO 8601 timestamp |
| `execution_info.execution_time` | `number \| null` | Core-conditional | 🔧 | — | Seconds (float) |
| `column_name` | `string \| null` | Core | 🔧 | — | **data test only** — column under test; from `dbt.test_metadata` parquet |
| `test_type` | `string \| null` | Core | 🔧 | — | **data test only** — `"generic"` · `"singular"`; from `dbt.nodes` parquet |
| `severity` | `string \| null` | Core | 🔧 | — | **data test only** — `"ERROR"` · `"WARN"`; from `dbt.test_metadata` parquet |
| `test_metadata` | `TestMetadata \| null` | Core | 🔧 | — | **data test only** — from `dbt.test_metadata` parquet |
| `test_metadata.name` | `string` | Core | 🔧 | — | **data test only** — e.g., `"not_null"`, `"unique"`, `"relationships"` |
| `test_metadata.kwargs` | `Record<string, unknown>` | Core | 🔍 | — | **data test only** — unstructured JSON; column name confirmed in parquet schema, exact serialization 🔍 |
| `raw_code` | `string \| null` | Core | 🔧 | — | **data test only** — SQL template; from `dbt.nodes` parquet |
| `compiled_code` | `string \| null` | Core | 🔍 | — | **data test only** — fully rendered SQL; confirm presence in `dbt.nodes` parquet |
| `model` | `string \| null` | Core | 🔍 | — | **unit test only** — the `ref(...)` expression identifying the model under test; in `dbt.unit_tests` parquet 🔍 |
| `given` | `UnitTestFixture[]` | Core | 🔧 | — | **unit test only** — input row fixtures; from `dbt.unit_tests` parquet |
| `given[*].input` | `string` | Core | 🔧 | — | **unit test only** — `ref(...)` or `source(...)` expression |
| `given[*].rows` | `Record<string, unknown>[]` | Core | 🔍 | — | **unit test only** — row data as parsed JSON; confirm parquet serialization format 🔍 |
| `expect` | `UnitTestExpect \| null` | Core | 🔧 | — | **unit test only** — expected output rows; from `dbt.unit_tests` parquet |
| `expect.rows` | `Record<string, unknown>[]` | Core | 🔍 | — | **unit test only** — expected row data; confirm parquet serialization format 🔍 |
| `num_given` | `number \| null` | Core | 🔍 | — | **unit test only** — count of `given` fixtures; from `dbt.unit_tests` parquet 🔍 |
| `num_given_rows` | `number \| null` | Core | 🔍 | — | **unit test only** — total input rows across all fixtures 🔍 |
| `num_expect_rows` | `number \| null` | Core | 🔍 | — | **unit test only** — expected output row count 🔍 |
| `config` | *(absent)* | — | ❌ | — | Class B: GraphQL `config` blob has no direct parquet column; individual fields (severity) promoted individually |
| `project_id` | *(absent)* | — | ❌ | — | Class B: platform metadata — not in any parquet table |
| `last_run_id` | *(absent)* | — | ❌ | — | Class B: run-system internal ID; no parquet path |
| `last_job_definition_id` | *(absent)* | — | ❌ | — | Class B: platform job system ID; no parquet path |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
// Discriminated union on resource_type — as decided in ADR-3
type TestDetail = DataTestDetail | UnitTestDetail;

// Shared fields factored here for documentation; Rust uses NodeBase struct
interface TestBase {
  unique_id: string;
  name: string;
  package_name: string | null;
  description: string | null;
  original_file_path: string | null;
  tags: string[];
  fqn: string[];
  file_path: string | null;
  patch_path: string | null;
  meta: Record<string, unknown> | null;
  depends_on: EdgeRef[];
  execution_info: TestExecutionInfo | null;
}

interface DataTestDetail extends TestBase {
  resource_type: "test";
  column_name: string | null;
  test_type: string | null;
  severity: string | null;
  test_metadata: TestMetadata | null;
  raw_code: string | null;
  compiled_code: string | null;
}

interface UnitTestDetail extends TestBase {
  resource_type: "unit_test";
  model: string | null;
  given: UnitTestFixture[];
  expect: UnitTestExpect | null;
  num_given: number | null;
  num_given_rows: number | null;
  num_expect_rows: number | null;
}

interface TestExecutionInfo {
  status: string | null;
  error: string | null;
  completed_at: string | null;
  execution_time: number | null;
}

interface TestMetadata {
  name: string;
  kwargs: Record<string, unknown>;
}

interface UnitTestFixture {
  input: string;
  rows: Record<string, unknown>[];
}

interface UnitTestExpect {
  rows: Record<string, unknown>[];
}

// EdgeRef is shared with ModelDetail and SourceDetail
```

### Risk register

1. **Two parquet sources required for data test fields.** A complete data test response
   requires joining `dbt.nodes` (for `name`, `fqn`, `tags`, `raw_code`, `original_file_path`,
   `description`) with `dbt.test_metadata` (for `column_name`, `severity`, `kwargs`,
   `test_metadata.name`). The handler must perform a LEFT JOIN on `unique_id`. Verify the
   join key column name in both parquet files before implementing.

2. **Unit test fields are in a separate parquet table.** Unit test row fixtures (`given`,
   `expect`, `num_given`, `num_given_rows`, `num_expect_rows`, `model`) come from
   `dbt.unit_tests.parquet`. The handler must detect `resource_type = 'unit_test'` from
   `dbt.nodes` and JOIN against `dbt.unit_tests` only for that variant. Parquet
   serialization of `given` and `expect` as JSON strings or nested structs is unconfirmed.

3. **`compiled_code` presence in parquet is unverified for tests.** Confirm against the
   actual `dbt.nodes.parquet` schema. Test nodes may not populate that column. If absent,
   omit the field and remove from the contract rather than returning `null`.

4. **`execution_info` requires a run_results JOIN.** `dbt_rt.run_results` must be LEFT
   JOINed on `unique_id` to get `status`, `error`, `completed_at`,
   and `execution_time`. The handler emits `execution_info: null` when no row is returned (no Capability flag — see ADR-7).

5. **`meta` JSONB presence in parquet is unverified.** Same risk as `SourceDetail` Risk #3.
   Confirm before adding to the SELECT; downgrade to ❌ Class B if absent.

6. **`kwargs` is unstructured JSON — fragile for relationship tests.** Per FEATURE-TO-ENDPOINT-MAPPING.md
   (F-14): parsing relationship test metadata requires matching `kwargs` keys (`to:`, `field:`,
   `column_name:`). This is a FE concern, not a handler concern — document so the FE team
   does not expect a structured object.

7. **Handler must route on `resource_type` from parquet, not path prefix.** The endpoint
   accepts a `unique_id` that may start with `test.` or `unit_test.`. The handler should
   read `resource_type` from the parquet row to determine which JOIN path and which response
   struct to use. Consistent with ADR-1's NodeBase pattern.

8. **`num_given`, `num_given_rows`, `num_expect_rows` may need to be derived.** If
   `dbt.unit_tests.parquet` stores serialized fixture arrays rather than pre-computed counts,
   these fields may need to be computed at query time (`array_length`) rather than read
   directly. Verify before implementing.

## Design notes — `GET /api/v1/exposures/:id`

This contract introduces no new ADR. It does, however, surface two FE-impacting decisions
the coordinator should be aware of before promoting:

1. **Class C exclusions are heavier here than on any prior resource.** Six fields the
   dbt-ui `ExposureView` GraphQL hook fetches (`autoBiProvider`, `integrationId`,
   `freshnessStatus`, `quality`, `upstreamStats`, `maxSnapshottedAt`) are flagged
   `subGraphs: ['internal']` in codex-api and have no parquet path. They are listed in
   the field reference as Class B `❌ absent` rather than Class C 412-stubs — they are
   not "Discovery public, CodexDB-only"; they are Discovery-internal *and* CodexDB-only.
   `healthIssues` and `projectId` are the same shape. The FE `ExposureView` must render
   graceful null states for the header trust signals badge, the modify-integration link,
   and the freshness chip when these fields are absent.

2. **No `referenced_by` on exposures.** Exposures are terminal leaf nodes — no resource
   refs an exposure. The handler must omit the field (not return `[]`), consistent with
   `SourceDetail`'s omission of `depends_on` and `SeedDetail`'s omission of `depends_on`.

Skip ADR promotion — both decisions follow established CC-5 and CC-2 conventions.

---

## `GET /api/v1/exposures/:id`

Powers: `ExposureView` / `ResourceDetailsPage` in dbt-ui — header card (type, maturity,
owner, link to BI tool), General tab (description, upstream parents, meta).
dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/ExposureView.tsx`
GraphQL hooks: `packages/metadata/dbt-explorer/src/hooks/dbtStrategy/useExposure.ts` → `src/hooks/discovery/exposure.ts` (`GetExposureByUniqueId`)

Exposures are downstream consumers of dbt artifacts — dashboards, ML applications, ad-hoc
analyses — declared in YAML so dbt can render them in the lineage graph. They are
**leaf nodes**: nothing refs an exposure, so this contract has `depends_on` but no
`referenced_by`. Exposures are **not executed by dbt**, so there is no `execution_info`,
no `columns`, no `catalog`, no `materialized`, no SQL body. They live in their own
parquet table — `dbt.exposures.parquet` — not in `dbt.nodes` (schema confirmed in
`crates/dbt-index/src/parquet.rs::ExposureRow`). All warehouse-shaped fields
(`database_name`, `schema_name`, `identifier`, `relation_name`) are intentionally omitted
because exposures have no warehouse object.

This is the **smallest** detail contract: 16 fields total (vs. 30+ for models).

### Example response

Fields marked `// 🔧` are not yet returned — they require a backend change (this endpoint
has no handler today; expect every field to be 🔧).
Fields marked `// 🔍` are parquet presence unverified — confirm schema before implementing.

```json
{
  "unique_id": "exposure.jaffle_shop.revenue_dashboard",
  "name": "revenue_dashboard",
  "resource_type": "exposure",
  "package_name": "jaffle_shop",
  "description": "Top-line revenue dashboard used by the finance team.",
  "original_file_path": "models/exposures.yml",
  "file_path": "models/exposures.yml",
  "tags": ["finance", "exec"],
  "fqn": ["jaffle_shop", "revenue_dashboard"],
  "label": "Revenue Dashboard",
  "exposure_type": "dashboard",
  "maturity": "high",
  "url": "https://bi.example.com/dashboards/revenue",
  "owner_name": "Jane Doe",
  "owner_email": "jane.doe@example.com",
  "meta": { "team": "finance" },
  "depends_on": [
    { "unique_id": "model.jaffle_shop.orders", "edge_type": "model" },
    { "unique_id": "source.jaffle_shop.raw_jaffle.orders", "edge_type": "source" }
  ],
  "created_at": 1747432300.5
}
```

This response has **no conditional sections**. Exposures have no execution surface and no
catalog surface, so no capability gates apply. `created_at` is the per-resource
"Definition updated as of …" timestamp per ADR-5 (epoch seconds, sourced from
`dbt.exposures.created_at`).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"exposure.pkg.name"` — no handler today |
| `name` | `string` | Core | 🔧 | — | From `dbt.exposures.name` |
| `resource_type` | `"exposure"` | Core | 🔧 | — | Always `"exposure"` for this endpoint |
| `package_name` | `string \| null` | Core | 🔧 | — | From `dbt.exposures.package_name` |
| `description` | `string \| null` | Core | 🔧 | — | From `dbt.exposures.description` |
| `original_file_path` | `string \| null` | Core | 🔧 | — | From `dbt.exposures.original_file_path`; absolute-rooted YAML path |
| `file_path` | `string \| null` | Core | 🔧 | — | From `dbt.exposures.file_path`; project-relative — `ExposureView` reads both — see Risk #1 |
| `tags` | `string[]` | Core | 🔧 | — | From `dbt.exposures.tags` (list_utf8 column) |
| `fqn` | `string[]` | Core | 🔧 | — | From `dbt.exposures.fqn` (list_utf8 column) |
| `label` | `string \| null` | Core | 🔧 | — | Display label override; from `dbt.exposures.label` |
| `exposure_type` | `string \| null` | Core | 🔧 | — | `"dashboard"` · `"notebook"` · `"analysis"` · `"ml"` · `"application"` — see Risk #2 |
| `maturity` | `string \| null` | Core | 🔧 | — | `"high"` · `"medium"` · `"low"` — see Risk #2 |
| `url` | `string \| null` | Core | 🔧 | — | Link to the upstream BI/app dashboard |
| `owner_name` | `string \| null` | Core | 🔧 | — | From `dbt.exposures.owner_name` |
| `owner_email` | `string \| null` | Core | 🔧 | — | From `dbt.exposures.owner_email` |
| `meta` | `Record<string, unknown> \| null` | Core | 🔍 | — | JSON-string column in parquet; needs `json_parse` at query time — see Risk #3 |
| `depends_on` | `EdgeRef[]` | Core | 🔧 | — | 1-hop upstream models + sources; derived from `dbt.exposures.depends_on_nodes` — see Risk #4 |
| `depends_on[*].unique_id` | `string` | Core | 🔧 | — | |
| `depends_on[*].edge_type` | `string` | Core | 🔧 | — | Resolved from the dependency's `resource_type` (model/source) — see Risk #4 |
| `patch_path` | *(absent)* | — | ❌ | — | Not in `dbt.exposures.parquet` schema (only `file_path` and `original_file_path`); dbt-ui reads `patchPath` from GraphQL, but exposures are defined directly in YAML — the patch concept does not apply — see Risk #5 |
| `referenced_by` | *(absent)* | — | ❌ | — | Exposures are terminal leaf nodes; nothing refs an exposure. Omit entirely, not empty array |
| `manifest_generated_at` | *(absent)* | — | ❌ | — | Class B: environment-level field on the GraphQL `applied` wrapper, not on the exposure row; ingest timestamp lives in `dbt.exposures.ingested_at` and is internal |
| `parents[]` (Discovery shape) | *(absent)* | — | ❌ | — | dbt-ui's GraphQL `parents` field is replaced by `depends_on` (CC-1 / CC-2: snake_case, REST naming) — same data, REST shape |
| `auto_bi_provider` | *(absent)* | — | ❌ | — | Class B: `subGraphs: ['internal']` in codex-api; auto-exposures are Platform-tier per FEATURE-TO-ENDPOINT-MAPPING.md F-18 — see Risk #6 |
| `integration_id` | *(absent)* | — | ❌ | — | Class B: `subGraphs: ['internal']`; auto-exposure-only field; no parquet path |
| `freshness_status` | *(absent)* | — | ❌ | — | Class B: `subGraphs: ['internal']`; aggregated from upstream source freshness — derive FE-side from `freshness` on each `depends_on` if needed |
| `quality` | *(absent)* | — | ❌ | — | Class B: `subGraphs: ['internal']`; aggregated worst test status across ancestors — derive FE-side |
| `upstream_stats` | *(absent)* | — | ❌ | — | Class B: `subGraphs: ['internal']`; Discovery-API aggregate |
| `max_snapshotted_at` | *(absent)* | — | ❌ | — | Class B: `subGraphs: ['internal']`; oldest snapshot timestamp across ancestors |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api (matches `ModelDetail` / `SourceDetail` Risk #7) |
| `project_id` | *(absent)* | — | ❌ | — | Class B: Cloud-specific; no parquet path |
| `created_at` | `number \| null` | Core | 🔧 | — | Epoch seconds (float); from `dbt.exposures.created_at`. Per ADR-5, this is the "Definition updated as of …" timestamp surfaced to `ExposureView`. Empirically verified column present in `dbt.exposures.parquet`. |
| `execution_info` | *(absent)* | — | ❌ | — | Exposures are not executed by dbt; no `dbt_rt.run_results` row exists for an exposure. Per ADR-5 the field is omitted from `DefinitionNodeBase` entirely — this row is documentation only. |
| `columns` | *(absent)* | — | ❌ | — | Exposures have no columns; they are downstream consumers |
| `materialized` | *(absent)* | — | ❌ | — | Exposures are not materialized to a warehouse object |
| `raw_code` | *(absent)* | — | ❌ | — | Exposures have no SQL body — YAML-only definition |
| `compiled_code` | *(absent)* | — | ❌ | — | Exposures have no SQL body |
| `database_name` | *(absent)* | — | ❌ | — | Exposures have no warehouse relation |
| `schema_name` | *(absent)* | — | ❌ | — | Exposures have no warehouse relation |
| `identifier` | *(absent)* | — | ❌ | — | Exposures have no warehouse relation |
| `relation_name` | *(absent)* | — | ❌ | — | Exposures have no warehouse relation |
| `access_level` | *(absent)* | — | ❌ | — | Model-only governance field |
| `group_name` | *(absent)* | — | ❌ | — | Not modeled on exposures |
| `contract_enforced` | *(absent)* | — | ❌ | — | Not applicable to exposures |
| `catalog` | *(absent)* | — | ❌ | — | Exposures have no warehouse object to catalog |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface ExposureDetail {
  unique_id: string;
  name: string;
  resource_type: "exposure";
  package_name: string | null;
  description: string | null;
  original_file_path: string | null;
  file_path: string | null;
  tags: string[];
  fqn: string[];
  label: string | null;
  exposure_type: string | null;
  maturity: string | null;
  url: string | null;
  owner_name: string | null;
  owner_email: string | null;
  meta: Record<string, unknown> | null;
  depends_on: EdgeRef[];
  created_at: number | null;   // ADR-5: per-resource "Definition updated as of …" timestamp; epoch seconds
}

// EdgeRef is shared with ModelDetail, SourceDetail, SeedDetail, SnapshotDetail
interface EdgeRef {
  unique_id: string;
  edge_type: string;
}
```

### Risk register

1. **`file_path` vs. `original_file_path` for the header file list.** `ExposureView.tsx`
   passes both `appliedExposure?.patchPath` and `appliedExposure?.filePath` to
   `ResourceDetailsHeader` (deduplicating imported-auto paths). The `dbt.exposures.parquet`
   schema has `file_path` AND `original_file_path` — these are likely the same value for
   YAML-defined exposures, but `file_path` may be a project-relative form while
   `original_file_path` is absolute or root-anchored. Confirm semantics with the dbt-index
   team before the FE consumes both. If they are always equal, the FE should pick one and
   the handler can omit the duplicate.

2. **`exposure_type` and `maturity` enum values need verification.** dbt's manifest defines
   `exposure_type ∈ {dashboard, notebook, analysis, ml, application}` and
   `maturity ∈ {high, medium, low}` but does not validate the strings at parse time. The
   parquet column is plain `utf8` (no enum constraint). Document the expected values so
   FE engineers do not silently fall through unknown strings. `ExposureStatusTileSection.tsx`
   branches on `exposureType === 'dashboard'` — case sensitivity must be confirmed.

3. **`meta` is stored as a JSON string in `dbt.exposures.parquet`.** The parquet column is
   `[utf8] meta: Option<String>` — i.e., serialized JSON, not a parquet struct. The handler
   must `json_parse` (or DuckDB `json_extract`) at query time. Returning the raw string is
   incorrect; the contract specifies `Record<string, unknown>`. Verify the JSON shape is
   always an object (not array or primitive) before implementing.

4. **`depends_on` requires resolving `edge_type` from the dependency's resource type.**
   `dbt.exposures.depends_on_nodes` is a list of `unique_id` strings only — the resource
   type is implicit in the prefix (`model.`, `source.`, `metric.`, `seed.`). The handler
   must either (a) parse the prefix to derive `edge_type`, or (b) JOIN against
   `dbt.nodes` / `dbt.metrics` / etc. to read the canonical `resource_type`. Parsing the
   prefix is faster and matches what `ExposureRow` already encodes. `dbt.exposures.depends_on_macros`
   exists separately and is intentionally not surfaced — macros are not user-visible nodes.

5. **`patch_path` is absent from the parquet row but present in `ExposureView`'s
   GraphQL.** Exposures are defined directly in `.yml` (no separate `.sql` + patch
   structure), so dbt's manifest typically does not emit a distinct `patch_path` for them.
   `dbt.exposures.parquet` has only `file_path` and `original_file_path` — no
   `patch_path`. Document as ❌ Class B; do not chase. If a future dbt version starts
   writing `patch_path` to `dbt.exposures`, it can be added additively.

6. **The header trust-signals badge (`healthIssues`), modify-integration link
   (`integrationId`), and auto-exposure provider chip (`autoBiProvider`) will not render.**
   All three are Class B (`subGraphs: ['internal']` in codex-api per
   FEATURE-TO-ENDPOINT-MAPPING.md F-18). The dbt-ui `ExposureView` must render graceful
   null states. The auto-exposure flow (`pathIsImported` + `IMPORTED_AUTO_EXPOSURE_PATH_PREFIX`
   gating) is moot here — auto-exposures are Platform-tier and not in scope for
   dbt-docs-server.

7. **No `execution_info` on this response.** Unlike models/seeds/snapshots/tests, exposures
   have no parquet row in `dbt_rt.run_results` — they are not executable. The contract
   omits `execution_info` entirely (consistent with ADR-5 for definition-only resources).
   FE engineers should not look for a status badge here; an exposure's "health" is
   derivable from upstream node statuses only.

8. **No `Capabilities` flag additions for this endpoint.** All exposure fields are
   unconditional Core. Per ADR-7, the absent `execution_info` / `catalog` / `freshness`
   surfaces are simply not part of this response — they aren't routed through `Capabilities`.

9. **New handler file required.** No `src/handlers/exposures.rs` exists today
   (confirmed against the worktree handler directory listing). Implementation should
   compose the shared `NodeBase` Rust struct per ADR-1's backend prerequisite, then add
   the exposure-specific fields (`label`, `exposure_type`, `maturity`, `url`,
   `owner_name`, `owner_email`). Register the route in `src/server.rs` and the type in
   `web/src/api.ts`.

## Design notes — `GET /api/v1/groups/:id`

Groups are the first **definition-only** resource type in the contract set. Every
endpoint before it (`models`, `sources`, `seeds`, `snapshots`, `tests`) returns
node-shaped data from `dbt.nodes`. Groups live in their own parquet table
(`dbt.groups`) and have no SQL body, no columns, no warehouse relation, no run
results, no catalog stats, no freshness, no lineage. The endpoint's purpose is
narrow: render the GroupView details panel plus an inline list of member models.

Two design choices worth flagging for the coordinator before promotion:

1. **`owner` is a nested object, not flattened scalars.** The Discovery API
   exposes `ownerName`, `ownerEmail`, `ownerSlack`, `ownerGithub` as four
   sibling fields. CC-2 says preserve nested shape; here the nesting does not
   exist in the upstream GraphQL response — we'd be **introducing** it. The
   upside is a cleaner type (`owner: { name, email, slack, github } | null`)
   that scales if more contact channels are added (Teams, PagerDuty, …). The
   downside is divergence from the FE engineers' Discovery API mental model.
   FEATURE-TO-ENDPOINT-MAPPING.md row 10 (Phase 3 cross-ref) recommends the
   nested shape explicitly. **Recommended: nested object.** Coordinator may
   override if FE prefers flat parity with Discovery.

2. **Inline `models[]` member list vs. sub-resource.** The dbt-ui GroupView
   renders a paginated table of member models inline on the page (currently
   client-paginated; server returns all members). Inlining keeps the page to
   one round trip but unbounded — a group with 200 members returns a 200-item
   array. This mirrors `ModelDetail` Risk #5 (`depends_on`/`referenced_by`
   unbounded). v0 keeps it inline with a `?first=` cap and `truncated: true`
   flag, deferring `GET /api/v1/groups/:id/models` to when a real pagination
   need surfaces. No new ADR required — same pattern as edges on `ModelDetail`.

3. **No new capability flag.** Groups have no run/catalog/freshness surface. Per ADR-7,
   `Capabilities` is distribution-gated only — parquet-presence-based fields don't
   belong there in the first place. `meta`, `tags`, `owner.slack`, `owner.github` are
   parquet-schema verification questions, not capability questions.

---

## `GET /api/v1/groups/:id`

Powers: `GroupView` / `ResourceDetailsPage` in dbt-ui.
dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/GroupView.tsx`
GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/discovery/group.ts` (`GetGroupByUniqueId`)

Groups are **definition-only** — they have no SQL, no columns, no warehouse
relation, no run results, no catalog stats, no freshness, and no lineage edges.
The parquet source is `dbt.groups` (one row per group; columns `unique_id, name,
description, package_name, file_path, original_file_path, owner_name, owner_email,
config, ingested_at`). Member models are not stored on the group row — they are
discovered via `dbt.nodes WHERE group_name = :group.name` (the FK lives on the
node, not the group). `owner_slack` and `owner_github` are absent from the
top-level `dbt.groups` schema and are likely embedded inside the `config` JSONB
blob — verification is required before they ship; see Risk #1.

### Example response

Fields marked `// 🔧` are not yet returned — they require a backend change
(no group-detail handler exists today).
Fields marked `// 🔍` are parquet-unverified — confirm schema before implementing.

```json
{
  "unique_id": "group.jaffle_shop.finance",
  "name": "finance",
  "resource_type": "group",
  "package_name": "jaffle_shop",
  "description": "Finance domain — revenue, payments, billing models.",
  "original_file_path": "models/_groups.yml",
  "tags": ["finance", "core"],
  "owner": {
    "name": "Finance Data Team",
    "email": "finance-data@jaffle.example",
    "slack": "#finance-data",
    "github": "jaffle/finance-data-team"
  },
  "meta": { "domain": "finance", "tier": "gold" },
  "models": [
    {
      "unique_id": "model.jaffle_shop.orders",
      "name": "orders",
      "database_name": "prod",
      "schema_name": "dbt_prod",
      "contract_enforced": true
    },
    {
      "unique_id": "model.jaffle_shop.payments",
      "name": "payments",
      "database_name": "prod",
      "schema_name": "dbt_prod",
      "contract_enforced": false
    }
  ],
  "model_count": 2,
  "truncated": false,
  "ingested_at": "2026-05-19T08:30:00Z"
}
```

`owner` is `null` when neither `owner_name` nor `owner_email` is set on the
group definition. Individual sub-fields (`slack`, `github`) are independently
nullable.

`ingested_at` is the per-resource "Definition updated as of …" timestamp for
groups per ADR-5. Groups are the one ADR-5 resource type that lacks a `created_at`
column in parquet, so `dbt.groups.ingested_at` (ISO 8601) is the fallback.

`models[]` is capped at `?first=` (default 100). `truncated: true` signals the
client must paginate via the deferred `GET /api/v1/groups/:id/models` sub-resource
once it exists. `model_count` is the **total** member count, not the returned-array
length.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

No handler exists today — every field is at minimum 🔧. Fields that additionally
require schema verification are marked 🔍.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"group.pkg.name"` — primary key in `dbt.groups` |
| `name` | `string` | Core | 🔧 | — | Group name (e.g., `"finance"`) |
| `resource_type` | `"group"` | Core | 🔧 | — | Always `"group"` for this endpoint |
| `package_name` | `string \| null` | Core | 🔧 | — | From `dbt.groups.package_name` |
| `description` | `string \| null` | Core | 🔧 | — | From `dbt.groups.description` |
| `original_file_path` | `string \| null` | Core | 🔧 | — | From `dbt.groups.original_file_path` — path to the `.yml` defining the group |
| `tags` | `string[]` | Core | 🔧 | — | Empirically confirmed: NOT a top-level column in `dbt.groups.parquet` (schema is `unique_id, name, description, package_name, file_path, original_file_path, owner_name, owner_email, config, ingested_at`). Handler must `json_extract(config, '$.tags')`, defaulting to `[]` on absence — see Risk #3 |
| `owner` | `OwnerInfo \| null` | Core | 🔧 | — | Nested object — see Design note #1; `null` when no owner fields set |
| `owner.name` | `string \| null` | Core | 🔧 | — | From `dbt.groups.owner_name` |
| `owner.email` | `string \| null` | Core | 🔧 | — | From `dbt.groups.owner_email` |
| `owner.slack` | `string \| null` | Core | 🔧 | — | Empirically confirmed absent at the top level (only `owner_name` and `owner_email` are dedicated columns). Handler must `json_extract_string(config, '$.owner.slack')` if present; emit `null` otherwise — see Risk #1 |
| `owner.github` | `string \| null` | Core | 🔧 | — | Empirically confirmed absent at the top level. Same `json_extract` path on `config` as `owner.slack` — see Risk #1 |
| `meta` | `Record<string, unknown> \| null` | Core | 🔧 | — | Empirically confirmed absent at the top level. Handler must `json_extract(config, '$.meta')` and parse as JSON object; default to `null` on absence — see Risk #3 |
| `models` | `GroupMember[]` | Core | 🔧 | — | Member models from `dbt.nodes WHERE group_name = :name AND resource_type = 'model'`; capped by `?first=` — see Risk #2 |
| `models[*].unique_id` | `string` | Core | 🔧 | — | From `dbt.nodes.unique_id` |
| `models[*].name` | `string` | Core | 🔧 | — | From `dbt.nodes.name` |
| `models[*].database_name` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.database_name` |
| `models[*].schema_name` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.schema_name` |
| `models[*].contract_enforced` | `boolean \| null` | Core | 🔧 | — | From `dbt.nodes.contract_enforced`; `null` if unset (mirrors `ModelDetail`) |
| `model_count` | `number` | Core | 🔧 | — | Total count of member models — unaffected by `?first=` truncation |
| `truncated` | `boolean` | Core | 🔧 | — | `true` if `model_count > models.length`; prompts deferred sub-resource |
| `project_id` | *(absent)* | — | ❌ | — | Class B: Cloud concept; not in local parquet (Discovery `projectId` is the multi-env Cloud project ID) |
| `last_updated_at` | *(absent)* | — | ❌ | — | Class B: Cloud-managed environment timestamp; not in `dbt.groups` (the parquet has `ingested_at` which is server-local, not semantically equivalent) |
| `file_path` | *(absent)* | — | ❌ | — | Internal compiled path; `original_file_path` covers the UI use case |
| `models[*].materialized` | *(absent)* | — | ❌ | — | Out of scope for the inline summary; consumers wanting it call `GET /api/v1/models/:id` |
| `models[*].description` | *(absent)* | — | ❌ | — | Same — kept off the summary row to bound payload size |
| `depends_on` | *(absent)* | — | ❌ | — | Groups have no upstream dependencies; omit entirely (not empty array) — mirrors `SourceDetail` convention |
| `referenced_by` | *(absent)* | — | ❌ | — | The "referenced_by" relationship for a group is its `models[]` member list; do not duplicate as edges |
| `columns` | *(absent)* | — | ❌ | — | Groups are definition-only; no columns |
| `raw_code` / `compiled_code` | *(absent)* | — | ❌ | — | Groups have no SQL body |
| `materialized` / `relation_name` / `database_name` / `schema_name` / `identifier` | *(absent)* | — | ❌ | — | Groups have no warehouse relation |
| `access_level` / `group_name` / `contract_enforced` | *(absent)* | — | ❌ | — | Model-level config; not applicable to groups (a group does not belong to a group) |
| `ingested_at` | `string \| null` | Core | 🔧 | — | ISO 8601 timestamp; from `dbt.groups.ingested_at` (the most recent index write that touched this row). Per ADR-5, this is the "Definition updated as of …" timestamp for groups; **groups are the one ADR-5 resource that has no `created_at` column** in parquet (verified against the sample project schema), so `ingested_at` is the fallback. If `dbt-index` adds a `created_at` column to `dbt.groups` later, flip to `created_at` like the other 5 endpoints. |
| `execution_info` | *(absent)* | — | ❌ | — | Groups never run — definition-only. Per ADR-5 the field is omitted from `DefinitionNodeBase` entirely — this row is documentation only. |
| `catalog` | *(absent)* | — | ❌ | — | No warehouse relation; nothing to catalog |
| `freshness` | *(absent)* | — | ❌ | — | No source semantics |
| `fqn` | *(absent)* | — | ❌ | — | Not in `dbt.groups` parquet schema; groups are identified by `unique_id` alone |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api |
| `patch_path` | *(absent)* | — | ❌ | — | Class B: YAML-only resource — `original_file_path` IS the `.yml` file containing the group definition; the patch concept does not apply (a "patch" is a separate YAML that augments a non-YAML primary definition, e.g. `.sql` + `schema.yml`). Discovery's `patchPath` would be null or duplicate `originalFilePath` for this resource. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface GroupDetail {
  unique_id: string;
  name: string;
  resource_type: "group";
  package_name: string | null;
  description: string | null;
  original_file_path: string | null;
  tags: string[];
  owner: OwnerInfo | null;
  meta: Record<string, unknown> | null;
  models: GroupMember[];
  model_count: number;
  truncated: boolean;
  ingested_at: string | null;  // ADR-5: groups have no `created_at` column in parquet — `ingested_at` (ISO 8601) is the fallback
}

interface OwnerInfo {
  name: string | null;
  email: string | null;
  slack: string | null;
  github: string | null;
}

interface GroupMember {
  unique_id: string;
  name: string;
  database_name: string | null;
  schema_name: string | null;
  contract_enforced: boolean | null;
}
```

### Risk register

1. **`owner.slack` and `owner.github` absent at top level — [RESOLVED].**
   Empirically confirmed against `sl-schema-evolution/sample_project/target/index/dbt.groups.parquet`:
   the schema has only `owner_name` and `owner_email` as dedicated owner columns.
   Slack/GitHub handles, if present, live inside the `config` JSON column.
   Handler extracts via `json_extract_string(config, '$.owner.slack')` /
   `'$.owner.github'`. If the JSON doesn't contain them, fields ship as `null`.
   Frontend renders gracefully either way. **Decision: ship Core-stable with
   these documented JSON paths as the starting recommendation; if a real project
   populates these fields under a different JSON path, fix forward in a follow-up
   patch.** The sample project's 2 groups have `null` for both, so we can't
   round-trip-verify against this corpus; the integration test materializes when
   a real project surfaces the data. No dbt-index schema change required.

2. **`models[]` is unbounded without a cap.** A group with 200 member models
   returns a 200-row array. Same problem as `ModelDetail` Risk #5 for edges.
   Mitigation for v0: accept `?first=` (default 100, max 500), report `model_count`
   as the total, and set `truncated: true` when truncated. Cursor pagination via
   `GET /api/v1/groups/:id/models` is deferred until a real pagination need
   surfaces (no v0 UI consumer hits the cap — typical groups have <50 members).

3. **`tags` and `meta` parquet provenance — [RESOLVED].** Empirically
   confirmed against the sample project: neither `tags` nor `meta` is a top-level
   column in `dbt.groups.parquet`. Both must be sourced from the `config` JSON
   column (`json_extract(config, '$.tags')` / `'$.meta'`). If the JSON omits them,
   ship `[]` / `null` respectively. **Decision: same posture as Risk #1 above
   — ship Core-stable; documented paths are the starting recommendation; fix
   forward if real-project data surfaces a different JSON shape.** Sample-project
   data has empty `config` for both in its 2 rows; the integration test materializes
   when a real project surfaces the data. No dbt-index schema change required.

4. **Member-model query joins on `(package_name, name)`, not `unique_id` — [DECIDED].**
   `dbt.nodes.group_name` stores the group **name** (e.g., `"finance"`), not the full
   `unique_id` (`"group.jaffle_shop.finance"`). Decision: the handler scopes the JOIN
   by package as well as name to prevent cross-package collisions:
   `SELECT n.* FROM dbt.nodes n JOIN dbt.groups g ON n.group_name = g.name
   AND n.package_name = g.package_name WHERE g.unique_id = :id`. This matches
   dbt-core's group resolution (a group is local to its package). Verified safe
   for the two-row sample project; revisit if a multi-package project surfaces a
   case where this is wrong.

5. **No `execution_info` despite groups appearing in run-result contexts.**
   `dbt build` emits run results for member models, not for the group itself.
   The GroupView component has commented-out `updatedAt` logic — confirmed
   intentional (the Discovery API returns `lastUpdatedAt` on the parent
   `environment.definition`, not on the group). Do not add `execution_info`;
   any "last updated" surface belongs on `/api/v1/project` or a future
   environment-level endpoint, not here.

6. **`resource_type` value choice — singular vs. discriminator parity.**
   The unique_id prefix is `group.` (singular). Other resource types use the
   prefix verbatim (`model`, `source`, `seed`, `snapshot`). Choose `"group"`
   to match the prefix and the icon parity table (`group → RyeconGroup`). The
   list-endpoint surface (when `GET /api/v1/groups` lands) should also serialize
   the type as `"group"`, not `"groups"`.

7. **Definition-only resources may need `NodeBase` to be relaxed.** Per ADR-1's
   backend prerequisite, all typed detail handlers compose a shared `NodeBase`
   struct. `NodeBase` currently includes `fqn: Vec<String>` (required) — but
   groups have no `fqn` in `dbt.groups` parquet. Implementer must either
   (a) make `fqn` `Option<Vec<String>>` on `NodeBase`, (b) synthesize a 2-element
   `[package_name, name]` for groups, or (c) accept that group handler diverges
   from `NodeBase`. Decide before the generic dispatcher lands (ADR-1 deferred
   item) since the dispatcher assumes all typed handlers compose `NodeBase`.

## Design notes — `GET /api/v1/macros/:id`

Macros are the first **definition-only** resource type in this contract series — they have
no `dbt_rt.run_results` entry, no warehouse relation, no catalog stats, and no columns.
This means the contract excludes every Core-conditional surface (`execution_info`,
`catalog`, `freshness`) and every column-related field. The response is materially smaller
than `ModelDetail` / `SourceDetail` / `SeedDetail`, with no new capability flags introduced.

Two contract decisions worth flagging for coordinator review (neither rises to a full ADR
since both follow precedent set by ADR-1/ADR-2 and existing contracts):

1. **`arguments[]` is inlined, not promoted to a sub-resource.** Mirrors how `columns[]`
   is inlined on `ModelDetail`. The argument count per macro is bounded by author practice
   (typically <10), so pagination is not a concern. Shape preserved verbatim from the
   GraphQL `MacroArgument` type: `{ name, description, type }`.

2. **`depends_on` and `referenced_by` are inlined as `MacroEdgeRef[]` despite not flowing
   through `dbt.edges`.** The `dbt.edges` table is `edge_type: "ref"` only and ignores
   macro relationships entirely. Both edge sets are derivable from parquet:
   - `depends_on` from this macro's own `dbt.macros.depends_on_macros` list column;
   - `referenced_by` from inverse scans of `dbt.nodes.depends_on_macros` (and other
     resource tables that carry a `depends_on_macros` column — exposures, metrics,
     saved_queries, semantic_models, unit_tests).

   This is a handler implementation detail, not a contract change. The wire shape matches
   the existing `EdgeRef` type used by `ModelDetail` / `SourceDetail`. No new capability
   flag is needed because both are pure-parquet derivations available in Core (`dbt parse`
   suffices).

---

## `GET /api/v1/macros/:id`

Powers: `MacroView` / `ResourceDetailsPage` in dbt-ui.
dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/MacroView.tsx`
GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/discovery/macro.ts` (`GetMacroByUniqueId`)

Macros are Jinja templates compiled into nodes at parse time — pure definition-only
resources with no warehouse representation. They live in their own parquet table
(`dbt.macros`, **not** `dbt.nodes`), which is why the handler cannot share the existing
`nodes.rs` SELECT and must own its own query. As a definition-only resource, `MacroDetail`
has **no** `execution_info`, **no** `columns`, **no** `catalog`, **no** `materialized`,
**no** `relation_name`, **no** `freshness`, and **no** `database_name` / `schema_name` —
macros never land in a warehouse. The detail page renders three tabs: General (description,
metadata, relationships), Arguments (the inlined `arguments[]`), and Code (the raw
`macro_sql`).

### Example response

Fields marked `// 🔧` are not yet returned — no handler exists today; everything is 🔧.
Fields marked `// 🔍` are parquet-unverified — confirm schema before implementing.

```json
{
  "unique_id": "macro.jaffle_shop.cents_to_dollars",
  "name": "cents_to_dollars",
  "resource_type": "macro",
  "package_name": "jaffle_shop",
  "description": "Convert an integer cents column to a dollar-denominated decimal.",
  "original_file_path": "macros/cents_to_dollars.sql",
  "file_path": "macros/cents_to_dollars.sql",
  "patch_path": "macros/schema.yml",
  "macro_sql": "{% macro cents_to_dollars(column_name, scale=2) -%}\n  ({{ column_name }} / 100)::numeric(16, {{ scale }})\n{%- endmacro %}",
  "meta": { "owner": "data-eng" },
  "docs_show": true,
  "supported_languages": ["sql"],
  "arguments": [
    {
      "name": "column_name",
      "type": "string",
      "description": "The integer column holding cent values."
    },
    {
      "name": "scale",
      "type": "integer",
      "description": "Decimal scale to round the output to."
    }
  ],
  "depends_on": [
    { "unique_id": "macro.dbt.type_numeric", "edge_type": "macro" }
  ],
  "referenced_by": [
    { "unique_id": "model.jaffle_shop.orders", "edge_type": "macro" },
    { "unique_id": "model.jaffle_shop.payments", "edge_type": "macro" }
  ],
  "created_at": 1746000000.0
}
```

No capability gates apply to this response — every field is either Core (parquet-backed and
unconditional) or a Class B exclusion. No `execution_info`, `catalog`, or `freshness` block
exists for macros. `created_at` is the per-resource "Definition updated as of …" timestamp
per ADR-5 (epoch seconds, sourced from `dbt.macros.created_at`).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

No handler exists for `GET /api/v1/macros/:id` today; every included field is 🔧 (or 🔍).

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"macro.pkg.name"` — from `dbt.macros.unique_id` |
| `name` | `string` | Core | 🔧 | — | From `dbt.macros.name` |
| `resource_type` | `"macro"` | Core | 🔧 | — | Always `"macro"` for this endpoint; constant in handler — not a parquet column |
| `package_name` | `string \| null` | Core | 🔧 | — | From `dbt.macros.package_name` |
| `description` | `string \| null` | Core | 🔧 | — | From `dbt.macros.description` |
| `original_file_path` | `string \| null` | Core | 🔧 | — | From `dbt.macros.original_file_path`; relative to project root |
| `file_path` | `string \| null` | Core | 🔧 | — | From `dbt.macros.file_path`; relative to project root |
| `patch_path` | `string \| null` | Core | 🔧 | — | From `dbt.macros.patch_path`; YAML schema file declaring the macro's arguments |
| `macro_sql` | `string \| null` | Core | 🔧 | — | From `dbt.macros.macro_sql`; the Jinja template source |
| `meta` | `Record<string, unknown> \| null` | Core | 🔍 | — | JSON blob — `dbt.macros.meta` is declared `Option<String>` (serialized JSON); confirm round-trip parses cleanly. Same risk class as `meta` on `SeedDetail` / `SourceDetail` |
| `docs_show` | `boolean` | Core | 🔧 | — | From `dbt.macros.docs_show`; whether the macro should appear in generated docs. FE may use this to hide internal helpers — currently the dbt-ui MacroView does not gate on it, but the value is cheap to expose |
| `supported_languages` | `string[]` | Core | 🔧 | — | From `dbt.macros.supported_languages`; e.g., `["sql"]`, `["python"]`. Empty array if unset |
| `arguments` | `MacroArgument[]` | Core | 🔍 | — | From `dbt.macros.arguments` (stored as JSON string — `Option<String>`). Handler must `json_extract` and re-serialize as a list of `{name, type, description}` objects. Empty array if no declared arguments |
| `arguments[*].name` | `string` | Core | 🔍 | — | Required field on each argument |
| `arguments[*].type` | `string \| null` | Core | 🔍 | — | Declared argument type (e.g., `"string"`, `"integer"`); free-form Jinja convention, not validated |
| `arguments[*].description` | `string \| null` | Core | 🔍 | — | Per-argument description from YAML schema patch |
| `depends_on` | `MacroEdgeRef[]` | Core | 🔧 | — | Upstream macros this macro calls. Derived from `dbt.macros.depends_on_macros` (list column). Each entry's `edge_type` is `"macro"`. Empty array if the macro depends on no other macros |
| `depends_on[*].unique_id` | `string` | Core | 🔧 | — | e.g., `"macro.dbt.type_numeric"` |
| `depends_on[*].edge_type` | `"macro"` | Core | 🔧 | — | Always `"macro"` for macro-to-macro edges |
| `referenced_by` | `MacroEdgeRef[]` | Core | 🔧 | — | Downstream resources that invoke this macro. Derived by scanning every parquet table that carries a `depends_on_macros` list column (`dbt.nodes`, `dbt.exposures`, `dbt.metrics`, `dbt.saved_queries`, `dbt.semantic_models`, `dbt.unit_tests`, and `dbt.macros` itself) for entries containing this macro's `unique_id`. See Risk #2 |
| `referenced_by[*].unique_id` | `string` | Core | 🔧 | — | |
| `referenced_by[*].edge_type` | `"macro"` | Core | 🔧 | — | Always `"macro"` — this is a Jinja-call relationship, not a SQL `ref()` |
| `tags` | *(absent)* | — | ❌ | — | Class B for macros: `dbt.macros` parquet has no `tags` column. GraphQL exposes `tags` but it is sourced from manifest-only metadata that codex-api persists separately — no parquet path. Document explicitly so FE engineers don't chase it. See Risk #3 |
| `fqn` | *(absent)* | — | ❌ | — | Class B for macros: `dbt.macros` parquet has no `fqn` column (unlike `dbt.nodes`). dbt manifests do not assign an FQN to macros; their identity is `package.macro_name` |
| `run_id` | *(absent)* | — | ❌ | — | Class B: Cloud invocation ID; not in local parquet. GraphQL exposes `runId` but it's a CodexDB-only concept |
| `project_id` | *(absent)* | — | ❌ | — | Class B: Cloud project ID; not in local parquet. GraphQL exposes `projectId` but it's a CodexDB-only concept |
| `created_at` | `number \| null` | Core | 🔧 | — | Epoch seconds (float); from `dbt.macros.created_at`. Per ADR-5 the field is exposed on every ADR-5–scoped detail endpoint for consistency. No current dbt-ui consumer renders this for macros (`MacroView` shows `updatedAt = undefined`), but the data is free to surface and a future UI consumer can pick it up without a wire-format break. Empirically verified column present in `dbt.macros.parquet` across 671 rows. |
| `execution_info` | *(absent)* | — | ❌ | — | Macros are not runnable — `dbt_rt.run_results` does not track macro executions. Per ADR-5 the field is omitted from `DefinitionNodeBase` entirely — this row is documentation only. |
| `columns` | *(absent)* | — | ❌ | — | Macros are templates; they have no warehouse columns |
| `catalog` | *(absent)* | — | ❌ | — | Macros have no warehouse relation; no catalog stats apply |
| `materialized` | *(absent)* | — | ❌ | — | Not applicable to macros |
| `database_name` / `schema_name` / `identifier` / `relation_name` | *(absent)* | — | ❌ | — | Macros do not land in a warehouse |
| `raw_code` | *(absent)* | — | ❌ | — | Macro template source is in `macro_sql`; there is no separate `raw_code` field on `dbt.macros` |
| `compiled_code` | *(absent)* | — | ❌ | — | Macros are not compiled standalone; they are inlined into other nodes' compiled SQL |
| `access_level` / `group_name` / `contract_enforced` | *(absent)* | — | ❌ | — | Not applicable to macros |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api — consistent with `ModelDetail` precedent |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface MacroDetail {
  unique_id: string;
  name: string;
  resource_type: "macro";
  package_name: string | null;
  description: string | null;
  original_file_path: string | null;
  file_path: string | null;
  patch_path: string | null;
  macro_sql: string | null;
  meta: Record<string, unknown> | null;
  docs_show: boolean;
  supported_languages: string[];
  arguments: MacroArgument[];
  depends_on: MacroEdgeRef[];
  referenced_by: MacroEdgeRef[];
  created_at: number | null;   // ADR-5: per-resource "Definition updated as of …" timestamp; epoch seconds
}

interface MacroArgument {
  name: string;
  type: string | null;
  description: string | null;
}

// MacroEdgeRef is structurally identical to EdgeRef but with a narrower edge_type domain
interface MacroEdgeRef {
  unique_id: string;
  edge_type: "macro";
}
```

### Risk register

1. **No handler exists yet; SELECT must target `dbt.macros`, not `dbt.nodes`.** Every other
   typed detail endpoint to date (`models`, `sources`, `seeds`, `snapshots`, `tests`) reads
   primarily from `dbt.nodes`. Macros live in `dbt.macros`, a fully separate parquet table
   with its own column set. The `nodes.rs` query layout does not transfer — this endpoint
   needs its own handler file (`src/handlers/macros.rs`) with its own SELECT. `NodeBase`
   (ADR-1 backend prerequisite) still composes cleanly: `unique_id`, `name`,
   `resource_type`, `package_name`, `description`, `original_file_path`. `tags` and `fqn`
   from `NodeBase` are omitted on the wire for macros — the Rust struct will need
   per-resource-type filtering of `NodeBase` fields during serialization, OR `MacroDetail`
   declines to compose `NodeBase` and duplicates the six common columns. Decide before
   implementation.

2. **`referenced_by` requires a fan-out scan across six parquet tables — [DECIDED for v0: accept fan-out].** Macro `referenced_by` must scan `depends_on_macros` list columns on `dbt.nodes`, `dbt.exposures`, `dbt.metrics`, `dbt.saved_queries`, `dbt.semantic_models`, `dbt.unit_tests`, and `dbt.macros` itself. DuckDB's `list_contains` over a list-typed column is supported; cost is O(rows) per table per request. Decision: ship v0 with the fan-out; profile the macros endpoint against the 671-row sample project after merge. If a popular utility macro (e.g., a project-wide `dbt_utils.*` wrapper) is observed dominating request time, the follow-up is a one-time inverted-index build at server boot (`materialize macro_edges as SELECT macro_unique_id, referrer_unique_id FROM …`); the index is immutable during a server's lifetime so the build cost is paid once.

3. **`tags` is fetched by GraphQL but has no parquet path — [RESOLVED].**
   Empirically confirmed against `dbt.macros.parquet` (671 rows in the sample project):
   schema is `unique_id, name, package_name, file_path, original_file_path, macro_sql, description, depends_on_macros, arguments, docs_show, patch_path, supported_languages, meta, created_at, ingested_at` — no `tags` column. The FE must render a graceful absent state.
   Document explicitly so FE engineers don't add a `tags?` optional to the type and silently
   render an empty tag list as "no tags" when it should be "tags unavailable in this
   build." Treating as ❌ Class B with no upgrade path is the correct call.

4. **`meta` JSONB parsing is unverified.** Same parquet-storage shape as on `SeedDetail` /
   `SourceDetail` (`meta` stored as `Option<String>` JSON). Confirm DuckDB's
   `json_extract` / `json_object` round-trip cleanly into a `serde_json::Value` for the
   response. Resolved together with the same risk on the seeds/sources contracts.

5. **`arguments` JSON shape is parquet-stored, not first-class.** `dbt.macros.arguments`
   is `Option<String>` containing a serialized JSON array. The handler must parse it once
   per row, validate each entry has at least a `name` field, and re-emit as
   `MacroArgument[]`. Malformed entries (missing `name`) should be filtered out, not error
   the request — log a warning. Confirm the JSON shape against a real ingested macro before
   committing to the typed `MacroArgument` interface — if there are additional fields in
   the JSON (e.g., `default`), decide whether to surface them or strictly project to
   `{name, type, description}`.

6. **`depends_on` cardinality is unbounded but practically small.** Macros that call many
   other macros are rare; a v0 implementation can omit a `?first=` cap. If a pathological
   macro emerges (50+ upstream calls), promote to the same `truncated` + pagination story
   documented in `ModelDetail` Risk #5.

7. **`runId` / `projectId` deliberately dropped from response.** Both are CodexDB-specific
   identifiers with no analog in stateless docs. Avoid the temptation to "stub them as
   `null`" — that would imply they could one day be populated locally, which they cannot.
   Document explicitly as Class B so a future engineer doesn't try to wire them up.

8. **`MacroEdgeRef.edge_type` is a singleton constant.** Every entry in both `depends_on`
   and `referenced_by` has `edge_type: "macro"`. The literal-typed `"macro"` in the
   TypeScript interface signals this to FE engineers and forecloses on confusion with model
   `"ref"` edges. The Rust handler should emit the string literal, not derive it from
   parquet (there is no edge_type column for macro relationships).

## Design notes — `GET /api/v1/metrics/:id`

The following observations did not warrant a full ADR but should inform the integrated
contract. Promote any of these to an ADR only if the coordinator decides the question is
load-bearing for v0 implementation.

1. **No `execution_info` on metrics.** Metrics are Semantic Layer definitions, not
   warehouse-materialized objects. `dbt build` does not "run" a metric in a way that
   produces a `dbt_rt.run_results` row keyed on a `metric.*` `unique_id` — Discovery API
   reflects this (the `MetricDefinitionNode` GraphQL type has no `executionInfo`,
   `lastRunStatus`, or `lastRunError` fields). The contract omits `execution_info`
   entirely (not `null`-gated). MetricView in dbt-ui confirms this: it has only a
   `general` tab with no run-status badge.

2. **No `catalog` on metrics.** Metrics are not warehouse relations. No `dbt.catalog_tables`
   row exists for a metric `unique_id`. Omit entirely.

3. **No `columns` on metrics.** Columns are a property of relations (models, sources,
   seeds, snapshots). Metrics expose `measures`, `dimensions`, and `time_granularity`
   instead — these live on the underlying `semantic_model`, not on the metric itself in
   parquet. The dbt-ui MetricView does not render a Columns tab.

4. **`type_params` is a JSON blob, not a discriminated union.** The `dbt.metrics.parquet`
   schema stores `type_params` as an opaque JSON string (`Option<String>`, serialized via
   `jjson(m, "type_params")` in `build_metric_row`). The shape varies by metric `type`
   (`simple` uses `measure`; `ratio` uses `numerator`/`denominator`; `derived` uses
   `metrics[]` + `expr`; `cumulative` uses `window` + `grain_to_date`). This contract
   returns `type_params` as `Record<string, unknown>` and **does not** introduce a
   discriminated union on the Rust side — the front end already handles the variants
   via Zod (`zTypeParams` in the dbt-ui hook). Promoting to a discriminated union would
   require parsing JSON in the handler and would double the response-type surface area
   without a current UI consumer asking for it.

5. **`formula` is fetched by the GraphQL hook but absent from `dbt.metrics.parquet` — [RESOLVED].**
   The hook selects `formula`, and the introspected GraphQL type exposes it
   (`MetricDefinitionNode.formula: Maybe<String>`). `MetricRow` in
   `crates/dbt-index/src/parquet.rs` has no `formula` column — only `metric_filter`
   and `type_params`. Empirically confirmed against the sample project: for `derived`
   metrics the expression lives at `type_params.expr` (observed:
   `"total_enrollments / total_classes_enrolled"`). The contract classifies `formula`
   as ❌ Class B; FE reads `type_params.expr` directly for derived metrics. No dbt-index
   schema change required — see Risk #3.

6. **`runGeneratedAt` header timestamp — [RESOLVED via `created_at`].** MetricView
   renders "Definition updated as of …" in the header using
   `metric.definition.runGeneratedAt`. Prior framing claimed the parquet had no
   per-metric timestamp; empirically refuted — `dbt.metrics.parquet` has both
   `created_at: double` (epoch seconds) and `ingested_at: timestamp[us, tz=UTC]`.
   Per ADR-5, the contract surfaces `created_at` as the per-resource "Definition
   updated as of …" timestamp. `run_generated_at` itself remains ❌ Class B in the
   field reference (the Cloud-API name has no parquet analogue), but the FE no
   longer needs to fall back to project-level metadata. See Risk #7.

7. **No new capability flag introduced.** None of the metric fields require a flag.
   Per ADR-7 `Capabilities` is distribution-gated only; metrics have no execution, no
   catalog, no freshness — the question doesn't arise.

---

## `GET /api/v1/metrics/:id`

Powers: `MetricView` / `ResourceDetailsPage` in dbt-ui.
dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/MetricView.tsx`
GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/dbtStrategy/useMetric.ts` → `src/hooks/discovery/metric.ts` (`GetMetricByUniqueId`)

Metrics are Semantic Layer (MetricFlow) definitions: business-logic aggregations declared
in YAML and resolved at query time, not materialized as warehouse objects. Their parquet
home is `dbt.metrics.parquet` (`MetricRow` in `crates/dbt-index/src/parquet.rs`), which is
written by `dbt --use-index` parsing of the manifest. Each metric has a `type` discriminator
(`simple`, `ratio`, `derived`, `cumulative`) whose meaning is carried by the `type_params`
JSON blob — this contract preserves that shape rather than imposing a Rust-side discriminated
union (see Design note 4). Metrics have **no `execution_info`, `catalog`, or `columns`** —
those concepts do not apply to Semantic Layer definitions (see Design notes 1–3).

### Example response

Fields marked `// 🔧` are not yet returned — they require a backend change.
Fields marked `// 🔍` are parquet-unverified — confirm schema before implementing.

```json
{
  "unique_id": "metric.jaffle_shop.total_revenue",
  "name": "total_revenue",
  "resource_type": "metric",
  "package_name": "jaffle_shop",
  "label": "Total revenue",
  "description": "Sum of order amounts across all completed orders.",
  "original_file_path": "models/marts/metrics.yml",
  "file_path": "models/marts/metrics.yml",
  "fqn": ["jaffle_shop", "total_revenue"],
  "tags": ["finance"],
  "metric_type": "simple",
  "type_params": {
    "measure": { "name": "order_amount", "alias": null, "filter": null },
    "input_measures": [
      { "name": "order_amount", "alias": null, "filter": null }
    ]
  },
  "filter": {
    "where_filters": [
      { "where_sql_template": "{{ Dimension('orders__status') }} = 'completed'" }
    ]
  },
  "time_granularity": "day",
  "semantic_model_name": "orders",
  "input_metric_names": [],
  "group_name": "finance",
  "meta": { "owner": "data-eng" },
  "depends_on": [
    { "unique_id": "semantic_model.jaffle_shop.orders", "edge_type": "semantic_model" }
  ],
  "referenced_by": [
    { "unique_id": "saved_query.jaffle_shop.weekly_revenue", "edge_type": "saved_query" }
  ],
  "created_at": 1747432300.5
}
```

`created_at` is the per-resource "Definition updated as of …" timestamp per ADR-5
(epoch seconds, sourced from `dbt.metrics.created_at`).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"metric.pkg.name"` — from `dbt.metrics.unique_id` |
| `name` | `string` | Core | 🔧 | — | From `dbt.metrics.name` |
| `resource_type` | `"metric"` | Core | 🔧 | — | Always `"metric"` for this endpoint |
| `package_name` | `string \| null` | Core | 🔧 | — | From `dbt.metrics.package_name` |
| `label` | `string \| null` | Core | 🔧 | — | Human-readable name; from `dbt.metrics.label` |
| `description` | `string \| null` | Core | 🔧 | — | From `dbt.metrics.description` |
| `original_file_path` | `string \| null` | Core | 🔧 | — | From `dbt.metrics.original_file_path`; path to the YAML file relative to project root |
| `file_path` | `string \| null` | Core | 🔧 | — | From `dbt.metrics.file_path`; rendered model-relative path; powers MetricView header file link |
| `fqn` | `string[]` | Core | 🔧 | — | From `dbt.metrics.fqn`; rendered in GeneralView LineageSection |
| `tags` | `string[]` | Core | 🔧 | — | From `dbt.metrics.tags` |
| `metric_type` | `string \| null` | Core | 🔧 | — | `"simple"` · `"ratio"` · `"derived"` · `"cumulative"` · `"conversion"`; from `dbt.metrics.metric_type` (= manifest `type`). Discriminator for `type_params` shape — see Design note 4. Empirically verified against `dbt.metrics.parquet` in `sl-schema-evolution/sample_project` (all 5 values observed). |
| `type_params` | `Record<string, unknown> \| null` | Core | 🔧 | — | Variant-shaped per `metric_type`; from `dbt.metrics.type_params` JSON column — deserialize the stored JSON string into a JSON object. Shape mirrors manifest v10 `metrics[].type_params` |
| `filter` | `Record<string, unknown> \| null` | Core | 🔧 | — | Where-filter object; from `dbt.metrics.metric_filter` JSON column. Discovery GraphQL exposes this as untyped `JSONObject`; the dbt-ui renderer reads `filter.where_filters[].where_sql_template`. Preserve the manifest shape; do not flatten |
| `time_granularity` | `string \| null` | Core | 🔧 | — | `"day"` · `"week"` · `"month"` · `"quarter"` · `"year"`; from `dbt.metrics.time_granularity` |
| `semantic_model_name` | `string \| null` | Core | 🔧 | — | Denormalized from `type_params.metric_aggregation_params.semantic_model`; from `dbt.metrics.semantic_model_name` |
| `input_metric_names` | `string[]` | Core | 🔧 | — | Names of input metrics for `ratio` (numerator/denominator) and `derived` (metrics[]) types; from `dbt.metrics.input_metric_names` (denormalized in `build_metric_row`) |
| `group_name` | `string \| null` | Core | 🔧 | — | From `dbt.metrics.group_name` (= manifest `group`) |
| `meta` | `Record<string, unknown> \| null` | Core | 🔍 | — | JSONB blob; `dbt.metrics.meta` is `Option<String>` in `MetricRow` (JSON-serialized). Confirm DuckDB JSON parsing is wired before exposing as object vs. raw string |
| `depends_on` | `EdgeRef[]` | Core | 🔧 | — | 1-hop upstream from `dbt.edges` parquet; typically points to a `semantic_model.*` (for `simple`/`cumulative`) or `metric.*` entries (for `ratio`/`derived`). Maps to `parents` in GraphQL |
| `depends_on[*].unique_id` | `string` | Core | 🔧 | — | |
| `depends_on[*].edge_type` | `string` | Core | 🔧 | — | e.g., `"semantic_model"`, `"metric"` |
| `referenced_by` | `EdgeRef[]` | Core | 🔧 | — | 1-hop downstream; typically `saved_query.*` or downstream `metric.*` (derived/ratio consumers). Maps to `children` in GraphQL |
| `referenced_by[*].unique_id` | `string` | Core | 🔧 | — | |
| `referenced_by[*].edge_type` | `string` | Core | 🔧 | — | |
| `formula` | *(absent)* | — | ❌ | — | Class B: not in `dbt.metrics.parquet`. For `derived` metrics, the expression lives in `type_params.expr` — FE should read it there. See Design note 5 |
| `run_generated_at` | *(absent)* | — | ❌ | — | Class B: Discovery's `runGeneratedAt` is a Cloud manifest-snapshot timestamp without a parquet analogue under that name. The "Definition updated as of …" header is served by the per-resource `created_at` row above (per ADR-5); the FE consumes `created_at`, not `run_generated_at`. See Design note 6 |
| `patch_path` | *(absent)* | — | ❌ | — | Class B: `MetricRow` has no `patch_path` column (unlike `NodeRow`/`MacroRow`). Metrics are defined directly in YAML; `original_file_path` is the YAML file. The MetricView header file-link logic falls back to `filePath` |
| `created_at` | `number \| null` | Core | 🔧 | — | Epoch seconds (float); from `dbt.metrics.created_at`. Per ADR-5, this is the "Definition updated as of …" timestamp surfaced to `MetricView`. Empirically verified column present in `dbt.metrics.parquet` across 43 rows in the sample project. |
| `execution_info` | *(absent)* | — | ❌ | — | Metrics do not execute in the warehouse sense; no `dbt_rt.run_results` row keyed on `metric.*`. See Design note 1. Per ADR-5 the field is omitted from `DefinitionNodeBase` entirely — this row is documentation only. |
| `catalog` | *(absent)* | — | ❌ | — | Metrics are not warehouse relations; no `dbt.catalog_tables` row. See Design note 2 |
| `columns` | *(absent)* | — | ❌ | — | Metrics expose measures/dimensions/granularity via `type_params` and the upstream `semantic_model`, not columns. See Design note 3 |
| `materialized` | *(absent)* | — | ❌ | — | Not applicable; metrics are not materialized |
| `relation_name` | *(absent)* | — | ❌ | — | Not applicable; metrics are not warehouse objects |
| `database_name` | *(absent)* | — | ❌ | — | Not applicable; same reason as `relation_name` |
| `schema_name` | *(absent)* | — | ❌ | — | Not applicable; same reason as `relation_name` |
| `identifier` | *(absent)* | — | ❌ | — | Not applicable; same reason as `relation_name` |
| `access_level` | *(absent)* | — | ❌ | — | Model-only governance field; not applicable to metrics |
| `contract_enforced` | *(absent)* | — | ❌ | — | Model-only governance field; not applicable to metrics |
| `raw_code` | *(absent)* | — | ❌ | — | Metrics have no SQL body; closest is `type_params.expr` for derived metrics |
| `compiled_code` | *(absent)* | — | ❌ | — | Metrics have no SQL body |
| `ai_context` | *(absent)* | — | ❌ | — | `dbt.metrics.ai_context` exists but is Proprietary/dbt-v2-specific; not a Discovery-public field. Defer until a UI consumer exists |
| `config` | *(absent)* | — | ❌ | — | `dbt.metrics.config` JSON exists but has no Discovery-public schema; defer until a UI consumer exists. Mirrors `TestDetail` Risk: the GraphQL `config` blob has no FE consumer for metrics either |
| `refs` | *(absent)* | — | ❌ | — | `dbt.metrics.refs` JSON exists but is denormalized into `depends_on` via `dbt.edges`; do not duplicate |
| `sources` | *(absent)* | — | ❌ | — | Same rationale as `refs` |
| `depends_on_macros` | *(absent)* | — | ❌ | — | Denormalized into the generic `depends_on` edge view if needed; metrics rarely reference macros directly. Defer until a UI consumer exists |
| `project_id` | *(absent)* | — | ❌ | — | Class B: Cloud-specific; no parquet path |
| `last_run_id` | *(absent)* | — | ❌ | — | Class B: Cloud-specific run ID; no parquet path |
| `last_job_definition_id` | *(absent)* | — | ❌ | — | Class B: Cloud-specific job ID; no parquet path |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface MetricDetail {
  unique_id: string;
  name: string;
  resource_type: "metric";
  package_name: string | null;
  label: string | null;
  description: string | null;
  original_file_path: string | null;
  file_path: string | null;
  fqn: string[];
  tags: string[];
  metric_type: string | null;       // "simple" | "ratio" | "derived" | "cumulative" | "conversion"
  type_params: Record<string, unknown> | null;
  filter: Record<string, unknown> | null;
  time_granularity: string | null;  // "day" | "week" | "month" | "quarter" | "year"
  semantic_model_name: string | null;
  input_metric_names: string[];
  group_name: string | null;
  meta: Record<string, unknown> | null;
  depends_on: EdgeRef[];
  referenced_by: EdgeRef[];
  created_at: number | null;        // ADR-5: per-resource "Definition updated as of …" timestamp; epoch seconds
}

// EdgeRef is shared with ModelDetail, SourceDetail, SeedDetail, SnapshotDetail, TestDetail
interface EdgeRef {
  unique_id: string;
  edge_type: string;
}
```

### Risk register

1. **`type_params` JSON deserialization shape.** `dbt.metrics.parquet` stores
   `type_params` as a JSON-serialized string (`Option<String>` in `MetricRow`). The
   handler must deserialize it into a JSON object for the response; returning the raw
   string would leak an implementation detail and break the FE Zod schema
   (`zTypeParams`). Verify DuckDB `json_parse`/`json` extension availability in the
   `dbt-docs-server` query path, or perform deserialization in Rust after the Arrow
   batch is returned. Same risk applies to `filter`, `meta`, `refs`, `sources`, `config`.

2. **`filter` shape divergence between manifest and renderer — [DECIDED: our contract follows manifest/parquet truth].** The dbt-ui Zod schema in `discovery/metric.ts` declares `zFilter` as `{ where_sql_template: string | null }`, but `MetricFilterTable.tsx` reads `metric.filter.where_filters[].where_sql_template`. The GraphQL `MetricDefinitionNode.filter` is typed as untyped `JSONObject`. The manifest v10 shape (which `dbt.metrics.metric_filter` mirrors via `jjson(m, "filter")`) has `{ where_filters: [{ where_sql_template }] }`. **The dbt-docs-server contract uses the nested `where_filters[]` shape that matches the manifest.** The FE Zod inconsistency is an internal dbt-ui issue — flag it separately so the FE schema is fixed before consuming this endpoint; it doesn't block our contract.

3. **`formula` is fetched but absent from parquet — [RESOLVED].** The GraphQL hook
   selects `formula`, but `MetricRow` has no `formula` column. Empirically verified
   against `sl-schema-evolution/sample_project/target/index/dbt.metrics.parquet`: for
   `derived` metrics the expression lives at `type_params.expr` (observed value:
   `"total_enrollments / total_classes_enrolled"`). The contract treats `formula` as
   ❌ Class B; the FE reads `type_params.expr` directly when it needs the derived-metric
   expression. No dbt-index change required.

4. **`meta` JSONB presence — [RESOLVED].** Same parquet-storage shape as on
   `SourceDetail` / `SeedDetail` (`meta` stored as `Option<String>` JSON). Empirically
   verified: the `meta` column is present in `dbt.metrics.parquet` (confirmed via the
   sample project schema). Handler must JSON-parse on the way out; rolls into the
   cross-cutting JSON helper decision (see Open Question Q4 in the PR description).

5. **`depends_on` may include `semantic_model.*` and `metric.*` mixed.** Unlike model
   `depends_on` (which is typically `model.*`/`source.*`/`seed.*`), metric upstream
   edges depend on the metric `type`: `simple`/`cumulative` depend on a `semantic_model.*`;
   `ratio`/`derived` depend on other `metric.*` entries. The FE must not assume a single
   upstream resource type. Document explicitly so FE engineers don't filter edges by
   `edge_type === "model"` and silently drop semantic model parents.

6. **Q35 semantic-layer blocker — [RETIRED].** Prior framing claimed
   `dbt.metrics.parquet` would be empty for OSS Core projects pending Core v2.
   Empirically refuted: the `sl-schema-evolution/sample_project` index contains 43
   metric rows across all 5 `metric_type` variants, written by the standard
   index path (`.artifact_meta.json: write_source: "DirectWrite"`). The SL parquet
   tables are emitted today by any project with a `semantic_manifest.json`, regardless
   of toolchain. No 404 risk; no capability gate needed.

7. **`run_generated_at` mapping — [RESOLVED].** Prior framing claimed no per-metric
   timestamp existed. Empirically refuted: `dbt.metrics.parquet` has **both**
   `created_at: double` (epoch seconds, when the metric was first ingested) and
   `ingested_at: timestamp[us, tz=UTC]` (when this index write touched the row).
   Recommendation: surface `created_at` as the resource's "Definition updated as of …"
   timestamp in the response (Core 🔧). FE no longer needs a project-level fallback.

8. **No pagination cap on `depends_on`/`referenced_by`.** Same risk as `ModelDetail`
   Risk #5 and `SnapshotDetail` Risk #4. A `derived` metric that aggregates many input
   metrics, or a popular metric referenced by many saved queries, would return an
   unbounded array. Add a `?first=` cap with `truncated: true` consistent with the
   model and snapshot contracts.

## Design notes — `GET /api/v1/saved_queries/:id`

Two judgment calls in this contract. Both are now codified by ADR-5 (Semantic-Layer
resources omit `execution_info` entirely) and CC-7 (JSON-string columns are parsed
handler-side). The notes below remain as supporting context for the saved-queries
endpoint specifically:

  1. Lives in a dedicated `dbt.saved_queries` parquet table (no `dbt.nodes` row).
  2. Has no `execution_info` analogue — saved queries are a Semantic Layer definition,
     not a build target. `dbt build` does not produce a `dbt_rt.run_results` row for a
     `saved_query.*` unique_id.

**1. JSON-column unpacking convention (`query_params`, `exports`).**

The parquet schema (`SavedQueryRow` in `crates/dbt-index/src/parquet.rs:1252`) stores
`query_params` and `exports` as opaque JSON strings, not as Arrow nested types. The
contract returns them as fully parsed JSON objects whose shape matches the Discovery
API field-for-field. This is consistent with CC-2 (preserve nested Discovery shape) and
extends precedent set by `catalog.stats[]`, which is also handler-parsed. Risk #1
captures the DuckDB `json_extract` / parse-side cost. Decision: the handler parses on
the way out; the REST contract MUST NOT expose stringified JSON.

**2. No `execution_info` on this response.**

Saved queries are not built. They are queried at runtime through the Semantic Layer
service against `dbt-mantle`/`dbt_sl`. The on-disk `dbt_rt.run_results.parquet`
contains rows for `model.*`, `seed.*`, `snapshot.*`, `test.*`, `unit_test.*` —
**never** `saved_query.*`. Therefore this contract omits `execution_info` entirely,
and the Field reference table has no `Core-conditional` rows for run state. The
closest analogue — "definition last generated at" — is exposed as `created_at`
(parquet-backed; epoch seconds in `dbt.saved_queries.created_at`).

---

## `GET /api/v1/saved_queries/:id`

Powers: `SavedQueryView` / `ResourceDetailsPage` in dbt-ui.
dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/SavedQueryView.tsx`
GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/discovery/savedQuery.ts` (`GetSavedQueryByUniqueId`) and `src/hooks/dbtStrategy/useSavedQuery.ts`

Saved queries are Semantic Layer entities that bundle a metric selection
(`metrics[]`), grouping (`group_by[]`), filtering (`where.where_filters[]`), and
optional ordering/limit into a reusable query, plus zero-or-more **exports** — saved
materializations of the query result into a warehouse table or view. They live in
`dbt.saved_queries.parquet` (see `SavedQueryRow` in `crates/dbt-index/src/parquet.rs`),
**not** in `dbt.nodes` — they have no SQL body, no warehouse relation of their own
(exports materialize into separate relations), no run results, no columns, and no
catalog. `depends_on` typically references the metrics and semantic models the query
selects from; `referenced_by` is generally empty (nothing depends on a saved query in
the build graph). `query_params` and `exports` are parquet-stored JSON strings — the
handler parses them server-side; the REST contract returns nested JSON objects per
CC-2.

### Example response

Fields marked `// 🔧` are not yet returned — there is no `/api/v1/saved_queries/:id`
handler today. Fields marked `// 🔍` are parquet presence unverified — confirm
schema before implementing.

```json
{
  "unique_id": "saved_query.jaffle_shop.weekly_revenue_summary",
  "name": "weekly_revenue_summary",
  "resource_type": "saved_query",
  "label": "Weekly Revenue Summary",
  "package_name": "jaffle_shop",
  "description": "Weekly revenue by region, materialized to the analytics schema.",
  "original_file_path": "models/semantic/saved_queries.yml",
  "file_path": "models/semantic/saved_queries.yml",
  "fqn": ["jaffle_shop", "semantic", "weekly_revenue_summary"],
  "tags": ["finance", "weekly"],
  "group_name": "finance",
  "created_at": 1747320731.0,
  "query_params": {
    "metrics": ["revenue", "order_count"],
    "group_by": ["customer__region", "metric_time__week"],
    "order_by": ["-metric_time__week"],
    "limit": 1000,
    "where": {
      "where_filters": [
        { "where_sql_template": "{{ Dimension('customer__region') }} != 'INTERNAL'" }
      ]
    }
  },
  "exports": [
    {
      "name": "weekly_revenue_summary__warehouse",
      "config": {
        "alias": "weekly_revenue_summary",
        "export_as": "table",
        "schema": "analytics",
        "database": "prod"
      }
    }
  ],
  "depends_on": [
    { "unique_id": "metric.jaffle_shop.revenue", "edge_type": "metric" },
    { "unique_id": "metric.jaffle_shop.order_count", "edge_type": "metric" },
    { "unique_id": "semantic_model.jaffle_shop.customers", "edge_type": "semantic_model" }
  ],
  "referenced_by": []
}
```

There are no capability-gated fields on this response: saved queries have no
`execution_info`, no `catalog`, no `freshness`. See Design note 2.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"saved_query.pkg.name"`; from `dbt.saved_queries.unique_id` |
| `name` | `string` | Core | 🔧 | — | from `dbt.saved_queries.name` |
| `resource_type` | `"saved_query"` | Core | 🔧 | — | Always `"saved_query"` for this endpoint |
| `label` | `string \| null` | Core | 🔧 | — | Display label; from `dbt.saved_queries.label` |
| `package_name` | `string \| null` | Core | 🔧 | — | from `dbt.saved_queries.package_name` |
| `description` | `string \| null` | Core | 🔧 | — | from `dbt.saved_queries.description` |
| `original_file_path` | `string \| null` | Core | 🔧 | — | YAML file path relative to project root |
| `file_path` | `string \| null` | Core | 🔧 | — | from `dbt.saved_queries.file_path`; same `.yml` as `original_file_path` for most projects |
| `fqn` | `string[]` | Core | 🔧 | — | from `dbt.saved_queries.fqn` |
| `tags` | `string[]` | Core | 🔧 | — | from `dbt.saved_queries.tags` |
| `group_name` | `string \| null` | Core | 🔧 | — | from `dbt.saved_queries.group_name` |
| `created_at` | `number \| null` | Core | 🔧 | — | Epoch seconds (float); from `dbt.saved_queries.created_at`. Per ADR-5, this is the "Definition updated as of …" timestamp surfaced to `SavedQueryView`; Discovery API analogue is `runGeneratedAt` — see Risk #5. Empirically verified column present in the sample project. |
| `query_params` | `QueryParams \| null` | Core | 🔧 | — | Parsed from JSON-string column `dbt.saved_queries.query_params`; see Design note 1 |
| `query_params.metrics` | `string[]` | Core | 🔧 | — | List of metric names selected by the query |
| `query_params.group_by` | `string[]` | Core | 🔧 | — | List of dimension or entity references (e.g., `"customer__region"`) |
| `query_params.order_by` | `string[]` | Core | 🔍 | — | Discovery API returns flat strings (e.g., `"-metric_time__week"`); confirm parquet JSON shape — see Risk #2 |
| `query_params.limit` | `number \| null` | Core | 🔧 | — | Row cap applied at SL query time |
| `query_params.where` | `QueryParamsWhere \| null` | Core | 🔧 | — | Wrapper object holding `where_filters[]` |
| `query_params.where.where_filters` | `WhereFilter[]` | Core | 🔧 | — | Empty array if no filters |
| `query_params.where.where_filters[*].where_sql_template` | `string` | Core | 🔧 | — | Jinja-templated SQL filter — Discovery API: `whereSqlTemplate` (CC-1 rewrites to snake_case) |
| `exports` | `Export[]` | Core | 🔧 | — | Parsed from JSON-string column `dbt.saved_queries.exports`; empty array if no exports defined |
| `exports[*].name` | `string` | Core | 🔧 | — | Export identifier (used as the materialized relation suffix) |
| `exports[*].config` | `ExportConfig \| null` | Core | 🔧 | — | Materialization config; `null` only if YAML omits the `config:` block |
| `exports[*].config.alias` | `string \| null` | Core | 🔧 | — | Override for the materialized relation name |
| `exports[*].config.export_as` | `string \| null` | Core | 🔧 | — | `"table"` · `"view"` — Discovery API: `exportAs` (CC-1 rewrites to snake_case) |
| `exports[*].config.schema` | `string \| null` | Core | 🔧 | — | Schema for the materialized relation |
| `exports[*].config.database` | `string \| null` | Core | 🔧 | — | Database for the materialized relation |
| `depends_on` | `EdgeRef[]` | Core | 🔧 | — | 1-hop upstream from `dbt.edges`; typically metrics + semantic models |
| `depends_on[*].unique_id` | `string` | Core | 🔧 | — | |
| `depends_on[*].edge_type` | `string` | Core | 🔧 | — | |
| `referenced_by` | `EdgeRef[]` | Core | 🔧 | — | 1-hop downstream from `dbt.edges`; typically empty |
| `referenced_by[*].unique_id` | `string` | Core | 🔧 | — | |
| `referenced_by[*].edge_type` | `string` | Core | 🔧 | — | |
| `parents` | *(absent)* | — | ❌ | — | Discovery API exposes `parents` (full node summaries). dbt-docs-server uses `depends_on` (edge refs only). FE caller resolves names via `GET /api/v1/nodes/:id` if needed. |
| `children` | *(absent)* | — | ❌ | — | Same as `parents` — covered by `referenced_by`. |
| `project_id` | *(absent)* | — | ❌ | — | Class B: Cloud project ID; not in local parquet |
| `run_generated_at` | *(absent)* | — | ❌ | — | Class B: Cloud manifest snapshot timestamp. Closest local analogue is `created_at` — see Risk #5 |
| `execution_info` | *(absent)* | — | ❌ | — | Saved queries are never executed by `dbt build`; no `dbt_rt.run_results` row exists. See Design note 2. Per ADR-5 the field is omitted from `DefinitionNodeBase` entirely — this row is documentation only. |
| `columns` | *(absent)* | — | ❌ | — | Saved queries have no declared columns — the column set is derived at SL query time from `query_params.metrics` and `query_params.group_by` |
| `catalog` | *(absent)* | — | ❌ | — | Saved queries have no warehouse relation of their own (exports do, but those are separate models from dbt's perspective) |
| `materialized` | *(absent)* | — | ❌ | — | Materialization lives on each export (`exports[*].config.export_as`), not on the saved query itself |
| `relation_name` | *(absent)* | — | ❌ | — | See `materialized` — relations are per-export |
| `raw_code` | *(absent)* | — | ❌ | — | Saved queries are declarative YAML, not SQL |
| `compiled_code` | *(absent)* | — | ❌ | — | Saved queries are declarative YAML, not SQL |
| `meta` | *(absent)* | — | ❌ | — | `dbt.saved_queries` schema has no `meta` column (unlike `dbt.nodes`); the `config` column exists but is not exposed |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api |
| `patch_path` | *(absent)* | — | ❌ | — | Class B: YAML-only resource — `original_file_path` IS the `.yml` file containing the saved query definition; the patch concept does not apply (a "patch" is a separate YAML that augments a non-YAML primary definition, e.g. `.sql` + `schema.yml`). Discovery's `patchPath` would be null or duplicate `originalFilePath` for this resource. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface SavedQueryDetail {
  unique_id: string;
  name: string;
  resource_type: "saved_query";
  label: string | null;
  package_name: string | null;
  description: string | null;
  original_file_path: string | null;
  file_path: string | null;
  fqn: string[];
  tags: string[];
  group_name: string | null;
  created_at: number | null;
  query_params: QueryParams | null;
  exports: Export[];
  depends_on: EdgeRef[];
  referenced_by: EdgeRef[];
}

interface QueryParams {
  metrics: string[];
  group_by: string[];
  order_by: string[];
  limit: number | null;
  where: QueryParamsWhere | null;
}

interface QueryParamsWhere {
  where_filters: WhereFilter[];
}

interface WhereFilter {
  where_sql_template: string;
}

interface Export {
  name: string;
  config: ExportConfig | null;
}

interface ExportConfig {
  alias: string | null;
  export_as: string | null;
  schema: string | null;
  database: string | null;
}

// EdgeRef is shared with ModelDetail, SourceDetail, SeedDetail, SnapshotDetail
interface EdgeRef {
  unique_id: string;
  edge_type: string;
}
```

### Risk register

1. **`query_params` and `exports` are JSON strings in parquet.** Both columns are
   `Option<String>` in `SavedQueryRow` (`crates/dbt-index/src/parquet.rs:1261-1262`).
   The handler must parse them server-side and emit nested JSON objects matching the
   contract above. Options: (a) DuckDB `json_extract` per field, or (b) read the raw
   string and parse in Rust via `serde_json`. Option (b) is simpler and avoids one
   query plan per nested field; recommend it unless profiling shows a hot path. If
   parsing fails for a malformed row, return `null` for the affected field — never a
   stringified blob.

2. **`query_params.order_by` shape is unverified.** The Discovery GraphQL surface
   returns `orderBy` as a flat `string[]` (each entry encoding direction with a `-`
   prefix). The parquet JSON blob is whatever the dbt parser emits — it could be flat
   strings, or it could be objects like `{ "metric": "...", "descending": true }`.
   Inspect a real `dbt.saved_queries.parquet` `query_params` value before implementing.
   If the shape diverges from Discovery, adopt the parquet shape and document the
   transformation here. Recommend treating this field as 🔍 until verified.

3. **`depends_on` edges may include macros, not just metrics/semantic models.**
   `SavedQueryRow` has both `depends_on_nodes: Vec<String>` and
   `depends_on_macros: Vec<String>`. The Discovery API's `parents` field surfaces
   resource nodes (metrics, semantic models), not macros. The handler should
   probably restrict `depends_on` to non-macro edges — otherwise the FE will render
   a `Macro` chip for every Jinja templating dependency, which is noise. Decide:
   include macros in `depends_on` (consistent with `ModelDetail`), or filter them
   out (consistent with the UI expectation). Default to the model precedent
   (include); flag in implementation review if the UX feels wrong.

4. **`referenced_by` is typically empty but not guaranteed.** A saved query can
   theoretically be referenced by an `exposure`. Verify `dbt.edges.parquet` records
   `parent_unique_id` for `saved_query.*` when an exposure depends on a saved query.
   If yes, this contract is correct as-is. If no (exposures only reference models),
   strike `referenced_by` from the response. Verify with a project that exercises
   the case before implementing.

5. **`run_generated_at` ≠ `created_at`.** The Discovery API field `runGeneratedAt`
   is the manifest-generation timestamp from CodexDB, which is a project-wide
   concept (when the latest manifest was ingested). The parquet `created_at` is a
   per-row epoch-seconds float that may represent the parse-time of the saved query
   YAML. These are **not** the same thing. The SavedQueryView header renders
   "Definition updated as of <date>" using `runGeneratedAt` — if `created_at` is
   project-wide-constant in parquet, it serves the same UX purpose; if it varies
   per-row, it conveys something more granular and useful. Either way, document the
   semantic difference in the FE-facing API docs so engineers don't expect Cloud
   parity.

6. **No execution_info on this endpoint.** This is intentional (Design note 2), but
   worth restating for any future engineer who sees runnable detail endpoints carry
   `execution_info` and asks why saved queries don't. Document at the top of the
   handler: "Saved queries are
   declarative SL definitions; they have no run-time execution status. If a saved
   query's exports are materialized, run status lives on the resulting model
   nodes, queryable via `GET /api/v1/models/:id`."

7. **`exports[*].config` may be `null` for sparse YAML.** A saved query with no
   `config:` block under its export still has a `name`. The handler must tolerate
   `{ "name": "...", "config": null }` rather than synthesizing an empty
   `ExportConfig`. The FE will render `config: null` as "No materialization
   configured" rather than four empty cells.

## Design notes — `GET /api/v1/semantic_models/:id`

Three non-obvious decisions arise here. The coordinator should decide whether any of these warrant promotion to a full ADR before this contract is merged into `API-CONTRACTS.md`.

**1. `entities`, `dimensions`, `measures` are inlined as arrays, not promoted to sub-resources.**
The dbt-ui detail page renders all three on the same view via tabs and section components (`DimensionsView`, `MeasuresView`, `SemanticModelEntities`) — they are conceptually part of the semantic model itself, not independent resources. Inline mirrors the GraphQL shape (Discovery returns them on the `SemanticModelDefinitionNode`) and is consistent with how `columns[]` is inlined on `ModelDetail`. The fan-out is bounded by spec authorship (typically tens of entries, not hundreds) so no pagination cap is proposed. If a future "metric usage" surface needs to look up measures across all semantic models, a `GET /api/v1/measures` collection endpoint can be added additively.

**2. No `execution_info`, no `catalog`, no capability gating.**
Semantic models are **spec-only** — they declare entities/dimensions/measures on top of an existing model but are not themselves executed against the warehouse. Their parquet source (`dbt.semantic_models` + `dbt.semantic_{entities,measures,dimensions}`) is written by `dbt parse` / `dbt build` during semantic-manifest ingestion (see `crates/dbt-index/src/ingest/semantic_manifest.rs`) and contains no run-result columns. `RESOURCES_WITH_EXECUTION_INFO` in dbt-ui (`hooks/discovery/types.ts:53`) confirms only `Model | Seed | Snapshot` carry execution_info. ADR-2 doesn't apply (no `dbt_rt.run_results` row). ADR-4 (bare execution_info naming) is moot.

**3. Measure `agg` and dimension `type` are surfaced as raw strings, not discriminated unions.**
The dbt-ui `SemanticAspectCard` renders `agg` and `type` as uppercase badges (`SUM`, `COUNT_DISTINCT`, `CATEGORICAL`, `TIME`) with no behavior conditional on the value. MetricFlow defines a closed set of enum values for both, but the consumer treats them as opaque strings. Keep as `string | null` rather than introducing a TypeScript union — keeps the contract stable as MetricFlow extends the enums and matches the precedent set by `materialized` and `access_level` on `ModelDetail`. A measure's `agg_params` (e.g., the `percentile` argument for `percentile` agg) is exposed alongside as an opaque JSON-string column (see `dbt.semantic_measures.agg_params`); if a frontend needs typed access it should parse it locally.

---

## `GET /api/v1/semantic_models/:id`

Powers: `SemanticModelView` / `ResourceDetailsPage` in dbt-ui.
dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/details/components/DetailPages/SemanticModelView.tsx`
GraphQL hooks: `packages/metadata/dbt-explorer/src/hooks/dbtStrategy/useSemanticModel.ts` → `src/hooks/discovery/semanticModel.ts` (`GetSemanticModelByUniqueId`)

Semantic models are MetricFlow / Semantic Layer specs that bind structured aggregation surfaces (entities, dimensions, measures) onto an underlying dbt model. They are **spec-only** — defined in `.yml` and parsed during `dbt parse` / `dbt build`, but never themselves executed against the warehouse. Their parquet source is the `dbt.semantic_models` table (one row per semantic model) plus three sibling tables joined on the parent `unique_id`: `dbt.semantic_entities`, `dbt.semantic_measures`, `dbt.semantic_dimensions`. Because there are no run results, this endpoint has **no `execution_info`, no `catalog`, and no capability gating** — every Class A field is unconditional. The upstream relation is the model the semantic model is built on (the `model:` reference in YAML, captured as `dbt.semantic_models.model` and as a single edge in `dbt.edges`). Downstream consumers are metrics that reference the measures and saved queries that select the dimensions.

### Example response

Fields marked `// 🔧` are not yet returned — there is no handler today; expect almost every field to be a fresh `SELECT`.
Fields marked `// 🔍` are parquet presence unverified — confirm before implementing.

```json
{
  "unique_id": "semantic_model.jaffle_shop.orders",
  "name": "orders",
  "resource_type": "semantic_model",
  "package_name": "jaffle_shop",
  "description": "Semantic model over the orders fact table.",
  "label": "Orders",
  "original_file_path": "models/semantic_models.yml",
  "file_path": "semantic_models.yml",
  "tags": ["finance", "semantic"],
  "fqn": ["jaffle_shop", "semantic_models", "orders"],
  "meta": { "owner": "data-eng" },
  "group_name": "finance",
  "model": {
    "unique_id": "model.jaffle_shop.fct_orders",
    "name": "fct_orders",
    "access_level": "public",
    "alias": "fct_orders"
  },
  "primary_entity": "order",
  "entities": [
    {
      "name": "order",
      "type": "primary",
      "description": "Unique order identifier.",
      "label": null,
      "expr": "order_id",
      "role": null
    },
    {
      "name": "customer",
      "type": "foreign",
      "description": "Customer that placed the order.",
      "label": null,
      "expr": "customer_id",
      "role": null
    }
  ],
  "dimensions": [
    {
      "name": "ordered_at",
      "type": "time",
      "description": "Timestamp the order was placed.",
      "label": null,
      "expr": "ordered_at",
      "is_partition": false,
      "time_granularity": "day",
      "type_params": { "time_granularity": "day" }
    },
    {
      "name": "status",
      "type": "categorical",
      "description": "Order lifecycle status.",
      "label": null,
      "expr": "status",
      "is_partition": false,
      "time_granularity": null,
      "type_params": null
    }
  ],
  "measures": [
    {
      "name": "order_total",
      "agg": "sum",
      "description": "Sum of order totals.",
      "label": null,
      "expr": "amount",
      "create_metric": true,
      "agg_time_dimension": "ordered_at",
      "agg_params": null,
      "non_additive_dimension": null
    },
    {
      "name": "order_count",
      "agg": "count",
      "description": "Number of orders.",
      "label": null,
      "expr": "1",
      "create_metric": false,
      "agg_time_dimension": "ordered_at",
      "agg_params": null,
      "non_additive_dimension": null
    }
  ],
  "depends_on": [
    { "unique_id": "model.jaffle_shop.fct_orders", "edge_type": "ref" }
  ],
  "referenced_by": [
    { "unique_id": "metric.jaffle_shop.total_orders", "edge_type": "metric" },
    { "unique_id": "saved_query.jaffle_shop.orders_by_month", "edge_type": "saved_query" }
  ],
  "created_at": 1747432300.5
}
```

`created_at` is the per-resource "Definition updated as of …" timestamp per ADR-5
(epoch seconds, sourced from `dbt.semantic_models.created_at`).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

There is no `GET /api/v1/semantic_models/:id` handler today, so every Class A row below is 🔧 (or 🔍 where parquet presence is unverified). Class A rows are the bulk of the contract.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"semantic_model.pkg.name"` |
| `name` | `string` | Core | 🔧 | — | From `dbt.semantic_models.name` |
| `resource_type` | `"semantic_model"` | Core | 🔧 | — | Always `"semantic_model"` for this endpoint |
| `package_name` | `string \| null` | Core | 🔧 | — | From `dbt.semantic_models.package_name` |
| `description` | `string \| null` | Core | 🔧 | — | |
| `label` | `string \| null` | Core | 🔧 | — | Human-readable label from `dbt.semantic_models.label` |
| `original_file_path` | `string \| null` | Core | 🔧 | — | YAML file containing the semantic model spec |
| `file_path` | `string \| null` | Core | 🔧 | — | `dbt.semantic_models.file_path` (relative) |
| `patch_path` | *(absent)* | — | ❌ | — | Class B: YAML-only resource — `dbt.semantic_models` parquet has no `patch_path` column. `original_file_path` IS the YAML file containing the spec; the patch concept does not apply (a "patch" is a separate YAML that augments a non-YAML primary definition, e.g. `.sql` + `schema.yml`). Discovery's `patchPath` would be null or duplicate `originalFilePath` for this resource. |
| `tags` | `string[]` | Core | 🔧 | — | Empirically verified absent at the top-level of `dbt.semantic_models.parquet`. Handler must extract from the `config` JSON column via `json_extract(config, '$.tags')`, defaulting to `[]` when absent — see Risk #2 |
| `fqn` | `string[]` | Core | 🔧 | — | `dbt.semantic_models.fqn` (3-part: `[pkg, semantic_models, name]`) |
| `meta` | `Record<string, unknown> \| null` | Core | 🔍 | — | Likely embedded in `config` JSON column on `dbt.semantic_models`; same risk class as model `meta` |
| `group_name` | `string \| null` | Core | 🔧 | — | `dbt.semantic_models.group_name` |
| `model` | `UpstreamModelRef \| null` | Core | 🔧 | — | The model the semantic model is built on; from `dbt.semantic_models.model` joined to `dbt.nodes` for `access_level`/`alias` |
| `model.unique_id` | `string` | Core | 🔧 | — | Direct read of `dbt.semantic_models.model` — empirically verified to store an already-resolved `model.{pkg}.{name}` unique_id (e.g. `"model.another_semantic_model"`), not a raw `ref()` string. No edges JOIN needed. |
| `model.name` | `string` | Core | 🔧 | — | |
| `model.access_level` | `string \| null` | Core | 🔧 | — | Pulled from the joined model row |
| `model.alias` | `string \| null` | Core | 🔧 | — | Pulled from the joined model row |
| `primary_entity` | `string \| null` | Core | 🔧 | — | `dbt.semantic_models.primary_entity`; entity name designated primary at the model level |
| `entities` | `SemanticEntity[]` | Core | 🔧 | — | All rows of `dbt.semantic_entities` where `unique_id` matches; empty array if none |
| `entities[*].name` | `string` | Core | 🔧 | — | |
| `entities[*].type` | `string \| null` | Core | 🔧 | — | `"primary"` · `"natural"` · `"foreign"` · `"unique"` (MetricFlow enum) |
| `entities[*].description` | `string \| null` | Core | 🔧 | — | |
| `entities[*].label` | `string \| null` | Core | 🔧 | — | |
| `entities[*].expr` | `string \| null` | Core | 🔧 | — | SQL expression resolving to the entity column |
| `entities[*].role` | `string \| null` | Core | 🔧 | — | `dbt.semantic_entities.entity_role`; aliased to `role` in JSON to drop redundant prefix |
| `dimensions` | `SemanticDimension[]` | Core | 🔧 | — | All rows of `dbt.semantic_dimensions` where `unique_id` matches |
| `dimensions[*].name` | `string` | Core | 🔧 | — | |
| `dimensions[*].type` | `string \| null` | Core | 🔧 | — | `"time"` · `"categorical"` (MetricFlow enum) — from `dbt.semantic_dimensions.dimension_type`; aliased to `type` in JSON to match GraphQL |
| `dimensions[*].description` | `string \| null` | Core | 🔧 | — | |
| `dimensions[*].label` | `string \| null` | Core | 🔧 | — | |
| `dimensions[*].expr` | `string \| null` | Core | 🔧 | — | SQL expression resolving to the dimension column |
| `dimensions[*].is_partition` | `boolean \| null` | Core | 🔧 | — | Whether dimension is a partition column (time dimensions only) |
| `dimensions[*].time_granularity` | `string \| null` | Core | 🔧 | — | `"day"` · `"week"` · `"month"` etc.; populated only for `type == "time"` |
| `dimensions[*].type_params` | *(absent)* | — | ❌ | — | Class B: empirically verified — `dbt.semantic_dimensions` parquet schema has no `type_params` column. Equivalent information is split across `dimension_type` (categorical/time), `time_granularity`, and `validity_params` (JSON-encoded), each of which IS a parquet column and is exposed directly. The FE should consume those instead of expecting a combined `type_params` object — see Risk #4 |
| `measures` | `SemanticMeasure[]` | Core | 🔧 | — | All rows of `dbt.semantic_measures` where `unique_id` matches |
| `measures[*].name` | `string` | Core | 🔧 | — | |
| `measures[*].agg` | `string \| null` | Core | 🔧 | — | `"sum"` · `"count"` · `"count_distinct"` · `"average"` · `"max"` · `"min"` · `"percentile"` · `"sum_boolean"` · `"median"` (MetricFlow enum) |
| `measures[*].description` | `string \| null` | Core | 🔧 | — | |
| `measures[*].label` | `string \| null` | Core | 🔧 | — | |
| `measures[*].expr` | `string \| null` | Core | 🔧 | — | SQL expression resolving to the measure column |
| `measures[*].create_metric` | `boolean \| null` | Core | 🔧 | — | Whether this measure auto-generates a simple metric |
| `measures[*].agg_time_dimension` | `string \| null` | Core | 🔧 | — | Time dimension this measure is aggregated over (for time-aware metrics) |
| `measures[*].agg_params` | `Record<string, unknown> \| null` | Core | 🔧 | — | Opaque JSON; e.g., `{"percentile": 0.95, "use_discrete_percentile": false}` for `agg == "percentile"`. Stored as JSON-string column; parse before emission. |
| `measures[*].non_additive_dimension` | `Record<string, unknown> \| null` | Core | 🔧 | — | Opaque JSON; defines a dimension along which the measure cannot be naively summed (e.g., end-of-period balances) |
| `depends_on` | `EdgeRef[]` | Core | 🔧 | — | 1-hop upstream — exactly one entry pointing at the underlying model. From `dbt.edges` filtered by `from = this.unique_id`. |
| `depends_on[*].unique_id` | `string` | Core | 🔧 | — | |
| `depends_on[*].edge_type` | `string` | Core | 🔧 | — | Typically `"ref"` |
| `referenced_by` | `EdgeRef[]` | Core | 🔧 | — | 1-hop downstream — metrics and saved queries that consume this semantic model. From `dbt.edges` filtered by `to = this.unique_id`. |
| `referenced_by[*].unique_id` | `string` | Core | 🔧 | — | |
| `referenced_by[*].edge_type` | `string` | Core | 🔧 | — | |
| `materialized` | *(absent)* | — | ❌ | — | Semantic models are spec-only; no materialization |
| `database_name` | *(absent)* | — | ❌ | — | Semantic models reference a model; no warehouse relation of their own |
| `schema_name` | *(absent)* | — | ❌ | — | Same reason |
| `relation_name` | *(absent)* | — | ❌ | — | Same reason |
| `identifier` | *(absent)* | — | ❌ | — | Same reason |
| `access_level` | *(absent)* | — | ❌ | — | Not applicable; semantic models inherit governance from the underlying model |
| `contract_enforced` | *(absent)* | — | ❌ | — | Not applicable |
| `raw_code` | *(absent)* | — | ❌ | — | Semantic models have no SQL body (YAML spec only) |
| `compiled_code` | *(absent)* | — | ❌ | — | Same reason |
| `columns` | *(absent)* | — | ❌ | — | Columns are surfaced as dimensions/measures/entities on the parent model; not redundantly on the semantic model |
| `created_at` | `number \| null` | Core | 🔧 | — | Epoch seconds (float); from `dbt.semantic_models.created_at`. Per ADR-5, this is the "Definition updated as of …" timestamp surfaced to `SemanticModelView` (replaces the prior recommendation to fall back to project-level `runGeneratedAt`). Empirically verified column present across 10 rows in the sample project. |
| `execution_info` | *(absent)* | — | ❌ | — | Class B: semantic models are not executed (no `dbt_rt.run_results` rows); `RESOURCES_WITH_EXECUTION_INFO` in dbt-ui excludes them — see Design note #2. Per ADR-5 the field is omitted from `DefinitionNodeBase` entirely — this row is documentation only. |
| `catalog` | *(absent)* | — | ❌ | — | Class B: no warehouse relation; no `dbt.catalog_tables` row |
| `freshness` | *(absent)* | — | ❌ | — | Source-only concept |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api — matches `ModelDetail` Risk #7 |
| `project_id` | *(absent)* | — | ❌ | — | Class B: Cloud-tier concept |
| `run_generated_at` | *(absent)* | — | ❌ | — | Cloud-tier run timestamp; dbt-ui header uses it for "Definition updated as of …" — replace with `dbt.project.last_indexed_at` at the project level if needed (Class B at the resource level) — see Risk #6 |
| `job_definition_id` | *(absent)* | — | ❌ | — | Class B: Cloud scheduler concept |
| `run_id` | *(absent)* | — | ❌ | — | Class B: Cloud run ID |
| `account_id` | *(absent)* | — | ❌ | — | Class B: Cloud tenant concept |
| `environment_id` | *(absent)* | — | ❌ | — | Class B: Cloud environment concept |
| `dbt_version` | *(absent)* | — | ❌ | — | Class B at the resource level; available on `GET /api/v1/project` if needed |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface SemanticModelDetail {
  unique_id: string;
  name: string;
  resource_type: "semantic_model";
  package_name: string | null;
  description: string | null;
  label: string | null;
  original_file_path: string | null;
  file_path: string | null;
  tags: string[];
  fqn: string[];
  meta: Record<string, unknown> | null;
  group_name: string | null;
  model: UpstreamModelRef | null;
  primary_entity: string | null;
  entities: SemanticEntity[];
  dimensions: SemanticDimension[];
  measures: SemanticMeasure[];
  depends_on: EdgeRef[];
  referenced_by: EdgeRef[];
  created_at: number | null;   // ADR-5: per-resource "Definition updated as of …" timestamp; epoch seconds
}

interface UpstreamModelRef {
  unique_id: string;
  name: string;
  access_level: string | null;
  alias: string | null;
}

interface SemanticEntity {
  name: string;
  type: string | null;
  description: string | null;
  label: string | null;
  expr: string | null;
  role: string | null;
}

interface SemanticDimension {
  name: string;
  type: string | null;
  description: string | null;
  label: string | null;
  expr: string | null;
  is_partition: boolean | null;
  time_granularity: string | null;
  type_params: Record<string, unknown> | null;
}

interface SemanticMeasure {
  name: string;
  agg: string | null;
  description: string | null;
  label: string | null;
  expr: string | null;
  create_metric: boolean | null;
  agg_time_dimension: string | null;
  agg_params: Record<string, unknown> | null;
  non_additive_dimension: Record<string, unknown> | null;
}

// EdgeRef is shared with ModelDetail / SourceDetail / SeedDetail
interface EdgeRef {
  unique_id: string;
  edge_type: string;
}
```

### Risk register

1. **No existing handler — greenfield endpoint with parallel parquet reads.** Unlike model/seed/snapshot (which extend the shared `dbt.nodes` SELECT), this handler reads from `dbt.semantic_models` (the row) plus three sibling tables (`dbt.semantic_entities`, `dbt.semantic_measures`, `dbt.semantic_dimensions`) filtered by the same `unique_id`. Plus a JOIN to `dbt.nodes` for the `model.access_level`/`model.alias` denormalization (the `dbt.semantic_models.model` column already stores a resolved `model.{pkg}.{name}` unique_id — see Risk #3 — so no `dbt.edges` traversal is needed for the model field). `depends_on`/`referenced_by` still come from `dbt.edges`. Implement with the existing fan-out-by-tokio-join pattern; profile against moderately-sized projects (50+ semantic models with 10–30 measures each).

2. **`tags` is not a top-level column on `dbt.semantic_models` — [RESOLVED].** Empirically confirmed against the sample project's `dbt.semantic_models.parquet`: schema is `unique_id, name, model, label, description, package_name, file_path, original_file_path, fqn, node_relation, primary_entity, defaults, depends_on_nodes, depends_on_macros, refs, group_name, config, created_at, ingested_at` — no `tags` column. The contract surfaces `tags` by extracting from the `config` JSON blob (`json_extract(config, '$.tags')`), defaulting to `[]` on absence. No dbt-index schema change required.

3. **`model.unique_id` resolution — [RESOLVED; prior framing was wrong].** Earlier draft claimed `dbt.semantic_models.model` stores a raw `ref('fct_orders')` string requiring resolution via `dbt.edges`. Empirically refuted: the column already contains a resolved `model.{pkg}.{name}` unique_id (observed: `"model.another_semantic_model"`). The handler reads `dbt.semantic_models.model` directly and JOINs `dbt.nodes` only to extract `access_level` and `alias` for the header denormalization — no `dbt.edges` traversal needed. One fewer query than originally scoped.

4. **`dimensions[*].type_params` shape — [RESOLVED].** GraphQL exposes `SemanticModelDimension.typeParams` as `JSONObject`. Empirically confirmed against `dbt.semantic_dimensions.parquet`: schema is `unique_id, name, dimension_type, description, label, expr, is_partition, time_granularity, validity_params, config, ingested_at` — no `type_params` column. The information GraphQL packs into `typeParams` is split across three first-class parquet columns (`dimension_type`, `time_granularity`, `validity_params`). Contract decision: expose those three directly and mark `type_params` ❌ Class B; the FE consumes the split fields. No dbt-index ingest change required.

5. **`measures[*].agg_params` and `non_additive_dimension` are stored as JSON-string columns.** `SemanticMeasureRow` (`crates/dbt-index/src/parquet.rs:1190`) declares both as `Option<String>` (JSON-encoded). The handler must `serde_json::from_str` each to emit them as JSON objects in the response, not as escaped strings. If parsing fails, emit `null` and log; do not bubble the error to the client.

6. **`run_generated_at` mapping — [RESOLVED].** Prior framing claimed no per-resource timestamp existed. Empirically refuted: `dbt.semantic_models.parquet` has both `created_at: double` and `ingested_at: timestamp[us, tz=UTC]`. Recommendation: surface `created_at` (epoch seconds) as the resource's "Definition updated as of …" timestamp. FE no longer needs to fall back to project-level metadata for this resource.

7. **`primary_entity` may duplicate one of the `entities[]` rows.** `dbt.semantic_models.primary_entity` stores the name of an entity (often also listed in `dbt.semantic_entities` with `entity_type = "primary"`). The dbt-ui does not currently consume `primary_entity` directly — it filters `entities[].type == "primary"`. **Decide before implementing** whether to (a) emit `primary_entity` as a denormalized convenience field alongside `entities`, (b) drop it and let the FE filter `entities`, or (c) emit only when no `entities[].type == "primary"` row exists (defensive fallback for legacy projects where the YAML only specifies `primary_entity` shorthand). Option (a) matches the parquet shape with no compute cost and is the proposed default in the example response.

8. **`depends_on` is single-entry by spec but the handler should still emit it as an array.** A semantic model declares exactly one underlying `model:`. Despite this, returning `depends_on` as an `EdgeRef[]` (length-1 array) keeps the contract uniform with `ModelDetail` and avoids special-case TypeScript narrowing. The contract is **not** to inline the upstream model into a singleton `depends_on` object — keep it array-shaped. The `model` field on the response is the convenience denormalization that adds `access_level` and `alias` for the header display.

9. **`semantic_relationships` is intentionally omitted from v0.** `dbt.semantic_relationships` (parquet schema at `crates/dbt-index/src/parquet.rs:1237`) captures `from_unique_id → to_unique_id` PK/FK relationships across semantic models. The dbt-ui detail page does not render these (no GraphQL field is fetched), so they are excluded for v0. If a future "Relationships" tab is added in `SemanticModelView`, expose as a new sub-resource `GET /api/v1/semantic_models/:id/relationships` rather than retrofitting an inline array — fan-out is unbounded across the whole semantic graph.


---

## Design notes — `GET /api/v1/sources`

LIST returns **one row per source table**. The unit of identity is the dbt source node — `dbt.nodes` row with `resource_type = 'source'`, `unique_id` of the form `source.<package>.<source_name>.<table_name>` — matching the existing DETAIL endpoint `GET /api/v1/sources/:id` exactly. The handler does **not** introduce a synthetic identifier or invent a new `resource_type` value.

dbt-ui's `SourceCollectionFilterView` rolls per-table rows up into per-(package, source_name) collections client-side via `getTableData` (group by `sourceName`, count tables, take `database`/`schema` from the first table, compute `maxFreshnessStatus`). The REST LIST endpoint stays at per-table granularity for three reasons:

1. **Consistency with detail.** Every other resource's LIST is 1:1 with its DETAIL (`/api/v1/seeds` row keys to `/api/v1/seeds/:id`). Pre-aggregating sources server-side would make sources the lone exception and produce a LIST row whose `unique_id` can't be passed to `:id` — a footgun for any consumer that drills down.
2. **No invented vocabulary.** The actual dbt model only has `resource_type = 'source'`; collections are a dbt-ui display rollup, not a dbt concept. Synthesizing `source_collection.*` ids and `resource_type: "source_collection"` would inject UI shape into the API contract.
3. **FE keeps the aggregation it already does.** `appliedAllSources.ts` already fetches per-table rows and the FilterView already aggregates them client-side. Mirroring that on the server provides no FE benefit and locks the contract into one specific presentation.

### Parquet sources

Verified against `~/codaz/sl-schema-evolution/sample_project/target/index/` and the existing `src/handlers/sources.rs` (the DETAIL handler is the authoritative reference for source parquet columns):

- `dbt.nodes.parquet` is the **only** parquet for source rows — there is no separate `dbt.sources.parquet`. Filter on `resource_type = 'source'`. Columns consumed by this LIST: `unique_id`, `name`, `resource_type`, `package_name`, `source_name`, `source_description`, `database_name`, `schema_name`, `identifier`, `loader`, `tags`, `meta` (CC-7 JSON-string), `original_file_path`. Sample project has zero source rows; the column set is empirically confirmed via the DETAIL handler's `SOURCE_DETAIL_NODE_SQL`.
- `dbt.source_freshness.parquet` carries `unique_id` (the per-table source node id), `status`, `snapshotted_at`, `max_loaded_at`, `max_loaded_at_time_ago`, `warn_after_count`, `warn_after_period`, `error_after_count`, `error_after_period`, `created_at`. Joined LEFT to `dbt.nodes` by `unique_id` so tables without a freshness row emit `freshness: null` per current doctrine (no `has_source_freshness` gate).

### Filters (from dbt-ui)

Three filter dropdowns in `SourceCollectionFilterView.tsx`, all client-side today (GraphQL response is unfiltered):

- **Status** (URL param `freshness`) — single value from `FreshnessStatus` enum plus a `no_data` sentinel for tables with no `dbt.source_freshness` row. REST: `freshness_status` query param accepting a comma-separated list (CSV-OR semantics per ADR-6).
- **Database** (URL param `database`) — single value. REST: `databases` query param accepting comma-separated list, exact match.
- **Schema** (URL param `schema`) — single value. REST: `schemas` query param accepting comma-separated list, exact match.

dbt-ui dropdowns are single-select today; ADR-6 standardizes CSV-OR across all LIST endpoints so the FE can pass either one or many. The dbt-ui rollup behaviour (collection's worst-of status, "no_data when every table lacks a row") becomes "this table's status, or `no_data` if this table has no row" at per-table granularity — the FE either filters the per-table rows directly with the same semantics, or applies its existing client-side rollup and uses the same dropdown unchanged.

### No sorts exposed

`SourceCollectionFilterView.tsx` exposes no `Sort` UI. Per ADR-6 default (`name:asc`), the LIST sorts by `n.name ASC` and `?sort` against an empty allowlist returns 400. Same pattern as the exposures contract — forward-compatible without quietly changing render order.

### Decisions worth flagging to the coordinator

1. **Per-table granularity is the canonical shape.** If the FE ever asks for a server-side rollup (e.g., for a separate `/api/v1/source_collections` endpoint or a query-param flag like `?group_by=source_name`), surface it as a NEW endpoint or a flag — do not retroactively change this endpoint's row shape.

2. **`freshness` is single-table here, not a worst-of rollup.** `freshness.status` is the status of THIS table's freshness check from `dbt.source_freshness` (or `null` if absent). The dbt-ui rollup (`maxFreshnessStatus`) stays client-side and operates over the per-table rows this endpoint returns.

3. **No `has_*` flag is introduced.** Per the parent doctrine in API-CONTRACTS.md § "Backend conventions", `has_run_results` / `has_catalog_stats` / `has_source_freshness` are vestigial. The `freshness` field is `null` when no per-table row exists in `dbt.source_freshness.parquet` (or the parquet is absent entirely).

---

## `GET /api/v1/sources`

Powers: `SourceCollectionFilterView` table rows in dbt-ui (per-table rows; FE rolls up to collections client-side).

dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/components/FilterPages/SourceCollectionFilterView.tsx`

GraphQL hooks:
- `packages/metadata/dbt-explorer/src/hooks/discovery/appliedAllSources.ts` (`AllSources` query — per-table source rows; client-aggregated for display)
- `packages/metadata/dbt-explorer/src/hooks/discovery/appliedSources.ts` (`AppliedSources` query — per-table detail; `sourceName`, `database`, `schema`, `freshness` rendered)
- `packages/metadata/dbt-explorer/src/hooks/discovery/appliedMaterializationFields.ts` (`AppliedMaterializedFields` query — distinct database/schema values for the filter dropdowns)
- `packages/metadata/dbt-explorer/src/hooks/discovery/sourceFreshness.ts` (`SourceFreshness` query — per-table freshness; the FE applies `maxFreshnessStatus` client-side)

### Query parameters

Per ADR-6: `first`, `after`, and `sort` are universal. Filters are resource-specific and use CSV-OR semantics.

| Param | Type | Default | Notes |
|---|---|---|---|
| `first` | `u32` | `100` (max `1000`) | Per-page row count. Server clamps. Hard max `5000`; clamped server-side per ADR-6. |
| `after` | `string` (opaque base64) | — | Cursor returned as `page_info.end_cursor` on the previous page. Omit for the first page. Tampering yields 400. |
| `sort` | `string` | `name:asc` | **Rejected with 400 if provided.** `SourceCollectionFilterView` exposes no sort UI. Default is internal; clients should not pass `?sort=...`. Forward-compatible: if sorts are added later, this becomes additive. |
| `freshness_status` | `string` (CSV) | — | OR'd list of `pass` · `warn` · `error` · `runtime_error` · `no_data`. `no_data` matches source-table rows with no `dbt.source_freshness` row. Unknown values → 400. |
| `databases` | `string` (CSV) | — | OR'd list of exact-match values against `dbt.nodes.database_name`. Case-sensitive. |
| `schemas` | `string` (CSV) | — | OR'd list of exact-match values against `dbt.nodes.schema_name`. Case-sensitive. |

Unknown query params are silently ignored (matches the existing `list_models` handler convention).

### Example response

Fields marked `// 🔧` are not yet returned — this endpoint does not exist yet, so every populated field is `🔧` at implementation time. Fields marked `// 🔍` need empirical confirmation against a project with non-empty freshness data before implementation.

```json
{
  "data": [
    {
      "unique_id": "source.jaffle_shop.raw_jaffle.orders",
      "name": "orders",
      "resource_type": "source",
      "package_name": "jaffle_shop",
      "source_name": "raw_jaffle",
      "source_description": "Operational raw tables from the application DB.",
      "database_name": "raw",
      "schema_name": "jaffle_shop",
      "identifier": "orders",
      "loader": "fivetran",
      "tags": [],
      "freshness": {
        "status": "warn",
        "snapshotted_at": "2026-05-19T10:00:00Z",
        "max_loaded_at": "2026-05-19T09:45:00Z"
      }
    },
    {
      "unique_id": "source.jaffle_shop.stripe.charges",
      "name": "charges",
      "resource_type": "source",
      "package_name": "jaffle_shop",
      "source_name": "stripe",
      "source_description": "Charges exported from Stripe via Fivetran.",
      "database_name": "raw",
      "schema_name": "stripe",
      "identifier": "charges",
      "loader": "fivetran",
      "tags": [],
      "freshness": null
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiPHN0YXJ0X3NvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "end_cursor": "eyJzIjoiPHNvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "has_next_page": false
  }
}
```

`freshness` is `null` when no `dbt.source_freshness` row exists for the source table's `unique_id` (i.e., `dbt source freshness` has not run for this table, or the parquet is absent). When present, `freshness.status` is the per-table status verbatim (with `"runtime error"` normalized to `runtime_error` per CC-1).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `data` | `SourceSummary[]` | Core | 🔧 | — | ADR-6 envelope key. Empty array when no source rows. |
| `data[*].unique_id` | `string` | Core | 🔧 | — | Real source-table unique_id of the form `source.<package>.<source_name>.<table_name>`. Pass directly to `GET /api/v1/sources/:id`. |
| `data[*].name` | `string` | Core | 🔧 | — | The source-table name (`dbt.nodes.name`, e.g. `"orders"`). NOT the source block name — that's `source_name` below. |
| `data[*].resource_type` | `"source"` | Core | 🔧 | — | Literal `"source"`. Same value as the DETAIL endpoint. |
| `data[*].package_name` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.package_name`. |
| `data[*].source_name` | `string \| null` | Core | 🔧 | — | The source block name in YAML (`dbt.nodes.source_name`, e.g. `"raw_jaffle"`). FE groups by this to render collections. |
| `data[*].source_description` | `string \| null` | Core | 🔧 | — | The source block's description (`dbt.nodes.source_description`). Constant across all tables in the same source block but surfaced per-row for FE convenience. |
| `data[*].database_name` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.database_name`. Renders the Database column. |
| `data[*].schema_name` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.schema_name`. Renders the Schema column. |
| `data[*].identifier` | `string \| null` | Core | 🔧 | — | Warehouse-side identifier from `dbt.nodes.identifier` (typically equals `name` unless overridden in YAML). |
| `data[*].loader` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.loader` (e.g., `"fivetran"`). |
| `data[*].tags` | `string[]` | Core | 🔧 | — | From `dbt.nodes.tags`. Empty array when absent. |
| `data[*].freshness` | `Freshness \| null` | Core-conditional | 🔧 | — | `null` when no `dbt.source_freshness` row exists for this `unique_id`. JSON null, no capability gate. |
| `data[*].freshness.status` | `string` | Core-conditional | 🔧 | — | One of `pass` · `warn` · `error` · `runtime_error`. Normalized from `dbt.source_freshness.status` (which stores `"runtime error"` space-separated) to snake_case per CC-1. |
| `data[*].freshness.snapshotted_at` | `string \| null` | Core-conditional | 🔍 | — | ISO 8601; from `dbt.source_freshness.snapshotted_at` cast to VARCHAR. |
| `data[*].freshness.max_loaded_at` | `string \| null` | Core-conditional | 🔍 | — | ISO 8601; from `dbt.source_freshness.max_loaded_at`. |
| `page_info` | `PageInfo` | Core | 🔧 | — | ADR-6 cursor envelope. See `PageInfo` definition in the shared types. Replaces the offset-era `total`/`offset`/`limit` triple. |
| `page_info.total_count` | `number` | Core | 🔧 | — | Total row count under the current filter set; ignores `first`/`after`. Separate `COUNT(*)` query per request. |
| `page_info.start_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor pointing to the FIRST row of the current page. `null` when `data` is empty. Symmetric with `end_cursor`. |
| `page_info.end_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor; pass back as `?after=...` to fetch the next page. `null` when `has_next_page` is `false`. |
| `page_info.has_next_page` | `boolean` | Core | 🔧 | — | `true` if at least one more row exists past this page. Server implements via `LIMIT first+1` and trim. |
| `data[*].max_loaded_at_time_ago` | *(absent)* | — | ❌ | — | The FE computes relative time client-side from `max_loaded_at`; the per-table `dbt.source_freshness.max_loaded_at_time_ago` is staleness-prone (snapshot of relative time at run). Defer to detail if ever needed. |
| `data[*].criteria` | *(absent)* | — | ❌ | — | Freshness thresholds (`warn_after` / `error_after`) are surfaced by the DETAIL endpoint. Not rendered in the FilterView; omit here. |
| `data[*].meta` | *(absent)* | — | ❌ | — | JSON-string per CC-7; not rendered in the FilterView. Defer to detail. |
| `data[*].columns` | *(absent)* | — | ❌ | — | Per-table columns; not rendered in the FilterView. Defer to detail. |
| `data[*].catalog` | *(absent)* | — | ❌ | — | Per-table catalog stats; not rendered in the FilterView. Defer to detail. |
| `data[*].health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api. The FilterView does not render health badges. |
| `data[*].file_path` | *(absent)* | — | ❌ | — | Path to the `.yml`; not rendered in the FilterView. Defer to detail. |
| `data[*].patch_path` | *(absent)* | — | ❌ | — | Sources are YAML-only; `patchPath` does not apply (see DETAIL contract for the same exclusion). |
| `data[*].execution_info` | *(absent)* | — | ❌ | — | Sources are not executed by `dbt build`. `freshness` is the conditional execution-adjacent surface for sources. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface SourceListResponse {
  data: SourceSummary[];
  page_info: PageInfo;
}

interface SourceSummary {
  // Real source-table unique_id: source.<package>.<source_name>.<table_name>.
  // Pass directly to GET /api/v1/sources/:id.
  unique_id: string;
  // The source-table name (e.g. "orders"). For the source block name, see source_name.
  name: string;
  resource_type: "source";
  package_name: string | null;
  source_name: string | null;
  source_description: string | null;
  database_name: string | null;
  schema_name: string | null;
  identifier: string | null;
  loader: string | null;
  tags: string[];
  // null when no dbt.source_freshness row exists for this unique_id.
  freshness: Freshness | null;
}

interface Freshness {
  // "pass" | "warn" | "error" | "runtime_error" (snake_case normalized).
  status: string;
  snapshotted_at: string | null;
  max_loaded_at: string | null;
}
// PageInfo: see ADR-6 § "Shared TypeScript types" for the canonical declaration.

```

### Risk register

1. **Per-table granularity is locked in.** This LIST returns one row per source table — same granularity as the DETAIL endpoint. If a future product requirement asks for server-side collection rollups (one row per `(package, source_name)`), expose it as a NEW endpoint (e.g., `/api/v1/source_collections`) or a documented `?group_by=source_name` flag. Do NOT retroactively change this endpoint's row shape; doing so breaks `unique_id` round-tripping with the DETAIL endpoint.

2. **No handler exists today.** No list handler in `src/handlers/sources.rs` — only `get_source`. The implementation PR must add `list_sources` + `list_source_facets`, register `GET /api/v1/sources` and `GET /api/v1/sources/facets` in `src/server.rs`, and add the response types to `web/src/api.ts`. Share parquet-column constants with the existing detail handler to avoid drift.

3. **Freshness status normalization is product-load-bearing.** `dbt.source_freshness.status` stores `"runtime error"` (space-separated) for the worst tier. The LIST handler must normalize to `runtime_error` (snake_case) per CC-1 BEFORE returning. The FE's filter dropdown and `maxFreshnessStatus` client-side rollup both assume the snake_case form. Reuse the same enum the DETAIL endpoint emits.

4. **`no_data` filter value has no parquet column.** The FE filter dropdown includes a `no_data` sentinel meaning "show source tables with no `dbt.source_freshness` row." Translate `?freshness_status=no_data` into a `WHERE NOT EXISTS (SELECT 1 FROM dbt.source_freshness sf WHERE sf.unique_id = n.unique_id)` predicate. When combined with other values (e.g., `?freshness_status=no_data,warn`), the predicate is `(sf.status = 'warn') OR <not-exists-clause>`.

5. **Empty project handling.** When `dbt.nodes.parquet` has zero rows with `resource_type = 'source'`, return `{ "data": [], "page_info": { "end_cursor": null, "has_next_page": false } }` — not a 404. Consistent with `list_models` behavior. Sample project (`~/codaz/sl-schema-evolution/sample_project/target/index/`) exercises this path.

6. **`?sort` rejection vs. forward compatibility.** Validate `?sort` against an empty allowlist and return 400 with "sort is not supported on this endpoint" (matches the exposures contract precedent). Do NOT silently ignore — a future addition would change rendered order without the client opting in, masking regressions.

7. **`tags` array vs. CC-7 JSON-string.** `dbt.nodes.tags` is a native list column in parquet, not a JSON-string. Read it directly with the Arrow `ListArray` extractor; do NOT route through `json_parse_or_null`. Confirmed against the schema in `dbt.nodes.parquet` (`'tags'` is `list<element: string>`).

8. **`appliedAllSources` over-fetches today.** The dbt-ui hook fetches columns/health_issues per source for lineage rendering that the FilterView doesn't render. The REST LIST endpoint deliberately omits those (see ❌ rows). If the FE later renders columns or health badges on the LIST view itself, those become additive surfaces — not retrofitted into the existing row shape.

---

## `GET /api/v1/sources/facets`

Powers: filter dropdowns in `SourceCollectionFilterView` (Status, Database,
Schema).

### Query parameters

None. The facets endpoint takes no query parameters per ADR-6.

### Example response

```json
{
  "freshness_status": [
    { "value": "pass", "count": null },
    { "value": "warn", "count": null },
    { "value": "error", "count": null },
    { "value": "runtime_error", "count": null },
    { "value": "no_data", "count": null }
  ],
  "databases": [
    { "value": "raw", "count": null }
  ],
  "schemas": [
    { "value": "jaffle_shop", "count": null },
    { "value": "stripe", "count": null }
  ]
}
```

`freshness_status` values are static (the dbt-ui dropdown is built from the
`FreshnessStatus` enum + a `no_data` sentinel; values do not depend on the
project's data). `databases` and `schemas` are project-specific — distinct
values from `dbt.nodes` filtered to `resource_type = 'source'`.

When `dbt.nodes.parquet` has zero source rows, `databases` and `schemas`
are empty arrays; `freshness_status` is still populated with its static
list (matches the dbt-ui dropdown rendering — the Status dropdown is
visible even on empty projects).

The `count` field is reserved for a future enhancement that returns the
number of matching collections per facet value. Today it is always `null` —
matching the `list_model_facets` convention.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `freshness_status` | `FacetValue[]` | Core | 🔧 | — | Static enum values: `pass`, `warn`, `error`, `runtime_error`, `no_data`. Order matches dbt-ui dropdown order: `no_data` first as the "missing data" sentinel, then `freshnessStatuses` enum in dbt-ui order. Server-derived from a constant, not from parquet. |
| `freshness_status[*].value` | `string` | Core | 🔧 | — | snake_case status; matches `data[*].freshness.status` shape on the LIST endpoint. |
| `freshness_status[*].count` | `number \| null` | Core | 🔧 | — | Always `null` today; reserved for per-facet count enrichment. Matches `list_model_facets.owners[*].count` convention. |
| `databases` | `FacetValue[]` | Core | 🔧 | — | Distinct non-null `dbt.nodes.database_name` values for `resource_type = 'source'`, sorted ascending. Mirrors `useAppliedMaterializedFields` GraphQL hook with `types: [Source]`. Empty array on projects with no source nodes. |
| `databases[*].value` | `string` | Core | 🔧 | — | Raw database name; case-preserved. |
| `databases[*].count` | `number \| null` | Core | 🔧 | — | Always `null` today; reserved. |
| `schemas` | `FacetValue[]` | Core | 🔧 | — | Distinct non-null `dbt.nodes.schema_name` values for `resource_type = 'source'`, sorted ascending. Mirrors `useAppliedMaterializedFields` GraphQL hook with `types: [Source]`. |
| `schemas[*].value` | `string` | Core | 🔧 | — | Raw schema name; case-preserved. |
| `schemas[*].count` | `number \| null` | Core | 🔧 | — | Always `null` today; reserved. |
| `owners` | *(absent)* | — | ❌ | — | Sources have no owner concept in dbt — owners live on models (via `dbt.groups`) and exposures (via `owner_name`). The FilterView has no Owner dropdown for sources. |
| `tags` | *(absent)* | — | ❌ | — | Not exposed as a filter in `SourceCollectionFilterView` today; per-table tag rollup is undefined for collections. |
| `loaders` | *(absent)* | — | ❌ | — | `dbt.nodes.loader` exists but no dropdown in dbt-ui. Add additively if a Loader filter is ever requested. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface SourceFacetsResponse {
  freshness_status: FacetValue[];
  databases: FacetValue[];
  schemas: FacetValue[];
}

// FacetValue is shared with ModelFacetsResponse / ExposureFacetsResponse
interface FacetValue {
  value: string;
  count: number | null;
}
```

### Risk register

1. **`freshness_status` values are server constants, not parquet-derived.**
   The dropdown is built from `freshnessStatuses` (the dbt-ui exported
   constant) plus a `no_data` sentinel. The list of values must stay in
   sync with the dbt-dag `freshnessStatuses` export — if dbt-dag ever adds
   a new freshness state (`stale`, etc.), this facet endpoint must be
   updated. There is no parquet-level enforcement.

2. **`databases` / `schemas` source is `dbt.nodes` filtered to sources, not
   a separate parquet.** Matches the dbt-ui's
   `useAppliedMaterializedFields` GraphQL hook with `types: [Source]`. The
   handler SQL is `SELECT DISTINCT database_name AS value FROM dbt.nodes
   WHERE resource_type = 'source' AND database_name IS NOT NULL ORDER BY
   database_name` (and same for schema). Reuse the models facets pattern;
   do not invent a different filter mechanism.

3. **Database / schema collisions across resource types.** A database name
   like `"raw"` may appear in both the models facets endpoint (sourced
   from `dbt.nodes` filtered to `resource_type IN ('model', 'seed', ...)`
   if/when that handler is added) and the sources facets endpoint. The
   two endpoints return scoped lists from the same parquet table filtered
   differently — this is correct and intentional. FE engineers should not
   assume a global database catalog.

4. **Empty project handling.** When `dbt.nodes.parquet` has zero source
   rows, the response is `{ "freshness_status": [...static...],
   "databases": [], "schemas": [] }` — not a 404 and not an error. The
   static `freshness_status` list is still populated so the dropdown
   renders consistently.

5. **`count` enrichment deferred consistently.** Today the only producer
   of facet `count` data is "none." If `list_model_facets` ever starts
   populating `count`, this endpoint must follow the same pattern at the
   same time. Coordinate before shipping a partial implementation.

6. **No `has_*` capability gate is introduced.** Per the parent doctrine
   in API-CONTRACTS.md § "Backend conventions", `has_run_results` /
   `has_catalog_stats` / `has_source_freshness` are vestigial. The
   facets endpoint has no execution-, catalog-, or freshness-conditional
   surface. The `freshness_status` values are static; the `databases` and
   `schemas` values are derived from `dbt.nodes` which is always present
   for any indexed project. Do not invent a new `has_*` flag.

7. **Parquet schema empirically anchored to the detail handler.** The
   column names this contract relies on (`package_name`, `source_name`,
   `database_name`, `schema_name`, `resource_type`) are exactly the
   columns selected by `SOURCE_DETAIL_NODE_SQL` in
   `src/handlers/sources.rs` — the authoritative reference per the plan.
   No new column dependencies are introduced. If a future index format
   renames any of these columns, both the detail handler and this list
   handler must update together.

---

## Design notes

`SeedFilterView` is the simplest of the resource list views: three columns
(`Name`, `Row count`, `Last executed`), no filter dropdowns, no client-side
sort controls. The GraphQL hook (`appliedSeeds.ts`) over-fetches considerably
— `tags`, `description`, `database`, `schema`, `meta`, `alias`, full
`executionInfo`, full `catalog` (including `columns[]`, `bytesStat`,
`stats[]`) — but the rendered `TableData` projection in `SeedFilterView.tsx`
keeps only `name`, `uniqueId`, `lastExecuted` (from
`executionInfo.executeCompletedAt`), and `rowCount` (from
`catalog.rowCountStat`).

The LIST contract returns just what the rendered view needs plus the
ADR-1 `NodeBase` fields (`unique_id`, `name`, `resource_type`,
`package_name`, `description`, `original_file_path`) so the response is
useful to other consumers without forcing a second fetch. Per ADR-6, the
top-level envelope key is `data` — *not* `seeds`. The existing models
handler still uses a resource-named key (`models`); that's pre-doctrine
and is being renamed in a companion PR. Author this contract with `data`.

The FACETS endpoint is intentionally minimal. Seeds expose no filter
dropdowns in dbt-ui, so the response body is `{}`. The endpoint exists for
API uniformity across resource types (so clients can hit
`GET /api/v1/<resource>/facets` for every resource without special-casing
seeds).

Parquet verification against the sample project
(`~/codaz/sl-schema-evolution/sample_project/target/index/`):

- `dbt.nodes.parquet`: all `NodeBase` columns present; `resource_type='seed'`
  yields 6 rows in the sample.
- `dbt_rt.run_results.parquet`: 6 rows for `seed.%` `unique_id` values
  (`dbt seed` writes one row per seed per invocation, like models). The
  `created_at` column is the run completion timestamp; the models list
  handler already extracts this same field via the `last_run` CTE.
- `dbt.catalog_stats.parquet`: zero rows for `seed.%` in the sample
  project. `dbt docs generate` against DuckDB does not emit per-stat rows
  for seeds in this index. The contract still exposes `row_count` as a
  `🔍` field — the column exists in the schema and other adapters
  (Snowflake, BigQuery) are known to populate it; the implementation must
  tolerate the empty result set and return `null`.

## `GET /api/v1/seeds`

Powers: `SeedFilterView` / `ResourceFilterPage` in dbt-ui.

dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/components/FilterPages/SeedFilterView.tsx`
GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/discovery/appliedSeeds.ts`

### Query parameters

Per ADR-6: `first`, `after`, and `sort` are universal. Seeds expose no
filter parameters because the dbt-ui view has no filter dropdowns. The
default sort is `name:asc` — seeds have no naturally-meaningful sort
order beyond the dbt-ui column set (Name, Row count, Last executed), so
the ADR-6 default is fine and matches the user-visible order on first
load.

| Param | Type | Default | Notes |
|---|---|---|---|
| `first` | `u32` | `100` (max `1000`) | Per-page row count. Server clamps. Hard max `5000`; values above are clamped per ADR-6. |
| `after` | `string` (opaque base64) | — | Cursor returned as `page_info.end_cursor` on the previous page. Omit for the first page. Tampering yields 400. |
| `sort` | `string` | `name:asc` | Format `<column>:<asc\|desc>`. Allowlisted columns: `name`, `row_count`, `executed_at`. Unknown column → 400. |

### Example response

Fields marked `// 🔧` are not yet returned — this endpoint does not exist yet,
so every populated field is `🔧` at implementation time. Fields marked `// 🔍`
are parquet-unverified for the specific stat-key mapping and require
confirmation against a real index (see Risk #2).

```json
{
  "data": [
    {
      "unique_id": "seed.jaffle_shop.raw_customers",
      "name": "raw_customers",
      "resource_type": "seed",
      "package_name": "jaffle_shop",
      "description": "Raw customer seed file loaded from CSV.",
      "original_file_path": "seeds/raw_customers.csv",
      "row_count": 935,                              // 🔍 from dbt.catalog_stats; null when has_catalog_stats false or stat absent
      "executed_at": "2026-05-15T10:28:03Z"          // 🔧 from dbt_rt.run_results.created_at; null when has_run_results false
    },
    {
      "unique_id": "seed.jaffle_shop.raw_orders",
      "name": "raw_orders",
      "resource_type": "seed",
      "package_name": "jaffle_shop",
      "description": null,
      "original_file_path": "seeds/raw_orders.csv",
      "row_count": null,
      "executed_at": null
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiPHN0YXJ0X3NvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "end_cursor": "eyJzIjoiPHNvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "has_next_page": false
  }
}
```

`row_count` is `null` when `has_catalog_stats` is false (i.e., `dbt docs
generate` has not run) or when the adapter did not emit a row-count stat
for this seed. `executed_at` is `null` when `has_run_results` is false
(i.e., `dbt seed` / `dbt build` has not run for this seed).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `data` | `SeedSummary[]` | Core | 🔧 | — | ADR-6 envelope key. Empty array when no seeds. |
| `data[*].unique_id` | `string` | Core | 🔧 | — | e.g., `"seed.pkg.name"`. From `dbt.nodes.unique_id`. |
| `data[*].name` | `string` | Core | 🔧 | — | From `dbt.nodes.name`. |
| `data[*].resource_type` | `"seed"` | Core | 🔧 | — | Always `"seed"` for this endpoint. |
| `data[*].package_name` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.package_name`. |
| `data[*].description` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.description`. |
| `data[*].original_file_path` | `string \| null` | Core | 🔧 | — | Path to the CSV file relative to project root; from `dbt.nodes.original_file_path`. |
| `data[*].row_count` | `number \| null` | Core-conditional | 🔍 | `has_catalog_stats` | Approximate row count from `dbt.catalog_stats` keyed by `stat_id`; canonical stat key unverified — see Risk #2. `null` when stat absent or `dbt docs generate` has not run. |
| `data[*].executed_at` | `string \| null` | Core-conditional | 🔧 | `has_run_results` | ISO 8601 completion timestamp; from `dbt_rt.run_results.created_at` via a `last_run` CTE (same pattern as the models list handler). `null` when `dbt seed` / `dbt build` has not run for this seed. |
| `page_info` | `PageInfo` | Core | 🔧 | — | ADR-6 cursor envelope. See `PageInfo` definition in the shared types. Replaces the offset-era `total`/`offset`/`limit` triple. |
| `page_info.total_count` | `number` | Core | 🔧 | — | Total row count under the current filter set; ignores `first`/`after`. Separate `COUNT(*)` query per request. |
| `page_info.start_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor pointing to the FIRST row of the current page. `null` when `data` is empty. Symmetric with `end_cursor`. |
| `page_info.end_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor; pass back as `?after=...` to fetch the next page. `null` when `has_next_page` is `false`. |
| `page_info.has_next_page` | `boolean` | Core | 🔧 | — | `true` if at least one more row exists past this page. Server implements via `LIMIT first+1` and trim. |
| `data[*].tags` | *(absent)* | — | ❌ | — | Over-fetched by GraphQL hook but not rendered in `SeedFilterView`; defer to `GET /api/v1/seeds/:id`. |
| `data[*].database_name` | *(absent)* | — | ❌ | — | Over-fetched; not rendered. Defer to detail. |
| `data[*].schema_name` | *(absent)* | — | ❌ | — | Over-fetched; not rendered. Defer to detail. |
| `data[*].identifier` | *(absent)* | — | ❌ | — | Maps to `dbt.nodes.alias`; over-fetched by hook, not rendered. Defer to detail. |
| `data[*].meta` | *(absent)* | — | ❌ | — | Over-fetched; not rendered. Defer to detail. |
| `data[*].columns` | *(absent)* | — | ❌ | — | Over-fetched (full `catalog.columns[]`); not rendered. Defer to detail. |
| `data[*].catalog.bytes_stat` | *(absent)* | — | ❌ | — | Over-fetched; not rendered in list view. Defer to detail. |
| `data[*].catalog.stats[]` | *(absent)* | — | ❌ | — | Over-fetched; not rendered in list view. Defer to detail. |
| `data[*].execution_info` | *(absent)* | — | ❌ | — | Full execution object over-fetched; list view only renders `executeCompletedAt`. The summary projects that one field as a top-level `executed_at` (same shape as `ModelSummary.executed_at`). |
| `data[*].project_id` | *(absent)* | — | ❌ | — | Class B: Cloud concept; not in parquet. |
| `data[*].last_run_id` | *(absent)* | — | ❌ | — | Class B: Cloud run ID; not in local parquet. |
| `data[*].health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api. |

### Type definition

Type definition (for codegen reference). The field reference table above is the authoritative contract.

```typescript
interface SeedListResponse {
  data: SeedSummary[];
  page_info: PageInfo;
}

interface SeedSummary {
  unique_id: string;
  name: string;
  resource_type: "seed";
  package_name: string | null;
  description: string | null;
  original_file_path: string | null;
  // Core-conditional: null when has_catalog_stats false or stat absent.
  row_count: number | null;
  // Core-conditional: null when has_run_results false. ISO 8601.
  executed_at: string | null;
}
// PageInfo: see ADR-6 § "Shared TypeScript types" for the canonical declaration.

```

### Risk register

1. **`executed_at` resilience to missing `dbt_rt.run_results` view.** The
   models list handler builds two SQL strings — one with the `last_run` CTE
   and one without — and falls back when `query_scalar` returns `None`
   (signal that the parquet view is absent). The seeds list handler must
   follow the same pattern, casting the missing column to
   `NULL::VARCHAR AS executed_at` so the Arrow column type is stable. Reuse
   the models pattern; do not invent a second mechanism.

2. **`row_count` stat key is unverified.** `dbt.catalog_stats` stores stats
   keyed by `stat_id`, and the canonical key for row count is
   adapter-specific. The sample project (DuckDB) emits zero
   `catalog_stats` rows for seeds, which is not enough to validate the key
   name. Before implementing, confirm against a Snowflake or BigQuery
   index. If the stat is absent, return `null` — never bubble an error.

3. **No filter dropdowns means no `WHERE` accelerators today.** The handler
   needs only `WHERE n.resource_type = 'seed'` plus optional sort/limit.
   If a future iteration adds filter dropdowns (e.g., by package), follow
   the models handler's `parse_*` + comma-OR pattern.

4. **`sort` allowlist is short by design.** Three columns: `name`,
   `row_count`, `executed_at`. `row_count` and `executed_at` sort use
   `NULLS LAST` to match the models handler's convention so projects
   without catalog/run-results don't push `null` rows to the top.

5. **`data` envelope key vs. the existing models handler's `models` key.**
   The models handler still returns `{ "models": [...] }`. That predates
   ADR-6 and is being renamed in a companion PR. The seeds handler must
   land with `data` from day one; do not copy the `models` precedent.

6. **`tags`, `meta`, `database_name`, `schema_name`, `identifier` are over-
   fetched by the GraphQL hook.** `SeedFilterView.tsx` projects only four
   fields out of the much larger `SeedAppliedListNode` shape. The REST
   contract intentionally drops the unused fields rather than mirroring
   GraphQL one-for-one — preserves CC-2 (preserve shape *of what we
   actually return*) without paying for fields no client renders. They
   remain available on `GET /api/v1/seeds/:id`.

## `GET /api/v1/seeds/facets`

Powers: filter dropdowns for the list view above.

### Query parameters

No query parameters.

### Example response

The seeds list has no filter dropdowns, so the response body is an empty
object. The endpoint exists for API uniformity across resources.

```json
{}
```

### Field reference

No facet keys; the endpoint exists for API uniformity.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| *(none)* | — | — | — | — | Seeds expose no filter dropdowns in dbt-ui (`SeedFilterView.tsx`). Response body is `{}`. |

### Type definition

Type definition (for codegen reference). The field reference table above is the authoritative contract.

```typescript
// Intentionally empty — present for API uniformity with other resources.
// If filters are ever added to SeedFilterView, append optional keys here
// following the FacetValue shape used by ModelFacetsResponse.
type SeedFacetsResponse = Record<string, never>;
```

### Risk register

No facet-specific risks; see LIST endpoint Risk register above.

---

## Design notes

`SnapshotFilterView.tsx` is one of the simplest resource list views in
dbt-explorer. It renders four columns — `Name`, `Row count`, `Size`, and
(Snowflake-only) `Last modified` — backed by a single GraphQL hook,
`appliedSnapshots.ts`. There are **no filter dropdowns** and **no
user-facing sort controls**. The view paginates through
`environment.applied.snapshots` (cursor-based GraphQL); the REST endpoint
mirrors that read-mostly shape but uses cursor pagination (`first`/`after` + `page_info`) per ADR-6.

Snapshots are dbt-build-runnable resources: `dbt build` and `dbt snapshot`
both produce `dbt_rt.run_results` rows for `snapshot.*` unique_ids. The
existing snapshot **detail** contract therefore inlines `execution_info`
(status/completed_at/execution_time) and `catalog`
(type/owner/primary_key/row_count_stat/bytes_stat/stats[]). The LIST
contract is intentionally narrower: only the fields rendered by
`SnapshotFilterView.tsx` plus the ADR-1 `NodeBase` fields and the snapshot
config columns that the detail row already exposes cheaply
(`materialized`, `strategy`, `updated_at`).

Why `execution_info` and `catalog` are kept nested (not flattened) on the
LIST row:

- The detail contract already establishes the nested shape. Flattening on
  LIST would create two parallel shapes for the same underlying data
  (cf. the seeds list contract that projected `executed_at` and
  `row_count` as top-level fields — that pre-doctrine pattern is fine
  for seeds because seeds expose no `catalog.stats[]` or `primary_key`,
  but snapshots do, and nesting preserves room for adding `bytes_stat`
  to the LIST projection without renaming the existing top-level field).
- CC-2 (preserve nested objects from Discovery API shape; do not flatten)
  applies. The dbt-ui hook over-fetches the full `catalog { rowCountStat,
  bytesStat, stats { ... } }` block; the REST projection trims the
  `stats[]` array down to the one stat actually rendered
  (`last_modified`, Snowflake-only) but keeps the parent nesting.
- Per ADR-6 the nested objects emit JSON `null` when their backing
  parquet row is absent — no `has_*` capability flag is invented for the
  LIST endpoint.

Top-level envelope key is `data` per ADR-6 — *not* `snapshots`. The
existing models handler still uses `models`; that's pre-doctrine and is
being renamed in a companion PR. Author this contract with `data` from
day one.

The FACETS endpoint is intentionally minimal. Snapshots expose no filter
dropdowns in dbt-ui, so the response body is `{}`. The endpoint exists
for API uniformity across resource types (so clients can hit
`GET /api/v1/<resource>/facets` for every resource without
special-casing snapshots).

Parquet verification against the sample project
(`~/codaz/sl-schema-evolution/sample_project/target/index/`):

- `dbt.nodes.parquet`: all `NodeBase` columns present for snapshots; the
  sample project (jaffle-shop-style SL) **may contain zero rows with
  `resource_type='snapshot'`**. The handler must tolerate an empty
  result and return `{ "data": [], "page_info": { "end_cursor": null, "has_next_page": false } }`. Snapshot config
  columns (`materialized` is always `"snapshot"`; `strategy` and
  `updated_at` come from the snapshot `config` JSON or top-level
  columns) are 🔍 — confirm column names against a real snapshot index
  before implementing.
- `dbt_rt.run_results.parquet`: confirmed schema (`unique_id`, `status`,
  `execution_time`, `created_at`). The models list handler's `last_run`
  CTE pattern transfers directly — same `MAX(created_at) GROUP BY
  unique_id` shape, then `LEFT JOIN` onto `dbt.nodes` filtered by
  `resource_type='snapshot'`.
- `dbt.catalog_tables.parquet`: schema confirmed via the sources detail
  handler (`table_type` → `type`, `table_owner` → `owner`,
  `table_comment` → `comment`). For snapshots in the DuckDB sample
  project, this view typically has zero matching rows because
  `dbt docs generate` on DuckDB does not emit per-snapshot rows. The
  handler must tolerate the empty result and emit `catalog: null`.
- `dbt.catalog_stats.parquet`: schema confirmed via the sources detail
  handler (`stat_id`, `stat_label`, `stat_value`, `description`,
  `include_in_stats`). The `last_modified` stat is **Snowflake-only**;
  other adapters either omit it or use a different `stat_id`. On the
  DuckDB sample project this view has zero rows for any `snapshot.*`
  unique_id, which means every per-stat field
  (`row_count_stat`, `bytes_stat`, `last_modified_stat`) is `null` in a
  sample-project round-trip. That is the expected behavior, not a bug.

No `has_*` capability flag is introduced. Per ADR-6, `Capabilities`
carries only distribution-gated flags; the absence of `catalog`,
`execution_info`, or per-stat values is purely a project-state concern,
communicated by emitting JSON `null` on the field itself.

## `GET /api/v1/snapshots`

Powers: `SnapshotFilterView` / `ResourceFilterPage` in dbt-ui.

dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/components/FilterPages/SnapshotFilterView.tsx`
GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/discovery/appliedSnapshots.ts`

### Query parameters

Per ADR-6: `first`, `after`, and `sort` are universal. Snapshots expose
no filter parameters because the dbt-ui view has no filter dropdowns.
Default sort is `name:asc` — snapshots have no naturally-meaningful sort
order beyond the rendered columns (Name, Row count, Size, Last
modified), so the ADR-6 default matches the user-visible order on first
load.

| Param | Type | Default | Notes |
|---|---|---|---|
| `first` | `u32` | `100` (max `1000`) | Per-page row count. Server clamps. Hard max `5000`; values above are clamped per ADR-6. |
| `after` | `string` (opaque base64) | — | Cursor returned as `page_info.end_cursor` on the previous page. Omit for the first page. Tampering yields 400. |
| `sort` | `string` | `name:asc` | Format `<column>:<asc\|desc>`. Allowlisted columns: `name`, `package_name`, `updated_at`. Sorting by nested `execution_info.*` or `catalog.*` is **not** allowed in v0 to avoid `LEFT JOIN` ordering complexity; if a sort by `completed_at` or `row_count` is ever required, add it explicitly to the allowlist with the same `NULLS LAST` semantics as the models handler. Unknown column → 400. |

### Example response

Fields marked `// 🔧` are not yet returned — this endpoint does not exist
yet, so every populated field is `🔧` at implementation time. Fields
marked `// 🔍` are parquet-unverified for the specific column name or
stat-key mapping and require confirmation against a real index (see
Risk register).

```json
{
  "data": [
    {
      "unique_id": "snapshot.jaffle_shop.orders_snapshot",
      "name": "orders_snapshot",
      "resource_type": "snapshot",
      "package_name": "jaffle_shop",
      "materialized": "snapshot",
      "strategy": "timestamp",
      "updated_at": "updated_at",
      "execution_info": {
        "status": "success",
        "completed_at": "2026-05-15T10:32:11Z",
        "error": null
      },
      "catalog": {
        "row_count_stat": 42000,
        "bytes_stat": 3145728,
        "last_modified_stat": "2026-05-15 10:30:00"
      }
    },
    {
      "unique_id": "snapshot.jaffle_shop.customers_snapshot",
      "name": "customers_snapshot",
      "resource_type": "snapshot",
      "package_name": "jaffle_shop",
      "materialized": "snapshot",
      "strategy": "check",
      "updated_at": null,
      "execution_info": null,
      "catalog": null
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiPHN0YXJ0X3NvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "end_cursor": "eyJzIjoiPHNvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "has_next_page": false
  }
}
```

`execution_info` is `null` when `dbt_rt.run_results` has no row for this
snapshot (i.e., `dbt build` / `dbt snapshot` has not run for it).
`catalog` is `null` when `dbt.catalog_tables` has no row for this
snapshot (i.e., `dbt docs generate` has not run for it).

Within `catalog`, each per-stat field (`row_count_stat`, `bytes_stat`,
`last_modified_stat`) is independently `null` when the corresponding
adapter-specific `stat_id` row is absent from `dbt.catalog_stats` —
`last_modified_stat` in particular is **Snowflake-only** today.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `data` | `SnapshotSummary[]` | Core | 🔧 | — | ADR-6 envelope key. Empty array when no snapshots. |
| `data[*].unique_id` | `string` | Core | 🔧 | — | e.g., `"snapshot.pkg.name"`. From `dbt.nodes.unique_id`. |
| `data[*].name` | `string` | Core | 🔧 | — | From `dbt.nodes.name`. |
| `data[*].resource_type` | `"snapshot"` | Core | 🔧 | — | Always `"snapshot"` for this endpoint. |
| `data[*].package_name` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.package_name`. |
| `data[*].materialized` | `"snapshot"` | Core | 🔧 | — | Always `"snapshot"` for this resource type. Included for symmetry with the detail contract and to avoid forcing clients to special-case the resource on display. From `dbt.nodes.materialized`. |
| `data[*].strategy` | `string \| null` | Core | 🔍 | — | Snapshot strategy: `"timestamp"` or `"check"`. Sourced from the snapshot's `config` JSON (`config->>'strategy'`) in `dbt.nodes.config`, parsed handler-side via `json_parse_or_null` per CC-7. Confirm `config` is present as a JSON-string column for snapshots before implementing — see Risk #1. |
| `data[*].updated_at` | `string \| null` | Core | 🔍 | — | Name of the timestamp column used by the `timestamp` strategy (e.g., `"updated_at"`). `null` for `check`-strategy snapshots. Sourced from `config->>'updated_at'` (same JSON-parse path as `strategy`). Despite the name, this is **not** a timestamp value — it's the column name dbt watches. Surfaced because the GraphQL hook over-fetches it via `applied.snapshots.node` for downstream tooling; included here to spare clients a second fetch when populating snapshot config previews. |
| `data[*].execution_info` | `ExecutionInfo \| null` | Core-conditional | 🔧 | — | `null` when `dbt_rt.run_results` has no row for this snapshot. Same shape and population strategy as `ModelDetail.execution_info`; the `last_run` CTE pattern from the models list handler transfers directly. |
| `data[*].execution_info.status` | `string \| null` | Core-conditional | 🔧 | — | `"success"` · `"error"` · `"skipped"` (raw `dbt_rt.run_results.status` value; no normalization). |
| `data[*].execution_info.completed_at` | `string \| null` | Core-conditional | 🔧 | — | Derived from `dbt_rt.run_results.created_at` via `CAST(... AS VARCHAR)`. Same `"2026-05-14 17:41:56.652026-07"`-style space-separated local-timezone format as `ModelDetail.execution_info.completed_at` (see ADR-2 Risk #1). |
| `data[*].execution_info.error` | `string \| null` | Core-conditional | 🔧 | — | `null` on success; populated when `status = "error"`. Maps to ADR-4's `error` (bare name, not `last_run_error`). Confirm `dbt_rt.run_results` exposes an error/message column at implementation time — if absent, surface `null` and document. |
| `data[*].catalog` | `SnapshotListCatalogInfo \| null` | Core-conditional | 🔧 | — | `null` when `dbt.catalog_tables` has no row for this snapshot. Narrower than `SnapshotDetail.catalog`: omits `type`, `owner`, `primary_key`, and the full `stats[]` array (those remain on the detail endpoint). |
| `data[*].catalog.row_count_stat` | `number \| null` | Core-conditional | 🔍 | — | Approximate row count from `dbt.catalog_stats` keyed by adapter-specific `stat_id`. Always `null` in the DuckDB sample project (catalog_stats has zero snapshot rows there). Canonical stat key per adapter is unverified — see Risk #2. |
| `data[*].catalog.bytes_stat` | `number \| null` | Core-conditional | 🔍 | — | Size in bytes from `dbt.catalog_stats`. Same caveats as `row_count_stat`. Used by `SnapshotFilterView.tsx` to render the `Size` column via `prettyBytes`. |
| `data[*].catalog.last_modified_stat` | `string \| null` | Core-conditional | 🔍 | — | Last-modified timestamp from `dbt.catalog_stats` where `stat_id = 'last_modified'`. **Snowflake-only** today (`SnapshotFilterView.tsx` only renders the `Last modified` column when `adapterType === 'snowflake'`); always `null` on other adapters and on the DuckDB sample project. Returned as the raw stat value (a string per `dbt.catalog_stats.stat_value`), not parsed into ISO 8601. |
| `page_info` | `PageInfo` | Core | 🔧 | — | ADR-6 cursor envelope. See `PageInfo` definition in the shared types. Replaces the offset-era `total`/`offset`/`limit` triple. |
| `page_info.total_count` | `number` | Core | 🔧 | — | Total row count under the current filter set; ignores `first`/`after`. Separate `COUNT(*)` query per request. |
| `page_info.start_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor pointing to the FIRST row of the current page. `null` when `data` is empty. Symmetric with `end_cursor`. |
| `page_info.end_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor; pass back as `?after=...` to fetch the next page. `null` when `has_next_page` is `false`. |
| `page_info.has_next_page` | `boolean` | Core | 🔧 | — | `true` if at least one more row exists past this page. Server implements via `LIMIT first+1` and trim. |
| `data[*].tags` | *(absent)* | — | ❌ | — | Over-fetched by the GraphQL hook (`tags` on `applied.snapshots.node`) but not rendered in `SnapshotFilterView`. Defer to `GET /api/v1/snapshots/:id`. |
| `data[*].description` | *(absent)* | — | ❌ | — | Same as `tags` — over-fetched but not rendered in the list view. |
| `data[*].database_name` | *(absent)* | — | ❌ | — | Over-fetched (`database`); not rendered in list view. Defer to detail. |
| `data[*].schema_name` | *(absent)* | — | ❌ | — | Over-fetched (`schema`); not rendered in list view. Defer to detail. |
| `data[*].identifier` | *(absent)* | — | ❌ | — | Over-fetched (`alias`); not rendered in list view. Defer to detail. |
| `data[*].raw_code` | *(absent)* | — | ❌ | — | Over-fetched; not rendered in list view. Defer to detail. |
| `data[*].compiled_code` | *(absent)* | — | ❌ | — | Over-fetched; not rendered in list view. Defer to detail. |
| `data[*].meta` | *(absent)* | — | ❌ | — | Over-fetched; not rendered in list view. Defer to detail. |
| `data[*].catalog.type` | *(absent)* | — | ❌ | — | Over-fetched in the hook's `catalog { type }` but not rendered in the list view. Defer to detail. |
| `data[*].catalog.owner` | *(absent)* | — | ❌ | — | Same as `catalog.type`. Defer to detail. |
| `data[*].catalog.columns` | *(absent)* | — | ❌ | — | Over-fetched (full per-column type/tag breakdown); not rendered in list view. Defer to detail. |
| `data[*].catalog.primary_key` | *(absent)* | — | ❌ | — | Detail-only — see `SnapshotDetail.catalog.primary_key`. |
| `data[*].catalog.stats` | *(absent)* | — | ❌ | — | Detail-only — the list view projects only the `last_modified` stat (and only on Snowflake). The full `stats[]` array remains on `GET /api/v1/snapshots/:id`. |
| `data[*].check_cols` | *(absent)* | — | ❌ | — | `config->>'check_cols'` is detail-only — the list view does not render the check-strategy column list. Defer to detail. |
| `data[*].last_run_id` | *(absent)* | — | ❌ | — | Class B: Cloud-specific run ID; no parquet path. The GraphQL hook returns it (`executionInfo.lastRunId`) but it has no source-available equivalent. |
| `data[*].last_success_run_id` | *(absent)* | — | ❌ | — | Class B: same as `last_run_id`. |
| `data[*].project_id` | *(absent)* | — | ❌ | — | Class B: Cloud concept; not in parquet. |
| `data[*].health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api. |
| `data[*].adapter_type` | *(absent)* | — | ❌ | — | The dbt-ui view reads `environment.adapterType` (top-level, not per-row) to decide whether to render the `Last modified` column. Adapter type is a project-level capability and lives on `GET /api/v1/capabilities` (or equivalent), not on per-row summaries. |

### Type definition

Type definition (for codegen reference). The field reference table above is the authoritative contract.

```typescript
interface SnapshotListResponse {
  data: SnapshotSummary[];
  page_info: PageInfo;
}

interface SnapshotSummary {
  unique_id: string;
  name: string;
  resource_type: "snapshot";
  package_name: string | null;
  materialized: "snapshot";
  // From config JSON; "timestamp" or "check".
  strategy: string | null;
  // From config JSON; column name for timestamp-strategy snapshots.
  updated_at: string | null;
  // null when dbt_rt.run_results has no row for this snapshot.
  execution_info: SnapshotListExecutionInfo | null;
  // null when dbt.catalog_tables has no row for this snapshot.
  catalog: SnapshotListCatalogInfo | null;
}

// Shape parallels ModelDetail.ExecutionInfo + ADR-4's `error` bare name.
interface SnapshotListExecutionInfo {
  status: string | null;
  completed_at: string | null;
  error: string | null;
}

// Narrower than SnapshotDetail.catalog: only the per-stat fields rendered
// by SnapshotFilterView. The full catalog (type/owner/primary_key/stats[])
// remains on GET /api/v1/snapshots/:id.
interface SnapshotListCatalogInfo {
  row_count_stat: number | null;
  bytes_stat: number | null;
  // Snowflake-only — null on other adapters.
  last_modified_stat: string | null;
}
// PageInfo: see ADR-6 § "Shared TypeScript types" for the canonical declaration.

```

### Risk register

1. **`strategy` and `updated_at` live inside the `config` JSON column.**
   `dbt.nodes` stores per-resource configuration as a JSON-string column
   (`config`), parsed handler-side via the shared `json_parse_or_null`
   helper per CC-7. Confirm that `config` is present and that
   `strategy` / `updated_at` are top-level keys inside it for snapshots
   before implementing. If they live under a nested sub-object
   (e.g. `config->>'snapshot'->>'strategy'`), update the SELECT
   expression accordingly. Failed parse must emit `null`, never bubble
   to the client.

2. **`row_count_stat` / `bytes_stat` / `last_modified_stat` keys are
   adapter-specific.** `dbt.catalog_stats` is keyed by
   `stat_id` strings that vary by adapter (e.g. `"bytes"` vs
   `"num_bytes"`). The sources detail handler already pivots from this
   table via a per-stat SQL projection; the snapshot list handler must
   reuse that pattern (one `LEFT JOIN` per stat key, filtered on
   `stat_id`). The exact key names per adapter must be confirmed
   against real Snowflake / BigQuery / Postgres indexes before
   implementing — the DuckDB sample project has zero `catalog_stats`
   rows for snapshots and cannot validate them. Until verified, the
   handler should tolerate any missing key and return `null` for that
   field, never bubble an error.

3. **Sample project may have zero snapshots.** The reference parquet at
   `~/codaz/sl-schema-evolution/sample_project/target/index/` is a
   semantic-layer-focused project and may not contain any snapshots.
   The handler must return `{ "data": [], "page_info": { "end_cursor": null,
   "has_next_page": false } }` cleanly when `dbt.nodes` yields no rows with
   `resource_type='snapshot'`. Integration tests must cover this case
   explicitly — do not rely on a populated sample for green tests.

4. **`execution_info.error` column presence in
   `dbt_rt.run_results`.** ADR-4 mandates the bare field name `error`
   (not `last_run_error`). The existing models handler does **not**
   return `error` — only `status`, `execution_time`, and
   `completed_at`. The snapshot LIST handler must add it to the run
   results CTE projection; if `dbt_rt.run_results` exposes an error /
   message column under a different name, document the mapping at
   implementation time and keep the wire field as `error`.

5. **No filter dropdowns means no `WHERE` accelerators today.** The
   handler needs only `WHERE n.resource_type = 'snapshot'` plus
   optional sort/limit. If a future iteration adds filter dropdowns
   (e.g. by strategy or by package), follow the models handler's
   `parse_*` + comma-OR pattern.

6. **`sort` allowlist is intentionally narrow.** Three columns:
   `name`, `package_name`, `updated_at` (top-level `dbt.nodes`
   columns or trivially-derivable JSON keys). Sorting by nested
   `execution_info.*` or `catalog.*` is deferred to avoid the LEFT
   JOIN ordering / NULLS-LAST complexity for v0; add explicitly to
   the allowlist when a real consumer surfaces.

7. **`data` envelope key vs. the existing models handler's `models`
   key.** The models handler still returns `{ "models": [...] }`.
   That predates ADR-6 and is being renamed in a companion PR. The
   snapshots handler must land with `data` from day one; do not copy
   the `models` precedent.

8. **`last_modified_stat` is Snowflake-only by design.** The dbt-ui
   view only renders the `Last modified` column when
   `adapterType === 'snowflake'` (per `ADAPTERS_WITH_LAST_MODIFIED_STAT`
   in `SnapshotFilterView.tsx`). The REST contract still exposes the
   field unconditionally — it's just `null` on every other adapter.
   This keeps the response schema stable across environments and
   pushes the rendering-gate decision to the client (where it
   already lives).

## `GET /api/v1/snapshots/facets`

Powers: filter dropdowns for the list view above.

### Query parameters

No query parameters.

### Example response

The snapshots list has no filter dropdowns, so the response body is an
empty object. The endpoint exists for API uniformity across resources.

```json
{}
```

### Field reference

No facet keys; the endpoint exists for API uniformity.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| *(none)* | — | — | — | — | Snapshots expose no filter dropdowns in dbt-ui (`SnapshotFilterView.tsx`). Response body is `{}`. |

### Type definition

Type definition (for codegen reference). The field reference table above is the authoritative contract.

```typescript
// Intentionally empty — present for API uniformity with other resources.
// If filters are ever added to SnapshotFilterView, append optional keys
// here following the FacetValue shape used by ModelFacetsResponse.
type SnapshotFacetsResponse = Record<string, never>;
```

### Risk register

No facet-specific risks; see LIST endpoint Risk register above.

---

## Design notes

Five decisions are specific to this LIST + FACETS pair and warrant calling out before review.

**1. Discriminated union on `resource_type` per ADR-3 — the LIST returns mixed-shape rows.**
ADR-3 settled the question for the detail endpoint: one endpoint covers both `test.*` and `unit_test.*` `unique_id`s, response shape narrowed by `resource_type`. The LIST inherits the same choice for the same reason — both folds into the same "Tests" tab in dbt-ui (`TestFilterView.tsx`), and `testTypesDisplay = ['unit', 'data']` collapses both into one filter dropdown. Splitting LIST into `/api/v1/tests` and `/api/v1/unit_tests` would force the FE to issue two paginated queries and merge them client-side. Instead, the LIST returns a single `data[]` envelope where each row carries a shared base (unique_id, name, resource_type, package_name, test_type, severity, tested_node_unique_id, tested_column, execution_info) plus union-arm-specific fields (data test → test_metadata; unit_test → fixture row counts). Clients narrow on `resource_type` exactly as they do for the detail endpoint.

**2. Per ADR-4: bare `execution_info` field names; no `last_known_result`.**
ADR-4 dropped `lastKnownResult` because it requires run history (the concept "did the test pass before a schema change invalidated it?" has no meaning in a single-snapshot index). The LIST therefore does **not** carry a `last_known_result` field; the current test outcome is `execution_info.status` and that is the only signal. Similarly, no `last_run_status` / `executeCompletedAt` — the inline `execution_info` uses bare `status`, `error`, `completed_at`, `execution_time` per ADR-4.

**3. Filter naming: dbt-ui → REST mapping.**
The FE filter view exposes three filters (`testStatus`/`status`/`testType`) with mappings as follows. The REST contract uses snake_case (CC-1) and singular-tense names consistent with the resource it filters on:

| dbt-ui search param | GraphQL filter | REST query param | Values |
|---|---|---|---|
| `testStatus` (UI label "Test result") | `lastKnownResults` | `result` | `pass` · `fail` · `warn` · `error` · `skipped` · `unknown` |
| `status` (UI label "Run status") | `status` | `run_status` | `success` · `error` · `skipped` · `reused` |
| `testType` (UI label "Test Type") | `testTypes` | `test_type` | `unit` · `data` (mirrors `testTypesDisplay`) |

Per ADR-4 the `result` and `run_status` filters both project onto `dbt_rt.run_results.status` at the SQL level — see Risk #4 for why both knobs coexist despite degenerating to one column in snapshot mode.

**4. Shared `tested_node_unique_id` field synthesized from two parquet sources.**
The FE renders a "Tested resource" column on every row (both variants). The parquet shape is asymmetric: `dbt.test_metadata.attached_node` carries the tested model's `unique_id` for data tests, but `dbt.unit_tests` stores the model as a name string in `model` and as a `unique_id` in `depends_on_nodes[0]`. The handler synthesizes a single `tested_node_unique_id` field on every row by reading `test_metadata.attached_node` for `resource_type = 'test'` and `unit_tests.depends_on_nodes[0]` for `resource_type = 'unit_test'`. This keeps the FE table cell logic uniform across the union — every row has the same field name regardless of variant.

**5. ADR-6 envelope; default sort `name:asc`; FACETS returns real keys (not `{}`).**
The LIST envelope is the ADR-6 standard `{ data, total, offset, limit }`. Top-level key is `data` — not `tests`. Default sort is `name:asc`; the FE exposes no sort controls so the allowlist holds only `name`. Filters are accepted as CSV via `?result=`, `?run_status=`, `?test_type=`. Unlike the seeds / semantic_models contracts (which return `{}` for facets because their FE views have no filter dropdowns), this contract surfaces three facet keys — `results`, `run_statuses`, `test_types` — because the FE filter view has three dropdowns to populate.

Parquet verification against the sample project (`~/codaz/sl-schema-evolution/sample_project/target/index/`):

- `dbt.nodes.parquet`: 20 rows with `resource_type = 'test'`, 2 rows with `resource_type = 'unit_test'` (the unit_test variant is present despite the `dbt.unit_tests` table being separate). All `NodeBase` columns present (`unique_id`, `name`, `package_name`, `original_file_path`, `tags`, `fqn`).
- `dbt.test_metadata.parquet`: 20 rows; one-to-one with `nodes.resource_type = 'test'`. Columns: `test_name` (e.g., `"accepted_values"`, `"expression_is_true"`), `test_namespace` (e.g., `"dbt_utils"`), `column_name` (sometimes null), `attached_node` (the tested model's `unique_id`), `severity` (all `null` in this sample — see Risk #2), `kwargs` (JSON string).
- `dbt.unit_tests.parquet`: 2 rows; one-to-one with `nodes.resource_type = 'unit_test'`. Columns: `model` (raw name string e.g. `"orders"`), `given` (JSON-string array), `expect` (JSON-string object), `depends_on_nodes` (`List(Utf8)`).
- `dbt_rt.run_results.parquet`: 20 rows for `test.*` + `unit_test.*` `unique_id`s; `status` column values were `"skipped"` (sample project never ran successfully) — the column type is `string` and accepts the full test-status enum (`pass`/`fail`/`warn`/`error`/`skipped`).
- `dbt_rt.test_failures.parquet`: 0 rows in this sample; column shape is `(unique_id, invocation_id, failure_rows)`. Not used by the LIST (failures are a detail-page concern); documented here so reviewers know it exists.

---

## `GET /api/v1/tests`

Powers: `TestFilterView` / `ResourceFilterPage` in dbt-ui.

dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/components/FilterPages/TestFilterView.tsx`
GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/discovery/appliedTests.ts` (`GetTestListByUniqueId` query, returns `AppliedTests`)

Paginated list of test nodes covering **both** `test.*` and `unit_test.*` `unique_id` prefixes in a single response (ADR-3 union). Six columns render in the FE table: Name, Test type, Test result, Run status, Tested resource, Column.

### Query parameters

Per ADR-6: `first`, `after`, and `sort` are universal. The FE exposes three filter dropdowns and no sort controls.

| Param | Type | Default | Notes |
|---|---|---|---|
| `first` | `u32` | `100` (max `1000`) | Per-page row count. Server clamps. Hard max `5000`; matches the `/api/v1/models` handler. |
| `after` | `string` (opaque base64) | — | Cursor returned as `page_info.end_cursor` on the previous page. Omit for the first page. Tampering yields 400. |
| `sort` | `string` | `name:asc` | Format `<column>:<asc\|desc>`. Allowlisted columns: `name` (the only column the FE could sort on if a control existed). Invalid column or direction → `400`. |
| `result` | `string` (csv) | — | Test outcome filter; values from `{pass, fail, warn, error, skipped, unknown}`. Multiple values OR'd: `?result=fail,error`. Maps to `dbt_rt.run_results.status` (see Risk #4). |
| `run_status` | `string` (csv) | — | Run-engine status filter; values from `{success, error, skipped, reused}`. Multiple values OR'd. Maps to `dbt_rt.run_results.status` (see Risk #4). |
| `test_type` | `string` (csv) | — | `unit` · `data`. `data` matches `resource_type = 'test'`; `unit` matches `resource_type = 'unit_test'`. Multiple values OR'd: `?test_type=unit,data` (no-op since it returns everything). |

### Example response

The `data` array carries **both** `test` and `unit_test` rows in their respective union-arm shapes. The shared base (unique_id, name, resource_type, package_name, test_type, severity, tested_node_unique_id, tested_column, execution_info) appears on every row. Variant-specific fields (test_metadata for data tests; given/expect row counts for unit tests) appear only on the matching variant — absent (not `null`) on the other.

Fields marked `// 🔧` are not yet returned — this endpoint does not exist yet, so every populated field is `🔧` at implementation time. Fields marked `// 🔍` are parquet-unverified for the specific value mapping and require confirmation against a real index (see Risk #2 for `severity`).

```json
{
  "data": [
    {
      "unique_id": "test.jaffle_shop.not_null_orders_order_id.d12f0947c8",
      "name": "not_null_orders_order_id",
      "resource_type": "test",
      "package_name": "jaffle_shop",
      "test_type": "not_null",
      "tested_node_unique_id": "model.jaffle_shop.orders",
      "tested_column": "order_id",
      "severity": "ERROR",
      "test_metadata": {
        "namespace": null,
        "kwargs": { "column_name": "order_id", "model": "ref('orders')" }
      },
      "execution_info": {
        "status": "pass",
        "error": null,
        "completed_at": "2026-05-15T10:32:11Z"
      }
    },
    {
      "unique_id": "test.jaffle_shop.dbt_utils_expression_is_true_orders_subtotal.b1416e07ec",
      "name": "dbt_utils_expression_is_true_orders_subtotal",
      "resource_type": "test",
      "package_name": "jaffle_shop",
      "test_type": "expression_is_true",
      "tested_node_unique_id": "model.jaffle_shop.orders",
      "tested_column": null,
      "severity": "WARN",
      "test_metadata": {
        "namespace": "dbt_utils",
        "kwargs": { "expression": "subtotal >= 0", "model": "ref('orders')" }
      },
      "execution_info": {
        "status": "fail",
        "error": "Got 12 results, expected 0.",
        "completed_at": "2026-05-15T10:32:14Z"
      }
    },
    {
      "unique_id": "unit_test.jaffle_shop.orders.test_supply_costs_sum_correctly",
      "name": "test_supply_costs_sum_correctly",
      "resource_type": "unit_test",
      "package_name": "jaffle_shop",
      "test_type": "unit",
      "tested_node_unique_id": "model.jaffle_shop.orders",
      "tested_column": null,
      "severity": null,
      "num_given": 1,
      "num_given_rows": 3,
      "num_expect_rows": 3,
      "execution_info": {
        "status": "pass",
        "error": null,
        "completed_at": "2026-05-15T10:32:15Z"
      }
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiPHN0YXJ0X3NvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "end_cursor": "eyJzIjoiPHNvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "has_next_page": false
  }
}
```

`execution_info` is `null` when `has_run_results` is false (i.e., `dbt build` / `dbt test` has not run). When `has_run_results` is true but a specific test has no `run_results` row, `execution_info` is still `null` (no fabrication).

Variant-specific fields (`test_metadata` on data tests; `num_given` / `num_given_rows` / `num_expect_rows` on unit tests) are **absent** (omitted from the JSON, not serialized as `null`) on the opposite variant. Clients narrow on `resource_type` before reading them.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

Fields that appear in only one variant are flagged in the Notes column. Every row below is `🔧` (or `🔍` where parquet semantics are unverified) because no LIST handler exists today.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `data` | `TestSummary[]` | Core | 🔧 | — | ADR-6 envelope key. Discriminated union of `DataTestSummary \| UnitTestSummary`. Empty array when no tests. |
| `data[*].unique_id` | `string` | Core | 🔧 | — | e.g., `"test.pkg.name.hash"` or `"unit_test.pkg.model.name"`. From `dbt.nodes.unique_id`. |
| `data[*].name` | `string` | Core | 🔧 | — | From `dbt.nodes.name`. |
| `data[*].resource_type` | `"test" \| "unit_test"` | Core | 🔧 | — | Discriminator — determines which variant fields are present. |
| `data[*].package_name` | `string \| null` | Core | 🔧 | — | From `dbt.nodes.package_name`. |
| `data[*].test_type` | `string \| null` | Core | 🔧 | — | For `resource_type = 'test'`: from `dbt.test_metadata.test_name` (e.g., `"not_null"`, `"unique"`, `"accepted_values"`, `"relationships"`, custom test names like `"expression_is_true"`). For `resource_type = 'unit_test'`: always the literal string `"unit"`. The FE collapses these into two display buckets (`unit`, `data`) via `translateFromTestType`; the `?test_type=` query param accepts those bucket values. |
| `data[*].tested_node_unique_id` | `string \| null` | Core | 🔧 | — | The model under test as a `unique_id`. For `resource_type = 'test'`: `dbt.test_metadata.attached_node`. For `resource_type = 'unit_test'`: `dbt.unit_tests.depends_on_nodes[0]` (the first model dependency). Synthesized server-side so the FE has a uniform field across the union — see Design note #4. |
| `data[*].tested_column` | `string \| null` | Core | 🔧 | — | Column under test for column-level data tests (`not_null`, `unique`, `accepted_values`, etc.). From `dbt.test_metadata.column_name`. Always `null` for unit tests and for table-level data tests (`relationships`, `expression_is_true`, etc.). |
| `data[*].severity` | `string \| null` | Core | 🔍 | — | `"ERROR"` · `"WARN"` for data tests; `null` for unit tests. From `dbt.test_metadata.severity`. **All values null in the sample project** — see Risk #2; column type confirmed as `string`. |
| `data[*].execution_info` | `TestExecutionInfo \| null` | Core-conditional | 🔧 | `has_run_results` | `null` when `dbt build` hasn't run (capability flag false) OR when this specific test has no `run_results` row. Present on both variants. |
| `data[*].execution_info.status` | `string \| null` | Core-conditional | 🔧 | `has_run_results` | `"pass"` · `"fail"` · `"error"` · `"warn"` · `"skipped"` · `"reused"`. From `dbt_rt.run_results.status` (column type confirmed `string`). ADR-4 bare name (no `last_run_status`). |
| `data[*].execution_info.error` | `string \| null` | Core-conditional | 🔧 | `has_run_results` | Error message; from `dbt_rt.run_results.message`. `null` when status is `"pass"` / `"skipped"`. ADR-4 bare name (no `last_run_error`). |
| `data[*].execution_info.completed_at` | `string \| null` | Core-conditional | 🔧 | `has_run_results` | ISO 8601 timestamp; from `dbt_rt.run_results.created_at`. ADR-4 bare name (no `executeCompletedAt`). |
| `data[*].test_metadata` | `TestMetadataSummary \| null` | Core | 🔧 | — | **data test only** — *absent* on unit_test rows (not `null`). From `dbt.test_metadata`. |
| `data[*].test_metadata.namespace` | `string \| null` | Core | 🔧 | — | **data test only** — from `dbt.test_metadata.test_namespace` (e.g., `"dbt_utils"` for `dbt_utils.expression_is_true`; `null` for first-party tests like `not_null`). |
| `data[*].test_metadata.kwargs` | `Record<string, unknown>` | Core | 🔍 | — | **data test only** — unstructured JSON, deserialized handler-side via CC-7 `json_parse_or_null`. From `dbt.test_metadata.kwargs` (a JSON string column). Parse failure → `null` + `tracing::warn`; never escaped JSON in the wire response. |
| `data[*].num_given` | `number \| null` | Core | 🔍 | — | **unit test only** — *absent* on data test rows. Count of `given` fixtures; derived via `json_array_length(given)` or by parsing `dbt.unit_tests.given` (JSON string) and counting top-level entries. See Risk #5. |
| `data[*].num_given_rows` | `number \| null` | Core | 🔍 | — | **unit test only** — total input rows across all fixtures. Derived from `dbt.unit_tests.given` JSON. See Risk #5. |
| `data[*].num_expect_rows` | `number \| null` | Core | 🔍 | — | **unit test only** — expected output row count. Derived from `dbt.unit_tests.expect` JSON. See Risk #5. |
| `page_info` | `PageInfo` | Core | 🔧 | — | ADR-6 cursor envelope. See `PageInfo` definition in the shared types. Replaces the offset-era `total`/`offset`/`limit` triple. |
| `page_info.total_count` | `number` | Core | 🔧 | — | Total row count under the current filter set; ignores `first`/`after`. Separate `COUNT(*)` query per request. |
| `page_info.start_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor pointing to the FIRST row of the current page. `null` when `data` is empty. Symmetric with `end_cursor`. |
| `page_info.end_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor; pass back as `?after=...` to fetch the next page. `null` when `has_next_page` is `false`. |
| `page_info.has_next_page` | `boolean` | Core | 🔧 | — | `true` if at least one more row exists past this page. Server implements via `LIMIT first+1` and trim. |
| `data[*].original_file_path` | *(absent)* | — | ❌ | — | Available on the detail endpoint but not rendered in the FE filter table; defer to `GET /api/v1/tests/:id`. |
| `data[*].tags` | *(absent)* | — | ❌ | — | Available on the detail endpoint; not rendered in the LIST view. |
| `data[*].fqn` | *(absent)* | — | ❌ | — | Available on the detail endpoint; not rendered. |
| `data[*].description` | *(absent)* | — | ❌ | — | Not rendered in the LIST view; defer to detail. |
| `data[*].depends_on` | *(absent)* | — | ❌ | — | Lineage edges are a detail-page concern (`tested_node_unique_id` synthesized from `depends_on_nodes[0]` is the only LIST-level need). |
| `data[*].raw_code` | *(absent)* | — | ❌ | — | SQL body is a detail-page concern (Code tab). |
| `data[*].compiled_code` | *(absent)* | — | ❌ | — | Same as `raw_code`. |
| `data[*].given` | *(absent)* | — | ❌ | — | **unit test only** — full fixture rows are a detail-page concern; LIST exposes only the row counts (`num_given`, `num_given_rows`). |
| `data[*].expect` | *(absent)* | — | ❌ | — | **unit test only** — same as `given`. |
| `data[*].last_known_result` | *(absent)* | — | ❌ | — | Per ADR-4, dropped. The current outcome is `execution_info.status`. |
| `data[*].project_id` | *(absent)* | — | ❌ | — | Class B: Cloud concept; not in parquet. |
| `data[*].last_run_id` | *(absent)* | — | ❌ | — | Class B: Cloud run ID; not in local parquet. |
| `data[*].health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface TestListResponse {
  data: TestSummary[];   // ADR-3 discriminated union
  page_info: PageInfo;
}

// Discriminated union on resource_type — mirrors GET /api/v1/tests/:id (ADR-3).
type TestSummary = DataTestSummary | UnitTestSummary;

// Shared fields factored here for documentation; Rust composes a NodeBase.
interface TestSummaryBase {
  unique_id: string;
  name: string;
  package_name: string | null;
  test_type: string | null;
  tested_node_unique_id: string | null;
  tested_column: string | null;
  severity: string | null;
  execution_info: TestExecutionInfo | null;
}

interface DataTestSummary extends TestSummaryBase {
  resource_type: "test";
  test_metadata: TestMetadataSummary | null;
}

interface UnitTestSummary extends TestSummaryBase {
  resource_type: "unit_test";
  num_given: number | null;
  num_given_rows: number | null;
  num_expect_rows: number | null;
}

interface TestExecutionInfo {
  status: string | null;        // ADR-4 bare name
  error: string | null;         // ADR-4 bare name
  completed_at: string | null;  // ADR-4 bare name
}

interface TestMetadataSummary {
  namespace: string | null;
  kwargs: Record<string, unknown>;
}
// PageInfo: see ADR-6 § "Shared TypeScript types" for the canonical declaration.

```

### Risk register

1. **Two parquet sources required per variant; one query joins both.** A complete LIST query joins `dbt.nodes` (filter on `resource_type IN ('test', 'unit_test')` for the base columns) with `dbt.test_metadata` LEFT JOIN (only the `test` variant has rows here) and `dbt.unit_tests` LEFT JOIN (only the `unit_test` variant has rows there). The handler can emit a single SQL string with both LEFT JOINs and project NULLs for the other variant's columns, then decode the union arm based on `resource_type` in the Rust mapper — same pattern as the detail endpoint will use.

2. **`severity` is `string` in parquet but empirically `null` in the sample.** The `dbt.test_metadata.severity` column type is confirmed `string`, but all 20 rows in the sample project carry `null`. Production dbt projects do populate this column (`"ERROR"` / `"WARN"`); the contract documents the field as `Core 🔍` to flag that the value-mapping verification waits on a populated index. The handler must SELECT it and emit verbatim; do not invent a default.

3. **`run_results.status` is the only outcome signal — `result` and `run_status` filter the same column.** dbt-docs-server is a snapshot server (ADR-4): there is no separate "last known result" timeline. The FE filter view exposes both "Test result" (mapped to `?result=`) and "Run status" (mapped to `?run_status=`); both target `dbt_rt.run_results.status`. The two filters use different value enums (`pass`/`fail`/`warn`/`error`/`skipped`/`unknown` vs. `success`/`error`/`skipped`/`reused`), so a request like `?run_status=success` is degenerate for tests (test runs never report `success`; they report `pass`). The handler accepts both for FE parity but documents the degeneracy in the OpenAPI description. Future cleanup: deprecate `?run_status=` for tests once the FE drops the dropdown.

4. **`test_type` filter semantics: bucket vs. literal.** The `?test_type=` query param accepts the FE's display buckets (`unit`, `data`) — not the literal `test_name` values (`not_null`, `unique`, etc.). `?test_type=unit` filters `resource_type = 'unit_test'`; `?test_type=data` filters `resource_type = 'test'`. The response `data[*].test_type` field, however, returns the **literal** test name (e.g., `"not_null"`, `"accepted_values"`) for data tests and `"unit"` for unit tests. Asymmetric on purpose: the filter mirrors `testTypesDisplay`; the response value preserves the parquet datum so downstream filtering (e.g., facet counts in a future iteration) can drill down by test name.

5. **`num_given` / `num_given_rows` / `num_expect_rows` are derived, not stored.** `dbt.unit_tests` does not pre-compute these counts; `given` and `expect` are JSON strings (`"[{...}, {...}]"`). DuckDB's `json_array_length()` can count top-level array entries cheaply, but summing rows across `given[*].rows[*]` requires either (a) a `json_extract` + nested aggregation or (b) parsing the JSON in Rust and counting. Pick (b) for now: parse with `json_parse_or_null` (CC-7), fall through to `null` on parse failure. Two rows in the sample project; performance is fine.

6. **`tested_node_unique_id` synthesis is asymmetric across the union.** Data tests pull from `dbt.test_metadata.attached_node` (always a `unique_id`); unit tests pull from `dbt.unit_tests.depends_on_nodes[0]` (a `List(Utf8)` of `unique_id`s). The list ordering of `depends_on_nodes` is the order dbt emitted dependencies — not guaranteed to put the model-under-test first, but in practice it does (the unit test's primary subject is the first dependency). Document this assumption; add a Rust unit test asserting first-element semantics. If a unit test ever has zero or multiple model dependencies, fall back to `null` rather than picking arbitrarily.

7. **`kwargs` is unstructured JSON — fragile for `relationships` tests.** Mirrors detail-contract Risk #6: `relationships` tests pack `to:`, `field:`, `column_name:` into kwargs. The LIST exposes `test_metadata.kwargs` as a `Record<string, unknown>` blob; the FE is responsible for any structural narrowing. Document the shape so the FE team does not expect a typed object.

8. **No sort controls in v0; allowlist is one column.** The FE filter view exposes no sort controls. The `?sort=` allowlist holds `name` only. Adding more (e.g., `executed_at`, `severity`) is additive and does not break the contract; resist pre-adding them — every allowlisted column is a tested code path.

9. **Pagination semantics across the union.** `total` counts the **combined** set of `test.*` + `unit_test.*` rows after filters. A page of `limit=10` may contain any mix of variants depending on the sort and filter. Clients that want a variant-specific count should issue two queries: `?test_type=data` and `?test_type=unit`. Do not expose `total_by_resource_type` in v0 — that's surface area without a consumer.

---

## `GET /api/v1/tests/facets`

Powers: the three filter dropdowns in `TestFilterView` — Test result, Run status, Test Type.

### Query parameters

None. Any query parameters supplied are ignored (per the seeds / semantic_models contracts — keeps the endpoint hospitable to future query-string additions).

### Example response

Each key contains a `FacetValue[]` array. `count` is `null` today — reserved for a future enhancement that returns the number of tests matching each filter value without a full LIST query (matches the existing `ModelFacetsResponse` shape).

```json
{
  "results": [
    { "value": "pass", "count": null },
    { "value": "fail", "count": null },
    { "value": "warn", "count": null },
    { "value": "error", "count": null },
    { "value": "skipped", "count": null },
    { "value": "unknown", "count": null }
  ],
  "run_statuses": [
    { "value": "success", "count": null },
    { "value": "error", "count": null },
    { "value": "skipped", "count": null },
    { "value": "reused", "count": null }
  ],
  "test_types": [
    { "value": "unit", "count": null },
    { "value": "data", "count": null }
  ]
}
```

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `results` | `FacetValue[]` | Core | 🔧 | — | Six values from `testStatusesPlusUnknown` (dbt-dag `resourceConstants.ts`): `pass`, `fail`, `warn`, `error`, `skipped`, `unknown`. Server constant — does **not** query parquet for distinct values, because the enum is fixed by dbt-core (`testStatusesPlusUnknown` is the closed set). Pattern matches `accesses` in `ModelFacetsResponse`. |
| `run_statuses` | `FacetValue[]` | Core | 🔧 | — | Four values from `runStatuses` (dbt-dag `resourceConstants.ts`): `success`, `error`, `skipped`, `reused`. Server constant. |
| `test_types` | `FacetValue[]` | Core | 🔧 | — | Two display buckets from `testTypesDisplay`: `unit`, `data`. Server constant. |
| `results[*].value` | `string` | Core | 🔧 | — | The filter value the FE submits as `?result=<value>`. |
| `results[*].count` | `number \| null` | Core | 🔧 | — | Always `null` today. Reserved for future per-facet counts. Same shape as `ModelFacetsResponse.accesses[*].count`. |
| `run_statuses[*].value` | `string` | Core | 🔧 | — | Same shape as `results[*].value`. |
| `run_statuses[*].count` | `number \| null` | Core | 🔧 | — | Same shape. |
| `test_types[*].value` | `string` | Core | 🔧 | — | Same shape. |
| `test_types[*].count` | `number \| null` | Core | 🔧 | — | Same shape. |
| `severities` | *(absent)* | — | ❌ | — | The FE has no severity filter dropdown. Adding one is additive — append a `severities` key whose values are sourced from `dbt.test_metadata.severity` (server constant `["ERROR", "WARN"]` plus the empirical-null case if needed). |
| `packages` | *(absent)* | — | ❌ | — | No FE filter; add when a filter ships. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface TestFacetsResponse {
  results: FacetValue[];
  run_statuses: FacetValue[];
  test_types: FacetValue[];
}

// Identical shape to ModelFacetsResponse.FacetValue — share the type at the
// generator level so client codegen produces a single FacetValue interface.
interface FacetValue {
  value: string;
  count: number | null;   // always null today
}
```

### Risk register

1. **All three facet value-sets are server constants — no parquet query.** `runStatuses`, `testStatusesPlusUnknown`, and `testTypesDisplay` are closed enums defined in dbt-dag's `resourceConstants.ts` and (in the case of run/test statuses) ultimately in dbt-core (`core/dbt/contracts/results.py`). Sourcing them from parquet would surface only the values that happen to exist in the current index — a project with no `warn` outcomes would lose the `warn` filter option until one fired. Server constants keep the filter UI stable across project state. This is consistent with `ModelFacetsResponse.modeling_layers` and `accesses`, which are also server-constant.

2. **`results` enum may drift from dbt-core.** `testStatusesPlusUnknown` adds `unknown` on top of `testStatuses` (the dbt-core enum) and intentionally drops `reused` (per the comment in `resourceConstants.ts`: "filters use lastKnownResult … which has no reused value"). The handler must match dbt-dag's `testStatusesPlusUnknown`, not dbt-core's `testStatuses`. Add a test that asserts the array contents are exactly `["pass", "fail", "warn", "error", "skipped", "unknown"]` to catch drift if dbt-dag evolves.

3. **`count: null` is a deliberate stub.** Mirrors `ModelFacetsResponse.FacetValue.count`. Populating real counts requires either (a) running a `GROUP BY` per facet key on every facets request or (b) caching counts and invalidating on index reload. Neither is in v0 scope; the field is reserved so adding counts later is additive (no schema break).

4. **No `severities` facet despite the field existing.** The FE filter view has no severity dropdown today. The contract documents the absence (see `severities` row in the field reference) so reviewers don't add it speculatively. When the FE adds a severity dropdown, append `severities: FacetValue[]` and the matching `?severity=` query param on the LIST endpoint.

---

## Design notes — `GET /api/v1/exposures` + `GET /api/v1/exposures/facets`

Powers the `ExposureFilterView` page in dbt-ui — the project-level table of all
exposures with a single Owner filter (plus two feature-gated Cloud-only filters).

dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/components/FilterPages/ExposureFilterView.tsx`

GraphQL hooks:
- `packages/metadata/dbt-explorer/src/hooks/discovery/appliedExposures.ts` (`AppliedExposures` query — list rows)
- `packages/metadata/dbt-explorer/src/hooks/discovery/exposreOwners.ts` (`GetExposureOwners` query — owner facet; filename misspelling is intentional, matches the source)

Per ADR-5, exposures are definition-only (no `execution_info`, no `dbt_rt.run_results`
row); the row surface includes `created_at` (epoch seconds from
`dbt.exposures.created_at`) as the "Definition updated as of …" timestamp surrogate.

This contract covers only the **list row** and **facets** shapes. The detail
endpoint already exists in `API-CONTRACTS.md` § `GET /api/v1/exposures/:id`. Row
fields are a subset of detail fields chosen to power the FilterView table columns
(Name, Health, Type, Owner, Owner email) plus the inline-edge `depends_on[]`
truncation envelope from CC-6 to support upstream-count badges.

### Decisions worth flagging to the coordinator

1. **No sorts are exposed.** The dbt-ui `ExposureFilterView` does not call any
   GraphQL `orderBy` argument and renders rows in server-returned order
   (`AppliedExposures` returns the codex-api default). Per ADR-6 the LIST endpoint
   still defaults to `name:asc` for client-stable rendering; `?sort` is **not**
   accepted (validation rejects any value). If the FE adds sort UI later, add the
   sort allowlist additively — no schema change.

2. **Owner filter is exact match, single value.** `OwnerFilterDropdown` writes a
   single `owner=<name>` query param (no comma-separated multi-select). The
   handler validates exact match against `dbt.exposures.owner_name` (`= '<value>'`,
   not `IN (...)`). Keep parity with `ExposureFilterView`'s contract until a
   product requirement justifies a list filter.

3. **`AutoBiProvider` and `ExposureMode` filters are Class C (Platform-only).**
   Both filters are feature-gated in dbt-ui behind `hasAccountFeature('explorerAutoExposures')`.
   `dbt.exposures.parquet` has no `auto_bi_provider`, no `definition_type`, no
   `integration_id` column (verified empirically — schema has only the core
   exposure spec fields plus `created_at`, `ingested_at`, `meta`, `config`,
   `depends_on_*`). Both filters are **omitted** from the facets response — not
   stubbed `412` — because the upstream signal does not exist in any parquet
   column to gate on. Same rationale used for the row-side `auto_bi_provider`
   field on `GET /api/v1/exposures/:id`.

4. **`health_issues[]` is not exposed on the list row.** Class B per ADR-5
   discussion in the detail contract — `subGraphs: ['internal']` in codex-api,
   aggregated from upstream test/source health and not in parquet. The FE
   `ExposureTrustSignalCell` renders a graceful empty state on `null`/`[]`. List
   row omits the field entirely (consistent with detail-contract Class B
   treatment of internal-only signals).

5. **`depends_on` truncation per CC-6.** The list row includes `depends_on[]`
   (1-hop upstream models + sources, parsed from `dbt.exposures.depends_on_nodes`)
   capped at the CC-6 default of 500 with `depends_on_truncated: true` signaling.
   FilterView itself does not render this today, but the row shape mirrors the
   detail contract to keep type definitions reusable across the list and detail
   pages; future "upstream count" badges become an FE-only change.

---

## `GET /api/v1/exposures`

Powers: `ExposureFilterView` table rows in dbt-ui.

### Query parameters

| Parameter | Type | Default | Notes |
|---|---|---|---|
| `first` | `u32` | `100` (max `1000`) | Per-page row count. Server clamps. Per ADR-6 cursor envelope. |
| `after` | `string` (opaque base64) | — | Cursor returned as `page_info.end_cursor` on the previous page. Omit for the first page. Tampering yields 400. |
| `sort` | `string` | `name:asc` | **Rejected with 400 if provided.** ExposureFilterView exposes no sorts. The default is internal; clients should not pass `?sort=...`. |
| `owner` | `string` | — | Exact-match filter against `dbt.exposures.owner_name`. Single value only (no comma-separated list). |

`?auto_bi_provider` and `?definition_type` are **not accepted** — see Design notes #3.
Passing them is silently ignored (unknown query params are not 400s, per the
existing `list_models` handler convention).

### Example response

```json
{
  "data": [
    {
      "unique_id": "exposure.jaffle_shop.revenue_dashboard",
      "name": "revenue_dashboard",
      "exposure_type": "dashboard",
      "maturity": "high",
      "owner_name": "Jane Doe",
      "owner_email": "jane.doe@example.com",
      "tags": ["finance", "exec"],
      "created_at": 1747432300.5,
      "depends_on": [
        { "unique_id": "model.jaffle_shop.orders", "edge_type": "model" },
        { "unique_id": "source.jaffle_shop.raw_jaffle.orders", "edge_type": "source" }
      ],
      "depends_on_truncated": false
    },
    {
      "unique_id": "exposure.jaffle_shop.churn_notebook",
      "name": "churn_notebook",
      "exposure_type": "notebook",
      "maturity": "medium",
      "owner_name": "Alex Park",
      "owner_email": "alex.park@example.com",
      "tags": [],
      "created_at": 1747104900.0,
      "depends_on": [
        { "unique_id": "model.jaffle_shop.customers", "edge_type": "model" }
      ],
      "depends_on_truncated": false
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiPHN0YXJ0X3NvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "end_cursor": "eyJzIjoiPHNvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "has_next_page": false
  }
}
```

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

Per ADR-6, the response envelope is fixed: `data: T[]`, `total: number`, `offset: number`, `limit: number`. No handler exists today; every row field is 🔧.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `data` | `ExposureSummary[]` | Core | 🔧 | — | ADR-6 list envelope key — not `exposures` |
| `page_info` | `PageInfo` | Core | 🔧 | — | ADR-6 cursor envelope. See `PageInfo` definition in the shared types. Replaces the offset-era `total`/`offset`/`limit` triple. |
| `page_info.total_count` | `number` | Core | 🔧 | — | Total row count under the current filter set; ignores `first`/`after`. Separate `COUNT(*)` query per request. |
| `page_info.start_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor pointing to the FIRST row of the current page. `null` when `data` is empty. Symmetric with `end_cursor`. |
| `page_info.end_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor; pass back as `?after=...` to fetch the next page. `null` when `has_next_page` is `false`. |
| `page_info.has_next_page` | `boolean` | Core | 🔧 | — | `true` if at least one more row exists past this page. Server implements via `LIMIT first+1` and trim. |
| `data[*].unique_id` | `string` | Core | 🔧 | — | e.g., `"exposure.pkg.name"` — from `dbt.exposures.unique_id` |
| `data[*].name` | `string` | Core | 🔧 | — | From `dbt.exposures.name`; FilterView "Name" column |
| `data[*].exposure_type` | `string \| null` | Core | 🔧 | — | `"dashboard"` · `"notebook"` · `"analysis"` · `"ml"` · `"application"` — from `dbt.exposures.exposure_type`; FilterView "Type" column. See `GET /api/v1/exposures/:id` Risk #2 (enum unvalidated at parse time). |
| `data[*].maturity` | `string \| null` | Core | 🔧 | — | `"high"` · `"medium"` · `"low"` — from `dbt.exposures.maturity`. Not surfaced as a FilterView column today, but available to consumers (the detail contract surfaces it on the header card). |
| `data[*].owner_name` | `string \| null` | Core | 🔧 | — | From `dbt.exposures.owner_name`; FilterView "Owner" column |
| `data[*].owner_email` | `string \| null` | Core | 🔧 | — | From `dbt.exposures.owner_email`; FilterView "Owner email" column |
| `data[*].tags` | `string[]` | Core | 🔧 | — | From `dbt.exposures.tags` (list_utf8 column). Empty array if none. |
| `data[*].created_at` | `number \| null` | Core | 🔧 | — | Epoch seconds (float); from `dbt.exposures.created_at`. Per ADR-5 the definition-time surrogate for "last updated" on definition-only resources. |
| `data[*].depends_on` | `EdgeRef[]` | Core | 🔧 | — | 1-hop upstream models + sources; derived from `dbt.exposures.depends_on_nodes`. Truncated at 500 per CC-6 — see Risk #2. `edge_type` resolved from the `unique_id` prefix (`model.` → `"model"`, `source.` → `"source"`, etc.), matching the detail contract Risk #4. |
| `data[*].depends_on_truncated` | `boolean` | Core | 🔧 | — | CC-6 truncation signal. `true` when the underlying upstream list exceeded 500 entries and was capped. |
| `data[*].depends_on[*].unique_id` | `string` | Core | 🔧 | — | |
| `data[*].depends_on[*].edge_type` | `string` | Core | 🔧 | — | `"model"` / `"source"` / `"metric"` / `"seed"` — derived from the prefix |
| `data[*].health_issues` | *(absent)* | — | ❌ | — | Class B: `subGraphs: ['internal']` in codex-api; aggregated from upstream test/source health. Not in parquet. Matches detail-contract treatment. FilterView "Health" column renders graceful empty state. |
| `data[*].auto_bi_provider` | *(absent)* | — | ❌ | — | Class C (Platform-only) — feature-gated in dbt-ui behind `explorerAutoExposures`. No column in `dbt.exposures.parquet`. See Design notes #3. |
| `data[*].integration_id` | *(absent)* | — | ❌ | — | Class C (Platform-only) — auto-exposure-only field. No parquet path. |
| `data[*].definition_type` | *(absent)* | — | ❌ | — | Class C (Platform-only) — drives the FilterView "Exposure Mode" column. No parquet path. |
| `data[*].manifest_generated_at` | *(absent)* | — | ❌ | — | Class B: environment-level field on the GraphQL `applied` wrapper, not a per-row column |
| `data[*].package_name` | *(absent)* | — | ❌ | — | Available on the detail endpoint; intentionally omitted from list rows (FilterView does not consume it) |
| `data[*].description` | *(absent)* | — | ❌ | — | Available on the detail endpoint; not a FilterView column |
| `data[*].meta` | *(absent)* | — | ❌ | — | Available on the detail endpoint; not a FilterView column |
| `data[*].url` | *(absent)* | — | ❌ | — | Available on the detail endpoint; not a FilterView column (the link target is rendered from the detail page) |
| `data[*].label` | *(absent)* | — | ❌ | — | Available on the detail endpoint; not a FilterView column |
| `data[*].file_path` | *(absent)* | — | ❌ | — | Available on the detail endpoint header; not a FilterView column |
| `data[*].original_file_path` | *(absent)* | — | ❌ | — | Available on the detail endpoint header; not a FilterView column |
| `data[*].fqn` | *(absent)* | — | ❌ | — | Available on the detail endpoint; not a FilterView column |
| `data[*].execution_info` | *(absent)* | — | ❌ | — | Per ADR-5 exposures never execute; omitted from `DefinitionNodeBase` |
| `data[*].referenced_by` | *(absent)* | — | ❌ | — | Exposures are terminal leaf nodes; nothing refs an exposure (matches detail contract) |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface ExposureListResponse {
  data: ExposureSummary[];
  page_info: PageInfo;
}

interface ExposureSummary {
  unique_id: string;
  name: string;
  exposure_type: string | null;
  maturity: string | null;
  owner_name: string | null;
  owner_email: string | null;
  tags: string[];
  created_at: number | null;       // epoch seconds; ADR-5 definition surrogate
  depends_on: EdgeRef[];           // capped at 500 per CC-6
  depends_on_truncated: boolean;   // CC-6 truncation signal
}

// EdgeRef shared with ModelDetail, SourceDetail, SeedDetail, SnapshotDetail, ExposureDetail
interface EdgeRef {
  unique_id: string;
  edge_type: string;
}
// PageInfo: see ADR-6 § "Shared TypeScript types" for the canonical declaration.

```

### Risk register

1. **No handler exists today.** No `src/handlers/exposures.rs` in the worktree. The
   implementation PR must add the handler, register `GET /api/v1/exposures` and
   `GET /api/v1/exposures/facets` in `src/server.rs`, and add the response types to
   `web/src/api.ts`. Composing the shared `DefinitionNodeBase` Rust struct (ADR-5
   backend prerequisite) is recommended but the list row is a flat projection — no
   `NodeBase` composition is strictly required at the row level.

2. **`depends_on[]` truncation requires per-row LIMIT.** With CC-6's 500-entry cap
   per row, a project with 1000 exposures × 1000 upstream models per exposure
   would generate 500 000 rows from a naive `LEFT JOIN dbt.edges`. The handler
   should either (a) build the `depends_on[]` array via a correlated subquery
   with `LIMIT 500` per exposure unique_id, or (b) build it from
   `dbt.exposures.depends_on_nodes` (already a list_utf8 column on the row) with
   `list_slice(depends_on_nodes, 1, 500)` and a length comparison for the
   `depends_on_truncated` flag. Option (b) is cheaper and matches the detail
   contract Risk #4 prefix-parsing approach. Pick before implementation.

3. **`owner` filter exactness vs. `dbt.exposures.owner_name` nullability.**
   `owner_name` is a nullable utf8 column. A `WHERE owner_name = '<value>'`
   predicate excludes NULL owners — matching `ExposureFilterView` behavior (the
   dropdown lists only non-null `name` values from the `GetExposureOwners`
   GraphQL query). Document this explicitly so the handler does not accidentally
   `COALESCE` away the filter.

4. **`total` count requires a separate query.** Standard pattern shared with
   `list_models` — count query + rows query, both filtered. Reuse the
   `query_scalar(count_sql)` + `query_arrow(rows_sql)` shape from
   `crates/dbt-docs-server/src/handlers/models.rs::list_models`.

5. **`?sort` rejection vs. forward compatibility.** The handler should validate
   `?sort` against an empty allowlist (returns 400 with "sort is not supported on
   this endpoint"). If a sort allowlist is added later, it becomes additive — no
   client breakage. **Do not** silently ignore `?sort`: a future addition would
   change the rendered order without the client opting in, masking bugs.

6. **`exposure_type` and `maturity` enum unvalidation.** Both columns are plain
   `utf8` in `dbt.exposures.parquet` (no enum constraint). The handler must pass
   the raw string through. FE consumers branching on these values (e.g.,
   `ExposureStatusTileSection.tsx` checks `exposureType === 'dashboard'`) need
   case-sensitive matches. Document the expected lowercase enum values in
   `web/src/api.ts` as a TypeScript string-literal union *for consumer
   convenience only* — the runtime validation lives in the dbt parser, not in
   this handler.

7. **Empty project handling.** When `dbt.exposures.parquet` has zero rows (a
   project with no exposures), the handler must return
   `{ "data": [], "page_info": { "end_cursor": null, "has_next_page": false } }` — not a 404.
   Consistent with `list_models` behavior.

---

## `GET /api/v1/exposures/facets`

Powers: filter dropdowns in `ExposureFilterView` (Owner dropdown today; the
feature-gated AutoBiProvider and Exposure Mode dropdowns are Platform-only and
not surfaced here).

### Query parameters

None. The facets endpoint takes no query parameters per ADR-6.

### Example response

```json
{
  "owners": [
    { "value": "Alex Park", "count": null },
    { "value": "Jane Doe", "count": null }
  ]
}
```

When `dbt.exposures.parquet` has zero rows (or every `owner_name` is NULL), the
response is `{ "owners": [] }`.

The `count` field is reserved for a future enhancement that returns the number
of matching exposures per owner. Today it is always `null` — matching the
`list_model_facets` convention.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `owners` | `FacetValue[]` | Core | 🔧 | — | Distinct non-null `dbt.exposures.owner_name` values, sorted ascending. Sourced from `dbt.exposures` (NOT `dbt.groups`) because exposure owners are spec'd in the exposure YAML, not in `dbt.groups`. |
| `owners[*].value` | `string` | Core | 🔧 | — | Owner name as written in the exposure YAML's `owner.name` field |
| `owners[*].count` | `number \| null` | Core | 🔧 | — | Always `null` today; reserved for per-facet count enrichment. Matches `list_model_facets.owners[*].count` convention. |
| `auto_bi_providers` | *(absent)* | — | ❌ | — | Class C (Platform-only): no `auto_bi_provider` column in `dbt.exposures.parquet`. Filter is feature-gated in dbt-ui behind `explorerAutoExposures`. See Design notes #3. |
| `exposure_modes` | *(absent)* | — | ❌ | — | Class C (Platform-only): no `definition_type` column in `dbt.exposures.parquet`. Filter is feature-gated. |
| `exposure_types` | *(absent)* | — | ❌ | — | Not exposed as a filter in `ExposureFilterView`; the FE has no Type dropdown today. If a Type filter is added later, this facet becomes additive — the values are known statics (`dashboard`, `notebook`, `analysis`, `ml`, `application`) and don't require a parquet query. |
| `maturities` | *(absent)* | — | ❌ | — | Not exposed as a filter in `ExposureFilterView`; the FE has no Maturity dropdown today. Same reasoning as `exposure_types`. |
| `tags` | *(absent)* | — | ❌ | — | Not exposed as a filter in `ExposureFilterView` today. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface ExposureFacetsResponse {
  owners: FacetValue[];
}

// FacetValue is shared with ModelFacetsResponse
interface FacetValue {
  value: string;
  count: number | null;
}
```

### Risk register

1. **Owner source is `dbt.exposures`, not `dbt.groups`.** The `list_model_facets`
   handler sources owners from `dbt.groups.name` because models inherit owners
   via their dbt group. Exposures declare their owner directly in YAML
   (`owners.name` / `owners.email`) — there is no group indirection. The
   facets SQL is `SELECT DISTINCT owner_name AS value FROM dbt.exposures WHERE
   owner_name IS NOT NULL ORDER BY owner_name`. Do not copy the model handler's
   `dbt.groups` query verbatim.

2. **Owner-name collisions across resource types.** A user named `"Jane Doe"`
   may appear as both a `dbt.groups.name` (model owner) and a
   `dbt.exposures.owner_name` (exposure owner). The two facets endpoints
   (`/api/v1/models/facets` and `/api/v1/exposures/facets`) return owner lists
   from different tables and may differ — this is correct and intentional. FE
   engineers should not assume a global owners catalog.

3. **Empty project handling.** When `dbt.exposures.parquet` has zero rows, the
   handler must return `{ "owners": [] }` — not an error and not a 404.

4. **`count` enrichment deferred consistently.** Today the only producer of
   facet `count` data is "none." If `list_model_facets` ever starts populating
   `count`, this endpoint should follow the same pattern at the same time — do
   not asymmetrically populate one and not the other. Coordinate before
   shipping.

5. **No feature-gated filter facets.** The Cloud-only `auto_bi_providers` and
   `exposure_modes` filters in `ExposureFilterView` are *not* stubbed with 412
   `upgrade_path: "platform"` because the underlying signal (the
   `auto_bi_provider` and `definition_type` columns) does not exist on any
   parquet row to gate on. Stubbing 412 on a facets endpoint that has no
   parquet column would be inventing a capability gate where there is no
   surface — a Class B treatment, not Class C. See Design notes #3.

6. **`dbt.exposures.parquet` schema empirically verified.** Schema confirmed
   via `pq.read_table(...).schema` on the reference index at
   `~/codaz/sl-schema-evolution/sample_project/target/index/dbt.exposures.parquet`:
   `unique_id, name, exposure_type, label, owner_name, owner_email, url,
   maturity, description, package_name, file_path, original_file_path, fqn,
   depends_on_nodes, depends_on_macros, refs, sources, metrics, tags, meta,
   config, created_at, ingested_at`. No `auto_bi_provider`, no `integration_id`,
   no `definition_type` column.

7. **No `has_*` capability gate is introduced.** The capability flags listed in
   `API-CONTRACTS.md` § "Backend conventions" are vestigial for definition-only
   resources per the parent doctrine — exposures have no execution-, catalog-,
   or freshness-gated surface, and the facets endpoint has no other conditional
   surface either. Do not invent a new `has_*` flag for this contract.

---

## Design notes

These notes flag the three deviations from the "uniform LIST" template that
groups specifically requires. Coordinator review should focus on these; the
field reference table below assumes each is settled the way it's stated here.

1. **The dbt-ui hook is plural-lookup-by-ID, not a paginated LIST.**
   `useGroupsByIds` in `definitionGroups.ts` takes a `uniqueIds: string[]`
   filter and runs a GraphQL `groups(first, after, filter: { uniqueIds })`
   query. This is how the dbt-ui `GroupView` reaches the surrounding-groups
   table from a model page — the consumer arrives already knowing which group
   IDs it wants, not asking "give me all groups paginated."

   **The REST `GET /api/v1/groups` endpoint paginates the FULL set of groups
   in `dbt.groups.parquet`,** not a caller-supplied ID list. The by-IDs
   pattern is a dbt-ui artifact of how groups are reached from a model page,
   not the canonical LIST shape. A caller that already has a set of group IDs
   should hit `GET /api/v1/groups/:id` once per ID; v0 does not add an
   `?ids=` query parameter to LIST. If a real use case for batch-by-ID
   surfaces, a dedicated `POST /api/v1/groups/batch` (request body carries
   the ID list, no URL-length limit) is the right shape — additive, not a
   refactor of LIST.

2. **`GroupDetail` composes `NodeBase` directly (not `DefinitionNodeBase`)
   because `dbt.groups` has no `fqn` column.** Per ADR-5, the LIST row
   reflects this: no `fqn`, no `execution_info`, no `tags` at the top level
   (tags live in the `config` JSON blob — see the detail contract for the
   `json_extract` path and Risk #3 in the detail contract). Groups are the
   one ADR-5 resource type that lacks `fqn`; every other definition-only
   resource (`exposure`, `macro`, `metric`, `saved_query`, `semantic_model`)
   composes `DefinitionNodeBase` which carries `fqn`.

3. **`model_count` is a JOIN-aggregated field, not a parquet column.**
   `dbt.groups.parquet` has no `model_count` column. The handler must
   aggregate against `dbt.nodes` per row:

   ```sql
   SELECT g.unique_id, g.name, g.owner_name, g.owner_email,
          (SELECT COUNT(*) FROM dbt.nodes n
           WHERE n.group_name = g.name
             AND n.package_name = g.package_name
             AND n.resource_type = 'model') AS model_count
   FROM dbt.groups g
   ORDER BY g.name ASC
   LIMIT :limit OFFSET :offset
   ```

   The JOIN is scoped by `package_name` as well as `name` to prevent
   cross-package collisions — same rule documented in the groups detail
   contract Risk #4. The aggregation is marked 🔧 in the field reference
   (needs handler-side SQL change).

4. **No filters, no sorts, no facets.** The dbt-ui `GroupFilterView` exposes
   no filter pills and no sortable columns — it renders a static table sorted
   client-side. The REST endpoint accepts `?sort=name:<asc|desc>` for API
   uniformity (every LIST endpoint accepts a `sort` parameter) but does not
   add custom filter parameters. `GET /api/v1/groups/facets` returns `{}` —
   the endpoint exists for API uniformity (every LIST endpoint has a
   matching FACETS endpoint) so client codegen can treat all resource types
   identically.

---

## `GET /api/v1/groups`

Powers: `GroupFilterView` in dbt-ui.

dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/components/FilterPages/GroupFilterView.tsx`

GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/discovery/definitionGroups.ts` (`useGroupsByIds` / `useGroups` — `GetGroupsByUniqueId` query)

**No new ADR needed.** This endpoint follows ADR-1 (type-specific), ADR-5
(definition-only — no `execution_info`, no `fqn`), and ADR-6 (envelope
shape). See Design notes above for the three group-specific deviations from
the uniform LIST template.

### Query parameters

| Parameter | Type | Default | Notes |
|---|---|---|---|
| `first` | `u32` | `100` (max `1000`) | Per-page row count. Server clamps. Clamped to `[1, 5000]`; mirrors `GET /api/v1/models` |
| `after` | `string` (opaque base64) | — | Cursor returned as `page_info.end_cursor` on the previous page. Omit for the first page. Tampering yields 400. |
| `sort` | `string` | `name:asc` | `<column>:<asc\|desc>`; only `name` is sortable. `400 Bad Request` for unknown columns or directions |

No filter parameters in v0. The dbt-ui `GroupFilterView` exposes no filter
pills; if a real filter need surfaces (e.g., "groups in package X"), add
a `?package_name=<csv>` parameter via the established `csv → IN (…)` pattern
documented for `GET /api/v1/models`.

### Example response

Fields marked `// 🔧` are not yet returned — they require a backend change
(no group LIST handler exists today).
Fields marked `// 🔍` are parquet-unverified — confirm schema before implementing.

```json
{
  "data": [
    {
      "unique_id": "group.jaffle_shop.finance",
      "name": "finance",
      "owner_name": "Finance Data Team",
      "owner_email": "finance-data@jaffle.example",
      "owner_github": "jaffle/finance-data-team",
      "owner_slack": "#finance-data",
      "model_count": 12
    },
    {
      "unique_id": "group.jaffle_shop.marketing",
      "name": "marketing",
      "owner_name": "Marketing Analytics",
      "owner_email": "marketing-data@jaffle.example",
      "owner_github": null,
      "owner_slack": null,
      "model_count": 5
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiPHN0YXJ0X3NvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "end_cursor": "eyJzIjoiPHNvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "has_next_page": false
  }
}
```

`owner_github` and `owner_slack` are likely sourced from a `json_extract`
against the `dbt.groups.config` JSON column (verified empirically: the
parquet schema has only `owner_name` and `owner_email` as dedicated owner
columns; see the detail contract Risk #1 for the resolved verification
trail). If the JSON omits them, ship `null`. The frontend renders gracefully
either way.

`model_count` is the count of `dbt.nodes` rows where
`group_name = g.name AND package_name = g.package_name AND resource_type = 'model'`.
It is **not** a column in `dbt.groups.parquet`; see Design note #3.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

No handler exists today — every populated field is at minimum 🔧.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"group.jaffle_shop.finance"` — primary key in `dbt.groups` |
| `name` | `string` | Core | 🔧 | — | Group name (e.g., `"finance"`) |
| `owner_name` | `string \| null` | Core | 🔧 | — | From `dbt.groups.owner_name` |
| `owner_email` | `string \| null` | Core | 🔧 | — | From `dbt.groups.owner_email` |
| `owner_github` | `string \| null` | Core | 🔍 | — | Empirically confirmed absent at the top level of `dbt.groups.parquet`; handler must `json_extract_string(config, '$.owner.github')` and emit `null` on absence — see detail contract Risk #1 |
| `owner_slack` | `string \| null` | Core | 🔍 | — | Same provenance as `owner_github` — `json_extract_string(config, '$.owner.slack')` — see detail contract Risk #1 |
| `model_count` | `number` | Core | 🔧 | — | Aggregated via `SELECT COUNT(*) FROM dbt.nodes WHERE group_name = g.name AND package_name = g.package_name AND resource_type = 'model'`; not a parquet column on `dbt.groups` — see Design note #3 |
| `resource_type` | *(absent)* | — | ❌ | — | LIST row omits the discriminator — the endpoint name already constrains the type. Detail responses keep `resource_type` for ADR-1's deferred generic dispatcher. |
| `package_name` | *(absent)* | — | ❌ | — | Out of scope for the summary row; consumers wanting it call `GET /api/v1/groups/:id` |
| `description` | *(absent)* | — | ❌ | — | Same — kept off the summary row to bound payload size; columns shown in `GroupFilterView` are only Name / Model count / Owner * |
| `original_file_path` | *(absent)* | — | ❌ | — | Same — detail-only |
| `tags` | *(absent)* | — | ❌ | — | Same — detail-only; also lives in `config` JSON blob (no top-level column) |
| `meta` | *(absent)* | — | ❌ | — | Same — detail-only; lives in `config` JSON blob |
| `models` | *(absent)* | — | ❌ | — | Inline member list belongs on detail responses, not list rows; `model_count` scalar covers the summary use case |
| `fqn` | *(absent)* | — | ❌ | — | Per ADR-5, groups are the one definition-only resource without an `fqn` column in parquet — `GroupDetail` composes `NodeBase` directly (not `DefinitionNodeBase`) for this reason. See Design note #2. |
| `execution_info` | *(absent)* | — | ❌ | — | Groups never run — definition-only per ADR-5. Field omitted from both `DefinitionNodeBase` and the LIST row entirely. |
| `catalog` | *(absent)* | — | ❌ | — | No warehouse relation; nothing to catalog |
| `freshness` | *(absent)* | — | ❌ | — | No source semantics |
| `ingested_at` | *(absent)* | — | ❌ | — | Detail-only timestamp; not a column shown in `GroupFilterView` |
| `created_at` | *(absent)* | — | ❌ | — | `dbt.groups` has no `created_at` column; groups use `ingested_at` as the ADR-5 fallback on the detail response. Not a column shown in `GroupFilterView`. |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api |
| `project_id` | *(absent)* | — | ❌ | — | Class B: Cloud concept; not in local parquet |
| `last_updated_at` | *(absent)* | — | ❌ | — | Class B: Cloud-managed environment timestamp; the parquet has `ingested_at` which is server-local, not semantically equivalent |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface GroupListResponse {
  data: GroupSummary[];
  page_info: PageInfo;
}

interface GroupSummary {
  unique_id: string;
  name: string;
  owner_name: string | null;
  owner_email: string | null;
  owner_github: string | null;
  owner_slack: string | null;
  model_count: number;
}
// PageInfo: see ADR-6 § "Shared TypeScript types" for the canonical declaration.

```

### Risk register

1. **`owner_github` and `owner_slack` provenance is `config` JSON, not a
   parquet column.** Empirically confirmed against
   `sl-schema-evolution/sample_project/target/index/dbt.groups.parquet`: the
   schema has only `owner_name` and `owner_email` as dedicated owner columns.
   Handler must `json_extract_string(config, '$.owner.github')` /
   `'$.owner.slack'` and emit `null` on absence. The sample project's two
   rows both have `null` for these fields, so the integration test cannot
   round-trip verify against this corpus today — the test materializes when
   a real project surfaces populated data. Same posture as the detail
   contract Risk #1: ship Core-stable with these JSON paths as the starting
   recommendation; fix forward in a follow-up if a real project uses a
   different JSON shape. No dbt-index schema change required.

2. **`model_count` requires a per-row aggregation.** The naive SQL is the
   correlated subquery shown above. A `LEFT JOIN ... GROUP BY` formulation
   is equivalent and may be cheaper at scale; the implementer should pick
   the form that nextest profiles best against a realistic project. Either
   way, the JOIN must be scoped by `package_name` AND `name` — `dbt.nodes.group_name`
   stores the group **name** (not the full `unique_id`), so a single-column
   join risks cross-package collisions; same rule as the detail contract
   Risk #4. Verified safe for the two-row sample project; revisit if a
   multi-package project surfaces a case where this is wrong.

3. **No filter parameters in v0 — but LIST must still accept `?sort`.**
   `GroupFilterView` is filter-less and effectively sort-less (it sorts
   client-side after fetching). Server-side sorting on `name` is supported
   for API uniformity; other columns are unsorted in v0. If `model_count`
   becomes a sort target (a reasonable product ask), add it to the allowlist
   alongside the aggregation column alias in the SQL ORDER BY — same pattern
   as `executed_at` on `GET /api/v1/models`.

4. **LIST row omits `description`, `original_file_path`, and `tags` —
   intentional trade-off.** Mirrors `ModelSummary` precedent: the summary
   row carries only what `GroupFilterView` actually displays (Name, Model
   count, Owner *). Consumers wanting the full surface call
   `GET /api/v1/groups/:id`. Promoting these to the summary would bloat the
   payload for a 200-group response with fields the FE table doesn't render.

5. **Default `limit` of 1000 may exceed typical group counts by 10–100×.**
   `ModelSummary` uses `DEFAULT_LIMIT = 1000`. Groups are an order of
   magnitude rarer than models (a project with 500 models typically has
   <30 groups). The default is intentionally generous to avoid pagination
   for any realistic project; if memory profiling surfaces a concern, lower
   the default to 100 (matching dbt-ui's `INITIAL_PAGE_SIZE`) — the
   parameter is client-tunable either way.

---

## `GET /api/v1/groups/facets`

Powers: filter pill metadata for `GroupFilterView` in dbt-ui — but
`GroupFilterView` has no filter pills today. The endpoint exists for API
uniformity (every LIST endpoint has a matching FACETS endpoint) so client
codegen can treat all resource types identically.

### Query parameters

None. The endpoint takes no query parameters and ignores any provided.

### Example response

```json
{}
```

The empty-object response is canonical and stable — adding a filter to the
groups LIST in the future would add a new key to this object (e.g.,
`{ "package_names": [...] }`) without breaking existing consumers, who
already iterate `Object.entries(facets)` per the uniform codegen pattern.

### Field reference

No populated fields in v0. The table below documents the empty-object
contract explicitly so reviewers don't infer "the endpoint is unfinished."

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| *(empty object)* | `{}` | Core | 🔧 | — | No filter parameters exist on `GET /api/v1/groups` in v0, so the facets response is the empty object. Endpoint is implemented for API uniformity — `GroupFilterView` does not call it today. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
// v0: no filterable fields → empty object.
// Adding a future filter (e.g., `package_name`) would extend this type
// additively: { package_names: FacetValue[] }.
type GroupFacetsResponse = Record<string, never>;

// FacetValue is shared with other LIST FACETS responses (ModelFacetsResponse, etc.)
interface FacetValue {
  value: string;
  count: number | null;
}
```

### Risk register

1. **Empty-object response surface is intentional, not a stub.** Reviewers
   should not flag the `{}` response as "the endpoint isn't implemented."
   It is the canonical response for a resource type that has no filterable
   fields. If a filter is added later, the response gains a key — that's an
   additive change, not a breaking one.

2. **Client codegen must tolerate empty `{}` from the FACETS endpoint.**
   The TypeScript `Record<string, never>` type encodes this; OpenAPI codegen
   should emit an empty object schema (`type: object`, no `properties`,
   `additionalProperties: false`). Test that `Object.keys(facets).length`
   is safe to call against this response shape.

3. **Do not introduce an `owners` facet by inferring it from `owner_name`.**
   `GET /api/v1/models/facets` exposes an `owners` facet sourced from
   `dbt.groups.name` (the model-to-group ownership relationship). The
   groups LIST is not gated by owner — every group *is* an owner — so
   re-exposing `owners` here would be a category error. The list of owner
   names is `data[*].owner_name` from the LIST endpoint itself; no facet
   needed.

---

## `GET /api/v1/macros`

Paginated, filterable list of macro definitions. Powers the table on the dbt-ui
macros filter page.

### Query parameters

| Param | Type | Default | Notes |
|---|---|---|---|
| `package` | `string` | *(none)* | Exact-match package name filter. Mirrors `MacroDefinitionFilter.packageName` in the GraphQL hook and the single-select `PackageFilterDropdown` in `MacroFilterView`. Empty string → no filter (treated as absent). |
| `sort` | `string` | `name:asc` | `<column>:<asc\|desc>`. Allowlisted column: `name`. The dbt-ui table exposes no sort headers (`MacroFilterView` columns are render-only), so `name` is the only sortable column for v0. Invalid column → 400. |
| `first` | `u32` | `100` (max `1000`) | Per-page row count. Server clamps. Clamped to `[1, 5000]` (same envelope as `GET /api/v1/models`). |
| `after` | `string` (opaque base64) | — | Cursor returned as `page_info.end_cursor` on the previous page. Omit for the first page. Tampering yields 400. |
Per ADR-6 the LIST endpoint uses cursor pagination (`first`/`after` + `page_info`) for
v0; cursor pagination (`?first=&after=` per CC-4) is deferred until a real
consumer hits the offset-pagination ceiling. The macros parquet in the sample
project is 671 rows, well inside the `HARD_MAX_LIMIT=5000` envelope.

### Example response

Fields marked `// 🔧` are not yet returned — no list handler exists today; every
field on the row is 🔧 (or 🔍 where parquet shape is unverified).

```json
{
  "data": [
    {
      "unique_id": "macro.jaffle_shop.cents_to_dollars",
      "name": "cents_to_dollars",
      "package_name": "jaffle_shop",
      "description": "Convert an integer cents column to a dollar-denominated decimal.",
      "arguments": [
        { "name": "column_name", "type": "string",  "description": "The integer column holding cent values." },
        { "name": "scale",       "type": "integer", "description": "Decimal scale to round the output to." }
      ]
    },
    {
      "unique_id": "macro.dbt_utils.surrogate_key",
      "name": "surrogate_key",
      "package_name": "dbt_utils",
      "description": null,
      "arguments": []
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiPHN0YXJ0X3NvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "end_cursor": "eyJzIjoiPHNvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "has_next_page": false
  }
}
```

No capability gates apply to any field on this response — every included field
is Core (parquet-backed, unconditional). No `execution_info` block exists on
macro rows per ADR-5. `tags` is absent (Class B — see Field reference).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

No handler exists for `GET /api/v1/macros` today; every included field is 🔧 (or 🔍).

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"macro.jaffle_shop.cents_to_dollars"` — from `dbt.macros.unique_id`. Required, non-null in parquet schema (`[utf8!]`). |
| `name` | `string` | Core | 🔧 | — | From `dbt.macros.name`. The dbt-ui table renders this as the link cell; `parseUniqueId(macro.uniqueId).remainder` derives an equivalent value in the frontend today. |
| `package_name` | `string \| null` | Core | 🔧 | — | From `dbt.macros.package_name`. Parquet column is `[utf8]` (nullable); in practice every macro carries a package but the wire type honors the schema. |
| `description` | `string \| null` | Core | 🔧 | — | From `dbt.macros.description`. Null when the YAML schema patch declares no description; the dbt-ui Description column renders `null` as an empty cell. |
| `arguments` | `MacroArgument[]` | Core | 🔍 | — | From `dbt.macros.arguments` (`[utf8] Option<String>`, JSON-string). Handler must deserialize via the shared `json_parse_or_null` helper per CC-7. Empty array when no arguments are declared OR when the JSON parse fails (warning emitted; never bubbles to the client). Re-emitted as `[{name, type, description}, …]` to match the detail-endpoint shape. The dbt-ui list view flattens this to a CSV of `name` values, but the wire shape preserves the nested objects so the FE can stop flattening when convenient. |
| `arguments[*].name` | `string` | Core | 🔍 | — | Required on each entry; argument JSON objects missing `name` are filtered out by the handler (logged, not erroring). |
| `arguments[*].type` | `string \| null` | Core | 🔍 | — | Declared Jinja type hint (e.g., `"string"`, `"integer"`); free-form, not validated. |
| `arguments[*].description` | `string \| null` | Core | 🔍 | — | Per-argument description from the YAML schema patch. |
| `tags` | *(absent)* | — | ❌ | — | Class B for macros: `dbt.macros` parquet has no `tags` column (empirically confirmed against 671-row sample project — see Risk #3 on `GET /api/v1/macros/:id`). GraphQL surfaces `tags` but it is manifest-only metadata that codex-api persists separately. Documented here so FE engineers don't add an optional `tags?` to the list-row TypeScript type. |
| `group_name` | *(absent)* | — | ❌ | — | Class B for macros: `dbt.macros` parquet has no `group_name` column. Macros are not group-owned in the dbt grouping model; the `MacroFilterView` does not render an Owner column. |
| `meta` | *(absent)* | — | ❌ | — | Available on the detail endpoint (`GET /api/v1/macros/:id`). Omitted from the list row to keep the row small; no `MacroFilterView` column renders meta. Promote to the list row only if a future filter/column needs it. |
| `macro_sql` | *(absent)* | — | ❌ | — | Available on the detail endpoint. List rows would render hundreds of KB of Jinja source if this were inlined; FE never displays it in the list view. |
| `file_path` / `original_file_path` / `patch_path` | *(absent)* | — | ❌ | — | Available on the detail endpoint. The list view renders no path information. |
| `docs_show` / `supported_languages` | *(absent)* | — | ❌ | — | Available on the detail endpoint. Filtering by `docs_show=false` is not exposed in `MacroFilterView`; if/when an "include internal helpers" toggle is added, promote `docs_show` to the row. |
| `created_at` | *(absent)* | — | ❌ | — | Available on the detail endpoint per ADR-5 ("Definition updated as of …"). The list view shows no per-row timestamp; the parent `dbt.macros.parquet` ingest time is exposed via the global `lastUpdatedAt` plumbing (handled by `useCacheMonitor`), not per-row. |
| `depends_on` / `referenced_by` | *(absent)* | — | ❌ | — | Class A but list-inappropriate: macro edges are inlined on the detail endpoint (see `GET /api/v1/macros/:id` contract). Inlining them on every list row would amplify the response size by O(edges) per row with no `MacroFilterView` consumer. |
| `execution_info` | *(absent)* | — | ❌ | — | Per ADR-5 macros are not runnable; the field is omitted from `DefinitionNodeBase` entirely. This row is documentation only. |
| `run_id` / `project_id` | *(absent)* | — | ❌ | — | Class B: CodexDB-only identifiers; no parquet path. GraphQL `runId` exposed on the macro node is dropped from the REST surface. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface MacroListResponse {
  data: MacroSummary[];
  page_info: PageInfo;
}

interface MacroSummary {
  unique_id: string;
  name: string;
  package_name: string | null;
  description: string | null;
  arguments: MacroArgument[];
}

// Structurally identical to the MacroArgument on MacroDetail. Kept as a single
// shared TS type at codegen time; duplicated in the contract here for clarity.
interface MacroArgument {
  name: string;
  type: string | null;
  description: string | null;
}
// PageInfo: see ADR-6 § "Shared TypeScript types" for the canonical declaration.

```

### Risk register

1. **`arguments` JSON parse is per-row, not free.** `dbt.macros.arguments` is stored as a serialized JSON string per CC-7. For a 1000-row page the handler invokes `json_parse_or_null` 1000 times. Profile against the sample project after the first cut; if parse cost dominates, an alternative is to defer `arguments` to a sub-resource (`GET /api/v1/macros/:id/arguments`) and drop the field from the list row — but the `MacroFilterView` already needs argument names in the Arguments column, so that would cost a round trip per row in the worst case. Decision: ship inlined for v0; revisit only with a profiling-backed regression.

2. **No sort surface despite `sort=name:asc` default.** The dbt-ui `MacroFilterView` does not render sortable column headers. The contract still accepts `?sort=name:asc|desc` so that the handler shares the same `parse_sort` plumbing as `GET /api/v1/models` and so that an MCP/AI consumer (or a future FE toggle) can sort without a contract change. Promoting additional columns to the allowlist (e.g., `package_name`, `created_at`) is additive and does not require a new ADR.

3. **`package` filter is exact-match, single-value.** The dbt-ui `PackageFilterDropdown` is a single-select widget keyed on `PACKAGE_NAME_PARAM`, so the REST contract mirrors that (no CSV; no `IN (…)` semantics). Promoting to CSV (`?package=jaffle_shop,dbt_utils`) is additive and can be done without breaking existing callers — the parser would treat a single value as a one-element list. Deferred until a multi-select UX lands.

4. **`description` from `dbt.macros.parquet` is `[utf8]` non-Option in the Rust schema.** Per `MacroRow` definition (`crates/dbt-index/src/parquet.rs` line 1136) `description: &'a str` is declared without `Option`, but the empirical 671-row sample includes macros with no description. The parquet column itself is nullable at the Arrow level (the Rust binding owns the conversion); the handler must treat the Arrow column as `StringArray` with `is_null` checks (the same pattern as `batches_to_model_rows`). The wire type is `string \| null` — confirm against a fresh ingest before committing the handler.

5. **`MacroDefinitionFilter` GraphQL surface includes `uniqueIds` (multi-ID lookup).** `useDiscoveryDefinitionMacros` accepts `filter.uniqueIds` and pauses the query when `uniqueIds?.length === 0` — used elsewhere in the dbt-ui for "give me these specific macros" lookups (not for the filter page). Omitted from the REST list contract for v0 because `MacroFilterView` does not exercise it. If a future caller needs it, add `?unique_id=` (CSV) as an additive filter; do not retrofit a `POST /api/v1/macros/batch` endpoint.

6. **Empty-string vs absent `package` query param.** The handler must treat `?package=` (empty value) as absent, not as "package_name = ''", consistent with how `MacroFilterView` handles `ALL_PACKAGES_VALUE` — the dropdown's "All packages" sentinel is mapped to `undefined` in the hook (`qsPackageName === ALL_PACKAGES_VALUE ? undefined : qsPackageName`). Confirm the filter handler short-circuits on `Some("")` the same way `build_list_sql` in `models.rs` already does for its filters (`.filter(|s| !s.is_empty())`).

---

## `GET /api/v1/macros/facets`

All filter facet values for the macros list. Single facet key per the
`MacroFilterView` UX: `packages`.

### Query parameters

None. Facet values are static for the lifetime of an index snapshot; per ADR-6
the facets endpoint takes no query parameters and returns the full distinct set.

### Example response

```json
{
  "packages": [
    { "value": "dbt",          "count": null },
    { "value": "dbt_utils",    "count": null },
    { "value": "jaffle_shop",  "count": null }
  ]
}
```

`count` is `null` in v0 — reserved for a future enhancement that returns the
number of macros per package without forcing the client to issue a follow-up
list query for each facet value (same convention as `FacetValue` in
`/api/v1/models/facets`).

If the parquet contains no macros (e.g., a fresh project), the response is
`{ "packages": [] }` — never `{}`. The empty-array shape matches the ADR-6
guidance that absent facets return `{}` only when the facet key itself is
inapplicable; for the macros list `packages` is always applicable.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

No handler exists for `GET /api/v1/macros/facets` today; every field is 🔧.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `packages` | `FacetValue[]` | Core | 🔧 | — | Distinct package names from `dbt.macros.package_name`, sorted ascending. See Risk #2 for the `dbt.packages` join decision. |
| `packages[*].value` | `string` | Core | 🔧 | — | The package name; matches the `?package=` query parameter on the list endpoint exactly (no transformation). |
| `packages[*].count` | `number \| null` | Core | 🔧 | — | Always `null` in v0; reserved for future "macros per package" counts. Shape mirrors `FacetValue` on `/api/v1/models/facets`. |
| `tags` | *(absent)* | — | ❌ | — | No tags facet — `dbt.macros` has no `tags` column (Class B, see list endpoint Field reference). |
| `owners` | *(absent)* | — | ❌ | — | No owners facet — macros are not group-owned in the dbt model. |

### Type definition

```typescript
interface MacroFacetsResponse {
  packages: FacetValue[];
}

// Structurally identical to the FacetValue used on /api/v1/models/facets.
interface FacetValue {
  value: string;
  count: number | null;
}
```

### Risk register

1. **No handler exists yet.** Net-new endpoint; SQL is a one-line `SELECT DISTINCT package_name FROM dbt.macros WHERE package_name IS NOT NULL ORDER BY package_name`. The handler should follow the `list_model_facets` pattern in `src/handlers/models.rs` (spawn-blocking + `query_arrow` + `batches_to_*_names` extractor). A new `MacroFacetsResponse` struct lives in `src/handlers/macros.rs` (net-new file alongside the macros detail handler — see Risk #1 on the detail contract).

2. **`dbt.packages` join is optional for v0.** The GraphQL `appliedPackageNames` hook returns *all* packages installed in the project (filtered by `PackageResourceType.Macro`), which is conceptually `dbt.packages` filtered to packages that ship macros — equivalent to `SELECT DISTINCT package_name FROM dbt.macros` for any project where every installed package ships at least one macro (which is the common case: `dbt_utils`, `dbt`, etc. all ship macros). Empirically the difference is zero in the sample project. Decision for v0: query `dbt.macros` only — simpler, fewer joins, no risk of stale `dbt.packages` rows surfacing as facet values for packages whose macros were never indexed. If a corner case emerges where a package ships *only* non-macro resources but the FE still wants to expose it as a filter option, switch to `SELECT DISTINCT p.package_name FROM dbt.packages p JOIN dbt.macros m ON m.package_name = p.package_name`.

3. **Sort order is alphabetical, fixed.** No `?sort=` parameter on facets. Mirrors `list_model_facets`, which hardcodes the order of `modeling_layers` (convention) and `owners` (alphabetical from SQL `ORDER BY`). Document explicitly so FE engineers don't expect the facet endpoint to honor the same `?sort=` plumbing as the list endpoint.

4. **`count: null` is a deliberate forward-compatibility hook.** Returning the field with a `null` value (rather than omitting it) preserves the wire shape if/when counts are populated in a future revision. Removing the field later would be a breaking change; keeping it as `null` today costs nothing.

5. **Empty parquet case.** When `dbt.macros.parquet` has zero rows (fresh project, never `dbt parse`'d), the SELECT DISTINCT returns zero rows. Handler must emit `{ "packages": [] }`, not `{}` and not 404 — the endpoint succeeded; the facet just has no values. Confirm against the `query_arrow` empty-batch path; `batches_to_owner_names` already handles this for the models facets endpoint.

6. **No capability flag introduced.** `dbt.macros.parquet` is always present when the index exists (it's emitted by `dbt parse` unconditionally, not by `dbt build` / `dbt docs generate`). The endpoint does not need a `has_*` gate.

---

## `GET /api/v1/metrics`

Paginated, non-filterable, non-sortable list of metric definitions. Powers
the table on the dbt-ui metrics filter page.

### Query parameters

| Param | Type | Default | Notes |
|---|---|---|---|
| `sort` | `string` | `name:asc` | `<column>:<asc\|desc>`. Allowlisted column: `name`. The dbt-ui `MetricFilterView` exposes no sortable column headers; `name` is accepted so the handler can share the same `parse_sort` plumbing as `GET /api/v1/models` and so that an MCP/AI consumer (or a future FE toggle) can sort without a contract change. Invalid column → 400. |
| `first` | `u32` | `100` (max `1000`) | Per-page row count. Server clamps. Clamped to `[1, 5000]` (same envelope as `GET /api/v1/models` and `GET /api/v1/macros`). |
| `after` | `string` (opaque base64) | — | Cursor returned as `page_info.end_cursor` on the previous page. Omit for the first page. Tampering yields 400. |
No filter query parameters. The `MetricFilterView` component renders no
filter widgets (no package dropdown, no type dropdown, no owner dropdown)
— mirroring that, the REST contract exposes zero filter params for v0.
Promoting `package`, `metric_type`, `group_name`, or `tag` to filterable
parameters is additive and does not require a new ADR; flip the
corresponding facets row to populated values on the facets endpoint at the
same time.

Per ADR-6 the LIST endpoint uses cursor pagination (`first`/`after` + `page_info`)
for v0; cursor pagination (`?first=&after=` per CC-4) is deferred until a
real consumer hits the offset-pagination ceiling. The metrics parquet in
the sample project is 43 rows — well inside the `HARD_MAX_LIMIT=5000`
envelope.

### Example response

Fields marked `// 🔧` are not yet returned — no list handler exists today;
every field on the row is 🔧 (or 🔍 where parquet shape is unverified).
Nested JSON-string columns (`type_params`, `filter`, `meta`) deserialize
handler-side per CC-7.

```json
{
  "data": [
    {
      "unique_id": "metric.jaffle_shop.total_revenue",
      "name": "total_revenue",
      "package_name": "jaffle_shop",
      "group_name": "finance",
      "metric_type": "simple",
      "semantic_model_name": "orders",
      "tags": ["finance"],
      "description": "Sum of order amounts across all completed orders.",
      "input_metric_names": [],
      "input_metric_names_truncated": false,
      "created_at": 1747432300.5
    },
    {
      "unique_id": "metric.jaffle_shop.gross_margin",
      "name": "gross_margin",
      "package_name": "jaffle_shop",
      "group_name": "finance",
      "metric_type": "derived",
      "semantic_model_name": null,
      "tags": ["finance"],
      "description": "(revenue - cogs) / revenue.",
      "input_metric_names": ["total_revenue", "total_cogs"],
      "input_metric_names_truncated": false,
      "created_at": 1747432308.1
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiPHN0YXJ0X3NvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "end_cursor": "eyJzIjoiPHNvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "has_next_page": false
  }
}
```

No capability gates apply to any field on this response — every included
field is Core (parquet-backed, unconditional). No `execution_info` block
exists on metric rows per ADR-5. `catalog`, `columns`, `materialized`,
`relation_name`, `database_name`, `schema_name`, `identifier`,
`access_level`, `contract_enforced`, `raw_code`, `compiled_code` are all
absent (Class B for metrics — see Field reference and the detail-endpoint
contract for the per-field rationale).

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

No handler exists for `GET /api/v1/metrics` today; every included field is 🔧 (or 🔍).

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"metric.jaffle_shop.total_revenue"` — from `dbt.metrics.unique_id`. Required, non-null in parquet schema (`[utf8!]`). |
| `name` | `string` | Core | 🔧 | — | From `dbt.metrics.name`. Renders as the link cell in the dbt-ui table; `parseUniqueId(metric.uniqueId).remainder` derives an equivalent value in the frontend today. |
| `package_name` | `string \| null` | Core | 🔧 | — | From `dbt.metrics.package_name`. Parquet column is `[utf8]` (nullable). |
| `group_name` | `string \| null` | Core | 🔧 | — | From `dbt.metrics.group_name` (= manifest `group`). `null` when ungrouped. |
| `metric_type` | `string \| null` | Core | 🔧 | — | `"simple"` · `"ratio"` · `"derived"` · `"cumulative"` · `"conversion"`; from `dbt.metrics.metric_type` (= manifest `type`). Empirically verified against `dbt.metrics.parquet` in `sl-schema-evolution/sample_project` (all 5 values observed). Renders in the dbt-ui Type column today (`TableData.type` in `MetricFilterView`). |
| `semantic_model_name` | `string \| null` | Core | 🔧 | — | From `dbt.metrics.semantic_model_name`. Denormalized from `type_params.metric_aggregation_params.semantic_model` at index-build time (`build_metric_row` in `crates/dbt-index/src/parquet.rs`); `null` for `ratio`/`derived` types whose `type_params` references other metrics rather than a semantic model. |
| `tags` | `string[]` | Core | 🔧 | — | From `dbt.metrics.tags` (`[list_utf8!]` column — always present, possibly empty). Empty array when no tags declared. |
| `description` | `string \| null` | Core | 🔧 | — | From `dbt.metrics.description`. Renders in the dbt-ui Description column; `null` renders as an empty cell. |
| `input_metric_names` | `string[]` | Core | 🔧 | — | From `dbt.metrics.input_metric_names` (denormalized in `build_metric_row` from `type_params`). Names of input metrics for `ratio` (numerator/denominator) and `derived` (metrics[]) types; empty array for `simple`/`cumulative`/`conversion`. Truncated per CC-6 — see `input_metric_names_truncated` below and Risk #2. |
| `input_metric_names_truncated` | `boolean` | Core | 🔧 | — | Per CC-6: `true` when the row's `input_metric_names` array was truncated to fit the optional `?first=<n>` cap (default 500). Always `false` for v0 — metrics with >500 input metrics do not exist in practice (a `derived` metric formula spanning 500 inputs would be a different product problem), but the flag is included now so the wire shape is stable when a real cap is added. See Risk #2. |
| `created_at` | `number \| null` | Core | 🔧 | — | Epoch seconds (float); from `dbt.metrics.created_at`. Per ADR-5, this is the per-resource "Definition updated as of …" timestamp surfaced to FE consumers in lieu of `execution_info.completed_at` (metrics do not execute — see Design notes 1 and 6 on the detail-endpoint contract). Empirically verified column present in `dbt.metrics.parquet` across 43 rows in the sample project. |
| `label` | *(absent)* | — | ❌ | — | Available on the detail endpoint (`GET /api/v1/metrics/:id`). Omitted from the list row because the dbt-ui table has no "Label" column; renderers can fall back to `name` when needed. |
| `type_params` | *(absent)* | — | ❌ | — | JSON-string per CC-7. Available on the detail endpoint. Omitted from the list row because the `MetricFilterView` does not render any `type_params` field and inlining variant-shaped JSON would inflate the row payload by 5–10× for `derived`/`cumulative` metrics. Detail endpoint deserializes per CC-7 (`Record<string, unknown> \| null`). |
| `filter` | *(absent)* | — | ❌ | — | JSON-string per CC-7 (`dbt.metrics.metric_filter`). Available on the detail endpoint. Omitted from the list row — no `MetricFilterView` column renders the filter. |
| `agg_params` | *(absent)* | — | ❌ | — | No `agg_params` column on `dbt.metrics.parquet`; this field name appears in CC-7's enumeration of JSON-string columns but does not apply to the metrics row. Documented here so reviewers don't add it. |
| `validity_params` | *(absent)* | — | ❌ | — | Same as `agg_params` — not a column on `MetricRow`. Lives on `dbt.semantic_measures` (see SemanticMeasureRow) not metrics. |
| `non_additive_dimension` | *(absent)* | — | ❌ | — | Same as `agg_params` — not a column on `MetricRow`. Lives on the underlying semantic measure, not the metric. |
| `time_granularity` | *(absent)* | — | ❌ | — | Available on the detail endpoint. Omitted from the list row — no `MetricFilterView` column renders it. |
| `meta` | *(absent)* | — | ❌ | — | Available on the detail endpoint. JSON-string per CC-7 (`dbt.metrics.meta` is `Option<String>`). Omitted from the list row to keep the row small; no `MetricFilterView` column renders meta. Promote to the list row only if a future filter/column needs it. |
| `depends_on` / `referenced_by` | *(absent)* | — | ❌ | — | Class A but list-inappropriate: edges are inlined on the detail endpoint. Inlining them on every list row would amplify the response size by O(edges) per row with no `MetricFilterView` consumer. |
| `fqn` | *(absent)* | — | ❌ | — | Available on the detail endpoint. Omitted from the list row — the dbt-ui table renders `name`, not the FQN tuple. |
| `file_path` / `original_file_path` | *(absent)* | — | ❌ | — | Available on the detail endpoint. The list view renders no path information. |
| `formula` | *(absent)* | — | ❌ | — | Class B per detail Design note 5: not a column on `dbt.metrics.parquet`. For `derived` metrics the expression lives at `type_params.expr` — read it there on the detail endpoint, not on the list row. |
| `run_generated_at` | *(absent)* | — | ❌ | — | Class B per detail Design note 6: Discovery's `runGeneratedAt` has no parquet analogue under that name. `created_at` (above) replaces it for the list row's "Definition updated as of …" header semantics. |
| `patch_path` | *(absent)* | — | ❌ | — | Class B: `MetricRow` has no `patch_path` column (unlike `NodeRow`/`MacroRow`). Metrics are defined directly in YAML; `original_file_path` is the YAML file (available on the detail endpoint only). |
| `ai_context` | *(absent)* | — | ❌ | — | `dbt.metrics.ai_context` exists but is Proprietary/dbt-v2-specific; not a Discovery-public field. Defer until a UI consumer exists. |
| `config` | *(absent)* | — | ❌ | — | JSON-string per CC-7. `dbt.metrics.config` exists but has no Discovery-public schema; defer until a UI consumer exists. |
| `refs` / `sources` / `metrics` | *(absent)* | — | ❌ | — | JSON-string per CC-7. Denormalized into `depends_on` via `dbt.edges` on the detail endpoint; do not duplicate on the list row. |
| `depends_on_macros` | *(absent)* | — | ❌ | — | Denormalized into the generic `depends_on` edge view on the detail endpoint; metrics rarely reference macros directly. Not exposed on the list row. |
| `materialized` / `relation_name` / `database_name` / `schema_name` / `identifier` | *(absent)* | — | ❌ | — | Not applicable; metrics are not warehouse objects. See detail-endpoint Field reference for the per-field rationale. |
| `access_level` / `contract_enforced` | *(absent)* | — | ❌ | — | Model-only governance fields; not applicable to metrics. |
| `raw_code` / `compiled_code` | *(absent)* | — | ❌ | — | Metrics have no SQL body; closest is `type_params.expr` for derived metrics (available on the detail endpoint). |
| `columns` | *(absent)* | — | ❌ | — | Metrics expose measures/dimensions/granularity via `type_params` and the upstream `semantic_model`, not columns. See detail Design note 3. |
| `catalog` | *(absent)* | — | ❌ | — | Metrics are not warehouse relations; no `dbt.catalog_tables` row. See detail Design note 2. |
| `execution_info` | *(absent)* | — | ❌ | — | Per ADR-5 metrics are not runnable; the field is omitted from `DefinitionNodeBase` entirely. No `dbt_rt.run_results` row keyed on `metric.*`. See detail Design note 1. This row is documentation only. |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api. |
| `usage_query_count` | *(absent)* | — | ❌ | — | Class B: no parquet path; Discovery-API-internal. |
| `project_id` / `run_id` / `last_run_id` / `last_job_definition_id` | *(absent)* | — | ❌ | — | Class B: Cloud-specific identifiers; no parquet path. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface MetricListResponse {
  data: MetricSummary[];
  page_info: PageInfo;
}

interface MetricSummary {
  unique_id: string;
  name: string;
  package_name: string | null;
  group_name: string | null;
  metric_type: string | null;        // "simple" | "ratio" | "derived" | "cumulative" | "conversion"
  semantic_model_name: string | null;
  tags: string[];
  description: string | null;
  input_metric_names: string[];
  input_metric_names_truncated: boolean;
  created_at: number | null;          // ADR-5: per-resource "Definition updated as of …" timestamp; epoch seconds
}
// PageInfo: see ADR-6 § "Shared TypeScript types" for the canonical declaration.

```

### Risk register

1. **No handler exists yet.** Net-new endpoint; SQL is a straight projection of the columns above from `dbt.metrics` with `ORDER BY name ASC LIMIT … OFFSET …`. The handler should follow the `list_models` pattern in `src/handlers/models.rs` (spawn-blocking + `query_arrow` + Arrow-typed row extractor). A new `MetricListResponse` / `MetricSummary` struct lives in `src/handlers/metrics.rs` (net-new file alongside the metrics detail handler, parallel to the macros file plan in the macros list contract's Risk #1).

2. **`input_metric_names` truncation per CC-6 — flag emitted now, cap deferred.** CC-6 lets the LIST endpoint accept `?first=<n>` (default cap 500) on inline edge arrays and signal truncation per-row. `input_metric_names` is the only inline array on the metric summary that could theoretically be unbounded (a `derived` metric aggregating N input metrics). Empirically in the sample project the maximum observed `input_metric_names.len()` is 2 (for `ratio` metrics with numerator + denominator); no metric in the wild approaches the 500-row cap. Decision: emit `input_metric_names_truncated: false` unconditionally for v0 (no `?first=` query param plumbed yet); add the param when a real consumer needs it. The flag is in the wire shape now so adding the cap later is a wire-stable change.

3. **JSON-string columns are intentionally absent from the list row.** Per CC-7, `type_params`, `filter`, `meta`, `config`, `refs`, `sources`, `metrics`, and `agg_params`/`validity_params`/`non_additive_dimension` (which do not apply to `MetricRow` regardless) deserialize handler-side on the detail endpoint. They are omitted from the list row to keep the per-row payload small and to avoid duplicating the detail-endpoint's per-variant Zod consumer plumbing. If a future filter (e.g., "metrics where `type_params.expr` contains …") needs one of these, plumb it as a server-side predicate, not a wire-shape field on the list row.

4. **`metric_type` filter is not exposed despite a clear use case.** The dbt-ui `MetricFilterView` has no Type dropdown today, but the GraphQL `MetricDefinitionFilter` does expose a `metricType` filter on the underlying API. For parity with `MacroFilterView`'s `package` filter pattern, a future `?metric_type=simple,derived` (CSV → `IN (…)`) is the natural addition. Deferred to v1; flagged here so reviewers know the parameter slot is reserved and the `metric_types` facet row on the facets endpoint is the populated-shape forward hook.

5. **`created_at` precision is epoch-seconds float, not ISO 8601 string.** Same shape as the detail endpoint per ADR-5. FE consumers must format via `new Date(value * 1000).toISOString()` (or equivalent), not parse as a string. Document at the TS interface site; mismatching this on the FE produces an `Invalid Date` rather than a 500.

6. **Empty parquet case.** When `dbt.metrics.parquet` has zero rows (a project with no semantic manifest, OR an OSS Core project before SL parquet emission was wired — see retired Risk #6 on the detail contract: empirically refuted), the handler must emit `{ "data": [], "page_info": { "end_cursor": null, "has_next_page": false } }`, not 404. Mirrors the empty-facets convention on the facets endpoint below.

7. **`semantic_model_name` is denormalized at index-build time, not query time.** The handler must not attempt to re-derive it by parsing `type_params.metric_aggregation_params.semantic_model` — the parquet column is authoritative (`build_metric_row` is the only writer). If a future schema change moves the denormalization elsewhere, update the column source in one place. Documented to head off a subtle correctness regression where a handler-side re-derivation diverges from the parquet truth.

---

## `GET /api/v1/metrics/facets`

All filter facet values for the metrics list. **No facets in v0** — the
`MetricFilterView` renders no filter widgets and the contract's list-side
query parameters expose no filters. Per ADR-6, an endpoint with no
applicable facets returns the empty object.

### Query parameters

None. Facet values are static for the lifetime of an index snapshot; per
ADR-6 the facets endpoint takes no query parameters.

### Example response

```json
{}
```

Per ADR-6: an empty object means "no facets are applicable to this list
endpoint." This is distinct from `{ "metric_types": [] }`, which would mean
"the `metric_types` facet exists but the parquet contains no metrics."
Returning `{}` is a forward-compatible signal: when a future revision adds
filter parameters to the list endpoint (e.g., `?metric_type=`,
`?package=`, `?group_name=`, `?tag=`), this endpoint flips to
`{ "metric_types": […], "packages": […], "owners": […], "tags": […] }`
without a wire-shape break.

If the parquet contains no metrics (e.g., a project with no semantic
manifest), the response is still `{}` — the empty-object shape is
inapplicable-to-this-list, not empty-but-applicable.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

No handler exists for `GET /api/v1/metrics/facets` today; the entire
response body is the literal `{}`.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `metric_types` | *(absent)* | — | ❌ | — | Reserved for a future revision that adds a `?metric_type=` filter to the list endpoint (see list-endpoint Risk #4). Distinct values would be sourced from `SELECT DISTINCT metric_type FROM dbt.metrics WHERE metric_type IS NOT NULL ORDER BY metric_type` — empirically `{simple, ratio, derived, cumulative, conversion}` across the sample project. |
| `packages` | *(absent)* | — | ❌ | — | Reserved for a future revision that adds a `?package=` filter (parallel to `GET /api/v1/macros/facets.packages`). Distinct values from `dbt.metrics.package_name`. |
| `owners` | *(absent)* | — | ❌ | — | Reserved for a future revision that adds a `?owner=` (= `group_name`) filter. Distinct values from `dbt.metrics.group_name` (skip nulls). |
| `tags` | *(absent)* | — | ❌ | — | Reserved for a future revision that adds a `?tag=` filter. Distinct values from `unnest(dbt.metrics.tags)`; `tags` is a `[list_utf8!]` column. |

### Type definition

```typescript
// Intentionally empty — no facets applicable in v0.
interface MetricFacetsResponse {
  // Forward-compatible: add `metric_types?`, `packages?`, `owners?`, `tags?`
  // here when the list endpoint adds the corresponding filter parameters.
  // See list-endpoint Risk #4.
}
```

### Risk register

1. **No handler exists yet.** Net-new endpoint; the handler is a one-liner that returns `Json(serde_json::json!({}))` (or a `#[derive(Serialize)] struct MetricFacetsResponse {}`). No SQL, no spawn-blocking, no parquet read. The endpoint should still exist (rather than 404) so that the FE can hit it unconditionally as part of the standard `useResourceFacets` plumbing without a special case for "this resource has no facets" — same convention as the empty-array facet rows on `GET /api/v1/macros/facets`.

2. **`{}` vs. `{ "metric_types": [], … }` is a deliberate v0 choice.** Returning explicit empty arrays for the four forward-reserved facets (`metric_types`, `packages`, `owners`, `tags`) would commit to those keys as part of the wire shape today. Returning `{}` keeps the wire shape open: when a real filter param is added on the list endpoint, the corresponding facet key is added on the facets endpoint at the same time, and clients that read the new key get a populated array on day one. ADR-6 explicitly endorses this: "FACETS: `{}` (no filters)."

3. **No capability flag introduced.** `dbt.metrics.parquet` is always present when the index exists (it is emitted by the standard index path for any project with a `semantic_manifest.json`, regardless of toolchain — see retired Risk #6 on the detail contract). The endpoint does not need a `has_*` gate, and no new capability flag is introduced by this contract (consistent with detail Design note 7).

4. **Empty parquet case.** When `dbt.metrics.parquet` has zero rows, the response is still `{}` — the endpoint is wire-stable regardless of project state. No special handling required.

5. **Sort order is not specified.** Mirrors `list_model_facets` — facet values, when present, are returned in an order determined by SQL `ORDER BY` (alphabetical for project-specific values; convention order for enum-style values like `metric_types`). Document explicitly at promotion time so FE engineers don't expect the facet endpoint to honor a `?sort=` plumbing.

---

## `GET /api/v1/saved_queries`

Paginated list of saved-query definitions. Powers the table on the dbt-ui
saved-queries filter page.

### Query parameters

| Param | Type | Default | Notes |
|---|---|---|---|
| `sort` | `string` | `name:asc` | `<column>:<asc\|desc>`. Allowlisted column: `name`. The dbt-ui table renders no sortable headers (`SavedQueryFilterView` columns are render-only), so `name` is the only sortable column for v0. Invalid column → 400. |
| `first` | `u32` | `100` (max `1000`) | Per-page row count. Server clamps. Clamped to `[1, 5000]` (same envelope as `GET /api/v1/models` and `GET /api/v1/macros`). |
| `after` | `string` (opaque base64) | — | Cursor returned as `page_info.end_cursor` on the previous page. Omit for the first page. Tampering yields 400. |
| _(no separate inline-edge param)_ | — | — | Per the updated CC-6, the inline `depends_on_nodes[]` array is capped server-side at 500 entries with `depends_on_nodes_truncated: true` set when exceeded. The client cannot tune the cap (the `first` name is reserved for cursor pagination on this LIST endpoint per ADR-6). |

Per ADR-6 the LIST endpoint uses cursor pagination (`first`/`after` + `page_info`). The saved-queries parquet typically has tens of rows even in mature projects, so single-page responses are the common case; the cursor envelope still applies for shape consistency with every other LIST endpoint.

No filter parameters are accepted. The dbt-ui `SavedQueryFilterView` does not
render any `Dropdown` filter widgets (unlike `MacroFilterView` which exposes a
package dropdown); the GraphQL hook accepts a `GenericMaterializedFilter` but
the view never populates it. Future filters (e.g., `?package=`, `?tags=`,
`?group=`) are additive per ADR-6 and can be added without a contract break.

### Example response

Fields marked `// 🔧` are not yet returned — no list handler exists today; every
field on the row is 🔧 (or 🔍 where parquet shape is unverified).

```json
{
  "data": [
    {
      "unique_id": "saved_query.jaffle_shop.weekly_revenue_summary",
      "name": "weekly_revenue_summary",
      "package_name": "jaffle_shop",
      "group_name": "finance",
      "tags": ["finance", "weekly"],
      "description": "Weekly revenue by region, materialized to the analytics schema.",
      "created_at": 1747320731.0,
      "depends_on_nodes": [
        "metric.jaffle_shop.revenue",
        "metric.jaffle_shop.order_count",
        "semantic_model.jaffle_shop.customers"
      ],
      "depends_on_nodes_truncated": false
    },
    {
      "unique_id": "saved_query.jaffle_shop.orders_by_month",
      "name": "orders_by_month",
      "package_name": "jaffle_shop",
      "group_name": null,
      "tags": [],
      "description": null,
      "created_at": 1747320731.0,
      "depends_on_nodes": [
        "metric.jaffle_shop.order_count"
      ],
      "depends_on_nodes_truncated": false
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiPHN0YXJ0X3NvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "end_cursor": "eyJzIjoiPHNvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "has_next_page": false
  }
}
```

No capability gates apply to any field on this response — every included field
is Core (parquet-backed, unconditional). No `execution_info` block exists on
saved-query rows per ADR-5.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

No handler exists for `GET /api/v1/saved_queries` today; every included field is 🔧 (or 🔍).

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"saved_query.jaffle_shop.weekly_revenue_summary"` — from `dbt.saved_queries.unique_id`. Required, non-null in parquet schema (`[utf8!]`). |
| `name` | `string` | Core | 🔧 | — | From `dbt.saved_queries.name`. The dbt-ui table renders this as the link cell; `parseUniqueId(savedQuery.uniqueId).remainder` derives an equivalent value in the frontend today. |
| `package_name` | `string \| null` | Core | 🔧 | — | From `dbt.saved_queries.package_name`. Parquet column is `[utf8]` (nullable in the Arrow schema even though `SavedQueryRow.package_name` is `&'a str` on the Rust side); honor the schema and emit `null` when absent. |
| `group_name` | `string \| null` | Core | 🔧 | — | From `dbt.saved_queries.group_name` (`[utf8]`, nullable). The `SavedQueryFilterView` table does not render Owner today; surfacing this on the list row is forward-compatible with adding an Owner column without a contract bump. |
| `tags` | `string[]` | Core | 🔧 | — | From `dbt.saved_queries.tags` (`[list_utf8!]`, native Arrow `List(Utf8)` — empty list rather than null when no tags). arrow_json serializes lists as JSON arrays; emit `[]` for the no-tag case. |
| `description` | `string \| null` | Core | 🔧 | — | From `dbt.saved_queries.description`. Null when the YAML schema patch declares no description; the dbt-ui Description column renders `null` as an empty cell. |
| `created_at` | `number \| null` | Core | 🔧 | — | Epoch seconds (float) from `dbt.saved_queries.created_at` (`[float]`, nullable per `SavedQueryRow`). Per ADR-5, this is the "Definition updated as of …" timestamp surfaced to `SavedQueryFilterView`'s row; Discovery API analogue is `runGeneratedAt` — see Risk #2. |
| `depends_on_nodes` | `string[]` | Core | 🔧 | — | Truncatable per CC-6. From `dbt.saved_queries.depends_on_nodes` (`[list_utf8!]`). Typically references `metric.*` / `semantic_model.*` unique_ids; macros are intentionally excluded — see Risk #3. Default cap: 500 (overridable via `?first=<n>`). |
| `depends_on_nodes_truncated` | `boolean` | Core | 🔧 | — | `true` when the underlying list exceeded `first` and the response is truncated; `false` otherwise. CC-6 compliance signal. |
| `label` | *(absent)* | — | ❌ | — | Available on the detail endpoint (`GET /api/v1/saved_queries/:id`). Omitted from the list row because the `SavedQueryFilterView` table renders no Label column. Promote if a future column needs it. |
| `query_params` | *(absent)* | — | ❌ | — | Available on the detail endpoint. JSON-string column per CC-7; deserializing on every list row would amplify response size and parse cost (`metrics[]`, `group_by[]`, `where.where_filters[]` per row). The `SavedQueryFilterView` table never renders query params; users navigate to the detail page to inspect them. |
| `exports` | *(absent)* | — | ❌ | — | Available on the detail endpoint. JSON-string column per CC-7; same rationale as `query_params` — list row is the wrong place to materialize per-row exports. |
| `depends_on_macros` | *(absent)* | — | ❌ | — | Available on the detail endpoint indirectly via `depends_on[*]` (where the per-edge `edge_type` carries `"macro"`). For the LIST row, macro edges are excluded from `depends_on_nodes[]` to keep the lineage chip surface focused on metrics / semantic models — see Risk #3. |
| `refs` | *(absent)* | — | ❌ | — | `dbt.saved_queries.refs` is an internal serialized representation of YAML `ref()` calls; the resolved `depends_on_nodes[]` is the canonical surface. The list row does not duplicate it. |
| `config` | *(absent)* | — | ❌ | — | JSON-string parquet column per CC-7; available on the detail endpoint only if a real consumer needs it. The `SavedQueryFilterView` does not render config. |
| `fqn` | *(absent)* | — | ❌ | — | Available on the detail endpoint. The list view renders no FQN information; the row keeps `unique_id` for routing and `name` for display. |
| `file_path` / `original_file_path` | *(absent)* | — | ❌ | — | Available on the detail endpoint. The list view renders no path information. |
| `meta` | *(absent)* | — | ❌ | — | `dbt.saved_queries` parquet has no `meta` column (unlike `dbt.nodes`). Detail endpoint also omits this. |
| `execution_info` | *(absent)* | — | ❌ | — | Per ADR-5 saved queries are not runnable (no `dbt_rt.run_results` rows for `saved_query.*`); the field is omitted from `DefinitionNodeBase` entirely. This row is documentation only. |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api. |
| `run_id` / `project_id` / `run_generated_at` | *(absent)* | — | ❌ | — | Class B: CodexDB-only identifiers; no parquet path. The closest local analogue to `runGeneratedAt` is the per-row `created_at` above — see Risk #2. |
| `referenced_by` | *(absent)* | — | ❌ | — | Class A but list-inappropriate. Available on the detail endpoint via `referenced_by[*]` from `dbt.edges`. Saved queries are rarely referenced by anything (an exposure can theoretically depend on a saved query — see the detail-endpoint Risk #4); the list row would carry an array of size zero for almost every row. Inline only if a `SavedQueryFilterView` consumer needs it. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface SavedQueryListResponse {
  data: SavedQuerySummary[];
  page_info: PageInfo;
}

interface SavedQuerySummary {
  unique_id: string;
  name: string;
  package_name: string | null;
  group_name: string | null;
  tags: string[];
  description: string | null;
  created_at: number | null;
  depends_on_nodes: string[];
  depends_on_nodes_truncated: boolean;
}
// PageInfo: see ADR-6 § "Shared TypeScript types" for the canonical declaration.

```

### Risk register

1. **No handler exists yet.** Net-new endpoint; SQL is a single `SELECT … FROM dbt.saved_queries` with optional `ORDER BY name ASC LIMIT … OFFSET …`. The handler should follow the `list_models` pattern in `src/handlers/models.rs` (spawn-blocking + `query_arrow` + `batches_to_*_rows` extractor) and live in a new `src/handlers/saved_queries.rs` file alongside the existing detail handler (or together with the planned detail handler if neither exists at integration time). Extraction of `tags: Vec<String>` reuses the `extract_str_list` helper that `models.rs::extract_node_detail` already defines for `fqn` / `tags` on `ModelDetail`; promote it to `handlers/node_base.rs` if duplication appears.

2. **`run_generated_at` ≠ `created_at`.** Carries forward the same caveat from the saved-queries detail-endpoint contract (Risk #5 there). The Discovery API field `runGeneratedAt` is the manifest-generation timestamp from CodexDB (project-wide). The parquet `created_at` is a per-row epoch-seconds float that may represent the parse-time of the saved query YAML; empirically it is *constant across all saved-query rows in a given parquet ingest* in the sample project, which means it serves the same UX purpose as `runGeneratedAt` for v0 even though the semantics differ. Document the divergence in the FE-facing API docs so engineers don't expect Cloud parity. If a future SL execution-history backfill (the ADR-5 revisit trigger) lands, a real `last_run_at` analogue would supersede `created_at` for header rendering.

3. **`depends_on_nodes[]` excludes macros.** `SavedQueryRow` carries both `depends_on_nodes: Vec<String>` and `depends_on_macros: Vec<String>` (parquet.rs:1263-1264). The list row surfaces only `depends_on_nodes` — the macro list is noise for a filter-page lineage chip (every Jinja-using saved query depends on a `macro.dbt.*` helper). The detail endpoint exposes the full edge set (including macros) via the `depends_on` array shape from `dbt.edges`. Document so MCP consumers know `depends_on_nodes` is a node-only projection, not the full edge graph.

4. **`tags` is `List(Utf8)` in parquet; empty-list vs null disambiguation.** `dbt.saved_queries.tags` is declared `[list_utf8!]` (non-null list column) in `SavedQueryRow`, so the contract returns `[]` (never `null`) when no tags are declared. Confirm against the live parquet during handler implementation — if the underlying Arrow column is actually nullable (the `!` suffix governs the Rust binding, not the on-disk schema), wrap the extractor with the same `is_null(0)` guard `extract_str_list` already uses.

5. **`depends_on_nodes[]` on the list row is read from `dbt.saved_queries.parquet`, not joined against `dbt.edges`.** The detail endpoint reads from `dbt.edges` (giving the per-edge `edge_type` discriminator); the list row prefers the parquet-native list column because it avoids a per-row join across `dbt.edges` for every page rendered. The two MUST agree on contents (both are populated from the same dbt parse run). If a future ingest divergence is observed (e.g., `dbt.edges` includes only resolved edges and `depends_on_nodes` includes pre-resolution refs), switch the list-row source to `dbt.edges`. For v0, trust the parquet column.

6. **`first` cap applies per row, total list bytes still bounded by `limit`.** A request with `limit=1000` and `first=500` could theoretically inline `1000 × 500 = 500,000` entries across the response. Empirically saved queries depend on ≤10 nodes each, so this is a paper risk. If a misbehaving project pushes the inline size past a reasonable threshold (e.g., 1 MB), the response will stream out fine but FE rendering may stall — surface the existing `truncated:true` flag as the escape hatch and document the per-row `first` default of 500 as conservative.

7. **No sort surface despite `sort=name:asc` default.** The dbt-ui `SavedQueryFilterView` does not render sortable column headers. The contract still accepts `?sort=name:asc|desc` so the handler shares the same `parse_sort` plumbing as `GET /api/v1/models` and so an MCP / AI consumer (or a future FE toggle) can sort without a contract change. Promoting additional columns to the allowlist (e.g., `package_name`, `created_at`) is additive and does not require a new ADR.

8. **GraphQL `filter` is unused by the dbt-ui view.** `useDiscoveryDefinitionSavedQueries` accepts a `filter: GenericMaterializedFilter` argument; `SavedQueryFilterView.tsx` never passes it. The REST contract mirrors the *view's* surface, not the *hook's* surface — no filter query parameters in v0. If a future view adds a package / tag / owner dropdown, add `?package=`, `?tags=`, `?group=` as additive filters per ADR-6 without a new ADR.

---

## `GET /api/v1/saved_queries/facets`

All filter facet values for the saved-queries list. Per ADR-6 the facets
endpoint takes no query parameters and returns the full distinct set for each
facet key.

### Query parameters

None.

### Example response

```json
{}
```

`SavedQueryFilterView` exposes no filter dropdowns; the facets endpoint
correctly returns an empty object (`{}`), distinct from
`{ "packages": [] }` (which would assert that "packages" is an applicable but
empty facet — it isn't, because no `package` filter is supported in v0).

Per ADR-6 guidance the empty-object shape is **the correct response when no
facet keys apply** to the resource. The endpoint exists at this URL so the FE
codegen pipeline can emit a consistent `<Resource>FacetsResponse` type for
every list endpoint and so a future `?package=` filter can be added by
populating one key on this response (e.g., `{ "packages": [...] }`) without
the FE having to discover a new URL.

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

No handler exists for `GET /api/v1/saved_queries/facets` today; every potential field is currently absent by design.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `packages` | *(absent)* | — | ❌ | — | No package facet — `SavedQueryFilterView` does not render a `PackageFilterDropdown`. The parquet column `dbt.saved_queries.package_name` is present and could populate a facet if a future view needs it; ship empty until then. |
| `tags` | *(absent)* | — | ❌ | — | No tags facet — `SavedQueryFilterView` does not render a tag dropdown. The parquet `dbt.saved_queries.tags` (`[list_utf8!]`) supports a distinct-unnest query when needed. |
| `groups` / `owners` | *(absent)* | — | ❌ | — | No owners facet — `SavedQueryFilterView` does not render an Owner dropdown. The parquet `dbt.saved_queries.group_name` is available for a future facet but is not surfaced in v0. |

### Type definition

```typescript
// Empty object literal — no facet keys apply because the dbt-ui filter view
// exposes no filter dropdowns. The shape is a deliberate ADR-6 contract:
// future facets (packages, tags, groups) appear as additive keys.
interface SavedQueryFacetsResponse {}
```

### Risk register

1. **`{}` is the correct empty shape, not `null` and not `204 No Content`.** The endpoint succeeded; it just has no facet keys to report. Handler must emit `{ }` (HTTP 200, `Content-Type: application/json`) so the FE codegen pipeline can generate `SavedQueryFacetsResponse` as a typed object. Returning `204` or `null` would break consumers that destructure the response without a null guard.

2. **Reserving facet keys for future additive growth.** When a future iteration of `SavedQueryFilterView` introduces a `PackageFilterDropdown` (mirroring `MacroFilterView`), the response shape evolves to `{ "packages": FacetValue[] }`. This is additive per ADR-6 — existing clients that ignore unknown keys continue to work. Document at the top of the handler that adding facets is non-breaking; *removing* a facet key once shipped would be breaking (the FE would lose a filter dropdown).

3. **No capability flag introduced.** `dbt.saved_queries.parquet` is always present when the index exists (emitted by `dbt parse` unconditionally, not by `dbt build` / `dbt docs generate`). The endpoint does not need a `has_*` gate and never returns 412.

4. **Sort order is irrelevant for the empty case but locked-in for forward compatibility.** If facet keys are added later, each `FacetValue[]` must follow the alphabetical sort convention already used by `list_model_facets` (`owners` ORDER BY in SQL) and `list_macro_facets` (`packages` ORDER BY). No `?sort=` parameter on this endpoint, ever.

5. **No empty-parquet handling needed.** Because the response is `{}` regardless of parquet content, an empty `dbt.saved_queries.parquet` produces the same response as a populated one. The handler does not need to query the parquet at all for v0; it can return a `Json(SavedQueryFacetsResponse {})` literal. When facets are added, the query path joins back in.

---

## Design notes

Three decisions are specific to this LIST + FACETS pair and warrant calling out before review.

**1. ADR-5: definition-only, no `execution_info`; `created_at` is the per-row freshness signal.**
Semantic models are spec-only — they declare entities, dimensions, and measures on top of an existing model but are not themselves executed against the warehouse. Per ADR-5, the LIST row composes `DefinitionNodeBase` and omits `execution_info` entirely (not null-gated). The "Definition updated as of …" timestamp the FE would otherwise pull from a run-result column is sourced from `dbt.semantic_models.created_at` (a `double` column storing epoch seconds — empirically verified across 10 rows in the sample project). The LIST row carries `created_at` directly; clients that want a project-level freshness fallback go to `GET /api/v1/project`.

**2. `entities[]` is JOIN-derived from `dbt.semantic_entities`, not a column on `dbt.semantic_models`.**
The FE `SemanticModelFilterView` renders an "Entities" column as a comma-separated list of entity names (`flatMap(node => node.name).join(', ')`). The corresponding GraphQL hook (`definitionSemanticModels.ts`) fetches `entities { name, type }` as a nested object array on each `SemanticModelDefinitionNode`. The parquet shape mirrors GraphQL: `dbt.semantic_models` has no `entities` list column — entity rows live in `dbt.semantic_entities` keyed by the parent semantic model's `unique_id`. The handler must `JOIN dbt.semantic_entities e ON e.unique_id = sm.unique_id` (or per-row fan-out) and emit `entities[]` as a JSON array of `{name, type, ...}` objects. The FE flattens to comma-separated names at the cell level; emitting the structured array (not a server-side joined string) keeps the LIST aligned with the detail-endpoint shape and avoids re-parsing on the client.

**3. CC-6: `entities[]` follows the inline-edge truncation convention.**
`entities[]` is the only inline array on this LIST row. Per CC-6, accept an optional `?first=<n>` query parameter that caps the per-row entity count and emit a sibling `truncated: true` flag on each row whose entities were capped. Default cap: 500. In practice semantic models declare a handful of entities (tens, never hundreds), so the cap is defensive — keep the contract aligned with `depends_on` / `referenced_by` on typed detail endpoints rather than carving an exception.

---

## `GET /api/v1/semantic_models`

Powers: `SemanticModelFilterView` in dbt-ui.

dbt-ui component: `packages/metadata/dbt-explorer/src/pages/account/project/resource/components/FilterPages/SemanticModelFilterView.tsx`

GraphQL hook: `packages/metadata/dbt-explorer/src/hooks/discovery/definitionSemanticModels.ts` (`GetSemanticModelsByUniqueId`)

Paginated list of semantic models. The FE table renders three columns — Name, Entities (comma-separated names), Description — and currently exposes no filters or sorts. ADR-6 default sort (`name:asc`) applies; `?sort` is accepted with `name` as the only allowlisted column (additive: more allowlisted columns can be added without breaking the contract). The response envelope is the ADR-6 standard: `{ data, total, offset, limit }`.

### Query parameters

| Param | Type | Default | Notes |
|---|---|---|---|
| `first` | `u32` | `100` (max `1000`) | Per-page row count. Server clamps. Per ADR-6 cursor envelope. |
| `after` | `string` (opaque base64) | — | Cursor returned as `page_info.end_cursor` on the previous page. Omit for the first page. Tampering yields 400. |
| `sort` | `<col>:<asc\|desc>` | `name:asc` | Allowlisted columns: `name`. Invalid column or direction → `400`. |

No filter parameters are accepted in v0 — the FE filter view has none. Future filters (e.g., `?group_name=`, `?primary_entity=`) are additive.

### Example response

`execution_info` is **omitted entirely** per ADR-5 — it is not a null field on the row.
`entities[]` is a JOIN-derived array of `{name, type, ...}` objects (not a pre-joined string).
Fields marked `// 🔧` require a backend change (no LIST handler exists today).

```json
{
  "data": [
    {
      "unique_id": "semantic_model.jaffle_shop.orders",
      "name": "orders",
      "package_name": "jaffle_shop",
      "group_name": "finance",
      "primary_entity": "order",
      "entities": [
        { "name": "order", "type": "primary" },
        { "name": "customer", "type": "foreign" }
      ],
      "description": "Semantic model over the orders fact table.",
      "created_at": 1747432300.5,
      "truncated": false
    },
    {
      "unique_id": "semantic_model.jaffle_shop.customers",
      "name": "customers",
      "package_name": "jaffle_shop",
      "group_name": null,
      "primary_entity": "customer",
      "entities": [
        { "name": "customer", "type": "primary" }
      ],
      "description": null,
      "created_at": 1747432301.1,
      "truncated": false
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiPHN0YXJ0X3NvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "end_cursor": "eyJzIjoiPHNvcnRfdmFsdWU+IiwiaSI6Ijx1bmlxdWVfaWQ+In0",
    "has_next_page": false
  }
}
```

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

There is no `GET /api/v1/semantic_models` handler today — every row below is 🔧 (or 🔍 where parquet presence is unverified). Class A fields are the bulk of the contract.

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `unique_id` | `string` | Core | 🔧 | — | e.g., `"semantic_model.pkg.name"`; from `dbt.semantic_models.unique_id` |
| `name` | `string` | Core | 🔧 | — | From `dbt.semantic_models.name`; renders the Name column |
| `package_name` | `string \| null` | Core | 🔧 | — | From `dbt.semantic_models.package_name` |
| `group_name` | `string \| null` | Core | 🔧 | — | From `dbt.semantic_models.group_name`; not surfaced by the v0 FE filter view but cheap to include for parity with the detail endpoint |
| `primary_entity` | `string \| null` | Core | 🔧 | — | From `dbt.semantic_models.primary_entity`; entity name designated primary at the semantic-model level. May overlap with one of the `entities[].type == "primary"` rows — see detail-contract Risk #7 |
| `entities` | `SemanticEntityRef[]` | Core | 🔧 | — | JOIN-derived from `dbt.semantic_entities` on parent `unique_id`. Subset of the detail-endpoint `SemanticEntity` (LIST shows `name`, `type`; the detail endpoint adds `description`, `label`, `expr`, `role`). Honors `?first=<n>` per CC-6. See Design note #2 |
| `entities[*].name` | `string` | Core | 🔧 | — | From `dbt.semantic_entities.name` |
| `entities[*].type` | `string \| null` | Core | 🔧 | — | `"primary"` · `"natural"` · `"foreign"` · `"unique"` (MetricFlow enum); from `dbt.semantic_entities.entity_type` |
| `description` | `string \| null` | Core | 🔧 | — | From `dbt.semantic_models.description`; renders the Description column |
| `created_at` | `number \| null` | Core | 🔧 | — | Epoch seconds (float); from `dbt.semantic_models.created_at`. ADR-5 freshness signal. Empirically verified column present across 10 rows in the sample project |
| `truncated` | `boolean` | Core | 🔧 | — | CC-6 flag: `true` when `entities[]` was capped by `?first=<n>` on this row; `false` otherwise. Always present so clients can branch without an undefined check |
| `execution_info` | *(absent)* | — | ❌ | — | ADR-5: semantic models are not executed (no `dbt_rt.run_results` rows). Field is omitted from `DefinitionNodeBase` entirely — this row is documentation only |
| `tags` | *(absent)* | — | ❌ | — | Not needed by the LIST view; the detail endpoint surfaces `tags` (extracted from `config` JSON, see detail Risk #2). Add to LIST only when a tag filter ships |
| `fqn` | *(absent)* | — | ❌ | — | Not needed by the LIST view; available on the detail endpoint |
| `label` | *(absent)* | — | ❌ | — | Not used by the FE filter table; detail endpoint surfaces it |
| `model` | *(absent)* | — | ❌ | — | The upstream model reference is a detail-page concern (header denormalization). LIST omits to keep the row lean |
| `meta` | *(absent)* | — | ❌ | — | Detail-only |
| `dimensions` | *(absent)* | — | ❌ | — | Detail-only; would multiply parquet reads N× on LIST |
| `measures` | *(absent)* | — | ❌ | — | Detail-only; same reasoning |
| `depends_on` | *(absent)* | — | ❌ | — | Detail-only |
| `referenced_by` | *(absent)* | — | ❌ | — | Detail-only |
| `health_issues` | *(absent)* | — | ❌ | — | Class B: no parquet path; `subGraphs: ['internal']` in codex-api |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface SemanticModelListResponse {
  data: SemanticModelSummary[];
  page_info: PageInfo;
}

interface SemanticModelSummary {
  unique_id: string;
  name: string;
  package_name: string | null;
  group_name: string | null;
  primary_entity: string | null;
  entities: SemanticEntityRef[];
  description: string | null;
  created_at: number | null;   // ADR-5: epoch seconds
  truncated: boolean;          // CC-6: entities[] capped on this row?
  // No execution_info per ADR-5 (definition-only).
}

interface SemanticEntityRef {
  name: string;
  type: string | null;
}
// PageInfo: see ADR-6 § "Shared TypeScript types" for the canonical declaration.

```

### Risk register

1. **No existing LIST handler — greenfield endpoint.** The `/api/v1/models` handler (`crates/dbt-docs-server/src/handlers/models.rs`) is the structural template: validate query params, build a count + rows SQL pair, run them under `tokio::task::spawn_blocking`, decode `RecordBatch` to typed rows. The new wrinkle is the JOIN against `dbt.semantic_entities` for `entities[]` — either as a single LEFT JOIN with `LIST(struct_pack(name := e.name, type := e.entity_type))` (DuckDB) or as a second query keyed by the page's `unique_id` values. Pick the single-JOIN form unless profiling shows a problem; matches the detail endpoint's parallel-reads pattern.

2. **`entities[]` ordering is unspecified by parquet.** `dbt.semantic_entities` rows have no stable ordering column — the FE renders them comma-separated, so insertion order from the YAML would feel most natural to authors. Decision: `ORDER BY e.name` to make the rendering deterministic across page reloads. The detail endpoint should match (it currently does not specify; a follow-up cleanup).

3. **`?first` semantics — per-row cap, not a global cap.** Unlike `?limit` (page size on the outer list), `?first` caps `entities[]` *within* each row. Two clients querying the same page with different `?first` values must see the same `data[]` length but different `entities[]` shapes. Document this in the handler and add a test (e.g., `?first=1` against a semantic model with 3 entities → 1 entity returned, `truncated: true`).

4. **`primary_entity` may duplicate an `entities[].name`.** Mirrors detail-contract Risk #7: the parquet shape allows both `dbt.semantic_models.primary_entity = "order"` and a row in `dbt.semantic_entities` with `name = "order"` and `entity_type = "primary"`. The LIST emits both fields verbatim — no deduplication. FE consumers that don't want the duplicate filter `entities[].type !== "primary"` client-side.

5. **No filters or sorts in v0.** The FE filter view exposes neither. The contract reserves `?sort=name:asc|desc` and the `?<filter>=<csv>` pattern from ADR-6 for additive future use, but the v0 handler accepts only `?sort=name:*`. Anything else → `400` with a clear message. Resist the temptation to pre-add unused filter params — they'd be untested surface area.

---

## `GET /api/v1/semantic_models/facets`

Returns the set of filter facet values for the semantic models LIST. There are no filters in v0; the FE filter view exposes none. Per ADR-6 the response is the empty object — a stable shape that lets clients call the endpoint unconditionally and surface facets the moment they exist.

### Query parameters

None. Any query parameters supplied are ignored (do not 400 — keeps the endpoint hospitable to future query-string additions).

### Example response

```json
{}
```

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| *(none — empty object)* | `{}` | Core | 🔧 | — | No filters exposed by the FE in v0; ADR-6 empty-facets shape. Future filters add keys here (e.g., `groups: FacetValue[]`, `primary_entities: FacetValue[]`) additively — no consumer breaks |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
interface SemanticModelFacetsResponse {
  // Intentionally empty in v0. Add facets as filters ship.
}
```

### Risk register

1. **Empty object today is correct, not a placeholder.** Resist the urge to pre-populate `groups` or `primary_entities` facets "because we'll need them eventually." Per CC-3 and ADR-3 thinking, filter surface area follows the FE — adding a facet without a filter to consume it is dead code on the server and dead UI on the client. Add facets when the corresponding filter ships in `SemanticModelFilterView.tsx`.

2. **Future facet additions are additive.** When a filter ships, append a new key (`<filter>s: FacetValue[]`) and a matching `?<filter>=<csv>` query param on the LIST endpoint. The empty-object today does not lock in any naming; the LIST `?<filter>` query name and the FACETS key name should match (CC-1: `snake_case` plural).

3. **No SQL today; handler is a constant.** The handler returns a static `{}` and does not touch the parquet backend. When facets are added, follow the `list_model_facets` pattern: one SQL query per facet, run inside `spawn_blocking`.


---

## `GET /api/v1/search`

Powers: project search page (`/proj/search/?search=<term>`).

dbt-ui page: `packages/metadata/dbt-explorer/src/pages/account/project/search/SearchPage.tsx`

dbt-ui result components: `packages/metadata/dbt-explorer/src/pages/account/project/search/components/SearchResultsContents.tsx`, `SearchResultsList.tsx`, `SearchResultItem.tsx`

GraphQL hook (authoritative shape reference): `packages/metadata/dbt-explorer/src/hooks/discovery/appliedSearch.ts` (`GetAppliedSearchResults`)

Current handler: **none.** This is a greenfield endpoint. Reference `src/handlers/models.rs` for the list/sort/cursor pattern and `src/handlers/nodes.rs` for cross-resource aggregation over `dbt.nodes`. The implementation combines both shapes, plus per-type fan-out to `dbt.exposures`, `dbt.macros`, `dbt.metrics`, `dbt.saved_queries`, `dbt.semantic_models`, `dbt.groups`, `dbt.unit_tests`. Implementation is a separate task.

ADR-8 above documents why this is one endpoint rather than N per-type search endpoints. Read ADR-8 before changing the envelope shape.

### Query parameters

| Parameter | Type | Required | Default | Notes |
|---|---|---|---|---|
| `q` | `string` | no | — | Search query. Max length 1024 (mirrors `MAXIMUM_QUERY_LENGTH` in dbt-ui `util/input.ts`). Tokenized whitespace-split, multi-token AND, case-insensitive (ILIKE). `%` and `_` are escaped server-side before ILIKE — see Risk #3. **Browse mode:** when `q` is absent or empty, no text predicate is applied — all rows matching the other filters (`?type=`, `?package=`, `?tag=`, `?modeling_layer=`) are returned with `matched_field: null` and `highlight: null` per hit. This mirrors catalog/Discovery search behavior (typing into the box progressively narrows; an empty box browses everything) and makes filter-only requests like `?type=model&modeling_layer=Marts` first-class. No min-length floor — any non-empty `q` (even 1 char) applies the predicate. |
| `type` | `string` | no | — | Comma-separated `resource_type` filter. Mirrors `/api/v1/nodes?type=`. Allowed values: `model`, `source`, `seed`, `snapshot`, `test`, `unit_test`, `exposure`, `metric`, `semantic_model`, `saved_query`, `macro`, `group`. Multiple values are OR'd: `?type=model,source`. Invalid values → 400. |
| `package` | `string` | no | — | Comma-separated `package_name` filter. Mirrors `/api/v1/nodes?package=`. Multiple values are OR'd: `?package=jaffle_shop,jaffle_marketing`. |
| `tag` | `string` | no | — | Comma-separated tag filter; a result matches if its `tags[]` contains any of the listed values (case-insensitive equality, not substring). Multiple values are OR'd. Resource types whose parquet table has no `tags` column (`macro`, `group`, `unit_test`) are silently excluded from the result set when `?tag=` is set. |
| `modeling_layer` | `string` | no | — | Comma-separated modeling-layer filter. Mirrors `/api/v1/models?modeling_layer=`. Allowed values: `Staging`, `Intermediate`, `Marts`. Multiple values are OR'd: `?modeling_layer=Staging,Marts`. **Implicitly types-to-models:** `modeling_layer` is only defined for models (server-computed from file-path prefix on `dbt.nodes.original_file_path`; see `ModelSummary.modeling_layer` in `src/handlers/models.rs`). When this filter is set, non-model resource types have no value to match against and are silently excluded from the result set — effectively narrowing the response to model hits only. Confirmed with design; this is acceptable behavior. |
| `first` | `number` | no | 50 | Page size; max 200 (matches the cap pattern in `/api/v1/models`). |
| `after` | `string` | no | — | Opaque cursor returned by a prior page's `page_info.end_cursor`. Clients MUST treat as opaque and not parse — see Risk #8. |

**Resolved design questions** (see Step 5 of the parity prompt that produced this contract):
- **Q-E1 endpoint shape:** unified `/api/v1/search` per ADR-8.
- **Q-E2 result envelope:** Discovery-shaped `{ matched_field, highlight, hit }` siblings; single integer `total_count` placed inside `page_info` per ADR-6 (UI evidence — `SearchResultsList.tsx` renders one badge: `getResultCountString(props.totalCount)` → `"N results"`, no per-type breakdown).
- **Q-E3 searchable fields:** `name`, `fqn`, `description`, `tags`, `column` only. `compiled_code`/`raw_code` deferred (not exposed today — see Risk #4).
- **Q-E4 query syntax:** see param table; empty/absent `q` triggers browse mode (catalog parity); quoting and fielded queries deferred (Risk #5).
- **Q-E5 polymorphism:** see "Per-type hit extras" subsection.
- **Q-E6 ranking:** alphabetical by `(name ASC, unique_id ASC)`; relevance ranking deferred (Risk #1).
- **Q-E7 highlight semantics:** per-field rules in the field reference notes; matching field disambiguation `name > column > tag > fqn > description`.
- **Q-E8 filters:** `?type=`, `?package=`, `?tag=`, `?modeling_layer=`. `?modeling_layer=` implicitly narrows to model hits only (the dimension is model-specific) — design-confirmed acceptable. Health filter and model-access filter were removed per Roxi (FEATURE-TO-ENDPOINT-MAPPING.md rows 37, 38) and are not part of this contract.
- **Q-F1 facets:** no `/api/v1/search/facets` endpoint. `ResourceFilterPanel` is fed by the existing `/<resource>/facets` family. See "Non-goal" note below.
- **Q-C1 conflict with `/nodes?q=`:** orthogonal. `/nodes?q=` is "list nodes, filter by substring" with a flat row shape; `/search?q=` is "find anything across resource types, with highlight metadata and an envelope shape". The two are intentionally not merged.
- **Q-E9 cursor encoding:** global ORDER BY (`name`, `unique_id`) over the UNION; cursor is opaque to clients regardless. Switching to per-type pagination later invalidates in-flight cursors — see Risk #8.
- **Q-C2 MCP overlap:** the POC's `search_dbt` MCP tool aligns with this endpoint and can share SQL. `search_sql` (LIKE on body) is MCP-only and not exposed here — see Risk #4.

**Non-goal — `/api/v1/search/facets`.** `SearchPage.tsx` mounts `ResourceFilterPanel` from `src/components/ResourceFilterPanel/`, which is fed by the existing per-resource `/facets` endpoints (static project-wide distinct values, not query-dependent counts). The widgets present after Roxi's removals: `TypeFilters`, `TagFilters`, `AdvancedFilters` (search-field selector). None of these need per-query counts. Do not design a `/search/facets` endpoint without first finding new UI evidence of demand.

### Error responses

| HTTP | `code` | When |
|---|---|---|
| 400 | `query_too_long` | `len(q) > 1024`. Matches the `MAXIMUM_QUERY_LENGTH` constant in dbt-ui `util/input.ts`. The 1024-char cap protects the SQL planner from pathological multi-token patterns. |
| 400 | `invalid_type` | `?type=` includes a value not in the documented `resource_type` set. Body lists the offending value(s). |
| 400 | `invalid_modeling_layer` | `?modeling_layer=` includes a value not in `{Staging, Intermediate, Marts}`. Body lists the offending value(s). |
| 400 | `invalid_cursor` | `?after=` is not URL-safe base64 of a valid `Cursor` JSON. Matches the existing list endpoints' `Cursor::decode → "invalid cursor"` behavior. Stale cursors after a server upgrade (e.g., Q-E9 strategy change) surface here rather than silently-wrong results. |

Empty / absent `?q=` is **not** an error — it triggers browse mode (see the `q` row in the query parameters table). All other filters (`?type=`, `?package=`, `?tag=`, `?modeling_layer=`) still apply. No `query_too_short` floor — single-char queries are accepted and matched verbatim. This mirrors catalog/Discovery behavior.

All error responses follow the existing error envelope shape (see `src/handlers/json.rs::error`). 412 is intentionally unused — `/api/v1/search` has no gated dependencies; capability flags (`has_source_freshness`) only null-gate response fields, never reject the request.

### Backend prerequisite — SQL skeleton

This subsection codifies the implementation decisions reached during contract review. Risk register items reference back to these by index.

**Empirical validation.** The CTEs in (1) and (2) below were executed end-to-end against the sample project at `/Users/eddowh/codaz/sl-schema-evolution/sample_project/target/index/` with `?q=order`. The pipeline returned 15 hits — 14 priority-1 `name` matches and one priority-2 `column` match (the `customers` model, which has `order_id`/`order_total` columns but no `order` in its name). The priority dedupe correctly resolved multi-field matches. Two SQL syntax issues were caught and corrected during validation: tag-matching must use `list_filter(tags, x -> x ILIKE ...)` (not `UNNEST(tags) AS t WHERE t ILIKE ...` — UNNEST returns a struct, not a scalar), and semantic_models tag extraction must use `(json_extract(config, '$.tags'))::varchar[]` (not `list_transform(json_extract(...), x -> json_extract_string(x, '$'))` — fails to bind).

**(1) UNION strategy — dynamically-pruned (decision for Q-E9).**

The handler builds a `UNION ALL` at request time, including only the branches whose `resource_type` appears in `?type=` (or all branches when `?type=` is absent). Five resource types live in `dbt.nodes` and share a branch shape; the other seven each get their own parquet table.

**Interaction with `?modeling_layer=`.** When `modeling_layer` is present, the requested-type set is intersected with `{model}` before branch construction: the dimension is model-only, so any non-model branch under that filter would return zero rows. Short-circuiting to a single nodes-with-`resource_type='model'` branch is both an optimization and a correctness move (avoids scanning eight parquet files for a request that can only match one).

```rust
// In the request handler:
let mut requested_types: BTreeSet<ResourceType> = params
    .type_filter
    .as_deref()
    .map(parse_csv_resource_types)
    .transpose()?                            // 400 on invalid
    .unwrap_or_else(ResourceType::all);

// modeling_layer is model-only; intersect to avoid scanning irrelevant branches.
if params.modeling_layer.is_some() {
    requested_types.retain(|t| *t == ResourceType::Model);
}

let mut branches: Vec<String> = Vec::new();
// dbt.nodes covers model / source / seed / snapshot / test
let nodes_types: Vec<_> = requested_types
    .iter()
    .filter(|t| t.lives_in_dbt_nodes())
    .map(|t| t.as_str())
    .collect();
if !nodes_types.is_empty() {
    branches.push(nodes_branch_sql(&nodes_types));
}
if requested_types.contains(&ResourceType::Exposure)     { branches.push(exposures_branch_sql()); }
if requested_types.contains(&ResourceType::Macro)        { branches.push(macros_branch_sql()); }
if requested_types.contains(&ResourceType::Metric)       { branches.push(metrics_branch_sql()); }
if requested_types.contains(&ResourceType::SavedQuery)   { branches.push(saved_queries_branch_sql()); }
if requested_types.contains(&ResourceType::SemanticModel){ branches.push(semantic_models_branch_sql()); }
if requested_types.contains(&ResourceType::Group)        { branches.push(groups_branch_sql()); }
if requested_types.contains(&ResourceType::UnitTest)     { branches.push(unit_tests_branch_sql()); }

let base_union = branches.join("\nUNION ALL\n");
```

Each branch projects the same column set (with `NULL` for type-specific columns that don't apply to that resource), so the outer SELECT is uniform:

```sql
-- branch for dbt.nodes (model/source/seed/snapshot/test)
SELECT
  unique_id, name, resource_type, package_name, fqn,
  tags,
  materialized, access_level,           -- type-specific (model)
  source_name,                          -- type-specific (source)
  NULL::varchar AS exposure_type,       -- not applicable
  description
FROM 'dbt.nodes.parquet'
WHERE resource_type IN (<requested_node_types>)

UNION ALL

-- branch for dbt.exposures
SELECT
  unique_id, name, 'exposure', package_name, fqn,
  tags,
  NULL::varchar, NULL::varchar,
  NULL::varchar,
  exposure_type,
  description
FROM 'dbt.exposures.parquet'

-- ...and so on for macros / metrics / saved_queries / semantic_models / groups / unit_tests
-- Branches whose parquet lacks columns (e.g., dbt.macros has no fqn, no tags) project NULL::varchar
-- and NULL::varchar[] for those positions.
```

**(2) `matched_field` selection — per-field UNION with priority dedupe (decision for Q-E7).**

A CTE materializes one row per `(unique_id, matched_field)` pair, tagged with a priority integer; the outer aggregation picks the lowest-priority match per `unique_id` via DuckDB's `arg_min`. Cleanest separation, each branch independently testable, and the planner sees all five predicates at once for shared scans.

**Browse-mode short-circuit.** When `q` is empty or absent, the `field_matches` / `winners` CTEs are skipped entirely — no field "won" because no predicate was applied. The outer SELECT becomes `SELECT b.*, NULL::varchar AS matched_field FROM base b ORDER BY name, unique_id LIMIT $first`, and `highlight` is emitted as `null` for every row in serialization. This avoids five no-op ILIKE scans of the base UNION and a meaningless aggregation. The handler branches on `params.q.is_some_and(|q| !q.is_empty())`.

```sql
WITH base AS (
  -- The dynamically-pruned UNION ALL from (1)
),
field_matches AS (
  SELECT unique_id, 'name'        AS matched_field, 1 AS priority
    FROM base
    WHERE name ILIKE '%' || $q_escaped || '%' ESCAPE '\\'

  UNION ALL
  SELECT b.unique_id, 'column', 2
    FROM base b
    JOIN 'dbt.node_columns.parquet' c USING (unique_id)
    WHERE c.column_name ILIKE '%' || $q_escaped || '%' ESCAPE '\\'

  UNION ALL
  SELECT unique_id, 'tag', 3
    FROM base
    WHERE tags IS NOT NULL
      AND len(list_filter(tags, x -> x ILIKE '%' || $q_escaped || '%' ESCAPE '\\')) > 0

  UNION ALL
  SELECT unique_id, 'fqn', 4
    FROM base
    WHERE fqn IS NOT NULL
      AND array_to_string(fqn, '.') ILIKE '%' || $q_escaped || '%' ESCAPE '\\'

  UNION ALL
  SELECT unique_id, 'description', 5
    FROM base
    WHERE description ILIKE '%' || $q_escaped || '%' ESCAPE '\\'
),
winners AS (
  SELECT unique_id, arg_min(matched_field, priority) AS matched_field
  FROM field_matches
  GROUP BY unique_id
)
SELECT b.*, w.matched_field
FROM base b
JOIN winners w USING (unique_id)
ORDER BY b.name, b.unique_id
LIMIT $first;
```

`$q_escaped` is the result of replacing `\`, `%`, `_` with their backslash-escaped forms before substitution into the SQL string (Risk #3). The Rust handler holds the escaped pattern; the SQL uses `ESCAPE '\\'` to keep the original wildcards inert.

**(3) `total_count` — separate parallel COUNT query (decision for total_count strategy).**

The page query and the count query share the same `base` UNION and `field_matches` predicates but differ in the outer shape. Issue both concurrently via `tokio::join!`; total latency is `max(page, count)` rather than `page + count`.

```rust
let (page_rows, total_count) = tokio::join!(
    backend.query(&page_sql, &[&first_clamped, &cursor_predicate]),
    backend.query_scalar::<u64>(&count_sql, &[]),
);
```

```sql
-- count_sql (same base + field_matches CTE, but outer is COUNT only):
WITH base AS (...), field_matches AS (...), winners AS (...)
SELECT COUNT(*) FROM winners;
-- The base UNION ALL and field_matches CTE are scanned twice (once per query).
-- For 10k-node projects this is fine; if it becomes hot, the next step is a
-- shared materialized view of `winners` cached per (q, filters) tuple.
```

**(4) Cursor encoding — reuse `pagination::Cursor` from list endpoints.**

The cursor is the URL-safe base64 of a JSON-serialized `{ sort_value, unique_id }` struct (see `src/handlers/pagination.rs::Cursor`). For search, `sort_value` is the `name` of the last row in the page. Decode errors return `400 invalid_cursor` (per the error table above), matching the existing `/api/v1/models` behavior. Clients MUST treat cursors as opaque.

**(5) `freshness_checked` JOIN gating.**

When `Capabilities.has_source_freshness` is `false`, the handler skips the `LEFT JOIN dbt.source_freshness` entirely and projects `NULL` for `freshness_checked`. This avoids a parquet read of a file that may not exist on disk (no `dbt source freshness` ever ran). When the capability is `true`, the JOIN's existence-predicate populates the boolean.

```rust
// In the dbt.nodes branch for source rows:
let freshness_join = if state.capabilities().has_source_freshness {
    "LEFT JOIN 'dbt.source_freshness.parquet' f USING (unique_id)"
} else {
    "" // skip the file read entirely
};
let freshness_projection = if state.capabilities().has_source_freshness {
    "f.unique_id IS NOT NULL AS freshness_checked"
} else {
    "NULL::boolean AS freshness_checked"
};
```

**(6a) `executed_at` LEFT JOIN — runnable types only.**

The nodes branch (model/source/seed/snapshot/test) LEFT JOINs an aggregation of `dbt_rt.run_results` per `unique_id`, projecting `CAST(MAX(created_at) AS VARCHAR) AS executed_at`. Same `MAX(created_at)` strategy used by `/api/v1/models/:id` for `execution_info.completed_at` (so the two endpoints agree on "last run"). Non-runnable branches project `NULL::VARCHAR AS executed_at` to keep the UNION shape uniform; the `SearchHit` field is `#[serde(skip_serializing_if = "Option::is_none")]` so non-runnable types serialize without the field, matching ADR-5. `unit_test` projects via the same union slot but currently has no rows in `dbt_rt.run_results` (the runtime doesn't write unit-test results into that table today) — `null` for now, populated automatically when the runtime starts emitting them.

```sql
-- in the nodes branch:
LEFT JOIN (
  SELECT unique_id, CAST(MAX(created_at) AS VARCHAR) AS executed_at
  FROM dbt_rt.run_results
  GROUP BY unique_id
) rr ON rr.unique_id = n.unique_id
```

**(6) `semantic_models` tags extraction.**

`dbt.semantic_models.parquet` carries `tags` inside the `config` JSON blob rather than as a top-level column. The extraction syntax matches the established pattern in the `/semantic_models/:id` contract (Risk #2 of that contract):

```sql
-- in the semantic_models branch:
SELECT
  unique_id, name, 'semantic_model', package_name, fqn,
  COALESCE(
    (json_extract(config, '$.tags'))::varchar[],
    []::varchar[]
  ) AS tags,
  -- ...other columns
FROM 'dbt.semantic_models.parquet'
```

Empirically: in the sample project, 9 of 10 semantic_models have `config` NULL (and thus no extractable tags) — the COALESCE ensures the column is always a non-null `varchar[]`. The `list_filter` predicate in (2) then correctly returns no match for those rows. This is the only branch where `tags` is JSON-derived; all other branches project the parquet `tags` column directly.

### Example response

A representative response for `?q=order&first=50`. Hits illustrate every required example: at least one model, one source-shaped extra, one macro (no `fqn`), and at least one each of `matched_field` ∈ `{name, description, column, tag, fqn}`, including one `highlight: null` (the name-match case) and one comma-joined `column` highlight per UI evidence #3.

```jsonc
{
  "data": [
    {
      // matched_field is a sibling of hit (CC-2; UI evidence #2 — reads data.matchedField directly).
      // Singular noun, rendered verbatim in "Includes {matchedField}: {highlight}".
      "matched_field": "name",
      // null when the match is on `name` — the row header already displays the name with
      // BoldedText highlighting, so a second "Includes name: ..." line is suppressed
      // by SearchResultItem.tsx (`if (!data.highlight) return null;`).
      "highlight": null,
      "hit": {
        "unique_id": "model.jaffle_shop.stg_orders",
        "resource_type": "model",
        "name": "stg_orders",
        "fqn": ["jaffle_shop", "staging", "stg_orders"],
        "package_name": "jaffle_shop",
        "materialized": "view",
        "access_level": "protected",
        // Last-run completion timestamp; null when dbt_rt.run_results has no row for this unique_id.
        // Same format as ExecutionInfo.completed_at on /api/v1/models/:id (Risk #1 of that contract).
        "executed_at": "2026-05-15 10:32:11.000000000-07:00"
      }
    },
    {
      "matched_field": "description",
      // 80-char window around the first match; "..." prefix/suffix when truncated.
      // <b> wraps each matched token (UI evidence #1; matches Discovery).
      "highlight": "...combining payments and <b>order</b> status, one row per <b>order</b>...",
      "hit": {
        "unique_id": "model.jaffle_shop.orders",
        "resource_type": "model",
        "name": "orders",
        "fqn": ["jaffle_shop", "orders"],
        "package_name": "jaffle_shop",
        "materialized": "table",
        "access_level": "public"
      }
    },
    {
      "matched_field": "column",
      // Comma-and-space-joined list of matching column names within this resource.
      // Each name wrapped in <b> on the matched substring. SearchResultItem.tsx
      // splits on `, ` and renders each as an individual <Link> to the columns tab
      // (UI evidence #3 — `matched_field === "column"` triggers special rendering).
      "highlight": "<b>order</b>_id, <b>order</b>_status",
      "hit": {
        "unique_id": "model.jaffle_shop.fct_orders",
        "resource_type": "model",
        "name": "fct_orders",
        "fqn": ["jaffle_shop", "marts", "fct_orders"],
        "package_name": "jaffle_shop",
        "materialized": "table",
        "access_level": "public"
      }
    },
    {
      "matched_field": "tag",
      // Single matched tag, full string, matched substring wrapped in <b>.
      // If multiple tags match, the first (alphabetically) is selected.
      "highlight": "<b>order</b>s-core",
      "hit": {
        "unique_id": "source.jaffle_shop.raw_jaffle.orders",
        "resource_type": "source",
        "name": "orders",
        "fqn": ["jaffle_shop", "raw_jaffle", "orders"],
        "package_name": "jaffle_shop",
        "source_name": "raw_jaffle",
        "freshness_checked": true  // null when has_source_freshness is false
      }
    },
    {
      "matched_field": "fqn",
      // Full dotted path with matched substring wrapped in <b>.
      "highlight": "jaffle_shop.staging.<b>order</b>_audit",
      "hit": {
        "unique_id": "test.jaffle_shop.order_audit",
        "resource_type": "test",
        "name": "order_audit",
        "fqn": ["jaffle_shop", "staging", "order_audit"],
        "package_name": "jaffle_shop",
        "test_type": "test"  // "test" | "unit_test" — mirrors ADR-3 discriminator
      }
    },
    {
      "matched_field": "description",
      "highlight": "...returns the latest <b>order</b> per customer...",
      "hit": {
        "unique_id": "macro.jaffle_shop.latest_order_per_customer",
        "resource_type": "macro",
        "name": "latest_order_per_customer",
        // Macros have no fqn — dbt.macros parquet lacks the column. Omitted, not null.
        "package_name": "jaffle_shop"
      }
    },
    {
      "matched_field": "name",
      "highlight": null,
      "hit": {
        "unique_id": "exposure.jaffle_shop.orders_dashboard",
        "resource_type": "exposure",
        "name": "orders_dashboard",
        "fqn": ["jaffle_shop", "orders_dashboard"],
        "package_name": "jaffle_shop",
        "exposure_type": "dashboard"
      }
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoic3RnX29yZGVycyIsImkiOiJtb2RlbC5qYWZmbGVfc2hvcC5zdGdfb3JkZXJzIn0=",
    "end_cursor": "eyJuYW1lIjoib3JkZXJzX2Rhc2hib2FyZCIsInVuaXF1ZV9pZCI6ImV4cG9zdXJlLmphZmZsZV9zaG9wLm9yZGVyc19kYXNoYm9hcmQifQ==",
    "has_next_page": true
  }
}
```

Empty-result response (no match for `?q=zzznothing`):

```jsonc
{
  "data": [],
  "page_info": {
    "total_count": 0,
    "start_cursor": null,
    "end_cursor": null,
    "has_next_page": false
  }
}
```

The UI's `NoSearchResults` component checks `props.data.length === 0` and renders the empty state; an empty `data[]` with `page_info.total_count: 0` is the correct trigger.

Browse-mode response (no `?q=`, just `?type=model&first=3`) — catalog parity, no text predicate, hits sorted alphabetically by `name`:

```jsonc
{
  "data": [
    {
      "matched_field": null,  // no field "won" — no predicate was applied
      "highlight": null,      // see SearchEdge type definition; both null in browse mode
      "hit": {
        "unique_id": "model.jaffle_shop.customers",
        "resource_type": "model",
        "name": "customers",
        "fqn": ["jaffle_shop", "marts", "customers"],
        "package_name": "jaffle_shop",
        "materialized": "table",
        "access_level": "public"
      }
    },
    {
      "matched_field": null,
      "highlight": null,
      "hit": {
        "unique_id": "model.jaffle_shop.fct_orders",
        "resource_type": "model",
        "name": "fct_orders",
        "fqn": ["jaffle_shop", "marts", "fct_orders"],
        "package_name": "jaffle_shop",
        "materialized": "table",
        "access_level": "public"
      }
    },
    {
      "matched_field": null,
      "highlight": null,
      "hit": {
        "unique_id": "model.jaffle_shop.stg_orders",
        "resource_type": "model",
        "name": "stg_orders",
        "fqn": ["jaffle_shop", "staging", "stg_orders"],
        "package_name": "jaffle_shop",
        "materialized": "view",
        "access_level": "protected"
      }
    }
  ],
  "page_info": {
    "total_count": 42,
    "start_cursor": "eyJzIjoiY3VzdG9tZXJzIiwiaSI6Im1vZGVsLmphZmZsZV9zaG9wLmN1c3RvbWVycyJ9",
    "end_cursor": "eyJzIjoic3RnX29yZGVycyIsImkiOiJtb2RlbC5qYWZmbGVfc2hvcC5zdGdfb3JkZXJzIn0=",
    "has_next_page": true
  }
}
```

### Field reference

Status legend: ✅ returned today · 🔧 needs backend change · 🔍 verify parquet schema · ❌ excluded (no parquet path or out of scope)

| Field | Type | Tier | Status | Capability gate | Notes |
|---|---|---|---|---|---|
| `data` | `SearchEdge[]` | Core | 🔧 | — | One envelope per result; empty array on no match. Sorted by `(hit.name ASC, hit.unique_id ASC)` over the UNION (Q-E9 option a). |
| `data[*].matched_field` | `string \| null` | Core | 🔧 | — | Singular noun. One of: `name`, `description`, `tag`, `column`, `fqn`. The UI renders verbatim ("Includes {matched_field}: ..."). Multi-field match disambiguation: priority `name > column > tag > fqn > description` — emit only the highest-priority match. Server-side: ILIKE has no offsets; the chosen field is re-scanned by case-insensitive substring search to format `highlight`. **`null` in browse mode** (empty/absent `?q=`): no field "won" because no predicate was applied; `highlight` is also `null`. |
| `data[*].highlight` | `string \| null` | Core | 🔧 | — | Inline match markup with `<b>...</b>` wrapping (UI evidence #1 — Discovery emits `<b>`, `BoldedText.tsx` parses via `BOLD_TAG_REGEX = /<\/? *[bB]>/gm`). `null` when `matched_field === "name"` (the row header already displays the bolded name, so the "Includes ..." line is suppressed by `SearchResultItem.tsx`). Per-field shape rules: see Q-E7 table below. |
| `data[*].hit` | `SearchHit` | Core | 🔧 | — | Polymorphic on `hit.resource_type` (discriminated union). |
| `data[*].hit.unique_id` | `string` | Core | 🔧 | — | Native dbt unique_id, e.g., `model.pkg.name`, `source.pkg.source_name.table`, `macro.pkg.name`, `unit_test.pkg.model_name.test_name`. Used as React key in `SearchResultsList.tsx`. |
| `data[*].hit.resource_type` | `string` | Core | 🔧 | — | Discriminator. One of: `model`, `source`, `seed`, `snapshot`, `test`, `unit_test`, `exposure`, `metric`, `semantic_model`, `saved_query`, `macro`, `group`. |
| `data[*].hit.name` | `string \| null` | Core | 🔧 | — | Resource short name. Nullable to match the UI's `SearchResultHit.name: string \| null` (the row component returns `null` if `name == null`, so back-end null is safe but expected to be rare). |
| `data[*].hit.fqn` | `string[]` | Core | 🔧 | — | Dotted-path components. Present for `model`, `source`, `seed`, `snapshot`, `test`, `unit_test`, `exposure`, `metric`, `semantic_model`, `saved_query`. Absent (field omitted) for `macro` and `group` — `dbt.macros` and `dbt.groups` parquet tables have no `fqn` column. `SearchResultItem.tsx` shows the "View lineage" link only when `hit.fqn !== undefined`, which matches: macros and groups have no lineage page. searchable; exposed by `GET /api/v1/<type>/:id` for every type that carries it. |
| `data[*].hit.package_name` | `string \| null` | Core | 🔧 | — | dbt package the resource belongs to. Used by `?package=` filter. |
| `data[*].hit.materialized` | `string \| null` | Type-specific | 🔧 | — | `resource_type === "model"` only. `"table"` · `"view"` · `"incremental"` · `"ephemeral"`. From `dbt.nodes.materialized`. |
| `data[*].hit.access_level` | `string \| null` | Type-specific | 🔧 | — | `resource_type === "model"` only. `"public"` · `"protected"` · `"private"`. From `dbt.nodes.access_level`. |
| `data[*].hit.source_name` | `string \| null` | Type-specific | 🔧 | — | `resource_type === "source"` only. The dbt source block name (e.g., `"raw_jaffle"`). From `dbt.nodes.source_name`. |
| `data[*].hit.freshness_checked` | `boolean \| null` | Type-specific (Core-conditional) | 🔧 | `has_source_freshness` | `resource_type === "source"` only. Boolean indicator from `dbt.source_freshness` (a row exists for this source ⇒ `true`). `null` when capability is false. Full freshness object lives on `/api/v1/sources/:id`, not in search results. |
| `data[*].hit.test_type` | `string` | Type-specific | 🔧 | — | `resource_type ∈ {"test", "unit_test"}`. Mirrors ADR-3's `resource_type` discriminator: `"test"` for `dbt.nodes` rows with `resource_type = "test"`, `"unit_test"` for rows in the `dbt.unit_tests` parquet. Allows the UI to route the result link to the unified `TestView.tsx` with the right narrowing. |
| `data[*].hit.exposure_type` | `string \| null` | Type-specific | 🔧 | — | `resource_type === "exposure"` only. `"dashboard"` · `"notebook"` · `"analysis"` · `"ml"` · `"application"`. From `dbt.exposures.exposure_type`. |
| `data[*].hit.executed_at` | `string \| null` | Type-specific (Core-conditional) | ✅ | — | Runnable resource types only (`model`, `source`, `seed`, `snapshot`, `test`, `unit_test`). Last-run completion timestamp — `MAX(created_at)` from `dbt_rt.run_results` grouped by `unique_id`, formatted as `CAST(... AS VARCHAR)` (space-separated local-timezone string, same format as `execution_info.completed_at` in `/api/v1/models/:id` — see Risk #1 of `/api/v1/models/:id`). `null` when the resource has no row in `dbt_rt.run_results` (i.e., `dbt build` hasn't run). **Omitted entirely** (per ADR-5) on non-runnable hit shapes (`exposure`, `metric`, `semantic_model`, `saved_query`, `macro`, `group`). Powers the "Last run: …" line on search result cards (mirrors the catalog `AssetHeader`). |
| `page_info` | `PageInfo` | Core | 🔧 | — | Standard CC-4 cursor envelope shared with every LIST endpoint (see ADR-6's shared `PageInfo` type). |
| `page_info.total_count` | `number` | Core | 🔧 | — | Total matching rows across the UNION (not just the current page). Renders as `getResultCountString(totalCount)` in `SearchResultsList.tsx` → `"N results"`. Single integer per UI evidence; no per-type breakdown is rendered. Placement under `page_info` follows ADR-6. |
| `page_info.start_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64 cursor of the FIRST row of the current page. `null` when `data` is empty. Symmetric with `end_cursor` per ADR-6. |
| `page_info.end_cursor` | `string \| null` | Core | 🔧 | — | Opaque base64-encoded `(name, unique_id)` of the last row in the page. `null` on empty result or last page. Clients MUST treat as opaque (Risk #8). |
| `page_info.has_next_page` | `boolean` | Core | 🔧 | — | `true` when at least one more result exists past `end_cursor`. |

**Per-field `highlight` shape rules (Q-E7):**

| `matched_field` | `highlight` shape |
|---|---|
| `name` | Full name with matched substring(s) wrapped in `<b>`. **In practice `null`** — emitting non-null is allowed by the schema but suppressed by `SearchResultItem.tsx`'s `if (!data.highlight) return null;` guard, so it would be invisible. Recommendation: emit `null`. |
| `fqn` | Full dotted path (e.g., `jaffle_shop.staging.stg_orders`) with matched substring(s) wrapped in `<b>`. Built from the `fqn[]` array by `.`-joining server-side. |
| `description` | 80-char window centered on the first match in `dbt.nodes.description` (or the corresponding column on `dbt.exposures`, `dbt.macros`, `dbt.metrics`, `dbt.saved_queries`, `dbt.semantic_models`, `dbt.groups`). Prefix `"..."` if truncated on the left; suffix `"..."` if truncated on the right. Matched tokens wrapped in `<b>`. |
| `tag` | The single matched tag, full string, with matched substring wrapped in `<b>`. If multiple tags match, pick the alphabetically-first and emit only that one. Tags filter for resource types whose parquet lacks a `tags` column (`macro`, `group`) is impossible — those types are never returned with `matched_field = "tag"`. |
| `column` | Comma-and-space-joined string (`"col_a, col_b"`) of all column names within this resource that match the query, each with the matched substring wrapped in `<b>`. Sourced from `dbt.node_columns.column_name`. The UI splits on `, ` and links each column individually to the model's columns tab — this is the only `matched_field` value with split-and-link rendering (UI evidence #3 — `SearchResultItem.tsx` `isColumnMatch` branch). |

**Excluded fields (`❌`)** — listed for the audit trail; not part of the response:

| Field | Why excluded |
|---|---|
| `data[*].hit.health_issues` | ❌ Class B. `subGraphs: ['internal']` in Discovery AND no parquet path. Never exposed by dbt-docs-server (load-bearing invariant). The UI's `getTrustSignalsFromHit` falls back to an empty `healthIssues: []` for any hit, which renders no badge — graceful null. |
| `data[*].hit.usage_query_count` | ❌ Class B. Platform-only; no parquet path. |
| `data[*].hit.warehouse_asset` | ❌ Class C. Account-search only (CodexDB); project-search does not surface warehouse assets. Hard exclusion per the load-bearing invariant. |
| `data[*].hit.compiled_code` / `raw_code` | ❌ Deferred (Q-E3). Class A (`dbt.nodes` has both as VARCHAR columns) but no existing endpoint exposes them today. Per the invariant — searchable fields ⊆ fields already exposed — body search is gated on first exposing the underlying fields on `/api/v1/models/:id`. See Risk #4. |
| `data[*].hit.meta` | ❌ Deferred (Q-E3). JSON-typed user metadata; substring matching against opaque JSON needs separate design. Risk #6. |
| `data[*].debug` | ❌ Deferred. Discovery's `searchResults.debug @include(if: $includeDebug)` is an operator tool surfaced via the `includeDebug` GraphQL variable. Out of scope for v0 REST. |
| `data[*].hit.column_description` (matched_field) | ❌ Deferred. `SearchFieldType` GraphQL enum has `columnDescription` as a distinct value, but the UI's `allSearchTypes` (`FilterProvider.tsx`) does not include it in the default set. Re-evaluate when per-column-description filtering becomes a product requirement. |
| `data[*].hit.modeling_layer` field | ❌ Not surfaced in the hit shape. The `?modeling_layer=` filter IS supported (see query parameters), but the value is server-computed from `original_file_path` at filter time and is not denormalized onto the hit. Consumers that need to display the layer on a result row should call `/api/v1/models/:id` or join the badge from existing model-list pagination. Mirrors the FE pattern: search results show `ResourceChip` (resource_type) only; layer-coloring lives on the model browse page. |

### Type definition

For codegen reference. The field reference table above is the authoritative contract.

```typescript
// Query parameters
interface SearchQueryParams {
  q?: string;                // absent or empty triggers browse mode (no text predicate)
  type?: string;             // comma-separated resource_type list
  package?: string;          // comma-separated package_name list
  tag?: string;              // comma-separated tag list
  modeling_layer?: string;   // comma-separated layer list (Staging|Intermediate|Marts);
                             // implicitly narrows to models since the dimension is model-only
  first?: number;            // default 50, max 200
  after?: string;            // opaque cursor
}

// Singular nouns; rendered verbatim by SearchResultItem.tsx as "Includes {matched_field}: ..."
type MatchedField = "name" | "description" | "tag" | "column" | "fqn";

// Discriminated union on hit.resource_type.
type SearchHit =
  | ModelHit
  | SourceHit
  | SeedHit
  | SnapshotHit
  | TestHit
  | UnitTestHit
  | ExposureHit
  | MetricHit
  | SemanticModelHit
  | SavedQueryHit
  | MacroHit
  | GroupHit;

interface SearchHitBase {
  unique_id: string;
  name: string | null;
  package_name: string | null;
}

// Runnable hits carry executed_at — null when dbt_rt.run_results has no row for this unique_id.
// Non-runnable hits (Exposure/Metric/SemanticModel/SavedQuery/Macro/Group) omit the field entirely (ADR-5).
interface RunnableHitMixin {
  executed_at: string | null;
}

interface ModelHit extends SearchHitBase, RunnableHitMixin {
  resource_type: "model";
  fqn: string[];
  materialized: string | null;
  access_level: string | null;
}

interface SourceHit extends SearchHitBase, RunnableHitMixin {
  resource_type: "source";
  fqn: string[];
  source_name: string | null;
  freshness_checked: boolean | null; // null when has_source_freshness is false
}

interface SeedHit extends SearchHitBase, RunnableHitMixin {
  resource_type: "seed";
  fqn: string[];
}

interface SnapshotHit extends SearchHitBase, RunnableHitMixin {
  resource_type: "snapshot";
  fqn: string[];
}

interface TestHit extends SearchHitBase, RunnableHitMixin {
  resource_type: "test";
  fqn: string[];
  test_type: "test";
}

interface UnitTestHit extends SearchHitBase, RunnableHitMixin {
  resource_type: "unit_test";
  fqn: string[];
  test_type: "unit_test";
}

interface ExposureHit extends SearchHitBase {
  resource_type: "exposure";
  fqn: string[];
  exposure_type: string | null;
}

interface MetricHit extends SearchHitBase {
  resource_type: "metric";
  fqn: string[];
}

interface SemanticModelHit extends SearchHitBase {
  resource_type: "semantic_model";
  fqn: string[];
}

interface SavedQueryHit extends SearchHitBase {
  resource_type: "saved_query";
  fqn: string[];
}

// Macro and group hits omit fqn — dbt.macros and dbt.groups parquet tables
// have no fqn column, and the UI shows "View lineage" only when fqn is defined.
interface MacroHit extends SearchHitBase {
  resource_type: "macro";
}

interface GroupHit extends SearchHitBase {
  resource_type: "group";
}

interface SearchEdge {
  // Both null in browse mode (empty/absent ?q=); both populated when ?q= is set.
  matched_field: MatchedField | null;
  highlight: string | null;
  hit: SearchHit;
}

// PageInfo is declared once in ADR-6 ("Shared TypeScript types") and reused by
// every LIST endpoint. Do not redeclare here; the search response uses the same
// type. ADR-6's shape:
//   interface PageInfo {
//     total_count: number;
//     start_cursor: string | null;
//     end_cursor: string | null;
//     has_next_page: boolean;
//   }

interface SearchResponse {
  data: SearchEdge[];
  page_info: PageInfo;
}
```

### Risk register

1. **No relevance ranking under DuckDB ILIKE.** DuckDB's `ILIKE` does not return match scores; the implementation uses a global `ORDER BY (name ASC, unique_id ASC)` over the UNION (Q-E6 option a; Q-E9 option a). This is a known downgrade from Discovery's OpenSearch-backed relevance. Promoting to a heuristic score (name-match > fqn-match > tag-match > description-match, alphabetical tie-break) is a future enhancement; Q-E6 option b. Defer until product feedback says alphabetical is insufficient. The contract states sort order explicitly so the UI and tests can rely on it.

2. **Description searches scan `dbt.nodes.description` directly — no `dbt.docs` join required.** The parquet sample shows `dbt.nodes.description` is populated inline (verified against `/Users/eddowh/codaz/sl-schema-evolution/sample_project/target/index/dbt.nodes.parquet`; descriptions present for `customers`, `enrollments`, `fct_orders`, `locations`). `dbt.docs.parquet` is the doc-block table (for `{{ doc() }}` references), not per-node descriptions. The handler does not join `dbt.docs` for resource description matching. Per-table description columns: `dbt.nodes.description`, `dbt.exposures.description`, `dbt.macros.description`, `dbt.metrics.description`, `dbt.saved_queries.description`, `dbt.semantic_models.description`, `dbt.groups.description`, `dbt.unit_tests.description`.

3. **`?q=` substring escaping is a server-side responsibility on every request.** `%` and `_` in user input are SQL wildcards under ILIKE. The handler MUST escape them (e.g., backslash-escape and pass `ESCAPE '\'`) before building the ILIKE predicate. This applies to every searchable field and every token. Parquet is read-only — there is no path to pre-sanitize input on ingest. Add a unit test for inputs like `100% pure`, `snake_case`, and Unicode lookalikes.

4. **Body search is deferred until `compiled_code`/`raw_code` are exposed on a detail endpoint.** Both are Class A in `dbt.nodes` parquet (VARCHAR columns; verified present) but no existing list or detail endpoint surfaces them today. Per the load-bearing invariant (searchable fields ⊆ exposed fields), they cannot be searchable until first exposed. Future path: (a) add `compiled_code`/`raw_code` to `GET /api/v1/models/:id`'s response in a separate PR, then (b) add a `has_body_search` capability flag and a `?include_body=true` parameter to `/search`. Not v0. The MCP tool `search_sql` is the only consumer currently asking for body search; it remains MCP-only and out of REST scope.

5. **Phrase and fielded queries are deferred.** No `"exact phrase"` (quoted-string) support, no `name:foo` (fielded) syntax. Multi-token query is whitespace-split AND. Document explicitly so future readers don't infer support from Discovery. Re-evaluate when product asks for advanced search; Q-E4.

6. **`meta` matching is deferred.** `meta` is a JSON-string column (CC-7) on `dbt.nodes` and most other resource parquet tables. Substring matching against opaque JSON needs a separate design (whole-blob ILIKE vs. JSON-path extraction). Excluded from v0; revisit when there's a concrete product requirement.

7. **Pagination consistency under writes is free here.** Cursor pagination is naturally stable because the parquet snapshot is immutable per server boot — no row can appear or disappear during a paged scan. This is a property of dbt-docs-server's snapshot model and is NOT guaranteed against a live database. Worth documenting so the assumption is not silently carried into future systems.

8. **Cursor encoding is opaque to clients but implementation-fixed.** Default global ORDER BY over the UNION (Q-E9 option a) means the cursor encodes `(name, unique_id)` of the last row. Switching to per-type pagination (Q-E9 option b — composite cursor with `resource_type` clustering) later would invalidate any in-flight cursors. The contract states cursors are opaque and clients MUST NOT parse them. The handler validates and rejects malformed cursors with 400; old clients holding stale cursors after a server upgrade get a clean error rather than silently-wrong results.

9. **Cross-endpoint `?q=` overlap is intentional.** `/api/v1/nodes?q=` and `/api/v1/models?q=` continue to exist with substring-filter semantics — narrow, flat-row, no highlight metadata. `/api/v1/search?q=` is the cross-resource find-anything surface with the envelope shape. The two are NOT merged (Q-C1 option a). Future readers may be tempted to deduplicate; document in code review that the divergence is the design. If consolidation is ever desired, route `/nodes?q=` through `/search` internally (Q-C1 option c) — but that is a deprecation-path change and out of scope for v0.

10. **`search_text` column on `dbt.nodes` is present but unpopulated in the sample index.** The parquet schema for `dbt.nodes` includes a `search_text varchar` column (verified at `/Users/eddowh/codaz/sl-schema-evolution/sample_project/target/index/dbt.nodes.parquet`). Sample values are `NULL` across all 38 rows. This appears to be reserved by dbt-index for a future denormalized full-text column. The handler MUST NOT depend on it being populated; if a future dbt-index version starts emitting `search_text` (a `[name, description, tags-joined, ...]` concatenation), the handler can opportunistically use it as a single-column ILIKE shortcut over the per-field UNION. Until then, the handler matches against the individual columns explicitly.

11. **Resource types whose parquet table lacks a `tags` column are silently excluded when `?tag=` is set.** Confirmed empirically: `dbt.macros.parquet` has no `tags` column; `dbt.groups.parquet` has no `tags` column; `dbt.unit_tests.parquet` has no `tags` column. Models, sources, seeds, tests, snapshots (all in `dbt.nodes`) have `tags: varchar[]`. Exposures, metrics, saved_queries have top-level `tags: varchar[]`. Semantic models surface `tags` via the `config` JSON blob (see the `/semantic_models/:id` contract's Risk #2 for the `json_extract(config, '$.tags')` pattern); the search handler must apply the same extraction for the `?tag=` filter. Document the silent exclusion explicitly so reviewers don't expect `?tag=foo` to return macros.

12. **Highlight extraction re-scans the matched field — DuckDB ILIKE returns no offsets.** For every row in the page, after the matcher decides which field "won" (priority `name > column > tag > fqn > description`), the handler re-runs a case-insensitive substring search on that field's value to locate the match position(s), then formats `highlight` per the per-field rules. This is two passes over the page's matched rows but only on fields the row already matched — the cost is bounded by `first` (default 50, max 200). Multi-token queries wrap each distinct matched token in its own `<b>...</b>` pair.

---

## ADR-9: Server-side analytics relay as the documented exception to the read-only GET pattern

> **Superseded by [ADR-10](#adr-10-browser-to-vortex-analytics-for-static-sites-supersedes-adr-9), and the endpoint it describes is deleted.** Kept for the
> reasoning, which ADR-10 answers point by point. `POST /api/v1/analytics/events` no
> longer exists; nor does any other `/api/v1/*` route.

**Status:** Decided — docs v2 emits product analytics through a server-side relay in this crate; the browser never talks to Vortex directly. (META-7739)
**Trigger to revisit:** A second write surface is proposed, OR the analytics volume/latency profile outgrows fire-and-forget and needs an ack/queue, OR the `docs.proto` contract moves into generated `proto-rust` types the CLI already links.

### Context

Every endpoint in this document until now is a read-only `GET` over an immutable parquet snapshot. dbt docs v2 needs product analytics (resource views, searches, lineage opens, upsell interactions). The open question was where the browser's events go and who owns consent + the ingest transport.

The events themselves are defined in `docs.proto` (`v1.public.events.docs`): 11 event messages sharing a `context` (anonymised dbt Cloud IDs) and an `is_logged_in` flag. Vortex is the ingest sink. Two in-repo siblings already emit to Vortex — `crates/dbt-index` (`telemetry.rs` + `vortex_sender.rs`) and `wizard/dbt-codex/codex-rs/vortex` — and both **deliberately** avoid `proto-rust` and the shared `vortex-client`, hand-rolling a self-contained sender and their wire structs with `#[derive(prost::Message)]`.

### Options considered

**(A) Browser → Vortex directly.**

The SPA POSTs protobuf/JSON straight to the Vortex ingest endpoint.

- Pro: no new server surface; least server code.
- **Con:** exposes the ingest URL and client platform header to the browser.
- **Con:** consent enforcement moves to the client, where it can be bypassed or drift from the server's `analytics_enabled` decision (`GET /api/v1/identity`).
- **Con:** PII/CORS risk — the browser would assemble the `context`, and cross-origin ingest needs CORS on Vortex.

→ **Rejected.** Consent and PII posture cannot be guaranteed from the browser.

**(B) Server-side relay, hand-rolled sender (dbt-index / codex-vortex precedent). — CHOSEN**

The SPA POSTs a batch to `POST /api/v1/analytics/events`; the handler enforces consent, maps each event to its concrete `docs.proto` wire type, and forwards it to Vortex via a self-contained `vortex_sender` ported from dbt-index.

- Pro: consent is enforced server-side, reusing the exact identity gate (`!do_not_track && send_anonymous_usage_stats`).
- Pro: the ingest URL and platform header never reach the browser.
- Pro: the server owns `context` assembly, so no email/PII can be placed on the wire by a client.
- Pro: matches two existing siblings; all new code is source-available-clean (only existing workspace deps: `prost`, `ureq`, `http`, `uuid`, `rand`).
- Con: introduces the first write endpoint — a documented exception to the read-only pattern.

→ **Chosen.** The relay is the only honest place to enforce consent and keep PII off the wire.

**(C) Relay via shared `proto-rust` + `vortex-client` codegen.**

Same relay shape as (B) but depends on the generated `proto-rust` types and the shared `vortex-client`.

- Pro: single source of truth for the wire types (generated from `docs.proto`).
- **Con:** heavier — drags in `cargo xtask protogen`, `protogen.toml`, and a `proto-rust`/`vortex-client` dependency; the `docs.*` types only exist after generating the proto, which is a separate PR's prerequisite (`docs.proto` lands via `dbt-labs/proto`).
- **Con:** diverges from the two siblings that deliberately hand-roll, for no benefit at this size (11 flat messages).

→ **Rejected for this PR.** Revisit if/when the CLI already links the generated `docs.*` types.

### Decision

`POST /api/v1/analytics/events` is the single write surface for docs v2 analytics and the **only documented exception** to the read-only GET pattern. It:

- Accepts a **batch** POST (`{ events: [...] }`); each event is internally tagged on `event_type`.
- Is **consent-gated**, reusing the identity `analytics_enabled` rule. When consent is off the handler is a no-op and returns `202 {"accepted":0,"skipped":N}` — never an error, so the client needs no consent branch.
- Is **fire-and-forget**: events are handed to a background worker (flushes ~every 500ms) and the handler returns `202` immediately. Analytics loss on Ctrl-C is acceptable; shutdown flush is out of scope (see the `TODO(META-7739)` in `server.rs`).
- Carries **no PII**: `context` holds anonymised dbt Cloud IDs only (no email); the server assembles the wire `context`.
- Hand-rolls the `docs.proto` wire structs in `src/handlers/analytics.rs` and ships a self-contained `src/vortex_sender.rs` (no `proto-rust`, no `vortex-client`).

The endpoint contract is at [`POST /api/v1/analytics/events`](#post-apiv1analyticsevents) below.

### Trigger conditions for revisiting

- A second write endpoint is proposed — re-examine whether "documented exception" should become a general write pattern.
- Fire-and-forget is insufficient (events must be acknowledged/durable) — the transport needs a queue or a synchronous ack.
- The generated `docs.*` types become available to the CLI without the protogen prerequisite — option (C) may then be cheaper than maintaining hand-rolled structs.

---

## `POST /api/v1/analytics/events`

Relays a batch of dbt docs v2 product-analytics events to Vortex. See [ADR-9](#adr-9-server-side-analytics-relay-as-the-documented-exception-to-the-read-only-get-pattern) for why this is a server-side relay and the only write endpoint.

The browser UI is the producer (item #3 of META-7739, separate PR). Events are hand-transcribed from `docs.proto` (`v1.public.events.docs`); enum-ish fields are lowercase dbt domain codes on the wire (`string`, per the proto's WIRE-VALUE CONTRACT).

### Request

`Content-Type: application/json`

```json
{
  "events": [
    {
      "event_type": "resource_viewed",
      "is_logged_in": true,
      "context": {
        "event_id": "b1c2…",
        "session_id": "s-abc",
        "dbt_cloud_account_id": "12345"
      },
      "resource_type": "model",
      "view_level": "detail",
      "resource_id": "model.jaffle_shop.customers"
    },
    {
      "event_type": "search_performed",
      "is_logged_in": false,
      "context": { "event_id": "d4e5…" },
      "search_query": "orders",
      "result_count": 7
    }
  ]
}
```

Each event is an object tagged by `event_type` (snake_case). `is_logged_in` and `context` are common to every event; the remaining fields are event-specific. All fields are optional and default to empty (`""` / `0` / `false`); `context` may be omitted entirely.

### Response

`202 Accepted` in all valid cases (fire-and-forget — a `202` does **not** guarantee delivery to Vortex):

```json
{ "accepted": 2, "skipped": 0 }
```

When analytics consent is off (`DO_NOT_TRACK=1` or `send_anonymous_usage_stats=false`), nothing is emitted and the batch is reported as skipped:

```json
{ "accepted": 0, "skipped": 2 }
```

`400 Bad Request` with the crate's `{code, message}` shape when the body is malformed or contains an unknown `event_type`:

```json
{ "code": "invalid_event", "message": "unknown variant `not_a_real_event`, …" }
```

### Event types & fields

Common to every event: `is_logged_in: bool`, `context: TelemetryContext`.

| `event_type` | Event-specific fields |
|---|---|
| `docs_site_opened` | `dbt_version: string`, `project_resource_count: int` |
| `resource_viewed` | `resource_type: string`, `view_level: string`, `resource_id: string` |
| `lineage_viewed` | `lineage_type: string`, `resource_type: string`, `resource_id: string` |
| `search_performed` | `search_query: string`, `result_count: int` |
| `filter_applied` | `resource_type: string`, `filter_type: string`, `filter_value: string` |
| `upsell_prompt_displayed` | `upsell_track: string`, `prompt_format: string`, `prompt_location: string` |
| `upsell_prompt_clicked` | `upsell_track: string`, `cta_label: string`, `referral_code: string` |
| `upsell_prompt_dismissed` | `upsell_track: string`, `dismiss_method: string` |
| `referral_link_clicked` | `referral_code: string`, `link_destination: string` |
| `account_logged_in` | `triggered_by_prompt: bool`, `upsell_track: string` |
| `error_encountered` | `error_category: string`, `error_message: string`, `field_name: string` |

`TelemetryContext` (all fields optional strings, anonymised — **no PII**): `event_id`, `feature`, `snowplow_domain_session_id`, `snowplow_domain_user_id`, `session_id`, `referrer_url`, `dbt_cloud_account_id`, `dbt_cloud_account_identifier`, `dbt_cloud_project_id`, `dbt_cloud_environment_id`, `dbt_cloud_user_id`.

### Type definition

```ts
interface TelemetryContext {
  event_id?: string;
  feature?: string;
  snowplow_domain_session_id?: string;
  snowplow_domain_user_id?: string;
  session_id?: string;
  referrer_url?: string;
  dbt_cloud_account_id?: string;
  dbt_cloud_account_identifier?: string;
  dbt_cloud_project_id?: string;
  dbt_cloud_environment_id?: string;
  dbt_cloud_user_id?: string;
}

interface EventBase {
  is_logged_in: boolean;
  context?: TelemetryContext;
}

type AnalyticsEvent =
  | (EventBase & { event_type: "docs_site_opened"; dbt_version?: string; project_resource_count?: number })
  | (EventBase & { event_type: "resource_viewed"; resource_type?: string; view_level?: string; resource_id?: string })
  | (EventBase & { event_type: "lineage_viewed"; lineage_type?: string; resource_type?: string; resource_id?: string })
  | (EventBase & { event_type: "search_performed"; search_query?: string; result_count?: number })
  | (EventBase & { event_type: "filter_applied"; resource_type?: string; filter_type?: string; filter_value?: string })
  | (EventBase & { event_type: "upsell_prompt_displayed"; upsell_track?: string; prompt_format?: string; prompt_location?: string })
  | (EventBase & { event_type: "upsell_prompt_clicked"; upsell_track?: string; cta_label?: string; referral_code?: string })
  | (EventBase & { event_type: "upsell_prompt_dismissed"; upsell_track?: string; dismiss_method?: string })
  | (EventBase & { event_type: "referral_link_clicked"; referral_code?: string; link_destination?: string })
  | (EventBase & { event_type: "account_logged_in"; triggered_by_prompt?: boolean; upsell_track?: string })
  | (EventBase & { event_type: "error_encountered"; error_category?: string; error_message?: string; field_name?: string });

interface AnalyticsBatchRequest {
  events: AnalyticsEvent[];
}

interface AnalyticsRelayResponse {
  accepted: number;
  skipped: number;
}
```

### Risk register

1. **A `202` is not a delivery guarantee.** Events are queued to a background worker and the response returns immediately. On process exit before the ~500ms flush, buffered events are lost. This is acceptable for product analytics; if durability is ever required, ADR-9's revisit trigger applies.
2. **Consent is enforced only server-side.** The client receives `202 {"accepted":0}` when consent is off and cannot tell "emitted" from "skipped" except via the `skipped` count. That is intentional — clients must not branch on consent; the server is the single authority (reuses `GET /api/v1/identity`'s rule).
3. **Wire types are hand-transcribed from `docs.proto`.** Field numbers/types are duplicated in `src/handlers/analytics.rs`. If `docs.proto` changes, this crate must be updated by hand (same maintenance posture as dbt-index and codex-vortex). The `enrichment` field (tag 1) is intentionally omitted — producers set it None and omitting it encodes identically.
4. **Unknown `event_type` fails the whole batch.** Deserialization is all-or-nothing: one unknown/malformed event returns `400 invalid_event` and nothing in the batch is emitted. This keeps the contract strict; if partial acceptance is ever wanted, switch to per-event deserialization with a per-event error list.

---

## ADR-10: Browser-to-Vortex analytics for static sites (supersedes ADR-9)

**Status:** Decided. Supersedes [ADR-9](#adr-9-server-side-analytics-relay-as-the-documented-exception-to-the-read-only-get-pattern) for the static-site distribution.

### Context

ADR-9 made `POST /api/v1/analytics/events` the single write surface and explicitly
rejected browser-to-Vortex on three grounds: consent enforcement, PII containment, and
CORS. That reasoning held while docs v2 was served by a process.

`dbt docs generate` produces a directory of static files with no server. There is no
relay to POST to, so ADR-9's mechanism is not available — the choice is browser-direct
or no analytics at all.

### Decision

The browser posts events to the Vortex collector. Each of ADR-9's three objections, re-examined:

**Consent.** Resolved at export time and inlined into `window.__DBT_DOCS__`. This is
not a weakening — it is the only place the question *can* be answered, because consent
lives in the project and profile, which only the machine running `dbt docs generate`
can read. A visitor's browser has nothing to ask. When consent is denied the producer
is configured `enabled: false`, which makes it drop every `logProto` and `flush`
silently, so no call site can leak through by forgetting to check.

**CORS.** Verified against the collector rather than assumed; no infrastructure change
was required.

**Event contents.** Unchanged, and deliberately so: the same eight events the relay
forwarded, carrying the same fields. The transport moved; the schema did not. Of ADR-9's
three objections this is the one that still has force, and it is tracked outside this
document.

Fields the relay hydrated server-side (`distribution`, `dbt_version`, `is_logged_in`,
and the `dbt_cloud_*` context ids) are baked into the bootstrap at export time from the
same `DistInfoProvider::telemetry_hydration()` seam that fed the relay.

### Options considered

1. **Drop analytics from static sites.** Simplest and safest, but leaves us unable to
   tell whether the feature is used at all and if it is, how we can improve it
2. **Keep a relay, require a server.** Reintroduces the exact dependency users told us
   was a blocker, for telemetry alone.
3. **Browser-direct (chosen).** Costs the properties analysed above; keeps the events
   the relay already sent.

### What is no longer true

- The ingest URL is now in the shipped bundle. No credential ships with it — there was
  never one to ship — but ADR-9's "keep the ingest URL off the browser" goal is
  abandoned rather than met.
- Field authority is weaker. The relay ignored client-supplied values for the hydrated
  fields; now they come from a bootstrap that whoever hosts the site can edit. Treat the
  hydrated fields as self-reported rather than authoritative.

### Risk register

1. **The event schema is carried over unchanged, and is reviewed separately.** This ADR
   moved the transport only. Whether the current fields are the right ones to send from
   a published site is a schema question, tracked outside this document and not resolved
   here.
2. **Consent is baked at generate time, not read at view time.** A site generated
   under consent keeps sending until it is regenerated. Revoking consent requires a
   rebuild. Acceptable for a static artifact; there is no mechanism that could do
   better without a server. Furthermore, most users will self-host, enabling them to control consent.
3. **No delivery guarantee.** Batching is off (`maxBatchBytes: -1`) so each event is
   sent as it is logged, but a tab closing mid-flight still loses it. Same posture as
   ADR-9's `202`.

### Revisit when

- The event schema review in risk 1 concludes, which may narrow what is sent.
- A served distribution needs analytics again, in which case `telemetry.ts` grows a
  second sink alongside this one.
