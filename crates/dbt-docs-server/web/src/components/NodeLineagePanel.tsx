import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { Compass, Share2, X } from 'lucide-react';

import { getColumns, toRelationshipItem } from '../lib/assetView';
import { inferResourceType } from '../lib/inferResourceType';
import {
  RESOURCE_TYPES_WITH_COLUMNS,
  type ResourceTypeExplorer,
} from '../lib/resourceType';
import { paths } from '../routes';
import {
  AssetMetadata,
  assetToMetadataProps,
  type ColumnItem,
  ColumnsView,
  DescriptionDisplay,
  DetailTabs,
  isTabType,
  type RelationshipItem,
  ResourcePanelHeader,
  ResourcePanelTitle,
  type ResourceType,
  Spinner,
  type TabType,
  useAssetDetail,
} from '../shared';
import { NoColumnMetadataFallback } from './NoColumnMetadataFallback';
import { Button } from './ui/Button';

interface Props {
  uniqueId: string | null;
  onClose: () => void;
}

/**
 * Slide-in lineage detail panel. Mirrors the 450px sliding shell from
 * `packages/metadata/dbt-explorer/src/components/Lineage/LineagePanelView.tsx`
 * but drives content off the local manifest detail fetch rather than the
 * Discovery API.
 */
export function NodeLineagePanel({ uniqueId, onClose }: Props) {
  const isOpen = Boolean(uniqueId);
  // Freeze last-shown id so close animates with the prior content still rendered.
  const [frozenId, setFrozenId] = useState<string | null>(uniqueId);
  useEffect(() => {
    if (uniqueId) setFrozenId(uniqueId);
  }, [uniqueId]);

  return (
    <div
      data-is-panel-open={isOpen}
      className={`absolute bottom-0 right-0 top-0 z-30 w-[450px] overflow-y-auto bg-bgMain shadow-hover transition-transform duration-300 motion-reduce:duration-0 ${
        isOpen ? 'translate-x-0' : 'translate-x-full'
      }`}
    >
      {frozenId && <PanelBody uniqueId={frozenId} onClose={onClose} />}
    </div>
  );
}

function PanelBody({ uniqueId, onClose }: { uniqueId: string; onClose: () => void }) {
  // The panel only receives a uniqueId, so resolve the type from its prefix to
  // dispatch the adapter to the right typed endpoint.
  const resourceType = inferResourceType(uniqueId) as ResourceType;
  const assetQuery = useAssetDetail({ uniqueId, resourceType });
  const asset = assetQuery.data ?? null;
  const isPending = assetQuery.isPending;
  const notFound = !assetQuery.isPending && assetQuery.data === null;
  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get('tab');
  const activeTab: TabType = isTabType(tabParam) ? tabParam : 'general';

  const setTab = (tab: TabType) => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.set('tab', tab);
        return next;
      },
      { replace: true },
    );
  };

  if (isPending && !asset) {
    return (
      <div className="flex items-center gap-2 p-6 text-sm">
        <Spinner /> Loading…
      </div>
    );
  }

  if (notFound || !asset) {
    return (
      <div className="p-6">
        <div className="flex justify-end">
          <Button
            variant="ghost"
            icon={<X className="size-3" />}
            size="icon-lg"
            ariaLabel="Close Panel"
            tooltip="Close Panel"
            onClick={onClose}
          />
        </div>
        <p className="text-sm text-fgDecorative">
          Detail not available for <code>{uniqueId}</code>.
        </p>
      </div>
    );
  }

  const explorerType = asset.resourceType as ResourceTypeExplorer;

  const columnItems: ColumnItem[] = getColumns(asset).map((c) => ({
    name: c.name,
    type: c.dataType,
    description: c.description,
  }));

  const dependsOn = (asset.dependsOn ?? []).map(toRelationshipItem);
  const referencedBy = (asset.referencedBy ?? []).map(toRelationshipItem);
  const hasRelations = dependsOn.length > 0 || referencedBy.length > 0;

  const showColumns = (RESOURCE_TYPES_WITH_COLUMNS as readonly string[]).includes(
    explorerType,
  );
  const tabs = [
    { type: 'general' as TabType },
    ...(showColumns
      ? [{ type: 'columns' as TabType, count: columnItems.length || undefined }]
      : []),
    ...(hasRelations
      ? [
          {
            type: 'relationships' as TabType,
            count: dependsOn.length + referencedBy.length,
          },
        ]
      : []),
  ];

  return (
    <div className="flex h-full flex-col">
      <ResourcePanelHeader
        resourceType={explorerType}
        actions={
          <>
            <Button
              variant="ghost"
              size="icon-lg"
              icon={<Compass className="size-4" />}
              ariaLabel="Open full details"
              tooltip="Open full details"
              onClick={() => window.location.assign(paths.details(asset.uniqueId))}
            />
            <Button
              variant="ghost"
              size="icon-lg"
              icon={<Share2 className="size-4" />}
              ariaLabel="Copy Link"
              tooltip="Copy Link"
              onClick={() => {
                navigator.clipboard.writeText(window.location.href).catch(() => {});
              }}
            />
            <Button
              variant="ghost"
              size="icon-lg"
              icon={<X className="size-4" />}
              ariaLabel="Close Panel"
              tooltip="Close Panel"
              onClick={onClose}
            />
          </>
        }
      />
      <div className="flex-1 overflow-y-auto">
        <ResourcePanelTitle
          name={asset.name}
          packageName={asset.packageName || null}
          resourceType={explorerType}
          access={'access' in asset ? asset.access : null}
          className="p-4"
        />
        <DetailTabs tabs={tabs} show activeTab={activeTab} onTabChange={setTab}>
          {(tab) => {
            if (tab === 'general') {
              return (
                <div className="m-6">
                  <div className="mb-6" data-testid="description-block">
                    <h2 className="mb-4 text-xs font-medium text-fgMain">
                      Description
                    </h2>
                    <DescriptionDisplay
                      description={asset.description}
                      className="mb-2 text-sm"
                    />
                  </div>
                  <AssetMetadata
                    {...assetToMetadataProps(asset)}
                    filePath={asset.originalFilePath ?? asset.filePath ?? null}
                    compact
                  />
                </div>
              );
            }
            if (tab === 'columns') {
              return (
                <div className="px-4">
                  <ColumnsView
                    columns={columnItems}
                    emptyState={<NoColumnMetadataFallback />}
                  />
                </div>
              );
            }
            if (tab === 'relationships') {
              return (
                <div className="m-6 space-y-6">
                  {dependsOn.length > 0 && (
                    <RelationshipSection
                      heading="Depends on"
                      items={dependsOn}
                      onSelect={(id) => window.location.assign(paths.details(id))}
                    />
                  )}
                  {referencedBy.length > 0 && (
                    <RelationshipSection
                      heading="Referenced by"
                      items={referencedBy}
                      onSelect={(id) => window.location.assign(paths.details(id))}
                    />
                  )}
                </div>
              );
            }
            return null;
          }}
        </DetailTabs>
      </div>
    </div>
  );
}

function RelationshipSection({
  heading,
  items,
  onSelect,
}: {
  heading: string;
  items: RelationshipItem[];
  onSelect: (uniqueId: string) => void;
}) {
  return (
    <div>
      <h2 className="text-xs font-medium text-fgDecorative">{heading}</h2>
      <div className="mt-4 space-y-2">
        {items.map((item) => (
          <button
            key={item.uniqueId}
            type="button"
            className="flex w-full items-center gap-1 text-left text-sm text-fgBrand hover:underline"
            onClick={() => onSelect(item.uniqueId)}
          >
            {item.name}
          </button>
        ))}
      </div>
    </div>
  );
}
