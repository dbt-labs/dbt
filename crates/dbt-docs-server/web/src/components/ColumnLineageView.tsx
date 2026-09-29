import { useCallback, useEffect, useMemo } from 'react';
import {
  Background,
  type Edge,
  type Node as ReactFlowNode,
  ReactFlow,
} from '@xyflow/react';

import { useTheme } from '../hooks/useTheme';
import { applyDagreLayout } from '../lib/dagreLayout';
import { decorateOutboundHref } from '../lib/outboundReferrer';
import { type ResourceTypeExplorer } from '../lib/resourceType';
import {
  type ColumnLineageGraph,
  type ColumnLineageResult,
  Spinner,
  UpgradeCard,
  useColumnLineage as useSharedColumnLineage,
  type UserState,
} from '../shared';
import { DAG_NODE_TYPE, DAG_NODE_TYPES, type DagNodeData } from './LineageV2/DagNode';

/** Local stand-in for dbt-dag's `DbtDagNode` -- same fields this file actually
 *  used (`dbtCloudProject`/`projectId` were dbt-dag-cloud-account plumbing
 *  the new engine has no equivalent of, and nothing here read them back). */
type ColumnLineageNode = {
  id: string;
  label: string;
  sublabel: string;
  parents: string[];
  resourceType: ResourceTypeExplorer;
  transformationType: TransformationType;
};

/** Local snake-cased edge shape the subgraph BFS below walks in both
 *  directions. Sourced directly from the domain {@link ColumnLineageGraph}'s
 *  `edges` via a trivial field rename — no string parsing. */
type ColumnLineageEdge = {
  from_node: string;
  from_column: string;
  to_node: string;
  to_column: string;
  kind: string;
};

/** Mirrors dbt-dag's retired `transformationTypes`/`TransformationType` --
 *  no longer fed to anything dbt-dag owns, kept only because
 *  `buildColumnSubgraph`'s BFS still computes it per node (see below). */
const TRANSFORMATION_TYPES = [
  'UNKNOWN',
  'RAW',
  'PARSE_ERROR',
  'PASSTHROUGH',
  'TRANSFORMATION',
  'RENAME',
] as const;
type TransformationType = (typeof TRANSFORMATION_TYPES)[number];

const TRANSFORMATION_VALUES = new Set<string>(TRANSFORMATION_TYPES);

function toTransformationType(kind: string | undefined): TransformationType {
  if (!kind) return 'UNKNOWN';
  const upper = kind.toUpperCase();
  return TRANSFORMATION_VALUES.has(upper) ? (upper as TransformationType) : 'UNKNOWN';
}

function resourceTypeFromUniqueId(uniqueId: string): ResourceTypeExplorer {
  const head = uniqueId.split('.')[0] ?? 'unknown';
  return head as ResourceTypeExplorer;
}

function sublabelFromUniqueId(uniqueId: string): string {
  const parts = uniqueId.split('.');
  return parts.slice(2).join('.') || uniqueId;
}

function splitCompositeId(id: string): { nodeUniqueId: string; column: string } {
  const lastDot = id.lastIndexOf('.');
  return lastDot >= 0
    ? { nodeUniqueId: id.slice(0, lastDot), column: id.slice(lastDot + 1) }
    : { nodeUniqueId: id, column: id };
}

const compositeId = (node: string, column: string) => `${node}.${column}`;

type FetchState =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ready'; result: ColumnLineageResult }
  | { kind: 'error'; message: string };

/** Per-node CLL fetch + cache, backed by the shared `useColumnLineage`. The
 *  Columns tab lazily loads on first column expand and reuses the cached
 *  response for subsequent expansions of other columns in the same node.
 *  Lazy: the query is disabled until `load()` triggers a fetch.
 *  Cache-backed per `rootUniqueId`: revisiting a node whose response is still
 *  in the react-query cache (within `gcTime`) renders immediately as `ready`
 *  without a fresh `load()`; only a cold cache key starts in `idle`. */
export function useColumnLineage(rootUniqueId: string) {
  const query = useSharedColumnLineage({ uniqueId: rootUniqueId }, { enabled: false });

  const state: FetchState = query.isFetching
    ? { kind: 'loading' }
    : query.error
      ? {
          kind: 'error',
          message:
            query.error instanceof Error ? query.error.message : String(query.error),
        }
      : query.data !== undefined
        ? { kind: 'ready', result: query.data }
        : { kind: 'idle' };

  const { refetch } = query;
  const load = useCallback(() => {
    void refetch();
  }, [refetch]);

  return { state, load };
}

interface MiniProps {
  rootUniqueId: string;
  columnName: string;
  state: FetchState;
  load: () => void;
  onSelect(uniqueId: string): void;
  userState: UserState | null;
}

/** Per-column expanded body: lazy-fires the node-scoped CLL fetch on
 *  first render (via the parent-owned hook so siblings reuse the
 *  cached response), then filters the edge set to the subgraph that
 *  touches `columnName` on `rootUniqueId`. */
export function ColumnLineageMini({
  rootUniqueId,
  columnName,
  state,
  load,
  onSelect,
  userState,
}: MiniProps) {
  // React Flow stamps `light` or `dark` on the canvas root for its own theming,
  // and this app's tokens are scoped by those exact class names (`:root .light` /
  // `:root .dark` in styles/tokens.css) -- same collision BaseDag.tsx already
  // works around. Left on the default (`light`), it re-themes every token
  // inside it regardless of the app's real theme.
  const { resolved } = useTheme();

  useEffect(() => {
    if (state.kind === 'idle') load();
  }, [state.kind, load]);

  const subgraph = useMemo(() => {
    if (state.kind !== 'ready' || state.result.kind !== 'ok') return null;
    return buildColumnSubgraph(state.result.graph, rootUniqueId, columnName);
  }, [state, rootUniqueId, columnName]);

  if (state.kind === 'idle' || state.kind === 'loading') {
    return (
      <p className="muted flex items-center gap-2" style={{ fontSize: 13 }}>
        <Spinner /> Loading column lineage…
      </p>
    );
  }

  if (state.kind === 'error') {
    return (
      <div className="err">
        Failed to load column lineage: <code className="inline">{state.message}</code>{' '}
        <button type="button" onClick={load} className="btn sm">
          Retry
        </button>
      </div>
    );
  }

  const { result } = state;
  if (result.kind === 'gated') {
    if (userState) {
      return (
        <UpgradeCard
          kind="columnLineage"
          userState={userState}
          variant="inline"
          decorateOutboundHref={decorateOutboundHref}
        />
      );
    }
    return null;
  }

  if (!subgraph || subgraph.nodes.length === 0) {
    return (
      <p className="muted" style={{ fontSize: 13 }}>
        No column-level lineage edges touch this column.
      </p>
    );
  }

  const primaryNodeIds = new Set(
    subgraph.nodes
      .filter(
        (n) =>
          splitCompositeId(n.id).nodeUniqueId === rootUniqueId &&
          splitCompositeId(n.id).column === columnName,
      )
      .map((n) => n.id),
  );

  // Combined into one line (DagNode has no separate sublabel slot) rather
  // than dropped -- which model a column belongs to is real information the
  // old dbt-dag rendering showed, not just decoration. DagNode's own
  // truncate-and-tooltip handles anything that doesn't fit.
  const rawNodes: ReactFlowNode<DagNodeData>[] = subgraph.nodes.map((n) => ({
    id: n.id,
    type: DAG_NODE_TYPE,
    position: { x: 0, y: 0 },
    selected: primaryNodeIds.has(n.id),
    data: {
      name:
        n.sublabel && n.sublabel !== n.label ? `${n.label} · ${n.sublabel}` : n.label,
      resourceType: n.resourceType,
    },
  }));
  const flowEdges: Edge[] = subgraph.nodes.flatMap((n) =>
    n.parents.map((parentId) => ({
      id: `${parentId}->${n.id}`,
      source: parentId,
      target: n.id,
      type: 'smoothstep',
    })),
  );
  const flowNodes = applyDagreLayout(rawNodes, flowEdges, { rankdir: 'LR' });

  return (
    <div className="lineage-frame dag-v2-canvas" style={{ height: 320 }}>
      <ReactFlow
        nodes={flowNodes}
        edges={flowEdges}
        nodeTypes={DAG_NODE_TYPES}
        colorMode={resolved}
        fitView
        nodesConnectable={false}
        nodesDraggable={false}
        elementsSelectable={false}
        panOnScroll
        onNodeClick={(_, node) => onSelect(splitCompositeId(node.id).nodeUniqueId)}
      >
        <Background color="var(--muted-foreground)" gap={20} size={1} />
      </ReactFlow>
    </div>
  );
}

/** Rename the domain graph's edges to the local snake-cased shape the BFS
 *  consumes. */
function edgesFromGraph(graph: ColumnLineageGraph): ColumnLineageEdge[] {
  return graph.edges.map((e) => ({
    from_node: e.fromNodeUniqueId,
    from_column: e.fromColumn,
    to_node: e.toNodeUniqueId,
    to_column: e.toColumn,
    kind: e.transformationType ?? '',
  }));
}

/** Value `normalizeLineageKind` (duckdb data source, the only production
 *  source) emits for a `scan`-kind edge -- e.g. a column only referenced in a
 *  WHERE/JOIN condition, never copied or transformed into the target's value.
 *  Filtered out below so it never appears as a false lineage node. Keep in
 *  sync with `shared/data-sources/duckdb/sql.ts`. */
const SCAN_KIND = 'indirect';

/** BFS the edge set both upstream and downstream from the target column,
 *  then materialize nodes for the reachable composite ids. Scan-kind edges
 *  are dropped first, so a column reachable only through one never enters
 *  the subgraph at all (matches VSCE's effective behavior). */
function buildColumnSubgraph(
  graph: ColumnLineageGraph,
  rootUniqueId: string,
  columnName: string,
): { nodes: ColumnLineageNode[] } {
  const edges = edgesFromGraph(graph).filter((e) => e.kind !== SCAN_KIND);
  const target = compositeId(rootUniqueId, columnName);

  const outgoing = new Map<string, ColumnLineageEdge[]>();
  const incoming = new Map<string, ColumnLineageEdge[]>();
  for (const e of edges) {
    const fromId = compositeId(e.from_node, e.from_column);
    const toId = compositeId(e.to_node, e.to_column);
    (outgoing.get(fromId) ?? outgoing.set(fromId, []).get(fromId)!).push(e);
    (incoming.get(toId) ?? incoming.set(toId, []).get(toId)!).push(e);
  }

  const reachable = new Set<string>([target]);
  const walk = (start: string, adj: Map<string, ColumnLineageEdge[]>) => {
    const stack = [start];
    while (stack.length) {
      const id = stack.pop()!;
      for (const e of adj.get(id) ?? []) {
        const next =
          adj === outgoing
            ? compositeId(e.to_node, e.to_column)
            : compositeId(e.from_node, e.from_column);
        if (!reachable.has(next)) {
          reachable.add(next);
          stack.push(next);
        }
      }
    }
  };
  walk(target, outgoing);
  walk(target, incoming);

  if (reachable.size <= 1 && !outgoing.has(target) && !incoming.has(target)) {
    return { nodes: [] };
  }

  const parentMap = new Map<string, Array<{ parentId: string; kind: string }>>();
  for (const e of edges) {
    const fromId = compositeId(e.from_node, e.from_column);
    const toId = compositeId(e.to_node, e.to_column);
    if (!reachable.has(fromId) || !reachable.has(toId)) continue;
    const arr = parentMap.get(toId) ?? [];
    arr.push({ parentId: fromId, kind: e.kind });
    parentMap.set(toId, arr);
  }

  const nodes: ColumnLineageNode[] = Array.from(reachable).map((id) => {
    const { nodeUniqueId, column } = splitCompositeId(id);
    const incomingEdges = parentMap.get(id) ?? [];
    const parents = incomingEdges.map((p) => p.parentId);
    return {
      id,
      label: column,
      sublabel: sublabelFromUniqueId(nodeUniqueId),
      parents,
      resourceType: resourceTypeFromUniqueId(nodeUniqueId),
      transformationType:
        parents.length === 0 ? 'RAW' : toTransformationType(incomingEdges[0]?.kind),
    };
  });

  return { nodes };
}
