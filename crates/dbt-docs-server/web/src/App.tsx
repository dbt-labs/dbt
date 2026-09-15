import type { ComponentType } from 'react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Route, Routes, useLocation, useNavigate, useParams } from 'react-router-dom';
import { Search as SearchIcon } from 'lucide-react';

import { AnalysisFilterView } from './components/AnalysisFilterView';
import { FullLineagePageV2 } from './components/LineageV2/FullLineagePage';
import { LocatePane, type LocatePaneMode } from './components/LocatePane';
import { MacroFilterView } from './components/MacroFilterView';
import { ModelFilterView } from './components/ModelFilterView';
import { PreviewDrawer } from './components/PreviewDrawer';
import {
  ExposureFilterView,
  GroupFilterView,
  MetricFilterView,
  SavedQueryFilterView,
  SeedFilterView,
  SemanticModelFilterView,
  SnapshotFilterView,
} from './components/SimpleFilterViews';
import { SourceCollectionPage } from './components/SourceCollectionPage';
import { SourceFilterView } from './components/SourceFilterView';
import { TestFilterView } from './components/TestFilterView';
import { Input } from './components/ui/Input';
import { useAllNodes } from './hooks/useAllNodes';
import { deriveUpgradeCapabilities } from './hooks/useCapabilities';
import { useIdentity } from './hooks/useIdentity';
import { useResizable } from './hooks/useResizable';
import { useTheme } from './hooks/useTheme';
import { deriveUserState } from './lib/deriveUserState';
import { inferResourceType, resolveAssetArgs } from './lib/inferResourceType';
import {
  initTelemetry,
  isTelemetryInitialized,
  trackDocsSiteOpened,
  trackResourceViewed,
  useTelemetryInitialized,
} from './lib/telemetry';
import { type View, viewFromPath } from './lib/viewFromPath';
import NotFoundPage from './pages/NotFoundPage';
import Overview from './pages/Overview';
import ResourceDetails from './pages/ResourceDetails';
import ResourceFilter from './pages/ResourceFilter';
import Search from './pages/Search';
import { paths, ROUTES } from './routes';
import {
  type Asset,
  type Project,
  useAssetCounts,
  useAssetDetail,
  useCapabilities,
  useDistribution,
  useFiles,
  useProject,
  type UserState,
  useSearchFacets,
} from './shared';
import { type NodeSummary } from './types';

export type { View };

/** Shared filter state — surfaced via LocatePane Filter mode, applied by
 *  AssetListView. Empty arrays mean "no narrowing"; multiple values are
 *  OR'd within a dimension and AND'd across dimensions. */
export type AssetFilters = {
  resourceType: string[];
  modelingLayer: string[];
  materialization: string[];
  pkg: string[];
  tag: string[];
};
const EMPTY_FILTERS: AssetFilters = {
  resourceType: [],
  modelingLayer: [],
  materialization: [],
  pkg: [],
  tag: [],
};

/** Single switch that hides the peek drawer while we re-evaluate the
 *  secondary preview surface. When false, onPeek navigates to the full
 *  detail page and the PreviewDrawer JSX is skipped — all surrounding state
 *  (previewId, previewCache, ESC handler) is kept intact so flipping this
 *  to true restores the prior behavior. TODO: revisit and restore. */
const SHOW_PEEK_DRAWER = false;

export default function App() {
  const theme = useTheme();
  const locateWidth = useResizable('dbt-docs-v2:locate-pane-w', 320, 220, 480);
  const navigate = useNavigate();
  const location = useLocation();

  // Shell data. project/capabilities/distribution/all-nodes are the blocking
  // set (gate the first paint); files / facets / counts are best-effort and
  // default to empty/null on failure so the chrome still renders.
  const projectQuery = useProject();
  const capabilitiesQuery = useCapabilities();
  const distQuery = useDistribution();
  const filesQuery = useFiles();
  const searchFacetsQuery = useSearchFacets();
  const assetCountsQuery = useAssetCounts();
  const allNodes = useAllNodes();

  // Telemetry consent gate. Resolved once on load from the site bootstrap;
  // `initTelemetry` only runs after it resolves, so no analytics init or
  // network call happens before consent is known. An unreadable bootstrap
  // resolves to consent-denied (see useIdentity), so telemetry never fails open.
  const identityQuery = useIdentity();
  useEffect(() => {
    if (identityQuery.data) initTelemetry(identityQuery.data);
  }, [identityQuery.data]);
  // Re-render the shell once consent resolves so render-time href decoration
  // (outbound `<a>` / learn-more links) reapplies — init flips a module
  // singleton in the effect above with no state change of its own.
  useTelemetryInitialized();

  const project = projectQuery.data ?? null;
  const capabilities = capabilitiesQuery.data ?? null;
  const distInfo = distQuery.data ?? null;
  const files = filesQuery.data ?? [];
  const searchFacets = searchFacetsQuery.data ?? null;
  const assetCounts = assetCountsQuery.data ?? null;
  const nodes = allNodes.nodes;
  const nodeTotal = allNodes.total;

  // Derive the upsell-component user state from the distribution, which separates
  // "build flavor" (`name`) from "auth state" (`is_logged_in`). Stays `null` until
  // it lands so the upsells don't flash through the Core default on first paint.
  const userState = deriveUserState(distInfo);
  const upgradeCapabilities = deriveUpgradeCapabilities(capabilities, distInfo);

  const [search, setSearch] = useState('');
  const [filters, setFilters] = useState<AssetFilters>(() => {
    const initial = viewFromPath(window.location.pathname);
    return {
      ...EMPTY_FILTERS,
      resourceType: initial.kind === 'list' && initial.type ? [initial.type] : [],
    };
  });
  const [previewId, setPreviewId] = useState<string | null>(null);

  const view = useMemo(() => viewFromPath(location.pathname), [location.pathname]);
  const isLineageRoute = location.pathname.startsWith('/lineage');
  const selectedId = view.kind === 'detail' ? view.uniqueId : null;

  // Resolve the selected node's `{ uniqueId, resourceType }` for useAssetDetail:
  // prefer the loaded nodes list's authoritative resource_type, else infer from
  // the id prefix. See resolveAssetArgs.
  const assetArgs = useMemo(
    () => resolveAssetArgs(selectedId, nodes),
    [selectedId, nodes],
  );
  const assetQuery = useAssetDetail(assetArgs);

  // Analytics: `docs_site_opened`, fired once per session. `nodeTotal` streams
  // in via useAllNodes auto-paging, so we wait for both it and `project`
  // rather than firing at init. Guarded by a ref so React re-renders /
  // StrictMode double-invokes don't re-emit.
  const docsSiteOpenedFired = useRef(false);
  useEffect(() => {
    if (docsSiteOpenedFired.current) return;
    if (!isTelemetryInitialized()) return;
    if (!project || nodeTotal == null) return;
    docsSiteOpenedFired.current = true;
    trackDocsSiteOpened({
      dbt_version: project.dbtVersion ?? '',
      project_resource_count: nodeTotal,
    });
  }, [project, nodeTotal, identityQuery.data]);

  // Analytics: `resource_viewed` on detail and list routes. The overview and /search
  // (a list view with no type) emit nothing — search is covered by
  // `search_performed`.
  useEffect(() => {
    if (!isTelemetryInitialized()) return;
    if (view.kind === 'detail') {
      trackResourceViewed({
        resource_type: assetArgs?.resourceType ?? inferResourceType(view.uniqueId),
        view_level: 'detail',
        resource_id: view.uniqueId,
      });
    } else if (view.kind === 'list' && view.type) {
      trackResourceViewed({
        resource_type: view.type,
        view_level: 'list',
        resource_id: '',
      });
    }
    // assetArgs is derived from the same selectedId `view` carries; keying on
    // `view` alone is the intended cardinality (one event per navigation).
  }, [view]);

  const detail = assetQuery.data ?? null;
  const detailLoading = assetQuery.isPending;
  const detailNotFound = !assetQuery.isPending && assetQuery.data === null;
  const detailFetchError = assetQuery.isError ? assetQuery.error : null;

  // Peek-drawer detail. Resolved the same way as the selected detail.
  const previewArgs = useMemo(
    () => resolveAssetArgs(previewId, nodes),
    [previewId, nodes],
  );
  const previewQuery = useAssetDetail(previewArgs);
  const previewDetail = previewQuery.data ?? null;

  // Blocking shell error: any of the critical queries failing, or a non-404
  // detail failure (promoted to the global banner, matching prior behavior).
  const error =
    projectQuery.error?.message ??
    capabilitiesQuery.error?.message ??
    distQuery.error?.message ??
    allNodes.error?.message ??
    (detailFetchError && !detailNotFound ? detailFetchError.message : null) ??
    null;

  // URL → filters sync: when the route's `:resourceType` param changes
  // (e.g. back/forward, Asset-tab click), mirror it into filters.resourceType
  // so AssetListView's multi-select narrowing stays consistent with the URL.
  useEffect(() => {
    const urlType = view.kind === 'list' && view.type ? view.type : null;
    setFilters((prev) => {
      if (
        urlType &&
        (prev.resourceType.length !== 1 || prev.resourceType[0] !== urlType)
      ) {
        return { ...prev, resourceType: [urlType] };
      }
      if (!urlType && view.kind !== 'list' && prev.resourceType.length > 0) {
        return { ...prev, resourceType: [] };
      }
      return prev;
    });
  }, [view]);

  // LocatePane mode (Assets/Files/Filter): URL-derived by default (back/
  // forward, direct links, and a fresh landing on /search all resolve here),
  // but picking a tab is a left-panel toggle, not a page — it only flips
  // `modeOverride` and never moves the user off whatever's on screen. Any
  // real navigation (a new pathname, from an actual drill-down click) clears
  // the override so the next page's own URL state takes over again.
  const urlMode: LocatePaneMode = useMemo(() => {
    if (location.pathname === paths.search()) return 'filter';
    const v = new URLSearchParams(location.search).get('view');
    return v === 'files' ? 'files' : 'assets';
  }, [location.pathname, location.search]);
  const [modeOverride, setModeOverride] = useState<LocatePaneMode | null>(null);
  useEffect(() => {
    setModeOverride(null);
  }, [location.pathname]);
  const mode: LocatePaneMode = modeOverride ?? urlMode;

  const onSelectMode = useCallback(
    (next: LocatePaneMode) => {
      setModeOverride(next === urlMode ? null : next);
    },
    [urlMode],
  );

  // Preserve the current LocatePane mode across navigations (e.g. opening a
  // node while browsing Files keeps the next page on Files too). Filter has
  // no per-node scope, so it falls back to Assets rather than carrying
  // forward — mirrors the old URL-contract default.
  const viewParam = useCallback((): 'assets' | 'files' => {
    return mode === 'files' ? 'files' : 'assets';
  }, [mode]);

  const onSelect = useCallback(
    (id: string) => {
      navigate(`${paths.details(id)}?view=${viewParam()}`);
      // Drilling into the full asset page closes the peek drawer — we
      // don't want the same asset open in two surfaces at once.
      setPreviewId(null);
    },
    [navigate, viewParam],
  );

  const onShowList = useCallback(
    (type: string | null) => {
      navigate(type ? paths.resource(type) : paths.search());
    },
    [navigate],
  );

  // LocatePane's project-root row (Assets or Files tab) navigates here.
  const onShowProject = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    navigate(`${paths.home()}?view=${viewParam()}`);
  }, [navigate, viewParam]);

  // Filter changes narrow whatever's on screen; they only navigate when the
  // current page can no longer represent the result:
  //  - a free-text search is active: only /search/ renders `query`, so any
  //    filter tweak has to stay there or the typed text would be silently
  //    dropped.
  //  - otherwise, when the asset-type filter narrows to exactly one value,
  //    mirror it into the URL so the link is shareable; 0 or 2+ values land
  //    on /search/, which owns the multi-select narrowing.
  const onSetFilters = useCallback(
    (next: AssetFilters) => {
      setFilters(next);
      if (search.trim()) {
        if (location.pathname !== paths.search()) navigate(paths.search());
        return;
      }
      const nextType = next.resourceType.length === 1 ? next.resourceType[0] : null;
      const currentType = view.kind === 'list' && view.type ? view.type : null;
      if (nextType !== currentType) {
        navigate(nextType ? paths.resource(nextType) : paths.search());
      } else if (view.kind !== 'list') {
        navigate(paths.search());
      }
    },
    [navigate, view, search, location.pathname],
  );

  const onSubmitTopbarSearch = useCallback(() => {
    if (location.pathname !== paths.search()) navigate(paths.search());
  }, [navigate, location.pathname]);

  // Top-left dbt logo → home reset (clear filters + search, return to home).
  const onResetHome = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    setSearch('');
    setPreviewId(null);
    navigate(paths.home());
  }, [navigate]);

  /** Open the peek drawer for a node. Lazy-fetches detail and caches it.
   *  TEMP: peek drawer is hidden while we re-evaluate the secondary preview
   *  surface — clicking an asset now opens the full detail page instead. The
   *  setPreviewId / previewCache / PreviewDrawer code is kept intact so we
   *  can flip it back on once the new design lands. TODO: revisit and restore. */
  const onPeek = useCallback(
    (id: string) => {
      navigate(`${paths.details(id)}?view=${viewParam()}`);
    },
    [navigate, viewParam],
  );

  const onClosePeek = useCallback(() => setPreviewId(null), []);

  // ESC closes the drawer.
  useEffect(() => {
    if (!previewId) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setPreviewId(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [previewId]);

  if (error) {
    return (
      <div className="app">
        <Topbar project={null} search="" onSearch={() => {}} />
        <div style={{ padding: 32 }}>
          <div className="err">
            <strong>Server error</strong>
            <pre style={{ margin: '8px 0 0', whiteSpace: 'pre-wrap' }}>{error}</pre>
          </div>
        </div>
      </div>
    );
  }

  if (!project || !capabilities || !distInfo || !nodes) {
    return (
      <div className="app">
        <Topbar project={null} search="" onSearch={() => {}} />
        <div className="muted" style={{ padding: 32 }}>
          Loading…
        </div>
      </div>
    );
  }

  if (isLineageRoute) {
    return (
      <Routes>
        <Route path={ROUTES.lineage} element={<FullLineagePageV2 />} />
      </Routes>
    );
  }

  return (
    <div className="app">
      <Topbar
        project={project}
        search={search}
        onSearch={setSearch}
        onResetHome={onResetHome}
        onSubmitSearch={onSubmitTopbarSearch}
      />
      <div
        className={`shell ${previewId ? 'has-preview' : ''}`}
        style={{ '--locate-pane-w': `${locateWidth.width}px` } as React.CSSProperties}
      >
        <LocatePane
          project={project}
          nodes={nodes}
          files={files}
          selectedId={selectedId}
          previewId={previewId}
          isListView={view.kind === 'list'}
          onPeek={onPeek}
          onSelect={onSelect}
          onShowList={onShowList}
          onShowProject={onShowProject}
          isHome={view.kind === 'home'}
          query={search}
          userState={userState}
          loadingProgress={
            nodeTotal != null ? { loaded: nodes.length, total: nodeTotal } : null
          }
          theme={theme.theme}
          onSetTheme={theme.setTheme}
          filters={filters}
          onSetFilters={onSetFilters}
          mode={mode}
          onSelectMode={onSelectMode}
          searchFacets={searchFacets}
          assetCounts={assetCounts}
        />
        <div
          className="shell__resize"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize side panel"
          onPointerDown={locateWidth.startDrag}
        />
        <main className="main">
          <Routes>
            <Route path={ROUTES.home} element={<Overview />} />
            <Route
              path={ROUTES.details}
              element={
                <DetailsRoute
                  detail={detail}
                  detailLoading={detailLoading}
                  detailNotFound={detailNotFound}
                  onSelect={onSelect}
                  hasColumnLineage={upgradeCapabilities?.hasCll ?? false}
                  userState={userState}
                />
              }
            />
            <Route
              path={ROUTES.sourceCollection}
              element={<SourceCollectionPage nodes={nodes ?? []} onSelect={onSelect} />}
            />
            <Route
              path={ROUTES.resource}
              element={
                <ResourceRoute
                  project={project}
                  nodes={nodes}
                  query={search}
                  filters={filters}
                  previewId={previewId}
                  onPeek={onPeek}
                />
              }
            />
            <Route
              path={ROUTES.search}
              element={
                <Search
                  project={project}
                  nodes={nodes}
                  query={search}
                  filters={filters}
                  onSetFilters={onSetFilters}
                  previewId={previewId}
                  onPeek={onPeek}
                />
              }
            />
            <Route path={ROUTES.notFound} element={<NotFoundPage />} />
          </Routes>
        </main>
        {/* Peek drawer is hidden — onPeek now navigates to the full detail
            page (see useCallback above). Keeping the markup, state wiring,
            and PreviewDrawer import in place so we can flip this back on
            once the secondary-preview UX is revisited. TODO: restore. */}
        {SHOW_PEEK_DRAWER && previewId && (
          <PreviewDrawer
            project={project}
            previewId={previewId}
            summary={nodes.find((n) => n.unique_id === previewId) ?? null}
            detail={previewDetail}
            onClose={onClosePeek}
            onOpenFull={onSelect}
          />
        )}
      </div>
    </div>
  );
}

type FilterViewProps = { project: Project; onPeek(uniqueId: string): void };

const FILTER_VIEWS: Partial<Record<string, ComponentType<FilterViewProps>>> = {
  model: ModelFilterView,
  seed: SeedFilterView,
  test: TestFilterView,
  metric: MetricFilterView,
  snapshot: SnapshotFilterView,
  exposure: ExposureFilterView,
  analysis: AnalysisFilterView,
  macro: MacroFilterView,
  group: GroupFilterView,
  semantic_model: SemanticModelFilterView,
  saved_query: SavedQueryFilterView,
};

/** Routes `:resourceType` to a typed FilterView; source is handled separately
 *  (no onPeek); unknown types fall back to the client-side filtered list. */
function ResourceRoute({
  project,
  nodes,
  query,
  filters,
  previewId,
  onPeek,
}: {
  project: Project;
  nodes: NodeSummary[];
  query: string;
  filters: AssetFilters;
  previewId: string | null;
  onPeek(uniqueId: string): void;
}) {
  const { resourceType } = useParams<{ resourceType: string }>();
  if (resourceType === 'source') return <SourceFilterView project={project} />;
  const View = resourceType ? (FILTER_VIEWS[resourceType] ?? null) : null;
  if (View) return <View project={project} onPeek={onPeek} />;
  return (
    <ResourceFilter
      project={project}
      nodes={nodes}
      query={query}
      filters={filters}
      previewId={previewId}
      onPeek={onPeek}
    />
  );
}

/** Bridges between the route param (`:dbtUniqueId`) and the parent's detail
 *  cache. Parent already drives the fetch off `selectedId` derived from the
 *  pathname, so this component just renders. */
function DetailsRoute({
  detail,
  detailLoading,
  detailNotFound,
  onSelect,
  hasColumnLineage,
  userState,
}: {
  detail: Asset | null;
  detailLoading: boolean;
  detailNotFound: boolean;
  onSelect: (id: string) => void;
  hasColumnLineage: boolean;
  userState: UserState | null;
}) {
  useParams<{ dbtUniqueId: string }>();
  return (
    <ResourceDetails
      detail={detail}
      detailLoading={detailLoading}
      detailNotFound={detailNotFound}
      onSelect={onSelect}
      hasColumnLineage={hasColumnLineage}
      userState={userState}
    />
  );
}

function Topbar({
  project,
  search,
  onSearch,
  onResetHome,
  onSubmitSearch,
}: {
  project: Project | null;
  search: string;
  onSearch: (v: string) => void;
  onResetHome?: () => void;
  onSubmitSearch?: () => void;
}) {
  return (
    <header className="topbar-v2">
      <div className="topbar-v2__left">
        <div className="topbar-v2__brand">
          {project && (
            <button
              type="button"
              className="topbar-v2__brand-btn topbar-v2__brand-btn--text"
              onClick={onResetHome}
              aria-label="Overview — reset view"
              title="Overview — reset view"
            >
              <div className="topbar-v2__brand-text">
                <div className="topbar-v2__brand-name">{project.name}</div>
                <div className="topbar-v2__brand-sub">
                  {project.adapterType ?? ''}
                  {project.dbtVersion ? ` · v${project.dbtVersion}` : ''}
                </div>
              </div>
            </button>
          )}
        </div>
      </div>
      <Input
        type="search"
        startIcon={{ icon: <SearchIcon className="size-3" /> }}
        placeholder="Search models, sources, tests, metrics…"
        value={search}
        onChange={(e) => onSearch(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault();
            onSubmitSearch?.();
          }
        }}
        aria-label={project ? `Search ${project.name}` : 'Search project'}
        className="w-full"
      />
    </header>
  );
}
