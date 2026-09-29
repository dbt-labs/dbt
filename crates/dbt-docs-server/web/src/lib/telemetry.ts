import { useSyncExternalStore } from 'react';

import {
  type AnalyticsEvent,
  type DocsSiteOpenedEvent,
  type Identity,
  type LineageViewedEvent,
  type ReferralLinkClickedEvent,
  type ResourceViewedEvent,
  type SearchPerformedEvent,
  type UpsellPromptClickedEvent,
  type UpsellPromptDismissedEvent,
  type UpsellPromptDisplayedEvent,
} from '../types';
import { readSiteBootstrap } from './siteBootstrap';
import { flushVortex, logEvent } from './vortexSink';

/** Base properties attached to every telemetry event once analytics is
 *  initialised. `isLoggedIn` is the V1 engagement-slice dimension (events are
 *  split by authenticated vs anonymous users). */
export interface TelemetryContext {
  isLoggedIn: boolean;
}

/** Distributive `Omit` — keeps the discriminated union intact instead of
 *  collapsing it to its common keys the way a plain `Omit<Union, K>` would. */
type DistributiveOmit<T, K extends keyof T> = T extends unknown ? Omit<T, K> : never;

/** An event as supplied by a call site: the wire {@link AnalyticsEvent} minus
 *  the fields {@link track} injects (`is_logged_in` + anonymised `context`). */
export type TrackableEvent = DistributiveOmit<
  AnalyticsEvent,
  'is_logged_in' | 'context'
>;

/** Debounce before a queued batch is handed to the producer, so a burst of
 *  navigation events coalesces into one pass. */
const FLUSH_DELAY_MS = 500;

let initialized = false;
let context: TelemetryContext | null = null;
let sessionId: string | null = null;
let queue: AnalyticsEvent[] = [];
let flushTimer: ReturnType<typeof setTimeout> | null = null;
let exitListener: (() => void) | null = null;

/** Subscribers notified when the `initialized` flag flips, so React consumers
 *  can re-render once consent resolves (init happens in a post-render effect). */
const initListeners = new Set<() => void>();

function notifyInitialized(): void {
  initListeners.forEach((listener) => listener());
}

/**
 * Consent-gated telemetry bootstrap. Call once, on app load, with the {@link Identity}
 * resolved from the site bootstrap.
 *
 * If `analytics_enabled` is not `true` this is a no-op: no session is started,
 * no events are queued, and no network calls to any analytics endpoint are
 * made. Consent fails closed — the caller must treat an identity-check failure
 * as `analytics_enabled: false` (see {@link useIdentity}) so a failed check
 * never enables telemetry.
 *
 * Idempotent: a second call is ignored (React StrictMode double-invokes effects
 * in dev, and the identity query only resolves once per session anyway).
 */
export function initTelemetry(identity: Identity): void {
  if (initialized) return;
  if (!identity.analytics_enabled) return;

  initialized = true;
  context = { isLoggedIn: identity.is_logged_in };
  sessionId = crypto.randomUUID();

  // Flush on page exit so a batch queued in the last ~500ms isn't lost. Loss
  // on a hard crash is acceptable (ADR-10, fire-and-forget); this just narrows
  // the window for the common navigate-away case.
  exitListener = () => {
    if (document.visibilityState === 'hidden') flushOnExit();
  };
  window.addEventListener('pagehide', flushOnExit);
  document.addEventListener('visibilitychange', exitListener);

  notifyInitialized();
}

/** Whether telemetry has been initialised (i.e. consent was granted). */
export function isTelemetryInitialized(): boolean {
  return initialized;
}

/**
 * React-observable view of {@link isTelemetryInitialized}. Consent is resolved
 * in a post-render effect and `initTelemetry` mutates a module singleton, so a
 * component that decorates an href at render time (e.g. an outbound `<a>`) would
 * otherwise capture the pre-consent value with no re-render to reapply it.
 * Subscribing here forces one re-render when consent lands.
 */
export function useTelemetryInitialized(): boolean {
  return useSyncExternalStore((listener) => {
    initListeners.add(listener);
    return () => initListeners.delete(listener);
  }, isTelemetryInitialized);
}

/** Base event properties, or `null` if telemetry was never initialised. */
export function getTelemetryContext(): TelemetryContext | null {
  return context;
}

/**
 * Enqueue an analytics event. No-op until {@link initTelemetry} has run under
 * consent, so no event is ever emitted without consent. Injects the
 * `is_logged_in` dimension and an anonymised `context` (a fresh per-event
 * `event_id` + the per-session `session_id`), then schedules a batched flush.
 */
export function track(event: TrackableEvent): void {
  if (!initialized || !context) return;

  queue.push({
    ...event,
    is_logged_in: context.isLoggedIn,
    context: {
      event_id: crypto.randomUUID(),
      session_id: sessionId ?? undefined,
    },
  } as AnalyticsEvent);

  scheduleFlush();
}

function scheduleFlush(): void {
  if (flushTimer != null) return;
  flushTimer = setTimeout(() => {
    flushTimer = null;
    void flush();
  }, FLUSH_DELAY_MS);
}

/**
 * Send the queued batch and clear it.
 *
 * Straight to the collector: there is no server in this architecture to relay through
 * (ADR-10). Fire-and-forget — event loss is acceptable, and analytics must never be
 * why a docs page misbehaves.
 */
async function flush(): Promise<void> {
  if (queue.length === 0) return;
  const events = queue;
  queue = [];
  const site = readSiteBootstrap();
  if (!site) return;
  try {
    await Promise.all(events.map((event) => logEvent(event, site)));
  } catch {
    // Intentionally ignored — analytics is best-effort.
  }
}

/**
 * Exit-time flush, which cannot wait for a promise.
 *
 * The producer sends each event as it is logged (batching is off), so by here there is
 * usually nothing queued — anything left gets one unawaited attempt plus a producer
 * flush, which is the best a closing tab allows.
 */
function flushOnExit(): void {
  if (flushTimer != null) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
  if (queue.length === 0) return;
  const events = queue;
  queue = [];

  const site = readSiteBootstrap();
  if (!site) return;
  for (const event of events) void logEvent(event, site);
  void flushVortex();
}

/* ---------- typed call-site helpers ---------- */

export function trackDocsSiteOpened(
  fields: Omit<DocsSiteOpenedEvent, 'event_type' | 'is_logged_in' | 'context'>,
): void {
  track({ event_type: 'docs_site_opened', ...fields });
}

export function trackResourceViewed(
  fields: Omit<ResourceViewedEvent, 'event_type' | 'is_logged_in' | 'context'>,
): void {
  track({ event_type: 'resource_viewed', ...fields });
}

export function trackSearchPerformed(
  fields: Omit<SearchPerformedEvent, 'event_type' | 'is_logged_in' | 'context'>,
): void {
  track({ event_type: 'search_performed', ...fields });
}

export function trackLineageViewed(
  fields: Omit<LineageViewedEvent, 'event_type' | 'is_logged_in' | 'context'>,
): void {
  track({ event_type: 'lineage_viewed', ...fields });
}

/** Emit one of the three upsell-prompt events. The caller supplies the tagged
 *  event so the discriminated fields stay type-checked per `event_type`. */
export function trackUpsellEvent(
  event: DistributiveOmit<
    UpsellPromptDisplayedEvent | UpsellPromptClickedEvent | UpsellPromptDismissedEvent,
    'is_logged_in' | 'context'
  >,
): void {
  track(event);
}

export function trackReferralLinkClicked(
  fields: Omit<ReferralLinkClickedEvent, 'event_type' | 'is_logged_in' | 'context'>,
): void {
  track({ event_type: 'referral_link_clicked', ...fields });
}

/** Test-only: reset the module singleton between cases. */
export function resetTelemetryForTests(): void {
  if (flushTimer != null) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
  if (exitListener) {
    window.removeEventListener('pagehide', flushOnExit);
    document.removeEventListener('visibilitychange', exitListener);
  }
  initialized = false;
  context = null;
  sessionId = null;
  queue = [];
  exitListener = null;
  notifyInitialized();
}
