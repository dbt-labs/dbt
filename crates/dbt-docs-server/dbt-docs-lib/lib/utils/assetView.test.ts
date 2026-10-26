import { describe, expect, test } from 'vitest';

import type { AssetColumn, MacroAsset, ModelAsset } from '../shared';
import { getColumns, toRelationshipItem } from './assetView';

const column: AssetColumn = {
  name: 'id',
  description: null,
  dataType: 'integer',
  declaredType: null,
  catalogType: null,
  tags: [],
  meta: {},
};

const modelAsset: ModelAsset = {
  uniqueId: 'model.my_pkg.my_model',
  name: 'my_model',
  resourceType: 'model',
  description: null,
  packageName: 'my_pkg',
  tags: [],
  rawCode: null,
  compiledCode: null,
  language: 'sql',
  access: null,
  contractEnforced: null,
  materializedType: null,
  group: null,
  relation: null,
  columns: [column],
};

const macroAsset: MacroAsset = {
  uniqueId: 'macro.my_pkg.my_macro',
  name: 'my_macro',
  resourceType: 'macro',
  description: null,
  packageName: 'my_pkg',
  tags: [],
  macroSql: 'select 1',
  arguments: [],
  path: 'macros/my_macro.sql',
};

describe('getColumns', () => {
  test('returns the columns for an asset shaped with a columns property', () => {
    expect(getColumns(modelAsset)).toBe(modelAsset.columns);
  });

  test('returns an empty array for an asset without a columns property', () => {
    expect(getColumns(macroAsset)).toEqual([]);
  });
});

describe('toRelationshipItem', () => {
  test('builds name from segments after the first two, and resourceType from the first', () => {
    expect(toRelationshipItem('model.my_pkg.my_model')).toEqual({
      uniqueId: 'model.my_pkg.my_model',
      name: 'my_model',
      resourceType: 'model',
    });
  });

  test('joins remaining segments for multi-part names', () => {
    expect(toRelationshipItem('model.my_pkg.staging.my_model')).toEqual({
      uniqueId: 'model.my_pkg.staging.my_model',
      name: 'staging.my_model',
      resourceType: 'model',
    });
  });

  test('falls back to the full uniqueId as name when only two segments are present', () => {
    expect(toRelationshipItem('model.pkg')).toEqual({
      uniqueId: 'model.pkg',
      name: 'model.pkg',
      resourceType: 'model',
    });
  });

  test('resourceType is an empty string for an empty uniqueId (split("") yields [""])', () => {
    expect(toRelationshipItem('')).toEqual({
      uniqueId: '',
      name: '',
      resourceType: '',
    });
  });
});
