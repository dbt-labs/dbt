import type { ReactNode } from 'react';
import type { ColumnDef, SortingState } from '@tanstack/react-table';

import type { AssetFilter, AssetSummary, ListSort, ResourceType } from '../shared';
import type { Project } from '../shared';
import {
  getResultCountString,
  SimpleLinkBreadcrumbs,
  useAssetList,
  useResourceLink,
} from '../shared';
import { ResourceFilterTable } from './ResourceFilterTable';

export interface GenericFilterViewProps<T extends AssetSummary> {
  label: string;
  project: Project;
  /** Single resource type listed by this view. */
  resourceType: ResourceType;
  columns: ColumnDef<T>[];
  emptyMessage?: string;
  /** Filter controls (e.g. facet dropdowns) rendered in the toolbar row above
   *  the table. */
  filterControls?: ReactNode;
  /** Extra list filter merged with `resourceTypes` — e.g. selected facet
   *  values. */
  filter?: Omit<AssetFilter, 'resourceTypes'>;
  /** Server-side sort threaded into the list fetch. Controlled by the caller;
   *  mirrors {@link ResourceFilterTable}'s sort props below. */
  sort?: ListSort;
  isSortable?: boolean;
  initialSortColumn?: string;
  initialSortDesc?: boolean;
  onChangeSort?(sortBy: SortingState): void;
}

/** Deep module: owns useAssetList + breadcrumb + heading + table. The per-type
 *  *FilterView components are thin adapters over this interface, supplying
 *  facet dropdown UI via `filterControls` and the selected values via
 *  `filter`. */
export function GenericFilterView<T extends AssetSummary>({
  label,
  project,
  resourceType,
  columns,
  emptyMessage,
  filterControls,
  filter,
  sort,
  isSortable,
  initialSortColumn,
  initialSortDesc,
  onChangeSort,
}: GenericFilterViewProps<T>) {
  const links = useResourceLink();
  const {
    data,
    total,
    isPending,
    isFetchingNextPage,
    hasNextPage,
    errorMessage,
    fetchNextPage,
  } = useAssetList<T>(
    { filter: { ...filter, resourceTypes: [resourceType] }, sort },
    label.toLowerCase(),
  );

  return (
    <div className="flex w-full flex-col gap-5 px-8 pb-20 pt-6 text-fgMain">
      <SimpleLinkBreadcrumbs
        className="font-caption mb-3 block text-fgDecorative"
        breadcrumbs={[{ text: project.name, href: links.home() }, { text: label }]}
      />

      <header className="flex items-baseline justify-between">
        <h1 className="m-0 text-2xl font-bold leading-tight text-fgMain">{label}</h1>
        {total != null && (
          <span className="text-base font-medium text-fgMain">
            {getResultCountString(total)}
          </span>
        )}
      </header>

      {filterControls && (
        <div className="relative z-30 flex flex-wrap gap-3">{filterControls}</div>
      )}

      <ResourceFilterTable
        columns={columns}
        data={data}
        isLoading={isPending || isFetchingNextPage}
        hasMore={hasNextPage}
        onLoadMore={fetchNextPage}
        total={total}
        shownCount={data.length}
        emptyMessage={emptyMessage}
        error={errorMessage}
        isSortable={isSortable}
        initialSortColumn={initialSortColumn}
        initialSortDesc={initialSortDesc}
        onChangeSort={onChangeSort}
      />
    </div>
  );
}
