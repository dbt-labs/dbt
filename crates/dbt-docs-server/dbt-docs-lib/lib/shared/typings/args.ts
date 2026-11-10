import type { ResourceType } from './domain/asset';

/**
 * Arguments identifying a single asset across any backend. `environmentId` is
 * optional because docs-only sources (dbt-docs-v2) have no notion of an
 * environment; the GraphQL Discovery API requires it.
 */
export type AssetArgs = {
  uniqueId: string;
  resourceType: ResourceType;
  environmentId?: string;
};

/** Filter applied when listing assets. Sources advertise which fields they
 *  honor via {@link MetadataDataSource.supportedFilters}. */
export type AssetFilter = {
  resourceTypes?: ResourceType[];
  search?: string;
  tags?: string[];
  /** Model-scoped: dbt-docs-server modeling layers (e.g. `['Marts']`). Only the
   *  `model` list endpoint honors it. */
  modelingLayers?: string[];
  /** Model-scoped: owner names. Only the `model` list endpoint honors it. */
  owners?: string[];
  /** Model/macro-scoped: package names. The `model` and `macro` list endpoints
   *  honor it. */
  packages?: string[];
  /** Test-scoped: test result statuses. Only the `test` list endpoint honors
   *  it. */
  results?: string[];
  /** Test-scoped: test types (e.g. `['data']`). Only the `test` list endpoint
   *  honors it. */
  testTypes?: string[];
};

/** Arguments for fetching the facet options that drive a resource type's list
 *  filter dropdowns. */
export type FacetsArgs = {
  resourceType: ResourceType;
};

/** Sort directive for a list request. `field` is a source-honored column name;
 *  adapters translate it (REST → `?sort=field:asc|desc`, GraphQL → orderBy). */
export type ListSort = {
  field: string;
  desc: boolean;
};

/** Cursor-paginated list request. `F` is the source-specific filter shape. */
export type ListArgs<F> = {
  filter?: F;
  cursor?: string | null;
  limit?: number;
  sort?: ListSort;
};

/** Arguments for node-level lineage. `depth` optionally caps how many hops
 *  out from the root the graph extends; omit for the source's default. */
export type LineageArgs = AssetArgs & {
  depth?: number;
};

/** Arguments for column-level lineage. Returns the whole-asset column lineage
 *  graph; consumers filter to a single column client-side. `environmentId` is
 *  optional for the same reason as {@link AssetArgs.environmentId}. */
export type ColumnLineageArgs = {
  uniqueId: string;
  environmentId?: string;
};
