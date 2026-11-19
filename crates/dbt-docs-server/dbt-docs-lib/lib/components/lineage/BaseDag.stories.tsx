import type { Meta, StoryObj } from '@storybook/react-vite';

import { storyLineage } from '../../shared/testing/storyFixtures';
import { storyDataSource } from '../../shared/testing/storySources';
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
