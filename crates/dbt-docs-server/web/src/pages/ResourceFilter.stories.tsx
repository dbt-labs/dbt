import type { Meta, StoryObj } from '@storybook/react-vite';

import type { AssetFilters } from '../App';
import { makeFakeProject } from '../shared';
import { storyNodes } from '../shared/testing/storyFixtures';
import ResourceFilter from './ResourceFilter';

const NO_FILTERS: AssetFilters = {
  resourceType: [],
  modelingLayer: [],
  materialization: [],
  pkg: [],
  tag: [],
};

const meta: Meta<typeof ResourceFilter> = {
  component: ResourceFilter,
  args: {
    project: makeFakeProject(),
    nodes: storyNodes(),
    query: '',
    filters: NO_FILTERS,
    previewId: null,
    onPeek: () => {},
  },
  parameters: { docsApp: { initialEntries: ['/list/model'] } },
};

export default meta;
type Story = StoryObj<typeof ResourceFilter>;

/**
 * A thin pass-through to `AssetListView`. The `:resourceType` route param does not
 * reach this component — `App.tsx` syncs it into `filters.resourceType`, and the list
 * reads the filters. So the narrowing you see here comes from `filters`, not from the
 * URL.
 */
export const Default: Story = {};

/** Narrowed the way the route would, via `filters.resourceType`. */
export const NarrowedToModels: Story = {
  args: { filters: { ...NO_FILTERS, resourceType: ['model'] } },
  parameters: { docsApp: { initialEntries: ['/list/model'] } },
};

export const NarrowedToSources: Story = {
  args: { filters: { ...NO_FILTERS, resourceType: ['source'] } },
  parameters: { docsApp: { initialEntries: ['/list/source'] } },
};

/** A resource type with no nodes in the fixture — the empty state a rarely-used type
 *  lands on. */
export const NoMatchingNodes: Story = {
  args: { filters: { ...NO_FILTERS, resourceType: ['saved_query'] } },
  parameters: { docsApp: { initialEntries: ['/list/saved_query'] } },
};
