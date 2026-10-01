import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { ChevronLeft, Copy, LayoutList, MoreHorizontal, X } from 'lucide-react';

import { getColumns, toRelationshipItem } from '../../lib/assetView';
import { inferResourceType } from '../../lib/inferResourceType';
import {
  RESOURCE_TYPES_WITH_COLUMNS,
  type ResourceTypeExplorer,
} from '../../lib/resourceType';
import { paths } from '../../routes';
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
  useProject,
} from '../../shared';
import { NoColumnMetadataFallback } from '../NoColumnMetadataFallback';
import { Button } from '../ui/Button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '../ui/DropdownMenu';
import { DagGeneralView } from './DagGeneralView';
import { DagResourceBadge } from './DagResourceBadge';

interface Props {
  uniqueId: string | null;
  onClose: () => void;
  collapsed: boolean;
  onToggleCollapse: () => void;
}

/**
 * Fork of `NodeLineagePanel` scoped to the new LineageV2 canvas -- kept as its
 * own file rather than editing the original in place, since that one is still
 * shared with the current production lineage view (see `FullLineagePage.tsx`,
 * the non-V2 one). Only the "general" tab for `model` differs today (see
 * DagGeneralView); everything else here is identical to NodeLineagePanel.
 */
export function DagNodePanel({
  uniqueId,
  onClose,
  collapsed,
  onToggleCollapse,
}: Props) {
  const isOpen = Boolean(uniqueId);
  // Freeze last-shown id so close animates with the prior content still rendered.
  const [frozenId, setFrozenId] = useState<string | null>(uniqueId);
  useEffect(() => {
    if (uniqueId) setFrozenId(uniqueId);
  }, [uniqueId]);

  return (
    <div
      data-is-panel-open={isOpen}
      className={`absolute bottom-0 right-0 top-0 z-30 overflow-y-auto bg-bgMain shadow-hover transition-[width,transform] duration-300 motion-reduce:duration-0 ${
        collapsed ? 'w-14' : 'w-[450px]'
      } ${isOpen ? 'translate-x-0' : 'translate-x-full'}`}
    >
      {frozenId && (
        <PanelBody
          uniqueId={frozenId}
          onClose={onClose}
          collapsed={collapsed}
          onToggleCollapse={onToggleCollapse}
        />
      )}
    </div>
  );
}

function PanelBody({
  uniqueId,
  onClose,
  collapsed,
  onToggleCollapse,
}: {
  uniqueId: string;
  onClose: () => void;
  collapsed: boolean;
  onToggleCollapse: () => void;
}) {
  // The panel only receives a uniqueId, so resolve the type from its prefix to
  // dispatch the adapter to the right typed endpoint.
  const resourceType = inferResourceType(uniqueId) as ResourceType;
  const assetQuery = useAssetDetail({ uniqueId, resourceType });
  const asset = assetQuery.data ?? null;
  const isPending = assetQuery.isPending;
  const notFound = !assetQuery.isPending && assetQuery.data === null;
  // The root project (this docs site) -- distinct from the asset's own
  // packageName, since a project can bundle several installed packages. Falls
  // back to the asset's own package if the project identity hasn't loaded yet
  // (it's fetched once, app-wide, so in practice this is already warm).
  const projectQuery = useProject();
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

  // Collapsed: a slim rail instead of the full panel, so the canvas gets most
  // of its width back without losing the selection entirely -- click the rail
  // (or its caret) to re-expand. Only reachable once the asset has loaded,
  // same as the caret that triggers it (see the header below).
  if (collapsed) {
    return (
      <CollapsedRail
        name={asset.name}
        resourceType={explorerType}
        onExpand={onToggleCollapse}
      />
    );
  }

  // Rich, mockup/prod-matched general tab. `source` and `exposure` already
  // read fine on the plain AssetMetadata table below, so they're left alone.
  // `unit_test` and `macro` fall back too -- macros never show up in a
  // lineage graph at all (confirmed: no macro edges in the lineage data layer).
  const RICH_GENERAL_TAB_TYPES = new Set([
    'model',
    'test',
    'metric',
    'semantic_model',
    'seed',
    'snapshot',
    'saved_query',
  ]);
  const showsRichGeneralTab = RICH_GENERAL_TAB_TYPES.has(asset.resourceType);
  const projectName = projectQuery.data?.name || asset.packageName;

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
        chip={<DagResourceBadge resourceType={explorerType} />}
        actions={
          <>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                {/* Not the shared `Button` -- Radix's `asChild` clones its
                 *  props onto a single DOM-rendering child, and `Button`
                 *  doesn't forward a ref (and wraps itself in a Tooltip when
                 *  one is passed), which breaks that cloning. Plain button,
                 *  same visual treatment as `Button`'s ghost/icon-lg. */}
                <button
                  type="button"
                  aria-label="More actions"
                  className="inline-flex items-center justify-center rounded-md p-2 text-fgMain hover:bg-bgMainHover"
                >
                  <MoreHorizontal className="size-4" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  onSelect={() => window.location.assign(paths.details(asset.uniqueId))}
                >
                  <span className="flex items-center gap-2">
                    <LayoutList className="size-4" /> Open in Catalog
                  </span>
                </DropdownMenuItem>
                <DropdownMenuItem
                  onSelect={() => {
                    const adpUrl = `${window.location.origin}${paths.details(asset.uniqueId)}`;
                    navigator.clipboard.writeText(adpUrl).catch(() => {});
                  }}
                >
                  <span className="flex items-center gap-2">
                    <Copy className="size-4" /> Copy link to clipboard
                  </span>
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
            {/* X minimizes here rather than closing outright -- the collapsed
             *  rail (below) is where a full close lives, alongside its own
             *  expand caret. Tooltip says "Minimize" even though the icon is
             *  the familiar X, so the change in scope is never ambiguous. */}
            <Button
              variant="ghost"
              size="icon-lg"
              icon={<X className="size-4" />}
              ariaLabel="Minimize panel"
              tooltip="Minimize panel"
              onClick={onToggleCollapse}
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
              if (showsRichGeneralTab) {
                return (
                  <div className="m-6">
                    <DagGeneralView asset={asset} projectName={projectName} />
                  </div>
                );
              }
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

/** Collapsed panel state: a slim vertical rail instead of the full 450px
 *  drawer. The badge keeps its normal (horizontal) orientation -- it's short
 *  and already narrow -- while the name runs vertically down the rail, since
 *  that's the part that actually identifies which node is selected. */
function CollapsedRail({
  name,
  resourceType,
  onExpand,
}: {
  name: string;
  resourceType: ResourceTypeExplorer;
  onExpand: () => void;
}) {
  return (
    <div className="flex h-full w-14 flex-col items-center gap-3 py-3">
      <Button
        variant="outline"
        size="icon-sm"
        icon={<ChevronLeft className="size-4" />}
        ariaLabel="Expand panel"
        tooltip="Expand panel"
        onClick={onExpand}
        className="h-9 w-9 rounded-full"
      />
      <button
        type="button"
        aria-label={`Expand ${name}`}
        onClick={onExpand}
        className="flex flex-col items-center gap-3 rounded-md px-1 py-2 hover:bg-bgMainHover"
      >
        <DagResourceBadge resourceType={resourceType} showText={false} />
        <span className="rotate-180 text-sm font-medium text-fgMain [writing-mode:vertical-rl]">
          {name}
        </span>
      </button>
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
