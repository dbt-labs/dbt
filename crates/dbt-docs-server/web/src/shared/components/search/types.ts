import type { ResourceTypeExplorer } from '../../../lib/resourceType';
import type { MatchedField } from '../../typings/domain/search';

/**
 * The matched-field vocabulary for a single search hit. Canonical definition
 * lives in the protocol-agnostic domain layer ({@link ../../typings/domain/search});
 * re-exported here for the search components and their existing consumers.
 *
 * Consumers whose source vocabulary differs (e.g. dbt-explorer's GraphQL
 * `SearchFieldType` enum) should map their values into this set before
 * handing the data to {@link SearchResultItem} — typical mappings:
 *   - `'code'`            → `'description'` (same `...{text}...` framing)
 *   - `'columnDescription'` → `'description'`
 *   - `'relation'`        → `'name'` (plain text passthrough)
 */
export type { MatchedField };

/** Minimum hit payload required to render a row. */
export type SearchResultHit = {
  name: string | null;
  uniqueId: string;
  resourceType: ResourceTypeExplorer;
  /**
   * Fully-qualified name path used by the lineage CTA. When present alongside
   * a `getLineageHref` builder, the row renders a "View lineage" link.
   */
  fqn?: string[];
};

/** Envelope for a single search edge. */
export type SearchResultDisplayData = {
  matchedField: MatchedField;
  /** HTML fragment with `<b>...</b>` runs marking matched substrings. */
  highlight: string | null;
  hit: SearchResultHit;
};
