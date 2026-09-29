import {
  CircleCheck,
  CircleMinus,
  ClipboardCheck,
  FileCheck,
  Infinity as InfinityIcon,
  Layers,
  type LucideIcon,
  RotateCw,
  Table as TableIcon,
} from 'lucide-react';

import { inferModelingLayer } from '../../lib/resourceType';

export type LensBadge = { label: string; icon: LucideIcon; color: string };

// A fixed, small "status color" palette for these badges -- distinct from
// the per-resource-type --fgViz*/--bgDag* tokens (see dagResourceColors.ts),
// since these badges are about materialization/layer/status, not resource
// kind. Plain hex rather than a token: nothing themed exists for this set.
const GREEN = '#22c55e';
const PURPLE = '#a855f7';
const TEAL = '#14b8a6';
const BLUE = '#3b82f6';
const ORANGE = '#f97316';
const RED = '#ef4444';
const GRAY = '#9ca3af';

const MATERIALIZATION_BADGES: Record<string, LensBadge> = {
  table: { label: 'Table', icon: TableIcon, color: GREEN },
  view: { label: 'View', icon: InfinityIcon, color: PURPLE },
};

const MODEL_LAYER_BADGES: Record<string, LensBadge> = {
  Staging: { label: 'Stg', icon: Layers, color: BLUE },
  Intermediate: { label: 'Int', icon: Layers, color: PURPLE },
  Marts: { label: 'Mart', icon: Layers, color: GREEN },
};

// Last run and Last test are wired below for whenever their data exists, but
// nothing calls into these two maps yet -- neither status is in the lineage
// payload today (see DagLensesDropdown's own note). Written once now so
// there's nothing left to design later, just a data source to plug in.
const LAST_RUN_BADGES: Record<string, LensBadge> = {
  reused: { label: 'Reused', icon: RotateCw, color: TEAL },
  success: { label: 'Success', icon: CircleCheck, color: GREEN },
  skipped: { label: 'Skipped', icon: CircleMinus, color: GRAY },
};

// Same icon for every status, per Jess: "use clipboard-check, dbt users will
// recognize it as our Test icon" -- colors are what carry the distinction.
const TEST_STATUS_BADGES: Record<string, LensBadge> = {
  pass: { label: 'Pass', icon: ClipboardCheck, color: GREEN },
  error: { label: 'Error', icon: ClipboardCheck, color: ORANGE },
  fail: { label: 'Fail', icon: ClipboardCheck, color: RED },
  warn: { label: 'Warn', icon: ClipboardCheck, color: ORANGE },
  unknown: { label: 'Unknown', icon: ClipboardCheck, color: GRAY },
};

/** The node fields lens badges read -- a subset of DagNodeData/AssetBase,
 *  spelled out here rather than imported so this module doesn't depend on
 *  the component layer. `lastRunStatus`/`lastTestStatus`/`queryCount` aren't
 *  populated by anything today; they're here so a future data-layer change
 *  only has to fill the field, not touch this mapping. */
export interface LensBadgeInput {
  materialized?: string | null;
  originalFilePath?: string | null;
  lastRunStatus?: string | null;
  lastTestStatus?: string | null;
  queryCount?: number | null;
}

/** Resolves the badge (if any) a node should show for the currently active
 *  lens. Returns null for "Default" (no badge -- just the resource-type
 *  badge, unchanged) and whenever the node has no value for that lens (e.g.
 *  every node under Last run/Last test/Query history today). */
export function lensBadgeFor(
  activeLens: string,
  node: LensBadgeInput,
): LensBadge | null {
  switch (activeLens) {
    case 'Materialization': {
      const key = node.materialized?.toLowerCase();
      return key ? (MATERIALIZATION_BADGES[key] ?? null) : null;
    }
    case 'Model layer': {
      const layer = inferModelingLayer(node.originalFilePath);
      return layer ? (MODEL_LAYER_BADGES[layer] ?? null) : null;
    }
    case 'Last run': {
      const key = node.lastRunStatus?.toLowerCase();
      return key ? (LAST_RUN_BADGES[key] ?? null) : null;
    }
    case 'Last test': {
      const key = node.lastTestStatus?.toLowerCase();
      return key ? (TEST_STATUS_BADGES[key] ?? null) : null;
    }
    case 'Query history': {
      return node.queryCount != null
        ? { label: String(node.queryCount), icon: FileCheck, color: GRAY }
        : null;
    }
    default:
      return null;
  }
}
