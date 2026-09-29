import type { Meta, StoryObj } from '@storybook/react-vite';

import {
  storyExposure,
  storyGroup,
  storyMacro,
  storyMetric,
  storyModel,
  storySavedQuery,
  storySemanticModel,
  storySource,
  storyTest,
} from '../shared/testing/storyFixtures';
import {
  gatedLineageStorySource,
  storyDataSource,
} from '../shared/testing/storySources';
import { NodeDetail } from './NodeDetail';

const meta: Meta<typeof NodeDetail> = {
  component: NodeDetail,
  args: {
    asset: storyModel(),
    onSelect: () => {},
    hasColumnLineage: false,
    userState: 'core',
  },
};

export default meta;
type Story = StoryObj<typeof NodeDetail>;

/**
 * The resource detail page. Which tabs exist is derived per resource type — a macro
 * gets Arguments, a semantic model gets Dimensions and Measures — so the stories below
 * are mostly a tour of that switch.
 */
export const Model: Story = {};

export const Source: Story = {
  args: { asset: storySource() },
};

/** Macros get an Arguments tab and no Columns tab. */
export const Macro: Story = {
  args: { asset: storyMacro() },
};

/** Semantic models get Dimensions and Measures tabs, both with counts. */
export const SemanticModel: Story = {
  args: { asset: storySemanticModel() },
};

/** Saved queries get a Query Exports tab. */
export const SavedQuery: Story = {
  args: { asset: storySavedQuery() },
};

export const Metric: Story = {
  args: { asset: storyMetric() },
};

export const Exposure: Story = {
  args: { asset: storyExposure() },
};

export const Test: Story = {
  args: { asset: storyTest() },
};

/** A group has neither code nor columns — the narrowest tab set. */
export const Group: Story = {
  args: { asset: storyGroup() },
};

/**
 * With column lineage available, each column row grows an expand caret that lazily
 * fetches the per-column subgraph.
 */
export const WithColumnLineage: Story = {
  args: { hasColumnLineage: true },
  parameters: { docsApp: { source: storyDataSource() } },
};

/**
 * Column lineage advertised but reported unavailable by the source: expanding a column
 * shows the upsell rather than an empty graph.
 */
export const ColumnLineageGated: Story = {
  args: { hasColumnLineage: true },
  parameters: { docsApp: { source: gatedLineageStorySource() } },
};

/** No config block means no Config tab, rather than an empty one. */
export const WithoutConfig: Story = {
  args: { asset: storyModel({ config: null }) },
};

/** No columns at all. The Columns tab is dropped and the
 *  no-column-metadata fallback explains how to populate it. */
export const WithoutColumns: Story = {
  args: { asset: storyModel({ columns: [] }) },
};

/** No code: an index written without a successful compile drops the Code tab. */
export const WithoutCode: Story = {
  args: { asset: storyModel({ rawCode: null, compiledCode: null }) },
};

/**
 * The sparsest realistic model: undocumented, uncontracted, no stats, no relations.
 * This is what a plain `docs generate` produces, so it needs to look deliberate rather
 * than broken.
 */
export const Sparse: Story = {
  args: {
    asset: storyModel({
      description: null,
      columns: [],
      compiledCode: null,
      config: null,
      meta: null,
      tags: [],
      access: null,
      contractEnforced: null,
      group: null,
      dependsOn: [],
      referencedBy: [],
      rowCountStat: null,
      bytesStat: null,
      primaryKey: null,
      owner: null,
    }),
  },
};

/** Capabilities still loading, so the column-lineage upsell has no user state to speak to
 *  and stays hidden. */
export const WithoutUserState: Story = {
  args: { hasColumnLineage: true, userState: null },
  parameters: { docsApp: { source: gatedLineageStorySource() } },
};
