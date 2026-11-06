import type { Meta, StoryObj } from '@storybook/react-vite';

import { NoLineageFallback } from './NoLineageFallback';

const meta: Meta<typeof NoLineageFallback> = {
  component: NoLineageFallback,
  args: { modelName: 'customers' },
  decorators: [
    (Story) => (
      <div className="w-[560px] rounded-lg border border-borderMuted">{Story()}</div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof NoLineageFallback>;

/**
 * Shown when a model has no lineage edges in the index. That is usually not a broken
 * model — it is an index built without lineage — so the copy offers the command that
 * would populate it, with the model's own name interpolated into the selector.
 */
export const Default: Story = {};

/** The generated command embeds the model name, so a long one is worth checking for
 *  overflow: the snippet is `shrink-0` and does not wrap. */
export const LongModelName: Story = {
  args: { modelName: 'int_order_items_joined_to_customers_and_products_and_locations' },
};
