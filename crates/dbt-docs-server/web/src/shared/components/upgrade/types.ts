/**
 * Shared types for the dbt-docs-v2 upsell surface — consumed by
 * `UpgradeCard`, `UpgradeStatusPanel`, and `UpgradeRailStack`.
 *
 * The kind / user-state matrix and per-cell copy come from the
 * "Gating and UI Review for docs v2" doc (Notion 358bb38e-bda7-…),
 * which is the source of truth for what ships in the v1 prototype.
 */

/** The set of upsell hooks the v1 prototype recognises. */
export type UpgradeHookKind =
  /** Column-level lineage — Core → Proprietary upsell. */
  | 'columnLineage'
  /** Cross-project / Mesh — multi-project upsell. */
  | 'mesh'
  /** dbt State (cost teaser) — always upsold, never gated by capability. */
  | 'dbtState'
  /** Query history on model pages — Proprietary → Platform upsell. */
  | 'queryHistory';

/**
 * User states the upsell pipeline cares about. The `dbt state` axis used to
 * live here as a `-no-state` / `-state-on` suffix but it can't be asserted
 * from the v1 BE signal — see `useCapabilities` for how the `hasDbtState`
 * flag is sourced separately (defaults `false`, i.e. "always upsell").
 */
export type UserState =
  /** OSS dbt user, dbt v2 binary not installed. */
  | 'core'
  /** dbt v2 binary installed but not authenticated. */
  | 'proprietary-anon'
  /** dbt v2 + logged in (implied by `has_column_lineage = true` in the v1 BE). */
  | 'proprietary-logged-in'
  /** Internal "full platform" — docs are embedded via the Catalog app. */
  | 'via-catalog';

/** Inline link rendered at the end of a description body. */
export interface CopyLearnMore {
  label: string;
  href: string;
}

/** Configurable CTA per `(kind, userState)` cell. */
export type CopyCta =
  /** Standard button CTA — labeled, opens an external destination. */
  | { kind: 'button'; label: string; href: string }
  /** Terminal-snippet CTA — copyable code block (e.g. `dbt login`). */
  | { kind: 'snippet'; command: string }
  /** Feature is already on; no CTA shown. Used for CLL when active. */
  | { kind: 'none' };

/** Per-cell copy. `hidden: true` means "do not render this row/card at
 *  all for this user state" — callers can pass the full kind pool and
 *  the registry filters down per user state. */
export type CopySpec =
  | { hidden: true }
  | {
      hidden?: false;
      /** Heading text shown on rail card (expanded + collapsed) and panel row. */
      title: string;
      /** Short tag line shown on the collapsed rail card. */
      subtitle: string;
      /** Optional long-form heading used by the `block` variant — sentence-cased,
       *  h2-sized. Falls back to `title` when absent. */
      headline?: string;
      /** Long-form description shown on the expanded rail card + panel row. */
      description: string;
      /** Optional inline link rendered at the end of the description. */
      learnMore?: CopyLearnMore;
      /** CTA — `'none'` is used when the feature is already active. */
      cta: CopyCta;
      /** When true, render the title with a "is ON" suffix and a green
       *  status dot instead of the muted dot. Used for the CLL row on
       *  dbt v2 logged-in. */
      onState?: boolean;
    };

/**
 * Consumer-facing analytics signal emitted by the upsell components. The
 * shape is intentionally decoupled from any product's telemetry SDK — the
 * component reports *what happened* (a display, a CTA click, a dismiss) and
 * the consumer maps it onto its own event pipeline. Optional everywhere so a
 * consumer that only cares about one action ignores the rest; consumers that
 * omit `onUpsellEvent` entirely see no behavioural change (e.g. dbt-explorer).
 */
export interface UpsellAnalyticsEvent {
  /** `clicked` covers both the CTA button and inline learn-more links; the
   *  consumer disambiguates via `ctaLabel` (button) vs `destination`
   *  (learn-more / referral link). */
  action: 'displayed' | 'clicked' | 'dismissed';
  /** The upsell hook — maps to `upsell_track`. */
  track: UpgradeHookKind;
  /** Rendered form — maps to `prompt_format` (e.g. `rail-card`, `panel-row`, `card`). */
  format?: string;
  /** Consumer-supplied surface — maps to `prompt_location`. */
  location?: string;
  /** CTA label on a button click — maps to `cta_label`. */
  ctaLabel?: string;
  /** Link href on a CTA / learn-more click — maps to a referral `link_destination`. */
  destination?: string;
  /** How the prompt was dismissed — maps to `dismiss_method` (e.g. `close-button`). */
  dismissMethod?: string;
}

export type OnUpsellEvent = (event: UpsellAnalyticsEvent) => void;

/**
 * Transforms an outbound href at the point of navigation. A consumer supplies
 * it to attach referral/attribution metadata (e.g. UTM params) to links leaving
 * for a tracked property. Defaults to identity, so consumers that omit it — and
 * those the transform doesn't recognise — see the href unchanged (e.g.
 * dbt-explorer).
 */
export type DecorateOutboundHref = (href: string) => string;

/** Default {@link DecorateOutboundHref} — returns the href unchanged. Shared so
 *  the upgrade components' opt-in default is defined once. */
export const identityHref: DecorateOutboundHref = (href) => href;

/** Full registry shape — every `(kind, userState)` cell must be present. */
export type UpgradeCopyRegistry = Record<UpgradeHookKind, Record<UserState, CopySpec>>;

/** Ordered list of all hook kinds. Useful for exhaustiveness checks. */
export const ALL_UPGRADE_HOOK_KINDS: UpgradeHookKind[] = [
  'columnLineage',
  'mesh',
  'dbtState',
  'queryHistory',
];

/** Ordered list of all user states. Useful for exhaustiveness checks. */
export const ALL_USER_STATES: UserState[] = [
  'core',
  'proprietary-anon',
  'proprietary-logged-in',
  'via-catalog',
];
