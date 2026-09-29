import { useEffect, useMemo } from 'react';

import type { AssetFilters } from '../App';
import { Badge } from '../components/ui/Badge';
import { NotificationBanner } from '../components/ui/NotificationBanner';
import { Pill } from '../components/ui/Pill';
import { FEATURE_FLAGS } from '../lib/featureFlags';
import { RESOURCE_TYPE_LABEL, type ResourceTypeExplorer } from '../lib/resourceType';
import { isTelemetryInitialized, trackSearchPerformed } from '../lib/telemetry';
import { paths } from '../routes';
import type { Project } from '../shared';
import {
  formatAbsoluteLocalDate,
  getResultCountString,
  type HighlightsByField,
  type ListArgs,
  type MatchedField,
  type ResourceType,
  type RichSearchResultMetadata,
  type SearchFilter,
  type SearchHit,
  type SearchResultDisplayData,
  type SearchResultHit,
  SearchResultsList,
  toTitleCase,
  useSearch,
} from '../shared';
import type { NodeSummary, SearchErrorCode } from '../types';
import { SEARCHABLE_RESOURCE_TYPES } from '../types';

/** Page size for cross-type search — skeleton/fetch granularity. Mirrors the
 *  `useSearch` server-side default. */
const SEARCH_PAGE_SIZE = 50;

interface Props {
  project: Project;
  /** Unused — kept so the existing /search route wiring in App.tsx still type-checks. */
  nodes: NodeSummary[];
  query: string;
  filters: AssetFilters;
  onSetFilters(next: AssetFilters): void;
  /** Unused — peek drawer is hidden; search results navigate directly. */
  previewId: string | null;
  /** Unused — see above. */
  onPeek(uniqueId: string): void;
}

/**
 * Maps the four documented search-rejection codes to user-facing copy. Unknown
 * codes fall through to the raw `message` so the user still gets context if the
 * search layer adds a code this hasn't caught up with.
 */
function formatSearchError(code: SearchErrorCode | string, message: string): string {
  switch (code) {
    case 'query_too_long':
      return 'Your search query is too long. Try shortening it.';
    case 'invalid_type':
      return 'One of the selected resource types is not recognised. Reset the type filter.';
    case 'invalid_modeling_layer':
      return 'One of the selected modeling layers is not recognised. Reset the modeling-layer filter.';
    case 'invalid_cursor':
      return 'The page you were viewing has expired. Re-run the search to continue.';
    default:
      return message;
  }
}

function buildExtras(hit: SearchHit) {
  const { materialized, access, sourceName } = hit;
  if (!materialized && !access && !sourceName) return undefined;
  return (
    <>
      {materialized && (
        <Badge text={toTitleCase(materialized)} variant="secondary" size="xs" />
      )}
      {access && <Badge text={toTitleCase(access)} variant="secondary" size="xs" />}
      {sourceName && <Badge text={sourceName} variant="secondary" size="xs" />}
    </>
  );
}

type ChipDimension =
  'resourceType' | 'modelingLayer' | 'pkg' | 'tag' | 'materialization';

interface ActiveChip {
  dimension: ChipDimension;
  value: string;
  label: string;
}

/** Flatten the active filter map into a single ordered chip list. Resource-type
 *  chips lead so they read top-to-bottom in the order the Filter pane uses. */
function getActiveChips(filters: AssetFilters): ActiveChip[] {
  const chips: ActiveChip[] = [];
  for (const value of filters.resourceType) {
    chips.push({
      dimension: 'resourceType',
      value,
      label: RESOURCE_TYPE_LABEL[value] ?? value,
    });
  }
  for (const value of filters.modelingLayer) {
    chips.push({ dimension: 'modelingLayer', value, label: toTitleCase(value) });
  }
  for (const value of filters.pkg) {
    chips.push({ dimension: 'pkg', value, label: value });
  }
  for (const value of filters.tag) {
    chips.push({ dimension: 'tag', value, label: value });
  }
  for (const value of filters.materialization) {
    chips.push({ dimension: 'materialization', value, label: toTitleCase(value) });
  }
  return chips;
}

export default function Search({ query, filters, onSetFilters }: Props) {
  const searchArgs = useMemo<ListArgs<SearchFilter>>(
    () => ({
      filter: {
        q: query.trim() || undefined,
        // Drop types the backend doesn't index (e.g. `analysis`) so stale state
        // can't 400 the API. The Filter pane already hides these. `analysis` is
        // additionally gated behind FEATURE_FLAGS.hasAnalysis so stale
        // URL/filter state can't leak it into search while the flag is off.
        resourceTypes: filters.resourceType.filter(
          (t) =>
            SEARCHABLE_RESOURCE_TYPES.has(t) &&
            (FEATURE_FLAGS.hasAnalysis || t !== 'analysis'),
        ) as ResourceType[],
        packages: filters.pkg,
        modelingLayers: filters.modelingLayer,
        tags: filters.tag,
        materializations: filters.materialization,
      },
      limit: SEARCH_PAGE_SIZE,
    }),
    [
      query,
      filters.resourceType,
      filters.pkg,
      filters.modelingLayer,
      filters.tag,
      filters.materialization,
    ],
  );

  const {
    data: hits,
    total,
    isPending,
    isFetchingNextPage,
    hasNextPage,
    error: queryError,
    errorCode,
    errorMessage,
    fetchNextPage,
  } = useSearch(searchArgs);

  // A structured 400 surfaces as code/message (kept as data); a network/5xx
  // failure surfaces as `failed` (react-query error).
  const error = errorCode ? { code: errorCode, message: errorMessage ?? '' } : null;
  const failed = queryError != null;

  // Analytics: `search_performed`, keyed on the executed query + its result
  // count. Fired once the query settles (`!isPending`) for a non-empty query —
  // emitting on raw keystrokes would over-count.
  const executedQuery = query.trim();
  useEffect(() => {
    if (!isTelemetryInitialized()) return;
    if (isPending || !executedQuery) return;
    trackSearchPerformed({ search_query: executedQuery, result_count: total ?? 0 });
    // Key on the executed query + result count only — re-fetches that don't
    // change either shouldn't re-emit.
  }, [executedQuery, total, isPending]);

  /**
   * Lookup from `uniqueId` to the original {@link SearchHit}. Lets the
   * rich-metadata builders (which receive only the shared
   * {@link SearchResultHit} shape) get back to the docs-v2 render fields like
   * `packageName` and `materialized`.
   */
  const hitsByUniqueId = useMemo(() => {
    const map = new Map<string, SearchHit>();
    for (const hit of hits) map.set(hit.uniqueId, hit);
    return map;
  }, [hits]);

  const displayData: SearchResultDisplayData[] = useMemo(
    () =>
      hits.map((hit) => ({
        matchedField: hit.matchedField ?? 'name',
        highlight: hit.highlight,
        hit: {
          name: hit.name,
          uniqueId: hit.uniqueId,
          resourceType: hit.resourceType as ResourceTypeExplorer,
          fqn: hit.fqn,
        },
      })),
    [hits],
  );

  const getRichMetadata = (
    hit: SearchResultHit,
  ): RichSearchResultMetadata | undefined => {
    const original = hitsByUniqueId.get(hit.uniqueId);
    if (!original) return undefined;
    const execDate = original.executedAt ? new Date(original.executedAt) : undefined;
    const lastRunLabel = execDate
      ? `Last run: ${formatAbsoluteLocalDate(execDate)}`
      : undefined;
    return {
      projectName: original.packageName ?? undefined,
      extras: buildExtras(original),
      lastRunLabel,
    };
  };

  /**
   * The docs-v2 backend returns a single `matched_field` + `highlight` per edge
   * (not the multi-field map dbt-explorer's GraphQL exposes). Synthesise a
   * single-entry payload so the shared pill row still fires for non-name
   * matches.
   */
  const getRichHighlights = (
    data: SearchResultDisplayData,
  ): HighlightsByField | undefined => {
    if (!data.highlight || data.matchedField === 'name') return undefined;
    return { [data.matchedField as MatchedField]: [data.highlight] };
  };

  const showSkeleton = isPending && hits.length === 0;
  const showLoadingMore = isFetchingNextPage;
  const activeChips = useMemo(() => getActiveChips(filters), [filters]);

  /**
   * Backend ANDs all filters, and only models carry a `modeling_layer`. If the
   * user has narrowed the type filter to a set that excludes `model`, combining
   * with any modeling layer guarantees an empty page — surface that as a
   * warning so the empty state is self-explanatory.
   */
  const modelingLayerConflict =
    filters.modelingLayer.length > 0 &&
    filters.resourceType.length > 0 &&
    !filters.resourceType.includes('model');

  const removeChip = (chip: ActiveChip) => {
    onSetFilters({
      ...filters,
      [chip.dimension]: filters[chip.dimension].filter((v) => v !== chip.value),
    });
  };

  const clearAllChips = () => {
    onSetFilters({
      ...filters,
      resourceType: [],
      modelingLayer: [],
      pkg: [],
      tag: [],
      materialization: [],
    });
  };

  return (
    <div className="flex flex-col gap-4 p-4">
      <div className="flex items-baseline justify-between">
        <h1 className="text-lg font-semibold">All results</h1>
        {total != null && (
          <span className="text-sm text-fgDecorative">
            {getResultCountString(total)}
          </span>
        )}
      </div>
      {activeChips.length > 0 && (
        <FilterChips
          chips={activeChips}
          onRemove={removeChip}
          onClearAll={clearAllChips}
        />
      )}
      {error && !modelingLayerConflict && (
        <NotificationBanner
          notification={{
            id: 'search-bad-request',
            type: 'warning',
            header: formatSearchError(error.code, error.message),
          }}
        />
      )}
      {modelingLayerConflict && (
        <NotificationBanner
          notification={{
            id: 'search-modeling-layer-conflict',
            type: 'info',
            header:
              'Modeling layer only applies to models. Clear the modeling layer filter, or add "Models" to the resource type filter, to see results.',
          }}
        />
      )}
      {failed && (
        <NotificationBanner
          notification={{
            id: 'search-failed',
            type: 'error',
            header: 'Search failed. Check your connection and try again.',
          }}
        />
      )}
      <SearchResultsList
        query={query}
        pageSize={SEARCH_PAGE_SIZE}
        isLoadingMore={showLoadingMore}
        skeleton={showSkeleton}
        hasMoreResults={hasNextPage}
        data={showSkeleton ? undefined : displayData}
        fetchMore={fetchNextPage}
        getResourceHref={(uniqueId) => paths.details(uniqueId)}
        variant="rich"
        getRichMetadata={getRichMetadata}
        getRichHighlights={getRichHighlights}
      />
    </div>
  );
}

function FilterChips({
  chips,
  onRemove,
  onClearAll,
}: {
  chips: ActiveChip[];
  onRemove: (chip: ActiveChip) => void;
  onClearAll: () => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      {chips.map((chip) => (
        <Pill
          key={`${chip.dimension}:${chip.value}`}
          id={`${chip.dimension}:${chip.value}`}
          value={chip.label}
          onClickRemove={() => onRemove(chip)}
        />
      ))}
      <button
        type="button"
        onClick={onClearAll}
        className="hover:text-fgDefault text-xs text-fgDecorative underline-offset-2 hover:underline"
      >
        Clear all
      </button>
    </div>
  );
}
