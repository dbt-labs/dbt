import type { Page } from '../page';
import type { ResourceType } from './asset';
import type { FacetValue } from './facets';

/** Which field of an asset a search hit matched on. Mirrors the dbt-docs-server
 *  `matched_field` vocabulary. */
export type MatchedField = 'name' | 'column' | 'tag' | 'fqn' | 'description';

/**
 * One cross-type search result. Minimal by design — a hit is not a full
 * {@link AssetSummary}; it carries just what the result list renders, plus the
 * per-edge match metadata (`matchedField`, `highlight`) folded onto the hit.
 * Type-specific render fields are sparse: present only when applicable to the
 * hit's `resourceType`.
 */
export interface SearchHit {
  uniqueId: string;
  resourceType: ResourceType;
  name: string | null;
  packageName: string | null;
  fqn?: string[];
  /** Field this hit matched on; null when the backend didn't say. */
  matchedField: MatchedField | null;
  /** HTML fragment with `<b>…</b>` runs marking matched substrings; null when
   *  absent. */
  highlight: string | null;
  /** model only */
  materialized?: string;
  /** model only */
  access?: string;
  /** source only */
  sourceName?: string;
  /** source only */
  freshnessChecked?: boolean;
  /** test / unit_test only */
  testType?: string;
  /** exposure only */
  exposureType?: string;
  /** runnable types only — last run completed_at. Absent on non-runnable hits. */
  executedAt?: string | null;
}

/**
 * Result of a cross-type search. Discriminated so a structured client error
 * (e.g. a 400 with a stable code the UI surfaces inline) stays distinct from a
 * successful — possibly empty — page. Mirrors the `fetchColumnLineage`
 * `{ kind }` precedent.
 */
export type SearchResult =
  | { kind: 'ok'; page: Page<SearchHit> }
  | { kind: 'error'; code: string; message: string };

/** Cross-type search filter — the subset of facets the global search honors.
 *  Distinct from {@link AssetFilter} (per-type list filters). */
export interface SearchFilter {
  q?: string;
  resourceTypes?: ResourceType[];
  packages?: string[];
  tags?: string[];
  modelingLayers?: string[];
  materializations?: string[];
}

/** Project-wide distinct facet values for the cross-type search/filter surface.
 *  Each list carries a project-wide count per value. */
export interface SearchFacets {
  accesses: FacetValue[];
  modelingLayers: FacetValue[];
  materializationTypes: FacetValue[];
  tags: FacetValue[];
  packages: FacetValue[];
}
