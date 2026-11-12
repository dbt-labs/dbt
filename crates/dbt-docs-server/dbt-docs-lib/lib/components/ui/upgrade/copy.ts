/**
 * Per-cell copy + CTA registry for the `(kind, userState)` upsell matrix.
 *
 * Lifted verbatim from the Notion "Gating and UI Review for docs v2" doc
 * (358bb38e-bda7-…). That doc is the source of truth for what ships in
 * the v1 prototype. Components do no string interpolation — every visible
 * row resolves to a single `CopySpec` here.
 *
 * Cells marked `{ hidden: true }` are not rendered for that user state.
 * Components are expected to skip these entries.
 *
 * Per-surface kind pools live in the helper functions at the bottom of
 * this file (see `getRailKindsForUserState`, `getHomePanelKindsForUserState`).
 * They exist because each surface upsells a different slice: the side panel
 * shows only Mesh, while the home panel aggregates dbt State, CLL, Mesh, and
 * query history.
 */

import { CopySpec, UpgradeCopyRegistry, UpgradeHookKind, UserState } from './types';

const CONTACT_SALES_URL = 'https://www.getdbt.com/contact';
const DBT_STATE_URL = 'https://state.dbt.com/';
// The anchor still says `fusion` because it is docs.getdbt.com's, not ours.
const DBT_V2_DOWNLOAD_URL =
  'https://docs.getdbt.com/docs/local/install-dbt?version=2.0#dbt-fusion-engine-recommended';

/** Standard "Upgrade — contact sales" CTA used by Mesh, Query history, etc. */
const UPGRADE_CTA = {
  kind: 'button',
  label: 'Upgrade',
  href: CONTACT_SALES_URL,
} as const;

/** `dbt login` snippet — used for `proprietary-anon` (dbt v2 binary present
 *  but not authenticated). Core users get the download button instead, since
 *  `dbt login` doesn't exist on Core. */
const DBT_LOGIN_SNIPPET = {
  kind: 'snippet',
  command: 'dbt login',
} as const;

/** Core-only CTA — sends the user to the dbt v2 install docs. Per the Notion
 *  gating doc: "Download dbt v2". */
const DOWNLOAD_DBT_V2_CTA = {
  kind: 'button',
  label: 'Download dbt v2',
  href: DBT_V2_DOWNLOAD_URL,
} as const;

const CLL_BASE = {
  title: 'Column-level lineage',
  subtitle: 'Go deeper with CLL',
  description:
    'See exactly where each column comes from and what transforms it along the way. Column-level lineage is available in dbt v2 — free when you download.',
  learnMore: {
    label: 'Learn more about generating CLL.',
    href: 'https://docs.getdbt.com/docs/build/view-documentation?version=2.0#dbt-docs-v2',
  },
  cta: DBT_LOGIN_SNIPPET,
};

const MESH_BASE = {
  title: 'Collaborate with teams',
  subtitle: 'Unlock across projects',
  description:
    'See every project in your org, not just this one. dbt platform brings teams together to browse, reference, and collaborate across models, metrics, and lineage across every project.',
  cta: UPGRADE_CTA,
};

const DBT_STATE_BASE = {
  title: 'dbt State',
  subtitle: 'Stop rebuilding models',
  /** Block-variant headline (home persistent card). Other variants fall
   *  back to `title`. */
  headline: "Stop rebuilding models that haven't changed.",
  description:
    'dbt State runs only the models whose data or code has changed since the last build reducing costs and shrinking pipeline build times.',
  /** Inline link rendered at the end of the description on the block
   *  variant; the rail/status surfaces use the `cta` button instead. */
  learnMore: {
    label: 'Reduce my build costs.',
    href: DBT_STATE_URL,
  },
  cta: {
    kind: 'button',
    label: 'dbt State',
    href: DBT_STATE_URL,
  } as const,
};

const QUERY_HISTORY_BASE = {
  title: 'Query history',
  subtitle: 'See model usage',
  description:
    'Query history shows how often each model is queried, so you can spot the high-impact ones. Part of dbt platform.',
  cta: UPGRADE_CTA,
};

const HIDDEN: CopySpec = { hidden: true };

/**
 * Full `(kind, userState)` registry. Every cell is present so an
 * exhaustiveness test can fail at unit-test time if a future kind /
 * state pair is missing.
 */
export const UPGRADE_COPY: UpgradeCopyRegistry = {
  columnLineage: {
    // Core has no dbt v2 binary, so `dbt login` is a dead-end command. Send
    // them to the install docs instead — per the Notion gating doc:
    // "Download dbt v2".
    core: { ...CLL_BASE, cta: DOWNLOAD_DBT_V2_CTA },
    'proprietary-anon': { ...CLL_BASE },
    // `proprietary-logged-in` is asserted from `has_column_lineage = true`
    // in the v1 BE — i.e. CLL is already active for this user. Render the
    // on-state (green dot, no CTA).
    'proprietary-logged-in': {
      ...CLL_BASE,
      cta: { kind: 'none' },
      onState: true,
    },
    'via-catalog': HIDDEN,
  },
  mesh: {
    core: { ...MESH_BASE },
    'proprietary-anon': { ...MESH_BASE },
    'proprietary-logged-in': { ...MESH_BASE },
    'via-catalog': HIDDEN,
  },
  // dbt State is "always upsell, assume not enabled" in v1. The BE doesn't
  // expose state-detection yet; once it does, the consumer can pass
  // `hasDbtState` into the resolver to flip these to HIDDEN. See
  // `useCapabilities` in the docs-v2 app.
  dbtState: {
    core: { ...DBT_STATE_BASE },
    'proprietary-anon': { ...DBT_STATE_BASE },
    'proprietary-logged-in': { ...DBT_STATE_BASE },
    // No state-detection yet on the BE — until that ships, dbt State is
    // surfaced for every user state including via-catalog (per the alpha
    // gating doc: "dbt State card persists" on via-catalog).
    'via-catalog': { ...DBT_STATE_BASE },
  },
  queryHistory: {
    core: { ...QUERY_HISTORY_BASE },
    'proprietary-anon': { ...QUERY_HISTORY_BASE },
    'proprietary-logged-in': { ...QUERY_HISTORY_BASE },
    'via-catalog': HIDDEN,
  },
};

/** Returns the resolved copy for a `(kind, userState)` pair. */
export function getUpgradeCopy(kind: UpgradeHookKind, userState: UserState): CopySpec {
  return UPGRADE_COPY[kind][userState];
}

/** Convenience predicate — narrows `CopySpec` to the visible variant. */
export function isUpgradeCopyVisible(
  spec: CopySpec,
): spec is Extract<CopySpec, { hidden?: false }> {
  return !('hidden' in spec && spec.hidden);
}

/**
 * Side-panel rail pool. The rail only ever upsells Mesh (cross-project
 * collaboration). Mesh's copy resolves to `{hidden: true}` for
 * `via-catalog`, so `UpgradeRailStack` renders nothing for that state.
 */
export function getRailKindsForUserState(_us: UserState): UpgradeHookKind[] {
  return ['mesh'];
}

/**
 * Home-page "Get more from dbt" panel pool — the aggregated catch-all
 * surface, shown in design order. Copy-gating drops the kinds that are
 * hidden for the current user state (e.g. `via-catalog` keeps only dbt
 * State).
 */
export function getHomePanelKindsForUserState(_us: UserState): UpgradeHookKind[] {
  return ['dbtState', 'columnLineage', 'mesh', 'queryHistory'];
}
