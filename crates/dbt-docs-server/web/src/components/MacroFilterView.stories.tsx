import type { Meta, StoryObj } from '@storybook/react-vite';

import { makeFakeProject } from '../shared';
import {
  emptyStorySource,
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
  storyDataSource,
} from '../shared/testing/storySources';
import { MacroFilterView } from './MacroFilterView';

const meta: Meta<typeof MacroFilterView> = {
  component: MacroFilterView,
  args: { project: makeFakeProject(), onPeek: () => {} },
};

export default meta;
type Story = StoryObj<typeof MacroFilterView>;

/** Macros list name, arguments and description, filtered by package — the fixture
 *  spans two packages so the dropdown has something to do. */
export const Default: Story = {};

/** Some macros take no arguments, which renders as an empty cell rather than `[]`. */
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
