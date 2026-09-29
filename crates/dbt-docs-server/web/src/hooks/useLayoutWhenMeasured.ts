import { useEffect } from 'react';
import { useNodesInitialized } from '@xyflow/react';

import { allMeasured } from '../lib/dagreLayout';
import { type LineageState, useLineageStore } from '../stores/lineageStore';

/** Whether the measurements have reached the store's own nodes. A primitive, so this
 *  subscription wakes only when the answer flips rather than on every store write. */
const measuredSelector = (state: LineageState) => allMeasured(state.nodes);

/**
 * The second half of the layout: wait for React Flow to measure the cards, then lay
 * the graph out on the sizes it found.
 *
 * A card's width is whatever its name renders to, and only the browser knows that. So
 * `hydrate` publishes the graph hidden and unpositioned, this hook waits for the
 * measurements, and `layoutMeasured` places the cards and reveals them. Nothing is
 * painted in between, so the stacked intermediate state is never seen.
 *
 * Two signals, because they answer different questions. `useNodesInitialized` is React
 * Flow's: every node it is rendering has been measured and has handle bounds. The
 * store's `allMeasured` is ours: those measurements have actually landed on the node
 * objects `applyDagreLayout` will read, which happens a tick later, when the dimension
 * changes come back through `onNodesChange`. Laying out on the first signal alone would
 * catch some cards still on the fallback box.
 *
 * Call it under a mounted `<ReactFlow>` — that is what does the measuring, and until it
 * is on screen there is nothing to wait for.
 */
/**
 * The second half of the DAG layout: wait for React Flow to measure the cards, then lay
 * the graph out on the sizes it found.
 *
 * Ensures all DAG nodes have calculated dimensions before the DAG is laid out.
 * The nodes are initially hidden but rendered so that we can measure the actual size
 * which varies depending on how long the node name is.
 */
export function useLayoutWhenMeasured(): void {
  // ReactFlow hook - true when all nodes are rendered/measured (but hidden)
  const initialized = useNodesInitialized();
  // custom hook - true once all nodes have measurements (which Dagre will use)
  const measured = useLineageStore(measuredSelector);
  const isLaidOut = useLineageStore((s) => s.isLaidOut);
  const layoutMeasured = useLineageStore((s) => s.layoutMeasured);

  useEffect(() => {
    if (!initialized || !measured || isLaidOut) return;
    layoutMeasured();
  }, [initialized, measured, isLaidOut, layoutMeasured]);
}
