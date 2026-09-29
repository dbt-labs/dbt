import type { Meta, StoryObj } from '@storybook/react-vite';
import type { ColumnDef } from '@tanstack/react-table';

import { PaginatedTable } from './PaginatedTable';

type Row = { name: string; materialization: string; rows: string };

const COLUMNS: ColumnDef<Row>[] = [
  { id: 'name', header: 'Name', accessorFn: (r) => r.name },
  {
    id: 'materialization',
    header: 'Materialization',
    accessorFn: (r) => r.materialization,
  },
  { id: 'rows', header: 'Row count', accessorFn: (r) => r.rows },
];

const NAMES = [
  'customers',
  'orders',
  'order_items',
  'products',
  'stg_customers',
  'stg_orders',
  'stg_products',
  'fct_orders',
  'dim_customers',
  'supplies',
  'locations',
  'returns',
];

function rows(count: number): Row[] {
  return Array.from({ length: count }, (_, i) => ({
    name: `${NAMES[i % NAMES.length]}_${i}`,
    materialization: i % 3 === 0 ? 'view' : 'table',
    rows: (1_000 * (i + 1)).toLocaleString(),
  }));
}

const meta: Meta<typeof PaginatedTable> = {
  component: PaginatedTable,
  decorators: [(Story) => <div className="w-[840px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof PaginatedTable>;

/** The default `loadMore` mode: a footer showing "Loaded n of m" plus a load-more
 *  control. Height is derived from `pageSize` and the row count. */
export const LoadMore: Story = {
  render: () => (
    <PaginatedTable<Row>
      columns={COLUMNS}
      data={rows(10)}
      isLoading={false}
      maxResultCount={24}
      pageSize={10}
      paginationType="loadMore"
      pagination={{ hasMorePages: true, onLoadMore: () => {}, isPageLoading: false }}
    />
  ),
};

/** Loading a further page keeps the existing rows and appends skeletons, rather than
 *  replacing the body — which would reset scroll to the top. */
export const LoadingMorePages: Story = {
  render: () => (
    <PaginatedTable<Row>
      columns={COLUMNS}
      data={rows(10)}
      isLoading={false}
      maxResultCount={24}
      pageSize={10}
      paginationType="loadMore"
      pagination={{ hasMorePages: true, onLoadMore: () => {}, isPageLoading: true }}
    />
  ),
};

/** The initial fetch, with no rows yet: the whole body is skeletons. */
export const InitialLoading: Story = {
  render: () => (
    <PaginatedTable<Row>
      columns={COLUMNS}
      data={[]}
      isLoading
      maxResultCount={undefined}
      pageSize={10}
      paginationType="loadMore"
      pagination={{ hasMorePages: false, onLoadMore: () => {}, isPageLoading: false }}
    />
  ),
};

/** Fewer rows than a page: the table shrinks to fit instead of padding to `pageSize`. */
export const PartialPage: Story = {
  render: () => (
    <PaginatedTable<Row>
      columns={COLUMNS}
      data={rows(3)}
      isLoading={false}
      maxResultCount={3}
      pageSize={10}
      paginationType="loadMore"
      pagination={{ hasMorePages: false, onLoadMore: () => {}, isPageLoading: false }}
    />
  ),
};

/** An unknown total drops the "Loaded n of m" label — the source could not say how
 *  many rows exist. */
export const UnknownTotal: Story = {
  render: () => (
    <PaginatedTable<Row>
      columns={COLUMNS}
      data={rows(10)}
      isLoading={false}
      maxResultCount={undefined}
      pageSize={10}
      paginationType="loadMore"
      pagination={{ hasMorePages: true, onLoadMore: () => {}, isPageLoading: false }}
    />
  ),
};

/** Empty gets a fixed four-row-tall body so the page does not collapse. */
export const Empty: Story = {
  render: () => (
    <PaginatedTable<Row>
      columns={COLUMNS}
      data={[]}
      isLoading={false}
      maxResultCount={0}
      pageSize={10}
      paginationType="loadMore"
      pagination={{ hasMorePages: false, onLoadMore: () => {}, isPageLoading: false }}
      emptyStateProps={{ header: 'No models match the current filters.' }}
    />
  ),
};

/** The other mode: numbered pages, with the caller owning the page index. */
export const PageBased: Story = {
  render: () => (
    <PaginatedTable<Row>
      columns={COLUMNS}
      data={rows(10)}
      isLoading={false}
      maxResultCount={42}
      pageSize={10}
      paginationType="pageBased"
      currentPage={0}
      setCurrentPage={() => {}}
    />
  ),
};

export const Sortable: Story = {
  render: () => (
    <PaginatedTable<Row>
      columns={COLUMNS}
      data={rows(10)}
      isLoading={false}
      maxResultCount={24}
      pageSize={10}
      paginationType="loadMore"
      pagination={{ hasMorePages: true, onLoadMore: () => {}, isPageLoading: false }}
      isSortable
      initialSortColumn="name"
    />
  ),
};
