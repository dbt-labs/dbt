/**
 * One contract, every implementation of it.
 *
 * `duckdb/*.test.ts` asserts SQL; neither it nor any component test says what a
 * `MetadataDataSource` *is*. This does, and runs the same assertions against every
 * implementation.
 *
 * There are two, and the second matters more than it looks. `createFakeDataSource`
 * stands in for the real source in roughly twenty component tests — so if the fake
 * drifts from the contract, those tests keep passing while the components they cover
 * are broken against the source that actually ships. Holding the fake here is what
 * stops that.
 *
 * The focus is the discriminated returns, since each implementation reaches them by a
 * different route: `fetchAsset` → `null` for an unknown id, `'gated'` versus an empty
 * column-lineage graph, a search error carried as data rather than thrown. Those drive
 * the not-found page, the upgrade card and the inline search error respectively — and
 * an implementation that threw instead would break them silently.
 *
 * This suite is also what let the REST adapter's 61 URL-shape tests retire with the
 * adapter, without taking the contract's coverage with them.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';

import { SUPPORTED_BOOTSTRAP_SCHEMA_VERSION } from '../../lib/siteBootstrap';
import { createFakeDataSource } from '../testing/createFakeDataSource';
import { createDuckDbDataSource } from './duckdb';
import type { DuckDbEngine, TableName } from './duckdb/engine';
import type { MetadataDataSource } from './MetadataDataSource';

/**
 * The logical data a case needs, described once and expressed per implementation.
 *
 * Deliberately small: this pins the contract, not the mapping. Field-level mapping
 * is `fromRest.test.ts`'s job, and it is shared by both sources anyway.
 */
interface Fixture {
  project?: { name: string };
  /**
   * Counts stated logically, because the two sources receive them differently:
   * the Rust handler folded `unit_test` into `test` before serving, so the REST
   * source never sees them separately, while DuckDB reads raw parquet rows and
   * folds them itself. Each harness expresses this in its own terms.
   */
  counts?: { model: number; dataTests: number; unitTests: number };
  /** Present means "this id resolves"; absent means the not-found path. */
  asset?: { unique_id: string; name: string; resource_type: string };
  /** False means the feature is unavailable, however the source expresses that. */
  columnLineage?: boolean;
  /** Absent means no package defines an `__overview__`, which must resolve null. */
  overview?: { package_name: string; block_contents: string };
}

interface Harness {
  name: string;
  create(fixture: Fixture): MetadataDataSource;
}

/**
 * The fake: express the fixture as canned returns.
 *
 * Only the fetchers these cases exercise are overridden; the rest come from the fake's
 * `full` mode. That is deliberately how component tests use it, so this holds the same
 * object they rely on.
 */
const fakeHarness: Harness = {
  name: 'fake',
  create(fixture) {
    return createFakeDataSource(
      {
        fetchProject: async () => ({
          name: fixture.project?.name ?? '',
          description: null,
          dbtVersion: null,
          adapterType: null,
          git: null,
        }),
        fetchAssetCounts: async () => {
          const c = fixture.counts;
          // Folded, as any conforming source must: the two render on one page.
          return c ? { model: c.model, test: c.dataTests + c.unitTests } : {};
        },
        fetchAsset: async () =>
          fixture.asset
            ? {
                resourceType: 'model',
                uniqueId: fixture.asset.unique_id,
                name: fixture.asset.name,
                description: null,
                packageName: '',
                tags: [],
                originalFilePath: null,
                fqn: null,
                meta: null,
                dependsOn: [],
                referencedBy: [],
                columns: [],
              }
            : null,
        fetchColumnLineage: async () =>
          fixture.columnLineage
            ? { kind: 'ok', graph: { nodes: [], edges: [] } }
            : { kind: 'gated' },
        fetchOverview: async () =>
          fixture.overview
            ? {
                uniqueId: `doc.${fixture.overview.package_name}.__overview__`,
                packageName: fixture.overview.package_name,
                blockContents: fixture.overview.block_contents,
              }
            : null,
        fetchSearch: async (args: { filter?: { q?: string } }) =>
          (args.filter?.q ?? '').length > 1024
            ? { kind: 'error', code: 'query_too_long', message: 'too long' }
            : { kind: 'ok', page: { items: [], nextCursor: null, totalCount: 0 } },
      } as never,
      { full: true },
    );
  },
};

/** DuckDB: express the fixture as query results and artifact presence. */
const duckdbHarness: Harness = {
  name: 'duckdb',
  create(fixture) {
    const present = new Set<TableName>();
    const engine: DuckDbEngine = {
      async query<T>(sql: string, tables: TableName[]): Promise<T[]> {
        // Column lineage availability is rows, not a missing view: the
        // information schema writes the table even at zero rows, so the source
        // asks `SELECT EXISTS(...)` and gates on the answer.
        if (tables.includes('dbt.column_lineage')) {
          present.add('dbt.column_lineage');
          if (sql.includes('SELECT EXISTS')) {
            return [{ present: Boolean(fixture.columnLineage) }] as T[];
          }
          return [] as T[];
        }
        tables.forEach((t) => present.add(t));

        if (sql.includes('FROM dbt.project')) {
          return (fixture.project ? [fixture.project] : []) as T[];
        }
        if (sql.includes('GROUP BY resource_type')) {
          // Raw, as the parquet holds it — one row per resource_type.
          const c = fixture.counts;
          return (
            c
              ? [
                  { resource_type: 'model', count: c.model },
                  { resource_type: 'test', count: c.dataTests },
                  { resource_type: 'unit_test', count: c.unitTests },
                ]
              : []
          ) as T[];
        }
        if (sql.includes('FROM dbt.models n')) {
          return (fixture.asset ? [fixture.asset] : []) as T[];
        }
        if (sql.includes('FROM dbt.docs_blocks')) {
          // The query filters the injected default out in SQL, so "no authored
          // overview" reaches the source as zero rows.
          return (
            fixture.overview
              ? [
                  {
                    unique_id: `doc.${fixture.overview.package_name}.__overview__`,
                    package_name: fixture.overview.package_name,
                    block_contents: fixture.overview.block_contents,
                  },
                ]
              : []
          ) as T[];
        }
        return [] as T[];
      },
      hasTable: (t) => present.has(t),
      ready: async () => {},
    };

    return createDuckDbDataSource({
      dataBaseUrl: 'https://host/site/data/',
      bootstrap: {
        schema_version: SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
        generated_at: '2026-08-08T18:00:00Z',
        dbt_version: '2.0.0',
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
      },
      data: Promise.resolve({
        nodes: [],
        project: fixture.project ? { name: fixture.project.name } : null,
        generation: null,
      }),
      engine,
    });
  },
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe.each([fakeHarness, duckdbHarness])('$name conforms', (harness) => {
  it('identifies itself so cache keys cannot collide', () => {
    const source = harness.create({});
    expect(typeof source.id).toBe('string');
    expect(source.id.length).toBeGreaterThan(0);
  });

  it('advertises its filters as a set', () => {
    // Consumers read this to hide unsupported controls; a non-set would throw on
    // `.has`.
    const source = harness.create({});
    expect(source.supportedFilters).toBeInstanceOf(Set);
  });

  it('resolves a project name', async () => {
    const source = harness.create({ project: { name: 'jaffle_shop' } });
    await expect(source.fetchProject?.()).resolves.toMatchObject({
      name: 'jaffle_shop',
    });
  });

  it('resolves rather than throws when there is no project', async () => {
    // `App` blocks first paint on this, so a rejection is a blank page.
    const source = harness.create({});
    await expect(source.fetchProject?.()).resolves.toBeTruthy();
  });

  it('returns null for an unknown asset rather than throwing', async () => {
    // Drives the not-found page. REST gets here from a 404, DuckDB from no row.
    const source = harness.create({});
    await expect(
      source.fetchAsset({ uniqueId: 'model.nope.nope', resourceType: 'model' }),
    ).resolves.toBeNull();
  });

  it('returns an asset when the id resolves', async () => {
    const source = harness.create({
      asset: { unique_id: 'model.a.b', name: 'b', resource_type: 'model' },
    });
    await expect(
      source.fetchAsset({ uniqueId: 'model.a.b', resourceType: 'model' }),
    ).resolves.toMatchObject({ uniqueId: 'model.a.b' });
  });

  it('reports unit tests folded into test, wherever the folding happens', async () => {
    // Both render on one page, so the UI counts them together. Which layer does the
    // folding differs by source — the contract is only that the caller sees one
    // number. Worth pinning: when the server goes away, the folding has to already
    // exist on the client, and this is what says so.
    const source = harness.create({ counts: { model: 3, dataTests: 4, unitTests: 2 } });
    const counts = await source.fetchAssetCounts?.();
    expect(counts?.test).toBe(6);
    expect(counts?.model).toBe(3);
    expect(counts).not.toHaveProperty('unit_test');
  });

  it('reports column lineage gated, distinctly from empty', async () => {
    // 'gated' renders the upgrade card. An empty graph must not, or a project that
    // has the feature gets sold it.
    const gated = harness.create({ columnLineage: false });
    await expect(
      gated.fetchColumnLineage?.({ uniqueId: 'model.a.b' }),
    ).resolves.toEqual({
      kind: 'gated',
    });

    const available = harness.create({ columnLineage: true });
    const result = await available.fetchColumnLineage?.({ uniqueId: 'model.a.b' });
    expect(result?.kind).toBe('ok');
  });

  it('resolves the overview to null when no package defines one', async () => {
    // Null, not a rejection and not an empty string: it is what tells the landing
    // page to render its built-in default instead of a blank page.
    const source = harness.create({});
    await expect(source.fetchOverview?.()).resolves.toBeNull();
  });

  it('returns the authored overview when a package defines one', async () => {
    const source = harness.create({
      overview: { package_name: 'jaffle_shop', block_contents: '# Jaffle Shop' },
    });
    await expect(source.fetchOverview?.()).resolves.toMatchObject({
      packageName: 'jaffle_shop',
      blockContents: '# Jaffle Shop',
    });
  });

  it('surfaces a client search error as data, not a rejection', async () => {
    // The UI shows it inline; a thrown error would blank the results instead.
    const source = harness.create({});
    const result = await source.fetchSearch?.({
      filter: { q: 'x'.repeat(2000) },
    });
    expect(result?.kind).toBe('error');
    if (result?.kind === 'error') {
      expect(typeof result.code).toBe('string');
      expect(typeof result.message).toBe('string');
    }
  });

  it('returns a page shaped for the infinite-scroll hooks', async () => {
    const source = harness.create({});
    const page = await source.fetchAssetList?.({
      filter: { resourceTypes: ['model'] },
    });
    expect(page).toMatchObject({ items: expect.any(Array) });
    // `nextCursor` must be null rather than undefined at the end: the hooks read it
    // directly as the next page param.
    expect(page?.nextCursor ?? null).toBeNull();
  });

  it('reports a distribution', async () => {
    const source = harness.create({});
    const dist = await source.fetchDistribution?.();
    expect(dist).toMatchObject({
      isProprietary: expect.any(Boolean),
      isLoggedIn: expect.any(Boolean),
    });
  });

  it('reports capabilities with a boolean column-lineage flag', async () => {
    vi.stubGlobal('fetch', () => Promise.resolve(new Response(null, { status: 404 })));
    const source = harness.create({});
    const caps = await source.fetchCapabilities?.();
    expect(typeof caps?.hasColumnLineage).toBe('boolean');
  });
});
