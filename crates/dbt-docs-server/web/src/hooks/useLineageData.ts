import { useMemo } from 'react';
import { Edge, Node as ReactFlowNode } from '@xyflow/react';

import { DAG_NODE_TYPE } from '../components/LineageV2/DagNode';
import { type LineageGraph, type ResourceType, useLineage } from '../shared';

export const LOCAL_PROJECT = 'local';
export const LOCAL_PROJECT_ID = 0;

/** The fields of dbt-dag's `DbtDagNode` this hook actually populates -- kept
 *  local rather than importing the real type, since only the old dbt-dag-based
 *  `LineageView`/`FullLineagePage` need the full shape (and cast into it at
 *  their own `<Dag nodes={...}>` call site); the new LineageV2 engine only
 *  ever reads `dagNodes.length`. */
export type LineageDagNode = {
  id: string;
  parents: string[];
  label: string;
  resourceType: ResourceType;
  dbtCloudProject: string;
  projectId: number;
  materializationType: string | null;
};

export function fqnFromUniqueId(uniqueId: string): string {
  // unique_id: `<resource_type>.<package>.<...path>.<name>` → drop resource_type prefix
  return uniqueId.split('.').slice(1).join('.');
}

export function defaultSelectorFor(uniqueId: string, depth: number): string {
  return `${depth}+${fqnFromUniqueId(uniqueId)}+${depth}`;
}

/** Lineage's REST endpoint keys off unique_id alone, but the shared contract
 *  requires a resource type. The unique_id prefix is the resource type. */
function resourceTypeFromUniqueId(uniqueId: string): ResourceType {
  return (uniqueId.split('.')[0] ?? 'model') as ResourceType;
}

export function useLineageData(
  rootUniqueId: string,
  depth: number,
): {
  data: LineageGraph | null;
  error: Error | null;
  dagNodes: LineageDagNode[];
  selector: string;
  /** False when the active data source has no `fetchLineage`. */
  isSupported: boolean;
  /** nodes formatted for reactflow */
  graphNodes: ReactFlowNode[];
  graphEdges: Edge[];
} {
  const query = useLineage(
    rootUniqueId
      ? {
          uniqueId: rootUniqueId,
          resourceType: resourceTypeFromUniqueId(rootUniqueId),
          depth,
        }
      : null,
  );
  const data = query.data ?? null;

  const dagNodes = useMemo<LineageDagNode[]>(() => {
    if (!data) return [];
    const parents = new Map<string, string[]>();
    for (const e of data.edges) {
      const arr = parents.get(e.downstreamUniqueId) ?? [];
      arr.push(e.upstreamUniqueId);
      parents.set(e.downstreamUniqueId, arr);
    }
    return data.nodes.map((n) => ({
      id: n.uniqueId,
      parents: parents.get(n.uniqueId) ?? [],
      label: n.name,
      resourceType: n.resourceType,
      dbtCloudProject: LOCAL_PROJECT,
      projectId: LOCAL_PROJECT_ID,
      materializationType: n.materialized ?? null,
    }));
  }, [data]);

  const graphNodes = useMemo<ReactFlowNode[]>(() => {
    if (!data) return [];
    return data.nodes.map((node) => ({
      id: node.uniqueId,
      // Renders as `DagNode`, registered under this key in DAG_NODE_TYPES. Without a
      // `type` React Flow falls back to its own default node, which knows nothing
      // about resource types.
      type: DAG_NODE_TYPE,
      position: { x: 0, y: 0 },
      // `label` is what React Flow's built-in node types render; without it a graph
      // using the defaults draws empty boxes. `DagNode` reads `name`/`resourceType`,
      // which `node` already carries.
      // TODO: `columnCount` drives DagNode's column-lineage chip and is not in the
      // lineage payload — it would need a per-node column count exported alongside
      // `dbt.column_lineage.parquet`. Until then every node renders in the
      // no-column-lineage state.
      data: { ...node, id: node.uniqueId, label: node.name, value: node.name },
    }));
  }, [data]);

  const graphEdges = useMemo<Edge[]>(() => {
    if (!data) return [];
    return data.edges.map((edge) => ({
      id: `${edge.downstreamUniqueId}-${edge.upstreamUniqueId}`,
      source: edge.upstreamUniqueId,
      target: edge.downstreamUniqueId,
      type: 'smoothstep',
    }));
  }, [data]);

  return {
    data,
    error: query.error,
    dagNodes,
    selector: defaultSelectorFor(rootUniqueId, depth),
    isSupported: query.isSupported,
    graphNodes,
    graphEdges,
  };
}
