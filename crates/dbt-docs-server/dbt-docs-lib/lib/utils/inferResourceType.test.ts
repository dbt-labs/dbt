import { describe, expect, test } from 'vitest';

import type { NodeSummary } from '../types';
import { inferResourceType, resolveAssetArgs } from './inferResourceType';

/**
 * `inferResourceType` parses the `<resource_type>.<package>.<name>` prefix.
 * The historic foot-gun: resources whose backend serves bare-name unique_ids
 * (only `saved_query` today) have no prefix, so they must fall back to
 * `saved_query` rather than misroute.
 */

const TYPED = [
  ['macro', 'macro.acme.my_macro'],
  ['semantic_model', 'semantic_model.acme.orders'],
  ['saved_query', 'saved_query.acme.weekly_revenue'],
  ['source', 'source.acme.raw.orders'],
  ['exposure', 'exposure.acme.weekly'],
  ['group', 'group.acme.finance'],
  ['model', 'model.acme.orders'],
  ['seed', 'seed.acme.country_codes'],
  ['snapshot', 'snapshot.acme.orders_snapshot'],
  ['test', 'test.acme.unique_orders_id'],
  ['unit_test', 'unit_test.acme.orders_unit'],
  ['metric', 'metric.acme.revenue'],
  ['analysis', 'analysis.acme.foo'],
  ['function', 'function.acme.foo'],
] as const;

describe('inferResourceType', () => {
  test.each(TYPED)('extracts %s from <type>.<package>.<name>', (type, id) => {
    expect(inferResourceType(id)).toBe(type);
  });

  test('falls back to saved_query for bare-name ids (the historic foot-gun)', () => {
    expect(inferResourceType('dbt_invocations_by_billing_email')).toBe('saved_query');
  });
});

describe('resolveAssetArgs', () => {
  const node = (unique_id: string, resource_type: string): NodeSummary => ({
    unique_id,
    name: unique_id.split('.').at(-1) ?? unique_id,
    resource_type,
  });

  test('null id → null', () => {
    expect(resolveAssetArgs(null, [])).toBeNull();
  });

  test('prefers the loaded nodes list resource_type over inference', () => {
    // A snapshot id whose list entry says it's a snapshot — inference would
    // also say snapshot here, so use a case where the list is authoritative.
    const nodes = [node('model.acme.orders', 'model')];
    expect(resolveAssetArgs('model.acme.orders', nodes)).toEqual({
      uniqueId: 'model.acme.orders',
      resourceType: 'model',
    });
  });

  test('falls back to prefix inference when the id is absent from nodes', () => {
    expect(resolveAssetArgs('metric.acme.revenue', [])).toEqual({
      uniqueId: 'metric.acme.revenue',
      resourceType: 'metric',
    });
  });

  test('falls back to saved_query for bare-name ids absent from nodes', () => {
    expect(resolveAssetArgs('weekly_revenue', undefined)).toEqual({
      uniqueId: 'weekly_revenue',
      resourceType: 'saved_query',
    });
  });
});
