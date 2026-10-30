import type { Meta, StoryObj } from '@storybook/react-vite';

import { SimpleLinkBreadcrumbs } from './SimpleLinkBreadcrumbs';

const meta: Meta<typeof SimpleLinkBreadcrumbs> = {
  component: SimpleLinkBreadcrumbs,
  args: {
    className: 'font-caption block text-fgDecorative',
    breadcrumbs: [
      { text: 'jaffle_shop', href: '#/' },
      { text: 'Models', href: '#/models' },
      { text: 'customers' },
    ],
  },
};

export default meta;
type Story = StoryObj<typeof SimpleLinkBreadcrumbs>;

/** The last crumb has no href, so it renders as plain text — the current page. */
export const Default: Story = {};

export const TwoLevels: Story = {
  args: {
    breadcrumbs: [{ text: 'jaffle_shop', href: '#/' }, { text: 'Models' }],
  },
};

/**
 * The interesting behaviour: when the trail overflows its container it collapses to
 * `.. / last`, and expands again on hover or keyboard focus. Overflow is measured with
 * a `ResizeObserver`, so it only happens at a real width — hence the narrow wrapper.
 * Hover it to see the full trail.
 */
export const CollapsesWhenOverflowing: Story = {
  args: {
    breadcrumbs: [
      { text: 'jaffle_shop_analytics_platform', href: '#/' },
      { text: 'Models', href: '#/models' },
      { text: 'marts', href: '#/models?modeling_layer=marts' },
      { text: 'finance', href: '#/groups/finance' },
      { text: 'int_order_items_joined_to_customers' },
    ],
  },
  decorators: [(Story) => <div className="w-64">{Story()}</div>],
};

/** The same trail with room to breathe, for comparison. */
export const LongTrailWithRoom: Story = {
  args: {
    breadcrumbs: [
      { text: 'jaffle_shop_analytics_platform', href: '#/' },
      { text: 'Models', href: '#/models' },
      { text: 'marts', href: '#/models?modeling_layer=marts' },
      { text: 'finance', href: '#/groups/finance' },
      { text: 'int_order_items_joined_to_customers' },
    ],
  },
  decorators: [(Story) => <div className="w-[900px]">{Story()}</div>],
};

export const SingleCrumb: Story = {
  args: { breadcrumbs: [{ text: 'jaffle_shop' }] },
};
