import type { Meta, StoryObj } from '@storybook/react-vite';

import type { AssetFilters } from '../App';
import { makeFakeProject } from '../shared';
import { storyNodes } from '../shared/testing/storyFixtures';
import type { NodeSummary } from '../types';
import { AssetListView } from './AssetListView';

const NO_FILTERS: AssetFilters = {
  resourceType: [],
  modelingLayer: [],
  materialization: [],
  pkg: [],
  tag: [],
};

/** More rows than the base fixture, so paging and column sizing have something to
 *  work with. */
function manyNodes(count: number): NodeSummary[] {
  const base = storyNodes();
  return Array.from({ length: count }, (_, i) => {
    const node = base[i % base.length] as NodeSummary;
    return {
      ...node,
      unique_id: `${node.unique_id}_${i}`,
      name: `${node.name}_${i}`,
    };
  });
}

const meta: Meta<typeof AssetListView> = {
  component: AssetListView,
  args: {
    project: makeFakeProject(),
    nodes: storyNodes(),
    query: '',
    filters: NO_FILTERS,
    previewId: null,
    onPeek: () => {},
  },
};

export default meta;
type Story = StoryObj<typeof AssetListView>;

/** The cross-type asset list, driven entirely off the in-memory node index — it takes
 *  its rows as a prop rather than fetching. */
export const Default: Story = {};

/** Enough rows to page. The list shows a fixed page and grows on demand rather than
 *  rendering the whole project. */
export const ManyRows: Story = {
  args: { nodes: manyNodes(120) },
};

/** A free-text query. Filtering happens client-side over the same node list. */
export const WithSearchQuery: Story = {
  args: { nodes: manyNodes(60), query: 'customers' },
};

/** Active filters render as removable chips above the table. */
export const WithActiveFilters: Story = {
  args: {
    nodes: manyNodes(60),
    filters: { ...NO_FILTERS, resourceType: ['model'], pkg: ['jaffle_shop'] },
  },
};

export const WithManyActiveFilters: Story = {
  args: {
    nodes: manyNodes(60),
    filters: {
      resourceType: ['model', 'source'],
      modelingLayer: ['marts'],
      materialization: ['table', 'view'],
      pkg: ['jaffle_shop', 'dbt_utils'],
      tag: ['daily'],
    },
  },
};

/** A query matching nothing — the empty state, with the query still visible so the
 *  reader can tell why. */
export const NoMatches: Story = {
  args: { query: 'zzzz_no_such_model' },
};

/** An empty project. */
export const NoNodes: Story = {
  args: { nodes: [] },
};

/** `previewId` marks the row currently open in the peek drawer. */
export const WithRowSelected: Story = {
  args: { previewId: 'model.jaffle_shop.customers' },
};
