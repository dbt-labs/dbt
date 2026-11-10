import { describe, expect, test } from 'vitest';

import type { Capabilities, Distribution } from '../shared';
import { deriveUpgradeCapabilities } from './useCapabilities';

const cap = (overrides: Partial<Capabilities> = {}): Capabilities => ({
  hasColumnLineage: false,
  hasQueryHistory: false,
  hasCostInsights: false,
  hasPerformance: false,
  hasRecommendations: false,
  hasHealthSignals: false,
  hasAutoExposures: false,
  hasMultiProject: false,
  hasMesh: false,
  hasRunResults: false,
  hasCatalogStats: false,
  hasDbtState: false,
  ...overrides,
});

const dist = (isProprietary: boolean, isLoggedIn: boolean): Distribution => ({
  isProprietary,
  isLoggedIn,
  version: '0.0.0',
});

describe('deriveUpgradeCapabilities', () => {
  test('returns null while capabilities are still loading', () => {
    expect(deriveUpgradeCapabilities(null, dist(false, false))).toBeNull();
  });

  test('returns null while distribution is still loading', () => {
    expect(deriveUpgradeCapabilities(cap(), null)).toBeNull();
  });

  test('core distribution → core flags', () => {
    expect(deriveUpgradeCapabilities(cap(), dist(false, false))).toEqual({
      hasCll: false,
      hasDbtState: false,
      isProprietary: false,
      isLoggedIn: false,
    });
  });

  test('dbt v2 + logged in + CLL → fully unlocked flags', () => {
    expect(
      deriveUpgradeCapabilities(cap({ hasColumnLineage: true }), dist(true, true)),
    ).toEqual({
      hasCll: true,
      hasDbtState: false,
      isProprietary: true,
      isLoggedIn: true,
    });
  });

  test('dbt v2 + not logged in → proprietary but anon, CLL off', () => {
    expect(deriveUpgradeCapabilities(cap(), dist(true, false))).toEqual({
      hasCll: false,
      hasDbtState: false,
      isProprietary: true,
      isLoggedIn: false,
    });
  });

  test('hasDbtState passthrough', () => {
    expect(
      deriveUpgradeCapabilities(cap({ hasDbtState: true }), dist(true, true))
        ?.hasDbtState,
    ).toBe(true);
  });
});
