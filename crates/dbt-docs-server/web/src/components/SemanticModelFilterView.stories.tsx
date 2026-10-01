import type { Meta, StoryObj } from '@storybook/react-vite';

import { makeFakeProject } from '../shared';
import {
  emptyStorySource,
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
} from '../shared/testing/storySources';
import { SemanticModelFilterView } from './SimpleFilterViews';

const meta: Meta<typeof SemanticModelFilterView> = {
  component: SemanticModelFilterView,
  args: { project: makeFakeProject(), onPeek: () => {} },
};

export default meta;
type Story = StoryObj<typeof SemanticModelFilterView>;

/** Semantic models list their entity names, comma-joined. */
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
