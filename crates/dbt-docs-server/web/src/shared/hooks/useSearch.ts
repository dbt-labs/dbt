import { useInfiniteQuery } from '@tanstack/react-query';

import { useMetadataDataSource } from '../context/MetadataDataProvider';
import type { ListArgs } from '../typings/args';
import type { SearchFilter, SearchHit, SearchResult } from '../typings/domain/search';
import { searchKey } from '../util/queryKeys';
import { UNSUPPORTED_SURFACE_MESSAGE } from './unsupportedSurface';

/** Server-side page size when the consumer omits `limit`. Mirrors dbt-docs-v2's
 *  `PROJECT_SEARCH_PAGE_SIZE`. */
const DEFAULT_PAGE_SIZE = 50;

export interface UseSearchResult {
  data: SearchHit[];
  total: number | null;
  isPending: boolean;
  isFetchingNextPage: boolean;
  hasNextPage: boolean;
  /** Non-200/non-400 failure (network/5xx) surfaced by react-query. */
  error: Error | null;
  /** Structured client-error code from a 400 search result; null otherwise. */
  errorCode: string | null;
  /** Structured client-error message from a 400 search result; null otherwise. */
  errorMessage: string | null;
  fetchNextPage(): void;
}

/**
 * Cursor-paginated cross-type search over the active {@link MetadataDataSource}.
 * Gates on the presence of `fetchSearch`. Unlike {@link useAssetList}, the
 * fetcher returns a discriminated {@link SearchResult}: `'ok'` pages flatten
 * into `data`, and the first `'error'` page's stable code/message surface as
 * `errorCode`/`errorMessage` (kept as data — a 400 is a valid response, not a
 * thrown failure). `error` carries only non-400 failures.
 */
export function useSearch(args: ListArgs<SearchFilter>): UseSearchResult {
  const source = useMetadataDataSource();

  const supported = 'fetchSearch' in source;

  const query = useInfiniteQuery({
    queryKey: searchKey(source.id, args),
    queryFn: ({ pageParam }) =>
      source.fetchSearch!({
        ...args,
        limit: args.limit ?? DEFAULT_PAGE_SIZE,
        cursor: pageParam,
      }),
    initialPageParam: null as string | null,
    getNextPageParam: (last) =>
      last.kind === 'ok' ? (last.page.nextCursor ?? undefined) : undefined,
    enabled: supported,
    select: (data) => {
      const okPages = data.pages.filter(
        (p): p is Extract<SearchResult, { kind: 'ok' }> => p.kind === 'ok',
      );
      const errorPage = data.pages.find(
        (p): p is Extract<SearchResult, { kind: 'error' }> => p.kind === 'error',
      );
      return {
        // Page 1's total is sticky — okPages[0] is always page 1.
        data: okPages.flatMap((p) => p.page.items),
        total: okPages[0]?.page.totalCount ?? null,
        errorCode: errorPage?.code ?? null,
        errorMessage: errorPage?.message ?? null,
      };
    },
  });

  return {
    data: query.data?.data ?? [],
    total: query.data?.total ?? null,
    isPending: supported ? query.isPending : false,
    isFetchingNextPage: query.isFetchingNextPage,
    hasNextPage: query.hasNextPage,
    error: query.error,
    errorCode: query.data?.errorCode ?? null,
    errorMessage: supported
      ? (query.data?.errorMessage ?? null)
      : UNSUPPORTED_SURFACE_MESSAGE,
    fetchNextPage: query.fetchNextPage,
  };
}
