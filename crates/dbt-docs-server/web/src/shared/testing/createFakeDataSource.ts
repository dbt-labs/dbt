// Imported only so the `{@link LIST_REGISTRY}` reference below resolves; eslint
// cannot see JSDoc links as usage.
// eslint-disable-next-line @typescript-eslint/no-unused-vars
import { LIST_REGISTRY } from '../data-sources/duckdb/lists';
import type { MetadataDataSource } from '../data-sources/MetadataDataSource';
import type { AssetArgs } from '../typings/args';
import type {
  Asset,
  AssetSummary,
  ModelAsset,
  ModelSummary,
  ResourceType,
} from '../typings/domain/asset';
import type { Capabilities } from '../typings/domain/capabilities';
import type { AssetCounts } from '../typings/domain/counts';
import type { Distribution } from '../typings/domain/distribution';
import type { ExecutionInfo } from '../typings/domain/executionInfo';
import type { Facets } from '../typings/domain/facets';
import type { FileEntry } from '../typings/domain/files';
import type { ProjectOverview } from '../typings/domain/overview';
import type { Project } from '../typings/domain/project';
import type { SearchFacets, SearchHit, SearchResult } from '../typings/domain/search';
import type { Page } from '../typings/page';

/** An {@link Asset} optionally carrying execution state, used by the fake to
 *  drive polling-related tests. */
export type FakeAsset = Asset & { executionInfo?: ExecutionInfo | null };

/** A minimal, valid {@link ModelAsset} the fake returns by default. */
export function makeFakeModelAsset(overrides: Partial<ModelAsset> = {}): ModelAsset {
  return {
    uniqueId: 'model.jaffle_shop.customers',
    name: 'customers',
    resourceType: 'model',
    description: 'A fake model for tests.',
    packageName: 'jaffle_shop',
    tags: [],
    rawCode: 'select 1 as id',
    compiledCode: 'select 1 as id',
    language: 'sql',
    access: 'protected',
    contractEnforced: false,
    materializedType: 'table',
    group: null,
    relation: { database: 'analytics', schema: 'dbt', identifier: 'customers' },
    columns: [],
    ...overrides,
  };
}

/** A minimal, valid {@link ModelSummary} the fake list returns by default. */
export function makeFakeAssetSummary(
  overrides: Partial<ModelSummary> = {},
): ModelSummary {
  return {
    uniqueId: 'model.jaffle_shop.customers',
    name: 'customers',
    resourceType: 'model',
    description: null,
    packageName: 'jaffle_shop',
    tags: [],
    ...overrides,
  };
}

/** A minimal {@link Facets} map for tests and stories; merge `overrides` to
 *  supply per-field options. */
export function makeFakeFacets(overrides: Facets = {}): Facets {
  return { ...overrides };
}

/** A minimal {@link AssetCounts} map for tests and stories; merge `overrides`
 *  to supply per-type tallies. */
export function makeFakeAssetCounts(overrides: AssetCounts = {}): AssetCounts {
  return { ...overrides };
}

/** A minimal, valid {@link Project} for tests and stories. */
export function makeFakeProject(overrides: Partial<Project> = {}): Project {
  return {
    name: 'jaffle_shop',
    adapterType: 'duckdb',
    dbtVersion: '1.8.0',
    description: 'A fake project for tests.',
    gitBranch: 'main',
    gitIsDirty: false,
    ...overrides,
  };
}

/** A minimal, valid {@link FileEntry} for tests and stories. */
export function makeFakeFileEntry(overrides: Partial<FileEntry> = {}): FileEntry {
  return {
    uniqueId: 'model.jaffle_shop.customers',
    name: 'customers',
    resourceType: 'model',
    packageName: 'jaffle_shop',
    originalFilePath: 'models/customers.sql',
    patchPath: null,
    ...overrides,
  };
}

/** A minimal, valid {@link SearchHit} for tests and stories. */
export function makeFakeSearchHit(overrides: Partial<SearchHit> = {}): SearchHit {
  return {
    uniqueId: 'model.jaffle_shop.customers',
    resourceType: 'model',
    name: 'customers',
    packageName: 'jaffle_shop',
    matchedField: 'name',
    highlight: null,
    ...overrides,
  };
}

/** Empty {@link SearchFacets} — every facet list empty. */
const EMPTY_SEARCH_FACETS: SearchFacets = {
  accesses: [],
  modelingLayers: [],
  materializationTypes: [],
  tags: [],
  packages: [],
};

/** Fields shared by every asset/summary, parameterized by resource type. */
function fakeBase(resourceType: ResourceType) {
  return {
    uniqueId: `${resourceType}.jaffle_shop.fake`,
    name: 'fake',
    resourceType,
    description: null,
    packageName: 'jaffle_shop',
    tags: [] as string[],
  };
}

/**
 * Build a minimal valid {@link Asset} detail for any registry resource type.
 * Mirrors the real source's per-type coverage (derived from {@link LIST_REGISTRY})
 * so a fake can stand in for any type the source serves.
 */
export function makeFakeAsset(
  resourceType: ResourceType,
  overrides: Partial<Asset> = {},
): Asset {
  const base = fakeBase(resourceType);
  let asset: Asset;
  switch (resourceType) {
    case 'model':
    case 'seed':
    case 'snapshot':
      asset = makeFakeModelAsset({ ...base, resourceType });
      break;
    case 'source':
      asset = {
        ...base,
        resourceType: 'source',
        sourceName: 'raw',
        identifier: 'fake',
        loader: null,
        freshness: null,
        relation: null,
        columns: [],
      };
      break;
    case 'exposure':
      asset = {
        ...base,
        resourceType: 'exposure',
        exposureType: 'dashboard',
        maturity: null,
        ownerName: null,
        ownerEmail: null,
        url: null,
        dependsOn: [],
      };
      break;
    case 'metric':
      asset = {
        ...base,
        resourceType: 'metric',
        label: null,
        typeParams: { kind: 'simple', measure: { name: 'm', filter: null } },
        group: null,
      };
      break;
    case 'macro':
      asset = {
        ...base,
        resourceType: 'macro',
        macroSql: '{% macro fake() %}{% endmacro %}',
        arguments: [],
        path: 'macros/fake.sql',
      };
      break;
    case 'semantic_model':
      asset = {
        ...base,
        resourceType: 'semantic_model',
        modelUniqueId: null,
        dimensions: [],
        measures: [],
        entities: [],
      };
      break;
    case 'test':
      asset = {
        ...base,
        resourceType: 'test',
        testType: 'generic',
        severity: null,
        columnName: null,
        dependsOn: [],
        rawCode: null,
        compiledCode: null,
      };
      break;
    case 'unit_test':
      asset = {
        ...base,
        resourceType: 'unit_test',
        modelUniqueId: null,
        given: [],
        expect: { rows: [] },
      };
      break;
    case 'saved_query':
      asset = {
        ...base,
        resourceType: 'saved_query',
        label: null,
        queryParams: { metrics: [], groupBy: [], where: [] },
        exports: [],
      };
      break;
    case 'group':
      asset = {
        ...base,
        resourceType: 'group',
        ownerName: null,
        ownerEmail: null,
      };
      break;
    default:
      // Registry-miss types (analysis/function/operation) resolve to a model.
      asset = makeFakeModelAsset({ ...base, resourceType: 'model' });
  }
  return { ...asset, ...overrides } as Asset;
}

/**
 * Build a minimal valid {@link AssetSummary} for any registry resource type.
 * Every summary is `AssetBase` + a literal `resourceType` + optional fields, so
 * the common base suffices.
 */
export function makeFakeSummary(
  resourceType: ResourceType,
  overrides: Partial<AssetSummary> = {},
): AssetSummary {
  return { ...fakeBase(resourceType), ...overrides } as AssetSummary;
}

/** Feature capabilities with every flag off — the conservative default. */
function allFalseCapabilities(): Capabilities {
  return {
    hasColumnLineage: false,
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
    hasDbtState: false,
  };
}

const EMPTY_PAGE: Page<AssetSummary> = {
  items: [],
  nextCursor: null,
  totalCount: null,
};

/** Options for {@link createFakeDataSource}. */
export interface FakeDataSourceOptions {
  /**
   * When true, stub every optional contract method with a benign default (so
   * the fake satisfies the full {@link MetadataDataSource} surface). Default
   * (false) keeps the minimal shape — only `fetchAsset` — so capability-gate
   * tests can observe the optional fetchers as *absent*.
   */
  full?: boolean;
}

/**
 * Build an in-memory {@link MetadataDataSource} for tests and stories — drives
 * components with no network. By default only `fetchAsset` is present (resolving
 * to a minimal `ModelAsset`); pass `{ full: true }` to stub every optional
 * method too. `overrides` always wins (spread last) in either mode.
 */
export function createFakeDataSource(
  overrides: Partial<MetadataDataSource> = {},
  opts: FakeDataSourceOptions = {},
): MetadataDataSource {
  const fullDefaults: Partial<MetadataDataSource> = opts.full
    ? {
        fetchAssetList: async (): Promise<Page<AssetSummary>> => EMPTY_PAGE,
        fetchFacets: async (): Promise<Facets> => ({}),
        fetchLineage: async () => ({ nodes: [], edges: [] }),
        fetchColumnLineage: async () => ({
          kind: 'ok' as const,
          graph: { nodes: [], edges: [] },
        }),
        fetchCapabilities: async (): Promise<Capabilities> => allFalseCapabilities(),
        fetchDistribution: async (): Promise<Distribution> => ({
          isProprietary: false,
          isLoggedIn: false,
        }),
        fetchAssetCounts: async (): Promise<AssetCounts> => ({}),
        fetchProject: async (): Promise<Project> => makeFakeProject(),
        // Null is the real "no authored overview" answer, not a missing stub —
        // consumers render their own default from it.
        fetchOverview: async (): Promise<ProjectOverview | null> => null,
        fetchFiles: async (): Promise<FileEntry[]> => [],
        fetchSearch: async (): Promise<SearchResult> => ({
          kind: 'ok',
          page: { items: [], nextCursor: null, totalCount: null },
        }),
        fetchSearchFacets: async (): Promise<SearchFacets> => EMPTY_SEARCH_FACETS,
        onAppliedUpdatedAt: () => {},
        onDefinitionUpdatedAt: () => {},
      }
    : {};

  return {
    id: 'fake',
    supportedFilters: new Set<string>(),
    fetchAsset: async (_args: AssetArgs): Promise<Asset | null> => makeFakeModelAsset(),
    ...fullDefaults,
    ...overrides,
  };
}
