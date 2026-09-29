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
import { TestFilterView } from './TestFilterView';

const meta: Meta<typeof TestFilterView> = {
  component: TestFilterView,
  args: { project: makeFakeProject(), onPeek: () => {} },
};

export default meta;
type Story = StoryObj<typeof TestFilterView>;

/** The fixture spans every status, so this is also the review of the status-badge
 *  column. */
export const Default: Story = {};

/**
 * The index writes a run status of `success`, but a *test* passes or fails — so
 * `success` is mapped to `pass` before it reaches the badge. This story feeds only
 * `success` rows to show that mapping rather than a row of "Unknown".
 */
export const RunStatusIsMappedToPass: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchAssetList: async () => ({
          items: storySummaries('test', 6).map((row) => ({
            ...row,
            status: 'success',
          })),
          nextCursor: null,
          totalCount: 6,
        }),
      }),
    },
  },
};

/** An unrecognised status falls back to `unknown` rather than rendering blank. */
export const UnrecognisedStatus: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchAssetList: async () => ({
          items: storySummaries('test', 4).map((row) => ({
            ...row,
            status: 'something_new',
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

export const UnsupportedSurface: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
};
