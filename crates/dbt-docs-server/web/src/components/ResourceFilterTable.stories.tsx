import type { Meta, StoryObj } from '@storybook/react-vite';
import type { ColumnDef } from '@tanstack/react-table';

import { ResourceFilterTable } from './ResourceFilterTable';

type Row = { name: string; layer: string; owner: string };

const COLUMNS: ColumnDef<Row>[] = [
  { id: 'name', header: 'Name', accessorFn: (r) => r.name, enableSorting: true },
  { id: 'layer', header: 'Modeling layer', accessorFn: (r) => r.layer },
  { id: 'owner', header: 'Owner', accessorFn: (r) => r.owner },
];

const ROWS: Row[] = Array.from({ length: 10 }, (_, i) => ({
  name: `model_${i}`,
  layer: ['Staging', 'Intermediate', 'Marts'][i % 3] as string,
  owner: ['data-platform', 'finance-analytics'][i % 2] as string,
}));

const meta: Meta<typeof ResourceFilterTable> = {
  component: ResourceFilterTable,
  decorators: [(Story) => <div className="w-[900px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof ResourceFilterTable>;

export const Default: Story = {
  render: () => (
    <ResourceFilterTable<Row>
      columns={COLUMNS}
      data={ROWS}
      isLoading={false}
      hasMore
      onLoadMore={() => {}}
      total={24}
    />
  ),
};

/** The initial fetch. Note the table only collapses to skeletons when there are *no*
 *  rows yet — see the next story. */
export const InitialLoading: Story = {
  render: () => (
    <ResourceFilterTable<Row>
      columns={COLUMNS}
      data={[]}
      isLoading
      hasMore={false}
      onLoadMore={() => {}}
      total={null}
    />
  ),
};

/** Loading with rows already present appends skeletons instead of replacing the body,
 *  so a load-more does not reset scroll to the top. */
export const LoadingMore: Story = {
  render: () => (
    <ResourceFilterTable<Row>
      columns={COLUMNS}
      data={ROWS}
      isLoading
      hasMore
      onLoadMore={() => {}}
      total={24}
    />
  ),
};

/**
 * An error replaces the table entirely with a single bordered message. This is also
 * how an *unsupported* surface renders — the hooks turn a missing fetcher into an
 * error rather than an empty list, precisely so it does not read as "no resources".
 */
export const LoadError: Story = {
  render: () => (
    <ResourceFilterTable<Row>
      columns={COLUMNS}
      data={[]}
      isLoading={false}
      hasMore={false}
      onLoadMore={() => {}}
      total={null}
      error="Not available in this docs site yet — this view is still being ported to run in the browser."
    />
  ),
};

export const Empty: Story = {
  render: () => (
    <ResourceFilterTable<Row>
      columns={COLUMNS}
      data={[]}
      isLoading={false}
      hasMore={false}
      onLoadMore={() => {}}
      total={0}
    />
  ),
};

export const EmptyWithCustomMessage: Story = {
  render: () => (
    <ResourceFilterTable<Row>
      columns={COLUMNS}
      data={[]}
      isLoading={false}
      hasMore={false}
      onLoadMore={() => {}}
      total={0}
      emptyMessage="No macros found."
    />
  ),
};

export const Sortable: Story = {
  render: () => (
    <ResourceFilterTable<Row>
      columns={COLUMNS}
      data={ROWS}
      isLoading={false}
      hasMore={false}
      onLoadMore={() => {}}
      total={10}
      isSortable
      initialSortColumn="name"
      onChangeSort={() => {}}
    />
  ),
};

/** An unknown total drops the "Loaded n of m" label. */
export const UnknownTotal: Story = {
  render: () => (
    <ResourceFilterTable<Row>
      columns={COLUMNS}
      data={ROWS}
      isLoading={false}
      hasMore
      onLoadMore={() => {}}
      total={null}
    />
  ),
};
