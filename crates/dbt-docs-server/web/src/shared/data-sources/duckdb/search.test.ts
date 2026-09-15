import { describe, expect, it } from 'vitest';

import {
  buildSearchQuery,
  escapeIlike,
  highlight,
  highlightDescription,
  highlightFor,
  MAX_PAGE_SIZE,
  tokenize,
} from './search';

function sqlFor(query: string, filter = {}) {
  const built = buildSearchQuery(query, filter, 50, 0);
  if (!built) throw new Error('expected a query');
  return built;
}

describe('tokenizing', () => {
  it('splits on whitespace and drops empties', () => {
    expect(tokenize('  my   first model ')).toEqual(['my', 'first', 'model']);
    expect(tokenize('   ')).toEqual([]);
  });
});

describe('escaping', () => {
  it('escapes the ILIKE wildcards, which the user typed as literals', () => {
    // Without this, a query of `%` matches every row in the project.
    expect(escapeIlike('100%')).toBe('100\\%');
    expect(escapeIlike('a_b')).toBe('a\\_b');
    expect(escapeIlike('a\\b')).toBe('a\\\\b');
  });

  it('escapes quotes so a query cannot break out of the literal', () => {
    expect(escapeIlike("o'brien")).toBe("o''brien");
  });

  it('carries the ESCAPE clause that makes the escaping mean anything', () => {
    expect(sqlFor('100%').sql).toContain("ESCAPE '\\'");
  });
});

describe('building the query', () => {
  it('returns null for an empty query rather than matching everything', () => {
    expect(buildSearchQuery('', {}, 50, 0)).toBeNull();
    expect(buildSearchQuery('   ', {}, 50, 0)).toBeNull();
  });

  it('returns null when the type filter names nothing searchable', () => {
    // "no results", not an error.
    expect(buildSearchQuery('x', { resourceTypes: ['analysis'] }, 50, 0)).toBeNull();
  });

  it('lists by type filter alone, with no search text', () => {
    // The Filter tab has no search box of its own -- ticking a type checkbox with
    // an empty query must still produce a real, filtered listing rather than
    // falling back to the empty-query "no results" case.
    const built = buildSearchQuery('', { resourceTypes: ['model'] }, 50, 0);
    expect(built).not.toBeNull();
    expect(built!.sql).not.toContain('field_matches');
    expect(built!.sql).not.toContain('winners');
    expect(built!.sql).toContain('AS matched_field');
    expect(built!.sql).toContain('ORDER BY cursor_key ASC, b.unique_id ASC');
  });

  it('lists by package/tag filter alone, with no search text', () => {
    const built = buildSearchQuery('', { packages: ['jaffle_shop'] }, 50, 0);
    expect(built).not.toBeNull();
    expect(built!.sql).toContain("b.package_name IN ('jaffle_shop')");
  });

  it('ANDs multiple tokens by intersecting on unique_id', () => {
    // Every token must match, though not necessarily in the same field — which is
    // why the intersect is on the id rather than the whole match row.
    const sql = sqlFor('my first').sql;
    expect(sql).toContain('token_matches_0');
    expect(sql).toContain('token_matches_1');
    expect(sql).toContain('INTERSECT');
  });

  it('uses one CTE and no intersect for a single token', () => {
    const sql = sqlFor('model').sql;
    expect(sql).toContain('field_matches AS (');
    expect(sql).not.toContain('INTERSECT');
  });

  it('searches all five fields with a fixed priority', () => {
    const sql = sqlFor('x').sql;
    for (const [field, priority] of [
      ['name', 1],
      ['column', 2],
      ['tag', 3],
      ['fqn', 4],
      ['description', 5],
    ] as const) {
      expect(sql).toContain(`'${field}'`);
      expect(sql).toContain(String(priority));
    }
  });

  it('narrows the union to the requested types', () => {
    const sql = sqlFor('x', { resourceTypes: ['model', 'macro'] }).sql;
    expect(sql).toContain('dbt.macros');
    // Other own-table branches drop out entirely.
    expect(sql).not.toContain('dbt.exposures');
    expect(sql).not.toContain('dbt.metrics');
  });

  it('ranks exact name matches above resource type', () => {
    // Deliberate divergence from dbt Catalog, which weights type more heavily and so
    // cannot guarantee an exact match ranks first.
    const sql = sqlFor('orders').sql;
    const exactAt = sql.indexOf("lower(b.name) = lower('orders')");
    const resourceAt = sql.indexOf('CASE b.resource_type');
    expect(exactAt).toBeGreaterThan(-1);
    expect(exactAt).toBeLessThan(resourceAt);
  });

  it('pads the ranking tiers so string order matches numeric order', () => {
    expect(sqlFor('x').sql).toContain(
      "LPAD(CAST((w.match_priority) AS VARCHAR), 1, '0')",
    );
  });

  it('sorts nameless rows last', () => {
    expect(sqlFor('x').sql).toContain("CASE WHEN b.name IS NULL THEN '9~9999'");
  });

  it('tie-breaks on unique_id so a page is stable', () => {
    expect(sqlFor('x').sql).toContain('ORDER BY cursor_key ASC, b.unique_id ASC');
  });

  it('caps the page size', () => {
    expect(buildSearchQuery('x', {}, 10_000, 0)!.limit).toBe(MAX_PAGE_SIZE);
  });

  it('finds the matched column in one join, not a query per row', () => {
    // The handler issued a separate query per column-matched row for this.
    const sql = sqlFor('id').sql;
    expect(sql).toContain('AS matched_column');
    expect(sql.match(/dbt\.node_columns/g)?.length).toBeLessThanOrEqual(2);
  });

  it('quotes filter values', () => {
    const sql = sqlFor('x', { packages: ["o'brien"], tags: ["a'b"] }).sql;
    expect(sql).toContain("'o''brien'");
    expect(sql).toContain("'a''b'");
  });
});

describe('highlighting', () => {
  it('wraps matches case-insensitively', () => {
    expect(highlight('My Orders Model', ['orders'])).toBe('My <b>Orders</b> Model');
  });

  it('escapes markup in the source text but not its own tags', () => {
    // A description containing HTML must render as text, or a docs page becomes an
    // injection point.
    const out = highlight('<script>alert(1)</script> orders', ['orders']);
    expect(out).toContain('&lt;script&gt;');
    expect(out).not.toContain('<script>');
    expect(out).toContain('<b>orders</b>');
  });

  it('treats regex metacharacters in the query as literals', () => {
    expect(highlight('a.b and axb', ['a.b'])).toBe('<b>a.b</b> and axb');
  });

  it('windows a long description around the match', () => {
    // A hit is only useful if the matched text is visible; taking the first 80
    // characters would often miss it entirely.
    const description = `${'x'.repeat(200)} needle ${'y'.repeat(200)}`;
    const out = highlightDescription(description, ['needle']);
    expect(out).toContain('<b>needle</b>');
    expect(out.startsWith('…')).toBe(true);
    expect(out.endsWith('…')).toBe(true);
  });

  it('does not window a short description', () => {
    expect(highlightDescription('all orders', ['orders'])).toBe('all <b>orders</b>');
  });

  it('highlights whichever field matched', () => {
    const row = {
      name: 'orders',
      description: 'all the orders',
      tags: ['finance', 'orders_daily'],
      fqn: ['jaffle', 'marts', 'orders'],
      matched_column: 'order_id',
    };
    expect(highlightFor({ ...row, matched_field: 'name' }, ['orders'])).toBe(
      '<b>orders</b>',
    );
    expect(highlightFor({ ...row, matched_field: 'tag' }, ['orders'])).toBe(
      '<b>orders</b>_daily',
    );
    expect(highlightFor({ ...row, matched_field: 'fqn' }, ['orders'])).toBe(
      'jaffle.marts.<b>orders</b>',
    );
    expect(highlightFor({ ...row, matched_field: 'column' }, ['order'])).toBe(
      '<b>order</b>_id',
    );
  });

  it('returns null when there is nothing to highlight', () => {
    expect(highlightFor({ matched_field: null }, ['x'])).toBeNull();
    expect(highlightFor({ matched_field: 'name', name: null }, ['x'])).toBeNull();
  });
});
