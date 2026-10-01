import { useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { type ColumnDef } from '@tanstack/react-table';

import { facetOptions, selectedFacetOption } from '../lib/facetOptions';
import { makeNameCell } from '../lib/nameCell';
import type { ListSort, ModelSummary } from '../shared';
import type { Project } from '../shared';
import {
  asCellRenderer,
  FilterDropdown,
  TimestampCell,
  TruncatedCell,
  useFacets,
} from '../shared';
import { GenericFilterView } from './GenericFilterView';

const SORT_COLS: Record<string, string> = {
  name: 'name',
  executed_at: 'executed_at',
};

interface Props {
  project: Project;
  onPeek(uniqueId: string): void;
}

/** Thin adapter over {@link GenericFilterView}: adds the model facet dropdowns,
 *  the `?modeling_layer=` deep-link, and server-side sort. The shell
 *  (breadcrumb/header/table) lives in GenericFilterView. */
export function ModelFilterView({ project, onPeek }: Props) {
  const [sort, setSort] = useState<ListSort | undefined>(undefined);

  // Modeling layer lives in the URL so the home "Show marts" CTA
  // (`?modeling_layer=Marts`) seeds the dropdown and the dropdown writes back.
  const [searchParams, setSearchParams] = useSearchParams();
  const layer = searchParams.get('modeling_layer') ?? '';
  const [owner, setOwner] = useState('');
  const [pkg, setPkg] = useState('');

  const { data: facets } = useFacets('model');

  const layerOptions = facetOptions(facets?.modelingLayers);
  const ownerOptions = facetOptions(facets?.owners);
  const packageOptions = facetOptions(facets?.packages);

  const setLayer = (value: string) => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (value) next.set('modeling_layer', value);
        else next.delete('modeling_layer');
        return next;
      },
      { replace: true },
    );
  };

  const filterControls = (
    <>
      <FilterDropdown
        name="Modeling layer"
        options={layerOptions}
        defaultOption={selectedFacetOption(layerOptions, layer)}
        onChange={(opt) => setLayer(String(opt.value))}
      />
      <FilterDropdown
        name="Owner"
        options={ownerOptions}
        defaultOption={selectedFacetOption(ownerOptions, owner)}
        onChange={(opt) => setOwner(String(opt.value))}
      />
      <FilterDropdown
        name="Package"
        options={packageOptions}
        defaultOption={selectedFacetOption(packageOptions, pkg)}
        onChange={(opt) => setPkg(String(opt.value))}
      />
    </>
  );

  const columns = useMemo<ColumnDef<ModelSummary>[]>(
    () => [
      makeNameCell<ModelSummary>(onPeek, (r) => r.uniqueId, {
        enableSorting: true,
      }),
      {
        id: 'modeling_layer',
        header: 'Modeling layer',
        accessorFn: (row) => row.modelingLayer ?? '',
        cell: asCellRenderer<ModelSummary>(TruncatedCell),
      },
      {
        id: 'owner',
        header: 'Owner',
        accessorFn: (row) => row.owner ?? '',
        cell: asCellRenderer<ModelSummary>(TruncatedCell),
      },
      {
        id: 'executed_at',
        header: 'Last executed',
        accessorFn: (row) => row.executedAt,
        enableSorting: true,
        cell: asCellRenderer<ModelSummary>(TimestampCell),
      },
      {
        id: 'row_count',
        header: 'Row count',
        accessorFn: (row) =>
          row.rowCountStat != null ? row.rowCountStat.toLocaleString() : '',
        cell: asCellRenderer<ModelSummary>(TruncatedCell),
      },
    ],
    [onPeek],
  );

  return (
    <GenericFilterView<ModelSummary>
      label="Models"
      project={project}
      resourceType="model"
      columns={columns}
      filter={{
        modelingLayers: layer ? [layer] : undefined,
        owners: owner ? [owner] : undefined,
        packages: pkg ? [pkg] : undefined,
      }}
      filterControls={filterControls}
      sort={sort}
      isSortable
      initialSortColumn="executed_at"
      initialSortDesc
      onChangeSort={(sortBy) => {
        const col = sortBy[0];
        if (!col) {
          setSort(undefined);
          return;
        }
        const field = SORT_COLS[col.id];
        if (!field) {
          setSort(undefined);
          return;
        }
        setSort({ field, desc: col.desc });
      }}
    />
  );
}
