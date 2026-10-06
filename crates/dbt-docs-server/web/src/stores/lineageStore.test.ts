import type { Edge, Node as ReactFlowNode } from '@xyflow/react';
import { beforeEach, describe, expect, it } from 'vitest';

import { ResourceType } from '../components/LineageV2/ResourceTypes';
import { isCardCompact, useLineageStore } from './lineageStore';

const ROOT = 'model.jaffle_shop.customers';

function graph(): { nodes: ReactFlowNode[]; edges: Edge[] } {
  return {
    nodes: [
      { id: 'a', position: { x: 0, y: 0 }, data: { label: 'a' } },
      { id: 'b', position: { x: 0, y: 0 }, data: { label: 'b' } },
    ],
    edges: [{ id: 'a-b', source: 'a', target: 'b' }],
  };
}

/** Stand in for React Flow: report the size it would have measured off each card.
 *  This is what `onNodesChange` does in the app, and what `layoutMeasured` waits for. */
function measure(sizes: Record<string, { width: number; height: number }>) {
  useLineageStore.getState().onNodesChange(
    Object.entries(sizes).map(([id, dimensions]) => ({
      id,
      type: 'dimensions' as const,
      dimensions,
      setAttributes: true,
    })),
  );
}

const SAME_SIZES = {
  a: { width: 245, height: 108 },
  b: { width: 245, height: 108 },
};

beforeEach(() => {
  useLineageStore.getState().reset();
});

describe('lineageStore', () => {
  it('starts empty', () => {
    const state = useLineageStore.getState();
    expect(state.status).toBe('empty');
    expect(state.nodes).toEqual([]);
    expect(state.rootUniqueId).toBeNull();
  });

  it('publishes the graph hidden and unpositioned on hydrate', () => {
    useLineageStore.getState().hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 3,
      downstreamDepth: 3,
      ...graph(),
    });

    const { nodes, edges, status, rootUniqueId, isLaidOut } =
      useLineageStore.getState();
    expect(status).toBe('ready');
    expect(rootUniqueId).toBe(ROOT);
    expect(edges).toHaveLength(1);
    expect(isLaidOut).toBe(false);
    // Hidden with `visibility`, not React Flow's `hidden` — a `hidden` node renders
    // null and is never measured, so the layout waiting on it would never run.
    for (const node of nodes) {
      expect(node.style?.visibility).toBe('hidden');
      expect(node.hidden).toBeUndefined();
      expect(node.position).toEqual({ x: 0, y: 0 });
    }
  });

  it('will not lay out until every card has been measured', () => {
    const store = useLineageStore.getState();
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 3,
      downstreamDepth: 3,
      ...graph(),
    });

    // One of the two measured. Laying out now would place `b` on the fallback box and
    // risk dropping it on its neighbour.
    measure({ a: { width: 245, height: 108 } });
    store.layoutMeasured();
    expect(useLineageStore.getState().isLaidOut).toBe(false);
    expect(useLineageStore.getState().nodes[0].style?.visibility).toBe('hidden');

    measure({ b: { width: 245, height: 108 } });
    store.layoutMeasured();
    expect(useLineageStore.getState().isLaidOut).toBe(true);
  });

  it('lays out on the measured sizes and reveals the cards', () => {
    const store = useLineageStore.getState();
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 3,
      downstreamDepth: 3,
      ...graph(),
    });
    measure(SAME_SIZES);
    store.layoutMeasured();

    const { nodes } = useLineageStore.getState();
    // dagre puts a source-side node left of its target with rankdir LR, so the
    // positions the store publishes are real, not the incoming (0, 0).
    expect(nodes[0].position.x).toBeLessThan(nodes[1].position.x);
    for (const node of nodes) expect(node.style?.visibility).toBe('visible');
  });

  it('gives a wider card a wider slot', () => {
    const store = useLineageStore.getState();

    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 3,
      downstreamDepth: 3,
      ...graph(),
    });
    measure(SAME_SIZES);
    store.layoutMeasured();
    const narrowGap =
      useLineageStore.getState().nodes[1].position.x -
      useLineageStore.getState().nodes[0].position.x;

    store.reset();
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 3,
      downstreamDepth: 3,
      ...graph(),
    });
    measure({ a: { width: 600, height: 108 }, b: { width: 245, height: 108 } });
    store.layoutMeasured();
    const wideGap =
      useLineageStore.getState().nodes[1].position.x -
      useLineageStore.getState().nodes[0].position.x;

    // The whole point: the graph is spaced by what the card actually renders to, so a
    // 600px card pushes its target further right than a 245px one does.
    expect(wideGap - narrowGap).toBe(600 - 245);
  });

  it('does not lay out twice', () => {
    const store = useLineageStore.getState();
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 3,
      downstreamDepth: 3,
      ...graph(),
    });
    measure(SAME_SIZES);
    store.layoutMeasured();

    const nodes = useLineageStore.getState().nodes;
    store.layoutMeasured();
    // Same array, not just equal: the hook calls this on an effect, and re-publishing
    // the graph on every pass would re-render the canvas for nothing.
    expect(useLineageStore.getState().nodes).toBe(nodes);
  });

  it('keeps the graph across a refetch of the same root, drops it on a new root', () => {
    const store = useLineageStore.getState();
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 3,
      downstreamDepth: 3,
      ...graph(),
    });

    store.startHydration(ROOT, 3, 3);
    expect(useLineageStore.getState().nodes).toHaveLength(2);
    expect(useLineageStore.getState().status).toBe('loading');

    store.startHydration('model.jaffle_shop.orders', 3, 3);
    expect(useLineageStore.getState().nodes).toEqual([]);
    expect(useLineageStore.getState().rootUniqueId).toBe('model.jaffle_shop.orders');
  });

  it('records the hop depths as the fetch opens, keeping the graph up meanwhile', () => {
    const store = useLineageStore.getState();
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 1,
      downstreamDepth: 1,
      ...graph(),
    });
    measure(SAME_SIZES);
    store.layoutMeasured();

    // The depths land immediately, before the wider fetch resolves -- but the graph
    // on screen is still correct lineage, just narrower, so it stays up rather than
    // flashing the canvas empty.
    store.startHydration(ROOT, 2, 1);
    const loading = useLineageStore.getState();
    expect(loading.upstreamDepth).toBe(2);
    expect(loading.downstreamDepth).toBe(1);
    expect(loading.status).toBe('loading');
    expect(loading.nodes).toHaveLength(2);
    expect(loading.isLaidOut).toBe(true);

    // A new root, on the other hand, still drops everything -- including the depths
    // it was requested at.
    store.startHydration('model.jaffle_shop.orders', 1, 1);
    const swapped = useLineageStore.getState();
    expect(swapped.nodes).toEqual([]);
    expect(swapped.isLaidOut).toBe(false);
    expect(swapped.upstreamDepth).toBe(1);
    expect(swapped.downstreamDepth).toBe(1);
  });

  it('does not collapse cards to badges while they are being measured', () => {
    const store = useLineageStore.getState();
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 1,
      downstreamDepth: 1,
      ...graph(),
    });
    measure(SAME_SIZES);
    store.layoutMeasured();
    const laidOutGap =
      useLineageStore.getState().nodes[1].position.x -
      useLineageStore.getState().nodes[0].position.x;

    // Zoom out past the LOD threshold: the cards on screen collapse to badges.
    store.setCompact(true);
    expect(isCardCompact(useLineageStore.getState())).toBe(true);

    // Now change hops. The new graph arrives unmeasured, so the cards go back in the
    // DOM to be measured -- and must be measured at full size even though the
    // viewport is still zoomed out. Measuring them collapsed would hand dagre a
    // badge-sized box for a full-sized card, and the first fitView back past the
    // threshold would expand them onto each other.
    store.startHydration(ROOT, 2, 2);
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 2,
      downstreamDepth: 2,
      ...graph(),
    });
    expect(useLineageStore.getState().isCompact).toBe(true);
    expect(isCardCompact(useLineageStore.getState())).toBe(false);

    // Same root and same card sizes as the first pass, so the same layout.
    measure(SAME_SIZES);
    store.layoutMeasured();
    expect(
      useLineageStore.getState().nodes[1].position.x -
        useLineageStore.getState().nodes[0].position.x,
    ).toBe(laidOutGap);

    // Laid out again, so the LOD collapse is back on.
    expect(isCardCompact(useLineageStore.getState())).toBe(true);
  });

  it('keeps the full card size while the cards are collapsed to badges', () => {
    const store = useLineageStore.getState();
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 1,
      downstreamDepth: 1,
      ...graph(),
    });
    measure(SAME_SIZES);
    store.layoutMeasured();

    // Zoomed out past the LOD threshold, React Flow re-measures every card as the
    // badge it collapsed to. Keeping that would have the next layout -- after a filter
    // change, say -- space the graph for badges, and the cards would land on each
    // other as soon as they expand back.
    store.setCompact(true);
    measure({ a: { width: 90, height: 24 }, b: { width: 90, height: 24 } });
    expect(useLineageStore.getState().nodes.map((n) => n.measured)).toEqual([
      SAME_SIZES.a,
      SAME_SIZES.b,
    ]);

    // The selected card stays full size when the rest collapse, so a new size for it
    // is a real one.
    store.onNodesChange([{ id: 'a', type: 'select', selected: true }]);
    measure({ a: { width: 300, height: 108 } });
    expect(useLineageStore.getState().nodes[0].measured).toEqual({
      width: 300,
      height: 108,
    });
  });

  it('tracks selection separately from nodes, without churning its identity', () => {
    const store = useLineageStore.getState();
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 3,
      downstreamDepth: 3,
      ...graph(),
    });
    measure(SAME_SIZES);
    store.layoutMeasured();
    // Neither node has a resource type, so the filter lets both through.
    const before = useLineageStore.getState().selectedNodeIds;
    expect(before).toEqual(['a', 'b']);

    // The point of holding selection as its own field: a click or a drag republishes
    // `nodes` but must leave `selectedNodeIds` referentially identical, so subscribers
    // -- the canvas re-fitting to it, for one -- stay put.
    store.onNodesChange([{ id: 'a', type: 'select', selected: true }]);
    store.onNodesChange([
      { id: 'a', type: 'position', position: { x: 10, y: 10 }, dragging: true },
    ]);
    const after = useLineageStore.getState();
    expect(after.nodes[0].position).toEqual({ x: 10, y: 10 });
    expect(after.nodes[0].selected).toBe(true);
    expect(after.selectedNodeIds).toBe(before);
  });

  it('records fetch failures and unsupported sources', () => {
    const store = useLineageStore.getState();
    store.failHydration(new Error('boom'));
    expect(useLineageStore.getState().status).toBe('error');
    expect(useLineageStore.getState().error?.message).toBe('boom');

    store.markUnsupported();
    expect(useLineageStore.getState().status).toBe('unsupported');
    expect(useLineageStore.getState().error).toBeNull();
  });

  it('relayouts the graph already in the store, without hiding it again', () => {
    const store = useLineageStore.getState();
    store.hydrate({
      rootUniqueId: ROOT,
      upstreamDepth: 3,
      downstreamDepth: 3,
      ...graph(),
    });
    measure(SAME_SIZES);
    store.layoutMeasured();
    const lr = useLineageStore.getState().nodes.map((n) => n.position);

    store.relayout({ rankdir: 'TB' });
    const { nodes, layout } = useLineageStore.getState();
    const tb = nodes.map((n) => n.position);
    expect(layout.rankdir).toBe('TB');
    expect(tb).not.toEqual(lr);
    // Top-to-bottom: the source now sits above its target.
    expect(tb[0].y).toBeLessThan(tb[1].y);
    // The cards are already measured, so flipping direction must not blank the canvas.
    for (const node of nodes) expect(node.style?.visibility).toBe('visible');
  });

  describe('selectedNodeIds', () => {
    // Seed and source feed the model, and a test hangs off it. The test is the one
    // type here that isn't visible by default.
    function typedGraph(): { nodes: ReactFlowNode[]; edges: Edge[] } {
      return {
        nodes: [
          { id: 'm', position: { x: 0, y: 0 }, data: { resourceType: 'model' } },
          { id: 's', position: { x: 0, y: 0 }, data: { resourceType: 'source' } },
          { id: 't', position: { x: 0, y: 0 }, data: { resourceType: 'test' } },
          { id: 'x', position: { x: 0, y: 0 }, data: { resourceType: 'seed' } },
        ],
        edges: [
          { id: 's-m', source: 's', target: 'm' },
          { id: 'x-m', source: 'x', target: 'm' },
          { id: 'm-t', source: 'm', target: 't' },
        ],
      };
    }

    const TYPED_SIZES = {
      m: { width: 245, height: 108 },
      s: { width: 245, height: 108 },
      t: { width: 245, height: 108 },
      x: { width: 245, height: 108 },
    };

    function hydrateTyped() {
      useLineageStore.getState().hydrate({
        rootUniqueId: ROOT,
        upstreamDepth: 3,
        downstreamDepth: 3,
        ...typedGraph(),
      });
    }

    function layOutTyped() {
      hydrateTyped();
      measure(TYPED_SIZES);
      useLineageStore.getState().layoutMeasured();
    }

    const hiddenNodeIds = () =>
      useLineageStore
        .getState()
        .nodes.filter((n) => n.hidden)
        .map((n) => n.id);
    const hiddenEdgeIds = () =>
      useLineageStore
        .getState()
        .edges.filter((e) => e.hidden)
        .map((e) => e.id);
    const positionOf = (id: string) =>
      useLineageStore.getState().nodes.find((n) => n.id === id)!.position;

    it('starts as the nodes the filter lets through, shown once laid out', () => {
      hydrateTyped();
      expect(useLineageStore.getState().selectedNodeIds).toEqual(['m', 's', 'x']);
      // A `hidden` node is never measured, so while the cards are being measured
      // every one of them has to stay in the DOM -- the unselected test included.
      expect(hiddenNodeIds()).toEqual([]);

      measure(TYPED_SIZES);
      useLineageStore.getState().layoutMeasured();
      expect(useLineageStore.getState().isLaidOut).toBe(true);
      expect(hiddenNodeIds()).toEqual(['t']);
      expect(hiddenEdgeIds()).toEqual(['m-t']);
    });

    it('shows exactly the nodes it is set to, and lays them out again', () => {
      layOutTyped();
      const modelBefore = positionOf('m');

      useLineageStore.getState().setSelectedNodeIds(['m', 't']);
      expect(useLineageStore.getState().selectedNodeIds).toEqual(['m', 't']);
      expect(hiddenNodeIds()).toEqual(['s', 'x']);
      expect(hiddenEdgeIds()).toEqual(['s-m', 'x-m']);
      // With its upstream hidden the model moves up into the first rank, rather than
      // leaving a gap where the source and seed were...
      expect(positionOf('m').x).toBeLessThan(modelBefore.x);
      // ...and the test, never laid out while it was hidden, is placed downstream.
      expect(positionOf('t').x).toBeGreaterThan(positionOf('m').x);
      for (const node of useLineageStore.getState().nodes) {
        expect(node.style?.visibility).toBe('visible');
      }
    });

    it('ignores the same selection in another order', () => {
      layOutTyped();
      const before = useLineageStore.getState();
      before.setSelectedNodeIds(['x', 's', 'm']);
      const after = useLineageStore.getState();
      // Same identity, so nothing subscribed to the selection re-fits for nothing.
      expect(after.selectedNodeIds).toBe(before.selectedNodeIds);
      expect(after.nodes).toBe(before.nodes);
    });

    it('holds a selection set mid-measurement until the layout applies it', () => {
      hydrateTyped();
      useLineageStore.getState().setSelectedNodeIds(['m']);
      expect(hiddenNodeIds()).toEqual([]);

      measure(TYPED_SIZES);
      useLineageStore.getState().layoutMeasured();
      expect(hiddenNodeIds()).toEqual(['s', 't', 'x']);
    });

    it('deselects the nodes it hides on the canvas', () => {
      layOutTyped();
      useLineageStore.getState().onNodesChange(
        ['m', 's', 'x'].map((id) => ({
          id,
          type: 'select' as const,
          selected: true,
        })),
      );
      useLineageStore.getState().setSelectedNodeIds(['m', 't']);
      const clicked = useLineageStore
        .getState()
        .nodes.filter((n) => n.selected)
        .map((n) => n.id);
      expect(clicked).toEqual(['m']);
    });

    describe('setResourceTypesVisible', () => {
      it('shows exactly the given types, without mutating the registry it replaces', () => {
        const before = useLineageStore.getState().visibleResourceTypes;
        expect(before).toEqual(
          new Set<ResourceType>(['model', 'exposure', 'source', 'seed']),
        );
        useLineageStore.getState().setResourceTypesVisible(new Set(['model', 'test']));

        expect(useLineageStore.getState().visibleResourceTypes).toEqual(
          new Set<ResourceType>(['model', 'test']),
        );
        expect(useLineageStore.getState().visibleResourceTypes).not.toBe(before);
        expect(before.has('source')).toBe(true);
        expect(before.has('test')).toBe(false);

        // `reset` restores the defaults, which only holds if the initial registry was
        // never written to.
        useLineageStore.getState().reset();
        expect(useLineageStore.getState().visibleResourceTypes).toEqual(
          new Set<ResourceType>(['model', 'exposure', 'source', 'seed']),
        );
      });

      it('selects the nodes of the visible types, and lays them out again', () => {
        layOutTyped();
        const modelBefore = positionOf('m');

        useLineageStore.getState().setResourceTypesVisible(new Set(['model', 'test']));
        expect(useLineageStore.getState().selectedNodeIds).toEqual(['m', 't']);
        expect(hiddenNodeIds()).toEqual(['s', 'x']);
        expect(positionOf('m').x).toBeLessThan(modelBefore.x);
      });

      it('holds a filter set mid-measurement until the layout applies it', () => {
        hydrateTyped();
        useLineageStore.getState().setResourceTypesVisible(new Set(['model']));
        expect(useLineageStore.getState().selectedNodeIds).toEqual(['m']);
        expect(hiddenNodeIds()).toEqual([]);

        measure(TYPED_SIZES);
        useLineageStore.getState().layoutMeasured();
        expect(hiddenNodeIds()).toEqual(['s', 't', 'x']);
      });

      it('leaves the selection alone when the filter lets the same nodes through', () => {
        layOutTyped();
        const { nodes, edges, selectedNodeIds } = useLineageStore.getState();
        // This graph has no macros, so turning them on selects nothing new.
        useLineageStore
          .getState()
          .setResourceTypesVisible(
            new Set(['model', 'exposure', 'source', 'seed', 'macro']),
          );

        expect(useLineageStore.getState().visibleResourceTypes.has('macro')).toBe(true);
        // Same identity: no relayout, and the viewport doesn't re-fit either.
        expect(useLineageStore.getState().selectedNodeIds).toBe(selectedNodeIds);
        expect(useLineageStore.getState().nodes).toBe(nodes);
        expect(useLineageStore.getState().edges).toBe(edges);
      });

      it("keeps models and the root's own type on, whatever it is handed", () => {
        // A test is off by default, but this graph is about one.
        const testRoot = 'test.jaffle_shop.not_null_id';
        useLineageStore.getState().hydrate({
          rootUniqueId: testRoot,
          upstreamDepth: 1,
          downstreamDepth: 1,
          nodes: [
            { id: 'm', position: { x: 0, y: 0 }, data: { resourceType: 'model' } },
            { id: testRoot, position: { x: 0, y: 0 }, data: { resourceType: 'test' } },
          ],
          edges: [{ id: 'm-t', source: 'm', target: testRoot }],
        });
        expect(useLineageStore.getState().selectedNodeIds).toEqual(['m', testRoot]);

        useLineageStore.getState().setResourceTypesVisible(new Set(['source']));
        expect(useLineageStore.getState().visibleResourceTypes).toEqual(
          new Set<ResourceType>(['source', 'model', 'test']),
        );
      });

      it('leaves the store alone when the types do not change', () => {
        layOutTyped();
        const before = useLineageStore.getState();
        // Same as the defaults.
        before.setResourceTypesVisible(
          new Set(['model', 'exposure', 'source', 'seed']),
        );
        const after = useLineageStore.getState();
        expect(after.visibleResourceTypes).toBe(before.visibleResourceTypes);
        expect(after.nodes).toBe(before.nodes);
        expect(after.selectedNodeIds).toBe(before.selectedNodeIds);
      });
    });
  });
});
