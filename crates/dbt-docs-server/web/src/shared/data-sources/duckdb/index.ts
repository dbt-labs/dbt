/**
 * `MetadataDataSource` over the site's own parquet, no server involved.
 *
 * Implements the same interface `createRestDataSource` does, so components and
 * hooks are untouched — `main.tsx` picks one and everything below the seam follows.
 * Rows are projected with the handlers' snake_case aliases and fed to the existing
 * `fromRest` mappers, which keeps one mapping layer and one set of tests for both
 * sources.
 *
 * **Nothing the shell blocks on touches DuckDB.** `App` gates its first paint on
 * project, capabilities, distribution and the node list, so those come from the
 * hyparquet bootstrap and the injected scalars — a cheap `HEAD` for the capability
 * probe, no 6.8 MB engine, and in particular no 836 KB column-lineage download to
 * answer "is column lineage available". DuckDB is reserved for what the user asks
 * for after paint.
 *
 * Every surface the REST source served is here.
 */

import type { SiteBootstrap } from '../../../lib/siteBootstrap';
import type {
  AssetArgs,
  AssetFilter,
  ColumnLineageArgs,
  FacetsArgs,
  LineageArgs,
  ListArgs,
} from '../../typings/args';
import type { Asset, AssetSummary } from '../../typings/domain/asset';
import type { Capabilities } from '../../typings/domain/capabilities';
import type { AssetCounts } from '../../typings/domain/counts';
import type { Distribution } from '../../typings/domain/distribution';
import type { Facets } from '../../typings/domain/facets';
import type { FileEntry } from '../../typings/domain/files';
import type { ColumnLineageResult, LineageGraph } from '../../typings/domain/lineage';
import type { ProjectOverview } from '../../typings/domain/overview';
import type { Project } from '../../typings/domain/project';
import type {
  SearchFacets,
  SearchFilter,
  SearchResult,
} from '../../typings/domain/search';
import type { Page } from '../../typings/page';
import {
  fromCapabilities,
  fromColumnLineageResponse,
  fromDistribution,
  fromFileList,
  fromLineageResponse,
  fromNodeCounts,
  fromProject,
  fromProjectOverview,
  fromSearchFacets,
  fromSearchResponse,
  type RestColumnLineageEdge,
  type RestFileEntry,
  type RestLineageEdge,
  type RestLineageNode,
  type RestNodeCounts,
  type RestProjectOverview,
} from '../mappers/fromWire';
import type { MetadataDataSource } from '../MetadataDataSource';
import type { BootstrapData } from './bootstrap';
import { detailSpecFor, nodeColumnsSql } from './details';
import { createEngine, type DuckDbEngine } from './engine';
import {
  buildListQuery,
  constantFacets,
  decodeCursor,
  encodeCursor,
  FACET_QUERIES,
  supportedFilterFields,
  toPage,
} from './lists';
import {
  buildSearchQuery,
  DEFAULT_PAGE_SIZE as SEARCH_PAGE_SIZE,
  highlightFor,
  MAX_QUERY_LENGTH,
  SEARCH_FACET_ACCESSES,
  SEARCH_FACET_LAYERS,
  SEARCH_FACET_MATERIALIZATIONS,
  SEARCH_FACET_PACKAGES,
  SEARCH_FACET_TAGS,
  tokenize,
} from './search';
import {
  COLUMN_LINEAGE_TABLES,
  columnLineageSql,
  COUNTS_SQL,
  COUNTS_TABLES,
  FILES_SQL,
  FILES_TABLES,
  LINEAGE_MAX_DEPTH,
  LINEAGE_TABLES,
  lineageEdgesSql,
  lineageNodesSql,
  NODE_EDGES_TABLES,
  nodeEdgesSql,
  normalizeLineageKind,
  OVERVIEW_TABLES,
  overviewSql,
  SAVED_QUERY_TABLES,
  savedQueryDependsOnSql,
} from './sql';

/** The relation whose *rows* mean column-level lineage is available. */
const COLUMN_LINEAGE_VIEW = 'dbt.column_lineage';

/**
 * Deserialize a column the index stores as a JSON string.
 *
 * `meta`, `config` and friends are `VARCHAR` in the parquet, not structured types.
 * The Rust handlers parsed them handler-side and mapped a parse failure to `null`
 * rather than an error (CC-7); doing the same here keeps a malformed value from
 * taking down a detail page, and keeps an escaped JSON string off the domain type.
 */
function parseJsonColumn(value: unknown): Record<string, unknown> | null {
  if (value === null || value === undefined) return null;
  if (typeof value === 'object') return value as Record<string, unknown>;
  if (typeof value !== 'string' || value === '') return null;
  try {
    let parsed: unknown = JSON.parse(value);
    // Some exporter paths double-encode: the VARCHAR cell is itself a JSON
    // string literal wrapping the real JSON text, so one parse yields a
    // string rather than an object. Parse again in that case.
    if (typeof parsed === 'string') {
      parsed = JSON.parse(parsed);
    }
    return typeof parsed === 'object' && parsed !== null
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

export interface DuckDbDataSourceOptions {
  /** Absolute URL of the `data/` directory, with a trailing slash. */
  dataBaseUrl: string;
  /** The injected build scalars. Supplies distribution and the CDN base. */
  bootstrap: SiteBootstrap;
  /**
   * The hyparquet first-paint read, already in flight.
   *
   * Passed as a promise rather than awaited by the caller so `main.tsx` can render
   * the shell's loading state immediately instead of staring at a blank page.
   */
  data: Promise<BootstrapData>;
  /** Injectable for tests. */
  engine?: DuckDbEngine;
}

export function createDuckDbDataSource(
  options: DuckDbDataSourceOptions,
): MetadataDataSource {
  const engine =
    options.engine ??
    createEngine({
      dataBaseUrl: options.dataBaseUrl,
      cdnBase: options.bootstrap.duckdb_cdn_base,
    });

  /** From the bootstrap, not DuckDB: `App` blocks on this. */
  async function fetchProject(): Promise<Project> {
    const { project } = await options.data;
    return fromProject(project ?? { name: '' });
  }

  /** From the injected scalars. No I/O at all. */
  async function fetchDistribution(): Promise<Distribution> {
    return fromDistribution({
      name: options.bootstrap.distribution,
      version: options.bootstrap.dbt_version,
      is_logged_in: options.bootstrap.is_logged_in,
    });
  }

  /**
   * Whether column lineage is actually present, asked once.
   *
   * A row probe, not a `HEAD`. Presence used to be the signal, but the
   * information schema writes *every* table even at zero rows — precisely so
   * `views.sql` always resolves — so the file is always there and a `HEAD` now
   * answers "yes" for every project. Rows are the only honest question.
   *
   * Still derived from the data rather than declared in the bootstrap: a flag
   * would be cheaper but could disagree with the artifacts sitting beside it.
   * The cost is one artifact fetch — a few hundred bytes when the table is empty,
   * and when it is not, the feature is on and the fetch was needed anyway.
   *
   * A failure means the relation is not there at all, which is a broken or
   * partial site; reported as "unavailable" rather than thrown, so the shell
   * still renders.
   */
  let columnLineage: Promise<boolean> | null = null;
  function hasColumnLineage(): Promise<boolean> {
    columnLineage ??= (async () => {
      try {
        const [row] = await engine.query<{ present: boolean }>(
          `SELECT EXISTS(SELECT 1 FROM ${COLUMN_LINEAGE_VIEW}) AS present`,
          [COLUMN_LINEAGE_VIEW],
        );
        return Boolean(row?.present);
      } catch {
        return false;
      }
    })();
    return columnLineage;
  }

  async function fetchCapabilities(): Promise<Capabilities> {
    return fromCapabilities({ has_column_lineage: await hasColumnLineage() });
  }

  async function fetchAssetCounts(): Promise<AssetCounts> {
    const rows = await engine.query<{ resource_type: string; count: number }>(
      COUNTS_SQL,
      COUNTS_TABLES,
    );
    // Fold `unit_test` into `test`, as the Rust handler did after its query: the two
    // render on one page, so the UI counts them together.
    const counts: RestNodeCounts = {};
    for (const row of rows) {
      const key = row.resource_type === 'unit_test' ? 'test' : row.resource_type;
      counts[key] = (counts[key] ?? 0) + Number(row.count ?? 0);
    }
    return fromNodeCounts(counts);
  }

  /**
   * The winning `__overview__` doc block, or null when the project defines none.
   *
   * The root package name comes from the bootstrap, as `fetchProject`'s does —
   * `dbt.project` is not registered in DuckDB, and pulling it in to resolve one
   * string would cost an artifact fetch on the landing page's critical path.
   */
  async function fetchOverview(): Promise<ProjectOverview | null> {
    const { project } = await options.data;
    const [row] = await engine.query<RestProjectOverview>(
      overviewSql(project?.name ?? ''),
      OVERVIEW_TABLES,
    );
    return row ? fromProjectOverview(row) : null;
  }

  async function fetchFiles(): Promise<FileEntry[]> {
    const files = await engine.query<RestFileEntry>(FILES_SQL, FILES_TABLES);
    return fromFileList({ files, total: files.length });
  }

  /**
   * One asset's detail, routed to its own mapper.
   *
   * `args.resourceType` picks the spec; types with no detail of their own
   * (`analysis`, `function`, `operation`) fall back to the generic node path, which
   * is what served them under REST too. Returns `null` for an unknown id, the
   * contract `useAssetDetail` turns into the not-found page.
   */
  async function fetchAsset(args: AssetArgs): Promise<Asset | null> {
    const spec = detailSpecFor(args.resourceType);

    const [detail] = await engine.query<Record<string, unknown>>(
      spec.sql(args.uniqueId),
      spec.tables,
    );
    if (!detail) return null;

    const [columns, edges, extras] = await Promise.all([
      spec.wantsColumns
        ? engine.query<Record<string, unknown>>(nodeColumnsSql(args.uniqueId), [
            'dbt.node_columns',
          ])
        : Promise.resolve([]),
      engine.query<{ direction: string; unique_id: string; edge_type: string }>(
        nodeEdgesSql(args.uniqueId),
        NODE_EDGES_TABLES,
      ),
      Promise.all(
        (spec.extras ?? []).map(
          async (extra) =>
            [
              extra.key,
              await engine.query<Record<string, unknown>>(
                extra.sql(args.uniqueId),
                extra.tables,
              ),
            ] as const,
        ),
      ),
    ]);

    const edgeRefs = (direction: string) =>
      edges
        .filter((e) => e.direction === direction)
        .map((e) => ({ unique_id: e.unique_id, edge_type: e.edge_type }));

    const row: Record<string, unknown> = {
      ...detail,
      columns,
      depends_on: edgeRefs('depends_on'),
      referenced_by: edgeRefs('referenced_by'),
      ...Object.fromEntries(extras),
    };
    for (const key of spec.jsonColumns) {
      row[key] = parseJsonColumn(row[key]);
    }
    // Freshness is flat in the query and nested on the wire shape.
    if (row.status !== undefined) {
      row.freshness = row.status
        ? {
            status: row.status,
            snapshotted_at: row.snapshotted_at ?? null,
            max_loaded_at: row.max_loaded_at ?? null,
          }
        : null;
    }
    // So is the group's owner.
    if (args.resourceType === 'group') {
      const meta = (row.meta ?? {}) as Record<string, unknown>;
      const owner = (meta.owner ?? {}) as Record<string, unknown>;
      row.owner = {
        name: row.owner_name ?? null,
        email: row.owner_email ?? null,
        github: owner.github ?? null,
        slack: owner.slack ?? null,
      };
    }

    return spec.map(row);
  }

  /**
   * Node-level lineage around one asset.
   *
   * `saved_query` roots bypass the recursive walk: they carry their dependencies
   * as a `depends_on_nodes` list rather than as `dbt.edges` rows, so the Rust
   * handler synthesized a one-hop graph for them and this does the same. Resource
   * types there are inferred from the id prefix, which is all the list gives us.
   */
  async function fetchLineage(args: LineageArgs): Promise<LineageGraph> {
    const maxDepth = Math.min(args.depth ?? LINEAGE_MAX_DEPTH, LINEAGE_MAX_DEPTH);

    if (args.uniqueId.startsWith('saved_query.')) {
      return savedQueryLineage(args.uniqueId, maxDepth);
    }

    const [nodes, edges] = await Promise.all([
      engine.query<RestLineageNode>(
        lineageNodesSql(args.uniqueId, maxDepth),
        LINEAGE_TABLES,
      ),
      engine.query<RestLineageEdge>(
        lineageEdgesSql(args.uniqueId, maxDepth),
        LINEAGE_TABLES,
      ),
    ]);

    return fromLineageResponse({
      root: args.uniqueId,
      max_depth: maxDepth,
      nodes,
      edges,
    });
  }

  async function savedQueryLineage(
    uniqueId: string,
    maxDepth: number,
  ): Promise<LineageGraph> {
    const [row] = await engine.query<{ depends_on_nodes: string[] | null }>(
      savedQueryDependsOnSql(uniqueId),
      SAVED_QUERY_TABLES,
    );
    const parents = row?.depends_on_nodes ?? [];

    return fromLineageResponse({
      root: uniqueId,
      max_depth: maxDepth,
      nodes: [
        {
          unique_id: uniqueId,
          name: shortName(uniqueId),
          resource_type: 'saved_query',
          depth: 0,
        },
        ...parents.map((parent) => ({
          unique_id: parent,
          name: shortName(parent),
          resource_type: resourceTypeFromId(parent),
          depth: -1,
        })),
      ],
      edges: parents.map((parent) => ({
        from_id: parent,
        to_id: uniqueId,
        edge_type: 'ref',
      })),
    });
  }

  /**
   * Column-level lineage for one column, single-hop in both directions.
   *
   * The discriminated result matters: `'gated'` is what makes
   * `ColumnLineageView` render the upgrade card, and it must be distinct from a
   * present-but-empty graph. Gating is the artifact's absence, so this asks the
   * engine to register it and checks whether it landed.
   */
  async function fetchColumnLineage(
    args: ColumnLineageArgs,
  ): Promise<ColumnLineageResult> {
    // Gating is decided before the query, by the same probe `fetchCapabilities`
    // uses. Deciding it from a *thrown* query instead — which is what this did —
    // rendered every mistake in this SQL as an upgrade card.
    if (!(await hasColumnLineage())) {
      return { kind: 'gated' };
    }

    // No column filter: the REST contract fetched every edge touching the node
    // and `ColumnLineageView` narrows client-side, so this matches it. An error
    // here is a real failure and propagates.
    const rows = await engine.query<RestColumnLineageEdge & { lineage_kind: string }>(
      columnLineageSql(args.uniqueId),
      COLUMN_LINEAGE_TABLES,
    );

    return {
      kind: 'ok',
      graph: fromColumnLineageResponse({
        root: args.uniqueId,
        edges: rows.map((row) => ({
          from_node: row.from_node,
          from_column: row.from_column,
          to_node: row.to_node,
          to_column: row.to_column,
          kind: normalizeLineageKind(row.lineage_kind),
        })),
      }),
    };
  }

  /**
   * One page of a resource-type list.
   *
   * Throws for a type with no list, matching the REST source — `analysis`,
   * `function` and `operation` had no list endpoint either, and the views that
   * render them use their own path.
   */
  async function fetchAssetList(
    args: ListArgs<AssetFilter>,
  ): Promise<Page<AssetSummary>> {
    const query = buildListQuery(args);
    if (!query) {
      const type = args.filter?.resourceTypes?.[0] ?? 'unknown';
      throw new Error(`no list query for resource type ${type}`);
    }

    const [rows, counted] = await Promise.all([
      engine.query<Record<string, unknown>>(query.sql, query.tables),
      engine.query<{ total: number }>(query.countSql, query.tables),
    ]);

    return toPage(rows, Number(counted[0]?.total ?? rows.length), query);
  }

  /** Filter options for one type's list controls. */
  async function fetchFacets(args: FacetsArgs): Promise<Facets> {
    const facets: Facets = constantFacets(args.resourceType);
    for (const facet of FACET_QUERIES[args.resourceType] ?? []) {
      const rows = await engine.query<{ value: string | null }>(
        facet.sql,
        facet.tables,
      );
      facets[facet.key] = rows
        .filter((row) => row.value !== null)
        // Counts stay null: the handlers did not compute them for these either, and
        // a wrong count beside a filter is worse than no count.
        .map((row) => ({ value: String(row.value), count: null }));
    }
    return facets;
  }

  /**
   * Cross-type search.
   *
   * The discriminated result carries a client error as *data*, matching the REST
   * contract where a 400 was a valid response the UI surfaced inline rather than a
   * thrown failure. An over-long query is the one such error here.
   */
  async function fetchSearch(args: ListArgs<SearchFilter>): Promise<SearchResult> {
    const query = args.filter?.q ?? '';
    if (query.length > MAX_QUERY_LENGTH) {
      return {
        kind: 'error',
        code: 'query_too_long',
        message: `Search queries are limited to ${MAX_QUERY_LENGTH} characters.`,
      };
    }

    const tokens = tokenize(query);
    const built = buildSearchQuery(
      query,
      {
        resourceTypes: args.filter?.resourceTypes,
        packages: args.filter?.packages,
        tags: args.filter?.tags,
      },
      args.limit ?? SEARCH_PAGE_SIZE,
      decodeCursor(args.cursor),
    );
    // No tokens, or a type filter naming nothing searchable: an empty page, not an
    // error.
    if (!built) {
      return { kind: 'ok', page: { items: [], nextCursor: null, totalCount: 0 } };
    }

    const [rows, counted] = await Promise.all([
      engine.query<Record<string, unknown>>(built.sql, built.tables),
      engine.query<{ total: number }>(built.countSql, built.tables),
    ]);
    const total = Number(counted[0]?.total ?? rows.length);
    const nextOffset = built.offset + rows.length;

    const page = fromSearchResponse({
      data: rows.map((row) => ({
        matched_field: (row.matched_field ?? null) as never,
        highlight: highlightFor(row, tokens),
        hit: row as never,
      })),
      page_info: {
        total_count: total,
        start_cursor: null,
        end_cursor: nextOffset < total ? encodeCursor(nextOffset) : null,
        has_next_page: nextOffset < total,
      },
    });
    return { kind: 'ok', page };
  }

  /** Project-wide facet values for the cross-type filter rail. */
  async function fetchSearchFacets(): Promise<SearchFacets> {
    const [accesses, layers, materializations, tags, packages] = await Promise.all([
      engine.query<{ value: string | null; cnt: number }>(SEARCH_FACET_ACCESSES, [
        'dbt.models',
      ]),
      engine.query<{ value: string | null; cnt: number }>(SEARCH_FACET_LAYERS, [
        'dbt.models',
      ]),
      engine.query<{ value: string | null; cnt: number }>(
        SEARCH_FACET_MATERIALIZATIONS,
        ['dbt.models'],
      ),
      engine.query<{ value: string | null; cnt: number }>(SEARCH_FACET_TAGS, [
        'dbt_internal.resources',
        'dbt.exposures',
        'dbt.metrics',
        'dbt.saved_queries',
      ]),
      engine.query<{ value: string | null; cnt: number }>(SEARCH_FACET_PACKAGES, [
        'dbt_internal.resources',
        'dbt.macros',
        'dbt.exposures',
      ]),
    ]);

    const facet = (rows: { value: string | null; cnt: number }[]) =>
      rows
        .filter((r) => r.value !== null)
        .map((r) => ({ value: String(r.value), count: Number(r.cnt) }));

    return fromSearchFacets({
      accesses: facet(accesses),
      modeling_layers: facet(layers),
      materialization_types: facet(materializations),
      tags: facet(tags),
      packages: facet(packages),
    });
  }

  return {
    id: 'duckdb-wasm',
    supportedFilters: supportedFilterFields(),
    fetchAsset,
    fetchCapabilities,
    fetchDistribution,
    fetchAssetCounts,
    fetchProject,
    fetchOverview,
    fetchFiles,
    fetchLineage,
    fetchColumnLineage,
    fetchAssetList,
    fetchFacets,
    fetchSearch,
    fetchSearchFacets,
  };
}

/** `model.pkg.orders` -> `orders`. The lineage list gives no display name. */
function shortName(uniqueId: string): string {
  const parts = uniqueId.split('.');
  return parts[parts.length - 1] ?? uniqueId;
}

/** `model.pkg.orders` -> `model`. Same inference the Rust handler used here. */
function resourceTypeFromId(uniqueId: string): string {
  return uniqueId.split('.')[0] ?? 'model';
}
