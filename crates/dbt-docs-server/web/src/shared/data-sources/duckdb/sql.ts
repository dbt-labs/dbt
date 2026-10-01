/**
 * SQL the browser runs against the site's parquet.
 *
 * Written against the **dbt information schema** — `dbt.models`, `dbt.seeds`,
 * `dbt.docs_blocks`, `dbt_rt.freshness`, and the `dbt_internal.resources` union
 * for the surfaces that span resource types. The relations themselves are defined
 * by the `views.sql` that ships beside the parquet, so nothing here declares a
 * view; these are queries over what that document registered.
 *
 * Every projection keeps its snake_case output aliases even where the source
 * column was renamed (`access AS access_level`, `data_type_declared AS
 * declared_type`, …). That is deliberate and load-bearing: `fromWire.ts` reads
 * those names, the mappers are the contract, and renaming an output column here
 * silently nulls a field rather than failing.
 *
 * What the REST era left behind: cursor envelopes, `total_count` round-trips, and
 * the two-to-four "view might be missing" variants per handler. The information
 * schema writes every table even at zero rows, so a single query per surface is
 * enough.
 */

/** Literal-escape a string for interpolation, mirroring the Rust `escape_str`. */
export function sqlStr(value: string): string {
  return `'${value.replace(/'/g, "''")}'`;
}

/**
 * One row of project identity.
 *
 * `project_id` and `description` are not in the information schema — nothing ever
 * set either — so they are projected as NULL to keep `RestProject`'s shape rather
 * than dropped, which would make the mapper read `undefined` for a field it
 * declares as nullable.
 */
export const PROJECT_SQL = `
SELECT project_name AS name,
       NULL AS project_id,
       NULL AS description,
       dbt_version,
       adapter_type,
       git_sha,
       git_branch,
       git_uncommitted_changes AS git_is_dirty
FROM dbt.project
LIMIT 1`;

export const PROJECT_TABLES = ['dbt.project'];

/**
 * The winning `{% docs __overview__ %}` block, or no row at all.
 *
 * Parity with dbt-docs v1, whose `OverviewCtrl` seeds from `doc.dbt.__overview__`
 * and then lets any doc named `__overview__` from a non-`dbt` package override it.
 * Three things here are load-bearing:
 *
 * - **The default is excluded, so "no authored overview" returns zero rows.** That
 *   is the signal the page falls back to its bundled default on. v1's built-in
 *   default described v1's own `Project`/`Database` tabs, so surfacing it here
 *   would be actively wrong.
 * - **We discriminate on `unique_id`, not `package_name <> 'dbt'`.** The ingest only
 *   started populating `package_name` recently; on an older index it is NULL
 *   everywhere, and `NULL <> 'dbt'` is NULL, which would silently drop every row —
 *   including a user's own overview. `unique_id` is non-null in every index ever
 *   written, and is exactly equivalent, since it is `doc.{package}.{name}` and no
 *   project may be named `dbt`.
 * - **Root project first, then package name.** v1 broke ties by manifest insertion
 *   order, which is filesystem-dependent and not reproducible here; this matches v1
 *   for any single-project case and is at least deterministic beyond it. `CASE WHEN`
 *   rather than `(package_name = x) DESC` keeps a NULL `package_name` in the ELSE
 *   branch with no NULLS FIRST/LAST subtlety, and `unique_id` makes the order total.
 */
export function overviewSql(rootPackage: string): string {
  return `
SELECT unique_id, package_name, content AS block_contents
FROM dbt.docs_blocks
WHERE name = '__overview__'
  AND unique_id <> 'doc.dbt.__overview__'
  AND content IS NOT NULL
  AND length(trim(content)) > 0
ORDER BY CASE WHEN package_name = ${sqlStr(rootPackage)} THEN 0 ELSE 1 END,
         package_name,
         unique_id
LIMIT 1`;
}

export const OVERVIEW_TABLES = ['dbt.docs_blocks'];

/**
 * Per-resource-type tallies, for the sidebar and the overview page.
 *
 * Counted over `dbt_internal.resources` rather than `dbt.dag_nodes`, which is the
 * obvious-looking choice and the wrong one: `dag_nodes` keeps only *enabled* rows
 * whose type participates in the DAG, so hooks and disabled resources would go
 * missing and five of these side tables have no rows there at all. The union is
 * every resource, which is what a count of resources has to be.
 *
 * The types with their own table are counted separately and summed. `unit_test`
 * folds into `test` in the mapper rather than here, since the two render on one
 * page.
 */
export const COUNTS_SQL = `
WITH raw AS (
  SELECT resource_type, COUNT(*) AS count FROM dbt_internal.resources GROUP BY resource_type
  UNION ALL SELECT 'unit_test',      COUNT(*) FROM dbt.unit_tests
  UNION ALL SELECT 'exposure',       COUNT(*) FROM dbt.exposures
  UNION ALL SELECT 'group',          COUNT(*) FROM dbt.groups
  UNION ALL SELECT 'macro',          COUNT(*) FROM dbt.macros
  UNION ALL SELECT 'metric',         COUNT(*) FROM dbt.metrics
  UNION ALL SELECT 'saved_query',    COUNT(*) FROM dbt.saved_queries
  UNION ALL SELECT 'semantic_model', COUNT(*) FROM dbt.semantic_models
)
SELECT resource_type, CAST(SUM(count) AS BIGINT) AS count
FROM raw
GROUP BY resource_type
ORDER BY resource_type`;

export const COUNTS_TABLES = [
  'dbt_internal.resources',
  'dbt.unit_tests',
  'dbt.exposures',
  'dbt.groups',
  'dbt.macros',
  'dbt.metrics',
  'dbt.saved_queries',
  'dbt.semantic_models',
];

/**
 * Every file-bearing resource, unpaginated — the sidebar's file tree.
 *
 * `patch_path` is the mapper's name for what the information schema calls
 * `properties_yml_file_path`, and it is only real for resources and macros.
 * `dbt.semantic_models` needs the null filter because its rows can lack a file
 * path entirely.
 */
export const FILES_SQL = `
SELECT unique_id, name, resource_type, package_name, original_file_path,
       properties_yml_file_path AS patch_path
FROM dbt_internal.resources
UNION ALL
SELECT unique_id, name, 'exposure', package_name, original_file_path, NULL
FROM dbt.exposures
UNION ALL
SELECT unique_id, name, 'metric', package_name, original_file_path, NULL
FROM dbt.metrics
UNION ALL
SELECT unique_id, name, 'macro', package_name, original_file_path,
       properties_yml_file_path
FROM dbt.macros
UNION ALL
SELECT unique_id, name, 'semantic_model', package_name, original_file_path, NULL
FROM dbt.semantic_models
WHERE original_file_path IS NOT NULL
UNION ALL
SELECT unique_id, name, 'group', package_name, original_file_path, NULL
FROM dbt.groups
UNION ALL
SELECT unique_id, name, 'unit_test', package_name, original_file_path, NULL
FROM dbt.unit_tests
UNION ALL
SELECT unique_id, name, 'saved_query', package_name, original_file_path, NULL
FROM dbt.saved_queries
ORDER BY original_file_path, name`;

export const FILES_TABLES = [
  'dbt_internal.resources',
  'dbt.exposures',
  'dbt.metrics',
  'dbt.macros',
  'dbt.semantic_models',
  'dbt.groups',
  'dbt.unit_tests',
  'dbt.saved_queries',
];

/**
 * Both edge directions for one node, shaped as `RestEdgeRef` rows.
 *
 * `edge_type` is projected as the constant `'ref'`: the information schema does
 * not publish it, and `RestEdgeRef` declares it. `'ref'` rather than NULL because
 * that is what the synthesized saved-query edges already use, so every edge the
 * UI sees carries one value rather than sometimes none.
 */
export function nodeEdgesSql(uniqueId: string): string {
  const id = sqlStr(uniqueId);
  return `
SELECT 'depends_on' AS direction, parent_unique_id AS unique_id, 'ref' AS edge_type
FROM dbt.edges
WHERE child_unique_id = ${id}
UNION ALL
SELECT 'referenced_by', child_unique_id, 'ref'
FROM dbt.edges
WHERE parent_unique_id = ${id}`;
}

export const NODE_EDGES_TABLES = ['dbt.edges'];

/**
 * `handlers/lineage.rs` — the depth cap, which is hard rather than a default.
 *
 * The Rust handler refused a larger `?max_depth`, so keeping it fixed here holds
 * graph size (and DAG render cost) to what the UI was built for.
 */
export const LINEAGE_MAX_DEPTH = 3;

/** The two recursive walks both lineage queries share. */
function lineageWalks(uniqueId: string, maxDepth: number): string {
  const id = sqlStr(uniqueId);
  return `
WITH RECURSIVE
upstream AS (
  SELECT parent_unique_id AS unique_id, -1 AS depth
  FROM dbt.edges WHERE child_unique_id = ${id}
  UNION ALL
  SELECT e.parent_unique_id, u.depth - 1
  FROM dbt.edges e JOIN upstream u ON e.child_unique_id = u.unique_id
  WHERE u.depth > ${-maxDepth}
),
downstream AS (
  SELECT child_unique_id AS unique_id, 1 AS depth
  FROM dbt.edges WHERE parent_unique_id = ${id}
  UNION ALL
  SELECT e.child_unique_id, d.depth + 1
  FROM dbt.edges e JOIN downstream d ON e.parent_unique_id = d.unique_id
  WHERE d.depth < ${maxDepth}
)`;
}

/**
 * Nodes within `maxDepth` of the root, with a signed depth (negative upstream,
 * 0 root, positive downstream).
 *
 * The `metadata` CTE unions the three resource tables that appear in `dbt.edges`
 * but not in the resource union — without them a metric or exposure in the graph
 * would resolve to no name and drop out of the join.
 */
export function lineageNodesSql(
  uniqueId: string,
  maxDepth = LINEAGE_MAX_DEPTH,
): string {
  const id = sqlStr(uniqueId);
  return `${lineageWalks(uniqueId, maxDepth)},
all_ids AS (
  SELECT ${id} AS unique_id, 0 AS depth
  UNION ALL
  SELECT unique_id, MIN(depth) FROM upstream GROUP BY unique_id
  UNION ALL
  SELECT unique_id, MAX(depth) FROM downstream GROUP BY unique_id
),
metadata AS (
  SELECT unique_id, name, resource_type, materialized, original_file_path
  FROM dbt_internal.resources
  UNION ALL
  SELECT unique_id, name, 'metric', NULL, NULL FROM dbt.metrics
  UNION ALL
  SELECT unique_id, name, 'semantic_model', NULL, NULL FROM dbt.semantic_models
  UNION ALL
  SELECT unique_id, name, 'exposure', NULL, NULL FROM dbt.exposures
)
SELECT m.unique_id, m.name, m.resource_type, m.materialized, m.original_file_path, MIN(a.depth) AS depth
FROM all_ids a JOIN metadata m ON m.unique_id = a.unique_id
GROUP BY m.unique_id, m.name, m.resource_type, m.materialized, m.original_file_path
ORDER BY depth, m.resource_type, m.name`;
}

/** Every edge whose both endpoints fall inside the subgraph. */
export function lineageEdgesSql(
  uniqueId: string,
  maxDepth = LINEAGE_MAX_DEPTH,
): string {
  const id = sqlStr(uniqueId);
  return `${lineageWalks(uniqueId, maxDepth)},
all_ids AS (
  SELECT ${id} AS unique_id
  UNION
  SELECT unique_id FROM upstream
  UNION
  SELECT unique_id FROM downstream
)
SELECT e.parent_unique_id AS from_id, e.child_unique_id AS to_id, 'ref' AS edge_type
FROM dbt.edges e
WHERE e.parent_unique_id IN (SELECT unique_id FROM all_ids)
  AND e.child_unique_id IN (SELECT unique_id FROM all_ids)
ORDER BY e.parent_unique_id, e.child_unique_id`;
}

export const LINEAGE_TABLES = [
  'dbt.edges',
  'dbt_internal.resources',
  'dbt.metrics',
  'dbt.semantic_models',
  'dbt.exposures',
];

/** `dbt.saved_queries.depends_on` for the saved-query lineage special case. */
export function savedQueryDependsOnSql(uniqueId: string): string {
  return `
SELECT depends_on AS depends_on_nodes
FROM dbt.saved_queries
WHERE unique_id = ${sqlStr(uniqueId)}
LIMIT 1`;
}

export const SAVED_QUERY_TABLES = ['dbt.saved_queries'];

/**
 * Single-hop column lineage touching one node, optionally one column.
 *
 * One row per edge touching the node, in either direction. Column names are
 * lowercased on both sides because the index stores them as the warehouse
 * reported them, while the UI addresses them in lowercase.
 */
export function columnLineageSql(uniqueId: string, column?: string): string {
  const id = sqlStr(uniqueId);
  const child = column
    ? `child_node_unique_id = ${id} AND LOWER(child_column_name) = ${sqlStr(column.toLowerCase())}`
    : `child_node_unique_id = ${id}`;
  const parent = column
    ? `parent_node_unique_id = ${id} AND LOWER(parent_column_name) = ${sqlStr(column.toLowerCase())}`
    : `parent_node_unique_id = ${id}`;

  // `parent`/`child` in the information schema, `from`/`to` in the UI's
  // vocabulary. The aliases do the translation so the mapper and the graph
  // component are untouched.
  return `
SELECT parent_node_unique_id AS from_node,
       LOWER(parent_column_name) AS from_column,
       child_node_unique_id AS to_node,
       LOWER(child_column_name) AS to_column,
       evolution AS lineage_kind
FROM dbt.column_lineage
WHERE (${child}) OR (${parent})`;
}

export const COLUMN_LINEAGE_TABLES = ['dbt.column_lineage'];

/**
 * Normalize the raw `evolution` value to the vocabulary the UI renders.
 *
 * An unrecognized value passes through rather than being dropped, so a new kind
 * shows up as itself instead of vanishing.
 */
export function normalizeLineageKind(raw: string): string {
  switch (raw) {
    case 'copy':
      return 'passthrough';
    case 'mod':
      return 'transform';
    case 'scan':
      return 'indirect';
    default:
      return raw;
  }
}
