import { useMemo } from 'react';
import type { Edge, Node as ReactFlowNode } from '@xyflow/react';

import type { DagNodeData } from '../components/LineageV2/DagNode';
import { RESOURCE_TYPE_ORDER } from '../lib/resourceType';

type GraphNode = ReactFlowNode<DagNodeData>;

export type ResourceGroup = { resourceType: string; items: GraphNode[] };

/** BFS over one edge direction from `startId`, not including it. `forward`
 *  walks source→target (downstream of root); the reverse adjacency (built by
 *  the caller) walks target→source (upstream of root). */
function reachableFrom(startId: string, adjacency: Map<string, string[]>): Set<string> {
  const seen = new Set<string>([startId]);
  const queue = [startId];
  while (queue.length > 0) {
    const current = queue.shift()!;
    for (const next of adjacency.get(current) ?? []) {
      if (!seen.has(next)) {
        seen.add(next);
        queue.push(next);
      }
    }
  }
  seen.delete(startId);
  return seen;
}

function buildAdjacency(edges: Edge[], reverse: boolean): Map<string, string[]> {
  const adjacency = new Map<string, string[]>();
  for (const edge of edges) {
    const [from, to] = reverse
      ? [edge.target, edge.source]
      : [edge.source, edge.target];
    const list = adjacency.get(from);
    if (list) list.push(to);
    else adjacency.set(from, [to]);
  }
  return adjacency;
}

/** Partitions the currently-loaded graph into upstream-of-root /
 *  downstream-of-root, each grouped by resource type and sorted by name.
 *  Shared by DagGroupsView (the cards) and DagGroupsMinimap (the proportional
 *  squares) so both read the exact same grouping off one BFS pass. */
export function useLineageGroupPartitions(
  rootUniqueId: string,
  nodes: GraphNode[],
  edges: Edge[],
) {
  return useMemo(() => {
    const byId = new Map(nodes.map((n) => [n.id, n]));
    const downstreamIds = reachableFrom(rootUniqueId, buildAdjacency(edges, false));
    const upstreamIds = reachableFrom(rootUniqueId, buildAdjacency(edges, true));

    const group = (ids: Set<string>): ResourceGroup[] => {
      const byType = new Map<string, GraphNode[]>();
      for (const id of ids) {
        const node = byId.get(id);
        if (!node) continue;
        const list = byType.get(node.data.resourceType);
        if (list) list.push(node);
        else byType.set(node.data.resourceType, [node]);
      }
      return RESOURCE_TYPE_ORDER.filter((type) => byType.has(type)).map((type) => ({
        resourceType: type,
        items: byType.get(type)!.sort((a, b) => a.data.name.localeCompare(b.data.name)),
      }));
    };

    return { upstream: group(upstreamIds), downstream: group(downstreamIds) };
  }, [rootUniqueId, nodes, edges]);
}
