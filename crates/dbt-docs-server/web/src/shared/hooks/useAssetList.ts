import { useInfiniteQuery } from '@tanstack/react-query';

import { useMetadataDataSource } from '../context/MetadataDataProvider';
import type { AssetFilter, ListArgs } from '../typings/args';
import type { AssetSummary } from '../typings/domain/asset';
import { listKey } from '../util/queryKeys';
import { UNSUPPORTED_SURFACE_MESSAGE } from './unsupportedSurface';

/** Server-side page size when the consumer omits `limit`. Mirrors dbt-docs-v2's
 *  retired `useResourceList`/`useModels` `PAGE_SIZE`. */
const DEFAULT_PAGE_SIZE = 50;

export interface UseAssetListResult<T> {
  data: T[];
  total: number | null;
  isPending: boolean;
  isFetchingNextPage: boolean;
  hasNextPage: boolean;
  error: Error | null;
  /** Pre-formatted message for the list table; null when there's no error. */
  errorMessage: string | null;
  fetchNextPage(): void;
}

/**
 * Cursor-paginated asset list over the active {@link MetadataDataSource}. Gates
 * on the presence of `fetchAssetList` (disabled when the source lacks it).
 * Return shape mirrors dbt-docs-v2's retired `useResourceList` so the app swap
 * is mechanical. `T` narrows the union to the per-type summary the caller asked
 * for via `args.filter.resourceTypes`.
 */
export function useAssetList<T extends AssetSummary = AssetSummary>(
  args: ListArgs<AssetFilter>,
  errorLabel?: string,
): UseAssetListResult<T> {
  const source = useMetadataDataSource();
  const supported = 'fetchAssetList' in source;

  const query = useInfiniteQuery({
    queryKey: listKey(source.id, args),
    queryFn: ({ pageParam }) =>
      source.fetchAssetList!({
        ...args,
        limit: args.limit ?? DEFAULT_PAGE_SIZE,
        cursor: pageParam,
      }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.nextCursor ?? undefined,
    enabled: supported,
    select: (data) => ({
      data: data.pages.flatMap((p) => p.items),
      total: data.pages[0]?.totalCount ?? null,
    }),
  });

  return {
    data: (query.data?.data ?? []) as T[],
    total: query.data?.total ?? null,
    isPending: supported ? query.isPending : false,
    isFetchingNextPage: query.isFetchingNextPage,
    hasNextPage: query.hasNextPage,
    error: query.error,
    errorMessage: !supported
      ? UNSUPPORTED_SURFACE_MESSAGE
      : query.error
        ? `Failed to load ${errorLabel ?? 'assets'}.`
        : null,
    fetchNextPage: query.fetchNextPage,
  };
}
