import type {
  AssetArgs,
  AssetFilter,
  ColumnLineageArgs,
  FacetsArgs,
  LineageArgs,
  ListArgs,
} from '../typings/args';
import type { Asset, AssetSummary } from '../typings/domain/asset';
import type { Capabilities } from '../typings/domain/capabilities';
import type { AssetCounts } from '../typings/domain/counts';
import type { Distribution } from '../typings/domain/distribution';
import type { Facets } from '../typings/domain/facets';
import type { FileEntry } from '../typings/domain/files';
import type { ColumnLineageResult, LineageGraph } from '../typings/domain/lineage';
import type { ProjectOverview } from '../typings/domain/overview';
import type { Project } from '../typings/domain/project';
import type {
  SearchFacets,
  SearchFilter,
  SearchResult,
} from '../typings/domain/search';
import type { Page } from '../typings/page';

/**
 * Protocol-agnostic contract for fetching metadata. Each consumer (catalog over
 * GraphQL, dbt-docs-v2 over REST) implements one `MetadataDataSource` that
 * translates its API into the shared domain shape. Everything below the
 * provider is backend-agnostic — components and hooks depend on this interface,
 * never on a generated GraphQL or REST type.
 *
 * `fetchAsset` is the only required fetcher; the rest are optional so a source
 * can advertise the surfaces it actually supports. Hooks gate on the presence
 * of a fetcher.
 */
export interface MetadataDataSource {
  /** Stable identity of this source. Used to namespace react-query cache keys
   *  so two sources never collide. */
  readonly id: string;

  /** Fetch a single asset. Returns `null` when not found. */
  fetchAsset(args: AssetArgs): Promise<Asset | null>;

  /** List assets, cursor-paginated. */
  fetchAssetList?(args: ListArgs<AssetFilter>): Promise<Page<AssetSummary>>;

  /** Facet options for a resource type's list filters. */
  fetchFacets?(args: FacetsArgs): Promise<Facets>;

  /** Node-level lineage graph for an asset, optionally depth-capped. */
  fetchLineage?(args: LineageArgs): Promise<LineageGraph>;

  /**
   * Column-level lineage for an asset (optionally one column). Returns a
   * discriminated result so a gated backend (feature unavailable) is distinct
   * from an empty graph — consumers render an upgrade upsell on `'gated'`.
   */
  fetchColumnLineage?(args: ColumnLineageArgs): Promise<ColumnLineageResult>;

  /** Feature capabilities this source/backend supports. */
  fetchCapabilities?(): Promise<Capabilities>;

  /**
   * Build identity of the backend (dbt v2 vs Core, login state). Distinct from
   * {@link fetchCapabilities}, which advertises feature support. A source
   * without a notion of distribution omits this.
   */
  fetchDistribution?(): Promise<Distribution>;

  /**
   * Project-wide per-resource-type asset counts. No-arg aggregate (like
   * {@link fetchDistribution}) — the tally spans every resource table, not a
   * single list. A source without a notion of project-wide counts omits this.
   */
  fetchAssetCounts?(): Promise<AssetCounts>;

  /**
   * Identity of the project this source serves (name, dbt/adapter versions, git
   * state). No-arg aggregate. A source with no notion of a single project (or
   * that exposes it some other way) omits this.
   */
  fetchProject?(): Promise<Project>;

  /**
   * The project's `{% docs __overview__ %}` block — the markdown the docs landing
   * page renders. Resolves `null` when no package defines one, which is the
   * "render the built-in default" signal and is distinct from a rejection. No-arg
   * aggregate. A source with no notion of project docs omits this.
   */
  fetchOverview?(): Promise<ProjectOverview | null>;

  /**
   * Flat list of every file-bearing resource in the project — the rows a file
   * tree is built from. No-arg. A source with no on-disk file view (e.g. a
   * GraphQL catalog) omits this.
   */
  fetchFiles?(): Promise<FileEntry[]>;

  /**
   * Cross-type cursor search. Spans every resource type (not REGISTRY-routed).
   * Returns a discriminated {@link SearchResult} so a structured client error
   * (a stable 400 code the UI surfaces inline) is distinct from an empty page.
   */
  fetchSearch?(args: ListArgs<SearchFilter>): Promise<SearchResult>;

  /**
   * Project-wide distinct facet values for the cross-type search/filter
   * surface. No-arg (global) — consumed independently of search paging.
   */
  fetchSearchFacets?(): Promise<SearchFacets>;

  /**
   * Freshness callbacks. A source (or its wrapper) calls these when it observes
   * that an asset's applied state or definition has a newer timestamp, so cache
   * consumers can invalidate. {@link wrapDataSource} wires these to react-query
   * `invalidateQueries`.
   */
  onAppliedUpdatedAt?(args: AssetArgs, updatedAt: string): void;
  onDefinitionUpdatedAt?(args: AssetArgs, updatedAt: string): void;

  /** Filter fields {@link fetchAssetList} honors. Consumers read this to hide
   *  unsupported filter controls. */
  readonly supportedFilters: ReadonlySet<string>;
}
