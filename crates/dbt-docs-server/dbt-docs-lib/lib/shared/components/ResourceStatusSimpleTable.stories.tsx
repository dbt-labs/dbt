import type { Meta, StoryObj } from '@storybook/react-vite';

import {
  type ResourceStatusRow,
  ResourceStatusSimpleTable,
} from './ResourceStatusSimpleTable';

const TEST_ROWS: ResourceStatusRow[] = [
  {
    name: 'not_null_customers_customer_id',
    uniqueId: 'test.jaffle_shop.not_null_customers_customer_id',
    resourceType: 'test',
    status: 'pass',
    statusKind: 'test',
  },
  {
    name: 'unique_customers_customer_id',
    uniqueId: 'test.jaffle_shop.unique_customers_customer_id',
    resourceType: 'test',
    status: 'fail',
    statusKind: 'test',
  },
  {
    name: 'accepted_values_orders_status',
    uniqueId: 'test.jaffle_shop.accepted_values_orders_status',
    resourceType: 'test',
    status: 'warn',
    statusKind: 'test',
  },
];

const meta: Meta<typeof ResourceStatusSimpleTable> = {
  component: ResourceStatusSimpleTable,
  args: { rows: TEST_ROWS },
  decorators: [
    (Story) => (
      <div className="w-[520px] rounded-lg border border-borderMuted">{Story()}</div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof ResourceStatusSimpleTable>;

/** Test rows: icon chip, name, status badge. */
export const TestStatuses: Story = {};

/** Freshness rows use the other badge vocabulary for the same strings. */
export const FreshnessStatuses: Story = {
  args: {
    rows: [
      {
        name: 'raw.customers',
        uniqueId: 'source.jaffle_shop.raw.customers',
        resourceType: 'source',
        status: 'pass',
        statusKind: 'freshness',
      },
      {
        name: 'raw.orders',
        uniqueId: 'source.jaffle_shop.raw.orders',
        resourceType: 'source',
        status: 'error',
        statusKind: 'freshness',
      },
      {
        name: 'raw.events',
        uniqueId: 'source.jaffle_shop.raw.events',
        resourceType: 'source',
        status: 'unconfigured',
        statusKind: 'freshness',
      },
    ],
  },
};

/** `statusKind: 'run'` renders no badge at all — only test and freshness map to one. */
export const RunKindHasNoBadge: Story = {
  args: {
    rows: [
      {
        name: 'customers',
        uniqueId: 'model.jaffle_shop.customers',
        resourceType: 'model',
        status: 'success',
        statusKind: 'run',
      },
    ],
  },
};

/** `onSelect` makes each name a button — how the panel hands a click back to the page. */
export const Selectable: Story = {
  args: { onSelect: () => {} },
};

/** An unrecognised or null status falls back to `unknown` rather than rendering an
 *  empty badge. */
export const UnrecognisedStatus: Story = {
  args: {
    rows: [
      {
        name: 'test_with_null_status',
        uniqueId: 'test.jaffle_shop.null_status',
        resourceType: 'test',
        status: null,
        statusKind: 'test',
      },
      {
        name: 'test_with_odd_status',
        uniqueId: 'test.jaffle_shop.odd_status',
        resourceType: 'test',
        status: 'something_new',
        statusKind: 'test',
      },
    ],
  },
};

export const LongNamesTruncate: Story = {
  args: {
    rows: [
      {
        name: 'accepted_values_int_order_items_joined_to_customers_status__completed__shipped__returned',
        uniqueId: 'test.jaffle_shop.long',
        resourceType: 'test',
        status: 'pass',
        statusKind: 'test',
      },
    ],
  },
};

/** No rows renders just the top border — callers hide the section themselves. */
export const NoRows: Story = {
  args: { rows: [] },
};
