/**
 * Cross-type search.
 *
 * Ported from `handlers/search.rs`, the largest surface in the API and the one with
 * the most behavior that is not expressible as "select some columns". Three parts:
 *
 * 1. **A base union** over every searchable artifact, each branch projecting the
 *    same fifteen columns so the rest of the query does not care which table a row
 *    came from.
 * 2. **Field matching**, one `ILIKE` leg per searchable field with a fixed priority
 *    (name < column < tag < fqn < description). Multi-token queries `INTERSECT` on
 *    `unique_id`, so every token must match somewhere — the tokens are ANDed, not
 *    ORed.
 * 3. **Ranking**, packed into one lexicographically-sortable string so a single sort
 *    key drives the order. Exact name match outranks resource type here,
 *    deliberately: when reading docs, the name the user typed matters more than the
 *    type hierarchy.
 *
 * Highlighting stays where it was — after the query, in code — because it needs to
 * window a long description around the match, and SQL is a poor place for that.
 *
 * Not carried over: the three "view might be missing" variants (`full`,
 * `no_freshness`, `no_rr`) the handler probed in order. The information schema
 * writes every table even at zero rows, so both `LEFT JOIN`s below read as nulls
 * the same way the `full` variant did, with nothing to probe. Also not carried
 * over: the N+1 query the handler issued per column-matched row to find *which*
 * column matched. That is one join here.
 */

import type { ResourceType } from '../../typings/domain/asset';
import { sqlStr } from './sql';

/** Matches the handler's caps. A longer query is a mistake, not a search. */
export const MAX_QUERY_LENGTH = 1024;
export const DEFAULT_PAGE_SIZE = 50;
export const MAX_PAGE_SIZE = 200;

/** Types the search union can reach, and where each lives. */
const NODE_TYPES: ResourceType[] = ['model', 'source', 'seed', 'snapshot', 'test'];

/**
 * Escape a value for `ILIKE`, including the wildcards themselves.
 *
 * Without this a query of `%` matches everything and `_` matches any character —
 * the user typed them as literals. Mirrors the handler's `escape_ilike`, and the
 * queries declare `ESCAPE '\'` to match.
 */
export function escapeIlike(value: string): string {
  return value.replace(/([\\%_])/g, '\\$1').replace(/'/g, "''");
}

/** Split a query into tokens. Every token must match for a row to qualify. */
export function tokenize(query: string): string[] {
  return query.trim().split(/\s+/).filter(Boolean);
}

/**
 * The fifteen columns every branch of the union projects.
 *
 * Reads `dbt_internal.resources`, the one surface here that genuinely spans
 * resource types — the alternative is a seven-way union of the typed tables, which
 * would read the same artifacts and restate the projection seven times.
 *
 * `unit_test` is not among {@link NODE_TYPES} and has its own branch, so the
 * `test_type` case only ever needs to recognize `test`.
 */
const NODES_BRANCH = `
SELECT n.unique_id, n.name, n.resource_type, n.package_name,
       n.fqn, n.tags, n.description,
       n.materialized, n.access AS access_level, n.source_name,
       sf.unique_id IS NOT NULL AS freshness_checked,
       CASE WHEN n.resource_type = 'test' THEN 'test' ELSE NULL END AS test_type,
       NULL::VARCHAR AS exposure_type,
       CAST(rr.created_at AS VARCHAR) AS executed_at,
       n.original_file_path
FROM dbt_internal.resources n
LEFT JOIN dbt_rt.freshness sf ON sf.unique_id = n.unique_id
LEFT JOIN dbt_rt.run_results_latest rr ON rr.unique_id = n.unique_id
WHERE n.resource_type IN (${NODE_TYPES.map(sqlStr).join(', ')})`;

/** One branch for a type with its own artifact; the padding keeps the shape. */
function ownBranch(
  table: string,
  resourceType: string,
  opts: {
    alias?: string;
    fqn?: string;
    tags?: string;
    description?: string;
    exposureType?: string;
    filePath?: string;
  } = {},
): string {
  const a = opts.alias ?? 'x';
  return `
SELECT ${a}.unique_id, ${a}.name, '${resourceType}'::VARCHAR AS resource_type, ${a}.package_name,
       ${opts.fqn ?? 'NULL::VARCHAR[]'} AS fqn,
       ${opts.tags ?? 'NULL::VARCHAR[]'} AS tags,
       ${opts.description ?? 'NULL::VARCHAR'} AS description,
       NULL::VARCHAR AS materialized, NULL::VARCHAR AS access_level,
       NULL::VARCHAR AS source_name, NULL::BOOLEAN AS freshness_checked,
       ${resourceType === 'unit_test' ? "'unit_test'::VARCHAR" : 'NULL::VARCHAR'} AS test_type,
       ${opts.exposureType ?? 'NULL::VARCHAR'} AS exposure_type,
       NULL::VARCHAR AS executed_at,
       ${opts.filePath ?? `${a}.original_file_path`}
FROM ${table} ${a}`;
}

/** Every branch, keyed so a type filter can select a subset. */
const BRANCHES: Partial<Record<ResourceType, string>> = {
  exposure: ownBranch('dbt.exposures', 'exposure', {
    fqn: 'x.fqn',
    tags: 'x.tags',
    description: 'x.description',
    exposureType: 'x.exposure_type',
  }),
  macro: ownBranch('dbt.macros', 'macro', { description: 'x.description' }),
  metric: ownBranch('dbt.metrics', 'metric', {
    fqn: 'x.fqn',
    tags: 'x.tags',
    description: 'x.description',
  }),
  saved_query: ownBranch('dbt.saved_queries', 'saved_query', {
    fqn: 'x.fqn',
    tags: 'x.tags',
    description: 'x.description',
  }),
  semantic_model: ownBranch('dbt.semantic_models', 'semantic_model', {
    fqn: 'x.fqn',
    // Semantic models keep tags inside `config` rather than in a column of their own.
    tags: "CAST(json_extract(x.config, '$.tags') AS VARCHAR[])",
    description: 'x.description',
  }),
  group: ownBranch('dbt.groups', 'group', { description: 'x.description' }),
  unit_test: ownBranch('dbt.unit_tests', 'unit_test', { description: 'x.description' }),
};

/** Artifacts the union reads. Every branch is registered whether filtered or not. */
export const SEARCH_TABLES = [
  'dbt_internal.resources',
  'dbt.node_columns',
  'dbt_rt.freshness',
  'dbt_rt.run_results_latest',
  'dbt.exposures',
  'dbt.macros',
  'dbt.metrics',
  'dbt.saved_queries',
  'dbt.semantic_models',
  'dbt.groups',
  'dbt.unit_tests',
];

/** Build the `base` CTE for the requested types, or `null` if none are searchable. */
function baseUnion(types: ResourceType[] | undefined): string | null {
  const wanted = types?.length ? types : undefined;
  const branches: string[] = [];

  const nodeTypes = wanted ? wanted.filter((t) => NODE_TYPES.includes(t)) : NODE_TYPES;
  if (nodeTypes.length) {
    branches.push(
      wanted
        ? NODES_BRANCH.replace(
            `IN (${NODE_TYPES.map(sqlStr).join(', ')})`,
            `IN (${nodeTypes.map(sqlStr).join(', ')})`,
          )
        : NODES_BRANCH,
    );
  }
  for (const [type, sql] of Object.entries(BRANCHES)) {
    if (!wanted || wanted.includes(type as ResourceType)) branches.push(sql);
  }

  return branches.length ? branches.join('\nUNION ALL\n') : null;
}

/**
 * The five field-match legs for one token.
 *
 * Note on `list_filter(tags, x -> …)`: newer DuckDB deprecates the `->` lambda
 * arrow in favour of `lambda x: …` and warns about it. The pinned duckdb-wasm
 * predates the new syntax and this matches the Rust handler, so `->` stays until
 * the pinned engine moves.
 */
function tokenLegs(token: string): string {
  const q = escapeIlike(token);
  const like = (expr: string) => `${expr} ILIKE '%' || '${q}' || '%' ESCAPE '\\'`;
  return `
  SELECT unique_id, 'name' AS matched_field, 1 AS priority FROM base WHERE ${like('name')}
  UNION ALL
  SELECT b.unique_id, 'column', 2 FROM base b
    JOIN dbt.node_columns c ON c.node_unique_id = b.unique_id
    WHERE ${like('c.column_name')}
  UNION ALL
  SELECT unique_id, 'tag', 3 FROM base
    WHERE tags IS NOT NULL
      AND len(list_filter(tags, x -> ${like('x')})) > 0
  UNION ALL
  SELECT unique_id, 'fqn', 4 FROM base
    WHERE fqn IS NOT NULL AND ${like("array_to_string(fqn, '.')")}
  UNION ALL
  SELECT unique_id, 'description', 5 FROM base WHERE ${like('description')}`;
}

/**
 * Match CTEs for the query's tokens.
 *
 * One token is a single CTE. Several become one CTE each plus an `INTERSECT` over
 * `unique_id`, so a row qualifies only by matching *every* token — possibly in
 * different fields, which is why the intersect is on the id rather than on the
 * whole match row.
 */
function matchCtes(tokens: string[]): string {
  if (tokens.length === 1) {
    return `field_matches AS (${tokenLegs(tokens[0]!)}\n)`;
  }

  const named = tokens.map((token, i) => ({ name: `token_matches_${i}`, token }));
  const ctes = named.map(({ name, token }) => `${name} AS (${tokenLegs(token)}\n)`);
  const intersect = named
    .map(({ name }) => `SELECT unique_id FROM ${name}`)
    .join('\n  INTERSECT\n  ');
  const union = named
    .map(({ name }) => `SELECT * FROM ${name}`)
    .join('\n  UNION ALL\n  ');

  return `${ctes.join(',\n')},
valid_unique_ids AS (
  ${intersect}
),
field_matches AS (
  SELECT * FROM (
  ${union}
  ) WHERE unique_id IN (SELECT unique_id FROM valid_unique_ids)
)`;
}

/**
 * The ranking key: several tiers packed into one sortable string.
 *
 * Each tier is left-padded to a fixed width so byte-order matches numeric order,
 * which lets one `ORDER BY` and one cursor slot carry all of it. Tiers, strongest
 * first: exact name match, prefix name match, which field matched, resource type,
 * modeling layer, then the raw name as a byte-for-byte tiebreak.
 *
 * Exact and prefix name match sit *above* resource type on purpose. dbt Catalog
 * weights type more heavily and consequently cannot guarantee an exact match ranks
 * first; for docs on a stateless run the typed name is the stronger signal.
 */
const RESOURCE_TIER_SQL = `CASE b.resource_type
    WHEN 'model' THEN '0' WHEN 'source' THEN '1' WHEN 'metric' THEN '2'
    WHEN 'exposure' THEN '3' WHEN 'snapshot' THEN '4' WHEN 'semantic_model' THEN '5'
    WHEN 'seed' THEN '6' WHEN 'saved_query' THEN '7' WHEN 'test' THEN '8'
    ELSE '9' END`;

// Marts first, unclassified last — the same path rules the model list uses.
const LAYER_TIER_SQL = `CASE
    WHEN lower(b.original_file_path) LIKE '%/marts/%'
      OR lower(b.original_file_path) LIKE '%/dim_%'
      OR lower(b.original_file_path) LIKE '%/fct_%'
      OR lower(b.original_file_path) LIKE 'marts/%' THEN '0'
    WHEN lower(b.original_file_path) LIKE '%/intermediate/%'
      OR lower(b.original_file_path) LIKE '%/int_%'
      OR lower(b.original_file_path) LIKE 'intermediate/%' THEN '1'
    WHEN lower(b.original_file_path) LIKE '%/staging/%'
      OR lower(b.original_file_path) LIKE '%/stg_%'
      OR lower(b.original_file_path) LIKE 'staging/%' THEN '2'
    ELSE '3' END`;

function rankingKey(query: string): string {
  const exact = `CASE WHEN lower(b.name) = lower(${sqlStr(query)}) THEN '0' ELSE '1' END`;
  const prefix = `CASE WHEN lower(b.name) LIKE lower('${escapeIlike(query)}') || '%' ESCAPE '\\' THEN '0' ELSE '1' END`;

  const tiers = [exact, prefix, 'w.match_priority', RESOURCE_TIER_SQL, LAYER_TIER_SQL]
    .map((expr) => `LPAD(CAST((${expr}) AS VARCHAR), 1, '0')`)
    .join('\n    || ');

  // A nameless row sorts last, and carries no name suffix to sort within.
  return `CASE WHEN b.name IS NULL THEN '9~9999' ELSE ${tiers} || (b.name) END`;
}

/**
 * Ranking key for a type/tag/package-only listing: no search text means no
 * name-match or field-match tiers to rank by, so this just orders by
 * resource type, modeling layer, then name.
 */
function rankingKeyTypeOnly(): string {
  const tiers = [RESOURCE_TIER_SQL, LAYER_TIER_SQL]
    .map((expr) => `LPAD(CAST((${expr}) AS VARCHAR), 1, '0')`)
    .join('\n    || ');

  return `CASE WHEN b.name IS NULL THEN '9~9999' ELSE ${tiers} || (b.name) END`;
}

export interface SearchFilter {
  resourceTypes?: ResourceType[];
  packages?: string[];
  tags?: string[];
}

export interface SearchQuery {
  sql: string;
  countSql: string;
  tables: string[];
  offset: number;
  limit: number;
}

/** Extra predicates from the filter, applied to the union's output. */
function filterWhere(filter: SearchFilter): string {
  const parts: string[] = [];
  if (filter.packages?.length) {
    parts.push(`\n  AND b.package_name IN (${filter.packages.map(sqlStr).join(', ')})`);
  }
  if (filter.tags?.length) {
    // Any overlap counts, matching how a tag filter reads in the UI.
    const list = filter.tags.map(sqlStr).join(', ');
    parts.push(
      `\n  AND b.tags IS NOT NULL AND len(list_intersect(b.tags, [${list}])) > 0`,
    );
  }
  return parts.join('');
}

/**
 * Build the search queries, or `null` when the request cannot match anything.
 *
 * `null` covers an empty query with no active filters (the blank-on-load state)
 * and a type filter naming only unsearchable types — both are "no results"
 * rather than errors. An empty query with a real type/package/tag filter set is
 * not `null`: the Filter tab stays on this same query builder regardless of how
 * many type checkboxes are ticked, so those filters need to produce a real,
 * filtered listing even without search text to rank or highlight against.
 */
export function buildSearchQuery(
  query: string,
  filter: SearchFilter,
  limit: number,
  offset: number,
): SearchQuery | null {
  const tokens = tokenize(query);
  const hasFilters = Boolean(
    filter.resourceTypes?.length || filter.packages?.length || filter.tags?.length,
  );
  if (!tokens.length && !hasFilters) return null;

  const base = baseUnion(filter.resourceTypes);
  if (!base) return null;

  const where = filterWhere(filter);
  const pageSize = Math.min(limit, MAX_PAGE_SIZE);

  if (!tokens.length) {
    const prelude = `WITH base AS (${base}
)`;

    return {
      sql: `${prelude}
SELECT b.*, NULL AS matched_field, NULL AS match_priority,
       ${rankingKeyTypeOnly()} AS cursor_key,
       NULL AS matched_column
FROM base b
WHERE 1 = 1${where}
ORDER BY cursor_key ASC, b.unique_id ASC
LIMIT ${pageSize} OFFSET ${offset}`,
      countSql: `${prelude}
SELECT COUNT(*) AS total
FROM base b
WHERE 1 = 1${where}`,
      tables: SEARCH_TABLES,
      offset,
      limit: pageSize,
    };
  }

  const prelude = `WITH base AS (${base}
),
${matchCtes(tokens)},
winners AS (
  SELECT unique_id, arg_min(matched_field, priority) AS matched_field,
         min(priority) AS match_priority
  FROM field_matches GROUP BY unique_id
)`;

  return {
    sql: `${prelude}
SELECT b.*, w.matched_field, w.match_priority,
       ${rankingKey(query)} AS cursor_key,
       -- Which column matched, so the highlight can name it. The handler issued a
       -- separate query per matched row for this; one join does.
       (SELECT LOWER(c.column_name) FROM dbt.node_columns c
        WHERE c.node_unique_id = b.unique_id
          AND c.column_name ILIKE '%' || '${escapeIlike(tokens[0]!)}' || '%' ESCAPE '\\'
        LIMIT 1) AS matched_column
FROM base b
JOIN winners w ON w.unique_id = b.unique_id
WHERE 1 = 1${where}
ORDER BY cursor_key ASC, b.unique_id ASC
LIMIT ${pageSize} OFFSET ${offset}`,
    countSql: `${prelude}
SELECT COUNT(*) AS total
FROM base b
JOIN winners w ON w.unique_id = b.unique_id
WHERE 1 = 1${where}`,
    tables: SEARCH_TABLES,
    offset,
    limit: pageSize,
  };
}

/** Characters that would be read as markup in the highlight fragment. */
function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/**
 * Wrap each occurrence of `token` in `<b>`, case-insensitively.
 *
 * The surrounding text is HTML-escaped first, so a description containing markup
 * renders as text rather than as tags. Only the `<b>` runs this adds are real
 * markup.
 */
export function highlight(text: string, tokens: string[]): string {
  let out = escapeHtml(text);
  for (const token of tokens) {
    if (!token) continue;
    const pattern = new RegExp(escapeRegExp(escapeHtml(token)), 'gi');
    out = out.replace(pattern, (match) => `<b>${match}</b>`);
  }
  return out;
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** How much description to show around a match. Matches the handler's window. */
const DESCRIPTION_WINDOW = 80;

/**
 * Highlight a description, windowed around the first match.
 *
 * A description can run for paragraphs, and a hit is only useful if the matched
 * text is visible in it — so the window is centred on the match rather than taken
 * from the start.
 */
export function highlightDescription(description: string, tokens: string[]): string {
  const lower = description.toLowerCase();
  const at = tokens
    .map((t) => lower.indexOf(t.toLowerCase()))
    .filter((i) => i >= 0)
    .sort((a, b) => a - b)[0];

  if (at === undefined || description.length <= DESCRIPTION_WINDOW) {
    return highlight(description.slice(0, DESCRIPTION_WINDOW), tokens);
  }

  const start = Math.max(0, at - Math.floor(DESCRIPTION_WINDOW / 2));
  const end = Math.min(description.length, start + DESCRIPTION_WINDOW);
  const slice = description.slice(start, end);
  const prefix = start > 0 ? '…' : '';
  const suffix = end < description.length ? '…' : '';
  return `${prefix}${highlight(slice, tokens)}${suffix}`;
}

/** Build the highlight fragment for one row, from whichever field matched. */
export function highlightFor(
  row: Record<string, unknown>,
  tokens: string[],
): string | null {
  const field = row.matched_field as string | null;
  switch (field) {
    case 'name':
      return row.name ? highlight(String(row.name), tokens) : null;
    case 'description':
      return row.description
        ? highlightDescription(String(row.description), tokens)
        : null;
    case 'column':
      return row.matched_column ? highlight(String(row.matched_column), tokens) : null;
    case 'tag': {
      const tags = Array.isArray(row.tags) ? (row.tags as string[]) : [];
      const matched = tags.find((tag) =>
        tokens.some((t) => tag.toLowerCase().includes(t.toLowerCase())),
      );
      return matched ? highlight(matched, tokens) : null;
    }
    case 'fqn': {
      const fqn = Array.isArray(row.fqn) ? (row.fqn as string[]).join('.') : null;
      return fqn ? highlight(fqn, tokens) : null;
    }
    default:
      return null;
  }
}

/**
 * Facet queries for the cross-type filter rail.
 *
 * Ported from the `SEARCH_FACETS_*_SQL` constants. Unlike the per-type list facets,
 * these *are* counted — the handler counted them and the rail displays the numbers.
 */
export const SEARCH_FACET_ACCESSES = `
SELECT access AS value, COUNT(*) AS cnt
FROM dbt.models
WHERE access IS NOT NULL
GROUP BY access
ORDER BY value`;

export const SEARCH_FACET_MATERIALIZATIONS = `
SELECT materialized AS value, COUNT(*) AS cnt
FROM dbt.models
WHERE materialized IS NOT NULL
GROUP BY materialized
ORDER BY value`;

/** Built from the same path rules as the model list, without the `b.` alias. */
export const SEARCH_FACET_LAYERS = `
SELECT CASE
  WHEN lower(original_file_path) LIKE '%/staging/%'
    OR lower(original_file_path) LIKE '%/stg_%'
    OR lower(original_file_path) LIKE 'staging/%' THEN 'Staging'
  WHEN lower(original_file_path) LIKE '%/intermediate/%'
    OR lower(original_file_path) LIKE '%/int_%'
    OR lower(original_file_path) LIKE 'intermediate/%' THEN 'Intermediate'
  WHEN lower(original_file_path) LIKE '%/marts/%'
    OR lower(original_file_path) LIKE '%/dim_%'
    OR lower(original_file_path) LIKE '%/fct_%'
    OR lower(original_file_path) LIKE 'marts/%' THEN 'Marts'
  ELSE NULL END AS value, COUNT(*) AS cnt
FROM dbt.models
GROUP BY value
HAVING value IS NOT NULL
ORDER BY value`;

export const SEARCH_FACET_TAGS = `
SELECT t.value, COUNT(*) AS cnt FROM (
  SELECT unnest(tags) AS value FROM dbt_internal.resources WHERE tags IS NOT NULL
  UNION ALL
  SELECT unnest(tags) AS value FROM dbt.exposures WHERE tags IS NOT NULL
  UNION ALL
  SELECT unnest(tags) AS value FROM dbt.metrics WHERE tags IS NOT NULL
  UNION ALL
  SELECT unnest(tags) AS value FROM dbt.saved_queries WHERE tags IS NOT NULL
) t
GROUP BY t.value
ORDER BY t.value`;

export const SEARCH_FACET_PACKAGES = `
SELECT pkg AS value, COUNT(*) AS cnt FROM (
  SELECT package_name AS pkg FROM dbt_internal.resources WHERE package_name IS NOT NULL
  UNION ALL
  SELECT package_name AS pkg FROM dbt.macros WHERE package_name IS NOT NULL
  UNION ALL
  SELECT package_name AS pkg FROM dbt.exposures WHERE package_name IS NOT NULL
) p
GROUP BY pkg
ORDER BY value`;
