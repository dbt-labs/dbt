import { useEffect } from 'react';

import { useLineageStore } from '../stores/lineageStore';
import { useLineageData } from './useLineageData';

// The data layer (LineageArgs.depth) only supports one symmetric depth today, not
// independent upstream/downstream. "max" also isn't real yet -- there's no bare-plus
// (unlimited) selector support wired up, so an infinite hop count falls back to this
// instead of feeding Infinity into a query. Real per-direction depth needs a
// data-layer change.
const MAX_DEPTH_FALLBACK = 50;

/**
 * Bootstrap action for the lineage store: read the graph out of DuckDB for
 * `rootUniqueId` and push it into `useLineageStore`, laid out and ready to render.
 *
 * Call this once, from the surface that owns the lineage route. Everything below it
 * — the canvas and any sibling view of the same graph — reads nodes, edges and
 * hydration status straight off the store, so no child needs the query, the layout
 * pass, or props threaded down to it.
 *
 * Returns nothing on purpose: `useLineageStatus()` is the way to read the loading,
 * error and unsupported branches, and it works at any depth.
 */
export function useHydrateLineageStore(
  rootUniqueId: string,
  upstreamDepth: number,
  downstreamDepth: number,
): void {
  // Best-effort until the data layer supports independent depths: fetch enough to
  // cover whichever side asked for more, symmetrically.
  const rawDepth = Math.max(upstreamDepth, downstreamDepth);
  const depth = Number.isFinite(rawDepth) ? Math.floor(rawDepth) : MAX_DEPTH_FALLBACK;
  const { data, error, isSupported, graphNodes, graphEdges } = useLineageData(
    rootUniqueId,
    depth,
  );
  const startHydration = useLineageStore((s) => s.startHydration);
  const hydrate = useLineageStore((s) => s.hydrate);
  const failHydration = useLineageStore((s) => s.failHydration);
  const markUnsupported = useLineageStore((s) => s.markUnsupported);
  const reset = useLineageStore((s) => s.reset);

  useEffect(() => {
    if (!rootUniqueId) {
      reset();
      return;
    }
    startHydration(rootUniqueId, upstreamDepth, downstreamDepth);
  }, [rootUniqueId, upstreamDepth, downstreamDepth, startHydration, reset]);

  useEffect(() => {
    if (!rootUniqueId) return;
    if (error) {
      failHydration(error);
      return;
    }
    if (!isSupported) {
      markUnsupported();
      return;
    }
    // Lineage resolves asynchronously; until it lands the store stays in `loading`.
    if (!data) return;
    hydrate({
      rootUniqueId,
      upstreamDepth,
      downstreamDepth,
      nodes: graphNodes,
      edges: graphEdges,
    });
  }, [
    rootUniqueId,
    upstreamDepth,
    downstreamDepth,
    data,
    error,
    isSupported,
    graphNodes,
    graphEdges,
    hydrate,
    failHydration,
    markUnsupported,
  ]);
}
