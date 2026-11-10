import { isTelemetryInitialized } from './telemetry';

/**
 * Referral attribution for outbound links (META-7738).
 *
 * docs-v2 does not export to Snowplow and `document.referrer` is not a reliable
 * attribution key (localhost port noise, HTTPS→HTTP stripping, privacy
 * settings). Instead we tag outbound links to Snowplow-tracked dbt Labs
 * properties with UTM params so the *landing* surface's own tracker attributes
 * the visit back to docs-v2 (`marketing_source = 'dbt-docs-v2'` in
 * `int_snowplow_page_views`). No UI-side click event is emitted.
 */

/** UTM params appended to a decorated outbound href. */
const REFERRAL_UTM: Readonly<Record<string, string>> = {
  utm_source: 'dbt-docs-v2',
  utm_medium: 'referral',
};

/**
 * dbt Labs properties that run Snowplow and can attribute an incoming referral.
 * Exact host match — a link off this list is left untouched. Protocol is
 * intentionally not constrained: an allowlisted `http://` link is decorated and
 * keeps its scheme (these are `https` in practice, but we don't rewrite it).
 */
const ALLOWED_HOSTS: ReadonlySet<string> = new Set([
  'getdbt.com',
  'www.getdbt.com',
  'docs.getdbt.com',
  'state.dbt.com',
  'cloud.getdbt.com',
]);

/**
 * Append referral UTM params to an outbound href bound for a Snowplow-tracked
 * dbt Labs property. Consent-gated: returns the href untouched until telemetry
 * is initialised (i.e. `analytics_enabled === true`), so no referral metadata
 * leaves the page without consent.
 *
 * Off-allowlist hosts and non-absolute / unparseable hrefs are returned as-is.
 * The destination's own query params and hash are preserved — these are curated
 * static links where the query (e.g. `?version=` doc routing) and `#anchor` are
 * meaningful navigation, not PII. UTM params are merged into any existing query.
 */
export function decorateOutboundHref(href: string): string {
  if (!isTelemetryInitialized()) return href;

  let url: URL;
  try {
    url = new URL(href);
  } catch {
    // Relative or otherwise unparseable — not an outbound link we decorate.
    return href;
  }

  if (!ALLOWED_HOSTS.has(url.hostname)) return href;

  // Merge UTM into the existing query, preserving the destination's own params
  // (e.g. `?version=`) and hash anchor.
  for (const [key, value] of Object.entries(REFERRAL_UTM)) {
    url.searchParams.set(key, value);
  }
  return url.toString();
}
