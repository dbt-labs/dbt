import type { ColumnDef } from '@tanstack/react-table';

import { Tooltip } from '../components/ui/Tooltip';

/** Factory for the standard resource-list name column: truncating tooltip + peek button.
 *  Replaces the 15-line inline cell block duplicated across every *FilterView.
 *  `getId` reads the row's unique id — domain summaries expose `uniqueId`, the
 *  nodes-backed views' `NodeSummary` exposes `unique_id`.
 *  No resource-type icon here by design — matches prod (dbt Explorer), which
 *  doesn't show one in the Name column either. */
export function makeNameCell<T extends { name: string }>(
  onPeek: (id: string) => void,
  getId: (row: T) => string,
  opts?: { enableSorting?: boolean },
): ColumnDef<T> {
  return {
    id: 'name',
    header: 'Name',
    size: 280,
    accessorFn: (row) => row.name,
    enableSorting: opts?.enableSorting,
    cell: (info) => (
      <div className="flex min-w-0 items-center gap-2">
        <Tooltip
          displayOnlyWhenTruncated
          content={info.row.original.name}
          childrenAreInteractive
          placement="top-start"
          className="min-w-0"
        >
          {(ref) => (
            <div ref={ref} className="min-w-0 truncate">
              <button
                type="button"
                className="cursor-pointer border-0 bg-transparent p-0 text-left text-fgBrand underline decoration-transparent underline-offset-[3px] transition-[text-decoration-color] duration-[120ms] hover:decoration-fgBrand"
                onClick={() => onPeek(getId(info.row.original))}
              >
                {info.row.original.name}
              </button>
            </div>
          )}
        </Tooltip>
      </div>
    ),
  };
}
