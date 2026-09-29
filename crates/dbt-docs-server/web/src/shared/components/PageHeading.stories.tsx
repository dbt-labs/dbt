import type { Meta, StoryObj } from '@storybook/react-vite';

import { Badge } from './Badge';
import { PageHeading } from './PageHeading';

const meta: Meta<typeof PageHeading> = {
  component: PageHeading,
  args: { children: 'customers' },
};

export default meta;
type Story = StoryObj<typeof PageHeading>;

export const Default: Story = {};

/** Truncation is on by default and the tooltip only appears once the text is
 *  actually clipped, so a narrow container is the only way to see either. */
export const Truncated: Story = {
  args: { children: 'int_order_items_joined_to_customers_and_products' },
  decorators: [(Story) => <div className="w-64">{Story()}</div>],
};

/** `shouldTruncate={false}` is for headings whose children render their own
 *  controls, where clipping would cut off a menu rather than a word. */
export const NotTruncated: Story = {
  args: {
    children: 'int_order_items_joined_to_customers_and_products',
    shouldTruncate: false,
  },
  decorators: [(Story) => <div className="w-64">{Story()}</div>],
};

/** The `additional` slots flank the heading — in the app this is where the resource
 *  icon and the status badges sit. */
export const WithAdjacentContent: Story = {
  args: {
    className: 'flex items-center gap-3',
    additional: {
      left: <Badge>model</Badge>,
      right: <Badge>public</Badge>,
    },
  },
};
