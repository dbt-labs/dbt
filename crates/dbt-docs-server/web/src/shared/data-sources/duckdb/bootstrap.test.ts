import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { readBootstrap } from './bootstrap';

/**
 * These read real parquet, not a mock.
 *
 * The fixtures are the actual output of `dbt compile --generate-info-schema` on a
 * three-model duckdb project, copied verbatim. Two reasons they are not
 * hand-written. First, ZSTD: hyparquet has no built-in decoder, so the read goes
 * through `hyparquet-compressors`, and a mocked reader would prove nothing about
 * whether the browser can open what dbt actually writes. Second, the empty tables:
 * this project has no seeds, snapshots or sources, and the information schema
 * still writes a file for each — schema-only, zero rows — which is the shape first
 * paint has to tolerate on nearly every real project.
 *
 * `dbt.models.parquet` carries `compiled_code` and `search_text`, so the column
 * projection first paint relies on is genuinely exercised.
 */
const FIXTURES = join(__dirname, '../../../test/fixtures/parquet');

/** Every artifact the fixture project has. */
const ALL = [
  'dbt.models',
  'dbt.seeds',
  'dbt.snapshots',
  'dbt.sources',
  'dbt.analyses',
  'dbt.functions',
  'dbt.hooks',
  'dbt.data_tests',
  'dbt.unit_tests',
  'dbt.project',
];

/** Serve fixture files for the requested parquet, 404 for anything else. */
function stubFetch(available: string[] = ALL) {
  vi.stubGlobal(
    'fetch',
    vi.fn((input: RequestInfo | URL) => {
      const url = typeof input === 'string' ? input : input.toString();
      const table = url.split('/').pop()?.replace('.parquet', '') ?? '';
      if (!available.includes(table)) {
        return Promise.resolve(new Response(null, { status: 404 }));
      }
      const bytes = readFileSync(join(FIXTURES, `${table}.parquet`));
      return Promise.resolve(
        new Response(new Uint8Array(bytes), {
          status: 200,
          headers: { 'content-type': 'application/octet-stream' },
        }),
      );
    }),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('readBootstrap', () => {
  it('reads the zstd parquet dbt writes', async () => {
    stubFetch();
    const { nodes } = await readBootstrap('https://host/site/data/');

    // Three models and three generic tests; every other resource table is
    // schema-only.
    expect(nodes).toHaveLength(6);
    expect(nodes.find((n) => n.name === 'my_first_model')).toMatchObject({
      unique_id: 'model.docs_v2_move.my_first_model',
      name: 'my_first_model',
      resource_type: 'model',
      package_name: 'docs_v2_move',
      materialized: 'view',
      description: 'An example model',
      database_name: 'compute_demo',
      schema_name: 'main',
      original_file_path: 'models/example/my_first_model.sql',
    });
  });

  it('unions every resource type, since there is no view layer here', async () => {
    // First paint has no DuckDB, so it cannot read `dbt_internal.resources`; the
    // union is these reads concatenated. A resource type left out of the list
    // would be a silently empty sidebar section.
    stubFetch();
    const { nodes } = await readBootstrap('https://host/site/data/');
    const urls = (fetch as unknown as { mock: { calls: unknown[][] } }).mock.calls.map(
      (c) => String(c[0]),
    );
    for (const table of ALL) {
      expect(urls).toContain(`https://host/site/data/${table}.parquet`);
    }
    expect(new Set(nodes.map((n) => n.resource_type))).toEqual(
      new Set(['model', 'test']),
    );
  });

  it('supplies the resource type the test tables do not carry', async () => {
    // `dbt.data_tests` has no `resource_type` column — the table *is* the type —
    // and the shell groups by it, so something has to fill it in. Asking parquet
    // for a column it does not declare is also a hard read error, so the two test
    // tables are read with a narrower column list.
    stubFetch();
    const { nodes } = await readBootstrap('https://host/site/data/');
    const tests = nodes.filter((n) => n.resource_type === 'test');
    expect(tests).toHaveLength(3);
    expect(tests[0]?.unique_id).toMatch(/^test\.docs_v2_move\./);
  });

  it('projects only the NodeSummary columns, leaving the code behind', async () => {
    stubFetch();
    const { nodes } = await readBootstrap('https://host/site/data/');
    // `dbt.models` carries `compiled_code` and `search_text`; first paint asks for
    // nine columns by name so they are never decoded. The information schema's
    // column names are already the domain field names, so nothing is remapped.
    const model = nodes.find((n) => n.resource_type === 'model')!;
    expect(Object.keys(model).sort()).toEqual([
      'database_name',
      'description',
      'materialized',
      'name',
      'original_file_path',
      'package_name',
      'resource_type',
      'schema_name',
      'unique_id',
    ]);
  });

  it('reads project identity, renaming the columns the mapper does not know', async () => {
    stubFetch();
    const { project, generation } = await readBootstrap('https://host/site/data/');
    // Two renames, both silent if missed: `project_name` → `name`, or the project
    // renders nameless, and `git_uncommitted_changes` → `git_is_dirty`, or the
    // dirty-worktree marker never shows.
    expect(project).toMatchObject({
      name: 'docs_v2_move',
      dbt_version: '2.0.0-preview.220',
      adapter_type: 'duckdb',
      git_branch: 'main',
      git_is_dirty: false,
    });
    expect(project).not.toHaveProperty('project_name');
    expect(project).not.toHaveProperty('git_uncommitted_changes');
    // The staleness stamp, which `dbt.project.last_full_parse_at` absorbed from
    // the index's single-row `dbt.generation` table.
    expect(generation).toMatch(/^\d{4}-\d{2}-\d{2}T/);
  });

  it('resolves artifact URLs against the data base, so subpath hosting works', async () => {
    stubFetch();
    await readBootstrap('https://host/group/project/data/');
    const calls = (fetch as unknown as { mock: { calls: unknown[][] } }).mock.calls;
    const urls = calls.map((c) => String(c[0]));
    expect(urls).toContain('https://host/group/project/data/dbt.models.parquet');
  });

  it('tolerates a missing project artifact', async () => {
    // A single-row convenience; a site is still usable without it.
    stubFetch(['dbt.models']);
    const result = await readBootstrap('https://host/site/data/');
    expect(result.nodes).toHaveLength(3);
    expect(result.project).toBeNull();
    expect(result.generation).toBeNull();
  });

  it('renders what it has when a resource artifact is unreadable', async () => {
    // A partially copied site. Losing the snapshots section is better than losing
    // the shell, and every type failing is the empty-shell case either way.
    stubFetch([]);
    const result = await readBootstrap('https://host/site/data/');
    expect(result.nodes).toEqual([]);
    expect(result.project).toBeNull();
  });
});
