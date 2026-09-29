import type { Meta, StoryObj } from '@storybook/react-vite';

import { RelationName } from './RelationName';

const meta: Meta<typeof RelationName> = {
  component: RelationName,
  args: {
    relation: { database: 'analytics', schema: 'dbt', identifier: 'customers' },
  },
  decorators: [(Story) => <div className="max-w-sm">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof RelationName>;

/** Copy affordance on by default: the whole row is clickable and a toast confirms. */
export const Default: Story = {};

/** `alias` wins over `identifier` — that is the point of having both, since an
 *  aliased model is addressed in the warehouse by its alias. */
export const AliasOverridesIdentifier: Story = {
  args: {
    relation: {
      database: 'analytics',
      schema: 'dbt',
      identifier: 'customers',
      alias: 'dim_customers',
    },
  },
};

/** Read-only: no copy button, no pointer cursor. */
export const WithoutCopy: Story = {
  args: { copy: false },
};

/** Long relations truncate with a tooltip that appears only once clipped. */
export const Truncated: Story = {
  args: {
    relation: {
      database: 'analytics_production',
      schema: 'dbt_marts_finance',
      identifier: 'int_order_items_joined_to_customers_and_products',
    },
  },
};

/** All three parts are required. A partial relation renders nothing rather than a
 *  half-qualified name that would not resolve if pasted into a query — which is
 *  common for resource types with no warehouse object at all. */
export const IncompleteRelationRendersNothing: Story = {
  args: { relation: { database: 'analytics', schema: null, identifier: 'customers' } },
};
