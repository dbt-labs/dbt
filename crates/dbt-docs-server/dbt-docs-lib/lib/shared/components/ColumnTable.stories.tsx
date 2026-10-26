import type { Meta, StoryObj } from '@storybook/react-vite';

import { ColumnTable, type Entry } from './ColumnTable';

const ENTRIES: Entry[] = [
  { key: 'materialization', data: 'table' },
  { key: 'rowCount', data: '128,450' },
  { key: 'access', data: 'public' },
  { key: 'packageName', data: 'jaffle_shop' },
];

const meta: Meta<typeof ColumnTable> = {
  component: ColumnTable,
  args: { tableEntries: ENTRIES, isLoading: false },
  decorators: [(Story) => <div className="max-w-md">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof ColumnTable>;

/** Keys are camelCase and get title-cased for display, which is why call sites pass
 *  field names rather than labels. */
export const Default: Story = {};

/** An explicit `title` overrides the derived one — for keys whose title-cased form
 *  reads badly. */
export const WithExplicitTitles: Story = {
  args: {
    tableEntries: [
      {
        key: 'fqn',
        title: 'Fully-qualified name',
        data: 'jaffle_shop.marts.customers',
      },
      { key: 'dbtVersion', title: 'dbt version', data: '1.10.2' },
    ],
  },
};

/** `compact` drops the outer dividers and tightens the row rhythm — the form used
 *  inside a panel rather than on a detail page. */
export const Compact: Story = {
  args: { compact: true },
};

/** Two keys are special-cased: a run status renders as a status badge and a
 *  completion timestamp is humanized. */
export const SpecialCasedKeys: Story = {
  args: {
    tableEntries: [
      { key: 'executionInfo.lastRunStatus', data: 'success' },
      { key: 'executionInfo.executeCompletedAt', data: '2026-02-11T16:12:18Z' },
    ],
  },
};

/** Rows with a `link` wrap their value in an internal link;
 *  `crossProjectLink` opens in a new tab instead. */
export const WithLinks: Story = {
  args: {
    tableEntries: [
      { key: 'testedModel', data: 'customers', link: '#/model/customers' },
      {
        key: 'upstreamProject',
        data: 'platform_core',
        link: '#/project/platform_core',
        crossProjectLink: true,
      },
    ],
  },
};

/** Long values truncate with a tooltip that only appears once clipped;
 *  `disableTooltip` opts a row out. */
export const TruncatedValues: Story = {
  args: {
    tableEntries: [
      {
        key: 'relation',
        data: 'analytics.dbt_production_marts.int_order_items_joined_to_customers',
      },
      {
        key: 'relationNoTooltip',
        title: 'Relation (tooltip disabled)',
        data: 'analytics.dbt_production_marts.int_order_items_joined_to_customers',
        disableTooltip: true,
      },
    ],
  },
};

/** Empty, null and empty-array values are filtered out, so a caller can hand over
 *  every possible field and let the table decide what to show. This renders two
 *  rows, not five. */
export const EmptyValuesAreDropped: Story = {
  args: {
    tableEntries: [
      { key: 'materialization', data: 'table' },
      { key: 'owner', data: '' },
      { key: 'group', data: null },
      { key: 'tags', data: [] },
      { key: 'packageName', data: 'jaffle_shop' },
    ],
  },
};

/** A `footer` renders inside the same divider wrapper, so it inherits the rhythm.
 *  It also keeps the table alive when there are no entries at all. */
export const WithFooter: Story = {
  args: {
    footer: <div className="my-5 text-sm text-fgBrand">View all 12 fields</div>,
  },
};

export const Loading: Story = {
  args: { isLoading: true },
};

/** No entries and no footer renders nothing — the detail page relies on this to omit
 *  a metadata block rather than show an empty one. */
export const EmptyRendersNothing: Story = {
  args: { tableEntries: [] },
};
