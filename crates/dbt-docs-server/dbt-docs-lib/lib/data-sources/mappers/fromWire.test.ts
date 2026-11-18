import { describe, expect, test } from 'vitest';

import {
  fromCapabilities,
  fromColumnLineageResponse,
  fromDistribution,
  fromExposureSummary,
  fromFileList,
  fromGroupSummary,
  fromLineageResponse,
  fromMacroSummary,
  fromMetricSummary,
  fromModelDetail,
  fromModelSummary,
  fromNodeCounts,
  fromProject,
  fromSavedQueryDetail,
  fromSavedQuerySummary,
  fromSearchFacets,
  fromSearchResponse,
  fromSeedSummary,
  fromSemanticModelSummary,
  fromSnapshotSummary,
  fromSourceSummary,
  fromTestDetail,
  fromTestSummary,
  type RestColumnLineageResponse,
  type RestLineageResponse,
  type RestTestDetail,
} from './fromWire';

describe('fromLineageResponse', () => {
  test('maps materialized onto summary nodes', () => {
    const r: RestLineageResponse = {
      root: 'model.shop.customers',
      max_depth: 1,
      nodes: [
        {
          unique_id: 'model.shop.customers',
          name: 'customers',
          resource_type: 'model',
          materialized: 'incremental',
          depth: 0,
        },
        {
          unique_id: 'source.shop.raw.orders',
          name: 'orders',
          resource_type: 'source',
          depth: -1,
        },
      ],
      edges: [
        {
          from_id: 'source.shop.raw.orders',
          to_id: 'model.shop.customers',
          edge_type: 'parent',
        },
      ],
    };

    const g = fromLineageResponse(r);
    expect(g.nodes[0].materialized).toBe('incremental');
    // Absent materialized → null.
    expect(g.nodes[1].materialized).toBeNull();
    expect(g.edges).toEqual([
      {
        upstreamUniqueId: 'source.shop.raw.orders',
        downstreamUniqueId: 'model.shop.customers',
      },
    ]);
  });
});

describe('fromCapabilities', () => {
  test('populates hasDbtState from has_dbt_state', () => {
    expect(
      fromCapabilities({ has_column_lineage: true, has_dbt_state: true }),
    ).toMatchObject({ hasColumnLineage: true, hasDbtState: true });
  });

  test('hasDbtState false when has_dbt_state absent', () => {
    expect(fromCapabilities({ has_column_lineage: false }).hasDbtState).toBe(false);
  });

  test('hasRunResults stays false even when has_dbt_state is true', () => {
    // REST exposes no run-results signal; hasDbtState and hasRunResults are
    // distinct capabilities.
    expect(
      fromCapabilities({ has_column_lineage: true, has_dbt_state: true }).hasRunResults,
    ).toBe(false);
  });
});

describe('fromColumnLineageResponse', () => {
  test('maps edges (camelCase) and groups nodes by destination column', () => {
    const r: RestColumnLineageResponse = {
      root: 'model.shop.customers',
      edges: [
        {
          from_node: 'source.shop.raw.orders',
          from_column: 'id',
          to_node: 'model.shop.customers',
          to_column: 'customer_id',
          kind: 'rename',
        },
      ],
    };

    const g = fromColumnLineageResponse(r);
    expect(g.edges).toEqual([
      {
        fromNodeUniqueId: 'source.shop.raw.orders',
        fromColumn: 'id',
        toNodeUniqueId: 'model.shop.customers',
        toColumn: 'customer_id',
        transformationType: 'rename',
      },
    ]);
    expect(g.nodes).toHaveLength(1);
    expect(g.nodes[0]).toMatchObject({
      nodeUniqueId: 'model.shop.customers',
      name: 'customer_id',
      parentColumns: ['source.shop.raw.orders:id'],
      transformationType: 'rename',
    });
  });
});

describe('fromDistribution', () => {
  test('maps name → isProprietary and is_logged_in → isLoggedIn', () => {
    expect(
      fromDistribution({ name: 'dbt', version: '2.0.0', is_logged_in: true }),
    ).toEqual({
      isProprietary: true,
      isLoggedIn: true,
      version: '2.0.0',
    });
  });

  test('name "oss" → isProprietary false', () => {
    expect(fromDistribution({ name: 'oss', is_logged_in: false }).isProprietary).toBe(
      false,
    );
  });
});

describe('fromProject', () => {
  test('maps snake_case fields to camelCase', () => {
    expect(
      fromProject({
        name: 'jaffle_shop',
        project_id: 'p1',
        description: 'shop',
        dbt_version: '1.8.0',
        adapter_type: 'duckdb',
        git_sha: 'abc',
        git_branch: 'main',
        git_is_dirty: true,
      }),
    ).toEqual({
      name: 'jaffle_shop',
      projectId: 'p1',
      description: 'shop',
      dbtVersion: '1.8.0',
      adapterType: 'duckdb',
      gitSha: 'abc',
      gitBranch: 'main',
      gitIsDirty: true,
    });
  });

  test('carries name through when optional fields are absent', () => {
    expect(fromProject({ name: 'solo' }).name).toBe('solo');
  });
});

describe('fromFileList', () => {
  test('maps each entry to camelCase', () => {
    expect(
      fromFileList({
        total: 1,
        files: [
          {
            unique_id: 'model.pkg.x',
            name: 'x',
            resource_type: 'model',
            package_name: 'pkg',
            original_file_path: 'models/x.sql',
            patch_path: null,
          },
        ],
      }),
    ).toEqual([
      {
        uniqueId: 'model.pkg.x',
        name: 'x',
        resourceType: 'model',
        packageName: 'pkg',
        originalFilePath: 'models/x.sql',
        patchPath: null,
      },
    ]);
  });

  test('null body (404) → empty list', () => {
    expect(fromFileList(null)).toEqual([]);
  });
});

describe('fromModelDetail column dataType precedence', () => {
  const model = (col: Record<string, string | null>) =>
    fromModelDetail({
      unique_id: 'model.shop.customers',
      name: 'customers',
      resource_type: 'model',
      columns: [{ name: 'id', ...col }],
    }).columns[0].dataType;

  test('prefers data_type over all others', () => {
    expect(
      model({
        data_type: 'data',
        declared_type: 'declared',
        inferred_type: 'inferred',
        catalog_type: 'catalog',
      }),
    ).toBe('data');
  });

  test('falls back to declared_type when data_type absent', () => {
    expect(
      model({
        data_type: null,
        declared_type: 'declared',
        inferred_type: 'inferred',
        catalog_type: 'catalog',
      }),
    ).toBe('declared');
  });

  test('falls back to inferred_type when data_type + declared_type absent', () => {
    expect(
      model({
        data_type: null,
        declared_type: null,
        inferred_type: 'inferred',
        catalog_type: 'catalog',
      }),
    ).toBe('inferred');
  });

  test('falls back to catalog_type last', () => {
    expect(
      model({
        data_type: null,
        declared_type: null,
        inferred_type: null,
        catalog_type: 'catalog',
      }),
    ).toBe('catalog');
  });

  test('null when no type source present', () => {
    expect(model({})).toBeNull();
  });
});

describe('fromModelSummary', () => {
  test('maps the happy path', () => {
    const s = fromModelSummary({
      unique_id: 'model.shop.customers',
      name: 'customers',
      package_name: 'shop',
      original_file_path: 'models/customers.sql',
      modeling_layer: 'marts',
      owner: 'data-team',
      executed_at: '2026-01-01T00:00:00Z',
      catalog: { row_count_stat: 42 },
    });
    expect(s).toMatchObject({
      uniqueId: 'model.shop.customers',
      name: 'customers',
      resourceType: 'model',
      packageName: 'shop',
      modelingLayer: 'marts',
      owner: 'data-team',
      executedAt: '2026-01-01T00:00:00Z',
      rowCountStat: 42,
    });
  });

  test('nulls optional fields when catalog/owner/layer absent', () => {
    const s = fromModelSummary({ unique_id: 'model.shop.x', name: 'x' });
    expect(s.packageName).toBe('');
    expect(s.modelingLayer).toBeNull();
    expect(s.owner).toBeNull();
    expect(s.executedAt).toBeNull();
    expect(s.rowCountStat).toBeNull();
  });
});

describe('fromSourceSummary', () => {
  test('maps source/database/schema names', () => {
    const s = fromSourceSummary({
      unique_id: 'source.shop.raw.orders',
      name: 'orders',
      package_name: 'shop',
      source_name: 'raw',
      source_description: 'Raw orders',
      database_name: 'analytics',
      schema_name: 'raw',
      tags: ['pii'],
    });
    expect(s).toMatchObject({
      resourceType: 'source',
      description: 'Raw orders',
      sourceName: 'raw',
      databaseName: 'analytics',
      schemaName: 'raw',
      tags: ['pii'],
    });
  });

  test('nulls names and empties tags when absent', () => {
    const s = fromSourceSummary({ unique_id: 'source.s.r.o', name: 'o' });
    expect(s.sourceName).toBeNull();
    expect(s.databaseName).toBeNull();
    expect(s.schemaName).toBeNull();
    expect(s.description).toBeNull();
    expect(s.tags).toEqual([]);
  });
});

describe('fromSeedSummary', () => {
  test('maps row_count and executed_at', () => {
    const s = fromSeedSummary({
      unique_id: 'seed.shop.countries',
      name: 'countries',
      row_count: 195,
      executed_at: '2026-01-02T00:00:00Z',
    });
    expect(s).toMatchObject({
      resourceType: 'seed',
      rowCount: 195,
      executedAt: '2026-01-02T00:00:00Z',
    });
  });

  test('nulls row_count/executed_at when absent', () => {
    const s = fromSeedSummary({ unique_id: 'seed.s.x', name: 'x' });
    expect(s.rowCount).toBeNull();
    expect(s.executedAt).toBeNull();
  });
});

describe('fromSnapshotSummary', () => {
  test('maps catalog stats', () => {
    const s = fromSnapshotSummary({
      unique_id: 'snapshot.shop.orders_snap',
      name: 'orders_snap',
      materialized: 'snapshot',
      catalog: {
        row_count_stat: 100,
        bytes_stat: 2048,
        last_modified_stat: '2026-01-03T00:00:00Z',
      },
    });
    expect(s).toMatchObject({
      resourceType: 'snapshot',
      rowCountStat: 100,
      bytesStat: 2048,
      lastModifiedStat: '2026-01-03T00:00:00Z',
    });
  });

  test('nulls stats when catalog absent', () => {
    const s = fromSnapshotSummary({ unique_id: 'snapshot.s.x', name: 'x' });
    expect(s.rowCountStat).toBeNull();
    expect(s.bytesStat).toBeNull();
    expect(s.lastModifiedStat).toBeNull();
  });
});

describe('fromTestSummary', () => {
  test('maps a data test', () => {
    const s = fromTestSummary({
      unique_id: 'test.shop.not_null_customers_id',
      name: 'not_null_customers_id',
      resource_type: 'test',
      tested_node_unique_id: 'model.shop.customers',
      tested_column: 'id',
      execution_info: { status: 'pass' },
    });
    expect(s).toMatchObject({
      resourceType: 'test',
      testType: 'data',
      status: 'pass',
      testedNodeUniqueId: 'model.shop.customers',
      testedColumn: 'id',
    });
  });

  test('discriminates unit_test → testType "unit"', () => {
    const s = fromTestSummary({
      unique_id: 'unit_test.shop.test_customers',
      name: 'test_customers',
      resource_type: 'unit_test',
    });
    expect(s.resourceType).toBe('unit_test');
    expect(s.testType).toBe('unit');
    expect(s.status).toBeNull();
  });

  test('nulls status when execution_info absent', () => {
    const s = fromTestSummary({
      unique_id: 'test.s.x',
      name: 'x',
      resource_type: 'test',
    });
    expect(s.status).toBeNull();
    expect(s.testedColumn).toBeNull();
  });
});

describe('fromTestDetail', () => {
  test('maps a data test to resourceType "test" even with a stray `given: null` key', () => {
    // Regression: the duckdb UNION ALL backing test-detail rows selects a
    // `given` column on both branches (NULL for data tests), so `'given' in d`
    // always matched -- the discriminator must be `resource_type` instead.
    const d = fromTestDetail({
      unique_id: 'test.shop.not_null_customers_id',
      name: 'not_null_customers_id',
      resource_type: 'test',
      package_name: 'shop',
      tags: [],
      fqn: ['shop', 'not_null_customers_id'],
      depends_on: [],
      given: null,
      expect: null,
    } as unknown as RestTestDetail);
    expect(d.resourceType).toBe('test');
  });

  test('maps a real unit test to resourceType "unit_test"', () => {
    const d = fromTestDetail({
      unique_id: 'unit_test.shop.test_customers',
      name: 'test_customers',
      resource_type: 'unit_test',
      package_name: 'shop',
      tags: [],
      fqn: ['shop', 'test_customers'],
      depends_on: [],
      given: [],
      expect: { rows: [] },
    } as unknown as RestTestDetail);
    expect(d.resourceType).toBe('unit_test');
  });
});

describe('fromMetricSummary', () => {
  test('maps metric_type', () => {
    const s = fromMetricSummary({
      unique_id: 'metric.shop.revenue',
      name: 'revenue',
      metric_type: 'simple',
      description: 'Total revenue',
      tags: ['finance'],
    });
    expect(s).toMatchObject({
      resourceType: 'metric',
      metricType: 'simple',
      description: 'Total revenue',
      tags: ['finance'],
    });
  });

  test('nulls metric_type when absent', () => {
    const s = fromMetricSummary({ unique_id: 'metric.s.x', name: 'x' });
    expect(s.metricType).toBeNull();
    expect(s.tags).toEqual([]);
  });
});

describe('fromSemanticModelSummary', () => {
  test('maps entity names', () => {
    const s = fromSemanticModelSummary({
      unique_id: 'semantic_model.shop.customers',
      name: 'customers',
      entities: [
        { name: 'customer', type: 'primary' },
        { name: 'order', type: 'foreign' },
      ],
    });
    expect(s.resourceType).toBe('semantic_model');
    expect(s.entities).toEqual(['customer', 'order']);
  });

  test('empty entities when absent', () => {
    const s = fromSemanticModelSummary({ unique_id: 'semantic_model.s.x', name: 'x' });
    expect(s.entities).toEqual([]);
  });
});

describe('fromSavedQuerySummary', () => {
  test('maps name/description/tags from base', () => {
    const s = fromSavedQuerySummary({
      unique_id: 'saved_query.shop.weekly',
      name: 'weekly',
      description: 'Weekly rollup',
      tags: ['report'],
    });
    expect(s).toMatchObject({
      resourceType: 'saved_query',
      name: 'weekly',
      description: 'Weekly rollup',
      tags: ['report'],
    });
  });
});

describe('fromMacroSummary', () => {
  test('maps argument names only', () => {
    const s = fromMacroSummary({
      unique_id: 'macro.shop.cents_to_dollars',
      name: 'cents_to_dollars',
      arguments: [{ name: 'column_name', type: 'string' }, { name: 'scale' }],
    });
    expect(s.resourceType).toBe('macro');
    expect(s.arguments).toEqual(['column_name', 'scale']);
  });

  test('empty arguments when absent', () => {
    const s = fromMacroSummary({ unique_id: 'macro.s.x', name: 'x' });
    expect(s.arguments).toEqual([]);
  });
});

describe('fromGroupSummary', () => {
  test('maps owner fields and model_count', () => {
    const s = fromGroupSummary({
      unique_id: 'group.shop.finance',
      name: 'finance',
      owner_name: 'Jane',
      owner_email: 'jane@shop.com',
      owner_github: 'jane',
      owner_slack: '@jane',
      model_count: 7,
    });
    expect(s).toMatchObject({
      resourceType: 'group',
      ownerName: 'Jane',
      ownerEmail: 'jane@shop.com',
      ownerGithub: 'jane',
      ownerSlack: '@jane',
      modelCount: 7,
    });
  });

  test('nulls owner fields and model_count when absent', () => {
    const s = fromGroupSummary({ unique_id: 'group.s.x', name: 'x' });
    expect(s.ownerName).toBeNull();
    expect(s.ownerEmail).toBeNull();
    expect(s.modelCount).toBeNull();
  });
});

describe('fromExposureSummary', () => {
  test('maps exposure_type and owner', () => {
    const s = fromExposureSummary({
      unique_id: 'exposure.shop.dash',
      name: 'dash',
      exposure_type: 'dashboard',
      owner_name: 'Jane',
      owner_email: 'jane@shop.com',
      tags: ['bi'],
    });
    expect(s).toMatchObject({
      resourceType: 'exposure',
      exposureType: 'dashboard',
      ownerName: 'Jane',
      ownerEmail: 'jane@shop.com',
      tags: ['bi'],
    });
  });

  test('nulls type/owner when absent', () => {
    const s = fromExposureSummary({ unique_id: 'exposure.s.x', name: 'x' });
    expect(s.exposureType).toBeNull();
    expect(s.ownerName).toBeNull();
    expect(s.ownerEmail).toBeNull();
    expect(s.tags).toEqual([]);
  });
});

describe('fromNodeCounts', () => {
  test('keeps known resource-type keys', () => {
    expect(fromNodeCounts({ model: 12, source: 3, test: 40, macro: 7 })).toEqual({
      model: 12,
      source: 3,
      test: 40,
      macro: 7,
    });
  });

  test('drops keys outside the ResourceType union', () => {
    const out = fromNodeCounts({ model: 1, doc: 5, column: 9 });
    expect(out).toEqual({ model: 1 });
    expect('doc' in out).toBe(false);
    expect('column' in out).toBe(false);
  });

  test('empty input → empty counts', () => {
    expect(fromNodeCounts({})).toEqual({});
  });
});

describe('fromSearchResponse', () => {
  test('folds matched_field/highlight onto the hit and maps the page', () => {
    const page = fromSearchResponse({
      data: [
        {
          matched_field: 'name',
          highlight: '<b>cust</b>omers',
          hit: {
            unique_id: 'model.shop.customers',
            resource_type: 'model',
            name: 'customers',
            package_name: 'shop',
            fqn: ['shop', 'customers'],
            materialized: 'table',
            access_level: 'public',
            executed_at: '2026-01-01T00:00:00Z',
          },
        },
        {
          matched_field: null,
          highlight: null,
          hit: {
            unique_id: 'source.shop.raw.orders',
            resource_type: 'source',
            name: 'orders',
            package_name: 'shop',
            source_name: 'raw',
            freshness_checked: true,
          },
        },
      ],
      page_info: {
        total_count: 2,
        start_cursor: null,
        end_cursor: 'cursor-1',
        has_next_page: true,
      },
    });

    expect(page.totalCount).toBe(2);
    expect(page.nextCursor).toBe('cursor-1');
    expect(page.items[0]).toEqual({
      uniqueId: 'model.shop.customers',
      resourceType: 'model',
      name: 'customers',
      packageName: 'shop',
      fqn: ['shop', 'customers'],
      matchedField: 'name',
      highlight: '<b>cust</b>omers',
      materialized: 'table',
      access: 'public',
      sourceName: undefined,
      freshnessChecked: undefined,
      testType: undefined,
      exposureType: undefined,
      executedAt: '2026-01-01T00:00:00Z',
    });
    expect(page.items[1]).toMatchObject({
      uniqueId: 'source.shop.raw.orders',
      resourceType: 'source',
      sourceName: 'raw',
      freshnessChecked: true,
      matchedField: null,
      highlight: null,
    });
  });

  test('nextCursor null on the last page', () => {
    const page = fromSearchResponse({
      data: [],
      page_info: {
        total_count: 0,
        start_cursor: null,
        end_cursor: 'ignored',
        has_next_page: false,
      },
    });
    expect(page.nextCursor).toBeNull();
    expect(page.items).toEqual([]);
  });
});

describe('fromSearchFacets', () => {
  test('maps snake_case facet lists to camelCase', () => {
    const f = fromSearchFacets({
      accesses: [{ value: 'public', count: 3 }],
      modeling_layers: [{ value: 'Marts', count: 5 }],
      materialization_types: [{ value: 'table', count: 8 }],
      tags: [{ value: 'pii', count: 2 }],
      packages: [{ value: 'shop', count: 12 }],
    });
    expect(f).toEqual({
      accesses: [{ value: 'public', count: 3 }],
      modelingLayers: [{ value: 'Marts', count: 5 }],
      materializationTypes: [{ value: 'table', count: 8 }],
      tags: [{ value: 'pii', count: 2 }],
      packages: [{ value: 'shop', count: 12 }],
    });
  });
});

describe('fromSavedQueryDetail', () => {
  test('flattens query_params.where into where_sql_template strings', () => {
    const a = fromSavedQueryDetail({
      unique_id: 'saved_query.shop.sq',
      name: 'sq',
      package_name: 'shop',
      fqn: ['shop', 'sq'],
      tags: [],
      query_params: {
        metrics: ['arr'],
        group_by: ["TimeDimension('metric_time', 'month')"],
        // Server shape: object with where_filters, not a string list.
        where: {
          where_filters: [
            { where_sql_template: "{{ Dimension('x') }} is not null" },
            { where_sql_template: null },
          ],
        },
        order_by: [],
        limit: null,
      },
      depends_on: [],
    } as never);

    expect(a.queryParams.where).toEqual(["{{ Dimension('x') }} is not null"]);
    expect(a.queryParams.metrics).toEqual(['arr']);
  });

  test('defaults where to [] when query_params is absent', () => {
    const a = fromSavedQueryDetail({
      unique_id: 'saved_query.shop.sq',
      name: 'sq',
      package_name: 'shop',
      fqn: ['shop', 'sq'],
      tags: [],
      depends_on: [],
    } as never);

    expect(a.queryParams.where).toEqual([]);
  });
});
