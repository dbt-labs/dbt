/**
 * `MetadataDataSource` variants for Storybook.
 *
 * Nearly every state a data-connected component can be in is a property of the
 * *source*, not of a prop: populated, empty, still loading, errored, or served by a
 * source that never implemented the surface at all (which the hooks report as an
 * error rather than an empty list — see `hooks/unsupportedSurface.ts`). Building
 * those five sources here keeps each story a single line and, more importantly, keeps
 * them consistent, so "Loading" means the same thing in every story that has one.
 *
 * `storyDataSource` pages for real (the cursor is a row offset), so a "Load more"
 * button in a story actually loads more instead of being a dead control.
 */

import type { MetadataDataSource } from '../data-sources/MetadataDataSource';
import type { AssetFilter, ListArgs } from '../typings/args';
import type {
  AssetSummary,
  ExposureSummary,
  GroupSummary,
  MacroSummary,
  MetricSummary,
  ModelSummary,
  ResourceType,
  SavedQuerySummary,
  SeedSummary,
  SemanticModelSummary,
  SnapshotSummary,
  SourceSummary,
  TestSummary,
} from '../typings/domain/asset';
import type { SearchFacets } from '../typings/domain/search';
import type { Page } from '../typings/page';
import { createFakeDataSource, makeFakeSummary } from './createFakeDataSource';
import {
  storyCapabilities,
  storyColumnLineage,
  storyCounts,
  storyExposure,
  storyFacets,
  storyFiles,
  storyGroup,
  storyLineage,
  storyMacro,
  storyMetric,
  storyModel,
  storyOverview,
  storySavedQuery,
  storySearchHits,
  storySemanticModel,
  storySource,
  storyTest,
} from './storyFixtures';

/** Every filter field any list honors — a story source should never hide a
 *  filter control just because the fake forgot to advertise it. */
const ALL_FILTERS: ReadonlySet<string> = new Set([
  'resourceTypes',
  'search',
  'tags',
  'modelingLayers',
  'owners',
  'packages',
  'results',
  'testTypes',
]);

/** A promise that never settles — the honest way to render a loading state, since
 *  it exercises the same code path a slow parquet read does. */
export function never<T>(): Promise<T> {
  return new Promise<T>(() => {});
}

/**
 * An already-rejected promise, for stories that pass a failed read directly as a
 * parameter (rather than through a data source).
 *
 * The no-op `catch` is load-bearing: a story's parameters are evaluated when the module
 * loads, so a bare `Promise.reject(...)` sits unhandled until whatever consumes it
 * mounts. That surfaces as an unhandled rejection, which fails the story test run —
 * and, because it happens during module evaluation, takes unrelated story files down
 * with it. Attaching a handler here marks it handled without changing what the
 * consumer sees.
 */
export function rejected<T>(message: string): Promise<T> {
  const promise = Promise.reject<T>(new Error(message));
  promise.catch(() => {});
  return promise;
}

const MODEL_NAMES = [
  'customers',
  'orders',
  'order_items',
  'products',
  'stg_customers',
  'stg_orders',
  'stg_products',
  'int_order_items_joined',
  'fct_orders',
  'dim_customers',
  'supplies',
  'locations',
];

const LAYERS = ['Staging', 'Intermediate', 'Marts'];
const OWNERS = ['data-platform', 'finance-analytics', null];
const PACKAGES = ['jaffle_shop', 'jaffle_shop', 'dbt_utils'];

/** Deterministic pseudo-variation, so a list looks heterogeneous without any story
 *  depending on a random draw. */
function pick<T>(values: readonly T[], i: number): T {
  return values[i % values.length] as T;
}

function modelSummaries(count: number): ModelSummary[] {
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `model.jaffle_shop.${pick(MODEL_NAMES, i)}_${i}`,
    name: `${pick(MODEL_NAMES, i)}`,
    resourceType: 'model' as const,
    description: i % 4 === 3 ? null : `Model ${pick(MODEL_NAMES, i)}.`,
    packageName: pick(PACKAGES, i),
    tags: i % 3 === 0 ? ['daily'] : [],
    modelingLayer: pick(LAYERS, i),
    owner: pick(OWNERS, i),
    executedAt: i % 5 === 4 ? null : `2026-02-11T1${i % 6}:12:00Z`,
    rowCountStat: i % 6 === 5 ? null : 1_000 * (i + 1) * 7,
  }));
}

function sourceSummaries(count: number): SourceSummary[] {
  const names = ['customers', 'orders', 'products', 'stores', 'events'];
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `source.jaffle_shop.raw.${pick(names, i)}_${i}`,
    name: pick(names, i),
    resourceType: 'source' as const,
    description: i % 3 === 2 ? null : `Raw ${pick(names, i)} table.`,
    packageName: 'jaffle_shop',
    tags: [],
    sourceName: i % 2 === 0 ? 'raw' : 'events',
    databaseName: 'raw',
    schemaName: i % 2 === 0 ? 'jaffle_shop' : 'segment',
  }));
}

function seedSummaries(count: number): SeedSummary[] {
  const names = ['country_codes', 'employees', 'store_hours', 'tax_rates'];
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `seed.jaffle_shop.${pick(names, i)}_${i}`,
    name: pick(names, i),
    resourceType: 'seed' as const,
    description: `Static ${pick(names, i)} lookup.`,
    packageName: 'jaffle_shop',
    tags: [],
    rowCount: 12 * (i + 1),
    executedAt: `2026-02-11T16:0${i % 10}:00Z`,
  }));
}

function snapshotSummaries(count: number): SnapshotSummary[] {
  const names = ['orders_snapshot', 'customers_snapshot', 'products_snapshot'];
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `snapshot.jaffle_shop.${pick(names, i)}_${i}`,
    name: pick(names, i),
    resourceType: 'snapshot' as const,
    description: i % 2 === 0 ? `SCD2 history for ${pick(names, i)}.` : null,
    packageName: 'jaffle_shop',
    tags: [],
    rowCountStat: 5_000 * (i + 1),
    bytesStat: 1_048_576 * (i + 1),
    lastModifiedStat: `2026-02-1${i % 2}T04:00:00Z`,
  }));
}

function testSummaries(count: number): TestSummary[] {
  const statuses = ['pass', 'pass', 'pass', 'warn', 'fail', 'error', 'skipped'];
  const columns = ['customer_id', 'order_id', null, 'status'];
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `test.jaffle_shop.assert_${i}`,
    name:
      i % 3 === 0
        ? `not_null_customers_customer_id_${i}`
        : `accepted_values_orders_status_${i}`,
    resourceType: 'test' as const,
    description: null,
    packageName: 'jaffle_shop',
    tags: [],
    testType: i % 7 === 6 ? ('unit' as const) : ('data' as const),
    status: pick(statuses, i),
    testedNodeUniqueId: 'model.jaffle_shop.customers',
    testedColumn: pick(columns, i),
  }));
}

function metricSummaries(count: number): MetricSummary[] {
  const names = ['revenue', 'order_count', 'aov', 'conversion_rate'];
  const kinds = ['simple', 'ratio', 'cumulative', 'derived'];
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `metric.jaffle_shop.${pick(names, i)}_${i}`,
    name: pick(names, i),
    resourceType: 'metric' as const,
    description: `Metric ${pick(names, i)}.`,
    packageName: 'jaffle_shop',
    tags: [],
    metricType: pick(kinds, i),
  }));
}

function semanticModelSummaries(count: number): SemanticModelSummary[] {
  const names = ['orders', 'customers', 'order_items'];
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `semantic_model.jaffle_shop.${pick(names, i)}_${i}`,
    name: pick(names, i),
    resourceType: 'semantic_model' as const,
    description: `Semantic model over ${pick(names, i)}.`,
    packageName: 'jaffle_shop',
    tags: [],
    entities: i % 2 === 0 ? ['order', 'customer'] : ['customer'],
  }));
}

function savedQuerySummaries(count: number): SavedQuerySummary[] {
  const names = ['weekly_revenue', 'monthly_orders', 'top_customers'];
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `saved_query.jaffle_shop.${pick(names, i)}_${i}`,
    name: pick(names, i),
    resourceType: 'saved_query' as const,
    description: `Saved query ${pick(names, i)}.`,
    packageName: 'jaffle_shop',
    tags: [],
  }));
}

function macroSummaries(count: number): MacroSummary[] {
  const names = ['cents_to_dollars', 'generate_schema_name', 'star', 'date_spine'];
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `macro.${pick(PACKAGES, i)}.${pick(names, i)}_${i}`,
    name: pick(names, i),
    resourceType: 'macro' as const,
    description: i % 3 === 2 ? null : `Macro ${pick(names, i)}.`,
    packageName: pick(PACKAGES, i),
    tags: [],
    arguments: i % 2 === 0 ? ['column_name', 'scale'] : [],
  }));
}

function groupSummaries(count: number): GroupSummary[] {
  const names = ['finance', 'marketing', 'product'];
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `group.jaffle_shop.${pick(names, i)}_${i}`,
    name: pick(names, i),
    resourceType: 'group' as const,
    description: `The ${pick(names, i)} analytics group.`,
    packageName: 'jaffle_shop',
    tags: [],
    ownerName: `${pick(names, i)} analytics`,
    ownerEmail: `${pick(names, i)}@example.com`,
    ownerGithub: `jaffle-${pick(names, i)}`,
    ownerSlack: `#${pick(names, i)}-analytics`,
    modelCount: 4 * (i + 1),
  }));
}

function exposureSummaries(count: number): ExposureSummary[] {
  const names = ['weekly_metrics', 'churn_model', 'exec_dashboard'];
  const kinds = ['dashboard', 'ml', 'analysis', 'application', 'notebook'];
  return Array.from({ length: count }, (_, i) => ({
    uniqueId: `exposure.jaffle_shop.${pick(names, i)}_${i}`,
    name: pick(names, i),
    resourceType: 'exposure' as const,
    description: `Exposure ${pick(names, i)}.`,
    packageName: 'jaffle_shop',
    tags: [],
    exposureType: pick(kinds, i),
    ownerName: 'Data Platform',
    ownerEmail: 'data-platform@example.com',
  }));
}

/**
 * A plausible list page for any resource type.
 *
 * Registry-miss types (`function`, `analysis`, `operation`) fall through to the
 * minimal `makeFakeSummary`, matching how the real source treats them.
 */
export function storySummaries(resourceType: ResourceType, count = 12): AssetSummary[] {
  switch (resourceType) {
    case 'model':
      return modelSummaries(count);
    case 'source':
      return sourceSummaries(count);
    case 'seed':
      return seedSummaries(count);
    case 'snapshot':
      return snapshotSummaries(count);
    case 'test':
    case 'unit_test':
      return testSummaries(count);
    case 'metric':
      return metricSummaries(count);
    case 'semantic_model':
      return semanticModelSummaries(count);
    case 'saved_query':
      return savedQuerySummaries(count);
    case 'macro':
      return macroSummaries(count);
    case 'group':
      return groupSummaries(count);
    case 'exposure':
      return exposureSummaries(count);
    default:
      return Array.from({ length: count }, (_, i) =>
        makeFakeSummary(resourceType, {
          uniqueId: `${resourceType}.jaffle_shop.fake_${i}`,
          name: `fake_${i}`,
        }),
      );
  }
}

/** The detail fixture for a resource type. */
function storyAssetFor(resourceType: ResourceType) {
  switch (resourceType) {
    case 'source':
      return storySource();
    case 'exposure':
      return storyExposure();
    case 'metric':
      return storyMetric();
    case 'macro':
      return storyMacro();
    case 'semantic_model':
      return storySemanticModel();
    case 'test':
      return storyTest();
    case 'saved_query':
      return storySavedQuery();
    case 'group':
      return storyGroup();
    default:
      return storyModel({ resourceType: resourceType === 'seed' ? 'seed' : 'model' });
  }
}

const STORY_SEARCH_FACETS: SearchFacets = {
  accesses: [
    { value: 'public', count: 24 },
    { value: 'protected', count: 96 },
    { value: 'private', count: 22 },
  ],
  modelingLayers: [
    { value: 'staging', count: 80 },
    { value: 'intermediate', count: 32 },
    { value: 'marts', count: 30 },
  ],
  materializationTypes: [
    { value: 'view', count: 88 },
    { value: 'table', count: 46 },
  ],
  tags: [
    { value: 'daily', count: 61 },
    { value: 'pii', count: 9 },
  ],
  packages: [
    { value: 'jaffle_shop', count: 142 },
    { value: 'dbt_utils', count: 18 },
  ],
};

/** Rows per list page. Small enough that "Load more" is reachable in a story. */
const STORY_PAGE_SIZE = 10;

/** Total rows the paging source will hand out across all pages. */
const STORY_TOTAL_ROWS = 24;

function pageOf<T>(rows: T[], cursor: string | null | undefined): Page<T> {
  const offset = cursor ? Number(cursor) : 0;
  const slice = rows.slice(offset, offset + STORY_PAGE_SIZE);
  const nextOffset = offset + slice.length;
  return {
    items: slice,
    nextCursor: nextOffset < rows.length ? String(nextOffset) : null,
    totalCount: rows.length,
  };
}

/**
 * The default story source: every surface implemented, every capability on, and
 * populated with the `storyFixtures` data.
 *
 * `overrides` wins, so a story that wants one surface to behave differently — empty,
 * hanging, rejecting — overrides just that fetcher and keeps the rest realistic.
 */
export function storyDataSource(
  overrides: Partial<MetadataDataSource> = {},
): MetadataDataSource {
  return {
    id: 'story',
    supportedFilters: ALL_FILTERS,
    fetchAsset: async ({ resourceType }) => storyAssetFor(resourceType),
    fetchAssetList: async (args: ListArgs<AssetFilter>) => {
      const resourceType = args.filter?.resourceTypes?.[0] ?? 'model';
      return pageOf(storySummaries(resourceType, STORY_TOTAL_ROWS), args.cursor);
    },
    fetchFacets: async () => storyFacets(),
    fetchLineage: async () => storyLineage(),
    fetchColumnLineage: async () => ({ kind: 'ok', graph: storyColumnLineage() }),
    fetchCapabilities: async () => storyCapabilities(),
    fetchDistribution: async () => ({
      isProprietary: true,
      isLoggedIn: true,
      version: '2.0.0',
    }),
    fetchAssetCounts: async () => storyCounts(),
    fetchProject: async () => ({
      name: 'jaffle_shop',
      description: 'The canonical example project.',
      dbtVersion: '1.10.2',
      adapterType: 'snowflake',
      gitBranch: 'main',
      gitSha: 'a1b2c3d',
      gitIsDirty: false,
    }),
    fetchOverview: async () => storyOverview(),
    fetchFiles: async () => storyFiles(),
    fetchSearch: async (args) => ({
      kind: 'ok',
      page: pageOf(storySearchHits(), args.cursor),
    }),
    fetchSearchFacets: async () => STORY_SEARCH_FACETS,
    onAppliedUpdatedAt: () => {},
    onDefinitionUpdatedAt: () => {},
    ...overrides,
  };
}

/** Every surface implemented, every surface empty — the "brand new project" and
 *  "filters match nothing" state. */
export function emptyStorySource(
  overrides: Partial<MetadataDataSource> = {},
): MetadataDataSource {
  return storyDataSource({
    fetchAsset: async () => null,
    fetchAssetList: async () => ({ items: [], nextCursor: null, totalCount: 0 }),
    fetchFacets: async () => ({}),
    fetchLineage: async () => ({ nodes: [], edges: [] }),
    fetchColumnLineage: async () => ({
      kind: 'ok',
      graph: { nodes: [], edges: [] },
    }),
    fetchAssetCounts: async () => ({}),
    fetchOverview: async () => null,
    fetchFiles: async () => [],
    fetchSearch: async () => ({
      kind: 'ok',
      page: { items: [], nextCursor: null, totalCount: 0 },
    }),
    ...overrides,
  });
}

/** Nothing ever resolves — the first-paint / cold-cache state. */
export function loadingStorySource(
  overrides: Partial<MetadataDataSource> = {},
): MetadataDataSource {
  return storyDataSource({
    fetchAsset: never,
    fetchAssetList: never,
    fetchFacets: never,
    fetchLineage: never,
    fetchColumnLineage: never,
    fetchAssetCounts: never,
    fetchProject: never,
    fetchOverview: never,
    fetchFiles: never,
    fetchSearch: never,
    ...overrides,
  });
}

/** Every fetch rejects. Components surface this as inline error copy rather than
 *  as an empty state. */
export function failingStorySource(
  message = 'Failed to read the docs index.',
  overrides: Partial<MetadataDataSource> = {},
): MetadataDataSource {
  const boom = async (): Promise<never> => {
    throw new Error(message);
  };
  return storyDataSource({
    fetchAsset: boom,
    fetchAssetList: boom,
    fetchFacets: boom,
    fetchLineage: boom,
    fetchColumnLineage: boom,
    fetchAssetCounts: boom,
    fetchProject: boom,
    fetchOverview: boom,
    fetchFiles: boom,
    fetchSearch: boom,
    ...overrides,
  });
}

/**
 * A source that implements only `fetchAsset`, so a surface that was never ported
 * reports itself unsupported. Distinct from
 * {@link emptyStorySource}: the hooks turn an absent fetcher into the
 * `UNSUPPORTED_SURFACE_MESSAGE` error, not into an empty list.
 */
export function minimalStorySource(
  overrides: Partial<MetadataDataSource> = {},
): MetadataDataSource {
  return createFakeDataSource({
    id: 'story-minimal',
    fetchAsset: async ({ resourceType }) => storyAssetFor(resourceType),
    ...overrides,
  });
}

/** Column lineage reported unavailable by the source — the upsell path. Everything
 *  else stays populated. */
export function gatedLineageStorySource(): MetadataDataSource {
  return storyDataSource({
    fetchColumnLineage: async () => ({ kind: 'gated' }),
    fetchCapabilities: async () => storyCapabilities({ hasColumnLineage: false }),
  });
}
