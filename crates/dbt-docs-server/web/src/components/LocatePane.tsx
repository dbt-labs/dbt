import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { Globe, Moon, Sun, SunMoon } from 'lucide-react';

import type { AssetFilters } from '../App';
import { FEATURE_FLAGS } from '../lib/featureFlags';
import { buildFileTreeItems } from '../lib/fileTree';
import { decorateOutboundHref } from '../lib/outboundReferrer';
import {
  inferModelingLayer,
  RESOURCE_TYPE_LABEL,
  RESOURCE_TYPE_ORDER,
  ResourceTypeIcon,
} from '../lib/resourceType';
import { handleUpsellEvent } from '../lib/upsellAnalytics';
import type { FileEntry, Project } from '../shared';
import {
  type AssetCounts,
  getRailKindsForUserState,
  type SearchFacets,
  UpgradeRailStack,
  type UserState,
} from '../shared';
import type { NodeSummary } from '../types';
import { SEARCHABLE_RESOURCE_TYPES } from '../types';
import { type FileTreeItemType, PaginatedFileTree } from './ui/PaginatedFileTree';
import { SegmentedButton } from './ui/SegmentedButton';

export type LocatePaneMode = 'assets' | 'files' | 'filter';

/** Top-level id for the Asset tab's "All assets" row. Sourdough hardcodes
 *  `'root'` as the synthetic-parent sentinel in its treeWalker, so anything
 *  intended to be a top-level row in any FileTree instance must declare
 *  `parent: 'root'`. The id namespace doesn't collide with Tree-tab ids
 *  (which start with the project name) so we can share `openDirectories`
 *  state across tabs without filtering. */
const ASSET_ALL_PATH = 'all-assets';

/** Rows shown by default in Filter mode; the rest sit behind a "More" toggle
 *  so the rail's first paint stays digestible. */
const PKG_VISIBLE_DEFAULT = 8;
const TAG_VISIBLE_DEFAULT = 8;

interface Props {
  project: Project;
  nodes: NodeSummary[];
  /** File rows for the on-disk Tree tab. One per file-bearing resource
   *  across every parquet table (models, sources, macros, exposures,
   *  metrics, semantic_models, groups, unit_tests, docs, saved_queries). */
  files: FileEntry[];
  selectedId: string | null;
  /** Currently previewed node (for highlight). */
  previewId?: string | null;
  /** Whether the main area is currently rendering the asset list. */
  isListView?: boolean;
  /** Single-click on a node row opens the peek drawer. */
  onPeek(uniqueId: string): void;
  /** Full navigation to the asset detail view — used by the Tree tab so
   *  "open the file" feels like opening the asset, not just previewing it. */
  onSelect(uniqueId: string): void;
  onShowList(type: string | null): void;
  /** Navigate to the project home (catalog landing). LocatePane's
   *  project-root row in Asset mode triggers this. */
  onShowProject(): void;
  /** True when the user is on `/`. Drives the project-row highlight. */
  isHome: boolean;
  query: string;
  loadingProgress?: { loaded: number; total: number } | null;
  theme: 'dark' | 'light' | 'system';
  onSetTheme(theme: 'dark' | 'light' | 'system'): void;
  filters: AssetFilters;
  onSetFilters(next: AssetFilters): void;
  /** URL-driven mode: `/search` → 'filter'; elsewhere `?view=assets|files`,
   *  default 'assets'. Selecting 'filter' doesn't navigate — see App.tsx's
   *  `filterModeOverride`. */
  mode: LocatePaneMode;
  /** Tab click. Owner handles URL navigation (and the /search ↔ project-home
   *  transitions). */
  onSelectMode(next: LocatePaneMode): void;
  /** Project-wide facet values from the metadata adapter (`fetchSearchFacets`).
   *  Null while the request is in flight; the filter pane falls back to deriving
   *  values client-side from `nodes`. */
  searchFacets?: SearchFacets | null;
  /** Project-wide per-resource-type counts from the metadata adapter
   *  (`fetchAssetCounts`). Authoritative across every resource table (macros,
   *  exposures, metrics, semantic_models, saved_queries, groups, …) — `nodes`
   *  alone only holds the resource-table types so per-type tallies derived from it
   *  under-count everything else. Null while the request is in flight. */
  assetCounts?: AssetCounts | null;
  /** Drives the rail upsell stack. Null while capabilities are loading —
   *  the rail collapses for that window. */
  userState: UserState | null;
}

export function LocatePane({
  project,
  nodes,
  files,
  selectedId,
  previewId,
  isListView,
  onPeek,
  onSelect,
  onShowList,
  onShowProject,
  isHome,
  query,
  loadingProgress,
  theme,
  onSetTheme,
  filters,
  onSetFilters,
  mode,
  onSelectMode,
  searchFacets,
  assetCounts,
  userState,
}: Props) {
  // Tree- and Asset-tab expand/collapse live here (not in their child
  // components) so switching tabs doesn't unmount the state. Default-open the
  // synthetic project root (Tree tab) and the "All assets" parent (Asset tab).
  const [openDirectories, setOpenDirectories] = useState<string[]>([
    project.name,
    ASSET_ALL_PATH,
  ]);
  // The Asset-tab single-row highlight follows `filters.resourceType` when it
  // narrows to one type. With zero or multiple selected, no single row owns
  // the highlight — "All assets" lights up only when the list view itself is
  // active *and* nothing else is selected.
  const typeFilter = filters.resourceType.length === 1 ? filters.resourceType[0] : null;

  // Query-filtered set used to compute per-type counts. Crucially this
  // does *not* apply `resourceType` — the type dimension's counts must be
  // independent of which types are currently selected, otherwise clicking
  // "Models" would collapse every other type's count to 0.
  const queryFiltered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return nodes;
    return nodes.filter(
      (n) =>
        n.name.toLowerCase().includes(needle) ||
        n.unique_id.toLowerCase().includes(needle),
    );
  }, [nodes, query]);

  const filtered = useMemo(() => {
    const rt = filters.resourceType;
    if (rt.length === 0) return queryFiltered;
    return queryFiltered.filter((n) => rt.includes(n.resource_type));
  }, [queryFiltered, filters.resourceType]);

  // Project-wide per-type counts from the metadata adapter
  // (`fetchAssetCounts`). The locally-
  // loaded `nodes` array only carries the resource-table types
  // (model/source/seed/snapshot/test/analysis); macros and the SL artifacts
  // live in separate tables and would otherwise read as 0. Both the Asset
  // tab and the Filter tab's Type dimension read these as the canonical,
  // unfiltered project-wide overview — counts do not narrow as other facets
  // are toggled.
  const typeCounts = useMemo(() => {
    if (assetCounts) return new Map(Object.entries(assetCounts));
    const m = new Map<string, number>();
    for (const n of nodes) m.set(n.resource_type, (m.get(n.resource_type) ?? 0) + 1);
    return m;
  }, [nodes, assetCounts]);

  const packageGroups = useMemo(() => {
    const m = new Map<string, NodeSummary[]>();
    for (const n of filtered) {
      const pkg = n.package_name ?? '(no package)';
      const arr = m.get(pkg) ?? [];
      arr.push(n);
      m.set(pkg, arr);
    }
    return [...m.entries()].sort(([a], [b]) => {
      const aSelf = a === project.name ? 0 : 1;
      const bSelf = b === project.name ? 0 : 1;
      if (aSelf !== bSelf) return aSelf - bSelf;
      return a.localeCompare(b);
    });
  }, [filtered, project]);

  // Hide analysis files from the File tree until FEATURE_FLAGS.hasAnalysis flips on.
  const visibleFiles = useMemo(
    () =>
      FEATURE_FLAGS.hasAnalysis
        ? files
        : files.filter((f) => f.resourceType !== 'analysis'),
    [files],
  );

  // When the user is on an asset detail page, highlight that asset's type row
  // in Asset mode — gives the tree a sense of "where you are" even though the
  // detail surface lives outside the type-list view. Resource type is parsed
  // from the unique_id (`<type>.<package>.<name>`) rather than looked up in
  // `nodes`, because macros/sources/etc. live in their own endpoints and
  // never appear in the node list.
  const selectedAssetType = useMemo(() => {
    if (isListView || !selectedId) return null;
    const type = selectedId.split('.')[0];
    return type && (RESOURCE_TYPE_ORDER as readonly string[]).includes(type)
      ? type
      : null;
  }, [isListView, selectedId]);

  return (
    <aside className="locate-pane">
      <div className="locate-pane__tabs">
        <SegmentedButton
          segments={[
            { label: 'Assets', value: 'assets' },
            { label: 'Files', value: 'files' },
            { label: 'Filter', value: 'filter' },
          ]}
          selectedValue={mode}
          onSelect={(v) => onSelectMode(v as LocatePaneMode)}
          size="sm"
          variant="stretch"
        />
      </div>
      <div className="locate-pane__body">
        {mode === 'assets' && (
          <AssetMode
            projectName={project.name}
            nodes={nodes}
            packageGroups={packageGroups}
            typeCounts={typeCounts}
            typeFilter={typeFilter}
            selectedAssetType={selectedAssetType}
            isListView={isListView ?? false}
            isHome={isHome}
            onShowList={onShowList}
            onShowProject={onShowProject}
            openDirectories={openDirectories}
            setOpenDirectories={setOpenDirectories}
          />
        )}
        {mode === 'files' && (
          <TreeMode
            files={visibleFiles}
            projectName={project.name}
            selectedId={previewId ?? selectedId}
            onSelect={onSelect}
            onShowProject={onShowProject}
            openDirectories={openDirectories}
            setOpenDirectories={setOpenDirectories}
          />
        )}
        {mode === 'filter' && (
          <FilterMode
            project={project}
            nodes={nodes}
            typeCounts={typeCounts}
            filters={filters}
            onSetFilters={onSetFilters}
            searchFacets={searchFacets ?? null}
          />
        )}
      </div>

      {/* Side-panel upsell rail — only upsells Mesh (cross-project
       *  collaboration). Mesh is hidden for via-catalog, so the rail renders
       *  nothing there. Collapses while userState is null (capabilities
       *  loading). */}
      {userState && (
        <UpgradeRailStack
          kinds={getRailKindsForUserState(userState)}
          userState={userState}
          onUpsellEvent={handleUpsellEvent}
          decorateOutboundHref={decorateOutboundHref}
          location="rail"
          className="locate-pane__connect-hook"
        />
      )}

      {/* Footer — theme toggle */}
      <footer className="locate-pane__footer">
        <div className="flex w-full justify-center">
          <SegmentedButton
            segments={[
              {
                label: 'Light',
                value: 'light',
                startIcon: <Sun className="size-3" />,
              },
              { label: 'Dark', value: 'dark', startIcon: <Moon className="size-3" /> },
              {
                label: 'System',
                value: 'system',
                startIcon: <SunMoon className="size-3" />,
              },
            ]}
            selectedValue={theme}
            onSelect={(v) => onSetTheme(v as 'dark' | 'light' | 'system')}
            size="sm"
          />
        </div>
      </footer>
    </aside>
  );
}

/* ---------- Asset mode ---------- */

/** Project root + per-type counters rendered through the same sourdough
 *  `PaginatedFileTree` the Tree tab uses, so both tabs share one row visual.
 *  Models as a flat list under a synthetic root:
 *
 *      <project name> (directory)        → onShowProject() → /
 *      ├─ Models  (file, info=count)     → onShowList('model')
 *      ├─ Tests   (file)                 → onShowList('test')
 *      └─ …
 *
 *  Folder-click navigates without collapsing (first click); the caret still
 *  toggles. The detail endpoint covers every `RESOURCE_TYPE_ORDER` type that
 *  appears in the resource tables, but list views work for all of them — so no
 *  click-gate is needed here. */
function AssetMode({
  projectName,
  nodes,
  packageGroups,
  typeCounts,
  typeFilter,
  selectedAssetType,
  isListView,
  isHome,
  onShowList,
  onShowProject,
  openDirectories,
  setOpenDirectories,
}: {
  projectName: string;
  nodes: NodeSummary[];
  packageGroups: [string, NodeSummary[]][];
  typeCounts: Map<string, number>;
  typeFilter: string | null;
  selectedAssetType: string | null;
  isListView: boolean;
  isHome: boolean;
  onShowList: (type: string | null) => void;
  onShowProject: () => void;
  openDirectories: string[];
  setOpenDirectories: React.Dispatch<React.SetStateAction<string[]>>;
}) {
  // Analysis is hidden across discovery surfaces while
  // `FEATURE_FLAGS.hasAnalysis` is off; flip the flag to re-enable.
  const orderedTypes = useMemo(
    () =>
      RESOURCE_TYPE_ORDER.filter((t) => FEATURE_FLAGS.hasAnalysis || t !== 'analysis'),
    [],
  );
  const rootLabel = projectName.trim() || 'Project';

  const items = useMemo<FileTreeItemType[]>(() => {
    const list: FileTreeItemType[] = [
      {
        id: ASSET_ALL_PATH,
        parent: 'root',
        data: {
          pathType: 'directory',
          name: rootLabel,
          iconOverride: {
            icon: <Globe className="size-3 shrink-0" />,
            label: rootLabel,
          },
          info: {
            text: [...typeCounts.values()].reduce((s, n) => s + n, 0).toLocaleString(),
          },
        },
      },
    ];
    for (const t of orderedTypes) {
      list.push({
        id: `${ASSET_ALL_PATH}/${t}`,
        parent: ASSET_ALL_PATH,
        data: {
          pathType: 'file',
          name: RESOURCE_TYPE_LABEL[t] ?? t,
          iconOverride: {
            icon: <ResourceTypeIcon type={t} className="size-3 shrink-0" />,
            label: RESOURCE_TYPE_LABEL[t] ?? t,
          },
          info: { text: (typeCounts.get(t) ?? 0).toLocaleString() },
        },
      });
    }
    return list;
  }, [orderedTypes, typeCounts, rootLabel]);

  const setOpenDirectoriesAdapter = useCallback(
    (fn: (dirs: string[] | undefined) => string[] | undefined) => {
      setOpenDirectories((prev) => fn(prev) ?? []);
    },
    [setOpenDirectories],
  );

  const onFileSelect = useCallback(
    (relativePath: string) => {
      const t = relativePath.slice(ASSET_ALL_PATH.length + 1);
      if (!t) return;
      onShowList(t);
    },
    [onShowList],
  );

  // First click on the project row navigates home (no toggle). Second click
  // (when already on home) collapses. Caret always toggles. This requires
  // `enableCloseFolderOnSecondClick` + `selectedFolder`.
  const onFolderSelect = useCallback(
    (relativePath: string) => {
      if (relativePath === ASSET_ALL_PATH) onShowProject();
    },
    [onShowProject],
  );

  const onSort = useCallback((a: string, b: string) => {
    const prefix = `${ASSET_ALL_PATH}/`;
    const aType = a.startsWith(prefix) ? a.slice(prefix.length) : null;
    const bType = b.startsWith(prefix) ? b.slice(prefix.length) : null;
    if (aType !== null && bType !== null) {
      const ai = (RESOURCE_TYPE_ORDER as readonly string[]).indexOf(aType);
      const bi = (RESOURCE_TYPE_ORDER as readonly string[]).indexOf(bType);
      return ai - bi;
    }
    return a.localeCompare(b);
  }, []);

  // Highlight the matching type row when the user has navigated into the
  // typed list view, or when they're viewing an asset whose type we can
  // resolve. Project-root row lights up only on home.
  const highlightType = isListView ? typeFilter : selectedAssetType;
  const selectedFile =
    highlightType !== null ? `${ASSET_ALL_PATH}/${highlightType}` : undefined;
  const selectedFolder = isHome ? ASSET_ALL_PATH : undefined;

  return (
    <div className="locate-pane__asset">
      <PaginatedFileTree
        items={items}
        rootNodeName="root"
        openDirectories={openDirectories}
        setOpenDirectories={setOpenDirectoriesAdapter}
        onFileSelect={onFileSelect}
        onFolderSelect={onFolderSelect}
        selectedFile={selectedFile}
        selectedFolder={selectedFolder}
        enableCloseFolderOnSecondClick
        onSort={onSort}
      />

      {packageGroups.length > 1 && (
        <p className="locate-pane__hint">
          {packageGroups.length} packages installed — switch to Files tab for
          per-package browsing
        </p>
      )}
    </div>
  );
}

/* ---------- Tree mode ---------- */

/** On-disk view of `<package>/<original_file_path>` for every file-bearing
 *  resource. Backed by sourdough's `PaginatedFileTree`: virtualized,
 *  keyboard-navigable, ARIA-tree. YAML files (`schema.yml`, `_models.yml`)
 *  are folders that expand to reveal the resources defined inline. */
function TreeMode({
  files,
  projectName,
  selectedId,
  onSelect,
  onShowProject,
  openDirectories,
  setOpenDirectories,
}: {
  files: FileEntry[];
  projectName: string;
  selectedId: string | null;
  onSelect: (uniqueId: string) => void;
  onShowProject: () => void;
  openDirectories: string[];
  setOpenDirectories: React.Dispatch<React.SetStateAction<string[]>>;
}) {
  // Build the full item set once per file-list change, then index it by
  // parent path. `items` (the prop we hand to sourdough) is derived
  // reactively from `openDirectories` so it only ever contains the root
  // plus rows under currently-open folders — sourdough's treeWalker /
  // useFileTreeHeight / generateLoadingItems are all O(items × …) per
  // render, and with ~6k items the full set turns each toggle into
  // ~200ms of jank. Lazy expansion is the canonical sourdough pattern
  // (see `loadChildren` / `PaginatedFileTree` docs).
  const treeIndex = useMemo(() => {
    const built = buildFileTreeItems(files, projectName);
    const childrenByParent = new Map<string, FileTreeItemType[]>();
    let rootItem: FileTreeItemType | undefined;
    for (const item of built.items) {
      if (item.parent === 'root') {
        rootItem = item;
        continue;
      }
      const arr = childrenByParent.get(item.parent);
      if (arr) arr.push(item);
      else childrenByParent.set(item.parent, [item]);
    }
    const uniqueIdToPath = new Map<string, string>();
    for (const [path, id] of built.pathToUniqueId) uniqueIdToPath.set(id, path);
    return {
      rootItem,
      childrenByParent,
      pathToUniqueId: built.pathToUniqueId,
      uniqueIdToPath,
    };
  }, [files, projectName]);

  const items = useMemo<FileTreeItemType[]>(() => {
    if (!treeIndex.rootItem) return [];
    const open = new Set(openDirectories);
    const out: FileTreeItemType[] = [treeIndex.rootItem];
    const queue: string[] = [treeIndex.rootItem.id];
    while (queue.length > 0) {
      const path = queue.shift() as string;
      if (!open.has(path)) continue;
      const children = treeIndex.childrenByParent.get(path);
      if (!children) continue;
      for (const child of children) {
        out.push(child);
        if (child.data?.pathType === 'directory') queue.push(child.id);
      }
    }
    return out;
  }, [treeIndex, openDirectories]);

  // PaginatedFileTree calls `setOpenDirectories` with a reducer-style fn that
  // can return undefined. Coerce undefined→[] before lifting into state.
  const setOpenDirectoriesAdapter = useCallback(
    (fn: (dirs: string[] | undefined) => string[] | undefined) => {
      setOpenDirectories((prev) => fn(prev) ?? []);
    },
    [setOpenDirectories],
  );

  const onFileSelect = useCallback(
    (relativePath: string) => {
      if (relativePath === projectName) {
        onShowProject();
        return;
      }
      const uniqueId = treeIndex.pathToUniqueId.get(relativePath);
      if (!uniqueId) return;
      onSelect(uniqueId);
    },
    [treeIndex, onSelect, onShowProject, projectName],
  );

  const selectedFile = selectedId
    ? treeIndex.uniqueIdToPath.get(selectedId)
    : undefined;

  // sourdough's PaginatedFileTree caps its container at maxHeight (default
  // 600px), which leaves a slab of whitespace in the locate-pane body when
  // the tree is deeply expanded. Measure the *parent* (the scrolling body
  // container) so the tree can grow to fill the pane — virtualized scroll
  // takes over beyond that. We can't measure the wrapper itself because its
  // height collapses to the tree's own content height (feedback loop).
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const [maxHeight, setMaxHeight] = useState<number>(600);
  useEffect(() => {
    const parent = wrapRef.current?.parentElement;
    if (!parent) return;
    const ro = new ResizeObserver(([entry]) => {
      const h = entry.contentRect.height;
      if (h > 0) setMaxHeight(Math.max(200, Math.floor(h)));
    });
    ro.observe(parent);
    return () => ro.disconnect();
  }, []);

  return (
    <div ref={wrapRef} className="locate-pane__tree">
      <PaginatedFileTree
        items={items}
        rootNodeName="root"
        openDirectories={openDirectories}
        setOpenDirectories={setOpenDirectoriesAdapter}
        onFileSelect={onFileSelect}
        selectedFile={selectedFile}
        maxHeight={maxHeight}
      />
    </div>
  );
}

/* ---------- Filter mode ---------- */

function FilterMode({
  project,
  nodes,
  typeCounts,
  filters,
  onSetFilters,
  searchFacets,
}: {
  project: Project;
  nodes: NodeSummary[];
  typeCounts: Map<string, number>;
  filters: AssetFilters;
  onSetFilters: (next: AssetFilters) => void;
  /** Project-wide distinct facet values from the metadata adapter
   *  (`fetchSearchFacets`). When present, drives the Materialization / Package /
   *  Tag option lists so the UI doesn't recompute distinct values from `nodes`. */
  searchFacets: SearchFacets | null;
}) {
  const [showAllPkgs, setShowAllPkgs] = useState(false);
  const [showAllTags, setShowAllTags] = useState(false);
  // Full universe of values per dimension, derived from the entire node set
  // (ignoring filter narrowing) so options stay present even when cross-
  // filtering temporarily drops their count to 0.
  // Prefer the server-emitted modeling-layer taxonomy (stable
  // Staging → Intermediate → Marts ordering, count-0 rows kept) over a
  // client-side derive from `nodes.original_file_path`.
  const allLayers = useMemo(() => {
    if (searchFacets) return searchFacets.modelingLayers.map((v) => v.value);
    const s = new Set<string>();
    for (const n of nodes) {
      const l = inferModelingLayer(n.original_file_path);
      if (l) s.add(l);
    }
    return [...s].sort();
  }, [nodes, searchFacets]);
  // Prefer adapter-provided search facets when available. Fall back to
  // client-side distinct over `nodes` so the pane still renders before the
  // facets request resolves (or if it fails).
  const allMats = useMemo(() => {
    if (searchFacets) return searchFacets.materializationTypes.map((v) => v.value);
    const s = new Set<string>();
    for (const n of nodes) if (n.materialized) s.add(n.materialized);
    return [...s].sort();
  }, [nodes, searchFacets]);
  const allPkgs = useMemo(() => {
    const src = searchFacets
      ? searchFacets.packages.map((v) => v.value)
      : (() => {
          const s = new Set<string>();
          for (const n of nodes) if (n.package_name) s.add(n.package_name);
          return [...s];
        })();
    return src.sort((a, b) => {
      const aSelf = a === project.name ? 0 : 1;
      const bSelf = b === project.name ? 0 : 1;
      if (aSelf !== bSelf) return aSelf - bSelf;
      return a.localeCompare(b);
    });
  }, [nodes, project, searchFacets]);
  const allTags = useMemo(
    () => (searchFacets ? searchFacets.tags.map((v) => v.value) : []),
    [searchFacets],
  );
  const tagCounts = useMemo(() => {
    const m = new Map<string, number>();
    if (searchFacets) {
      for (const v of searchFacets.tags) m.set(v.value, v.count ?? 0);
    }
    return m;
  }, [searchFacets]);
  // Filter mode drives cross-type search; SEARCHABLE_RESOURCE_TYPES mirrors the
  // BE allowlist. Analysis is additionally gated by FEATURE_FLAGS.hasAnalysis
  // until the BE enum learns about it. Types with 0 count are listed
  // (CheckboxRow disables them) so the dimension is self-describing.
  const allTypes = useMemo(
    () =>
      RESOURCE_TYPE_ORDER.filter(
        (t) =>
          SEARCHABLE_RESOURCE_TYPES.has(t) &&
          (FEATURE_FLAGS.hasAnalysis || t !== 'analysis'),
      ),
    [],
  );

  // Project-wide counts for modeling layer / materialization / package read
  // from the adapter's search facets so resource types outside those tables are
  // included in the tally. These are unfiltered baselines — cross-filtered
  // narrowing (e.g. "packages count assuming current type selection") will land
  // alongside the search/facets cross-filter params.
  const layerCounts = useMemo(() => {
    const m = new Map<string, number>();
    if (searchFacets) {
      for (const v of searchFacets.modelingLayers) m.set(v.value, v.count ?? 0);
    }
    return m;
  }, [searchFacets]);

  const matCounts = useMemo(() => {
    const m = new Map<string, number>();
    if (searchFacets) {
      for (const v of searchFacets.materializationTypes) m.set(v.value, v.count ?? 0);
    }
    return m;
  }, [searchFacets]);

  const pkgCounts = useMemo(() => {
    const m = new Map<string, number>();
    if (searchFacets) {
      for (const v of searchFacets.packages) m.set(v.value, v.count ?? 0);
    }
    return m;
  }, [searchFacets]);

  const toggle = (key: keyof AssetFilters, value: string) => {
    const arr = filters[key];
    const next = arr.includes(value) ? arr.filter((v) => v !== value) : [...arr, value];
    onSetFilters({ ...filters, [key]: next });
  };

  const clear = (key: keyof AssetFilters) => onSetFilters({ ...filters, [key]: [] });

  // Skip 0-count options so "Select all" matches the visibly-enabled rows;
  // selecting a disabled value would only ever return nothing.
  const selectAllWithCounts = (
    key: keyof AssetFilters,
    options: string[],
    counts: Map<string, number>,
  ) =>
    onSetFilters({
      ...filters,
      [key]: options.filter((o) => (counts.get(o) ?? 0) > 0),
    });

  return (
    <div className="locate-pane__filter">
      {/* Asset type — multi-select */}
      <FilterSection
        title="Asset type"
        onSelectAll={() => selectAllWithCounts('resourceType', allTypes, typeCounts)}
        onClear={() => clear('resourceType')}
      >
        {allTypes.map((t) => {
          const count = typeCounts.get(t) ?? 0;
          return (
            <CheckboxRow
              key={t}
              label={RESOURCE_TYPE_LABEL[t] ?? t}
              count={count}
              checked={filters.resourceType.includes(t)}
              onChange={() => toggle('resourceType', t)}
              disabled={count === 0 && !filters.resourceType.includes(t)}
            />
          );
        })}
      </FilterSection>

      {/* Modeling layer */}
      <FilterSection
        title="Modeling layer"
        onSelectAll={() => selectAllWithCounts('modelingLayer', allLayers, layerCounts)}
        onClear={() => clear('modelingLayer')}
      >
        {allLayers.map((l) => {
          const count = layerCounts.get(l) ?? 0;
          return (
            <CheckboxRow
              key={l}
              label={l}
              count={count}
              checked={filters.modelingLayer.includes(l)}
              onChange={() => toggle('modelingLayer', l)}
              disabled={count === 0 && !filters.modelingLayer.includes(l)}
            />
          );
        })}
      </FilterSection>

      {/* Materialization */}
      <FilterSection
        title="Materialization"
        onSelectAll={() => selectAllWithCounts('materialization', allMats, matCounts)}
        onClear={() => clear('materialization')}
      >
        {allMats.map((m) => {
          const count = matCounts.get(m) ?? 0;
          return (
            <CheckboxRow
              key={m}
              label={m.charAt(0).toUpperCase() + m.slice(1)}
              count={count}
              checked={filters.materialization.includes(m)}
              onChange={() => toggle('materialization', m)}
              disabled={count === 0 && !filters.materialization.includes(m)}
            />
          );
        })}
      </FilterSection>

      {/* Package — collapsed by default to keep the filter pane scannable on
          projects with long package lists (e.g. dbt-tld). Selected packages
          stay visible even when collapsed. */}
      {(() => {
        const head = allPkgs.slice(0, PKG_VISIBLE_DEFAULT);
        const tail = allPkgs.slice(PKG_VISIBLE_DEFAULT);
        const tailSelected = tail.filter((p) => filters.pkg.includes(p));
        const visiblePkgs = showAllPkgs ? allPkgs : [...head, ...tailSelected];
        const hiddenCount = allPkgs.length - PKG_VISIBLE_DEFAULT;
        return (
          <FilterSection
            title="Package"
            onSelectAll={() => {
              selectAllWithCounts('pkg', allPkgs, pkgCounts);
              setShowAllPkgs(true);
            }}
            onClear={() => clear('pkg')}
          >
            {visiblePkgs.map((p) => {
              const count = pkgCounts.get(p) ?? 0;
              return (
                <CheckboxRow
                  key={p}
                  label={p}
                  count={count}
                  checked={filters.pkg.includes(p)}
                  onChange={() => toggle('pkg', p)}
                  emphasis={p === project.name}
                  disabled={count === 0 && !filters.pkg.includes(p)}
                />
              );
            })}
            {hiddenCount > 0 && (
              <button
                type="button"
                className="locate-pane__type-toggle"
                onClick={() => setShowAllPkgs((v) => !v)}
                aria-expanded={showAllPkgs}
              >
                {showAllPkgs ? 'Less' : `More (${hiddenCount})`}
              </button>
            )}
          </FilterSection>
        );
      })()}

      {/* Tag — only renders when the server returns a non-empty tag facet.
          Same collapse-after-N pattern as Package; selected tags stay
          visible when collapsed. */}
      {allTags.length > 0 &&
        (() => {
          const head = allTags.slice(0, TAG_VISIBLE_DEFAULT);
          const tail = allTags.slice(TAG_VISIBLE_DEFAULT);
          const tailSelected = tail.filter((t) => filters.tag.includes(t));
          const visibleTags = showAllTags ? allTags : [...head, ...tailSelected];
          const hiddenCount = allTags.length - TAG_VISIBLE_DEFAULT;
          return (
            <FilterSection
              title="Tag"
              onSelectAll={() => {
                selectAllWithCounts('tag', allTags, tagCounts);
                setShowAllTags(true);
              }}
              onClear={() => clear('tag')}
            >
              {visibleTags.map((t) => (
                <CheckboxRow
                  key={t}
                  label={t}
                  count={tagCounts.get(t) ?? 0}
                  checked={filters.tag.includes(t)}
                  onChange={() => toggle('tag', t)}
                />
              ))}
              {hiddenCount > 0 && (
                <button
                  type="button"
                  className="locate-pane__type-toggle"
                  onClick={() => setShowAllTags((v) => !v)}
                  aria-expanded={showAllTags}
                >
                  {showAllTags ? 'Less' : `More (${hiddenCount})`}
                </button>
              )}
            </FilterSection>
          );
        })()}
    </div>
  );
}

/* ---------- Filter section primitives ---------- */

function FilterSection({
  title,
  children,
  onSelectAll,
  onClear,
}: {
  title: string;
  children: ReactNode;
  onSelectAll?: () => void;
  onClear?: () => void;
}) {
  return (
    <section className="locate-pane__filter-section">
      <header className="locate-pane__filter-section-head">
        <h4>{title}</h4>
      </header>
      <div className="locate-pane__filter-options">{children}</div>
      {(onSelectAll || onClear) && (
        <footer className="locate-pane__filter-section-foot">
          {onSelectAll ? (
            <button
              type="button"
              className="locate-pane__filter-action"
              onClick={onSelectAll}
            >
              Select all
            </button>
          ) : (
            <span />
          )}
          {onClear && (
            <button
              type="button"
              className="locate-pane__filter-action"
              onClick={onClear}
            >
              Clear all
            </button>
          )}
        </footer>
      )}
    </section>
  );
}

function CheckboxRow({
  label,
  count,
  checked,
  onChange,
  emphasis,
  disabled,
}: {
  label: string;
  count?: number;
  checked: boolean;
  onChange: () => void;
  emphasis?: boolean;
  disabled?: boolean;
}) {
  return (
    <label
      className={`locate-pane__checkbox-row ${checked ? 'is-checked' : ''} ${disabled ? 'is-disabled' : ''}`}
      title={disabled ? 'No resources in this project match this filter' : undefined}
      aria-label={
        disabled ? 'No resources in this project match this filter' : undefined
      }
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={onChange}
        disabled={disabled}
        className="locate-pane__checkbox"
      />
      <span className={`locate-pane__checkbox-label ${emphasis ? 'is-emphasis' : ''}`}>
        {label}
      </span>
      {count !== undefined && (
        <span className="locate-pane__checkbox-count">{count.toLocaleString()}</span>
      )}
    </label>
  );
}
