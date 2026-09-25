import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import { storyLineage } from '../../shared/testing/storyFixtures';
import { storyDataSource } from '../../shared/testing/storySources';
import { useLineageStore } from '../../stores/lineageStore';
import { BaseDag } from './BaseDag';

const meta: Meta<typeof BaseDag> = {
  component: BaseDag,
  args: {
    rootUniqueId: 'model.jaffle_shop.customers',
  },
  // React Flow measures its parent, so the canvas needs a sized one to render into at all.
  decorators: [(Story) => <div className="h-[520px] w-full">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof BaseDag>;

/** The default story fixture's lineage, laid out by dagre. */
export const Default: Story = {};

export const SimpleExample: Story = {
  args: {
    rootUniqueId: 'model.jaffle_shop.customers',
  },
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchLineage: async () => {
          const base = storyLineage();
          const extra = Array.from({ length: 10 }, (_, i) => ({
            uniqueId: `model.jaffle_shop.downstream_${i}`,
            name: `downstream_${i}`,
            resourceType: 'model' as const,
            description: null,
            packageName: 'jaffle_shop',
            tags: [],
            materialized: 'view',
          }));
          return {
            nodes: [...base.nodes, ...extra],
            edges: [
              ...base.edges,
              ...extra.map((n) => ({
                upstreamUniqueId: 'model.jaffle_shop.customers',
                downstreamUniqueId: n.uniqueId,
              })),
            ],
          };
        },
      }),
    },
  },
};

/** A different root must not inherit another resource's saved hop settings. */
export const ResetsHopsForDifferentRoot: Story = {
  beforeEach: () => {
    useLineageStore.getState().reset();
    useLineageStore.getState().startHydration('model.other.resource', 6, Infinity);
  },
  render: function RootSwitcher(args) {
    const [root, setRoot] = useState(args.rootUniqueId);
    return (
      <BaseDag
        rootUniqueId={root}
        topBarLeft={
          <button onClick={() => setRoot('model.jaffle_shop.orders')}>
            Show orders
          </button>
        }
      />
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const page = within(canvasElement.ownerDocument.body);
    await expect(await canvas.findByRole('button', { name: '1+' })).toBeVisible();
    await expect(canvas.getByRole('button', { name: '+1' })).toBeVisible();
    await userEvent.click(canvas.getByRole('button', { name: '1+' }));
    await userEvent.click(await page.findByRole('menuitemradio', { name: '6+' }));
    await userEvent.click(canvas.getByRole('button', { name: '+1' }));
    await userEvent.click(await page.findByRole('menuitemradio', { name: '+max' }));
    await userEvent.click(canvas.getByRole('button', { name: 'Show orders' }));
    await expect(await canvas.findByRole('button', { name: '1+' })).toBeVisible();
    await expect(canvas.getByRole('button', { name: '+1' })).toBeVisible();
  },
};
