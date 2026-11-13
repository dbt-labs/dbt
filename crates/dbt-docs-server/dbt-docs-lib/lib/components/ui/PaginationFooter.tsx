export interface PaginationFooterProps {
  onLoadMore(): void;
  isPageLoading?: boolean;
  hasMorePages?: boolean;
}

export function PaginationFooter({
  onLoadMore,
  isPageLoading,
  hasMorePages,
}: PaginationFooterProps) {
  if (!hasMorePages) return null;

  return (
    <div className="flex justify-center py-4">
      <button
        type="button"
        onClick={onLoadMore}
        disabled={isPageLoading}
        className="rounded-md border border-borderMain bg-bgMain px-3 py-1.5 text-sm text-fgMain hover:bg-bgMainHover disabled:pointer-events-none disabled:opacity-50"
      >
        {isPageLoading ? 'Loading…' : 'Load more'}
      </button>
    </div>
  );
}
