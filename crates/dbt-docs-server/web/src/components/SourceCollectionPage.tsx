import { useMemo } from 'react';
import { useParams } from 'react-router-dom';
import { useQueries } from '@tanstack/react-query';
import { type ColumnDef } from '@tanstack/react-table';
import { Clock, Copy, Table } from 'lucide-react';

import type { ResourceTypeExplorer } from '../lib/resourceType';
import type { FreshnessStatusValue, SourceAsset } from '../shared';
import {
  asCellRenderer,
  AssetHeader,
  type AssetHeaderIconItem,
  assetKey,
  AssetMetadata,
  DescriptionDisplay,
  DetailsSection,
  NodeStatusIconBadge,
  TimestampCell,
  useMetadataDataSource,
} from '../shared';
import { type NodeSummary } from '../types';
import { ResourceFilterTable } from './ResourceFilterTable';
import { Button } from './ui/Button';
import { Tooltip } from './ui/Tooltip';

interface Props {
  nodes: NodeSummary[];
  onSelect: (uniqueId: string) => void;
}

export function SourceCollectionPage({ nodes, onSelect }: Props) {
  const { sourceName } = useParams<{ sourceName: string }>();

  const sources = useMemo(
    () =>
      nodes.filter(
        (n) => n.resource_type === 'source' && n.unique_id.split('.')[2] === sourceName,
      ),
    [nodes, sourceName],
  );

  const source = useMetadataDataSource();
  const results = useQueries({
    queries: sources.map((s) => ({
      queryKey: assetKey(source.id, {
        uniqueId: s.unique_id,
        resourceType: 'source' as const,
      }),
      queryFn: () =>
        source.fetchAsset({ uniqueId: s.unique_id, resourceType: 'source' }),
      staleTime: 30_000,
    })),
  });

  const detailMap = useMemo(() => {
    const m = new Map<string, SourceAsset>();
    results.forEach((r, i) => {
      if (r.data) m.set(sources[i].unique_id, r.data as SourceAsset);
    });
    return m;
  }, [results, sources]);
  const isLoadingDetails = results.some((r) => r.isPending);
  const detailError = results.find((r) => r.error)?.error?.message ?? null;

  const firstSource = sources[0] ?? null;

  // The source block's own description, not any one table's — every table under
  // the same source shares it, so the first detail to land is as good as any.
  const sourceDescription = useMemo(
    () => Array.from(detailMap.values()).find((d) => d.sourceDescription)?.sourceDescription ?? null,
    [detailMap],
  );

  const maxLoadedAt = useMemo(() => {
    const dates = Array.from(detailMap.values())
      .map((d) => d.freshnessMaxLoadedAt ?? null)
      .filter(Boolean) as string[];
    return dates.length > 0 ? dates.reduce((a, b) => (a > b ? a : b)) : null;
  }, [detailMap]);

  const formattedLoadedAt = useMemo(
    () =>
      maxLoadedAt
        ? new Intl.DateTimeFormat('en-US', {
            month: 'short',
            day: 'numeric',
            year: 'numeric',
            hour: 'numeric',
            minute: '2-digit',
            timeZoneName: 'short',
          }).format(new Date(maxLoadedAt))
        : null,
    [maxLoadedAt],
  );

  const headerIcons = useMemo<AssetHeaderIconItem[]>(() => {
    const icons: AssetHeaderIconItem[] = [];
    if (formattedLoadedAt)
      icons.push({
        icon: <Clock className="size-3 align-middle" />,
        text: formattedLoadedAt,
      });
    if (sources.length > 0)
      icons.push({
        icon: <Table className="size-3 align-middle" />,
        text: `${sources.length} tables`,
      });
    return icons;
  }, [formattedLoadedAt, sources.length]);

  const columns = useMemo<ColumnDef<NodeSummary>[]>(
    () => [
      {
        id: 'name',
        header: 'Name',
        size: 280,
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
                    onClick={() => onSelect(info.row.original.unique_id)}
                  >
                    {info.row.original.name}
                  </button>
                </div>
              )}
            </Tooltip>
          </div>
        ),
      },
      {
        id: 'freshnessStatus',
        header: 'Freshness status',
        cell: (info) => {
          const detail = detailMap.get(info.row.original.unique_id);
          const status = detail?.freshnessStatus as
            FreshnessStatusValue | null | undefined;
          return status ? (
            <NodeStatusIconBadge kind="freshness" status={status} />
          ) : (
            <NodeStatusIconBadge kind="none" />
          );
        },
      },
      {
        id: 'freshnessCheckedAt',
        header: 'Freshness checked at',
        cell: () => '',
      },
      {
        id: 'lastLoadedAt',
        header: 'Last loaded at',
        accessorFn: (row) => detailMap.get(row.unique_id)?.freshnessMaxLoadedAt ?? null,
        cell: asCellRenderer<NodeSummary>(TimestampCell),
      },
    ],
    [onSelect, detailMap],
  );

  return (
    <div className="flex w-full flex-col gap-5 px-8 pb-20 pt-6 text-fgMain">
      <AssetHeader
        name={sourceName ?? ''}
        resourceType={'source' as ResourceTypeExplorer}
        packageName={firstSource?.package_name ?? null}
        headerIcons={headerIcons}
        actions={
          <Button
            variant="outline"
            icon={<Copy className="size-3" />}
            tooltip="Copy link"
            onClick={() => void navigator.clipboard.writeText(window.location.href)}
          />
        }
      />

      <DetailsSection heading="Description">
        <DescriptionDisplay description={sourceDescription} />
      </DetailsSection>

      {firstSource && (
        <DetailsSection heading="Details">
          <AssetMetadata
            resourceType="source"
            uniqueId={firstSource.unique_id}
            packageName={firstSource.package_name ?? null}
            loader={detailMap.get(firstSource.unique_id)?.loader ?? null}
            relation={{
              database: firstSource.database_name ?? null,
              schema: firstSource.schema_name ?? null,
              identifier: null,
            }}
            meta={detailMap.get(firstSource.unique_id)?.meta ?? null}
            isLoading={isLoadingDetails}
          />
        </DetailsSection>
      )}

      <h2 className="m-0 text-base font-semibold text-fgMain">Source tables</h2>

      <ResourceFilterTable
        columns={columns}
        data={sources}
        isLoading={isLoadingDetails}
        hasMore={false}
        onLoadMore={() => {}}
        total={sources.length}
        shownCount={sources.length}
        emptyMessage="No sources found."
        error={detailError}
      />
    </div>
  );
}
