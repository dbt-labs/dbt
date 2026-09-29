import type { Meta, StoryObj } from '@storybook/react-vite';

import { makeFakeProject } from '../shared';
import {
  emptyStorySource,
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
} from '../shared/testing/storySources';
import { SnapshotFilterView } from './SimpleFilterViews';

const meta: Meta<typeof SnapshotFilterView> = {
  component: SnapshotFilterView,
  args: { project: makeFakeProject(), onPeek: () => {} },
};

export default meta;
type Story = StoryObj<typeof SnapshotFilterView>;

/**
 * Snapshots show row count and a human-formatted size.
 *
 * The "Last modified" column is adapter-conditional — only Snowflake reports the stat
 * — so it is absent here. `makeFakeProject` defaults to duckdb.
 */
export const Default: Story = {};

/** On Snowflake the extra "Last modified" column appears. This is the whole reason
 *  the column set depends on `project.adapterType`. */
export const SnowflakeAddsLastModified: Story = {
  args: { project: makeFakeProject({ adapterType: 'snowflake' }) },
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
