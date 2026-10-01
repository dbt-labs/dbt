import { FC, useMemo } from 'react';

import { PaginationFooter } from '../../../components/ui/PaginationFooter';
import { TrustSignals } from '../../typings/trustSignals';
import { HighlightsByField } from './HighlightPills';
import { RichSearchResultItem, RichSearchResultMetadata } from './RichSearchResultItem';
import { SearchResultItem } from './SearchResultItem';
import { SearchResultDisplayData, SearchResultHit } from './types';

export const getResultCountString = (count: number | undefined): string | null => {
  if (count === undefined) return null;
  if (count === 1) return '1 result';
  return `${count} results`;
};

export interface SearchResultsListParams {
  pageSize: number;
  query: string;
  isLoadingMore: boolean;
  skeleton?: boolean;
  hasMoreResults: boolean;
  data: SearchResultDisplayData[] | undefined;
  fetchMore: () => void;
  testId?: string;
  /** Per-row link builder for the resource detail page. */
  getResourceHref: (uniqueId: string) => string;
  /** Per-row link builder for a specific column (optional). */
  getColumnHref?: (uniqueId: string, columnName: string) => string;
  /** Per-row link builder for the "View lineage" CTA (optional, default variant only). */
  getLineageHref?: (uniqueId: string, fqn: string[]) => string;
  /**
   * Optional resolver for per-hit trust signals. Callers that don't render
   * trust signals (e.g. docs-v2) can omit this entirely.
   */
  getTrustSignals?: (hit: SearchResultHit) => TrustSignals | undefined;
  /**
   * Switches the per-row presentation:
   *   - `'default'` (omitted) — single-line {@link SearchResultItem}, with
   *     "Includes column/tag/description: …" secondary line.
   *   - `'rich'` — card-style {@link RichSearchResultItem} with a metadata
   *     row (project, resource type, columns, …) and pill row.
   */
  variant?: 'default' | 'rich';
  /** Rich variant: per-row metadata (project name, columns, materialization, …). */
  getRichMetadata?: (hit: SearchResultHit) => RichSearchResultMetadata | undefined;
  /** Rich variant: per-row pill payload. */
  getRichHighlights?: (data: SearchResultDisplayData) => HighlightsByField | undefined;
}

/**
 * Pure presentational list of search results. Handles:
 *   - rendering an item per `data` entry
 *   - skeleton placeholders during the initial load and load-more
 *   - empty state
 *   - the `Load more` pagination footer
 *
 * Data fetching, filter pills, page heading, and breadcrumbs are intentionally
 * left to a smart container in the consumer (e.g. dbt-explorer's
 * `SearchResultsContents`).
 */
export const SearchResultsList: FC<SearchResultsListParams> = ({
  query,
  testId = 'search-result',
  isLoadingMore,
  skeleton,
  hasMoreResults,
  pageSize,
  data,
  fetchMore,
  getResourceHref,
  getColumnHref,
  getLineageHref,
  getTrustSignals,
  variant = 'default',
  getRichMetadata,
  getRichHighlights,
}) => {
  const displayedData = useMemo(() => {
    let result: (SearchResultDisplayData | 'skeleton')[] | undefined = data
      ? [...data]
      : undefined;
    if (skeleton || isLoadingMore) {
      result ??= [];
      const skeletons = new Array<'skeleton'>(pageSize).fill('skeleton');
      result.push(...skeletons);
    }
    return result;
  }, [data, isLoadingMore, pageSize, skeleton]);

  return (
    <div className={variant === 'rich' ? 'space-y-0' : 'space-y-2'}>
      {displayedData?.map((searchResult, index) => {
        if (searchResult === 'skeleton') {
          if (variant === 'rich') {
            return (
              <RichSearchResultItem
                key={`skeleton-${index}`}
                skeleton
                testId={`${testId}-${index}`}
              />
            );
          }
          return (
            <SearchResultItem
              key={`skeleton-${index}`}
              skeleton
              testId={`${testId}-${index}`}
            />
          );
        }
        if (variant === 'rich') {
          return (
            <RichSearchResultItem
              key={searchResult.hit.uniqueId}
              query={query}
              data={searchResult}
              testId={`${testId}-${index}`}
              getResourceHref={getResourceHref}
              getColumnHref={getColumnHref}
              metadata={getRichMetadata?.(searchResult.hit)}
              highlights={getRichHighlights?.(searchResult)}
              trustSignals={getTrustSignals?.(searchResult.hit)}
            />
          );
        }
        return (
          <SearchResultItem
            key={searchResult.hit.uniqueId}
            query={query}
            data={searchResult}
            testId={`${testId}-${index}`}
            getResourceHref={getResourceHref}
            getColumnHref={getColumnHref}
            getLineageHref={getLineageHref}
            trustSignals={getTrustSignals?.(searchResult.hit)}
          />
        );
      })}
      {displayedData?.length === 0 && <div>No results were found</div>}
      {hasMoreResults && (
        <PaginationFooter
          hasMorePages={hasMoreResults}
          onLoadMore={fetchMore}
          isPageLoading={isLoadingMore || skeleton}
        />
      )}
    </div>
  );
};
