export type { DagNodeData, DagNodeType } from './components/lineage';
export {
  alwaysVisibleResourceTypes,
  BaseDag,
  DAG_NODE_HEIGHT,
  DAG_NODE_TYPE,
  DAG_NODE_TYPES,
  DAG_NODE_WIDTH,
  DagBottomBar,
  DagGeneralView,
  DagGroupsMinimap,
  DagGroupsView,
  DagHopBar,
  DagHopControl,
  DagLensesDropdown,
  DagMinimapPanel,
  DagNode,
  DagNodePanel,
  DagResourceBadge,
  dagResourceColor,
  DagResourceFilterDropdown,
  DagZoomControl,
  DEFAULT_ADDITIONAL_RESOURCE_TYPES,
  DEFAULT_RESOURCE_TYPE_FILTER_LABELS,
  DEFAULT_VISIBLE_RESOURCE_TYPES,
  FullLineagePageV2,
  GLASS_PANEL_CLASSES,
  lensBadgeFor,
  LineageViewV2,
  ResourceType,
  TYPE_LABEL,
} from './components/lineage';
export {
  MetadataDataProvider,
  useMetadataDataSource,
} from './context/MetadataDataProvider';
export type {
  LinkTo,
  Navigation,
  NavigationLinkProps,
  NavigationPaths,
} from './context/NavigationProvider';
export { NavigationProvider, useNavigation } from './context/NavigationProvider';
// The contract a host's data source implements. Its domain types are exported from
// `dbt-docs-lib/domain` rather than from here, where `ResourceType` already names the
// lineage type.
export type { MetadataDataSource } from './data-sources/MetadataDataSource';
export {
  LinkPrefixProvider,
  useLinkPrefix,
  useLinkPrefixRequired,
} from './shared/links/LinkPrefixContext';
// The whole-project lineage view's stand-in root: the host routes to it, and its data
// source reads it as "walk from every entry point".
export {
  isProjectLineageRoot,
  PROJECT_LINEAGE_MAX_WALK,
  PROJECT_LINEAGE_ROOT,
  PROJECT_LINEAGE_UNBOUNDED_FROM,
} from './utils/projectLineage';

// This package bundles its dependencies, so its components read the query client and
// React Flow store of its own copies. Consumers take these from here
// rather than from the packages directly, or the providers they mount are a
// different context from the one those components look up.
export type { UseQueryResult } from '@tanstack/react-query';
export {
  QueryClient,
  QueryClientProvider,
  useInfiniteQuery,
  useQueries,
  useQuery,
} from '@tanstack/react-query';
export type {
  Edge,
  Node,
  NodeChange,
  NodeProps,
  OnConnect,
  OnEdgesChange,
  OnNodesChange,
} from '@xyflow/react';
export {
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  Background,
  ConnectionMode,
  getSmoothStepPath,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useNodesInitialized,
  useReactFlow,
  useViewport,
} from '@xyflow/react';
