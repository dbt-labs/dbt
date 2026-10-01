import { describe, expect, test } from 'vitest';

import type { FacetValue } from '../shared';
import { ALL_FACET_OPTION, facetOptions, selectedFacetOption } from './facetOptions';

describe('facetOptions', () => {
  test('returns only the "All" option when values is undefined', () => {
    expect(facetOptions(undefined)).toEqual([ALL_FACET_OPTION]);
  });

  test('prepends the "All" option and maps values without a count', () => {
    const values: FacetValue[] = [{ value: 'model', count: null }];
    expect(facetOptions(values)).toEqual([
      ALL_FACET_OPTION,
      { label: 'model', value: 'model' },
    ]);
  });

  test('includes the count suffix when count is present', () => {
    const values: FacetValue[] = [{ value: 'model', count: 5 }];
    expect(facetOptions(values)).toEqual([
      ALL_FACET_OPTION,
      { label: 'model (5)', value: 'model' },
    ]);
  });

  test('applies formatValue to the label but not the value', () => {
    const values: FacetValue[] = [{ value: 'model', count: 3 }];
    const result = facetOptions(values, (v) => v.toUpperCase());
    expect(result).toEqual([ALL_FACET_OPTION, { label: 'MODEL (3)', value: 'model' }]);
  });

  test('maps multiple values in order', () => {
    const values: FacetValue[] = [
      { value: 'a', count: 1 },
      { value: 'b', count: null },
    ];
    expect(facetOptions(values)).toEqual([
      ALL_FACET_OPTION,
      { label: 'a (1)', value: 'a' },
      { label: 'b', value: 'b' },
    ]);
  });
});

describe('selectedFacetOption', () => {
  test('returns the option matching the value', () => {
    const options = [ALL_FACET_OPTION, { label: 'model', value: 'model' }];
    expect(selectedFacetOption(options, 'model')).toEqual({
      label: 'model',
      value: 'model',
    });
  });

  test('falls back to ALL_FACET_OPTION when no option matches', () => {
    const options = [{ label: 'model', value: 'model' }];
    expect(selectedFacetOption(options, 'missing')).toBe(ALL_FACET_OPTION);
  });

  test('falls back to ALL_FACET_OPTION even when it is not present in options', () => {
    const options = [{ label: 'model', value: 'model' }];
    expect(selectedFacetOption(options, 'not-in-list')).toBe(ALL_FACET_OPTION);
  });
});
