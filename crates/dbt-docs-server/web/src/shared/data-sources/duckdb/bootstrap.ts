/**
 * First-paint data, read without DuckDB.
 *
 * DuckDB-WASM is ~6.8 MB brotli on a cold cache. Waiting for it before rendering
 * anything would be a visible regression against the REST app, so the shell,
 * sidebar and home page come from a handful of small artifacts read with
 * hyparquet — a ~88 KB pure-JS parquet reader — while the engine streams in behind
 * them.
 *
 * This is the one place that cannot use `views.sql`: there is no DuckDB here, so
 * there is no SQL and no `dbt_internal.resources` to read. The resource union has
 * to be done by reading each resource-type artifact and concatenating, which is
 * why {@link RESOURCE_TABLES} exists — the only place in this app that still names
 * information-schema tables in TypeScript. It is a list of *files*, not a schema:
 * the columns come from `NodeSummary`, and a resource type missing from it shows up
 * as an empty sidebar section rather than as wrong data.
 *
 * Only the nine `NodeSummary` columns the shell renders are decoded (`columns`
 * below); each file still crosses the wire whole, because artifacts are fetched
 * whole rather than by range.
 *
 * Everything past first paint — detail pages, lists, search, lineage — goes
 * through DuckDB.
 */

import type { AsyncBuffer } from 'hyparquet';
import { parquetReadObjects } from 'hyparquet';
import { compressors } from 'hyparquet-compressors';

import type { NodeSummary } from '../../../types';
import type { RestProject } from '../mappers/fromWire';

/**
 * The artifacts holding the shell's resource list, in the order they are listed.
 *
 * Every table `dbt_internal.resources` unions, plus `dbt.unit_tests` — which the
 * union leaves out (it is not node-backed) but the sidebar counts under `test`.
 * `dbt.checks` is absent deliberately: checks are project-quality rules, not
 * resources the docs site has a page for.
 */
const RESOURCE_TABLES = [
  'dbt.models',
  'dbt.seeds',
  'dbt.snapshots',
  'dbt.sources',
  'dbt.analyses',
  'dbt.functions',
  'dbt.hooks',
  'dbt.data_tests',
  'dbt.unit_tests',
];

/**
 * The columns first paint decodes out of each resource artifact.
 *
 * Parquet is columnar, so naming them keeps the code blobs from being decompressed
 * here even though they arrive in the same file.
 *
 * `dbt.data_tests` and `dbt.unit_tests` do not carry all nine — a unit test has no
 * `materialized`, `database_name` or `schema_name`, and neither table declares
 * `resource_type`, since the table *is* the type. hyparquet fails a read that names
 * an absent column, so each artifact is read with the columns it actually has and
 * the rest are filled in.
 */
const NODE_SUMMARY_COLUMNS = [
  'unique_id',
  'name',
  'resource_type',
  'package_name',
  'materialized',
  'description',
  'database_name',
  'schema_name',
  'original_file_path',
];
/** Single-row convenience: project identity and the parse stamp. */
const PROJECT = 'dbt.project';

export interface BootstrapData {
  /** Every node in the project, shaped exactly like the former `GET /api/v1/nodes`. */
  nodes: NodeSummary[];
  /** Project identity, or `null` when `dbt.project` is empty. */
  project: RestProject | null;
  /** When the project was last fully parsed (RFC3339), or `null` if unstamped. */
  generation: string | null;
}

/**
 * Read the first-paint slice.
 *
 * Fetches every artifact in parallel. A resource artifact that cannot be read is
 * skipped rather than fatal: a project with no snapshots still has a
 * `dbt.snapshots.parquet` — the information schema writes every table — but a
 * partially copied site should still render what it does have, and every resource
 * type failing shows up as an empty shell either way. `dbt.project` is a
 * single-row convenience and a site is usable without it.
 *
 * The parse stamp comes from `dbt.project.last_full_parse_at`, which absorbed the
 * index's single-row `dbt.generation` table.
 */
export async function readBootstrap(dataBaseUrl: string): Promise<BootstrapData> {
  const [resources, project] = await Promise.all([
    Promise.all(
      RESOURCE_TABLES.map((table) =>
        readRows(dataBaseUrl, table, columnsFor(table)).catch(() => []),
      ),
    ),
    readRows(dataBaseUrl, PROJECT).catch(() => []),
  ]);

  return {
    nodes: RESOURCE_TABLES.flatMap((table, i) =>
      (resources[i] ?? []).map((row) => toNodeSummary(row, table)),
    ),
    project: toRestProject(project[0]),
    generation: asIsoString(project[0]?.last_full_parse_at) ?? null,
  };
}

/**
 * Resource types whose table does not declare `resource_type`, and the value to
 * fill in. The table *is* the type for these two, so the column would be a
 * constant column — but the shell groups by it, so something has to supply it.
 */
const IMPLIED_RESOURCE_TYPE: Record<string, string> = {
  'dbt.data_tests': 'test',
  'dbt.unit_tests': 'unit_test',
};

/** The subset of {@link NODE_SUMMARY_COLUMNS} one artifact actually declares. */
function columnsFor(table: string): string[] {
  const implied = IMPLIED_RESOURCE_TYPE[table];
  if (!implied) return NODE_SUMMARY_COLUMNS;
  const absent =
    table === 'dbt.unit_tests'
      ? ['resource_type', 'materialized', 'database_name', 'schema_name']
      : ['resource_type'];
  return NODE_SUMMARY_COLUMNS.filter((c) => !absent.includes(c));
}

/**
 * One row as the shell wants it.
 *
 * The information schema's column names are already the `NodeSummary` field names,
 * so this only has to supply what the artifact does not carry.
 */
function toNodeSummary(row: Record<string, unknown>, table: string): NodeSummary {
  const implied = IMPLIED_RESOURCE_TYPE[table];
  return (implied ? { ...row, resource_type: implied } : row) as unknown as NodeSummary;
}

/**
 * Shape a `dbt.project` row the way `fromProject` reads it.
 *
 * Two columns need renaming, and both fail silently if missed: the table calls the
 * name `project_name`, and `git_is_dirty` is `git_uncommitted_changes` in the
 * information schema. Without the first the project renders nameless; without the
 * second the dirty-worktree marker never shows.
 *
 * Written field by field rather than spread, so the row's other columns —
 * `schema_version`, `quoting`, `last_full_parse_at` — stay out of the domain
 * object instead of riding along untyped.
 */
function toRestProject(row: Record<string, unknown> | undefined): RestProject | null {
  if (!row) return null;
  const str = (value: unknown) => (typeof value === 'string' ? value : undefined);
  return {
    name: str(row.project_name) ?? '',
    dbt_version: str(row.dbt_version),
    adapter_type: str(row.adapter_type),
    git_sha: str(row.git_sha) ?? null,
    git_branch: str(row.git_branch) ?? null,
    git_is_dirty:
      typeof row.git_uncommitted_changes === 'boolean'
        ? row.git_uncommitted_changes
        : null,
  };
}

/** Read every row of one artifact. */
async function readRows(
  dataBaseUrl: string,
  table: string,
  columns?: string[],
): Promise<Record<string, unknown>[]> {
  const url = new URL(`${table}.parquet`, dataBaseUrl).href;
  const file = await wholeFile(url);
  return parquetReadObjects({
    file,
    // The index is written with ZSTD, which is not one of hyparquet's built-ins.
    compressors,
    rowFormat: 'object',
    ...(columns ? { columns } : {}),
  });
}

/**
 * Fetch a whole parquet file as an `AsyncBuffer`.
 *
 * Deliberately not `asyncBufferFromUrl`, which does a HEAD then byte-range reads.
 * These artifacts are small and read in full, so one request beats several — and
 * it keeps the bootstrap off range requests, which not every static host serves
 * reliably.
 */
async function wholeFile(url: string): Promise<AsyncBuffer> {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`${res.status} ${res.statusText} fetching ${url}`);
  }
  const bytes = await res.arrayBuffer();
  return {
    byteLength: bytes.byteLength,
    slice: (start: number, end?: number) => bytes.slice(start, end),
  };
}

/**
 * Coerce a parquet timestamp to RFC3339.
 *
 * `last_full_parse_at` is `TIMESTAMP(µs, UTC)`, which hyparquet may surface as a `Date`,
 * a number of milliseconds, or a `BigInt` of microseconds depending on how it
 * decodes the logical type. This is a staleness label, so an unrecognized shape
 * degrades to "unknown" rather than throwing.
 */
function asIsoString(value: unknown): string | null {
  if (value instanceof Date) return value.toISOString();
  if (typeof value === 'number') return new Date(value).toISOString();
  if (typeof value === 'bigint') return new Date(Number(value / 1000n)).toISOString();
  if (typeof value === 'string') return value;
  return null;
}
