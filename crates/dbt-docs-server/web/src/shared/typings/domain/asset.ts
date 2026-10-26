export type ResourceType =
  | 'model'
  | 'seed'
  | 'snapshot'
  | 'source'
  | 'exposure'
  | 'metric'
  | 'macro'
  | 'semantic_model'
  | 'test'
  | 'unit_test'
  | 'saved_query'
  | 'function'
  | 'group'
  | 'analysis'
  | 'operation';

export type Relation = { database: string; schema: string; identifier: string };

/**
 * Column type provenance:
 * - `dataType`: resolved type the UI should display. Precedence (consumer-side):
 *   catalogType ?? declaredType ?? null. Adapters fill this.
 * - `declaredType`: type declared in schema.yml (user-authored).
 * - `catalogType`: type observed from the warehouse catalog (authoritative).
 */
export type AssetColumn = {
  name: string;
  description: string | null;
  dataType: string | null;
  declaredType: string | null;
  catalogType: string | null;
  tags: string[];
  meta: Record<string, unknown>;
  /** Ordinal position in the warehouse. */
  index?: number | null;
  /** Primary-key membership. Source: explorer-only today. */
  isPrimaryKey?: boolean | null;
};

/**
 * Common fields available on every asset.
 *
 * Optional fields follow approach #1: source-tagged optionals. Field is
 * canonical (could be supplied by either source); adapter fills when
 * available, leaves `undefined` / `null` otherwise. Components render
 * conditionally — do NOT introduce source-specific Asset variants.
 *
 * Tag each optional field's source in JSDoc when its only supplied by one:
 *   "Source: docs-only" | "Source: explorer-only"
 */
export type AssetBase = {
  uniqueId: string;
  name: string;
  resourceType: ResourceType;
  description: string | null;
  packageName: string;
  tags: string[];
  /** Project-relative file path of the resource.*/
  filePath?: string | null;
  /** Original (pre-compilation) path. Source: docs-only. */
  originalFilePath?: string | null;
  /** schema.yml patch path, if metadata is split out.*/
  patchPath?: string | null;
  /** Fully-qualified name parts (project, folder, ..., name). */
  fqn?: string[] | null;
  /** User-authored config block. */
  config?: Record<string, unknown> | null;
  /** User-authored meta block.*/
  meta?: Record<string, unknown> | null;
  /** Upstream dependency uniqueIds. Source: both. */
  dependsOn?: string[] | null;
  /** Downstream referencing uniqueIds. Source: both. */
  referencedBy?: string[] | null;
  /** Materialization of the underlying relation, when known. Carried on
   *  lineage summary nodes so the DAG can render materialization icons. */
  materialized?: MaterializationType | null;
};

export type FreshnessThreshold = {
  count: number;
  period: 'minute' | 'hour' | 'day';
};

export type FreshnessConfig = {
  warnAfter: FreshnessThreshold | null;
  errorAfter: FreshnessThreshold | null;
  filter: string | null;
};

export type AccessLevel = 'public' | 'protected' | 'private';

/** Known materialization names. Open-ended via `(string & {})` for custom mats. */
export type MaterializationType =
  | 'table'
  | 'view'
  | 'incremental'
  | 'ephemeral'
  | 'materialized_view'
  | 'dynamic_table'
  | (string & {});

export type ModelAsset = AssetBase & {
  resourceType: 'model' | 'seed' | 'snapshot';
  rawCode: string | null;
  compiledCode: string | null;
  language: 'sql' | 'python' | null;
  access: AccessLevel | null;
  contractEnforced: boolean | null;
  materializedType: MaterializationType | null;
  group: string | null;
  relation: Relation | null;
  columns: AssetColumn[];
  /** Warehouse row count from latest catalog. Source: explorer-only today. */
  rowCountStat?: number | null;
  /** Warehouse byte size from latest catalog. Source: explorer-only today. */
  bytesStat?: number | null;
  /** Declared primary key column names. Source: explorer-only today. */
  primaryKey?: string[] | null;
  /** Resource owner (free-form string). Source: explorer-only today. */
  owner?: string | null;
};

export type SourceAsset = AssetBase & {
  resourceType: 'source';
  sourceName: string;
  identifier: string;
  loader: string | null;
  freshness: FreshnessConfig | null;
  relation: Relation | null;
  columns: AssetColumn[];
  /** The source block's own description, distinct from `description` (this table's). */
  sourceDescription: string | null;
  /** Latest freshness run status (e.g. "pass", "warn", "error"). Source: docs-only. */
  freshnessStatus?: string | null;
  /** ISO timestamp of the most recent freshness check. Source: docs-only. */
  freshnessMaxLoadedAt?: string | null;
};

export type ExposureAsset = AssetBase & {
  resourceType: 'exposure';
  exposureType: 'analysis' | 'application' | 'dashboard' | 'ml' | 'notebook';
  maturity: 'low' | 'medium' | 'high' | null;
  ownerName: string | null;
  ownerEmail: string | null;
  url: string | null;
  dependsOn: string[];
};

export type Kind = 'simple' | 'ratio' | 'cumulative' | 'derived';

export type MeasureRef = { name: string; filter: string | null };
export type MetricInput = { name: string; alias: string | null; filter: string | null };

/** Discriminated by `kind`. Matches dbt SL metric-type contracts. */
export type MetricTypeParams =
  | { kind: 'simple'; measure: MeasureRef }
  | { kind: 'ratio'; numerator: MetricInput; denominator: MetricInput }
  | {
      kind: 'cumulative';
      measure: MeasureRef;
      window: string | null;
      grainToDate: string | null;
    }
  | { kind: 'derived'; metrics: MetricInput[]; expr: string | null };

export type MetricAsset = AssetBase & {
  resourceType: 'metric';
  label: string | null;
  typeParams: MetricTypeParams;
  group: string | null;
};

export type Argument = {
  name: string;
  type: string | null;
  description: string | null;
};

export type MacroAsset = AssetBase & {
  resourceType: 'macro';
  macroSql: string;
  arguments: Argument[];
  path: string;
};

export type Dimension = {
  name: string;
  type: 'categorical' | 'time';
  description: string | null;
};
export type Measure = {
  name: string;
  agg: string;
  expr: string | null;
  description: string | null;
};
export type Entity = {
  name: string;
  type: 'primary' | 'foreign' | 'unique' | 'natural';
  expr: string | null;
};

export type QueryExport = {
  name: string;
  config: Record<string, unknown>;
};

export type SemanticModelAsset = AssetBase & {
  resourceType: 'semantic_model';
  /** uniqueId of the underlying model this semantic model wraps. */
  modelUniqueId: string | null;
  dimensions: Dimension[];
  measures: Measure[];
  entities: Entity[];
};

export type TestAsset = AssetBase & {
  resourceType: 'test';
  testType: 'generic' | 'singular';
  severity: 'warn' | 'error' | null;
  columnName: string | null;
  dependsOn: string[];
  rawCode: string | null;
  compiledCode: string | null;
};

export type SavedQueryParams = {
  metrics: string[];
  groupBy: string[];
  where: string[];
  orderBy?: string[] | null;
  limit?: number | null;
};

export type MetricInfo = {
  type: string;
  expression?: string | null;
  grainToDate?: string | null;
  window?: { count: number; granularity: string } | null;
  measure?: string | null;
  numerator?: string | null;
  denominator?: string | null;
  filters?: string[] | null;
};

export type SavedQueryAsset = AssetBase & {
  resourceType: 'saved_query';
  label: string | null;
  queryParams: SavedQueryParams;
  exports: Array<{
    name: string;
    exportAs: 'table' | 'view' | 'cache';
    schema: string | null;
  }>;
};

export type FunctionAsset = AssetBase & {
  resourceType: 'function';
  rawCode: string | null;
  compiledCode: string | null;
  language: 'sql' | 'python' | null;
  arguments: Argument[];
  returnType: string | null;
  relation: Relation | null;
};

export type GroupAsset = AssetBase & {
  resourceType: 'group';
  ownerName: string | null;
  ownerEmail: string | null;
  ownerGithub?: string | null; // Source: docs-only
  ownerSlack?: string | null; // Source: docs-only
  models?: Array<{
    uniqueId: string;
    name: string;
    database: string | null;
    schema: string | null;
  }> | null; // Source: docs-only
};

export type AnalysisAsset = AssetBase & {
  resourceType: 'analysis';
  rawCode: string | null;
  compiledCode: string | null;
  language: 'sql' | 'python' | null;
};

export type UnitTestAsset = AssetBase & {
  resourceType: 'unit_test';
  modelUniqueId: string | null;
  given: Array<{ input: string; rows: Record<string, unknown>[] }>;
  expect: { rows: Record<string, unknown>[] };
};

export type OperationAsset = AssetBase & {
  resourceType: 'operation';
  rawCode: string | null;
  compiledCode: string | null;
};

export type Asset =
  | ModelAsset
  | SourceAsset
  | ExposureAsset
  | MetricAsset
  | MacroAsset
  | SemanticModelAsset
  | TestAsset
  | UnitTestAsset
  | SavedQueryAsset
  | FunctionAsset
  | GroupAsset
  | AnalysisAsset
  | OperationAsset;

// ---------------------------------------------------------------------------
// List summaries.
//
// `AssetSummary` is a discriminated union mirroring the `Asset` detail union:
// each member is `AssetBase` plus the type-specific fields the list table for
// that resource renders. Optional fields follow the same source-tagging
// convention as `AssetBase` — they're whatever the per-type list endpoint
// happens to supply, left `undefined` otherwise. Components narrow on
// `resourceType`.
// ---------------------------------------------------------------------------

export type ModelSummary = AssetBase & {
  resourceType: 'model';
  /** Modeling layer (e.g. "staging", "marts"). Source: docs-only. */
  modelingLayer?: string | null;
  /** Resource owner (free-form string). Source: docs-only. */
  owner?: string | null;
  /** ISO timestamp of the latest run. Source: docs-only. */
  executedAt?: string | null;
  /** Warehouse row count from latest catalog. Source: docs-only. */
  rowCountStat?: number | null;
};

export type SourceSummary = AssetBase & {
  resourceType: 'source';
  /** Parent source collection name. Source: docs-only. */
  sourceName?: string | null;
  /** Warehouse database name. Source: docs-only. */
  databaseName?: string | null;
  /** Warehouse schema name. Source: docs-only. */
  schemaName?: string | null;
};

export type SeedSummary = AssetBase & {
  resourceType: 'seed';
  /** Warehouse row count. Source: docs-only. */
  rowCount?: number | null;
  /** ISO timestamp of the latest run. Source: docs-only. */
  executedAt?: string | null;
};

export type SnapshotSummary = AssetBase & {
  resourceType: 'snapshot';
  /** Warehouse row count from latest catalog. Source: docs-only. */
  rowCountStat?: number | null;
  /** Warehouse byte size from latest catalog. Source: docs-only. */
  bytesStat?: number | null;
  /** ISO timestamp of last warehouse modification. Source: docs-only. */
  lastModifiedStat?: string | null;
};

export type TestSummary = AssetBase & {
  resourceType: 'test' | 'unit_test';
  /** Whether this is a data test or a unit test. Source: docs-only. */
  testType?: 'data' | 'unit';
  /** Latest run status (e.g. "pass", "fail", "warn", "error"). Source: docs-only. */
  status?: string | null;
  /** uniqueId of the node this test asserts against. Source: docs-only. */
  testedNodeUniqueId?: string | null;
  /** Column the test targets, when column-scoped. Source: docs-only. */
  testedColumn?: string | null;
};

export type MetricSummary = AssetBase & {
  resourceType: 'metric';
  /** Metric kind (simple/ratio/cumulative/derived). Source: docs-only. */
  metricType?: string | null;
};

export type SemanticModelSummary = AssetBase & {
  resourceType: 'semantic_model';
  /** Entity names declared on the semantic model. Source: docs-only. */
  entities?: string[];
};

export type SavedQuerySummary = AssetBase & {
  resourceType: 'saved_query';
};

export type MacroSummary = AssetBase & {
  resourceType: 'macro';
  /** Argument names (the list column renders names only). Source: docs-only. */
  arguments?: string[];
};

export type GroupSummary = AssetBase & {
  resourceType: 'group';
  /** Group owner name. Source: docs-only. */
  ownerName?: string | null;
  /** Group owner email. Source: docs-only. */
  ownerEmail?: string | null;
  /** Group owner GitHub handle. Source: docs-only. */
  ownerGithub?: string | null;
  /** Group owner Slack handle. Source: docs-only. */
  ownerSlack?: string | null;
  /** Number of models in the group. Source: docs-only. */
  modelCount?: number | null;
};

export type ExposureSummary = AssetBase & {
  resourceType: 'exposure';
  /** Exposure kind (analysis/application/dashboard/ml/notebook). Source: docs-only. */
  exposureType?: string | null;
  /** Exposure owner name. Source: docs-only. */
  ownerName?: string | null;
  /** Exposure owner email. Source: docs-only. */
  ownerEmail?: string | null;
};

export type AssetSummary =
  | ModelSummary
  | SourceSummary
  | SeedSummary
  | SnapshotSummary
  | TestSummary
  | MetricSummary
  | SemanticModelSummary
  | SavedQuerySummary
  | MacroSummary
  | GroupSummary
  | ExposureSummary;
