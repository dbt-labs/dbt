import type { Meta, StoryObj } from '@storybook/react-vite';

import { makeFakeProject } from '../shared';
import {
  emptyStorySource,
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
  storyDataSource,
  storySummaries,
} from '../shared/testing/storySources';
import { GroupFilterView } from './SimpleFilterViews';

const meta: Meta<typeof GroupFilterView> = {
  component: GroupFilterView,
  args: { project: makeFakeProject(), onPeek: () => {} },
};

export default meta;
type Story = StoryObj<typeof GroupFilterView>;

/** Groups have the widest column set here: a right-aligned model count plus four
 *  owner columns, any of which can be blank. */
export const Default: Story = {};

/** Model count falls back to `0` rather than blank when the source does not supply it,
 *  so the column stays numeric. */
export const WithoutModelCounts: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchAssetList: async () => ({
          items: storySummaries('group', 4).map((row) => ({
            ...row,
            modelCount: null,
          })),
          nextCursor: null,
          totalCount: 4,
        }),
      }),
    },
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

/** A source that never implemented the list surface reports itself unsupported rather
 *  than rendering an empty table. */
export const UnsupportedSurface: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
};
