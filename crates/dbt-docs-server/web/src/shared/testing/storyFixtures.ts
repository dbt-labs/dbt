/**
 * Fixtures for Storybook.
 *
 * `createFakeDataSource.ts` next door already builds *minimal valid* objects — the
 * right default for a test, which wants nothing on screen it did not put there. A
 * story wants the opposite: enough populated fields that a component renders the way a
 * real project makes it render, so a reviewer sees spacing, truncation and
 * filled-vs-empty states rather than a grid of nulls.
 *
 * So these build on the `makeFake*` helpers rather than replacing them, and every
 * builder takes `overrides` so a story can knock one field back to null to show the
 * degraded state.
 *
 * The data is deliberately jaffle_shop-shaped, matching the fake source, so uniqueIds
 * line up when a story wires several fixtures into the same graph.
 */

import type { NodeSummary } from '../../types';
import type { BootstrapData } from '../data-sources/duckdb/bootstrap';
import type {
  AssetColumn,
  ExposureAsset,
  GroupAsset,
  MacroAsset,
  MetricAsset,
  ModelAsset,
  SavedQueryAsset,
  SemanticModelAsset,
  SourceAsset,
  TestAsset,
} from '../typings/domain/asset';
import type { Capabilities } from '../typings/domain/capabilities';
import type { AssetCounts } from '../typings/domain/counts';
import type { ExecutionInfo } from '../typings/domain/executionInfo';
import type { Facets } from '../typings/domain/facets';
import type { FileEntry } from '../typings/domain/files';
import type { ColumnLineageGraph, LineageGraph } from '../typings/domain/lineage';
import type { ProjectOverview } from '../typings/domain/overview';
import type { SearchHit } from '../typings/domain/search';
import { makeFakeModelAsset } from './createFakeDataSource';

/** A fixed "now" so a story never renders a relative timestamp that drifts between
 *  screenshots. Sits a couple of hours after the run timestamps below. */
export const STORY_NOW = '2026-02-11T18:20:00Z';

/** Columns with descriptions, types and a primary key — the documented case. */
export function storyColumns(): AssetColumn[] {
  return [
    {
      name: 'customer_id',
      description: 'Surrogate key for the customer. Unique and never reused.',
      dataType: 'varchar',
      declaredType: 'varchar',
      catalogType: 'VARCHAR',
      tags: ['pii'],
      meta: {},
      index: 1,
      isPrimaryKey: true,
    },
    {
      name: 'first_order_date',
      description: 'Date of the first order this customer placed.',
      dataType: 'date',
      declaredType: null,
      catalogType: 'DATE',
      tags: [],
      meta: {},
      index: 2,
      isPrimaryKey: false,
    },
    {
      name: 'number_of_orders',
      description: 'Count of all orders placed by this customer.',
      dataType: 'bigint',
      declaredType: 'bigint',
      catalogType: 'BIGINT',
      tags: [],
      meta: {},
      index: 3,
      isPrimaryKey: false,
    },
    {
      // Undocumented on purpose: every columns view has a "no description" branch,
      // and a story showing only documented columns never exercises it.
      name: 'lifetime_value',
      description: null,
      dataType: 'double',
      declaredType: null,
      catalogType: 'DOUBLE',
      tags: [],
      meta: {},
      index: 4,
      isPrimaryKey: false,
    },
  ];
}

/** A fully-populated model — documented, contracted, with catalog stats. */
export function storyModel(overrides: Partial<ModelAsset> = {}): ModelAsset {
  return makeFakeModelAsset({
    uniqueId: 'model.jaffle_shop.customers',
    name: 'customers',
    description:
      'One row per customer, with first and most recent order dates and a lifetime ' +
      'order count. The canonical customer dimension.',
    packageName: 'jaffle_shop',
    tags: ['daily', 'marts'],
    rawCode:
      "with customers as (\n    select * from {{ ref('stg_customers') }}\n)\n\n" +
      'select\n    customer_id,\n    first_order_date,\n    number_of_orders\n' +
      'from customers',
    compiledCode:
      'with customers as (\n    select * from "analytics"."dbt"."stg_customers"\n)\n\n' +
      'select\n    customer_id,\n    first_order_date,\n    number_of_orders\n' +
      'from customers',
    language: 'sql',
    access: 'public',
    contractEnforced: true,
    materializedType: 'table',
    group: 'finance',
    relation: { database: 'analytics', schema: 'dbt', identifier: 'customers' },
    columns: storyColumns(),
    filePath: 'models/marts/customers.sql',
    originalFilePath: 'models/marts/customers.sql',
    patchPath: 'models/marts/_marts.yml',
    fqn: ['jaffle_shop', 'marts', 'customers'],
    config: {
      materialized: 'table',
      tags: ['daily', 'marts'],
      schema: 'dbt',
      unique_key: 'customer_id',
    },
    meta: { owner: 'data-platform', maturity: 'high' },
    dependsOn: ['model.jaffle_shop.stg_customers', 'model.jaffle_shop.stg_orders'],
    referencedBy: ['exposure.jaffle_shop.weekly_metrics', 'metric.jaffle_shop.revenue'],
    rowCountStat: 128_450,
    bytesStat: 4_194_304,
    primaryKey: ['customer_id'],
    owner: 'data-platform',
    ...overrides,
  });
}

/** A source with freshness configured and passing. */
export function storySource(overrides: Partial<SourceAsset> = {}): SourceAsset {
  return {
    uniqueId: 'source.jaffle_shop.raw.customers',
    name: 'customers',
    resourceType: 'source',
    description: 'Raw customer records as landed by the loader.',
    packageName: 'jaffle_shop',
    tags: ['raw'],
    sourceName: 'raw',
    identifier: 'customers',
    loader: 'fivetran',
    freshness: {
      warnAfter: { count: 12, period: 'hour' },
      errorAfter: { count: 24, period: 'hour' },
      filter: null,
    },
    relation: { database: 'raw', schema: 'jaffle_shop', identifier: 'customers' },
    columns: storyColumns().slice(0, 2),
    freshnessStatus: 'pass',
    freshnessMaxLoadedAt: '2026-02-11T16:05:00Z',
    filePath: 'models/staging/_sources.yml',
    ...overrides,
  };
}

export function storyExposure(overrides: Partial<ExposureAsset> = {}): ExposureAsset {
  return {
    uniqueId: 'exposure.jaffle_shop.weekly_metrics',
    name: 'weekly_metrics',
    resourceType: 'exposure',
    description: 'Executive weekly metrics dashboard.',
    packageName: 'jaffle_shop',
    tags: ['exec'],
    exposureType: 'dashboard',
    maturity: 'high',
    ownerName: 'Data Platform',
    ownerEmail: 'data-platform@example.com',
    url: 'https://bi.example.com/dashboards/weekly-metrics',
    dependsOn: ['model.jaffle_shop.customers', 'model.jaffle_shop.orders'],
    ...overrides,
  };
}

export function storyMetric(overrides: Partial<MetricAsset> = {}): MetricAsset {
  return {
    uniqueId: 'metric.jaffle_shop.revenue',
    name: 'revenue',
    resourceType: 'metric',
    description: 'Total revenue from completed orders.',
    packageName: 'jaffle_shop',
    tags: [],
    label: 'Revenue',
    typeParams: {
      kind: 'simple',
      measure: { name: 'order_total', filter: "status = 'completed'" },
    },
    group: 'finance',
    ...overrides,
  };
}

export function storyMacro(overrides: Partial<MacroAsset> = {}): MacroAsset {
  return {
    uniqueId: 'macro.jaffle_shop.cents_to_dollars',
    name: 'cents_to_dollars',
    resourceType: 'macro',
    description: 'Convert an integer cents column to a dollars decimal.',
    packageName: 'jaffle_shop',
    tags: [],
    macroSql:
      '{% macro cents_to_dollars(column_name, scale=2) %}\n' +
      '    ({{ column_name }} / 100)::numeric(16, {{ scale }})\n' +
      '{% endmacro %}',
    arguments: [
      { name: 'column_name', type: 'string', description: 'Column holding cents.' },
      { name: 'scale', type: 'integer', description: 'Decimal places to keep.' },
    ],
    path: 'macros/cents_to_dollars.sql',
    ...overrides,
  };
}

export function storySemanticModel(
  overrides: Partial<SemanticModelAsset> = {},
): SemanticModelAsset {
  return {
    uniqueId: 'semantic_model.jaffle_shop.orders',
    name: 'orders',
    resourceType: 'semantic_model',
    description: 'Order-grain semantic model backing the revenue metrics.',
    packageName: 'jaffle_shop',
    tags: [],
    modelUniqueId: 'model.jaffle_shop.orders',
    dimensions: [
      { name: 'ordered_at', type: 'time', description: 'Order placement timestamp.' },
      { name: 'status', type: 'categorical', description: 'Order fulfilment status.' },
    ],
    measures: [
      { name: 'order_total', agg: 'sum', expr: 'amount', description: 'Order amount.' },
      { name: 'order_count', agg: 'count', expr: null, description: 'Orders placed.' },
    ],
    entities: [
      { name: 'order', type: 'primary', expr: 'order_id' },
      { name: 'customer', type: 'foreign', expr: 'customer_id' },
    ],
    ...overrides,
  };
}

export function storyTest(overrides: Partial<TestAsset> = {}): TestAsset {
  return {
    uniqueId: 'test.jaffle_shop.unique_customers_customer_id',
    name: 'unique_customers_customer_id',
    resourceType: 'test',
    description: 'Asserts the customer surrogate key is unique.',
    packageName: 'jaffle_shop',
    tags: [],
    testType: 'generic',
    severity: 'error',
    columnName: 'customer_id',
    dependsOn: ['model.jaffle_shop.customers'],
    rawCode: null,
    compiledCode:
      'select customer_id from "analytics"."dbt"."customers"\n' +
      'group by customer_id having count(*) > 1',
    ...overrides,
  };
}

export function storySavedQuery(
  overrides: Partial<SavedQueryAsset> = {},
): SavedQueryAsset {
  return {
    uniqueId: 'saved_query.jaffle_shop.weekly_revenue',
    name: 'weekly_revenue',
    resourceType: 'saved_query',
    description: 'Revenue by week, exported for the BI layer.',
    packageName: 'jaffle_shop',
    tags: [],
    label: 'Weekly revenue',
    queryParams: {
      metrics: ['revenue', 'order_count'],
      groupBy: ['metric_time__week', 'customer__region'],
      where: ["{{ Dimension('order__status') }} = 'completed'"],
      orderBy: ['metric_time__week'],
      limit: 1000,
    },
    exports: [
      { name: 'weekly_revenue', exportAs: 'table', schema: 'reporting' },
      { name: 'weekly_revenue_vw', exportAs: 'view', schema: 'reporting' },
    ],
    ...overrides,
  };
}

export function storyGroup(overrides: Partial<GroupAsset> = {}): GroupAsset {
  return {
    uniqueId: 'group.jaffle_shop.finance',
    name: 'finance',
    resourceType: 'group',
    description: 'Models owned by the finance analytics team.',
    packageName: 'jaffle_shop',
    tags: [],
    ownerName: 'Finance Analytics',
    ownerEmail: 'finance-analytics@example.com',
    ownerGithub: 'jaffle-finance',
    ownerSlack: '#finance-analytics',
    models: [
      {
        uniqueId: 'model.jaffle_shop.customers',
        name: 'customers',
        database: 'analytics',
        schema: 'dbt',
      },
      {
        uniqueId: 'model.jaffle_shop.orders',
        name: 'orders',
        database: 'analytics',
        schema: 'dbt',
      },
    ],
    ...overrides,
  };
}

/** A small but branching lineage graph: two sources fan into two staging models,
 *  which fan into a mart, which a metric and an exposure consume. */
export function storyLineage(): LineageGraph {
  return {
    nodes: [
      {
        uniqueId: 'source.jaffle_shop.raw.customers',
        name: 'customers',
        resourceType: 'source',
        description: null,
        packageName: 'jaffle_shop',
        tags: [],
      },
      {
        uniqueId: 'source.jaffle_shop.raw.orders',
        name: 'orders',
        resourceType: 'source',
        description: null,
        packageName: 'jaffle_shop',
        tags: [],
      },
      {
        uniqueId: 'model.jaffle_shop.stg_customers',
        name: 'stg_customers',
        resourceType: 'model',
        description: null,
        packageName: 'jaffle_shop',
        tags: [],
        materialized: 'view',
      },
      {
        uniqueId: 'model.jaffle_shop.stg_orders',
        name: 'stg_orders',
        resourceType: 'model',
        description: null,
        packageName: 'jaffle_shop',
        tags: [],
        materialized: 'view',
      },
      {
        uniqueId: 'model.jaffle_shop.customers',
        name: 'customers',
        resourceType: 'model',
        description: null,
        packageName: 'jaffle_shop',
        tags: [],
        materialized: 'table',
      },
      {
        uniqueId: 'metric.jaffle_shop.revenue',
        name: 'revenue',
        resourceType: 'metric',
        description: null,
        packageName: 'jaffle_shop',
        tags: [],
      },
      {
        uniqueId: 'exposure.jaffle_shop.weekly_metrics',
        name: 'weekly_metrics',
        resourceType: 'exposure',
        description: null,
        packageName: 'jaffle_shop',
        tags: [],
      },
    ],
    edges: [
      {
        upstreamUniqueId: 'source.jaffle_shop.raw.customers',
        downstreamUniqueId: 'model.jaffle_shop.stg_customers',
      },
      {
        upstreamUniqueId: 'source.jaffle_shop.raw.orders',
        downstreamUniqueId: 'model.jaffle_shop.stg_orders',
      },
      {
        upstreamUniqueId: 'model.jaffle_shop.stg_customers',
        downstreamUniqueId: 'model.jaffle_shop.customers',
      },
      {
        upstreamUniqueId: 'model.jaffle_shop.stg_orders',
        downstreamUniqueId: 'model.jaffle_shop.customers',
      },
      {
        upstreamUniqueId: 'model.jaffle_shop.customers',
        downstreamUniqueId: 'metric.jaffle_shop.revenue',
      },
      {
        upstreamUniqueId: 'model.jaffle_shop.customers',
        downstreamUniqueId: 'exposure.jaffle_shop.weekly_metrics',
      },
    ],
  };
}

/** Column lineage over the same graph, including one error node — the degraded case a
 *  non-strict export produces. */
export function storyColumnLineage(): ColumnLineageGraph {
  return {
    nodes: [
      {
        uniqueId: 'model.jaffle_shop.stg_customers.customer_id',
        nodeUniqueId: 'model.jaffle_shop.stg_customers',
        name: 'customer_id',
        parentColumns: [],
        transformationType: 'passthrough',
        isError: false,
        errorCategory: null,
      },
      {
        uniqueId: 'model.jaffle_shop.customers.customer_id',
        nodeUniqueId: 'model.jaffle_shop.customers',
        name: 'customer_id',
        parentColumns: ['model.jaffle_shop.stg_customers.customer_id'],
        transformationType: 'passthrough',
        isError: false,
        errorCategory: null,
      },
      {
        uniqueId: 'model.jaffle_shop.customers.lifetime_value',
        nodeUniqueId: 'model.jaffle_shop.customers',
        name: 'lifetime_value',
        parentColumns: [],
        transformationType: null,
        isError: true,
        errorCategory: 'unresolved_expression',
      },
    ],
    edges: [
      {
        fromNodeUniqueId: 'model.jaffle_shop.stg_customers',
        fromColumn: 'customer_id',
        toNodeUniqueId: 'model.jaffle_shop.customers',
        toColumn: 'customer_id',
        transformationType: 'passthrough',
      },
    ],
  };
}

/** Search hits spanning several resource types and matched fields. */
export function storySearchHits(): SearchHit[] {
  return [
    {
      uniqueId: 'model.jaffle_shop.customers',
      resourceType: 'model',
      name: 'customers',
      packageName: 'jaffle_shop',
      fqn: ['jaffle_shop', 'marts', 'customers'],
      matchedField: 'name',
      highlight: '<b>customer</b>s',
      materialized: 'table',
      access: 'public',
      executedAt: '2026-02-11T16:12:00Z',
    },
    {
      uniqueId: 'model.jaffle_shop.stg_customers',
      resourceType: 'model',
      name: 'stg_customers',
      packageName: 'jaffle_shop',
      fqn: ['jaffle_shop', 'staging', 'stg_customers'],
      matchedField: 'fqn',
      highlight: 'staging.stg_<b>customer</b>s',
      materialized: 'view',
      access: 'protected',
      executedAt: '2026-02-11T16:11:00Z',
    },
    {
      uniqueId: 'source.jaffle_shop.raw.customers',
      resourceType: 'source',
      name: 'customers',
      packageName: 'jaffle_shop',
      matchedField: 'column',
      highlight: '<b>customer</b>_id',
      sourceName: 'raw',
      freshnessChecked: true,
    },
    {
      uniqueId: 'test.jaffle_shop.unique_customers_customer_id',
      resourceType: 'test',
      name: 'unique_customers_customer_id',
      packageName: 'jaffle_shop',
      matchedField: 'name',
      highlight: 'unique_<b>customer</b>s_<b>customer</b>_id',
      testType: 'generic',
      executedAt: '2026-02-11T16:12:30Z',
    },
    {
      uniqueId: 'exposure.jaffle_shop.weekly_metrics',
      resourceType: 'exposure',
      name: 'weekly_metrics',
      packageName: 'jaffle_shop',
      matchedField: 'description',
      // No highlight: the backend does not always return one, and the result item has
      // to fall back to the plain name.
      highlight: null,
      exposureType: 'dashboard',
    },
  ];
}

/** Per-type tallies adding up to a plausible mid-size project. */
export function storyCounts(overrides: AssetCounts = {}): AssetCounts {
  return {
    model: 142,
    source: 38,
    seed: 6,
    snapshot: 4,
    test: 311,
    unit_test: 12,
    exposure: 9,
    metric: 21,
    semantic_model: 7,
    saved_query: 5,
    macro: 34,
    group: 3,
    ...overrides,
  };
}

/** Facet options for the list-filter dropdowns. */
export function storyFacets(overrides: Facets = {}): Facets {
  return {
    packages: [
      { value: 'jaffle_shop', count: 142 },
      { value: 'dbt_utils', count: 18 },
      { value: 'audit_helper', count: 4 },
    ],
    tags: [
      { value: 'daily', count: 61 },
      { value: 'marts', count: 24 },
      { value: 'pii', count: 9 },
    ],
    owners: [
      { value: 'data-platform', count: 74 },
      { value: 'finance-analytics', count: 38 },
    ],
    modelingLayers: [
      { value: 'staging', count: 80 },
      { value: 'intermediate', count: 32 },
      { value: 'marts', count: 30 },
    ],
    materializations: [
      { value: 'view', count: 88 },
      { value: 'table', count: 46 },
      { value: 'incremental', count: 8 },
    ],
    results: [
      { value: 'pass', count: 288 },
      { value: 'fail', count: 12 },
      { value: 'warn', count: 7 },
      { value: 'skipped', count: 4 },
    ],
    ...overrides,
  };
}

/** Every capability on. Pair with the all-false default in `createFakeDataSource` to
 *  show both sides of a gate. */
export function storyCapabilities(overrides: Partial<Capabilities> = {}): Capabilities {
  return {
    hasColumnLineage: true,
    hasQueryHistory: true,
    hasCostInsights: true,
    hasPerformance: true,
    hasRecommendations: true,
    hasHealthSignals: true,
    hasAutoExposures: true,
    hasMultiProject: true,
    hasMesh: true,
    hasRunResults: true,
    hasCatalogStats: true,
    hasDbtState: true,
    ...overrides,
  };
}

/** A successful most-recent run. */
export function storyExecutionInfo(
  overrides: Partial<ExecutionInfo> = {},
): ExecutionInfo {
  return {
    job: {
      jobId: '4821',
      status: 'success',
      startedAt: '2026-02-11T16:10:00Z',
      completedAt: '2026-02-11T16:14:00Z',
    },
    node: {
      state: 'ran',
      status: 'success',
      startedAt: '2026-02-11T16:12:00Z',
      completedAt: '2026-02-11T16:12:18Z',
    },
    ...overrides,
  };
}

export function storyOverview(
  overrides: Partial<ProjectOverview> = {},
): ProjectOverview {
  return {
    uniqueId: 'doc.jaffle_shop.__overview__',
    packageName: 'jaffle_shop',
    blockContents: [
      '# Jaffle Shop',
      '',
      'The canonical example project. This overview block is authored in',
      '`models/overview.md` and rendered verbatim.',
      '',
      '## Where to start',
      '',
      '- `customers` — one row per customer',
      '- `orders` — one row per order',
      '',
      'See the [dbt docs](https://docs.getdbt.com) for more.',
    ].join('\n'),
    ...overrides,
  };
}

/** File entries spanning several packages and directories — enough nesting to
 *  exercise the tree builder. */
export function storyFiles(): FileEntry[] {
  return [
    {
      uniqueId: 'model.jaffle_shop.customers',
      name: 'customers',
      resourceType: 'model',
      packageName: 'jaffle_shop',
      originalFilePath: 'models/marts/customers.sql',
      patchPath: 'models/marts/_marts.yml',
    },
    {
      uniqueId: 'model.jaffle_shop.orders',
      name: 'orders',
      resourceType: 'model',
      packageName: 'jaffle_shop',
      originalFilePath: 'models/marts/orders.sql',
      patchPath: 'models/marts/_marts.yml',
    },
    {
      uniqueId: 'model.jaffle_shop.stg_customers',
      name: 'stg_customers',
      resourceType: 'model',
      packageName: 'jaffle_shop',
      originalFilePath: 'models/staging/stg_customers.sql',
      patchPath: null,
    },
    {
      uniqueId: 'source.jaffle_shop.raw.customers',
      name: 'customers',
      resourceType: 'source',
      packageName: 'jaffle_shop',
      originalFilePath: 'models/staging/_sources.yml',
      patchPath: null,
    },
    {
      uniqueId: 'macro.jaffle_shop.cents_to_dollars',
      name: 'cents_to_dollars',
      resourceType: 'macro',
      packageName: 'jaffle_shop',
      originalFilePath: 'macros/cents_to_dollars.sql',
      patchPath: null,
    },
    {
      uniqueId: 'macro.dbt_utils.star',
      name: 'star',
      resourceType: 'macro',
      packageName: 'dbt_utils',
      originalFilePath: 'macros/sql/star.sql',
      patchPath: null,
    },
  ];
}

/** The nine-column resource slice the shell reads at first paint. */
export function storyNodes(): NodeSummary[] {
  return [
    {
      unique_id: 'model.jaffle_shop.customers',
      name: 'customers',
      resource_type: 'model',
      package_name: 'jaffle_shop',
      materialized: 'table',
      description: 'One row per customer.',
      database_name: 'analytics',
      schema_name: 'dbt',
      original_file_path: 'models/marts/customers.sql',
    },
    {
      unique_id: 'model.jaffle_shop.orders',
      name: 'orders',
      resource_type: 'model',
      package_name: 'jaffle_shop',
      materialized: 'table',
      description: 'One row per order.',
      database_name: 'analytics',
      schema_name: 'dbt',
      original_file_path: 'models/marts/orders.sql',
    },
    {
      unique_id: 'model.jaffle_shop.stg_customers',
      name: 'stg_customers',
      resource_type: 'model',
      package_name: 'jaffle_shop',
      materialized: 'view',
      description: null,
      database_name: 'analytics',
      schema_name: 'dbt',
      original_file_path: 'models/staging/stg_customers.sql',
    },
    {
      unique_id: 'source.jaffle_shop.raw.customers',
      name: 'customers',
      resource_type: 'source',
      package_name: 'jaffle_shop',
      materialized: null,
      description: 'Raw customer records.',
      database_name: 'raw',
      schema_name: 'jaffle_shop',
      original_file_path: 'models/staging/_sources.yml',
    },
    {
      unique_id: 'test.jaffle_shop.unique_customers_customer_id',
      name: 'unique_customers_customer_id',
      resource_type: 'test',
      package_name: 'jaffle_shop',
      materialized: null,
      description: null,
      database_name: null,
      schema_name: null,
      original_file_path: 'models/marts/_marts.yml',
    },
    {
      unique_id: 'exposure.jaffle_shop.weekly_metrics',
      name: 'weekly_metrics',
      resource_type: 'exposure',
      package_name: 'jaffle_shop',
      materialized: null,
      description: 'Executive weekly metrics dashboard.',
      database_name: null,
      schema_name: null,
      original_file_path: 'models/marts/_exposures.yml',
    },
    {
      unique_id: 'macro.jaffle_shop.cents_to_dollars',
      name: 'cents_to_dollars',
      resource_type: 'macro',
      package_name: 'jaffle_shop',
      materialized: null,
      description: null,
      database_name: null,
      schema_name: null,
      original_file_path: 'macros/cents_to_dollars.sql',
    },
  ];
}

/** The resolved first-paint read, for stories of components that call `useAllNodes`.
 *  The context default is a promise that never settles, so a story without this sits
 *  in its loading state forever. */
export function storyBootstrapData(
  overrides: Partial<BootstrapData> = {},
): BootstrapData {
  return {
    nodes: storyNodes(),
    project: {
      name: 'jaffle_shop',
      description: 'The canonical example project.',
      dbtVersion: '1.10.2',
      adapterType: 'snowflake',
    },
    generation: '2026-02-11T16:20:00Z',
    ...overrides,
  } as BootstrapData;
}
