import { useMemo } from 'react';
import { type ColumnDef } from '@tanstack/react-table';
import prettyBytes from 'pretty-bytes';

import { makeNameCell } from '../lib/nameCell';
import type {
  ExposureSummary,
  GroupSummary,
  MetricSummary,
  SavedQuerySummary,
  SeedSummary,
  SemanticModelSummary,
  SnapshotSummary,
} from '../shared';
import type { Project } from '../shared';
import {
  asCellRenderer,
  RightAlignedTruncatedCell,
  TimestampCell,
  TruncatedCell,
} from '../shared';
import { GenericFilterView } from './GenericFilterView';

interface FilterViewProps {
  project: Project;
  onPeek(uniqueId: string): void;
}

const ADAPTERS_WITH_LAST_MODIFIED_STAT = new Set<string>(['snowflake']);

export function SeedFilterView({ project, onPeek }: FilterViewProps) {
  const columns = useMemo<ColumnDef<SeedSummary>[]>(
    () => [
      makeNameCell(onPeek, (r) => r.uniqueId),
      {
        id: 'row_count',
        header: 'Row count',
        size: 160,
        accessorFn: (row) =>
          row.rowCount != null ? row.rowCount.toLocaleString() : '',
        cell: asCellRenderer<SeedSummary>(TruncatedCell),
      },
      {
        id: 'executed_at',
        header: 'Last executed',
        accessorFn: (row) => row.executedAt,
        cell: asCellRenderer<SeedSummary>(TimestampCell),
      },
    ],
    [onPeek],
  );
  return (
    <GenericFilterView
      label="Seeds"
      project={project}
      resourceType="seed"
      columns={columns}
      emptyMessage="No seeds found."
    />
  );
}

export function ExposureFilterView({ project, onPeek }: FilterViewProps) {
  const columns = useMemo<ColumnDef<ExposureSummary>[]>(
    () => [
      makeNameCell(onPeek, (r) => r.uniqueId),
      {
        id: 'exposure_type',
        header: 'Type',
        size: 140,
        accessorFn: (row) => row.exposureType ?? '',
        cell: asCellRenderer<ExposureSummary>(TruncatedCell),
      },
      {
        id: 'owner',
        header: 'Owner',
        accessorFn: (row) => row.ownerName ?? '',
        cell: asCellRenderer<ExposureSummary>(TruncatedCell),
      },
      {
        id: 'owner_email',
        header: 'Owner email',
        accessorFn: (row) => row.ownerEmail ?? '',
        cell: asCellRenderer<ExposureSummary>(TruncatedCell),
      },
    ],
    [onPeek],
  );
  return (
    <GenericFilterView
      label="Exposures"
      project={project}
      resourceType="exposure"
      columns={columns}
      emptyMessage="No exposures found."
    />
  );
}

export function MetricFilterView({ project, onPeek }: FilterViewProps) {
  const columns = useMemo<ColumnDef<MetricSummary>[]>(
    () => [
      makeNameCell(onPeek, (r) => r.uniqueId),
      {
        id: 'metric_type',
        header: 'Type',
        size: 140,
        accessorFn: (row) => row.metricType ?? '',
        cell: asCellRenderer<MetricSummary>(TruncatedCell),
      },
      {
        id: 'description',
        header: 'Description',
        accessorFn: (row) => row.description ?? '',
        cell: asCellRenderer<MetricSummary>(TruncatedCell),
      },
    ],
    [onPeek],
  );
  return (
    <GenericFilterView
      label="Metrics"
      project={project}
      resourceType="metric"
      columns={columns}
      emptyMessage="No metrics found."
    />
  );
}

export function GroupFilterView({ project, onPeek }: FilterViewProps) {
  const columns = useMemo<ColumnDef<GroupSummary>[]>(
    () => [
      makeNameCell(onPeek, (r) => r.uniqueId),
      {
        id: 'model_count',
        header: 'Model count',
        accessorFn: (row) => String(row.modelCount ?? 0),
        cell: asCellRenderer<GroupSummary>(RightAlignedTruncatedCell),
      },
      {
        id: 'owner_name',
        header: 'Owner name',
        accessorFn: (row) => row.ownerName ?? '',
        cell: asCellRenderer<GroupSummary>(TruncatedCell),
      },
      {
        id: 'owner_email',
        header: 'Owner email',
        accessorFn: (row) => row.ownerEmail ?? '',
        cell: asCellRenderer<GroupSummary>(TruncatedCell),
      },
      {
        id: 'owner_github',
        header: 'Owner GitHub',
        accessorFn: (row) => row.ownerGithub ?? '',
        cell: asCellRenderer<GroupSummary>(TruncatedCell),
      },
      {
        id: 'owner_slack',
        header: 'Owner Slack',
        accessorFn: (row) => row.ownerSlack ?? '',
        cell: asCellRenderer<GroupSummary>(TruncatedCell),
      },
    ],
    [onPeek],
  );
  return (
    <GenericFilterView
      label="Groups"
      project={project}
      resourceType="group"
      columns={columns}
      emptyMessage="No groups found."
    />
  );
}

export function SemanticModelFilterView({ project, onPeek }: FilterViewProps) {
  const columns = useMemo<ColumnDef<SemanticModelSummary>[]>(
    () => [
      makeNameCell(onPeek, (r) => r.uniqueId),
      {
        id: 'entities',
        header: 'Entities',
        size: 200,
        accessorFn: (row) => (row.entities ?? []).join(', ') || '',
        cell: asCellRenderer<SemanticModelSummary>(TruncatedCell),
      },
      {
        id: 'description',
        header: 'Description',
        accessorFn: (row) => row.description ?? '',
        cell: asCellRenderer<SemanticModelSummary>(TruncatedCell),
      },
    ],
    [onPeek],
  );
  return (
    <GenericFilterView
      label="Semantic models"
      project={project}
      resourceType="semantic_model"
      columns={columns}
      emptyMessage="No semantic models found."
    />
  );
}

export function SavedQueryFilterView({ project, onPeek }: FilterViewProps) {
  const columns = useMemo<ColumnDef<SavedQuerySummary>[]>(
    () => [
      makeNameCell(onPeek, (r) => r.uniqueId),
      {
        id: 'description',
        header: 'Description',
        accessorFn: (row) => row.description ?? '',
        cell: asCellRenderer<SavedQuerySummary>(TruncatedCell),
      },
    ],
    [onPeek],
  );
  return (
    <GenericFilterView
      label="Saved queries"
      project={project}
      resourceType="saved_query"
      columns={columns}
      emptyMessage="No saved queries found."
    />
  );
}

export function SnapshotFilterView({ project, onPeek }: FilterViewProps) {
  const columns = useMemo<ColumnDef<SnapshotSummary>[]>(() => {
    const cols: ColumnDef<SnapshotSummary>[] = [
      makeNameCell(onPeek, (r) => r.uniqueId),
      {
        id: 'row_count',
        header: 'Row count',
        size: 160,
        accessorFn: (row) =>
          row.rowCountStat != null ? row.rowCountStat.toLocaleString() : '',
        cell: asCellRenderer<SnapshotSummary>(TruncatedCell),
      },
      {
        id: 'size',
        header: 'Size',
        size: 140,
        accessorFn: (row) => (row.bytesStat != null ? prettyBytes(row.bytesStat) : ''),
        cell: asCellRenderer<SnapshotSummary>(TruncatedCell),
      },
    ];
    if (
      project.adapterType &&
      ADAPTERS_WITH_LAST_MODIFIED_STAT.has(project.adapterType)
    ) {
      cols.push({
        id: 'last_modified',
        header: 'Last modified',
        accessorFn: (row) => row.lastModifiedStat ?? null,
        cell: asCellRenderer<SnapshotSummary>(TimestampCell),
      });
    }
    return cols;
  }, [onPeek, project.adapterType]);

  return (
    <GenericFilterView
      label="Snapshots"
      project={project}
      resourceType="snapshot"
      columns={columns}
      emptyMessage="No snapshots found."
    />
  );
}
