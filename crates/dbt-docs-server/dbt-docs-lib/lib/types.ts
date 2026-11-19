/**
 * Shared vocabulary that outlived the REST API.
 *
 * This was `api.ts`, a client for `/api/v1/*`. That API is gone — the site queries
 * parquet in the browser — but the wire *shapes* it defined are still the shapes the
 * app is written against: `dbt.nodes_index` rows arrive as {@link NodeSummary}, and
 * telemetry events are still the `docs.proto` types, now encoded client-side and sent
 * straight to the collector (ADR-10).
 */

/** Consent + identity gate for telemetry.
 *
 *  Resolved at export time rather than fetched: only the machine running
 *  `dbt docs generate` can read the project and profile, so consent rides into the
 *  page in `window.__DBT_DOCS__` (see `lib/siteBootstrap`). `analytics_enabled` is the
 *  consent flag — telemetry init is gated on it. `is_logged_in` is the V1
 *  engagement-slice dimension carried on every event (authenticated vs anonymous). */
export interface Identity {
  is_logged_in: boolean;
  analytics_enabled: boolean;
}

/** One row of `dbt.nodes_index` — the nine columns the shell needs, and the reason
 *  that artifact exists: the resource tables carry code blobs no list renders. */
export interface NodeSummary {
  unique_id: string;
  name: string;
  resource_type: string;
  package_name?: string | null;
  materialized?: string | null;
  description?: string | null;
  database_name?: string | null;
  schema_name?: string | null;
  original_file_path?: string | null;
}

/**
 * Anonymised per-event context.
 *
 * Every field is optional and defaults to `""` at encode time. docs v2 has no dbt
 * Cloud account/project/environment identity client-side, so in practice only
 * `event_id` + `session_id` are set; the `dbt_cloud_*` slots exist for wire-contract
 * parity with other emitters.
 */
export interface AnalyticsEventContext {
  event_id?: string;
  session_id?: string;
  feature?: string;
  referrer_url?: string;
  dbt_cloud_account_id?: string;
  dbt_cloud_project_id?: string;
  dbt_cloud_environment_id?: string;
}

/** Common envelope on every analytics event. `is_logged_in` is the one
 *  telemetry dimension carried on every event (authenticated vs anonymous). */
interface AnalyticsEventBase {
  is_logged_in: boolean;
  context?: AnalyticsEventContext;
}

export interface DocsSiteOpenedEvent extends AnalyticsEventBase {
  event_type: 'docs_site_opened';
  dbt_version: string;
  project_resource_count: number;
}

export interface ResourceViewedEvent extends AnalyticsEventBase {
  event_type: 'resource_viewed';
  resource_type: string;
  view_level: string;
  resource_id: string;
}

export interface LineageViewedEvent extends AnalyticsEventBase {
  event_type: 'lineage_viewed';
  lineage_type: string;
  resource_type: string;
  resource_id: string;
}

export interface SearchPerformedEvent extends AnalyticsEventBase {
  event_type: 'search_performed';
  search_query: string;
  result_count: number;
}

export interface UpsellPromptDisplayedEvent extends AnalyticsEventBase {
  event_type: 'upsell_prompt_displayed';
  upsell_track: string;
  prompt_format: string;
  prompt_location: string;
}

export interface UpsellPromptClickedEvent extends AnalyticsEventBase {
  event_type: 'upsell_prompt_clicked';
  upsell_track: string;
  cta_label: string;
  referral_code: string;
}

export interface UpsellPromptDismissedEvent extends AnalyticsEventBase {
  event_type: 'upsell_prompt_dismissed';
  upsell_track: string;
  dismiss_method: string;
}

export interface ReferralLinkClickedEvent extends AnalyticsEventBase {
  event_type: 'referral_link_clicked';
  referral_code: string;
  link_destination: string;
}

/** Discriminated union of every event the docs-v2 UI emits, keyed by the
 *  snake_case `event_type` tag `vortexSink` maps to a `docs.proto` wire type. */
export type AnalyticsEvent =
  | DocsSiteOpenedEvent
  | ResourceViewedEvent
  | LineageViewedEvent
  | SearchPerformedEvent
  | UpsellPromptDisplayedEvent
  | UpsellPromptClickedEvent
  | UpsellPromptDismissedEvent
  | ReferralLinkClickedEvent;

/** Resource types the search surface offers as type filters.
 *  `analysis` is included for future use; visibility is gated separately via
 *  `FEATURE_FLAGS.hasAnalysis`. */
export const SEARCHABLE_RESOURCE_TYPES: ReadonlySet<string> = new Set([
  'model',
  'source',
  'seed',
  'snapshot',
  'test',
  'analysis',
  'unit_test',
  'exposure',
  'metric',
  'semantic_model',
  'saved_query',
  'macro',
  'group',
]);

/** Structured search-rejection codes. The four documented codes —
 *  `invalid_type`, `invalid_modeling_layer`, `invalid_cursor`, `query_too_long` —
 *  are stable; new codes may be added. */
export type SearchErrorCode =
  'invalid_type' | 'invalid_modeling_layer' | 'invalid_cursor' | 'query_too_long';
