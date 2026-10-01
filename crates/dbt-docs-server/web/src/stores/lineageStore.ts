import {
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  type Edge,
  type Node as ReactFlowNode,
  type OnConnect,
  type OnEdgesChange,
  type OnNodesChange,
} from '@xyflow/react';
import { create } from 'zustand';
import { devtools } from 'zustand/middleware';
import { useShallow } from 'zustand/react/shallow';

import {
  allMeasured,
  applyDagreLayout,
  DEFAULT_LAYOUT_OPTIONS,
  hideForMeasurement,
  type LayoutOptions,
  reveal,
} from '../lib/dagreLayout';

/**
 * App-wide React Flow state.
 *
 * Every lineage surface reads its graph from here rather than holding its own
 * `useNodesState`/`useEdgesState`. Two reasons:
 *
 * 1. The graph is hydrated once, from DuckDB, by `useHydrateLineageStore`. Sibling
 *    views ("variants of the graph" — a minimap, a selection sidebar, a column-level
 *    overlay) then render off the same nodes and edges without each refetching or
 *    re-laying-out the same lineage.
 * 2. Performance. React Flow's own store publishes `nodes` on every drag, pan and
 *    zoom frame, so a component that subscribes to `nodes` just to derive something
 *    small from it re-renders continuously. The fix the React Flow docs recommend is
 *    exactly this: keep the derived thing (here, `selectedNodeIds`) as its own store
 *    field, so subscribers wake only when that field actually changes.
 *    https://reactflow.dev/learn/advanced-use/performance#optimized-solution
 *
 * This is a module singleton, so it holds one graph at a time — the one identified by
 * `rootUniqueId`. Hydrating a different root replaces it (see `startHydration`). Two
 * lineage roots rendered side by side would need a context-scoped store instead.
 */

/** Where the store's current graph is in its load cycle. */
export type LineageStatus =
  | 'empty'
  | 'loading'
  | 'ready'
  | 'error'
  /** The active data source has no `fetchLineage` — nothing is coming. */
  | 'unsupported';

/** The graph plus the root it was hydrated for. */
export type LineageGraphInput = {
  rootUniqueId: string;
  upstreamDepth: number;
  downstreamDepth: number;
  /** Positions are ignored. Hydration hides the cards and leaves them stacked; they
   *  are positioned by `layoutMeasured` once React Flow has measured them. */
  nodes: ReactFlowNode[];
  edges: Edge[];
};

export type LineageState = {
  // ---- graph ----
  nodes: ReactFlowNode[];
  edges: Edge[];
  /** Layout the current positions were computed with; reused by `relayout`. */
  layout: Required<LayoutOptions>;
  /** False between hydration and the measured layout, i.e. while the cards are in the
   *  DOM being measured but not yet placed or painted. `useLayoutWhenMeasured` is what
   *  moves it to true. */
  isLaidOut: boolean;

  // ---- hydration ----
  /** The lineage root the graph in the store belongs to, `null` when empty. */
  rootUniqueId: string | null;
  /** Hop counts being fetched -- the hop bar's `n+ / +n`. Written by
   *  `startHydration` as the fetch opens, so between a hop change and its graph
   *  landing these describe the request in flight, not the graph still on screen. */
  upstreamDepth: number;
  downstreamDepth: number;
  status: LineageStatus;
  error: Error | null;

  /** Ids of the selected nodes, maintained by `onNodesChange`. Subscribe to this
   *  instead of filtering `nodes` — see the note above. */
  selectedNodeIds: string[];

  /** Level-of-detail: true below the zoom threshold, where individual node cards
   *  read as unreadable clutter and every unselected node should collapse to just
   *  its resource badge. Written from a single `onMove` callback on <ReactFlow>
   *  (see BaseDag), guarded so `set` only fires on the rare frame the threshold is
   *  actually crossed -- not from each node polling `useViewport()` itself, which
   *  would re-render every node on every pan/zoom frame. */
  isCompact: boolean;

  /** Which lens DagLensesDropdown has selected -- lives here, not local state
   *  in that component, so DagNode can read it too and render the matching
   *  badge. See lib/lensBadges for the label→badge mapping. */
  activeLens: string;

  // ---- React Flow handlers, wired straight into <ReactFlow> ----
  onNodesChange: OnNodesChange;
  onEdgesChange: OnEdgesChange;
  onConnect: OnConnect;

  // ---- actions ----
  setNodes: (nodes: ReactFlowNode[]) => void;
  setEdges: (edges: Edge[]) => void;
  setCompact: (isCompact: boolean) => void;
  setActiveLens: (lens: string) => void;
  /** Mark a fetch as in flight, and record the root and hop depths it is for.
   *  Clears the graph when the root changes, so a stale graph never shows under a
   *  new root; a depth change keeps it, since a different hop count is still a view
   *  of the same lineage. */
  startHydration: (
    rootUniqueId: string,
    upstreamDepth: number,
    downstreamDepth: number,
  ) => void;
  /** The bootstrap action: publish the graph DuckDB returned, hidden and unpositioned,
   *  for React Flow to measure. `layoutMeasured` finishes the job. */
  hydrate: (input: LineageGraphInput) => void;
  /** Lay the graph out on the sizes React Flow measured, and reveal it. Called by
   *  `useLayoutWhenMeasured` once every card has a size; a no-op before that, and
   *  after the graph is already laid out. */
  layoutMeasured: () => void;
  failHydration: (error: Error) => void;
  markUnsupported: () => void;
  /** Re-run dagre over the graph already in the store, e.g. to flip LR ⇄ TB. */
  relayout: (options: LayoutOptions) => void;
  reset: () => void;
};

const EMPTY_SELECTION: string[] = [];

const initialState = {
  nodes: [] as ReactFlowNode[],
  edges: [] as Edge[],
  layout: DEFAULT_LAYOUT_OPTIONS,
  isLaidOut: false,
  rootUniqueId: null,
  upstreamDepth: 1,
  downstreamDepth: 1,
  status: 'empty' as LineageStatus,
  error: null,
  selectedNodeIds: EMPTY_SELECTION,
  isCompact: false,
  activeLens: 'Default',
};

function sameIds(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((id, i) => id === b[i]);
}

/** Recompute the selection, reusing the previous array when it hasn't changed. A new
 *  array on every drag frame would defeat the point of holding selection separately —
 *  `useShallow` compares members, but a subscriber to `selectedNodeIds` alone compares
 *  by reference. */
function nextSelection(nodes: ReactFlowNode[], previous: string[]): string[] {
  const ids = nodes.filter((n) => n.selected).map((n) => n.id);
  return sameIds(previous, ids) ? previous : ids;
}

/**
 * `create<T>()(...)` — note the empty call before the initializer — is required, not a
 * style choice. TypeScript has no higher-kinded types, so zustand cannot infer `T`
 * through a middleware wrapper; the curried form pins `T` first and lets the
 * middleware's own generics flow. `create<T>(devtools(...))` infers the state as
 * `unknown` inside the initializer.
 * https://zustand.docs.pmnd.rs/learn/guides/advanced-typescript
 *
 * The third argument to `set` is the devtools action label. Without it every entry in
 * the Redux DevTools timeline reads `anonymous`, which makes the timeline useless on a
 * store this size.
 */
export const useLineageStore = create<LineageState>()(
  devtools(
    (set, get) => ({
      ...initialState,

      onNodesChange: (changes) => {
        const nodes = applyNodeChanges(changes, get().nodes);
        set(
          { nodes, selectedNodeIds: nextSelection(nodes, get().selectedNodeIds) },
          false,
          'lineage/onNodesChange',
        );
      },

      onEdgesChange: (changes) => {
        set(
          { edges: applyEdgeChanges(changes, get().edges) },
          false,
          'lineage/onEdgesChange',
        );
      },

      onConnect: (connection) => {
        set({ edges: addEdge(connection, get().edges) }, false, 'lineage/onConnect');
      },

      setNodes: (nodes) => {
        set(
          { nodes, selectedNodeIds: nextSelection(nodes, get().selectedNodeIds) },
          false,
          'lineage/setNodes',
        );
      },

      setEdges: (edges) => {
        set({ edges }, false, 'lineage/setEdges');
      },

      setCompact: (isCompact) => {
        // The caller (BaseDag's onMove) already guards this to only fire on the
        // frame the threshold is crossed, but guard here too -- this is a set on
        // a module singleton, cheap insurance against a redundant write if that
        // ever changes.
        if (get().isCompact === isCompact) return;
        set({ isCompact }, false, 'lineage/setCompact');
      },

      setActiveLens: (lens) => {
        set({ activeLens: lens }, false, 'lineage/setActiveLens');
      },

      startHydration: (rootUniqueId, upstreamDepth, downstreamDepth) => {
        const isSameRoot = get().rootUniqueId === rootUniqueId;
        set(
          {
            rootUniqueId,
            // The depths describe the fetch, not the graph below, so they land now
            // rather than at `hydrate`. Everything else here still keys off the root
            // alone: a hop change is a wider or narrower view of lineage that is
            // already on screen and still correct, so it keeps rendering until the
            // new graph lands.
            upstreamDepth,
            downstreamDepth,
            status: 'loading',
            error: null,
            // Keep the current graph while refetching the same root — dropping it
            // would flash the canvas empty. A different root has nothing worth
            // keeping.
            nodes: isSameRoot ? get().nodes : [],
            edges: isSameRoot ? get().edges : [],
            selectedNodeIds: isSameRoot ? get().selectedNodeIds : EMPTY_SELECTION,
            // A new root's graph has not been laid out; the old root's still is, and
            // dropping that would hide a canvas that is on screen and correct.
            isLaidOut: isSameRoot ? get().isLaidOut : false,
          },
          false,
          'lineage/startHydration',
        );
      },

      hydrate: ({ rootUniqueId, upstreamDepth, downstreamDepth, nodes, edges }) => {
        set(
          {
            rootUniqueId,
            upstreamDepth,
            downstreamDepth,
            // Hidden and stacked wherever they came in. There is nothing to lay out
            // with yet: a card's width is whatever its name renders to, and only the
            // browser knows that. So publish them for React Flow to measure, and let
            // `layoutMeasured` place them once it has.
            nodes: hideForMeasurement(nodes),
            edges,
            status: 'ready',
            error: null,
            selectedNodeIds: EMPTY_SELECTION,
            isLaidOut: false,
          },
          false,
          'lineage/hydrate',
        );
      },

      layoutMeasured: () => {
        const { nodes, edges, layout, isLaidOut, rootUniqueId } = get();
        if (isLaidOut || !allMeasured(nodes)) return;
        set(
          {
            nodes: reveal(applyDagreLayout(nodes, edges, layout, rootUniqueId)),
            isLaidOut: true,
          },
          false,
          'lineage/layoutMeasured',
        );
      },

      failHydration: (error) => {
        set({ status: 'error', error }, false, 'lineage/failHydration');
      },

      markUnsupported: () => {
        set({ status: 'unsupported', error: null }, false, 'lineage/markUnsupported');
      },

      relayout: (options) => {
        const layout = { ...get().layout, ...options };
        // The cards are already measured by the time anything can ask for this, so it
        // runs straight away rather than going back through the hidden-and-measure
        // cycle — flipping LR ⇄ TB should not blank the canvas.
        set(
          {
            layout,
            nodes: reveal(
              applyDagreLayout(get().nodes, get().edges, layout, get().rootUniqueId),
            ),
          },
          false,
          'lineage/relayout',
        );
      },

      reset: () => {
        // `false`, not `true`: a replacing set would drop the actions along with the
        // state, leaving a store whose methods are gone.
        set(initialState, false, 'lineage/reset');
      },
    }),
    {
      // `enabled` is deliberately unset: zustand defaults it to
      // `import.meta.env.MODE !== 'production'`, which is already correct under vite,
      // and reading `import.meta.env` here trips the eslint config's
      // `turbo/no-undeclared-env-vars` (there is no turbo.json to declare it in).
      // Production builds therefore get the plain store, with no extension hook.
      name: 'lineage-store',
    },
  ),
);

// whether the card should render in the collapsed badge-only form
export const isCardCompact = (state: LineageState): boolean =>
  state.isCompact && state.isLaidOut;

/** The props `<ReactFlow>` needs, in one subscription. `useShallow` keeps the fresh
 *  object literal from re-rendering the canvas on every unrelated store write. */
const flowSelector = (state: LineageState) => ({
  nodes: state.nodes,
  edges: state.edges,
  onNodesChange: state.onNodesChange,
  onEdgesChange: state.onEdgesChange,
  onConnect: state.onConnect,
});

export function useLineageFlow() {
  return useLineageStore(useShallow(flowSelector));
}

/** Hydration state for the loading / error / unsupported branches, without
 *  subscribing to the graph itself. */
const statusSelector = (state: LineageState) => ({
  status: state.status,
  error: state.error,
  rootUniqueId: state.rootUniqueId,
});

export function useLineageStatus() {
  return useLineageStore(useShallow(statusSelector));
}
