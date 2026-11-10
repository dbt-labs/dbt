import type { Meta, StoryObj } from '@storybook/react-vite';

import { PropertyCard } from './PropertyCard';

const meta: Meta<typeof PropertyCard> = {
  component: PropertyCard,
  args: {
    title: 'Row count',
    children: '128,450',
  },
};

export default meta;
type Story = StoryObj<typeof PropertyCard>;

export const Default: Story = {};

/** `info` adds a tooltip trigger to the right of the title — for values whose
 *  provenance needs a sentence, like a stat that comes from the last catalog run
 *  rather than from live SQL. */
export const WithInfoTooltip: Story = {
  args: {
    info: 'From the most recent catalog run, not queried live.',
  },
};

/** The card is a fixed 20/56 box and both lines truncate, so long values clip
 *  rather than reflow the row of cards. */
export const TruncatedValue: Story = {
  args: {
    title: 'Relation',
    children: 'analytics.dbt_production_marts.int_order_items_joined_to_customers',
  },
};

/** How they actually appear — a wrapping row on a resource detail page. */
export const CardRow: Story = {
  render: () => (
    <div className="flex flex-wrap">
      <PropertyCard title="Materialization">table</PropertyCard>
      <PropertyCard title="Row count">128,450</PropertyCard>
      <PropertyCard title="Size">4.19 MB</PropertyCard>
      <PropertyCard title="Access">public</PropertyCard>
      <PropertyCard title="Contract" info="Enforced at build time.">
        Enforced
      </PropertyCard>
    </div>
  ),
};
