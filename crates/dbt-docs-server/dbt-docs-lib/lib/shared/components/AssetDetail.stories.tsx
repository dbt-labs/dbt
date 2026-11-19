import type { Meta, StoryObj } from '@storybook/react-vite';

import {
  storyExposure,
  storyGroup,
  storyMacro,
  storyMetric,
  storyModel,
  storySource,
} from '../testing/storyFixtures';
import { loadingStorySource, storyDataSource } from '../testing/storySources';
import { AssetDetail } from './AssetDetail';

const meta: Meta<typeof AssetDetail> = {
  component: AssetDetail,
  args: { asset: storyModel() },
};

export default meta;
type Story = StoryObj<typeof AssetDetail>;

/** Header + metadata + code + columns, composed from one resolved asset. */
export const Model: Story = {};

/** A source has no code, so that block drops out and the metadata switches to the
 *  loader/database/schema entries. */
export const Source: Story = {
  args: { asset: storySource() },
};

export const Exposure: Story = {
  args: { asset: storyExposure() },
};

export const Metric: Story = {
  args: { asset: storyMetric() },
};

export const Macro: Story = {
  args: { asset: storyMacro() },
};

/** A group has neither code nor columns — the sparsest shape this composes. */
export const Group: Story = {
  args: { asset: storyGroup() },
};

/** An undocumented model with no compiled code and no catalog columns — the most
 *  common real-world rendering. */
export const Sparse: Story = {
  args: {
    asset: storyModel({
      description: null,
      compiledCode: null,
      columns: [],
      tags: [],
      meta: null,
      access: null,
      contractEnforced: null,
      group: null,
    }),
  },
};

/**
 * Passing {@link AssetArgs} instead of a resolved asset makes the component fetch it
 * through `useAssetDetail`. Same component, the other half of its union prop.
 */
export const FetchedByArgs: Story = {
  args: undefined,
  render: () => (
    <AssetDetail uniqueId="model.jaffle_shop.customers" resourceType="model" />
  ),
  parameters: { docsApp: { source: storyDataSource() } },
};

export const FetchedLoading: Story = {
  args: undefined,
  render: () => (
    <AssetDetail uniqueId="model.jaffle_shop.customers" resourceType="model" />
  ),
  parameters: { docsApp: { source: loadingStorySource() } },
};

/** A missing asset renders nothing rather than an error — the route above it owns the
 *  404. */
export const FetchedNotFound: Story = {
  args: undefined,
  render: () => <AssetDetail uniqueId="model.jaffle_shop.nope" resourceType="model" />,
  parameters: {
    docsApp: { source: storyDataSource({ fetchAsset: async () => null }) },
  },
};
