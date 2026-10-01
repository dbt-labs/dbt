import type {
  AssetArgs,
  AssetFilter,
  ColumnLineageArgs,
  LineageArgs,
  ListArgs,
} from '../typings/args';
import type { SearchFilter } from '../typings/domain/search';

/**
 * react-query key for a single asset. Single source of truth shared by
 * `useAssetDetail` (read) and `wrapDataSource` (invalidation) — they must
 * produce byte-identical keys or invalidation silently misses.
 */
export function assetKey(sourceId: string, args: AssetArgs) {
  return [
    sourceId,
    'asset',
    args.environmentId,
    args.uniqueId,
    args.resourceType,
  ] as const;
}

/** react-query key for node-level lineage. Includes `depth` so distinct
 *  depth caps cache separately. Prefixed by `sourceId`. */
export function lineageKey(sourceId: string, args: LineageArgs) {
  return [
    sourceId,
    'lineage',
    args.environmentId,
    args.uniqueId,
    args.resourceType,
    args.depth,
  ] as const;
}

/** react-query key for column-level lineage. Mirrors `lineageKey`'s field
 *  order. Prefixed by `sourceId`. */
export function columnLineageKey(sourceId: string, args: ColumnLineageArgs) {
  return [sourceId, 'columnLineage', args.environmentId, args.uniqueId] as const;
}

/** react-query key for a cursor-paginated asset list. The filter is serialized
 *  deterministically (sorted array fields + search) so equivalent filters
 *  produce byte-identical keys regardless of array order — including the facet
 *  fields, so distinct facet selections never collide. Prefixed by `sourceId`. */
export function listKey(sourceId: string, args: ListArgs<AssetFilter>) {
  const f = args.filter ?? {};
  const filter = JSON.stringify({
    resourceTypes: [...(f.resourceTypes ?? [])].sort(),
    tags: [...(f.tags ?? [])].sort(),
    modelingLayers: [...(f.modelingLayers ?? [])].sort(),
    owners: [...(f.owners ?? [])].sort(),
    packages: [...(f.packages ?? [])].sort(),
    results: [...(f.results ?? [])].sort(),
    testTypes: [...(f.testTypes ?? [])].sort(),
    search: f.search ?? null,
  });
  const sort = args.sort
    ? `${args.sort.field}:${args.sort.desc ? 'desc' : 'asc'}`
    : null;
  return [sourceId, 'assetList', filter, args.limit ?? null, sort] as const;
}

/** react-query key for feature capabilities. Prefixed by `sourceId`. */
export function capabilitiesKey(sourceId: string) {
  return [sourceId, 'capabilities'] as const;
}

/** react-query key for a resource type's list facet options. Prefixed by
 *  `sourceId`. */
export function facetsKey(sourceId: string, resourceType: string) {
  return [sourceId, 'facets', resourceType] as const;
}

/** react-query key for build distribution. Prefixed by `sourceId`. */
export function distributionKey(sourceId: string) {
  return [sourceId, 'distribution'] as const;
}

/** react-query key for project-wide asset counts. Prefixed by `sourceId`. */
export function assetCountsKey(sourceId: string) {
  return [sourceId, 'assetCounts'] as const;
}

/** react-query key for project identity. Prefixed by `sourceId`. */
export function projectKey(sourceId: string) {
  return [sourceId, 'project'] as const;
}

/** react-query key for the project's overview block. Prefixed by `sourceId`. */
export function overviewKey(sourceId: string) {
  return [sourceId, 'overview'] as const;
}

/** react-query key for the project's file list. Prefixed by `sourceId`. */
export function filesKey(sourceId: string) {
  return [sourceId, 'files'] as const;
}

/** react-query key for a cursor-paginated cross-type search. Serializes the
 *  filter (sorted array fields + search text) and sort deterministically — same
 *  invariant as {@link listKey}, else equivalent filters collide. Prefixed by
 *  `sourceId`. */
export function searchKey(sourceId: string, args: ListArgs<SearchFilter>) {
  const f = args.filter ?? {};
  const filter = JSON.stringify({
    q: f.q ?? null,
    resourceTypes: [...(f.resourceTypes ?? [])].sort(),
    packages: [...(f.packages ?? [])].sort(),
    tags: [...(f.tags ?? [])].sort(),
    modelingLayers: [...(f.modelingLayers ?? [])].sort(),
    materializations: [...(f.materializations ?? [])].sort(),
  });
  const sort = args.sort
    ? `${args.sort.field}:${args.sort.desc ? 'desc' : 'asc'}`
    : null;
  return [sourceId, 'search', filter, args.limit ?? null, sort] as const;
}

/** react-query key for project-wide search facets. Prefixed by `sourceId`. */
export function searchFacetsKey(sourceId: string) {
  return [sourceId, 'searchFacets'] as const;
}
