import { afterEach, beforeEach, describe, expect, it, type Mock, vi } from 'vitest';

import type { AnalyticsEvent, Identity } from '../types';
import {
  type SiteBootstrap,
  SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
} from './siteBootstrap';
import {
  getTelemetryContext,
  initTelemetry,
  isTelemetryInitialized,
  resetTelemetryForTests,
  trackDocsSiteOpened,
  trackLineageViewed,
  trackReferralLinkClicked,
  trackResourceViewed,
  trackSearchPerformed,
  trackUpsellEvent,
} from './telemetry';
import { flushVortex, logEvent } from './vortexSink';

// The sink is the seam under test here: this file asserts what telemetry *hands* the
// producer, and `vortexSink.test.ts` covers what the producer does with it.
vi.mock('./vortexSink', () => ({
  logEvent: vi.fn(() => Promise.resolve()),
  flushVortex: vi.fn(() => Promise.resolve()),
}));

/** A generated site's bootstrap. `flush` needs one — there is nowhere else to send. */
function siteBootstrap(): SiteBootstrap {
  return {
    schema_version: SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
    generated_at: '2026-08-08T18:00:00Z',
    dbt_version: '2.0.0',
    distribution: 'dbt',
    is_logged_in: true,
    duckdb_cdn_base: 'https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.32.0',
    data_dir: 'index/',
    telemetry: {
      enabled: true,
      dbt_cloud_account_identifier: '',
      dbt_cloud_project_id: '',
      dbt_cloud_environment_id: '',
    },
  };
}

/** Every event handed to the producer, in order. */
function sentEvents(): AnalyticsEvent[] {
  return (logEvent as Mock).mock.calls.map((call) => call[0] as AnalyticsEvent);
}

const ENABLED: Identity = { is_logged_in: true, analytics_enabled: true };
const DISABLED: Identity = { is_logged_in: true, analytics_enabled: false };

describe('telemetry', () => {
  afterEach(() => resetTelemetryForTests());

  it('does not initialise when analytics is not consented', () => {
    initTelemetry(DISABLED);

    expect(isTelemetryInitialized()).toBe(false);
    expect(getTelemetryContext()).toBeNull();
  });

  it('initialises and captures the logged-in dimension when consented', () => {
    initTelemetry(ENABLED);

    expect(isTelemetryInitialized()).toBe(true);
    expect(getTelemetryContext()).toEqual({ isLoggedIn: true });
  });

  it('carries is_logged_in: false through to the context', () => {
    initTelemetry({ is_logged_in: false, analytics_enabled: true });

    expect(getTelemetryContext()).toEqual({ isLoggedIn: false });
  });

  it('is idempotent — a second call is a no-op', () => {
    initTelemetry(ENABLED);
    // A later call, even with different identity, must not re-initialise or
    // mutate the captured context.
    initTelemetry({ is_logged_in: false, analytics_enabled: true });

    expect(getTelemetryContext()).toEqual({ isLoggedIn: true });
  });

  it('stays uninitialised after a denied call, allowing a later consented init', () => {
    initTelemetry(DISABLED);
    expect(isTelemetryInitialized()).toBe(false);

    initTelemetry(ENABLED);
    expect(isTelemetryInitialized()).toBe(true);
    expect(getTelemetryContext()).toEqual({ isLoggedIn: true });
  });
});

describe('telemetry emission', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(logEvent).mockClear();
    vi.mocked(flushVortex).mockClear();
    window.__DBT_DOCS__ = siteBootstrap();
  });

  afterEach(() => {
    resetTelemetryForTests();
    vi.runOnlyPendingTimers();
    vi.useRealTimers();
    delete window.__DBT_DOCS__;
  });

  it('track is a no-op before init', () => {
    trackSearchPerformed({ search_query: 'x', result_count: 1 });
    vi.runAllTimers();
    expect(logEvent).not.toHaveBeenCalled();
  });

  it('track is a no-op after a consent-denied init', () => {
    initTelemetry(DISABLED);
    trackSearchPerformed({ search_query: 'x', result_count: 1 });
    vi.runAllTimers();
    expect(logEvent).not.toHaveBeenCalled();
  });

  it('injects is_logged_in and an anonymised context, then flushes a batch', () => {
    initTelemetry(ENABLED);
    trackSearchPerformed({ search_query: 'orders', result_count: 3 });

    // Debounced — nothing sent until the flush timer fires.
    expect(logEvent).not.toHaveBeenCalled();
    vi.runAllTimers();

    expect(logEvent).toHaveBeenCalledTimes(1);
    const event = sentEvents()[0] as unknown as Record<string, unknown>;
    expect(event.event_type).toBe('search_performed');
    expect(event.is_logged_in).toBe(true);
    const ctx = event.context as Record<string, unknown>;
    expect(typeof ctx.event_id).toBe('string');
    expect(typeof ctx.session_id).toBe('string');
    // No PII: only the anonymised context keys are present.
    expect(Object.keys(ctx).sort()).toEqual(['event_id', 'session_id']);
  });

  it('batches multiple events into a single flush', () => {
    initTelemetry(ENABLED);
    trackSearchPerformed({ search_query: 'a', result_count: 1 });
    trackResourceViewed({
      resource_type: 'model',
      view_level: 'detail',
      resource_id: 'model.p.a',
    });
    vi.runAllTimers();

    // One flush, both events — the producer takes them individually.
    expect(logEvent).toHaveBeenCalledTimes(2);
  });

  it('gives each event a distinct event_id but a shared session_id', () => {
    initTelemetry(ENABLED);
    trackSearchPerformed({ search_query: 'a', result_count: 1 });
    trackSearchPerformed({ search_query: 'b', result_count: 2 });
    vi.runAllTimers();

    const contexts = sentEvents().map((event) => event.context);
    expect(contexts[0]?.event_id).not.toBe(contexts[1]?.event_id);
    expect(contexts[0]?.session_id).toBe(contexts[1]?.session_id);
  });

  it('drains the queue through the producer on pagehide', () => {
    initTelemetry(ENABLED);
    trackSearchPerformed({ search_query: 'a', result_count: 1 });
    // Not yet flushed via the timer.
    window.dispatchEvent(new Event('pagehide'));

    expect(logEvent).toHaveBeenCalledTimes(1);
    // A closing tab cannot await, so the producer is told to push what it has.
    expect(flushVortex).toHaveBeenCalledTimes(1);

    // The timer flush must not double-send after the exit path drained the queue.
    vi.runAllTimers();
    expect(logEvent).toHaveBeenCalledTimes(1);
  });

  it('resetTelemetryForTests clears the queue and session', () => {
    initTelemetry(ENABLED);
    trackSearchPerformed({ search_query: 'a', result_count: 1 });
    resetTelemetryForTests();

    // The pending event is gone and telemetry is uninitialised again.
    expect(isTelemetryInitialized()).toBe(false);
    vi.runAllTimers();
    expect(logEvent).not.toHaveBeenCalled();
  });

  it.each([
    [
      () => trackDocsSiteOpened({ dbt_version: '1.9', project_resource_count: 42 }),
      {
        event_type: 'docs_site_opened',
        dbt_version: '1.9',
        project_resource_count: 42,
      },
    ],
    [
      () =>
        trackResourceViewed({
          resource_type: 'model',
          view_level: 'detail',
          resource_id: 'model.p.a',
        }),
      {
        event_type: 'resource_viewed',
        resource_type: 'model',
        view_level: 'detail',
        resource_id: 'model.p.a',
      },
    ],
    [
      () => trackSearchPerformed({ search_query: 'q', result_count: 5 }),
      { event_type: 'search_performed', search_query: 'q', result_count: 5 },
    ],
    [
      () =>
        trackLineageViewed({
          lineage_type: 'inline',
          resource_type: 'model',
          resource_id: 'model.p.a',
        }),
      {
        event_type: 'lineage_viewed',
        lineage_type: 'inline',
        resource_type: 'model',
        resource_id: 'model.p.a',
      },
    ],
    [
      () =>
        trackUpsellEvent({
          event_type: 'upsell_prompt_displayed',
          upsell_track: 'columnLineage',
          prompt_format: 'rail-card',
          prompt_location: 'rail',
        }),
      {
        event_type: 'upsell_prompt_displayed',
        upsell_track: 'columnLineage',
        prompt_format: 'rail-card',
        prompt_location: 'rail',
      },
    ],
    [
      () =>
        trackReferralLinkClicked({
          referral_code: '',
          link_destination: 'https://docs.getdbt.com',
        }),
      {
        event_type: 'referral_link_clicked',
        referral_code: '',
        link_destination: 'https://docs.getdbt.com',
      },
    ],
  ])('helper produces the correct event_type + fields', (emit, expected) => {
    initTelemetry(ENABLED);
    emit();
    vi.runAllTimers();

    expect(sentEvents()[0]).toMatchObject(expected);
  });
});
