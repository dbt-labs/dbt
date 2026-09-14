/**
 * Per-resource-type detail queries.
 *
 * `fetchAsset` was routing every type through `fromNodeDetail`, the generic
 * fallback the REST source kept for types with no endpoint of their own. That
 * rendered a model with a source's worth of fields — no columns, no materialization,
 * no contract. This routes each type to its own mapper instead, the way the REST
 * `REGISTRY` did.
 *
 * A detail is three queries: the base row, its columns, and its edges. Notably
 * *not* run results or catalog stats — those live on some list summaries but on no
 * detail shape, so joining them here would be dead weight.
 *
 * Each type reads its own information-schema table (`dbt.models`, `dbt.seeds`, …)
 * rather than filtering a union: the table *is* the resource type, so the
 * `resource_type` predicate the index needed is gone. The union
 * (`dbt_internal.resources`) is reserved for the surfaces that genuinely span
 * types, because reading it pulls every artifact.
 *
 * JSON-string columns (`meta`, `config`, `type_params`, `query_params`, `exports`,
 * `arguments`, …) are parsed by the caller, matching CC-7: they are `VARCHAR` in
 * the parquet, and the Rust handlers deserialized them handler-side rather than
 * letting an escaped string reach the wire.
 */

import type { Asset, ResourceType } from '../../typings/domain/asset';
import {
  fromExposureDetail,
  fromGroupDetail,
  fromMacroDetail,
  fromMetricDetail,
  fromModelDetail,
  fromNodeDetail,
  fromSavedQueryDetail,
  fromSeedDetail,
  fromSemanticModelDetail,
  fromSnapshotDetail,
  fromSourceDetail,
  fromTestDetail,
} from '../mappers/fromWire';
import { sqlStr } from './sql';

/**
 * Columns of one node, shaped as `RestNodeColumn`.
 *
 * The information schema renames all four type columns and keys the table on
 * `node_unique_id`. The aliases put the mapper's names back, so its
 * `data_type ?? declared_type ?? inferred_type ?? catalog_type` fallback chain —
 * and what the Columns tab shows — is unchanged.
 */
export function nodeColumnsSql(uniqueId: string): string {
  return `
SELECT LOWER(column_name) AS name,
       column_index AS index,
       data_type,
       data_type_declared AS declared_type,
       data_type_inferred AS inferred_type,
       data_type_actual AS catalog_type,
       description
FROM dbt.node_columns
WHERE node_unique_id = ${sqlStr(uniqueId)}
ORDER BY column_index`;
}

/** Which JSON-string columns a type carries, so the caller knows what to parse. */
export interface DetailSpec {
  sql(uniqueId: string): string;
  tables: string[];
  /** Fetch and attach `columns`. Only the relation-backed types have any. */
  wantsColumns: boolean;
  /** Extra per-type queries, attached to the row under these keys. */
  extras?: { key: string; sql(uniqueId: string): string; tables: string[] }[];
  /** Columns stored as JSON strings, parsed before mapping. */
  jsonColumns: string[];
  map(row: Record<string, unknown>): Asset;
}

/** `SELECT … FROM dbt.<table>` for one resource of that type. */
function nodeDetail(table: string, columns: string): (uniqueId: string) => string {
  return (uniqueId) => `
SELECT ${columns}
FROM ${table} n
WHERE n.unique_id = ${sqlStr(uniqueId)}
LIMIT 1`;
}

/** `SELECT … FROM <table>` for types with their own artifact. */
function ownDetail(table: string, columns: string): (uniqueId: string) => string {
  return (uniqueId) => `
SELECT ${columns}
FROM ${table} t
WHERE t.unique_id = ${sqlStr(uniqueId)}
LIMIT 1`;
}

/**
 * The fields every resource detail carries.
 *
 * `file_path` is gone: of the index's two path columns only `original_file_path`
 * was ever populated, so the information schema publishes just the one.
 * `patch_path` is the mapper's name for `properties_yml_file_path`.
 */
const NODE_BASE = `n.unique_id, n.name, n.resource_type, n.package_name, n.description,
  n.original_file_path, n.properties_yml_file_path AS patch_path, n.tags, n.fqn,
  n.meta, n.config, n.database_name, n.schema_name, n.identifier`;

export const DETAIL_REGISTRY: Partial<Record<ResourceType, DetailSpec>> = {
  model: {
    sql: nodeDetail(
      'dbt.models',
      // `group` is a SQL keyword, so it has to be quoted to be read as a column.
      `${NODE_BASE}, n.materialized, n.access AS access_level,
       n."group" AS group_name, n.contract_enforced, n.raw_code, n.compiled_code`,
    ),
    tables: ['dbt.models'],
    wantsColumns: true,
    jsonColumns: ['meta', 'config'],
    map: (row) => fromModelDetail(row as never),
  },

  seed: {
    sql: nodeDetail('dbt.seeds', `${NODE_BASE}`),
    tables: ['dbt.seeds'],
    wantsColumns: true,
    jsonColumns: ['meta', 'config'],
    map: (row) => fromSeedDetail(row as never),
  },

  snapshot: {
    sql: nodeDetail(
      'dbt.snapshots',
      `${NODE_BASE}, n.materialized, n.raw_code, n.compiled_code`,
    ),
    tables: ['dbt.snapshots'],
    wantsColumns: true,
    jsonColumns: ['meta', 'config'],
    map: (row) => fromSnapshotDetail(row as never),
  },

  source: {
    // Freshness is flat here and nested by the caller, matching the wire shape the
    // mapper expects.
    sql: (uniqueId) => `
SELECT ${NODE_BASE}, n.source_name, n.loader,
       sf.status, CAST(sf.snapshotted_at AS VARCHAR) AS snapshotted_at,
       CAST(sf.max_loaded_at AS VARCHAR) AS max_loaded_at
FROM dbt.sources n
LEFT JOIN dbt_rt.freshness sf ON sf.unique_id = n.unique_id
WHERE n.unique_id = ${sqlStr(uniqueId)}
LIMIT 1`,
    tables: ['dbt.sources', 'dbt_rt.freshness'],
    wantsColumns: true,
    jsonColumns: ['meta', 'config'],
    map: (row) => fromSourceDetail(row as never),
  },

  test: {
    // `test` and `unit_test` share this surface per ADR-3, discriminated on
    // `resource_type`. The union carries each side's own fields as NULL on the other.
    sql: (uniqueId) => {
      const id = sqlStr(uniqueId);
      // `dbt.data_tests` is `nodes` LEFT JOINed to `test_metadata` already, so
      // the test's own fields (`column_name`, `severity`) are columns here rather
      // than a join of ours. A singular test with no metadata still has a row,
      // with those columns null — the LEFT join is inside the view.
      return `
SELECT n.unique_id, n.name, 'test' AS resource_type, n.package_name, n.description,
       n.original_file_path, n.tags, n.fqn,
       n.column_name, n.severity,
       'data' AS test_type,
       n.raw_code, n.compiled_code,
       NULL AS model, NULL AS given, NULL AS expect
FROM dbt.data_tests n
WHERE n.unique_id = ${id}
UNION ALL
SELECT ut.unique_id, ut.name, 'unit_test', ut.package_name, ut.description,
       ut.original_file_path, NULL, NULL,
       NULL, NULL,
       'unit',
       NULL, NULL,
       ut.model, ut.given, ut.expect
FROM dbt.unit_tests ut
WHERE ut.unique_id = ${id}
LIMIT 1`;
    },
    tables: ['dbt.data_tests', 'dbt.unit_tests'],
    wantsColumns: false,
    jsonColumns: ['given', 'expect'],
    map: (row) => fromTestDetail(row as never),
  },

  macro: {
    sql: ownDetail(
      'dbt.macros',
      // No `file_path`: the information schema publishes only
      // `original_file_path`, which carried the identical value. Every
      // `AssetMetadata` caller already prefers it, so the File row is unaffected.
      `t.unique_id, t.name, 'macro' AS resource_type, t.package_name, t.description,
       t.original_file_path, t.properties_yml_file_path AS patch_path, t.macro_sql,
       t.arguments, t.meta`,
    ),
    tables: ['dbt.macros'],
    wantsColumns: false,
    jsonColumns: ['arguments', 'meta'],
    map: (row) => fromMacroDetail(row as never),
  },

  exposure: {
    sql: ownDetail(
      'dbt.exposures',
      `t.unique_id, t.name, 'exposure' AS resource_type, t.package_name, t.description,
       t.original_file_path, t.tags, t.fqn, t.exposure_type, t.maturity,
       t.url, t.owner_name, t.owner_email, t.meta`,
    ),
    tables: ['dbt.exposures'],
    wantsColumns: false,
    jsonColumns: ['meta'],
    map: (row) => fromExposureDetail(row as never),
  },

  metric: {
    sql: ownDetail(
      'dbt.metrics',
      `t.unique_id, t.name, 'metric' AS resource_type, t.package_name, t.description,
       t.original_file_path, t.fqn, t.tags, t.label, t.metric_type, t.type_params,
       t."group" AS group_name, t.meta`,
    ),
    tables: ['dbt.metrics'],
    wantsColumns: false,
    jsonColumns: ['type_params', 'meta'],
    map: (row) => fromMetricDetail(row as never),
  },

  saved_query: {
    sql: ownDetail(
      'dbt.saved_queries',
      `t.unique_id, t.name, 'saved_query' AS resource_type, t.package_name, t.description,
       t.original_file_path, t.label, t.fqn, t.tags, t.query_params, t.exports`,
    ),
    tables: ['dbt.saved_queries'],
    wantsColumns: false,
    jsonColumns: ['query_params', 'exports'],
    map: (row) => fromSavedQueryDetail(row as never),
  },

  semantic_model: {
    sql: ownDetail(
      'dbt.semantic_models',
      `t.unique_id, t.name, 'semantic_model' AS resource_type, t.package_name,
       t.description, t.original_file_path, t.fqn, t.model, t.config AS meta`,
    ),
    tables: ['dbt.semantic_models'],
    wantsColumns: false,
    jsonColumns: ['meta'],
    // Members live in their own artifacts, one row per member, so they are separate
    // queries rather than an aggregate — the detail page lists them in full.
    extras: [
      {
        key: 'entities',
        sql: (id) =>
          `SELECT name, entity_type AS type, expr FROM dbt.semantic_entities
           WHERE unique_id = ${sqlStr(id)} ORDER BY name`,
        tables: ['dbt.semantic_entities'],
      },
      {
        key: 'dimensions',
        sql: (id) =>
          `SELECT name, dimension_type AS type, description FROM dbt.semantic_dimensions
           WHERE unique_id = ${sqlStr(id)} ORDER BY name`,
        tables: ['dbt.semantic_dimensions'],
      },
      {
        key: 'measures',
        sql: (id) =>
          `SELECT name, agg, expr, description FROM dbt.semantic_measures
           WHERE unique_id = ${sqlStr(id)} ORDER BY name`,
        tables: ['dbt.semantic_measures'],
      },
    ],
    map: (row) => fromSemanticModelDetail(row as never),
  },

  group: {
    // Groups are keyed by (name, package) rather than unique_id, so the member query
    // has to match on both — the same reason the list's count is a correlated
    // subquery.
    sql: ownDetail(
      'dbt.groups',
      `t.unique_id, t.name, 'group' AS resource_type, t.package_name, t.description,
       t.original_file_path, t.owner_name, t.owner_email, t.config AS meta`,
    ),
    tables: ['dbt.groups'],
    wantsColumns: false,
    jsonColumns: ['meta'],
    extras: [
      {
        key: 'models',
        sql: (id) => `
SELECT n.unique_id, n.name
FROM dbt.models n
JOIN dbt.groups g
  ON g.name = n."group" AND g.package_name = n.package_name
WHERE g.unique_id = ${sqlStr(id)}
ORDER BY n.name
LIMIT 500`,
        tables: ['dbt.models', 'dbt.groups'],
      },
    ],
    map: (row) => fromGroupDetail(row as never),
  },
};

/** Generic-path SQL: the resource union, since the type is not known here. */
function genericDetailSql(uniqueId: string): string {
  return `
SELECT ${NODE_BASE}, n.raw_code, n.compiled_code
FROM dbt_internal.resources n
WHERE n.unique_id = ${sqlStr(uniqueId)}
LIMIT 1`;
}

/**
 * Fallback for types with no detail of their own.
 *
 * `analysis`, `function` and `operation` had no REST endpoint either; the generic
 * path is what served them there too. This is the one detail query that reads the
 * union, because it is reached precisely when the resource type is unknown — so
 * there is no typed table to pick.
 */
export const GENERIC_DETAIL: DetailSpec = {
  sql: genericDetailSql,
  tables: ['dbt_internal.resources'],
  wantsColumns: false,
  jsonColumns: ['meta', 'config'],
  map: (row) => fromNodeDetail(row as never),
};

/** The spec for a type, falling back to the generic node path. */
export function detailSpecFor(resourceType: ResourceType | undefined): DetailSpec {
  const spec = resourceType ? DETAIL_REGISTRY[resourceType] : undefined;
  return spec ?? GENERIC_DETAIL;
}
