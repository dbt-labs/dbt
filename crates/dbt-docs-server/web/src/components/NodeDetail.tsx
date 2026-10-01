import { createElement } from 'react';
import {
  Box,
  Camera,
  ChartColumn,
  CircleGauge,
  ClipboardCheck,
  Copy,
  Database,
  FileText,
  type LucideIcon,
  Save,
  Sprout,
  Table,
  Users,
  Waypoints,
} from 'lucide-react';

import { getColumns, toRelationshipItem } from '../lib/assetView';
import { filterConfig } from '../lib/configView';
import { decorateOutboundHref } from '../lib/outboundReferrer';
import {
  RESOURCE_TYPE_SINGULAR,
  RESOURCE_TYPES_WITH_COLUMNS,
  type ResourceTypeExplorer,
} from '../lib/resourceType';
import { handleUpsellEvent } from '../lib/upsellAnalytics';
import {
  ArgumentsView,
  type Asset,
  AssetCode,
  AssetHeader,
  type AssetHeaderIconItem,
  AssetMetadata,
  AssetRelationships,
  assetToMetadataProps,
  type ColumnItem,
  ColumnsView,
  ColumnTable,
  ConfigDisplay,
  DescriptionDisplay,
  DetailsSection,
  DetailTabs,
  DimensionsView,
  type GroupAsset,
  hasAssetMetadata,
  MeasuresView,
  type MetricAsset,
  MetricDetailsView,
  type ModelAsset,
  QueryExportsView,
  type SavedQueryAsset,
  SavedQueryParamsView,
  type SemanticModelAsset,
  type SourceAsset,
  type TabInfo,
  type TabType,
  UpgradeCard,
  type UserState,
} from '../shared';
import { ColumnLineageMini, useColumnLineage } from './ColumnLineageView';
import { LineageViewV2 } from './LineageV2/LineageView';
import { NoColumnMetadataFallback } from './NoColumnMetadataFallback';
import { Button } from './ui/Button';
import { Card } from './ui/Card';

interface Props {
  asset: Asset;
  onSelect(uniqueId: string): void;
  hasColumnLineage?: boolean;
  /** Drives the Columns-tab column-level-lineage upgrade empty state. Null
   *  while capabilities are loading. */
  userState: UserState | null;
}

const RESOURCE_TYPE_ICON: Record<string, LucideIcon> = {
  model: Box,
  source: Database,
  test: ClipboardCheck,
  exposure: CircleGauge,
  group: Users,
  metric: ChartColumn,
  semantic_model: Waypoints,
  seed: Sprout,
  macro: FileText,
  snapshot: Camera,
  saved_query: Save,
  analysis: FileText,
};

/** Coerce a field into `string[]`. Backend may emit a bare string. */
function toStringArray(value: unknown): string[] {
  if (Array.isArray(value))
    return value.filter((v): v is string => typeof v === 'string');
  if (typeof value === 'string') return [value];
  return [];
}

function getCode(asset: Asset): string | null {
  if (asset.resourceType === 'macro') return asset.macroSql || null;
  if ('rawCode' in asset) return (asset as ModelAsset).rawCode;
  return null;
}

function getCompiledCode(asset: Asset): string | null {
  if ('compiledCode' in asset) return (asset as ModelAsset).compiledCode;
  return null;
}

function getMaterialization(asset: Asset): string | null {
  if (
    asset.resourceType === 'model' ||
    asset.resourceType === 'seed' ||
    asset.resourceType === 'snapshot'
  )
    return asset.materializedType ?? null;
  return null;
}

function getTabsForAsset(asset: Asset): TabInfo[] {
  const hasConfig = filterConfig(asset.config ?? null) != null;
  return [
    ...getResourceTabsForAsset(asset),
    ...(hasConfig ? [{ type: 'config' as TabType }] : []),
  ];
}

function getResourceTabsForAsset(asset: Asset): TabInfo[] {
  const hasCode = Boolean(getCode(asset));
  const colCount = getColumns(asset).length;

  switch (asset.resourceType) {
    case 'seed':
      return [{ type: 'general' }, { type: 'columns', count: colCount }];
    case 'source':
      return [{ type: 'general' }, { type: 'columns', count: colCount }];
    case 'test':
      return [{ type: 'general' }, ...(hasCode ? [{ type: 'code' as TabType }] : [])];
    case 'exposure':
    case 'metric':
    case 'group':
      return [{ type: 'general' }];
    case 'macro':
      return [
        { type: 'general' },
        { type: 'arguments' as TabType },
        ...(hasCode ? [{ type: 'code' as TabType }] : []),
      ];
    case 'snapshot':
      return [
        { type: 'general' },
        ...(hasCode ? [{ type: 'code' as TabType }] : []),
        { type: 'columns', count: colCount },
      ];
    case 'semantic_model': {
      const sm = asset as SemanticModelAsset;
      return [
        { type: 'general' },
        { type: 'dimensions' as TabType, count: sm.dimensions.length },
        { type: 'measures' as TabType, count: sm.measures.length },
      ];
    }
    case 'saved_query': {
      const sq = asset as SavedQueryAsset;
      return [
        { type: 'general' },
        { type: 'queryExports' as TabType, count: sq.exports.length },
      ];
    }
    default: {
      const showColumns = (RESOURCE_TYPES_WITH_COLUMNS as readonly string[]).includes(
        asset.resourceType,
      );
      return [
        { type: 'general' },
        ...(hasCode ? [{ type: 'code' as TabType }] : []),
        ...(showColumns ? [{ type: 'columns' as TabType, count: colCount }] : []),
      ];
    }
  }
}

function typeParamsToMetricView(asset: MetricAsset) {
  const { typeParams } = asset;
  switch (typeParams.kind) {
    case 'simple':
      return { type: 'simple', measure: typeParams.measure.name };
    case 'ratio':
      return {
        type: 'ratio',
        numerator: typeParams.numerator.name,
        denominator: typeParams.denominator.name,
      };
    case 'cumulative':
      return {
        type: 'cumulative',
        measure: typeParams.measure.name,
        grainToDate: typeParams.grainToDate,
      };
    case 'derived':
      return { type: 'derived', expression: typeParams.expr };
  }
}

export function NodeDetail({ asset, onSelect, hasColumnLineage, userState }: Props) {
  const resourceType = asset.resourceType as ResourceTypeExplorer;
  const materialization = getMaterialization(asset);

  const headerIcons: AssetHeaderIconItem[] = [
    {
      icon: createElement(RESOURCE_TYPE_ICON[resourceType] ?? FileText, {
        className: 'size-3 align-middle',
      }),
      text: RESOURCE_TYPE_SINGULAR[resourceType] ?? asset.resourceType,
    },
  ];
  if (materialization) {
    headerIcons.push({
      icon: <Table className="size-3 align-middle" />,
      text: materialization.charAt(0).toUpperCase() + materialization.slice(1),
    });
  }

  const tabs = getTabsForAsset(asset);
  const cll = useColumnLineage(asset.uniqueId);

  const columnItems: ColumnItem[] = getColumns(asset).map((c) => ({
    name: c.name,
    type: c.dataType,
    description: c.description,
  }));

  const actions = (
    <div className="flex items-center gap-2">
      <Button
        variant="outline"
        icon={<Copy className="size-3" />}
        tooltip="Copy link"
        onClick={() => {
          void navigator.clipboard.writeText(window.location.href);
        }}
      />
    </div>
  );

  return (
    <article className="flex flex-col gap-6 px-8 pb-20 pt-6 text-fgMain">
      <DetailTabs
        tabs={tabs}
        show={true}
        stickyHeader={
          <AssetHeader
            name={asset.name}
            resourceType={resourceType}
            packageName={asset.packageName || null}
            headerIcons={headerIcons}
            actions={actions}
          />
        }
      >
        {(tabType) => {
          switch (tabType) {
            case 'general': {
              const baseMetaProps = assetToMetadataProps(asset);
              const assetMetadataProps = {
                ...baseMetaProps,
                // Prefer originalFilePath (source: docs-only) for the file display.
                filePath: asset.originalFilePath ?? baseMetaProps.filePath,
                userState,
                decorateOutboundHref,
              };

              return (
                <>
                  <DetailsSection heading="Description">
                    <DescriptionDisplay description={asset.description} />
                  </DetailsSection>

                  {asset.resourceType === 'group' && (
                    <DetailsSection heading="Owner">
                      <ColumnTable
                        isLoading={false}
                        tableEntries={[
                          { key: 'Name', data: (asset as GroupAsset).ownerName },
                          { key: 'Email', data: (asset as GroupAsset).ownerEmail },
                          { key: 'GitHub', data: (asset as GroupAsset).ownerGithub },
                          { key: 'Slack', data: (asset as GroupAsset).ownerSlack },
                        ].filter((e) => e.data != null)}
                      />
                    </DetailsSection>
                  )}

                  {asset.resourceType === 'group' &&
                    (asset as GroupAsset).models &&
                    (asset as GroupAsset).models!.length > 0 && (
                      <DetailsSection heading="Models">
                        <ul className="divide-y">
                          {(asset as GroupAsset).models!.map((m) => (
                            <li
                              key={m.uniqueId}
                              className="flex items-center justify-between px-6 py-3"
                            >
                              <button
                                type="button"
                                className="text-sm text-fgMain hover:underline"
                                onClick={() => onSelect(m.uniqueId)}
                              >
                                {m.name}
                              </button>
                              {(m.database || m.schema) && (
                                <span className="text-xs text-fgDecorative">
                                  {[m.database, m.schema].filter(Boolean).join('.')}
                                </span>
                              )}
                            </li>
                          ))}
                        </ul>
                      </DetailsSection>
                    )}

                  {asset.resourceType === 'source' &&
                    (asset as SourceAsset).freshnessStatus && (
                      <DetailsSection heading="Freshness">
                        <p className="p-6 text-sm capitalize text-fgMain">
                          {(asset as SourceAsset).freshnessStatus}
                        </p>
                        {(asset as SourceAsset).freshnessMaxLoadedAt && (
                          <p className="px-6 pb-6 text-xs text-fgDecorative">
                            Last loaded: {(asset as SourceAsset).freshnessMaxLoadedAt}
                          </p>
                        )}
                      </DetailsSection>
                    )}

                  {asset.resourceType === 'metric' && (
                    <MetricDetailsView
                      metric={typeParamsToMetricView(asset as MetricAsset)}
                    />
                  )}

                  {asset.resourceType !== 'macro' && asset.resourceType !== 'group' && (
                    <DetailsSection heading="Lineage" isCompact>
                      <div className="h-[480px]">
                        <LineageViewV2
                          rootUniqueId={asset.uniqueId}
                          modelName={asset.name}
                        />
                      </div>
                    </DetailsSection>
                  )}

                  {asset.resourceType === 'semantic_model' &&
                    (asset as SemanticModelAsset).entities.length > 0 && (
                      <DetailsSection heading="Entities">
                        <table className="w-full text-sm">
                          <thead>
                            <tr className="border-b text-left">
                              <th className="p-3 font-medium">Name</th>
                              <th className="p-3 font-medium">Type</th>
                              <th className="p-3 font-medium">Expr</th>
                            </tr>
                          </thead>
                          <tbody className="divide-y">
                            {(asset as SemanticModelAsset).entities.map((e) => (
                              <tr key={e.name}>
                                <td className="p-3">{e.name}</td>
                                <td className="p-3 text-fgDecorative">{e.type}</td>
                                <td className="p-3 text-fgDecorative">
                                  {e.expr ?? ''}
                                </td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                      </DetailsSection>
                    )}

                  {hasAssetMetadata(assetMetadataProps) && (
                    <DetailsSection heading="Metadata">
                      <AssetMetadata {...assetMetadataProps} />
                    </DetailsSection>
                  )}

                  {asset.resourceType === 'saved_query' &&
                    (() => {
                      const sq = asset as SavedQueryAsset;
                      return (
                        <SavedQueryParamsView
                          params={{
                            metrics: sq.queryParams.metrics,
                            groupBy: sq.queryParams.groupBy,
                            where: sq.queryParams.where,
                            orderBy: toStringArray(sq.queryParams.orderBy),
                            limit: sq.queryParams.limit,
                          }}
                        />
                      );
                    })()}

                  <DetailsSection heading="Relationships" className="!p-3">
                    <AssetRelationships
                      dependsOn={(asset.dependsOn ?? []).map(toRelationshipItem)}
                      referencedBy={(asset.referencedBy ?? []).map(toRelationshipItem)}
                      onSelect={onSelect}
                    />
                  </DetailsSection>
                </>
              );
            }

            case 'config': {
              const visibleConfig = filterConfig(asset.config ?? null);
              if (!visibleConfig) return null;
              return (
                <div className="p-4">
                  <Card className="overflow-hidden !p-3">
                    <ConfigDisplay config={visibleConfig} />
                  </Card>
                </div>
              );
            }

            case 'code':
              return (
                <AssetCode
                  rawCode={getCode(asset)}
                  compiledCode={getCompiledCode(asset)}
                />
              );

            case 'columns':
              return (
                <div className="p-4">
                  <ColumnsView
                    columns={columnItems}
                    disableSearch={columnItems.length === 0}
                    expandable={hasColumnLineage}
                    renderExpanded={(col) => (
                      <ColumnLineageMini
                        rootUniqueId={asset.uniqueId}
                        columnName={col.name}
                        state={cll.state}
                        load={cll.load}
                        userState={userState}
                        onSelect={onSelect}
                      />
                    )}
                    emptyState={
                      <>
                        <NoColumnMetadataFallback />
                        {columnItems.length === 0 &&
                          userState &&
                          userState !== 'proprietary-logged-in' && (
                            <div className="node-detail__columns-upgrade">
                              <UpgradeCard
                                kind="columnLineage"
                                userState={userState}
                                variant="inline"
                                onUpsellEvent={handleUpsellEvent}
                                decorateOutboundHref={decorateOutboundHref}
                                location="resource-detail-columns"
                              />
                            </div>
                          )}
                      </>
                    }
                  />
                </div>
              );

            case 'arguments':
              return (
                <div className="p-4">
                  <ArgumentsView
                    macroArguments={
                      asset.resourceType === 'macro'
                        ? asset.arguments.map((a) => ({
                            name: a.name,
                            type: a.type ?? null,
                            description: a.description ?? null,
                          }))
                        : []
                    }
                  />
                </div>
              );

            case 'dimensions':
              return (
                <div className="p-4">
                  <DimensionsView
                    dimensions={
                      asset.resourceType === 'semantic_model'
                        ? asset.dimensions.map((d) => ({
                            name: d.name,
                            type: d.type as 'categorical' | 'time',
                            description: d.description ?? null,
                          }))
                        : []
                    }
                  />
                </div>
              );

            case 'measures':
              return (
                <div className="p-4">
                  <MeasuresView
                    measures={
                      asset.resourceType === 'semantic_model'
                        ? asset.measures.map((m) => ({
                            name: m.name,
                            agg: m.agg,
                            expr: m.expr ?? null,
                            description: m.description ?? null,
                          }))
                        : []
                    }
                  />
                </div>
              );

            case 'queryExports':
              return (
                <div className="p-4">
                  <QueryExportsView
                    exports={
                      asset.resourceType === 'saved_query'
                        ? asset.exports.map((e) => ({
                            name: e.name,
                            config: {
                              export_as: e.exportAs,
                              schema: e.schema,
                            } as Record<string, unknown>,
                          }))
                        : []
                    }
                  />
                </div>
              );

            default:
              return null;
          }
        }}
      </DetailTabs>
    </article>
  );
}
