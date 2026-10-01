import type { QueryClient } from '@tanstack/react-query';

import type { MetadataDataSource } from '../data-sources/MetadataDataSource';
import type { AssetArgs } from '../typings/args';
import { assetKey } from '../util/queryKeys';

/** Telemetry event emitted for each fetcher call. */
export type DataSourceTelemetryEvent = {
  source: string;
  method: string;
  durationMs: number;
  ok: boolean;
};

/**
 * Knobs for {@link wrapDataSource}. Intentionally minimal in Phase 0 — this is
 * the seam where auth, richer retry/backoff, and telemetry sinks land later.
 */
export interface WrapDataSourceOptions {
  /** Called after each fetcher resolves or rejects. */
  telemetry?: (event: DataSourceTelemetryEvent) => void;
  /**
   * Total attempts per fetcher (including the first). Default 1 (no retry).
   *
   * Retry here is for **non-react-query call sites**. react-query-driven hooks
   * (e.g. `useAssetDetail`) own their own retry — do not set `retryAttempts > 1`
   * for a source consumed through such a hook, or the two layers compound.
   */
  retryAttempts?: number;
}

const FETCHERS = [
  'fetchAsset',
  'fetchAssetList',
  'fetchFacets',
  'fetchLineage',
  'fetchColumnLineage',
  'fetchCapabilities',
  'fetchDistribution',
  'fetchAssetCounts',
  'fetchProject',
  'fetchOverview',
  'fetchFiles',
  'fetchSearch',
  'fetchSearchFacets',
] as const satisfies readonly (keyof MetadataDataSource)[];

async function withRetry<T>(fn: () => Promise<T>, attempts: number): Promise<T> {
  let lastError: unknown;
  for (let i = 0; i < Math.max(1, attempts); i++) {
    try {
      return await fn();
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError;
}

/**
 * Decorate a {@link MetadataDataSource} with cross-cutting concerns: telemetry,
 * simple retry, and freshness invariants. The returned source's
 * `onAppliedUpdatedAt` / `onDefinitionUpdatedAt` invalidate the matching
 * react-query keys so stale detail views refetch. Data-source-level analogue of
 * dbt-explorer's `useCacheMonitor`, but react-query-based.
 */
export function wrapDataSource(
  source: MetadataDataSource,
  queryClient: QueryClient,
  opts: WrapDataSourceOptions = {},
): MetadataDataSource {
  const { telemetry, retryAttempts = 1 } = opts;

  const decorate =
    <Args extends unknown[], Result>(
      method: string,
      fn: (...args: Args) => Promise<Result>,
    ) =>
    async (...args: Args): Promise<Result> => {
      const start = Date.now();
      try {
        const result = await withRetry(() => fn(...args), retryAttempts);
        telemetry?.({
          source: source.id,
          method,
          durationMs: Date.now() - start,
          ok: true,
        });
        return result;
      } catch (error) {
        telemetry?.({
          source: source.id,
          method,
          durationMs: Date.now() - start,
          ok: false,
        });
        throw error;
      }
    };

  const wrapped: MetadataDataSource = {
    id: source.id,
    supportedFilters: source.supportedFilters,
    // Always present.
    fetchAsset: decorate('fetchAsset', source.fetchAsset.bind(source)),

    // Freshness invariants: a newer timestamp means cached data is stale.
    onAppliedUpdatedAt: (args: AssetArgs, updatedAt: string) => {
      queryClient.invalidateQueries({ queryKey: assetKey(source.id, args) });
      source.onAppliedUpdatedAt?.(args, updatedAt);
    },
    onDefinitionUpdatedAt: (args: AssetArgs, updatedAt: string) => {
      // A definition change is structural — invalidate everything for this source.
      queryClient.invalidateQueries({ queryKey: [source.id] });
      source.onDefinitionUpdatedAt?.(args, updatedAt);
    },
  };

  // Forward the optional fetchers only when the wrapped source provides them,
  // so capability detection (`'fetchLineage' in source`) keeps working.
  for (const method of FETCHERS) {
    if (method === 'fetchAsset') continue;
    const fn = source[method];
    if (typeof fn === 'function') {
      (wrapped as any)[method] = decorate(method, (fn as any).bind(source));
    }
  }

  return wrapped;
}
