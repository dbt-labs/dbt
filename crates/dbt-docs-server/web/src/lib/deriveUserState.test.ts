import { describe, expect, test } from 'vitest';

import type { Distribution } from '../shared';
import { deriveUserState } from './deriveUserState';

const dist = (isProprietary: boolean, isLoggedIn: boolean): Distribution => ({
  isProprietary,
  isLoggedIn,
  version: '0.0.0',
});

describe('deriveUserState', () => {
  test('returns null while distribution is still loading', () => {
    expect(deriveUserState(null)).toBeNull();
  });

  test('maps non-dbt-v2 (Core) to core', () => {
    expect(deriveUserState(dist(false, false))).toBe('core');
  });

  test('treats Core as core even if logged in', () => {
    expect(deriveUserState(dist(false, true))).toBe('core');
  });

  test('maps dbt v2 + not logged in to proprietary-anon', () => {
    expect(deriveUserState(dist(true, false))).toBe('proprietary-anon');
  });

  test('maps dbt v2 + logged in to proprietary-logged-in', () => {
    expect(deriveUserState(dist(true, true))).toBe('proprietary-logged-in');
  });
});
