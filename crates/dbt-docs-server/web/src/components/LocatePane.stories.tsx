import type { Meta, StoryObj } from '@storybook/react-vite';

import type { AssetFilters } from '../App';
import { makeFakeProject } from '../shared';
import { storyCounts, storyFiles, storyNodes } from '../shared/testing/storyFixtures';
import { LocatePane } from './LocatePane';

const NO_FILTERS: AssetFilters = {
  resourceType: [],
  modelingLayer: [],
  materialization: [],
  pkg: [],
  tag: [],
};

const SEARCH_FACETS = {
  accesses: [
    { value: 'public', count: 24 },
    { value: 'protected', count: 96 },
  ],
  modelingLayers: [
    { value: 'staging', count: 80 },
    { value: 'marts', count: 30 },
  ],
  materializationTypes: [
    { value: 'view', count: 88 },
    { value: 'table', count: 46 },
  ],
  tags: [
    { value: 'daily', count: 61 },
    { value: 'pii', count: 9 },
  ],
  packages: [
    { value: 'jaffle_shop', count: 142 },
    { value: 'dbt_utils', count: 18 },
  ],
};

const meta: Meta<typeof LocatePane> = {
  component: LocatePane,
  args: {
    project: makeFakeProject({ gitBranch: 'main', gitIsDirty: false }),
    nodes: storyNodes(),
    files: storyFiles(),
    selectedId: null,
    previewId: null,
    isListView: false,
    isHome: true,
    query: '',
    theme: 'dark',
    filters: NO_FILTERS,
    mode: 'assets',
    searchFacets: SEARCH_FACETS,
    assetCounts: storyCounts(),
    userState: 'core',
    onPeek: () => {},
    onSelect: () => {},
    onShowList: () => {},
    onShowProject: () => {},
    onSetTheme: () => {},
    onSetFilters: () => {},
    onSelectMode: () => {},
  },
  // The sidebar is a full-height rail; it needs the height to lay out its tabs,
  // scrolling body and footer rail.
  decorators: [
    (Story) => (
      <div className="flex h-[780px] w-[340px] border-r border-borderMuted">
        {Story()}
      </div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof LocatePane>;

/** Asset mode: the resource-type tree, grouped by type, with counts from
 *  `fetchAssetCounts`. */
export const AssetMode: Story = {};

/**
 * File mode: the on-disk tree built from `files`, which spans every file-bearing
 * resource rather than just `dbt.nodes` types.
 */
export const FileMode: Story = {
  args: { mode: 'files' },
};

/**
 * Filter mode, the `/search` surface. Facet values come from `fetchSearchFacets`; when
 * that is still in flight the pane derives them from `nodes` instead.
 */
export const FilterMode: Story = {
  args: { mode: 'filter' },
};

export const FilterModeWithSelections: Story = {
  args: {
    mode: 'filter',
    filters: { ...NO_FILTERS, resourceType: ['model'], tag: ['daily'] },
  },
};

/** Facets still loading — the fallback path, where values are derived client-side. */
export const FilterModeWithoutFacets: Story = {
  args: { mode: 'filter', searchFacets: null },
};

/**
 * Counts still loading. Worth its own story because `nodes` alone under-counts
 * everything outside `dbt.nodes` — macros, exposures, metrics — so this is visibly
 * different from the resolved state rather than just briefly blank.
 */
export const WithoutAssetCounts: Story = {
  args: { assetCounts: null },
};

export const WithSelectedNode: Story = {
  args: { selectedId: 'model.jaffle_shop.customers', isHome: false },
};

export const WithPreviewedNode: Story = {
  args: { previewId: 'model.jaffle_shop.customers', isHome: false },
};

/** A search query narrows the tree in place. */
export const WithQuery: Story = {
  args: { query: 'customer' },
};

export const WithQueryMatchingNothing: Story = {
  args: { query: 'zzzz' },
};

/** The first-paint progress readout, shown while `dbt.nodes` streams in. */
export const LoadingProgress: Story = {
  args: { nodes: [], loadingProgress: { loaded: 1200, total: 6472 } },
};

/** An empty project. */
export const NoNodes: Story = {
  args: { nodes: [], files: [], assetCounts: {} },
};

/** Light theme via the pane's own switcher, independent of the toolbar. */
export const LightTheme: Story = {
  args: { theme: 'light' },
};

export const SystemTheme: Story = {
  args: { theme: 'system' },
};

/** A dirty working tree gets its own indicator next to the branch name. */
export const DirtyGitBranch: Story = {
  args: {
    project: makeFakeProject({ gitBranch: 'feature/column-lineage', gitIsDirty: true }),
  },
};

/** No git information at all — a docs site generated outside a repo. */
export const WithoutGitInfo: Story = {
  args: {
    project: makeFakeProject({ gitBranch: null, gitIsDirty: null }),
  },
};

/** The footer rail upsell stack collapses while capabilities are still resolving,
 *  rather than flashing the wrong user state's copy. */
export const WithoutUserState: Story = {
  args: { userState: null },
};

export const ViaCatalogUserState: Story = {
  args: { userState: 'via-catalog' },
};

/** List view active — the project row and type rows highlight differently. */
export const ListViewActive: Story = {
  args: { isListView: true, isHome: false },
};
