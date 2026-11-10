import type { Meta, StoryObj } from '@storybook/react-vite';

import { makeFakeProject } from '../shared';
import {
  emptyStorySource,
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
  storyDataSource,
} from '../shared/testing/storySources';
import { ModelFilterView } from './ModelFilterView';

const meta: Meta<typeof ModelFilterView> = {
  component: ModelFilterView,
  args: { project: makeFakeProject(), onPeek: () => {} },
};

export default meta;
type Story = StoryObj<typeof ModelFilterView>;

/** Three facet dropdowns, a sortable name/last-executed table, and server-side sort. */
export const Default: Story = {};

/**
 * Modeling layer is held in the URL rather than in state, so the home page's
 * "Show marts" CTA can deep-link into a pre-filtered list. This story boots the router
 * at that URL, which is the only way to see the dropdown pre-selected.
 */
export const DeepLinkedToModelingLayer: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource(),
      initialEntries: ['/models?modeling_layer=Marts'],
    },
  },
};

/** No facet values: every dropdown collapses to just "All". */
export const WithoutFacets: Story = {
  parameters: {
    docsApp: { source: storyDataSource({ fetchFacets: async () => ({}) }) },
  },
};

export const Empty: Story = {
  parameters: { docsApp: { source: emptyStorySource() } },
};

export const Loading: Story = {
  parameters: { docsApp: { source: loadingStorySource() } },
};

export const LoadError: Story = {
  parameters: { docsApp: { source: failingStorySource() } },
};

export const UnsupportedSurface: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
};
