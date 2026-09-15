import { describe, expect, it } from 'vitest';

import type { ResourceType } from '../../typings/domain/asset';
import { DETAIL_REGISTRY, detailSpecFor, nodeColumnsSql } from './details';

describe('the detail registry', () => {
  it('covers every type that had its own REST detail endpoint', () => {
    expect(Object.keys(DETAIL_REGISTRY).sort()).toEqual([
      'exposure',
      'group',
      'macro',
      'metric',
      'model',
      'saved_query',
      'seed',
      'semantic_model',
      'snapshot',
      'source',
      'test',
    ]);
  });

  it('falls back to the resource union for types with no detail', () => {
    // `analysis`, `function` and `operation` were served that way under REST too.
    // The union is right here and nowhere else in this file: the fallback runs
    // precisely when the resource type is unknown, so there is no typed table to
    // pick.
    const spec = detailSpecFor('analysis');
    expect(spec).toBe(detailSpecFor('function'));
    expect(spec.sql('analysis.a.b')).toContain('FROM dbt_internal.resources n');
    expect(spec.sql('analysis.a.b')).toContain('n.unique_id =');
  });

  it('reads each type from its own table rather than filtering a union', () => {
    // The table *is* the resource type in the information schema, so the
    // `resource_type` predicate the index needed is gone — and reading the union
    // for a known type would pull every resource artifact to answer about one.
    expect(DETAIL_REGISTRY.model!.sql('model.a.b')).toContain('FROM dbt.models n');
    expect(DETAIL_REGISTRY.snapshot!.sql('snapshot.a.b')).toContain(
      'FROM dbt.snapshots n',
    );
    expect(DETAIL_REGISTRY.source!.sql('source.a.b')).toContain('FROM dbt.sources n');
    for (const type of ['model', 'snapshot', 'seed', 'source'] as ResourceType[]) {
      expect(DETAIL_REGISTRY[type]!.sql('x.a.b')).not.toContain('resource_type =');
    }
  });

  it('only asks for columns on the types that have them', () => {
    // A macro or metric has no relation, so a columns query would always be empty.
    expect(DETAIL_REGISTRY.model!.wantsColumns).toBe(true);
    expect(DETAIL_REGISTRY.source!.wantsColumns).toBe(true);
    expect(DETAIL_REGISTRY.macro!.wantsColumns).toBe(false);
    expect(DETAIL_REGISTRY.metric!.wantsColumns).toBe(false);
  });

  it('declares the JSON-string columns each type needs parsed', () => {
    // These are VARCHAR in the parquet; unparsed, an escaped JSON string reaches
    // the domain type (CC-7).
    expect(DETAIL_REGISTRY.model!.jsonColumns).toContain('meta');
    expect(DETAIL_REGISTRY.macro!.jsonColumns).toContain('arguments');
    expect(DETAIL_REGISTRY.metric!.jsonColumns).toContain('type_params');
    expect(DETAIL_REGISTRY.saved_query!.jsonColumns).toEqual([
      'query_params',
      'exports',
    ]);
  });

  it('escapes ids rather than breaking the query', () => {
    for (const type of Object.keys(DETAIL_REGISTRY) as ResourceType[]) {
      expect(DETAIL_REGISTRY[type]!.sql("model.a.o'brien")).toContain(
        "'model.a.o''brien'",
      );
    }
  });

  it("reads a test's own fields off dbt.data_tests, with no metadata join", () => {
    // `dbt.data_tests` is `nodes` LEFT JOINed to `test_metadata` inside the view,
    // so `column_name` and `severity` are columns here. The join this used to make
    // is what `USING (unique_id)` made ambiguous, breaking every test detail page;
    // there is no join left to get wrong.
    const sql = DETAIL_REGISTRY.test!.sql('test.a.b');
    expect(sql).toContain('FROM dbt.data_tests n');
    expect(sql).toContain('n.column_name');
    expect(sql).toContain('n.severity');
    expect(sql).not.toContain('dbt.test_metadata');
    expect(sql).not.toContain('USING (unique_id)');
    // A test's SQL, which `AssetCode` renders nothing without.
    expect(sql).toContain('n.raw_code');
  });

  it('keys semantic members on the parent id', () => {
    // Regression: these tables have no `semantic_model_unique_id`; members share the
    // parent's own `unique_id`, one row each.
    for (const extra of DETAIL_REGISTRY.semantic_model!.extras ?? []) {
      expect(extra.sql('semantic_model.a.b')).toContain('WHERE unique_id =');
      expect(extra.sql('semantic_model.a.b')).not.toContain('semantic_model_unique_id');
    }
  });

  it('matches group members on name and package, not id', () => {
    // Groups are keyed by (name, package); joining on unique_id would find nothing.
    // `group` is a SQL keyword, so the column has to be quoted to bind at all.
    const sql = DETAIL_REGISTRY.group!.extras![0]!.sql('group.a.b');
    expect(sql).toContain('g.name = n."group"');
    expect(sql).toContain('g.package_name = n.package_name');
  });

  it('orders columns by their declared position', () => {
    // Column order is meaningful in a table; alphabetical would be wrong.
    expect(nodeColumnsSql('model.a.b')).toContain('ORDER BY column_index');
  });

  it("puts the mapper's column-type names back", () => {
    // The information schema renames all four type columns and keys the table on
    // `node_unique_id`. The mapper reads the old names and falls through them in
    // order, so an un-aliased projection would null the Columns tab silently.
    const sql = nodeColumnsSql('model.a.b');
    expect(sql).toContain('WHERE node_unique_id =');
    expect(sql).toContain('data_type_declared AS declared_type');
    expect(sql).toContain('data_type_inferred AS inferred_type');
    expect(sql).toContain('data_type_actual AS catalog_type');
  });
});
