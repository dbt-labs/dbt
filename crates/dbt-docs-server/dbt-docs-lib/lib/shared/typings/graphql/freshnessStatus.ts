import type { FreshnessStatusValue } from '../domain/status';

/**
 * GraphQL freshness status enum (capitalized values). Accepts the
 * string-literal union so any consumer GQL client's enum can pass in without
 * cross-module enum nominal-type issues.
 */
export type GqlFreshnessStatusLiteral =
  'Pass' | 'Warn' | 'Error' | 'Outdated' | 'Unconfigured' | 'Skipped' | 'Unknown';

export const freshnessStatusFromGql = (
  status: GqlFreshnessStatusLiteral,
): FreshnessStatusValue => {
  switch (status) {
    case 'Pass':
      return 'pass';
    case 'Warn':
      return 'warn';
    case 'Error':
      return 'error';
    case 'Outdated':
      return 'outdated';
    case 'Unconfigured':
      return 'unconfigured';
    case 'Skipped':
      return 'skipped';
    case 'Unknown':
      return 'unknown';
  }
};
