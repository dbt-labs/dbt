/**
 * Stand-in `uniqueId` for the whole-project lineage view, which has no single root
 * node. It never resolves to a real resource: the data layer reads it as "walk
 * downstream from every entry point", and the layout treats a root that is not in
 * the graph as "no root" (see `alignRanksTowardRoot`).
 */
export const PROJECT_LINEAGE_ROOT = 'project.__all__';

export function isProjectLineageRoot(uniqueId: string | null | undefined): boolean {
  return uniqueId === PROJECT_LINEAGE_ROOT;
}

/** A hop count at or past this is "max" in the project view -- the hop menu's
 *  "max" is `Infinity`, which `useHydrateLineageStore` turns into 50. */
export const PROJECT_LINEAGE_UNBOUNDED_FROM = 50;

/** Walk limit standing in for "no limit" in SQL; far deeper than any real DAG. */
export const PROJECT_LINEAGE_MAX_WALK = 1000;
