import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import { storyColumns } from '../testing/storyFixtures';
import { type ColumnItem, ColumnsView } from './ColumnsView';

const COLUMNS: ColumnItem[] = storyColumns().map((column) => ({
  name: column.name,
  type: column.dataType,
  description: column.description,
  isPrimaryKey: column.isPrimaryKey ?? false,
}));

const meta: Meta<typeof ColumnsView> = {
  component: ColumnsView,
  args: { columns: COLUMNS },
  decorators: [(Story) => <div className="max-w-2xl">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof ColumnsView>;

/** The search box filters on name, type *and* description — type "varchar" or
 *  "surrogate" and the list narrows. */
export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const search = canvas.getByPlaceholderText('Search for columns');

    await expect(canvas.getByLabelText('customer_id')).toBeInTheDocument();
    await expect(canvas.getByLabelText('lifetime_value')).toBeInTheDocument();

    // Matching on name.
    await userEvent.type(search, 'lifetime');
    await expect(canvas.getByLabelText('lifetime_value')).toBeInTheDocument();
    await expect(canvas.queryByLabelText('customer_id')).toBeNull();

    // Matching on description, not name — `customer_id` has no "surrogate" in its
    // name, so this only passes because the filter reads descriptions too.
    await userEvent.clear(search);
    await userEvent.type(search, 'surrogate');
    await expect(canvas.getByLabelText('customer_id')).toBeInTheDocument();
    await expect(canvas.queryByLabelText('lifetime_value')).toBeNull();

    // And on type.
    await userEvent.clear(search);
    await userEvent.type(search, 'bigint');
    await expect(canvas.getByLabelText('number_of_orders')).toBeInTheDocument();
    await expect(canvas.queryByLabelText('customer_id')).toBeNull();

    await userEvent.clear(search);
    await userEvent.type(search, 'zzzz');
    await expect(canvas.getByText('No columns found.')).toBeVisible();
  },
};

/** Search hidden, for panels that are already scoped to a handful of columns. */
export const WithoutSearch: Story = {
  args: { disableSearch: true },
};

/** Each row gets a caret revealing `renderExpanded` — this is the column-lineage
 *  affordance on the model detail page. */
export const Expandable: Story = {
  args: {
    expandable: true,
    renderExpanded: (col) => (
      <div className="border-t border-borderMuted px-4 py-3 text-sm text-fgAlt">
        Upstream of <span className="font-medium text-fgMain">{col.name}</span>:
        stg_customers.{col.name}
      </div>
    ),
  },
};

export const WithConstraints: Story = {
  args: {
    columns: COLUMNS.map((col, i) => ({
      ...col,
      constraints: i === 0 ? [{ name: 'not_null' }, { type: 'unique' }] : undefined,
    })),
  },
};

/** `renderItem` replaces the row entirely — how a consumer swaps in its own card
 *  without losing the search box. */
export const CustomRenderItem: Story = {
  args: {
    renderItem: (col, query) => (
      <li
        key={col.name}
        className="rounded border border-borderMuted px-3 py-2 text-sm text-fgMain"
      >
        {col.name}
        {query && <span className="ml-2 text-fgDecorative">matched “{query}”</span>}
      </li>
    ),
  },
};

export const Loading: Story = {
  args: { isLoading: true },
};

/** No columns at all — the case where the catalog was never read. */
export const NoColumns: Story = {
  args: { columns: [] },
};

/** A caller-supplied empty state. The docs site uses this to explain *why* there are
 *  no columns rather than just stating that there are none. */
export const CustomEmptyState: Story = {
  args: {
    columns: [],
    emptyState: (
      <div className="rounded-lg border border-borderMuted p-6 text-center text-fgAlt">
        No column metadata in this docs site. Re-run with
        <code className="mx-1">--static-analysis strict</code> to include it.
      </div>
    ),
  },
};
