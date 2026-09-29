import type { Meta, StoryObj } from '@storybook/react-vite';

import { storyModel, storySource } from '../shared/testing/storyFixtures';
import { loadingStorySource, storyDataSource } from '../shared/testing/storySources';
import { NodeLineagePanel } from './NodeLineagePanel';

const meta: Meta<typeof NodeLineagePanel> = {
  component: NodeLineagePanel,
  args: { uniqueId: 'model.jaffle_shop.customers', onClose: () => {} },
  // The panel is absolutely positioned against its nearest positioned ancestor and
  // slides in from the right edge of it.
  decorators: [
    (Story) => (
      <div className="relative h-[600px] w-full overflow-hidden bg-bgBackground">
        {Story()}
      </div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof NodeLineagePanel>;

/** Open, on a model: general/columns/relationships tabs, resolved from the asset. */
export const Open: Story = {};

/**
 * A null `uniqueId` closes the panel — but the previously-shown content stays mounted
 * so the slide-out animates with something in it. Here nothing was shown first, so it
 * is simply off-screen to the right.
 */
export const Closed: Story = {
  args: { uniqueId: null },
};

/** A source: no relationships in the fixture, so that tab drops out and the tab bar
 *  shrinks. */
export const SourceAsset: Story = {
  args: { uniqueId: 'source.jaffle_shop.raw.customers' },
  parameters: {
    docsApp: {
      source: storyDataSource({ fetchAsset: async () => storySource() }),
    },
  },
};

/** A macro has no columns, so only the tabs that apply are rendered — the tab set is
 *  derived from the asset, not fixed. */
export const MacroAsset: Story = {
  args: { uniqueId: 'macro.jaffle_shop.cents_to_dollars' },
};

/** With no columns and no relationships, a single tab remains and the bar hides. */
export const SparseAsset: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchAsset: async () =>
          storyModel({ columns: [], dependsOn: [], referencedBy: [] }),
      }),
    },
  },
};

/** The active tab lives in the URL, so a panel deep-link opens on the right tab. */
export const DeepLinkedToColumnsTab: Story = {
  parameters: {
    docsApp: {
      initialEntries: ['/lineage?panel=model.jaffle_shop.customers&tab=columns'],
    },
  },
};

export const Loading: Story = {
  parameters: { docsApp: { source: loadingStorySource() } },
};

/** A uniqueId the source cannot resolve — the panel says so rather than rendering an
 *  empty shell, and still offers its close button. */
export const NotFound: Story = {
  args: { uniqueId: 'model.jaffle_shop.does_not_exist' },
  parameters: {
    docsApp: { source: storyDataSource({ fetchAsset: async () => null }) },
  },
};
