import { QueryClient } from '@tanstack/react-query';
import { describe, expect, it } from 'vitest';

import type { MetadataDataSource } from '../data-sources/MetadataDataSource';
import { createFakeDataSource } from '../testing/createFakeDataSource';
import { wrapDataSource } from './wrapDataSource';

/**
 * `wrapDataSource` forwards through a hardcoded `FETCHERS` allowlist and returns
 * a *new* object, so a fetcher missing from that list is silently dropped in
 * production while every other test keeps passing — hooks gate on
 * `method in source`, so the surface just never fires, and unit tests exercise
 * the unwrapped source.
 *
 * This suite exists to make that failure loud for every fetcher, present and
 * future, rather than once per fetcher someone remembers to cover.
 */
describe('wrapDataSource', () => {
  const wrap = (source: MetadataDataSource) =>
    wrapDataSource(source, new QueryClient());

  it('forwards every fetcher the source provides', () => {
    const source = createFakeDataSource({}, { full: true });
    const wrapped = wrap(source);

    const forwarded = wrapped as unknown as Record<string, unknown>;
    const missing = Object.entries(source)
      .filter(([, value]) => typeof value === 'function')
      .map(([name]) => name)
      .filter((name) => typeof forwarded[name] !== 'function');

    expect(missing).toEqual([]);
  });

  it('does not invent fetchers the source omits', () => {
    // The mirror of the above: hooks read absence as "unsupported", so the
    // wrapper must not advertise a surface the source cannot serve.
    const wrapped = wrap(createFakeDataSource());
    expect('fetchOverview' in wrapped).toBe(false);
    expect('fetchLineage' in wrapped).toBe(false);
  });

  it('preserves the source identity used to namespace cache keys', () => {
    expect(wrap(createFakeDataSource()).id).toBe('fake');
  });
});
