import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

import { DETAIL_REGISTRY, GENERIC_DETAIL, nodeColumnsSql } from './details';
import { buildListQuery, FACET_QUERIES } from './lists';
import type { SearchFilter } from './search';
import {
  buildSearchQuery,
  SEARCH_FACET_ACCESSES,
  SEARCH_FACET_LAYERS,
  SEARCH_FACET_MATERIALIZATIONS,
  SEARCH_FACET_PACKAGES,
  SEARCH_FACET_TAGS,
} from './search';
import {
  columnLineageSql,
  COUNTS_SQL,
  FILES_SQL,
  lineageEdgesSql,
  lineageNodesSql,
  nodeEdgesSql,
  overviewSql,
  PROJECT_SQL,
  savedQueryDependsOnSql,
} from './sql';

/**
 * Every query in this app, bound against a real information schema.
 *
 * This is the mechanical form of the habit `.agents/dbt-docs-server.md` asks for:
 * *run the SQL against real parquet before wiring it up*. Nine bugs in the
 * original port were column names that did not exist, and two more turned up in
 * the migration onto the information schema — none of which types, fixtures or
 * SQL-shape assertions can catch, because a projection is just a string until
 * something binds it.
 *
 * Opt-in, because it needs artifacts and a `duckdb` binary:
 *
 * ```
 * dbt compile --generate-info-schema --static-analysis strict
 * DBT_DOCS_REAL_ARTIFACTS=<project>/target/info_schema/v1 pnpm test
 * ```
 *
 * It asserts only that each query *binds and runs*, never what it returns: the
 * corpus is whatever project was pointed at, and a query that returns no rows
 * there is not wrong. Binding is the part that breaks on a rename.
 */
const ARTIFACTS = process.env.DBT_DOCS_REAL_ARTIFACTS;

/** Every query, named so a failure says which surface broke. */
function everyQuery(): { name: string; sql: string }[] {
  const q: { name: string; sql: string }[] = [];
  const add = (name: string, sql: string) => q.push({ name, sql });
  const id = 'model.a.b';

  add('project', PROJECT_SQL);
  add('counts', COUNTS_SQL);
  add('files', FILES_SQL);
  add('overview', overviewSql('pkg'));
  add('nodeEdges', nodeEdgesSql(id));
  add('lineageNodes', lineageNodesSql(id));
  add('lineageEdges', lineageEdgesSql(id));
  add('savedQueryDependsOn', savedQueryDependsOnSql('saved_query.a.b'));
  add('columnLineage', columnLineageSql(id));
  add('columnLineage/column', columnLineageSql(id, 'id'));
  add('nodeColumns', nodeColumnsSql(id));

  for (const [type, spec] of Object.entries(DETAIL_REGISTRY)) {
    add(`detail:${type}`, spec!.sql(id));
    for (const extra of spec!.extras ?? []) {
      add(`detail:${type}:${extra.key}`, extra.sql(id));
    }
  }
  add('detail:generic', GENERIC_DETAIL.sql('analysis.a.b'));

  for (const type of Object.keys(DETAIL_REGISTRY)) {
    const built = buildListQuery({ filter: { resourceTypes: [type] } } as never);
    if (!built) continue;
    add(`list:${type}`, built.sql);
    add(`count:${type}`, built.countSql);
  }
  // A filtered, sorted model page, so the composed WHERE and ORDER BY run too.
  const filtered = buildListQuery({
    filter: {
      resourceTypes: ['model'],
      owners: ['finance'],
      packages: ['pkg'],
      modelingLayers: ['Staging'],
    },
    sort: { field: 'owner', desc: true },
  } as never);
  if (filtered) {
    add('list:model/filtered', filtered.sql);
    add('count:model/filtered', filtered.countSql);
  }

  for (const [type, facets] of Object.entries(FACET_QUERIES)) {
    for (const f of facets!) add(`facet:${type}:${f.key}`, f.sql);
  }
  add('searchFacet:accesses', SEARCH_FACET_ACCESSES);
  add('searchFacet:layers', SEARCH_FACET_LAYERS);
  add('searchFacet:materializations', SEARCH_FACET_MATERIALIZATIONS);
  add('searchFacet:tags', SEARCH_FACET_TAGS);
  add('searchFacet:packages', SEARCH_FACET_PACKAGES);

  const empty = {} as SearchFilter;
  const one = buildSearchQuery('orders', empty, 50, 0);
  if (one) {
    add('search:one-token', one.sql);
    add('search:one-token/count', one.countSql);
  }
  // Several tokens take the INTERSECT path, which is different SQL.
  const many = buildSearchQuery('customer orders', empty, 50, 0);
  if (many) {
    add('search:many-tokens', many.sql);
    add('search:many-tokens/count', many.countSql);
  }
  const typed = buildSearchQuery(
    'id',
    { resourceTypes: ['model', 'exposure'] } as SearchFilter,
    50,
    0,
  );
  if (typed) add('search:type-filtered', typed.sql);

  return q;
}

describe.skipIf(!ARTIFACTS)(
  'every query binds against a real information schema',
  () => {
    it('has the artifacts it was pointed at', () => {
      expect(existsSync(join(ARTIFACTS!, 'views.sql'))).toBe(true);
    });

    for (const { name, sql } of everyQuery()) {
      it(name, () => {
        // Wrapped in a view so DuckDB binds every column without the query having
        // to return rows, and executed through `views.sql` so the relations are
        // exactly the ones the browser will have.
        const script = `.read views.sql\nCREATE OR REPLACE TEMP VIEW probe AS ${sql
          .trim()
          .replace(/;$/, '')};\nSELECT count(*) FROM probe;\n`;
        const out = execFileSync('duckdb', ['-noheader', '-list'], {
          cwd: ARTIFACTS,
          input: script,
          encoding: 'utf8',
          stdio: ['pipe', 'pipe', 'pipe'],
        });
        // A count on its own line means it bound and ran. Warnings are tolerated
        // and arrive on stdout with ANSI colour: the search queries use the `->`
        // lambda arrow, which the pinned duckdb-wasm requires and a newer CLI
        // deprecates, so a strict match would fail on a query that is correct for
        // the engine that will actually run it.
        const plain = out.replace(/\u001b\[[0-9;]*m/g, '');
        expect(plain).toMatch(/^\d+$/m);
      });
    }
  },
);
