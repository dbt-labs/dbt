/**
 * Shared metadata component and data layer.
 *
 * Forked from `@dbt-labs/metadata-shared` (dbt-ui) when the docs v2 UI moved into
 * this repo. Only the transitive closure of what this app imports came across, so
 * the platform-only pieces upstream carries -- charts/echarts, the urql GraphQL
 * client, app-nav, LaunchDarkly, Datadog RUM -- are absent by construction.
 *
 * This mirrors the upstream barrel (`src/metadata-shared.tsx`) with the non-forked
 * entries pruned, so a future re-sync can diff the two directly.
 *
 * Upstream is a shared abstraction between the dbt platform explorer and this app,
 * so the two can drift.
 */

export * from './components/AnnotationBadge';
export { ArgumentsView } from './components/ArgumentsView';
export * from './components/AssetCode';
export * from './components/AssetColumns';
export * from './components/AssetDetail';
export * from './components/AssetHeader';
export * from './components/AssetMetadata';
export * from './components/AssetRelationships';
export * from './components/AutoExposureChip';
export * from './components/Badge';
export * from './components/CodePreview';
export * from './components/CollapsibleSection';
export { ColumnCard } from './components/ColumnCard';
export * from './components/ColumnCardShell';
export * from './components/ColumnsView';
export * from './components/ColumnTable';
export * from './components/ConfigDisplay';
export * from './components/DataPlatformChip';
export { DescriptionDisplay } from './components/DescriptionDisplay';
export * from './components/DetailTabs';
export { DimensionsView } from './components/DimensionsView';
export * from './components/FilterDropdown';
export * from './components/LatestStatusSection';
export type { LineageQuickLink } from './components/LineageEmptyState';
export { LineageEmptyState } from './components/LineageEmptyState';
export { MeasuresView } from './components/MeasuresView';
export { MetricDetailsView } from './components/MetricDetailsView';
export * from './components/NodeStatusIconBadge';
export * from './components/PageHeading';
export * from './components/PaginatedTable';
export * from './components/PropertyCard';
export { QueryExportsView } from './components/QueryExportsView';
export * from './components/RelationName';
export * from './components/Resizable';
export * from './components/ResourceChip';
export { ResourcePanelHeader } from './components/ResourcePanelHeader';
export { ResourcePanelTitle } from './components/ResourcePanelTitle';
export * from './components/ResourceStatusSimpleTable';
export { SavedQueryParamsView } from './components/SavedQueryParamsView';
export * from './components/search';
export * from './components/SectionWithCard';
export type { SelectorInputProps } from './components/SelectorInput';
export { SelectorInput } from './components/SelectorInput';
export { SelectorLink } from './components/SelectorLink';
export * from './components/SemanticAspectCard';
export * from './components/SimpleLinkBreadcrumbs';
export * from './components/Spinner';
export * from './components/TestResultsSection';
export * from './components/TestStatusIcon';
export * from './components/Timestamp';
export * from './components/TruncatedCell';
export * from './components/TrustSignalsBadge';
export * from './components/upgrade';
export * from './components/UpstreamSourcesSection';
export * from './components/ValueTag';
export * from './context/MetadataDataProvider';
export type { MetadataDataSource } from './data-sources/MetadataDataSource';
export * from './hooks/useAssetCounts';
export * from './hooks/useAssetDetail';
export * from './hooks/useAssetList';
export * from './hooks/useCapabilities';
export * from './hooks/useColumnLineage';
export * from './hooks/useDistribution';
export * from './hooks/useFacets';
export * from './hooks/useFiles';
export * from './hooks/useLineage';
export * from './hooks/useLocalStorage';
export * from './hooks/useProject';
export * from './hooks/useProjectOverview';
export * from './hooks/useSearch';
export * from './hooks/useSearchFacets';
export * from './links';
export * from './mappers/assetToHeaderProps';
export * from './mappers/assetToMetadataProps';
export * from './testing/createFakeDataSource';
export * from './typings';
export * from './typings/trustSignals';
export * from './typings/usage';
export * from './util/array';
export * from './util/columnLabels';
export * from './util/dateUtils';
export * from './util/queryKeys';
export * from './util/resourceType';
export * from './util/string';
export * from './util/tableSorting';
export * from './util/testStatus';
export * from './util/trustSignals';
export * from './util/usage';
export * from './wrap/wrapDataSource';
