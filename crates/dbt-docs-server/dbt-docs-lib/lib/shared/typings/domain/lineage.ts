import { AssetBase } from './asset';

export type LineageEdge = {
  upstreamUniqueId: string;
  downstreamUniqueId: string;
};

export type LineageGraph = {
  /** Lightweight graph nodes — `AssetBase` (not the per-type `AssetSummary`
   *  union), since lineage carries only common fields with a wide resourceType. */
  nodes: AssetBase[];
  edges: LineageEdge[];
};

export type ColumnLineageNode = {
  uniqueId: string;
  nodeUniqueId: string;
  name: string;
  parentColumns: string[];
  transformationType: string | null;
  isError: boolean;
  errorCategory: string | null;
};

export type ColumnLineageEdge = {
  fromNodeUniqueId: string;
  fromColumn: string;
  toNodeUniqueId: string;
  toColumn: string;
  transformationType: string | null;
};

export type ColumnLineageGraph = {
  nodes: ColumnLineageNode[];
  edges: ColumnLineageEdge[];
};

/**
 * Result of a column-lineage fetch. Preserves the gated signal so consumers
 * can render an upgrade upsell instead of an empty graph: a source returns
 * `{ kind: 'gated' }` when the backend reports the feature is unavailable
 * (e.g. REST 412), and `{ kind: 'ok', graph }` otherwise (an empty graph is a
 * valid "no edges" result, distinct from gated).
 */
export type ColumnLineageResult =
  { kind: 'ok'; graph: ColumnLineageGraph } | { kind: 'gated' };
