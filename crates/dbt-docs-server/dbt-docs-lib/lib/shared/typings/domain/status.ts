/** Lowercase, domain-shaped freshness status. Independent of GraphQL casing. */
export type FreshnessStatusValue =
  'pass' | 'warn' | 'error' | 'outdated' | 'unconfigured' | 'skipped' | 'unknown';

export type TestStatusValue =
  'pass' | 'fail' | 'warn' | 'error' | 'skipped' | 'unknown';
