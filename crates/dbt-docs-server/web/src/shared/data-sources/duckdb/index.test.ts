import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  type SiteBootstrap,
  SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
} from '../../../lib/siteBootstrap';
import type { BootstrapData } from './bootstrap';
import type { DuckDbEngine, TableName } from './engine';
import { createDuckDbDataSource } from './index';

/**
 * Engine double recording what SQL ran and which artifacts it was asked for.
 *
 * The artifact list matters as much as the SQL: the whole point of registering
 * lazily is that a cold load must not pull column lineage or the code columns, and
 * that is only observable here.
 */
/**
 * `columnLineage` answers the presence probe every column-lineage path now makes.
 *
 * Gating asks the data whether `dbt.column_lineage` has rows, rather than whether
 * its file exists — the information schema writes every table even at zero rows,
 * so presence stopped being a signal. Answering it here keeps the probe out of
 * every unrelated test's `rowsBySql`.
 */
function fakeEngine(
  rowsBySql: (sql: string) => Record<string, unknown>[],
  { columnLineage = true }: { columnLineage?: boolean } = {},
): DuckDbEngine & {
  calls: { sql: string; tables: TableName[] }[];
} {
  const calls: { sql: string; tables: TableName[] }[] = [];
  const present = new Set<TableName>();
  return {
    calls,
    async query<T>(sql: string, tables: TableName[]): Promise<T[]> {
      calls.push({ sql, tables });
      tables.forEach((t) => present.add(t));
      if (sql.includes('SELECT EXISTS')) {
        return [{ present: columnLineage }] as T[];
      }
      return rowsBySql(sql) as T[];
    },
    hasTable: (t) => present.has(t),
    ready: async () => {},
  };
}

function siteBootstrap(overrides: Partial<SiteBootstrap> = {}): SiteBootstrap {
  return {
    schema_version: SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
    generated_at: '2026-08-08T18:00:00Z',
    dbt_version: '2.0.0-preview.208',
    distribution: 'dbt',
    is_logged_in: true,
    duckdb_cdn_base: 'https://cdn.example/duckdb',
    data_dir: 'index/',
    telemetry: {
      enabled: false,
      dbt_cloud_account_identifier: '',
      dbt_cloud_project_id: '',
      dbt_cloud_environment_id: '',
    },
    ...overrides,
  };
}

const BOOTSTRAP_DATA: BootstrapData = {
  nodes: [],
  project: { name: 'jaffle_shop', dbt_version: '2.0.0', adapter_type: 'duckdb' },
  generation: '2026-08-08T18:00:00Z',
};

function makeSource(
  engine: DuckDbEngine,
  data: BootstrapData = BOOTSTRAP_DATA,
  bootstrap = siteBootstrap(),
) {
  return createDuckDbDataSource({
    dataBaseUrl: 'https://host/site/data/',
    bootstrap,
    data: Promise.resolve(data),
    engine,
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('createDuckDbDataSource', () => {
  it('implements every fetcher the REST source did', () => {
    // Hooks gate on property presence, so a missing one silently disables its
    // surface. Nothing should be missing now.
    const source = makeSource(fakeEngine(() => []));
    for (const method of [
      'fetchAsset',
      'fetchAssetList',
      'fetchFacets',
      'fetchLineage',
      'fetchColumnLineage',
      'fetchCapabilities',
      'fetchDistribution',
      'fetchAssetCounts',
      'fetchProject',
      'fetchOverview',
      'fetchFiles',
      'fetchSearch',
      'fetchSearchFacets',
    ]) {
      expect(method in source).toBe(true);
    }
  });

  it('serves project from the bootstrap without touching the engine', async () => {
    // App blocks its first paint on this, so it must not wait on a 6.8 MB engine.
    const engine = fakeEngine(() => []);
    const project = await makeSource(engine).fetchProject?.();

    expect(project?.name).toBe('jaffle_shop');
    expect(engine.calls).toHaveLength(0);
  });

  it('serves distribution from the injected scalars with no I/O', async () => {
    const engine = fakeEngine(() => []);
    const dist = await makeSource(engine).fetchDistribution?.();

    // `name !== 'oss'` is what the UI reads as dbt v2.
    expect(dist).toMatchObject({ isProprietary: true, isLoggedIn: true });
    expect(engine.calls).toHaveLength(0);
  });

  it('derives capabilities from whether the relation has rows', async () => {
    // Not from whether the file exists, which is what this used to `HEAD`: the
    // information schema writes every table even at zero rows, so presence now
    // answers "yes" for every project and the feature would never read as gated.
    const engine = fakeEngine(() => []);

    const caps = await makeSource(engine).fetchCapabilities?.();

    expect(caps?.hasColumnLineage).toBe(true);
    expect(engine.calls[0]?.sql).toContain('SELECT EXISTS');
    expect(engine.calls[0]?.tables).toEqual(['dbt.column_lineage']);
  });

  it('reports column lineage unavailable when the relation has no rows', async () => {
    const caps = await makeSource(
      fakeEngine(() => [], { columnLineage: false }),
    ).fetchCapabilities?.();
    expect(caps?.hasColumnLineage).toBe(false);
  });

  it('asks the question once, however many surfaces ask it', async () => {
    // The probe fetches an artifact, so a page that renders capabilities and a
    // column-lineage graph must not pay for it twice.
    const engine = fakeEngine(() => []);
    const source = makeSource(engine);
    await source.fetchCapabilities?.();
    await source.fetchColumnLineage?.({ uniqueId: 'model.a.b' });
    expect(engine.calls.filter((c) => c.sql.includes('SELECT EXISTS'))).toHaveLength(1);
  });

  it('reports column lineage unavailable rather than failing when the relation is absent', async () => {
    // A partial or hand-copied site. The shell still has to render.
    const engine: DuckDbEngine = {
      query: async () => {
        throw new Error('Catalog Error: Table with name column_lineage does not exist');
      },
      hasTable: () => false,
      ready: async () => {},
    };
    const caps = await makeSource(engine).fetchCapabilities?.();
    expect(caps?.hasColumnLineage).toBe(false);
  });

  it('folds unit_test counts into test', async () => {
    // The two render on one page, so the UI counts them together — the Rust
    // handler did this after its query too.
    const engine = fakeEngine(() => [
      { resource_type: 'model', count: 3 },
      { resource_type: 'test', count: 4 },
      { resource_type: 'unit_test', count: 2 },
    ]);
    const counts = await makeSource(engine).fetchAssetCounts?.();

    expect(counts?.test).toBe(6);
    expect(counts?.model).toBe(3);
    expect(counts).not.toHaveProperty('unit_test');
  });

  it('coerces BigInt counts, which is how DuckDB returns them', async () => {
    const engine = fakeEngine(() => [
      { resource_type: 'model', count: 7n as unknown as number },
    ]);
    const counts = await makeSource(engine).fetchAssetCounts?.();
    expect(counts?.model).toBe(7);
  });

  it('maps file rows to the domain shape', async () => {
    const engine = fakeEngine(() => [
      {
        unique_id: 'model.a.b',
        name: 'b',
        resource_type: 'model',
        package_name: 'a',
        original_file_path: 'models/b.sql',
        patch_path: null,
      },
    ]);
    const files = await makeSource(engine).fetchFiles?.();
    expect(files).toHaveLength(1);
    expect(files?.[0]).toMatchObject({ uniqueId: 'model.a.b', name: 'b' });
  });

  it('returns null for an unknown asset, which drives the not-found page', async () => {
    const source = makeSource(fakeEngine(() => []));
    expect(
      await source.fetchAsset({ uniqueId: 'model.nope.nope', resourceType: 'model' }),
    ).toBeNull();
  });

  it('splits edges into dependsOn and referencedBy', async () => {
    const engine = fakeEngine((sql) => {
      if (sql.includes('dbt.edges')) {
        return [
          { direction: 'depends_on', unique_id: 'model.a.up', edge_type: 'ref' },
          { direction: 'referenced_by', unique_id: 'model.a.down', edge_type: 'ref' },
        ];
      }
      return [
        {
          unique_id: 'model.a.b',
          name: 'b',
          resource_type: 'analysis',
          package_name: 'a',
        },
      ];
    });

    const asset = await makeSource(engine).fetchAsset({
      uniqueId: 'model.a.b',
      resourceType: 'model',
    });

    expect(asset).toMatchObject({
      uniqueId: 'model.a.b',
      dependsOn: ['model.a.up'],
      referencedBy: ['model.a.down'],
    });
  });

  it('escapes quotes in ids rather than breaking the query', async () => {
    const engine = fakeEngine(() => []);
    await makeSource(engine).fetchAsset({
      uniqueId: "model.a.o'brien",
      resourceType: 'model',
    });
    expect(engine.calls[0]?.sql).toContain("'model.a.o''brien'");
  });

  it('asks only for the artifacts each surface needs', async () => {
    // Lazy registration is the whole reason a cold load stays cheap: the 836 KB of
    // column lineage must not be fetched to render a file tree.
    const engine = fakeEngine(() => []);
    const source = makeSource(engine);

    await source.fetchFiles?.();
    const filesTables = engine.calls.at(-1)?.tables ?? [];
    expect(filesTables).not.toContain('dbt.column_lineage');
    expect(filesTables).toContain('dbt_internal.resources');

    // A typed detail reads one artifact, not the union: the resource type is known,
    // and the union would pull every resource artifact to answer about one.
    await source.fetchAsset({ uniqueId: 'model.a.b', resourceType: 'model' });
    expect(engine.calls.at(-1)?.tables).toEqual(['dbt.models']);
  });
  describe('fetchOverview', () => {
    const overviewRow = {
      unique_id: 'doc.jaffle_shop.__overview__',
      package_name: 'jaffle_shop',
      block_contents: '# Jaffle Shop',
    };

    it('maps the winning row to the domain shape', async () => {
      const source = makeSource(fakeEngine(() => [overviewRow]));
      await expect(source.fetchOverview!()).resolves.toEqual({
        uniqueId: 'doc.jaffle_shop.__overview__',
        packageName: 'jaffle_shop',
        blockContents: '# Jaffle Shop',
      });
    });

    it('resolves null when no package defines an overview', async () => {
      // Zero rows is the "not defined" answer, and is what makes the page fall
      // back to its bundled default rather than render an empty page.
      const source = makeSource(fakeEngine(() => []));
      await expect(source.fetchOverview!()).resolves.toBeNull();
    });

    it('excludes the injected default by unique_id, not package_name', async () => {
      // `package_name` is NULL on any index written before the ingest populated
      // it, and `NULL <> 'dbt'` is NULL — that predicate would drop every row,
      // including the user's own overview.
      const engine = fakeEngine(() => []);
      await makeSource(engine).fetchOverview!();
      const { sql } = engine.calls[0];
      expect(sql).toContain("unique_id <> 'doc.dbt.__overview__'");
      expect(sql).not.toContain("package_name <> 'dbt'");
    });

    it('orders the root project ahead of dependency packages', async () => {
      // The choice happens in SQL under LIMIT 1, so it is only observable here.
      const engine = fakeEngine(() => []);
      await makeSource(engine).fetchOverview!();
      const { sql } = engine.calls[0];
      expect(sql).toContain("CASE WHEN package_name = 'jaffle_shop' THEN 0 ELSE 1 END");
      expect(sql).toContain('LIMIT 1');
    });

    it('escapes quotes in the project name rather than breaking the query', async () => {
      const engine = fakeEngine(() => []);
      const data = { ...BOOTSTRAP_DATA, project: { name: "o'brien" } };
      await makeSource(engine, data as BootstrapData).fetchOverview!();
      expect(engine.calls[0].sql).toContain("'o''brien'");
    });

    it('asks only for dbt.docs_blocks', async () => {
      const engine = fakeEngine(() => []);
      await makeSource(engine).fetchOverview!();
      expect(engine.calls[0].tables).toEqual(['dbt.docs_blocks']);
    });
  });

  describe('lineage', () => {
    it('is exposed now that it is ported', () => {
      expect('fetchLineage' in makeSource(fakeEngine(() => []))).toBe(true);
      expect('fetchColumnLineage' in makeSource(fakeEngine(() => []))).toBe(true);
    });

    it('caps depth at what the UI was built for', async () => {
      const engine = fakeEngine(() => []);
      // The Rust handler refused a larger `?max_depth`; a client-side query has no
      // one to refuse it, so the clamp has to live here.
      await makeSource(engine).fetchLineage?.({
        uniqueId: 'model.a.b',
        resourceType: 'model',
        depth: 99,
      });
      expect(engine.calls[0]?.sql).toContain('depth > -3');
      expect(engine.calls[0]?.sql).toContain('depth < 3');
    });

    it('unions the resource tables that appear in edges but not in nodes', async () => {
      // A metric or exposure in the graph would otherwise resolve to no name and
      // drop out of the join.
      const engine = fakeEngine(() => []);
      await makeSource(engine).fetchLineage?.({
        uniqueId: 'model.a.b',
        resourceType: 'model',
      });
      const sql = engine.calls[0]?.sql ?? '';
      expect(sql).toContain('dbt.metrics');
      expect(sql).toContain('dbt.semantic_models');
      expect(sql).toContain('dbt.exposures');
    });

    it('synthesizes a one-hop graph for saved queries', async () => {
      // They carry dependencies as a `depends_on_nodes` list, not as edge rows, so
      // the recursive walk would return nothing.
      const engine = fakeEngine((sql) =>
        sql.includes('dbt.saved_queries')
          ? [{ depends_on_nodes: ['model.a.orders', 'metric.a.revenue'] }]
          : [],
      );
      const graph = await makeSource(engine).fetchLineage?.({
        uniqueId: 'saved_query.a.weekly',
        resourceType: 'saved_query',
      });

      expect(graph?.nodes.map((n) => n.uniqueId)).toEqual([
        'saved_query.a.weekly',
        'model.a.orders',
        'metric.a.revenue',
      ]);
      // Resource types are inferred from the id prefix, which is all the list gives.
      expect(graph?.nodes[2]?.resourceType).toBe('metric');
      expect(graph?.edges).toHaveLength(2);
      // No recursive walk ran.
      expect(engine.calls.every((c) => !c.sql.includes('RECURSIVE'))).toBe(true);
    });

    it('reports column lineage gated when the relation has no rows', async () => {
      // Distinct from an empty graph: 'gated' is what renders the upgrade card,
      // and the difference is rows in `dbt.column_lineage` — not the presence of
      // its file, which the information schema always writes.
      const engine = fakeEngine(() => [], { columnLineage: false });
      const result = await makeSource(engine).fetchColumnLineage?.({
        uniqueId: 'model.a.b',
      });
      expect(result).toEqual({ kind: 'gated' });
    });

    it('lets a broken lineage query fail rather than reading as an upsell', async () => {
      // This used to map *any* thrown query to `gated`, so a mistake in the SQL
      // rendered as an upgrade card for a feature the user already had.
      const engine: DuckDbEngine = {
        query: async (sql: string) => {
          if (sql.includes('SELECT EXISTS')) return [{ present: true }] as never[];
          throw new Error('Binder Error: Referenced column "nope" not found');
        },
        hasTable: () => true,
        ready: async () => {},
      };
      await expect(
        makeSource(engine).fetchColumnLineage?.({ uniqueId: 'model.a.b' }),
      ).rejects.toThrow(/Binder Error/);
    });

    it('normalizes the raw evolution values to the vocabulary the UI renders', async () => {
      const engine = fakeEngine(() => [
        {
          from_node: 'model.a.up',
          from_column: 'id',
          to_node: 'model.a.b',
          to_column: 'id',
          lineage_kind: 'copy',
        },
        {
          from_node: 'model.a.up',
          from_column: 'name',
          to_node: 'model.a.b',
          to_column: 'upper',
          lineage_kind: 'mod',
        },
      ]);
      const result = await makeSource(engine).fetchColumnLineage?.({
        uniqueId: 'model.a.b',
      });

      expect(result?.kind).toBe('ok');
      const kinds =
        result?.kind === 'ok'
          ? result.graph.edges.map((e) => e.transformationType).sort()
          : [];
      // copy -> passthrough, mod -> transform.
      expect(kinds).toEqual(['passthrough', 'transform']);
    });

    it('returns an ok empty graph when present but nothing touches the node', async () => {
      const engine = fakeEngine(() => []);
      const result = await makeSource(engine).fetchColumnLineage?.({
        uniqueId: 'model.a.b',
      });
      // Present-but-empty must not read as gated, or the user sees an upsell for a
      // feature they already have.
      expect(result?.kind).toBe('ok');
    });
  });
});
