export interface PaginationProps {
  /** 0-indexed. */
  currentPage: number;
  onPageChange: (page: number) => void;
  /** 0-indexed — the last valid page number, not a count. */
  totalRows: number;
  rowsPerPage?: number;
}

export function Pagination({
  currentPage,
  onPageChange,
  totalRows,
  rowsPerPage = 1,
}: PaginationProps) {
  const totalPages = Math.max(1, Math.ceil((totalRows + 1) / rowsPerPage));
  if (totalPages <= 1) return null;

  return (
    <div className="flex items-center justify-center gap-3 text-sm text-fgMain">
      <button
        type="button"
        disabled={currentPage <= 0}
        onClick={() => onPageChange(currentPage - 1)}
        className="rounded-md px-2 py-1 hover:bg-bgMainHover disabled:pointer-events-none disabled:opacity-50"
      >
        Previous
      </button>
      <span className="text-fgDecorative">
        Page {currentPage + 1} of {totalPages}
      </span>
      <button
        type="button"
        disabled={currentPage >= totalPages - 1}
        onClick={() => onPageChange(currentPage + 1)}
        className="rounded-md px-2 py-1 hover:bg-bgMainHover disabled:pointer-events-none disabled:opacity-50"
      >
        Next
      </button>
    </div>
  );
}
