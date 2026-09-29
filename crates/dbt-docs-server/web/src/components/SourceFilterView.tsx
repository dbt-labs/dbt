import { useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { type ColumnDef } from '@tanstack/react-table';

import { paths } from '../routes';
import type { SourceSummary } from '../shared';
import type { Project } from '../shared';
import {
  asCellRenderer,
  FilterDropdown,
  SimpleLinkBreadcrumbs,
  TruncatedCell,
  useAssetList,
  useResourceLink,
} from '../shared';
import { ResourceFilterTable } from './ResourceFilterTable';
import type { DropdownOption } from './ui/DropdownButton';
import { Tooltip } from './ui/Tooltip';

type SourceCollection = {
  sourceName: string;
  totalTables: number;
  database: string | null;
  schema: string | null;
};

const ALL_DB: DropdownOption = { label: 'All', value: '' };
const ALL_SCHEMA: DropdownOption = { label: 'All', value: '' };

function sourceNameOf(s: SourceSummary): string | null {
  return s.sourceName ?? s.uniqueId.split('.')[2] ?? null;
}

interface Props {
  project: Project;
}

export function SourceFilterView({ project }: Props) {
  const links = useResourceLink();
  const {
    data: sources,
    isPending,
    isFetchingNextPage,
    hasNextPage,
    errorMessage,
    fetchNextPage,
  } = useAssetList<SourceSummary>({ filter: { resourceTypes: ['source'] } }, 'sources');
  const [selectedDb, setSelectedDb] = useState('');
  const [selectedSchema, setSelectedSchema] = useState('');

  const collections = useMemo<SourceCollection[]>(() => {
    const map = new Map<string, SourceCollection>();
    for (const n of sources) {
      const sourceName = sourceNameOf(n);
      if (!sourceName) continue;
      const existing = map.get(sourceName);
      if (existing) {
        existing.totalTables += 1;
      } else {
        map.set(sourceName, {
          sourceName,
          totalTables: 1,
          database: n.databaseName ?? null,
          schema: n.schemaName ?? null,
        });
      }
    }
    return Array.from(map.values());
  }, [sources]);

  const dbOptions = useMemo<DropdownOption[]>(() => {
    const values = [
      ...new Set(collections.map((c) => c.database).filter(Boolean)),
    ] as string[];
    return [ALL_DB, ...values.map((v) => ({ label: v, value: v }))];
  }, [collections]);

  const schemaOptions = useMemo<DropdownOption[]>(() => {
    const values = [
      ...new Set(collections.map((c) => c.schema).filter(Boolean)),
    ] as string[];
    return [ALL_SCHEMA, ...values.map((v) => ({ label: v, value: v }))];
  }, [collections]);

  const filtered = useMemo(
    () =>
      collections.filter((c) => {
        if (selectedDb && c.database !== selectedDb) return false;
        if (selectedSchema && c.schema !== selectedSchema) return false;
        return true;
      }),
    [collections, selectedDb, selectedSchema],
  );

  const columns = useMemo<ColumnDef<SourceCollection>[]>(
    () => [
      {
        id: 'name',
        header: 'Name',
        size: 280,
        cell: (info) => (
          <div className="flex min-w-0 items-center gap-2">
            <Tooltip
              displayOnlyWhenTruncated
              content={info.row.original.sourceName}
              childrenAreInteractive
              placement="top-start"
              className="min-w-0"
            >
              {(ref) => (
                <div ref={ref} className="min-w-0 truncate">
                  <Link
                    to={paths.sourceCollection(info.row.original.sourceName)}
                    className="text-fgBrand underline decoration-transparent underline-offset-[3px] transition-[text-decoration-color] duration-[120ms] hover:decoration-fgBrand"
                  >
                    {info.row.original.sourceName}
                  </Link>
                </div>
              )}
            </Tooltip>
          </div>
        ),
      },
      {
        id: 'tableCount',
        header: 'Table count',
        cell: (info) => (
          <span className="tabular-nums">{info.row.original.totalTables}</span>
        ),
      },
      {
        id: 'database',
        header: 'Database',
        accessorFn: (row) => row.database ?? '',
        cell: asCellRenderer<SourceCollection>(TruncatedCell),
      },
      {
        id: 'schema',
        header: 'Schema',
        accessorFn: (row) => row.schema ?? '',
        cell: asCellRenderer<SourceCollection>(TruncatedCell),
      },
    ],
    [],
  );

  const dbSelected = dbOptions.find((o) => o.value === selectedDb) ?? ALL_DB;
  const schemaSelected =
    schemaOptions.find((o) => o.value === selectedSchema) ?? ALL_SCHEMA;

  return (
    <div className="flex w-full flex-col gap-5 px-8 pb-20 pt-6 text-fgMain">
      <SimpleLinkBreadcrumbs
        className="font-caption mb-3 block text-fgDecorative"
        breadcrumbs={[{ text: project.name, href: links.home() }, { text: 'Sources' }]}
      />

      <header>
        <h1 className="m-0 text-2xl font-bold leading-tight text-fgMain">Sources</h1>
      </header>

      <div className="relative z-20 flex flex-wrap items-center gap-2">
        <FilterDropdown
          name="Database"
          options={dbOptions}
          defaultOption={dbSelected}
          onChange={(opt) => setSelectedDb(String(opt.value))}
        />
        <FilterDropdown
          name="Schema"
          options={schemaOptions}
          defaultOption={schemaSelected}
          onChange={(opt) => setSelectedSchema(String(opt.value))}
        />
      </div>

      <ResourceFilterTable
        columns={columns}
        data={filtered}
        isLoading={isPending || isFetchingNextPage}
        hasMore={hasNextPage}
        onLoadMore={fetchNextPage}
        total={hasNextPage ? null : filtered.length}
        shownCount={filtered.length}
        emptyMessage="No sources found."
        error={errorMessage}
      />
    </div>
  );
}
