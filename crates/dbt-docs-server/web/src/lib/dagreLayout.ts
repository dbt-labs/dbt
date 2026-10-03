import Dagre, { type Edge as DagreEdge, type graphlib } from '@dagrejs/dagre';
import { type Edge, type Node as ReactFlowNode, Position } from '@xyflow/react';

// these correspond to GraphLabel options in dagre
export interface LayoutOptions {
  /** Direction of the graph layout */
  rankdir?: 'TB' | 'LR';
  /** Vertical spacing between ranks */
  ranksep?: number;
  /** Horizontal spacing between nodes */
  nodesep?: number;
  /** Spacing around edges */
  edgesep?: number;
  /** Margin around the graph */
  marginx?: number;
  marginy?: number;
  /** Width of nodes */
  nodeWidth?: number;
  /** Height of nodes */
  nodeHeight?: number;
  /** Ranker used to layout nodes */
  ranker?: string;
  /** Acyclicer type */
  acyclicer?: string;
}

// Dagre layout constants.
//
// NODE_WIDTH / NODE_HEIGHT are a fallback, not the layout's idea of a card. The graph
// is laid out from `node.measured`, the size React Flow read off the rendered card, so
// a wide card gets a wide slot. These are only what an unmeasured node is given, and
// the store does not lay out until every card has been measured — so in practice they
// are reached only by a node dagre knows nothing about.
export const NODE_WIDTH = 245;
export const NODE_HEIGHT = 108;
export const RANK_SEPARATION = 100;
export const NODE_SEPARATION = 20;
export const EDGE_SEPARATION = 30;
export const GRAPH_MARGIN = 50;
export const DEFAULT_RANKER = 'tight-tree';
export const DEFAULT_ACYCLICER = 'greedy';

export const DEFAULT_LAYOUT_OPTIONS: Required<LayoutOptions> = {
  rankdir: 'LR',
  ranksep: RANK_SEPARATION,
  nodesep: NODE_SEPARATION,
  edgesep: EDGE_SEPARATION,
  marginx: GRAPH_MARGIN,
  marginy: GRAPH_MARGIN,
  nodeWidth: NODE_WIDTH,
  nodeHeight: NODE_HEIGHT,
  ranker: 'tight-tree',
  acyclicer: 'greedy',
};

function handlePositionsForRankdir(rankdir: 'TB' | 'LR') {
  return rankdir === 'TB'
    ? { sourcePosition: Position.Bottom, targetPosition: Position.Top }
    : { sourcePosition: Position.Right, targetPosition: Position.Left };
}

/** The box to reserve for a node: what React Flow measured off the rendered card,
 *  falling back to the design's geometry before the first measurement. `||` rather
 *  than `??` on purpose — a node measured at 0 is one that has not really been laid
 *  out by the browser yet, and 0 is not a size worth laying out around. */
function boxOf(node: ReactFlowNode, opts: Required<LayoutOptions>) {
  return {
    width: node.measured?.width || opts.nodeWidth,
    height: node.measured?.height || opts.nodeHeight,
  };
}

/** The graph shape {@link alignRanksTowardRoot} needs -- structural, so it owes
 *  nothing to dagre's exported class type. */
interface LaidOutGraph {
  nodes(): string[];
  hasNode(id: string): boolean;
  node(id: string): { x?: number; y?: number; width: number; height: number };
}

/**
 * Flush each rank against the edge that faces the root.
 *
 * Dagre centres every node in a rank on the rank's own axis, so a column of cards of
 * different widths (a card is as wide as its name) comes out ragged on both sides and
 * the gutter between one rank and the next zigzags. Aligning on the side facing the
 * root instead -- right edges upstream of it, left edges downstream -- gives every
 * rank one straight edge pointing at the node the graph is actually about.
 *
 * Runs on dagre's own centres, before they are rounded and converted to corners: every
 * node in a rank shares one centre on the rank axis, which is what makes that centre a
 * safe key to group by. Afterwards they no longer do, so this cannot be redone from
 * the positions it produces.
 *
 * A card only ever moves toward the root, and only inside the band dagre already
 * reserved for its rank -- the band is as wide as the rank's widest card, and that
 * card does not move at all. So rank separation is untouched and nothing can be
 * pushed into a neighbouring rank. Positions along the cross axis are left alone, so
 * cards within a rank keep their spacing too.
 *
 * A no-op when the root is not in the graph, and effectively one under `TB`, where
 * the rank axis is the fixed 108px card height and centred already is flush.
 */
function alignRanksTowardRoot(
  g: LaidOutGraph,
  rootId: string | null | undefined,
  rankdir: 'TB' | 'LR',
): void {
  if (!rootId || !g.hasNode(rootId)) return;
  // The axis ranks are separated along, and the node extent measured along it.
  const axis = rankdir === 'TB' ? 'y' : 'x';
  const extent = rankdir === 'TB' ? 'height' : 'width';
  const rootCentre = g.node(rootId)[axis];
  if (rootCentre === undefined) return;

  const ranks = new Map<number, string[]>();
  for (const id of g.nodes()) {
    const centre = g.node(id)[axis];
    if (centre === undefined) continue;
    const members = ranks.get(centre);
    if (members) members.push(id);
    else ranks.set(centre, [id]);
  }

  for (const [centre, members] of ranks) {
    // The root's own rank keeps dagre's centring: it is neither upstream nor
    // downstream of itself, so neither of its edges is the one facing the root.
    if (centre === rootCentre) continue;
    const widest = Math.max(...members.map((id) => g.node(id)[extent]));
    // Which way a card slides to go from centred to flush. Upstream of the root
    // (a smaller centre) it moves toward the root's side, i.e. up-axis; downstream,
    // the other way. Either way it is the edge nearest the root that lines up.
    const direction = centre < rootCentre ? 1 : -1;
    for (const id of members) {
      const node = g.node(id);
      node[axis] = centre + (direction * (widest - node[extent])) / 2;
    }
  }
}

/**
 * Hops from the root to every node it reaches: negative upstream, positive downstream,
 * and the shortest way round when there is more than one -- the same count the hop bar
 * selects on, so a `1+ / +1` graph is exactly the depths -1, 0 and 1. Walks every node
 * and edge, hidden ones included: a card's depth is its place in the lineage, not in
 * whatever the filter leaves on screen. `null` when the root is not in the graph.
 */
function depthsFromRoot(
  nodes: ReactFlowNode[],
  edges: Edge[],
  rootId: string | null | undefined,
): Map<string, number> | null {
  if (!rootId || !nodes.some((node) => node.id === rootId)) return null;
  const parents = new Map<string, string[]>();
  const children = new Map<string, string[]>();
  const link = (lists: Map<string, string[]>, from: string, to: string) => {
    const list = lists.get(from);
    if (list) list.push(to);
    else lists.set(from, [to]);
  };
  for (const { source, target } of edges) {
    link(children, source, target);
    link(parents, target, source);
  }
  const depths = new Map([[rootId, 0]]);
  for (const [next, step] of [
    [parents, -1],
    [children, 1],
  ] as const) {
    let frontier = [rootId];
    for (let depth = step; frontier.length > 0; depth += step) {
      const reached: string[] = [];
      for (const id of frontier) {
        for (const neighbour of next.get(id) ?? []) {
          if (depths.has(neighbour)) continue;
          depths.set(neighbour, depth);
          reached.push(neighbour);
        }
      }
      frontier = reached;
    }
  }
  return depths;
}

/**
 * A dagre ranker that puts every card in the column for its depth from the root.
 *
 * Dagre hands a ranker its own working copy of the graph, and a ranker's one job is to
 * give every node in it a `rank` so that each edge spans at least its `minlen` ranks.
 * That copy is not quite the graph we built: dagre has already stretched every
 * `minlen` (it doubles them, leaving a rank between columns for edge labels) and added
 * a bookkeeping root that points at every node. So the depths are scaled by the most
 * rank any edge asks for per column, and a node that isn't a card goes before
 * everything it points at.
 */
function rankByDepth(depths: Map<string, number>) {
  return (g: graphlib.Graph): void => {
    let scale = 1;
    for (const e of g.edges()) {
      const span = (depths.get(e.w) ?? NaN) - (depths.get(e.v) ?? NaN);
      if (span > 0) scale = Math.max(scale, Math.ceil(g.edge(e).minlen / span));
    }
    let first = Infinity;
    for (const v of g.nodes()) {
      const depth = depths.get(v);
      if (depth === undefined) continue;
      g.node(v).rank = depth * scale;
      first = Math.min(first, depth * scale);
    }
    for (const v of g.nodes()) {
      if (depths.has(v)) continue;
      const before = (g.outEdges(v) ?? []).map(
        (e: DagreEdge) => (g.node(e.w).rank ?? first) - g.edge(e).minlen,
      );
      g.node(v).rank = Math.min(first, ...before);
    }
  };
}

export function applyDagreLayout<T extends ReactFlowNode>(
  nodes: T[],
  edges: Edge[],
  options: LayoutOptions = {},
  /** The lineage root. Ranks are flushed against the edge facing it -- see
   *  {@link alignRanksTowardRoot}. Omit it and every rank stays dagre-centred. */
  rootId?: string | null,
  onDagreLayoutFailure?: (error: unknown) => void,
): T[] {
  if (nodes.length === 0) {
    return nodes;
  }

  const opts = { ...DEFAULT_LAYOUT_OPTIONS, ...options };

  // With a root, every card goes in the column for its depth from it, so cards the
  // same number of hops away line up. Dagre's own rankers can't promise that: they
  // rank by the edges between the cards, and a parent of the root that also feeds
  // another of its parents gets pushed a column further out, and hiding cards splits
  // the graph into pieces they rank on their own. So rank by depth instead. Every
  // visible card needs one, which it always has in a graph fetched around the root; if
  // one ever doesn't, dagre ranks as before.
  const depths = depthsFromRoot(nodes, edges, rootId);
  const pinned =
    depths != null && nodes.every((node) => node.hidden || depths.has(node.id));

  const g = new Dagre.graphlib.Graph({ directed: true, compound: false })
    .setGraph({
      rankdir: opts.rankdir,
      ranksep: opts.ranksep,
      nodesep: opts.nodesep,
      edgesep: opts.edgesep,
      // marginx: opts.marginx,
      // marginy: opts.marginy,
      // Dagre's types only admit the built-in ranker names, but its rank step calls a
      // function ranker directly (`lib/rank/index.ts`).
      ranker: pinned ? (rankByDepth(depths) as unknown as string) : opts.ranker,
      acyclicer: opts.acyclicer,
    })
    .setDefaultEdgeLabel(() => ({}));

  // Lay the graph out on the size each card actually renders to, not on one assumed
  // box. That is the whole point of measuring first: a card is as wide as its name.
  // A `hidden` node takes no room, so the rest close up over it; the edges below skip
  // it too, and it keeps its old position until it is shown and laid out again.
  nodes.forEach((node) => {
    if (!node.hidden) g.setNode(node.id, boxOf(node, opts));
  });

  // Add edges to dagre graph. With the columns pinned, only the edges that run out to
  // a deeper column: dagre expects every edge to point down the ranks, and one inside
  // a column (a parent of the root feeding another) has no order to add. React Flow
  // still draws it.
  edges.forEach((edge) => {
    if (!g.hasNode(edge.source) || !g.hasNode(edge.target)) return;
    if (pinned && depths.get(edge.target)! <= depths.get(edge.source)!) return;
    g.setEdge(edge.source, edge.target);
  });

  try {
    Dagre.layout(g);
  } catch (error) {
    onDagreLayoutFailure?.(error);
    return applyGridFallbackLayout(nodes, opts);
  }

  alignRanksTowardRoot(g, rootId, opts.rankdir);

  // Apply calculated positions to nodes
  const handles = handlePositionsForRankdir(opts.rankdir);
  return nodes.map((node) => {
    const dagreNode = g.node(node.id);

    if (!dagreNode) {
      return { ...node, ...handles };
    }

    return {
      ...node,
      ...handles,
      position: {
        // dagre reports a node's CENTRE; React Flow positions by the top-left corner.
        // Convert with the node's own box — `dagreNode.width`/`.height` are what was
        // handed to `setNode` above. Halving the constant instead would undo the
        // measuring: dagre would space the rank for a 600px card and then place it as
        // if it were 245px, dropping it 178px left of its slot and onto its neighbour.
        x: Math.round(dagreNode.x - dagreNode.width / 2),
        y: Math.round(dagreNode.y - dagreNode.height / 2),
      },
    };
  });
}

/** Last resort when dagre throws: a plain grid, spaced by the widest and tallest card
 *  so variable-size nodes still clear each other. */
export function applyGridFallbackLayout<T extends ReactFlowNode>(
  nodes: T[],
  opts: Required<LayoutOptions>,
): T[] {
  const cols = Math.ceil(Math.sqrt(nodes.length));
  const handles = handlePositionsForRankdir(opts.rankdir);
  const boxes = nodes.map((node) => boxOf(node, opts));
  const cellWidth = Math.max(...boxes.map((b) => b.width)) + opts.nodesep;
  const cellHeight = Math.max(...boxes.map((b) => b.height)) + opts.ranksep;
  return nodes.map((node, index) => {
    const col = index % cols;
    const row = Math.floor(index / cols);
    return {
      ...node,
      ...handles,
      position: {
        x: Math.round(opts.marginx + col * cellWidth),
        y: Math.round(opts.marginy + row * cellHeight),
      },
    };
  });
}

/**
 * Hide the cards while React Flow measures them.
 *
 * `style: { visibility: 'hidden' }`, deliberately, and NOT React Flow's `hidden`
 * property. A node with `hidden: true` renders `null` and never gets a ResizeObserver
 * attached, so it is never measured — the layout waiting on those measurements would
 * never run and the graph would never appear. `visibility: hidden` keeps the card in
 * the DOM at its true size and merely stops it being painted, which is the only form
 * of hidden that is still measurable.
 */
export function hideForMeasurement<T extends ReactFlowNode>(nodes: T[]): T[] {
  return nodes.map((node) => ({
    ...node,
    style: { ...node.style, visibility: 'hidden' as const },
  }));
}

/** Undo {@link hideForMeasurement}, once the cards have somewhere to be. */
export function reveal<T extends ReactFlowNode>(nodes: T[]): T[] {
  return nodes.map((node) => ({
    ...node,
    style: { ...node.style, visibility: 'visible' as const },
  }));
}

/** Whether every card carries the size React Flow measured for it — the precondition
 *  for laying out. One unmeasured card would be placed on the fallback box and land on
 *  its neighbours, which is the whole failure this protocol exists to avoid. */
export function allMeasured(nodes: ReactFlowNode[]): boolean {
  return (
    nodes.length > 0 && nodes.every((n) => n.measured?.width && n.measured?.height)
  );
}
