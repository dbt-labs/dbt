import type { ColumnDef, SortingState } from '@tanstack/react-table';

import { type DefaultTData, PaginatedTable } from '../shared';

interface ResourceFilterTableProps<T extends DefaultTData> {
  columns: ColumnDef<T>[];
  data: T[];
  isLoading: boolean;
  hasMore: boolean;
  onLoadMore(): void;
  total: number | null;
  shownCount?: number;
  emptyMessage?: string;
  error?: string | null;
  isSortable?: boolean;
  initialSortColumn?: string;
  initialSortDesc?: boolean;
  onChangeSort?(sortBy: SortingState): void;
}

const PAGE_SIZE = 10;

export function ResourceFilterTable<T extends DefaultTData>({
  columns,
  data,
  isLoading,
  hasMore,
  onLoadMore,
  total,
  error,
  emptyMessage,
  isSortable,
  initialSortColumn,
  initialSortDesc,
  onChangeSort,
}: ResourceFilterTableProps<T>) {
  if (error) {
    return (
      <p className="m-0 rounded-lg border border-borderMuted bg-bgMain p-8 text-center text-fgAlt">
        {error}
      </p>
    );
  }

  return (
    <PaginatedTable
      columns={columns}
      data={data}
      // Only collapse the whole table to skeletons on the initial fetch (no
      // rows yet). During load-more we keep existing rows and let
      // `pagination.isPageLoading` append skeletons at the bottom — passing
      // `isLoading` here too would replace the body and reset scroll to top.
      isLoading={isLoading && data.length === 0}
      maxResultCount={total ?? undefined}
      pageSize={PAGE_SIZE}
      paginationType="loadMore"
      pagination={{
        hasMorePages: hasMore,
        onLoadMore,
        isPageLoading: isLoading,
      }}
      isSortable={isSortable}
      initialSortColumn={initialSortColumn}
      initialSortDesc={initialSortDesc}
      onChangeSort={onChangeSort}
      emptyStateProps={{
        header: emptyMessage ?? 'No resources match the current filters.',
      }}
    />
  );
}
