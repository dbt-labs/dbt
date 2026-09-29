/**
 * Turn a wire-shaped fixture into a data source, for tests written against the API.
 *
 * A lot of component tests describe their data the way the server used to send it —
 * `{ data: [...], page_info: {...} }` with snake_case rows — and stubbed `fetch` so the
 * REST source would read it. With that source gone, the fixtures are still the right
 * description of the data; only the delivery changed.
 *
 * So rather than rewriting every fixture by hand into domain objects, these map them
 * through the same per-type mappers the real source uses and hand back a
 * `MetadataDataSource`. The tests keep asserting what they always did, and the mapping
 * they exercise is the one that ships.
 *
 * One caveat: rows must be shaped as the *source's projection* emits them, which is not
 * always identical to the old response body. `model` is the case that differs — its
 * projection selects flat `has_catalog` / `row_count_stat` columns and the mapper
 * assembles the nested `catalog` object from them, so a fixture supplying `catalog`
 * pre-nested will not round-trip.
 */

import { LIST_REGISTRY } from '../shared/data-sources/duckdb/lists';
import type { MetadataDataSource } from '../shared/data-sources/MetadataDataSource';
import { createFakeDataSource } from '../shared/testing/createFakeDataSource';
import type { AssetSummary, ResourceType } from '../shared/typings/domain/asset';
import type { Page } from '../shared/typings/page';

/** The list envelope the server used to return. */
export interface WireListEnvelope {
  data: Record<string, unknown>[];
  page_info?: {
    total_count?: number | null;
    end_cursor?: string | null;
    // Carried by fixtures captured from the old envelope. Unused: nothing paginates
    // backwards.
    start_cursor?: string | null;
    has_next_page?: boolean;
  };
}

/** Map a wire envelope to a domain page using the type's real mapper. */
export function pageFromWire(
  resourceType: ResourceType,
  envelope: WireListEnvelope,
): Page<AssetSummary> {
  const spec = LIST_REGISTRY[resourceType];
  if (!spec) throw new Error(`no list mapper for ${resourceType}`);
  return {
    items: envelope.data.map((row) => spec.map(row)),
    nextCursor: envelope.page_info?.has_next_page
      ? (envelope.page_info.end_cursor ?? null)
      : null,
    totalCount: envelope.page_info?.total_count ?? envelope.data.length,
  };
}

/**
 * A source serving one resource type's list from a wire envelope.
 *
 * Everything else comes from the fake's `full` mode, so a component that also reads
 * facets or counts still renders.
 */
export function listSource(
  resourceType: ResourceType,
  envelope: WireListEnvelope,
  overrides: Partial<MetadataDataSource> = {},
): MetadataDataSource {
  return createFakeDataSource(
    {
      fetchAssetList: async () => pageFromWire(resourceType, envelope),
      ...overrides,
    } as never,
    { full: true },
  );
}
