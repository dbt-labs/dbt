import { useMemo, useState } from 'react';
import { type ColumnDef } from '@tanstack/react-table';

import { useAllNodes } from '../hooks/useAllNodes';
import { makeNameCell } from '../lib/nameCell';
import type { Project } from '../shared';
import {
  asCellRenderer,
  FilterDropdown,
  SimpleLinkBreadcrumbs,
  TruncatedCell,
  useResourceLink,
} from '../shared';
import { type NodeSummary } from '../types';
import { ResourceFilterTable } from './ResourceFilterTable';
import type { DropdownOption } from './ui/DropdownButton';

interface Props {
  project: Project;
  onPeek(uniqueId: string): void;
}

const ALL_PKG: DropdownOption = { label: 'All', value: '' };

export function AnalysisFilterView({ project, onPeek }: Props) {
  const links = useResourceLink();
  const [selectedPackage, setSelectedPackage] = useState('');
  // Analyses have no list query of their own — they never did, on either side of the
  // API. The whole node index is already in memory, and there are few enough analyses
  // in any project that filtering it here beats a dedicated projection.
  const { nodes: allNodes, isPending, error } = useAllNodes();
  const analyses = useMemo(
    () => (allNodes ?? []).filter((n) => n.resource_type === 'analysis'),
    [allNodes],
  );
  const nodes = useMemo(
    () =>
      selectedPackage
        ? analyses.filter((n) => n.package_name === selectedPackage)
        : analyses,
    [analyses, selectedPackage],
  );

  const pkgOptions = useMemo<DropdownOption[]>(() => {
    const values = [
      ...new Set(analyses.map((n) => n.package_name).filter(Boolean)),
    ] as string[];
    return [ALL_PKG, ...values.map((v) => ({ label: v, value: v }))];
  }, [analyses]);

  const columns = useMemo<ColumnDef<NodeSummary>[]>(
    () => [
      makeNameCell<NodeSummary>(onPeek, (r) => r.unique_id),
      {
        id: 'package',
        header: 'Package',
        size: 160,
        accessorFn: (row) => row.package_name ?? '',
        cell: asCellRenderer<NodeSummary>(TruncatedCell),
      },
      {
        id: 'description',
        header: 'Description',
        accessorFn: (row) => row.description ?? '',
        cell: asCellRenderer<NodeSummary>(TruncatedCell),
      },
    ],
    [onPeek],
  );

  const pkgSelected = pkgOptions.find((o) => o.value === selectedPackage) ?? ALL_PKG;

  return (
    <div className="flex w-full flex-col gap-5 px-8 pb-20 pt-6 text-fgMain">
      <SimpleLinkBreadcrumbs
        className="font-caption mb-3 block text-fgDecorative"
        breadcrumbs={[{ text: project.name, href: links.home() }, { text: 'Analyses' }]}
      />

      <header>
        <h1 className="m-0 text-2xl font-bold leading-tight text-fgMain">Analyses</h1>
      </header>

      <div className="relative z-20 flex flex-wrap items-center gap-2">
        <FilterDropdown
          name="Package"
          options={pkgOptions}
          defaultOption={pkgSelected}
          onChange={(opt) => setSelectedPackage(String(opt.value))}
        />
      </div>

      <ResourceFilterTable
        columns={columns}
        data={nodes}
        isLoading={isPending}
        hasMore={false}
        onLoadMore={() => {}}
        total={nodes.length}
        shownCount={nodes.length}
        emptyMessage="No analyses found."
        error={error ? 'Failed to load analyses.' : null}
      />
    </div>
  );
}
