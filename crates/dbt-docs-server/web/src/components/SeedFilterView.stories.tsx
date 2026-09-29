import type { Meta, StoryObj } from '@storybook/react-vite';

import { makeFakeProject } from '../shared';
import {
  emptyStorySource,
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
} from '../shared/testing/storySources';
import { SeedFilterView } from './SimpleFilterViews';

const meta: Meta<typeof SeedFilterView> = {
  component: SeedFilterView,
  args: { project: makeFakeProject(), onPeek: () => {} },
};

export default meta;
type Story = StoryObj<typeof SeedFilterView>;

/** Seeds carry a row count and a last-executed timestamp. A seed that has never
 *  been run renders an em dash in the timestamp column. */
export const Default: Story = {};

export const Empty: Story = {
  parameters: { docsApp: { source: emptyStorySource() } },
};

export const Loading: Story = {
  parameters: { docsApp: { source: loadingStorySource() } },
};

export const LoadError: Story = {
  parameters: { docsApp: { source: failingStorySource() } },
};

/** A source that never implemented the list surface reports itself unsupported rather
 *  than rendering an empty table. */
export const UnsupportedSurface: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
};
