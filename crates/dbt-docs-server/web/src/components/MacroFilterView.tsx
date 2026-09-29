import { useMemo, useState } from 'react';
import { type ColumnDef } from '@tanstack/react-table';

import { facetOptions, selectedFacetOption } from '../lib/facetOptions';
import { makeNameCell } from '../lib/nameCell';
import type { MacroSummary } from '../shared';
import type { Project } from '../shared';
import { asCellRenderer, FilterDropdown, TruncatedCell, useFacets } from '../shared';
import { GenericFilterView } from './GenericFilterView';

interface Props {
  project: Project;
  onPeek(uniqueId: string): void;
}

export function MacroFilterView({ project, onPeek }: Props) {
  const [pkg, setPkg] = useState('');
  const { data: facets } = useFacets('macro');
  const packageOptions = facetOptions(facets?.packages);

  const columns = useMemo<ColumnDef<MacroSummary>[]>(
    () => [
      makeNameCell(onPeek, (r) => r.uniqueId),
      {
        id: 'arguments',
        header: 'Arguments',
        size: 240,
        accessorFn: (row) => (row.arguments ?? []).join(', ') || '',
        cell: asCellRenderer<MacroSummary>(TruncatedCell),
      },
      {
        id: 'description',
        header: 'Description',
        accessorFn: (row) => row.description ?? '',
        cell: asCellRenderer<MacroSummary>(TruncatedCell),
      },
    ],
    [onPeek],
  );

  return (
    <GenericFilterView
      label="Macros"
      project={project}
      resourceType="macro"
      columns={columns}
      emptyMessage="No macros found."
      filter={{ packages: pkg ? [pkg] : undefined }}
      filterControls={
        <FilterDropdown
          name="Package"
          options={packageOptions}
          defaultOption={selectedFacetOption(packageOptions, pkg)}
          onChange={(opt) => setPkg(String(opt.value))}
        />
      }
    />
  );
}
