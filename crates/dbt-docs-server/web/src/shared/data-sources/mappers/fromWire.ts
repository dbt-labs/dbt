/**
 * Wire rows to domain objects.
 *
 * The single mapping layer, shared by every data source. It was `rest/mappers/` when
 * REST was the only source; the DuckDB source reuses it verbatim by projecting its
 * SQL with the same snake_case column names, which is what keeps one mapping and one
 * set of tests instead of two that can disagree.
 *
 * The `Wire*` interfaces describe that shape. They began as the REST response bodies
 * and outlived the transport — a `Wire*` type is now a contract between a source's
 * projection and these functions, not a description of an HTTP payload.
 */

/**
 * Maps dbt-docs-server REST API response shapes to the protocol-agnostic
 * domain types defined in `typings/domain`. All types here are local mirrors
 * of the server structs — no runtime coupling to dbt-docs-v2's api.ts.
 */

import type {
  AnalysisAsset,
  Asset,
  AssetColumn,
  ExposureAsset,
  ExposureSummary,
  FunctionAsset,
  GroupAsset,
  GroupSummary,
  MacroAsset,
  MacroSummary,
  MetricAsset,
  MetricSummary,
  MetricTypeParams,
  ModelAsset,
  ModelSummary,
  OperationAsset,
  Relation,
  ResourceType,
  SavedQueryAsset,
  SavedQuerySummary,
  SeedSummary,
  SemanticModelAsset,
  SemanticModelSummary,
  SnapshotSummary,
  SourceAsset,
  SourceSummary,
  TestAsset,
  TestSummary,
  UnitTestAsset,
} from '../../typings/domain/asset';
import type { Capabilities } from '../../typings/domain/capabilities';
import type { AssetCounts } from '../../typings/domain/counts';
import type { Distribution } from '../../typings/domain/distribution';
import type { Facets, FacetValue } from '../../typings/domain/facets';
import type { FileEntry } from '../../typings/domain/files';
import type { ColumnLineageGraph, LineageGraph } from '../../typings/domain/lineage';
import type { ProjectOverview } from '../../typings/domain/overview';
import type { Project } from '../../typings/domain/project';
import type {
  MatchedField,
  SearchFacets,
  SearchHit,
} from '../../typings/domain/search';
import type { Page } from '../../typings/page';

// ---------------------------------------------------------------------------
// Local REST response type mirrors — intentionally separate from domain types.
// ---------------------------------------------------------------------------

interface RestNodeColumn {
  name: string;
  index?: number | null;
  data_type?: string | null;
  declared_type?: string | null;
  inferred_type?: string | null;
  catalog_type?: string | null;
  description?: string | null;
}

interface RestDetailBase {
  unique_id: string;
  name: string;
  resource_type: string;
  package_name?: string | null;
  description?: string | null;
  original_file_path?: string | null;
}

interface RestEdgeRef {
  unique_id: string;
  edge_type: string;
}

interface RestModelDetail extends RestDetailBase {
  file_path?: string | null;
  patch_path?: string | null;
  database_name?: string | null;
  schema_name?: string | null;
  identifier?: string | null;
  materialized?: string | null;
  access_level?: string | null;
  group_name?: string | null;
  contract_enforced?: boolean | null;
  language?: string | null;
  raw_code?: string | null;
  compiled_code?: string | null;
  tags?: string[];
  fqn?: string[];
  meta?: Record<string, unknown> | null;
  config?: Record<string, unknown> | null;
  columns?: RestNodeColumn[];
  depends_on?: RestEdgeRef[];
  referenced_by?: RestEdgeRef[];
}

interface RestSeedDetail extends RestDetailBase {
  file_path?: string | null;
  patch_path?: string | null;
  tags: string[];
  fqn: string[];
  database_name?: string | null;
  schema_name?: string | null;
  identifier?: string | null;
  meta: Record<string, unknown> | null;
  config?: Record<string, unknown> | null;
  columns: RestNodeColumn[];
  depends_on?: RestEdgeRef[];
  referenced_by: RestEdgeRef[];
}

interface RestSnapshotDetail extends RestDetailBase {
  patch_path?: string | null;
  tags: string[];
  fqn: string[];
  database_name?: string | null;
  schema_name?: string | null;
  identifier?: string | null;
  materialized: string;
  raw_code?: string | null;
  compiled_code?: string | null;
  meta: Record<string, unknown> | null;
  config?: Record<string, unknown> | null;
  depends_on: RestEdgeRef[];
  referenced_by: RestEdgeRef[];
  columns: RestNodeColumn[];
}

interface RestSourceDetail extends RestDetailBase {
  tags: string[];
  fqn: string[];
  database_name?: string | null;
  schema_name?: string | null;
  identifier?: string | null;
  source_name?: string | null;
  loader?: string | null;
  meta: Record<string, unknown> | null;
  config?: Record<string, unknown> | null;
  columns: RestNodeColumn[];
  referenced_by?: RestEdgeRef[];
  freshness: {
    status: string;
    snapshotted_at?: string | null;
    max_loaded_at?: string | null;
  } | null;
}

interface RestExposureDetail extends RestDetailBase {
  file_path?: string | null;
  tags: string[];
  fqn: string[];
  exposure_type?: string | null;
  maturity?: string | null;
  url?: string | null;
  owner_name?: string | null;
  owner_email?: string | null;
  meta: Record<string, unknown> | null;
  depends_on: RestEdgeRef[];
}

interface RestMetricDetail extends RestDetailBase {
  fqn: string[];
  tags: string[];
  label?: string | null;
  metric_type?: string | null;
  type_params: unknown;
  group_name?: string | null;
  meta: Record<string, unknown> | null;
  depends_on?: RestEdgeRef[];
  referenced_by?: RestEdgeRef[];
}

interface RestMacroArgument {
  name: string;
  type?: string | null;
  description?: string | null;
}

interface RestMacroDetail extends RestDetailBase {
  file_path?: string | null;
  patch_path?: string | null;
  macro_sql?: string | null;
  arguments?: RestMacroArgument[] | null;
  meta: Record<string, unknown> | null;
  depends_on?: RestEdgeRef[];
  referenced_by?: RestEdgeRef[];
}

interface RestSemanticDimension {
  name: string;
  type: string;
  description?: string | null;
}

interface RestSemanticMeasure {
  name: string;
  agg: string;
  expr?: string | null;
  description?: string | null;
}

interface RestSemanticEntity {
  name: string;
  type: string;
  expr?: string | null;
}

interface RestSemanticModelDetail extends RestDetailBase {
  fqn: string[];
  tags: string[];
  model: { unique_id: string; name: string } | null;
  entities: RestSemanticEntity[];
  dimensions: RestSemanticDimension[];
  measures: RestSemanticMeasure[];
  meta: Record<string, unknown> | null;
  depends_on?: RestEdgeRef[];
  referenced_by?: RestEdgeRef[];
}

interface RestTestDetailCommon extends RestDetailBase {
  tags: string[];
  fqn: string[];
  depends_on: RestEdgeRef[];
}

interface RestDataTestDetail extends RestTestDetailCommon {
  column_name?: string | null;
  test_type?: string | null;
  severity?: string | null;
  raw_code?: string | null;
  compiled_code?: string | null;
}

interface RestUnitTestDetail extends RestTestDetailCommon {
  model?: string | null;
  given: Array<{ input: string; rows: Record<string, unknown>[] }>;
  expect: { rows: Record<string, unknown>[] } | null;
}

export type RestTestDetail = RestDataTestDetail | RestUnitTestDetail;

interface RestSavedQueryDetail extends RestDetailBase {
  label?: string | null;
  fqn: string[];
  tags: string[];
  query_params?: {
    metrics?: string[] | null;
    group_by?: string[] | null;
    // The server emits `where` as a structured object, not a string list:
    // `{ where_filters: [{ where_sql_template }] }`.
    where?: {
      where_filters?: Array<{ where_sql_template?: string | null }> | null;
    } | null;
    order_by?: string[] | null;
    limit?: number | null;
  } | null;
  exports?: Array<{ name: string; config: Record<string, unknown> }> | null;
  depends_on: RestEdgeRef[];
  referenced_by?: RestEdgeRef[];
}

interface RestGroupOwner {
  name?: string | null;
  email?: string | null;
  github?: string | null;
  slack?: string | null;
}

interface RestGroupModelMember {
  unique_id: string;
  name: string;
  database_name?: string | null;
  schema_name?: string | null;
  contract_enforced?: boolean | null;
}

interface RestGroupDetail extends RestDetailBase {
  tags: string[];
  owner: RestGroupOwner | null;
  meta: Record<string, unknown> | null;
  referenced_by?: RestEdgeRef[];
  models?: RestGroupModelMember[] | null;
}

/** Generic node bag — used for analysis / function / operation fallback. */
interface RestNodeDetail extends RestDetailBase {
  raw_code?: string | null;
  compiled_code?: string | null;
  language?: string | null;
  tags?: string[] | null;
  fqn?: string[] | null;
  meta?: Record<string, unknown> | null;
  config?: Record<string, unknown> | null;
  arguments?: RestMacroArgument[] | null;
  return_type?: string | null;
  database_name?: string | null;
  schema_name?: string | null;
  identifier?: string | null;
  depends_on?: RestEdgeRef[];
  referenced_by?: RestEdgeRef[];
}

export interface RestCapabilities {
  has_column_lineage: boolean;
  has_dbt_state?: boolean;
}

/** `GET /api/v1/distribution` — build identity, not feature capability.
 *  `name` is the build flavor (`"oss"` = dbt Core, anything else = dbt v2). */
export interface RestDistribution {
  name: string;
  version?: string;
  is_logged_in: boolean;
}

/** `GET /api/v1/project` — the running project's identity and git state. */
export interface RestProject {
  name: string;
  project_id?: string;
  description?: string | null;
  dbt_version?: string;
  adapter_type?: string;
  git_sha?: string | null;
  git_branch?: string | null;
  git_is_dirty?: boolean | null;
}

/** One row of `dbt.docs_blocks` — the winning `__overview__` block. */
export interface RestProjectOverview {
  unique_id: string;
  package_name: string | null;
  block_contents: string;
}

/** One file-bearing resource. `patch_path` — `properties_yml_file_path` in the
 *  information schema — is populated only for resources and macros. */
export interface RestFileEntry {
  unique_id: string;
  name: string;
  resource_type: string;
  package_name: string;
  original_file_path: string;
  patch_path?: string | null;
}

/** `GET /api/v1/files` envelope. */
export interface RestFileListResponse {
  files: RestFileEntry[];
  total: number;
}

export interface RestLineageNode {
  unique_id: string;
  name: string;
  resource_type: string;
  materialized?: string | null;
  /** Used to infer the modeling layer (staging/intermediate/marts) for the
   *  DAG's "Model layer" lens -- see inferModelingLayer in lib/resourceType. */
  original_file_path?: string | null;
  depth: number;
}

export interface RestLineageEdge {
  from_id: string;
  to_id: string;
  edge_type: string;
}

export interface RestLineageResponse {
  root: string;
  max_depth: number;
  nodes: RestLineageNode[];
  edges: RestLineageEdge[];
}

export interface RestColumnLineageEdge {
  from_node: string;
  from_column: string;
  to_node: string;
  to_column: string;
  kind: string;
}

export interface RestColumnLineageResponse {
  root: string;
  edges: RestColumnLineageEdge[];
}

// ---------------------------------------------------------------------------
// List response envelope + per-resource summary mirrors.
// ---------------------------------------------------------------------------

/** Relay-style page envelope shared by every cursor-paginated list endpoint
 *  (ADR-6 in dbt-docs-server). `end_cursor` is null on the last page. */
export interface RestPageInfo {
  total_count: number;
  start_cursor?: string | null;
  end_cursor: string | null;
  has_next_page: boolean;
}

/** Generic `{ data, page_info }` list envelope. `T` is the summary row shape. */
export interface RestListResponse<T> {
  data: T[];
  page_info: RestPageInfo;
}

interface RestModelListCatalogInfo {
  row_count_stat?: number | null;
  bytes_stat?: number | null;
  last_modified_stat?: string | null;
}

interface RestModelSummary {
  unique_id: string;
  name: string;
  package_name?: string | null;
  original_file_path?: string | null;
  modeling_layer?: string | null;
  access_level?: string | null;
  contract_enforced?: boolean | null;
  owner?: string | null;
  executed_at?: string | null;
  catalog?: RestModelListCatalogInfo | null;
}

interface RestSourceListFreshness {
  status: string;
  snapshotted_at?: string | null;
  max_loaded_at?: string | null;
}

interface RestSourceSummary {
  unique_id: string;
  name: string;
  package_name?: string | null;
  source_name?: string | null;
  source_description?: string | null;
  database_name?: string | null;
  schema_name?: string | null;
  identifier?: string | null;
  loader?: string | null;
  tags?: string[];
  freshness?: RestSourceListFreshness | null;
}

interface RestSeedSummary {
  unique_id: string;
  name: string;
  package_name?: string | null;
  description?: string | null;
  original_file_path?: string | null;
  row_count?: number | null;
  executed_at?: string | null;
}

interface RestSnapshotListExecutionInfo {
  status?: string | null;
  completed_at?: string | null;
  error?: string | null;
}

interface RestSnapshotListCatalogInfo {
  row_count_stat?: number | null;
  bytes_stat?: number | null;
  last_modified_stat?: string | null;
}

interface RestSnapshotSummary {
  unique_id: string;
  name: string;
  package_name?: string | null;
  materialized?: string | null;
  strategy?: string | null;
  updated_at?: string | null;
  execution_info?: RestSnapshotListExecutionInfo | null;
  catalog?: RestSnapshotListCatalogInfo | null;
}

interface RestTestListExecutionInfo {
  status?: string | null;
  completed_at?: string | null;
  execution_time?: number | null;
}

interface RestTestSummary {
  unique_id: string;
  name: string;
  resource_type: string;
  package_name?: string | null;
  test_type?: string | null;
  tested_node_unique_id?: string | null;
  tested_column?: string | null;
  severity?: string | null;
  execution_info?: RestTestListExecutionInfo | null;
}

interface RestMetricSummary {
  unique_id: string;
  name: string;
  package_name?: string | null;
  group_name?: string | null;
  metric_type?: string | null;
  tags?: string[];
  description?: string | null;
}

interface RestSemanticEntityRef {
  name: string;
  type?: string | null;
}

interface RestSemanticModelSummary {
  unique_id: string;
  name: string;
  package_name?: string | null;
  group_name?: string | null;
  primary_entity?: string | null;
  entities?: RestSemanticEntityRef[];
  description?: string | null;
}

interface RestSavedQuerySummary {
  unique_id: string;
  name: string;
  package_name?: string | null;
  group_name?: string | null;
  tags?: string[];
  description?: string | null;
}

interface RestMacroSummary {
  unique_id: string;
  name: string;
  package_name?: string | null;
  description?: string | null;
  arguments?: RestMacroArgument[] | null;
}

interface RestGroupSummary {
  unique_id: string;
  name: string;
  package_name?: string | null;
  owner_name?: string | null;
  owner_email?: string | null;
  owner_github?: string | null;
  owner_slack?: string | null;
  model_count?: number | null;
}

interface RestExposureSummary {
  unique_id: string;
  name: string;
  package_name?: string | null;
  exposure_type?: string | null;
  maturity?: string | null;
  owner_name?: string | null;
  owner_email?: string | null;
  tags?: string[];
  description?: string | null;
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

function col(c: RestNodeColumn): AssetColumn {
  return {
    name: c.name,
    description: c.description ?? null,
    dataType:
      c.data_type ?? c.declared_type ?? c.inferred_type ?? c.catalog_type ?? null,
    declaredType: c.declared_type ?? null,
    catalogType: c.catalog_type ?? null,
    tags: [],
    meta: {},
    index: c.index ?? null,
  };
}

function relation(
  db?: string | null,
  schema?: string | null,
  identifier?: string | null,
): Relation | null {
  if (!db && !schema && !identifier) return null;
  return { database: db ?? '', schema: schema ?? '', identifier: identifier ?? '' };
}

function asResourceType(v: string): ResourceType {
  return v as ResourceType;
}

// ---------------------------------------------------------------------------
// Per-resource mappers
// ---------------------------------------------------------------------------

export function fromModelDetail(d: RestModelDetail): ModelAsset {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: asResourceType(d.resource_type) as ModelAsset['resourceType'],
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    filePath: d.file_path ?? null,
    originalFilePath: d.original_file_path ?? null,
    patchPath: d.patch_path ?? null,
    fqn: d.fqn ?? null,
    meta: d.meta ?? null,
    config: d.config ?? null,
    rawCode: d.raw_code ?? null,
    compiledCode: d.compiled_code ?? null,
    language: (d.language as ModelAsset['language']) ?? null,
    access: (d.access_level as ModelAsset['access']) ?? null,
    contractEnforced: d.contract_enforced ?? null,
    materializedType: d.materialized ?? null,
    group: d.group_name ?? null,
    relation: relation(d.database_name, d.schema_name, d.identifier),
    columns: (d.columns ?? []).map(col),
    dependsOn: (d.depends_on ?? []).map((e) => e.unique_id),
    referencedBy: (d.referenced_by ?? []).map((e) => e.unique_id),
  };
}

export function fromSeedDetail(d: RestSeedDetail): ModelAsset {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'seed',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    originalFilePath: d.original_file_path ?? null,
    patchPath: d.patch_path ?? null,
    fqn: d.fqn ?? null,
    meta: d.meta ?? null,
    config: d.config ?? null,
    rawCode: null,
    compiledCode: null,
    language: null,
    access: null,
    contractEnforced: null,
    materializedType: null,
    group: null,
    relation: relation(d.database_name, d.schema_name, d.identifier),
    columns: (d.columns ?? []).map(col),
    dependsOn: (d.depends_on ?? []).map((e) => e.unique_id),
    referencedBy: (d.referenced_by ?? []).map((e) => e.unique_id),
  };
}

export function fromSnapshotDetail(d: RestSnapshotDetail): ModelAsset {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'snapshot',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    originalFilePath: d.original_file_path ?? null,
    patchPath: d.patch_path ?? null,
    fqn: d.fqn ?? null,
    meta: null,
    config: d.config ?? null,
    rawCode: d.raw_code ?? null,
    compiledCode: d.compiled_code ?? null,
    language: null,
    access: null,
    contractEnforced: null,
    materializedType: d.materialized ?? null,
    group: null,
    relation: relation(d.database_name, d.schema_name, d.identifier),
    columns: (d.columns ?? []).map(col),
    dependsOn: (d.depends_on ?? []).map((e) => e.unique_id),
    referencedBy: (d.referenced_by ?? []).map((e) => e.unique_id),
  };
}

export function fromSourceDetail(d: RestSourceDetail): SourceAsset {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'source',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    originalFilePath: d.original_file_path ?? null,
    fqn: d.fqn ?? null,
    meta: d.meta ?? null,
    config: d.config ?? null,
    sourceName: d.source_name ?? '',
    identifier: d.identifier ?? '',
    loader: d.loader ?? null,
    // REST FreshnessInfo carries status/timestamps, not warn/error thresholds.
    freshness: null,
    freshnessStatus: d.freshness?.status ?? null,
    freshnessMaxLoadedAt: d.freshness?.max_loaded_at ?? null,
    relation: relation(d.database_name, d.schema_name, d.identifier),
    columns: (d.columns ?? []).map(col),
    referencedBy: (d.referenced_by ?? []).map((e) => e.unique_id),
  };
}

export function fromExposureDetail(d: RestExposureDetail): ExposureAsset {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'exposure',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    originalFilePath: d.original_file_path ?? null,
    fqn: d.fqn ?? null,
    meta: d.meta ?? null,
    exposureType: (d.exposure_type as ExposureAsset['exposureType']) ?? 'analysis',
    maturity: (d.maturity as ExposureAsset['maturity']) ?? null,
    ownerName: d.owner_name ?? null,
    ownerEmail: d.owner_email ?? null,
    url: d.url ?? null,
    dependsOn: d.depends_on.map((e) => e.unique_id),
  };
}

/** A metric/measure ref in the raw manifest JSON is sometimes just the bare
 *  name string (e.g. `"measure": "ad_id"`) and sometimes an object (e.g.
 *  `"measure": {"name": "ad_id", "filter": ...}`), depending on the metric
 *  and manifest schema version. Reading `.name` off a string silently
 *  produces `undefined` rather than an error, so this was rendering blank
 *  Expression rows for the string-shaped case -- handle both. */
function metricRefName(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value && typeof value === 'object' && 'name' in value) {
    return String((value as { name?: unknown }).name ?? '');
  }
  return '';
}

function parseMetricTypeParams(
  metricType: string | null | undefined,
  typeParams: unknown,
): MetricTypeParams {
  const p = typeParams as Record<string, unknown> | null | undefined;
  const kind = (metricType ?? 'simple') as MetricTypeParams['kind'];

  if (kind === 'ratio') {
    return {
      kind: 'ratio',
      numerator: { name: metricRefName(p?.numerator), alias: null, filter: null },
      denominator: { name: metricRefName(p?.denominator), alias: null, filter: null },
    };
  }
  if (kind === 'cumulative') {
    return {
      kind: 'cumulative',
      measure: { name: metricRefName(p?.measure), filter: null },
      window: p?.window != null ? String(p.window) : null,
      grainToDate: p?.grain_to_date != null ? String(p.grain_to_date) : null,
    };
  }
  if (kind === 'derived') {
    const metrics = (p?.metrics as unknown[] | undefined) ?? [];
    return {
      kind: 'derived',
      metrics: metrics.map((m) => ({
        name: metricRefName(m),
        alias: null,
        filter: null,
      })),
      expr: p?.expr != null ? String(p.expr) : null,
    };
  }
  return {
    kind: 'simple',
    measure: { name: metricRefName(p?.measure), filter: null },
  };
}

export function fromMetricDetail(d: RestMetricDetail): MetricAsset {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'metric',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    fqn: d.fqn ?? null,
    meta: d.meta ?? null,
    label: d.label ?? null,
    typeParams: parseMetricTypeParams(d.metric_type, d.type_params),
    group: d.group_name ?? null,
    dependsOn: (d.depends_on ?? []).map((e) => e.unique_id),
    referencedBy: (d.referenced_by ?? []).map((e) => e.unique_id),
  };
}

export function fromMacroDetail(d: RestMacroDetail): MacroAsset {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'macro',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: [],
    originalFilePath: d.original_file_path ?? null,
    patchPath: d.patch_path ?? null,
    meta: d.meta ?? null,
    macroSql: d.macro_sql ?? '',
    arguments: (d.arguments ?? []).map((a) => ({
      name: a.name,
      type: a.type ?? null,
      description: a.description ?? null,
    })),
    path: d.file_path ?? d.original_file_path ?? '',
    dependsOn: (d.depends_on ?? []).map((e) => e.unique_id),
    referencedBy: (d.referenced_by ?? []).map((e) => e.unique_id),
  };
}

export function fromSemanticModelDetail(
  d: RestSemanticModelDetail,
): SemanticModelAsset {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'semantic_model',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    fqn: d.fqn ?? null,
    meta: d.meta ?? null,
    modelUniqueId: d.model?.unique_id ?? null,
    dimensions: d.dimensions.map((dim) => ({
      name: dim.name,
      type: dim.type as 'categorical' | 'time',
      description: dim.description ?? null,
    })),
    measures: d.measures.map((m) => ({
      name: m.name,
      agg: m.agg,
      expr: m.expr ?? null,
      description: m.description ?? null,
    })),
    entities: d.entities.map((e) => ({
      name: e.name,
      type: e.type as 'primary' | 'foreign' | 'unique' | 'natural',
      expr: e.expr ?? null,
    })),
    dependsOn: (d.depends_on ?? []).map((e) => e.unique_id),
    referencedBy: (d.referenced_by ?? []).map((e) => e.unique_id),
  };
}

function isUnitTest(d: RestTestDetail): d is RestUnitTestDetail {
  // Not `'given' in d`: the duckdb union query selects a `given` column on
  // both branches (NULL for data tests), so the key is always present --
  // `resource_type` is the actual discriminator set correctly by both the
  // duckdb query and the real backend.
  return d.resource_type === 'unit_test';
}

export function fromTestDetail(d: RestTestDetail): TestAsset | UnitTestAsset {
  if (isUnitTest(d)) {
    const u = d as RestUnitTestDetail;
    return {
      uniqueId: u.unique_id,
      name: u.name,
      resourceType: 'unit_test',
      description: u.description ?? null,
      packageName: u.package_name ?? '',
      tags: u.tags ?? [],
      fqn: u.fqn ?? null,
      modelUniqueId: null,
      given: (u.given ?? []).map((g) => ({ input: g.input, rows: g.rows })),
      expect: u.expect ?? { rows: [] },
    };
  }
  const t = d as RestDataTestDetail;
  return {
    uniqueId: t.unique_id,
    name: t.name,
    resourceType: 'test',
    description: t.description ?? null,
    packageName: t.package_name ?? '',
    tags: t.tags ?? [],
    fqn: t.fqn ?? null,
    testType: t.test_type === 'singular' ? 'singular' : 'generic',
    severity: (t.severity as TestAsset['severity']) ?? null,
    columnName: t.column_name ?? null,
    dependsOn: (t.depends_on ?? []).map((e) => e.unique_id),
    rawCode: t.raw_code ?? null,
    compiledCode: t.compiled_code ?? null,
  };
}

export function fromSavedQueryDetail(d: RestSavedQueryDetail): SavedQueryAsset {
  const qp = d.query_params;
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'saved_query',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    fqn: d.fqn ?? null,
    label: d.label ?? null,
    queryParams: {
      metrics: qp?.metrics ?? [],
      groupBy: qp?.group_by ?? [],
      where: (qp?.where?.where_filters ?? [])
        .map((f) => f.where_sql_template)
        .filter((s): s is string => typeof s === 'string'),
      orderBy: qp?.order_by ?? null,
      limit: qp?.limit ?? null,
    },
    exports: (d.exports ?? []).map((e) => ({
      name: e.name,
      exportAs: (e.config.export_as as 'table' | 'view' | 'cache') ?? 'table',
      schema: (e.config.schema as string | null) ?? null,
    })),
    dependsOn: (d.depends_on ?? []).map((e) => e.unique_id),
    referencedBy: (d.referenced_by ?? []).map((e) => e.unique_id),
  };
}

export function fromGroupDetail(d: RestGroupDetail): GroupAsset {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'group',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    meta: d.meta ?? null,
    ownerName: d.owner?.name ?? null,
    ownerEmail: d.owner?.email ?? null,
    ownerGithub: d.owner?.github ?? null,
    ownerSlack: d.owner?.slack ?? null,
    models: d.models
      ? d.models.map((m) => ({
          uniqueId: m.unique_id,
          name: m.name,
          database: m.database_name ?? null,
          schema: m.schema_name ?? null,
        }))
      : null,
    referencedBy: (d.referenced_by ?? []).map((e) => e.unique_id),
  };
}

/** Fallback for analysis / function / operation via the generic /nodes endpoint. */
export function fromNodeDetail(d: RestNodeDetail): Asset {
  const base = {
    uniqueId: d.unique_id,
    name: d.name,
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    originalFilePath: d.original_file_path ?? null,
    fqn: d.fqn ?? null,
    meta: d.meta ?? null,
    config: d.config ?? null,
    dependsOn: (d.depends_on ?? []).map((e) => e.unique_id),
    referencedBy: (d.referenced_by ?? []).map((e) => e.unique_id),
  };

  if (d.resource_type === 'function') {
    return {
      ...base,
      resourceType: 'function',
      rawCode: d.raw_code ?? null,
      compiledCode: d.compiled_code ?? null,
      language: (d.language as FunctionAsset['language']) ?? null,
      arguments: (d.arguments ?? []).map((a) => ({
        name: a.name,
        type: a.type ?? null,
        description: a.description ?? null,
      })),
      returnType: d.return_type ?? null,
      relation: relation(d.database_name, d.schema_name, d.identifier),
    } satisfies FunctionAsset;
  }

  if (d.resource_type === 'operation') {
    return {
      ...base,
      resourceType: 'operation',
      rawCode: d.raw_code ?? null,
      compiledCode: d.compiled_code ?? null,
    } satisfies OperationAsset;
  }

  return {
    ...base,
    resourceType: 'analysis',
    rawCode: d.raw_code ?? null,
    compiledCode: d.compiled_code ?? null,
    language: (d.language as AnalysisAsset['language']) ?? null,
  } satisfies AnalysisAsset;
}

// ---------------------------------------------------------------------------
// Per-resource list summary mappers
// ---------------------------------------------------------------------------

export function fromModelSummary(d: RestModelSummary): ModelSummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'model',
    description: null,
    packageName: d.package_name ?? '',
    tags: [],
    originalFilePath: d.original_file_path ?? null,
    modelingLayer: d.modeling_layer ?? null,
    owner: d.owner ?? null,
    executedAt: d.executed_at ?? null,
    rowCountStat: d.catalog?.row_count_stat ?? null,
  };
}

export function fromSourceSummary(d: RestSourceSummary): SourceSummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'source',
    description: d.source_description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    sourceName: d.source_name ?? null,
    databaseName: d.database_name ?? null,
    schemaName: d.schema_name ?? null,
  };
}

export function fromSeedSummary(d: RestSeedSummary): SeedSummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'seed',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: [],
    originalFilePath: d.original_file_path ?? null,
    rowCount: d.row_count ?? null,
    executedAt: d.executed_at ?? null,
  };
}

export function fromSnapshotSummary(d: RestSnapshotSummary): SnapshotSummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'snapshot',
    description: null,
    packageName: d.package_name ?? '',
    tags: [],
    materialized: d.materialized ?? null,
    rowCountStat: d.catalog?.row_count_stat ?? null,
    bytesStat: d.catalog?.bytes_stat ?? null,
    lastModifiedStat: d.catalog?.last_modified_stat ?? null,
  };
}

export function fromTestSummary(d: RestTestSummary): TestSummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: d.resource_type === 'unit_test' ? 'unit_test' : 'test',
    description: null,
    packageName: d.package_name ?? '',
    tags: [],
    testType: d.resource_type === 'unit_test' ? 'unit' : 'data',
    status: d.execution_info?.status ?? null,
    testedNodeUniqueId: d.tested_node_unique_id ?? null,
    testedColumn: d.tested_column ?? null,
  };
}

export function fromMetricSummary(d: RestMetricSummary): MetricSummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'metric',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    metricType: d.metric_type ?? null,
  };
}

export function fromSemanticModelSummary(
  d: RestSemanticModelSummary,
): SemanticModelSummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'semantic_model',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: [],
    entities: (d.entities ?? []).map((e) => e.name),
  };
}

export function fromSavedQuerySummary(d: RestSavedQuerySummary): SavedQuerySummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'saved_query',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
  };
}

export function fromMacroSummary(d: RestMacroSummary): MacroSummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'macro',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: [],
    arguments: (d.arguments ?? []).map((a) => a.name),
  };
}

export function fromGroupSummary(d: RestGroupSummary): GroupSummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'group',
    description: null,
    packageName: d.package_name ?? '',
    tags: [],
    ownerName: d.owner_name ?? null,
    ownerEmail: d.owner_email ?? null,
    ownerGithub: d.owner_github ?? null,
    ownerSlack: d.owner_slack ?? null,
    modelCount: d.model_count ?? null,
  };
}

export function fromExposureSummary(d: RestExposureSummary): ExposureSummary {
  return {
    uniqueId: d.unique_id,
    name: d.name,
    resourceType: 'exposure',
    description: d.description ?? null,
    packageName: d.package_name ?? '',
    tags: d.tags ?? [],
    exposureType: d.exposure_type ?? null,
    ownerName: d.owner_name ?? null,
    ownerEmail: d.owner_email ?? null,
  };
}

// ---------------------------------------------------------------------------
// Capabilities, lineage, column-lineage
// ---------------------------------------------------------------------------

export function fromCapabilities(c: RestCapabilities): Capabilities {
  return {
    hasColumnLineage: c.has_column_lineage,
    hasQueryHistory: false,
    hasCostInsights: false,
    hasPerformance: false,
    hasRecommendations: false,
    hasHealthSignals: false,
    hasAutoExposures: false,
    hasMultiProject: false,
    hasMesh: false,
    hasRunResults: false,
    hasCatalogStats: false,
    hasDbtState: !!c.has_dbt_state,
  };
}

/** Response body of `GET /api/v1/nodes/counts` — per-resource-type tallies
 *  keyed by snake_case resource type. `unit_test` rows fold into `test` on the
 *  backend; absent or unknown keys are dropped by {@link fromNodeCounts}. */
export type RestNodeCounts = Record<string, number>;

/** Resource types the domain {@link AssetCounts} recognizes. Mirrors the
 *  `ResourceType` union — keys outside it are dropped so a backend that grows a
 *  new type can't smuggle an untyped key into the domain shape. */
const KNOWN_RESOURCE_TYPES: ReadonlySet<string> = new Set<ResourceType>([
  'model',
  'seed',
  'snapshot',
  'source',
  'exposure',
  'metric',
  'macro',
  'semantic_model',
  'test',
  'unit_test',
  'saved_query',
  'function',
  'group',
  'analysis',
  'operation',
]);

/** Map the project-wide node-counts response to {@link AssetCounts}, keeping
 *  only keys that name a known {@link ResourceType}. */
export function fromNodeCounts(raw: RestNodeCounts): AssetCounts {
  const out: AssetCounts = {};
  for (const [key, count] of Object.entries(raw)) {
    if (KNOWN_RESOURCE_TYPES.has(key)) out[key as ResourceType] = count;
  }
  return out;
}

export function fromDistribution(d: RestDistribution): Distribution {
  return {
    isProprietary: d.name !== 'oss',
    isLoggedIn: d.is_logged_in,
    version: d.version,
  };
}

export function fromProject(d: RestProject): Project {
  return {
    name: d.name,
    projectId: d.project_id,
    description: d.description,
    dbtVersion: d.dbt_version,
    adapterType: d.adapter_type,
    gitSha: d.git_sha,
    gitBranch: d.git_branch,
    gitIsDirty: d.git_is_dirty,
  };
}

export function fromProjectOverview(d: RestProjectOverview): ProjectOverview {
  return {
    uniqueId: d.unique_id,
    packageName: d.package_name,
    blockContents: d.block_contents,
  };
}

function fromFileEntry(f: RestFileEntry): FileEntry {
  return {
    uniqueId: f.unique_id,
    name: f.name,
    resourceType: f.resource_type,
    packageName: f.package_name,
    originalFilePath: f.original_file_path,
    patchPath: f.patch_path,
  };
}

/** Map the `GET /api/v1/files` envelope to a flat domain {@link FileEntry}
 *  list. Null body (404) → empty list. */
export function fromFileList(r: RestFileListResponse | null): FileEntry[] {
  return r ? r.files.map(fromFileEntry) : [];
}

export function fromLineageResponse(r: RestLineageResponse): LineageGraph {
  return {
    nodes: r.nodes.map((n) => ({
      uniqueId: n.unique_id,
      name: n.name,
      resourceType: asResourceType(n.resource_type),
      description: null,
      packageName: '',
      tags: [],
      materialized: n.materialized ?? null,
      originalFilePath: n.original_file_path ?? null,
    })),
    edges: r.edges.map((e) => ({
      upstreamUniqueId: e.from_id,
      downstreamUniqueId: e.to_id,
    })),
  };
}

// --- facets ----------------------------------------------------------------

/** Response body of `GET /api/v1/models/facets`. */
export interface RestModelFacets {
  modeling_layers: RestFacetValue[];
  owners: RestFacetValue[];
  packages: RestFacetValue[];
}

/** Response body of `GET /api/v1/tests/facets`. */
export interface RestTestFacets {
  results: RestFacetValue[];
  test_types: RestFacetValue[];
}

/** Response body of `GET /api/v1/macros/facets`. */
export interface RestMacroFacets {
  packages: RestFacetValue[];
}

interface RestFacetValue {
  value: string;
  count?: number | null;
}

function fromFacetValues(values: RestFacetValue[] | undefined): FacetValue[] {
  return (values ?? []).map((v) => ({ value: v.value, count: v.count ?? null }));
}

/** Map the model facets response, keyed by the `AssetFilter` field each drives. */
export function fromModelFacets(d: RestModelFacets): Facets {
  return {
    modelingLayers: fromFacetValues(d.modeling_layers),
    owners: fromFacetValues(d.owners),
    packages: fromFacetValues(d.packages),
  };
}

/** Map the test facets response, keyed by the `AssetFilter` field each drives. */
export function fromTestFacets(d: RestTestFacets): Facets {
  return {
    results: fromFacetValues(d.results),
    testTypes: fromFacetValues(d.test_types),
  };
}

/** Map the macro facets response, keyed by the `AssetFilter` field it drives. */
export function fromMacroFacets(d: RestMacroFacets): Facets {
  return { packages: fromFacetValues(d.packages) };
}

// --- search ----------------------------------------------------------------

/** One hit's match field, as serialized by `GET /api/v1/search`. Mirrors the
 *  domain {@link MatchedField} vocabulary. */
type RestSearchMatchedField = MatchedField;

/** One hit in a search response. Type-specific fields are absent (not null)
 *  when not applicable to the hit's `resource_type`. */
interface RestSearchHit {
  unique_id: string;
  resource_type: string;
  name: string | null;
  fqn?: string[];
  package_name: string | null;
  materialized?: string;
  access_level?: string;
  source_name?: string;
  freshness_checked?: boolean;
  test_type?: string;
  exposure_type?: string;
  executed_at?: string | null;
}

interface RestSearchEdge {
  matched_field: RestSearchMatchedField | null;
  /** HTML fragment with `<b>…</b>` runs marking matched substrings. */
  highlight: string | null;
  hit: RestSearchHit;
}

/** Response body of `GET /api/v1/search`. */
export interface RestSearchResponse {
  data: RestSearchEdge[];
  page_info: RestPageInfo;
}

/** Response body of `GET /api/v1/search/facets`. */
export interface RestSearchFacets {
  accesses: RestFacetValue[];
  modeling_layers: RestFacetValue[];
  materialization_types: RestFacetValue[];
  tags: RestFacetValue[];
  packages: RestFacetValue[];
}

/** Map a search response to a domain {@link Page} of {@link SearchHit}, folding
 *  each edge's `matched_field`/`highlight` onto the hit. `nextCursor` is the
 *  `end_cursor` only while another page exists (mirrors {@link toPage}). */
export function fromSearchResponse(r: RestSearchResponse): Page<SearchHit> {
  return {
    items: r.data.map((edge) => {
      const h = edge.hit;
      return {
        uniqueId: h.unique_id,
        resourceType: asResourceType(h.resource_type),
        name: h.name,
        packageName: h.package_name,
        fqn: h.fqn,
        matchedField: edge.matched_field,
        highlight: edge.highlight,
        materialized: h.materialized,
        access: h.access_level,
        sourceName: h.source_name,
        freshnessChecked: h.freshness_checked,
        testType: h.test_type,
        exposureType: h.exposure_type,
        executedAt: h.executed_at,
      };
    }),
    nextCursor: r.page_info.has_next_page ? (r.page_info.end_cursor ?? null) : null,
    totalCount: r.page_info.total_count ?? null,
  };
}

/** Map the project-wide search facets response, snake_case → camelCase. */
export function fromSearchFacets(d: RestSearchFacets): SearchFacets {
  return {
    accesses: fromFacetValues(d.accesses),
    modelingLayers: fromFacetValues(d.modeling_layers),
    materializationTypes: fromFacetValues(d.materialization_types),
    tags: fromFacetValues(d.tags),
    packages: fromFacetValues(d.packages),
  };
}

export function fromColumnLineageResponse(
  r: RestColumnLineageResponse,
): ColumnLineageGraph {
  // Group edges by destination column to build ColumnLineageNode entries.
  const nodeMap = new Map<
    string,
    { nodeUniqueId: string; columnName: string; parentColumns: string[]; kind: string }
  >();

  for (const edge of r.edges) {
    const toKey = `${edge.to_node}:${edge.to_column}`;
    if (!nodeMap.has(toKey)) {
      nodeMap.set(toKey, {
        nodeUniqueId: edge.to_node,
        columnName: edge.to_column,
        parentColumns: [],
        kind: edge.kind,
      });
    }
    nodeMap.get(toKey)!.parentColumns.push(`${edge.from_node}:${edge.from_column}`);
  }

  return {
    nodes: Array.from(nodeMap.entries()).map(([key, n]) => ({
      uniqueId: key,
      nodeUniqueId: n.nodeUniqueId,
      name: n.columnName,
      parentColumns: n.parentColumns,
      transformationType: n.kind ?? null,
      isError: false,
      errorCategory: null,
    })),
    edges: r.edges.map((e) => ({
      fromNodeUniqueId: e.from_node,
      fromColumn: e.from_column,
      toNodeUniqueId: e.to_node,
      toColumn: e.to_column,
      transformationType: e.kind ?? null,
    })),
  };
}
