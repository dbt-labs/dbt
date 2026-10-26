import { useState } from 'react';
import {
  type ColumnDef,
  flexRender,
  getCoreRowModel,
  type SortingState,
  useReactTable,
} from '@tanstack/react-table';
import { ArrowDown, ArrowUp, ArrowUpDown } from 'lucide-react';

import { LoadingBlock } from '../../components/ui/LoadingBlock';
import { Pagination } from '../../components/ui/Pagination';
import {
  PaginationFooter,
  type PaginationFooterProps,
} from '../../components/ui/PaginationFooter';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '../../components/ui/Table';

export type DefaultTData = object & {
  isLoadingRow?: boolean;
  testId?: string;
};

type PaginatedRowData = DefaultTData & {
  href?: string;
};

export type PaginationType = 'loadMore' | 'pageBased';

interface EmptyStateProps {
  header: string;
}

interface BasePaginatedTableProps<TData extends PaginatedRowData> {
  /** The list of all results */
  data: TData[];
  /** If true, we are loading the initial set of data */
  isLoading: boolean;
  /** An estimate or exact count of results that could be loaded in the future */
  maxResultCount: number | undefined;
  /** Number of results per page */
  pageSize: number;
  /** ColumnDefs to display in the table's heading */
  columns: ColumnDef<TData>[];
  /** Configuration for the empty state, shown when no rows are provided */
  emptyStateProps?: EmptyStateProps;
  /** A flag to indicate the table is sortable */
  isSortable?: boolean;
  /** The starting sort column */
  initialSortColumn?: string;
  /** Whether the initial sort is descending or ascending */
  initialSortDesc?: boolean;
  /** Callback to handle sort events */
  onChangeSort?(sortBy: SortingState): void;
}

interface PaginatedTableProps<
  TData extends PaginatedRowData,
> extends BasePaginatedTableProps<TData> {
  /** Type of pagination to use */
  paginationType?: PaginationType;
  /** Parameters for controlling load more pagination */
  pagination?: PaginationFooterProps;
  /** Current page number (0-based) for page-based pagination */
  currentPage?: number;
  /** Function to set the current page for page-based pagination */
  setCurrentPage?: (page: number) => void;
}

export const PaginatedTable = <TData extends object>({
  paginationType = 'loadMore',
  pagination,
  currentPage,
  setCurrentPage,
  ...props
}: PaginatedTableProps<TData>) => {
  if (paginationType === 'loadMore') {
    if (!pagination) {
      throw new Error('Pagination props are required for loadMore pagination type');
    }
    return <LoadMorePaginatedTable pagination={pagination} {...props} />;
  }
  if (currentPage === undefined || !setCurrentPage) {
    throw new Error(
      'currentPage and setCurrentPage are required for pageBased pagination type',
    );
  }
  return (
    <PageBasedPaginatedTable
      currentPage={currentPage}
      setCurrentPage={setCurrentPage}
      {...props}
    />
  );
};

interface LoadMorePaginatedTableProps<
  TData extends PaginatedRowData,
> extends BasePaginatedTableProps<TData> {
  /** Parameters for controlling pagination */
  pagination: PaginationFooterProps;
}

const LoadMorePaginatedTable = <TData extends object>({
  pagination,
  data,
  isLoading,
  maxResultCount,
  pageSize,
  columns,
  ...rest
}: LoadMorePaginatedTableProps<TData>) => {
  return (
    <>
      <DataTable
        data={data}
        isLoading={isLoading}
        loadingRowCount={pageSize}
        columns={columns}
        {...rest}
      />
      <div className="flex flex-col items-center gap-1 py-1">
        <PaginationFooter {...pagination} />
        {maxResultCount != null && (
          <p role="status" className="m-0 text-xs text-fgAlt">
            Loaded {data.length} of {maxResultCount}
          </p>
        )}
      </div>
    </>
  );
};

interface PageBasedPaginatedTableProps<
  TData extends PaginatedRowData,
> extends BasePaginatedTableProps<TData> {
  /** Current page number (0-based) */
  currentPage: number;
  /** Function to set the current page */
  setCurrentPage: (page: number) => void;
}

const PageBasedPaginatedTable = <TData extends object>({
  data,
  isLoading,
  maxResultCount,
  pageSize,
  columns,
  currentPage,
  setCurrentPage,
  ...rest
}: PageBasedPaginatedTableProps<TData>) => {
  return (
    <>
      <DataTable
        data={data}
        isLoading={isLoading}
        loadingRowCount={pageSize}
        columns={columns}
        {...rest}
      />
      <Pagination
        onPageChange={setCurrentPage}
        totalRows={maxResultCount ?? 0}
        currentPage={currentPage}
        rowsPerPage={pageSize}
      />
    </>
  );
};

interface DataTableProps<TData extends object> {
  data: TData[];
  columns: ColumnDef<TData>[];
  isLoading: boolean;
  loadingRowCount: number;
  emptyStateProps?: EmptyStateProps;
  isSortable?: boolean;
  initialSortColumn?: string;
  initialSortDesc?: boolean;
  onChangeSort?(sortBy: SortingState): void;
}

function DataTable<TData extends object>({
  data,
  columns,
  isLoading,
  loadingRowCount,
  emptyStateProps,
  isSortable,
  initialSortColumn,
  initialSortDesc,
  onChangeSort,
}: DataTableProps<TData>) {
  const [sorting, setSorting] = useState<SortingState>(
    initialSortColumn ? [{ id: initialSortColumn, desc: !!initialSortDesc }] : [],
  );

  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack's own API, not memoizable
  const table = useReactTable({
    data,
    columns,
    state: { sorting },
    manualSorting: true,
    defaultColumn: { enableSorting: false },
    onSortingChange: (updater) => {
      const next = typeof updater === 'function' ? updater(sorting) : updater;
      setSorting(next);
      onChangeSort?.(next);
    },
    getCoreRowModel: getCoreRowModel(),
  });

  if (isLoading && data.length === 0) {
    return (
      <Table>
        <TableHeader>
          {table.getHeaderGroups().map((headerGroup) => (
            <TableRow key={headerGroup.id}>
              {headerGroup.headers.map((header) => (
                <TableHead key={header.id}>
                  {header.isPlaceholder
                    ? null
                    : flexRender(header.column.columnDef.header, header.getContext())}
                </TableHead>
              ))}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody>
          {Array.from({ length: loadingRowCount }).map((_, rowIdx) => (
            <TableRow key={rowIdx}>
              {columns.map((column, colIdx) => (
                <TableCell key={column.id ?? colIdx}>
                  <LoadingBlock />
                </TableCell>
              ))}
            </TableRow>
          ))}
        </TableBody>
      </Table>
    );
  }

  return (
    <Table>
      <TableHeader>
        {table.getHeaderGroups().map((headerGroup) => (
          <TableRow key={headerGroup.id}>
            {headerGroup.headers.map((header) => {
              const canSort = isSortable && header.column.getCanSort();
              const sortDirection = header.column.getIsSorted();
              return (
                <TableHead key={header.id}>
                  {header.isPlaceholder ? null : canSort ? (
                    <button
                      type="button"
                      className="inline-flex items-center gap-1 border-0 bg-transparent p-0 font-medium text-fgAlt"
                      onClick={header.column.getToggleSortingHandler()}
                    >
                      {flexRender(header.column.columnDef.header, header.getContext())}
                      {(() => {
                        const SortIcon =
                          sortDirection === 'asc'
                            ? ArrowUp
                            : sortDirection === 'desc'
                              ? ArrowDown
                              : ArrowUpDown;
                        const label =
                          sortDirection === 'asc'
                            ? 'sort ascending'
                            : sortDirection === 'desc'
                              ? 'sort descending'
                              : 'sortable';
                        return <SortIcon className="size-3" aria-label={label} />;
                      })()}
                    </button>
                  ) : (
                    flexRender(header.column.columnDef.header, header.getContext())
                  )}
                </TableHead>
              );
            })}
          </TableRow>
        ))}
      </TableHeader>
      <TableBody>
        {data.length === 0 ? (
          <TableRow>
            <TableCell colSpan={columns.length} className="py-8 text-center text-fgAlt">
              {emptyStateProps?.header ?? 'No results.'}
            </TableCell>
          </TableRow>
        ) : (
          table.getRowModel().rows.map((row) => (
            <TableRow key={row.id}>
              {row.getVisibleCells().map((cell) => (
                <TableCell key={cell.id}>
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </TableCell>
              ))}
            </TableRow>
          ))
        )}
      </TableBody>
    </Table>
  );
}
