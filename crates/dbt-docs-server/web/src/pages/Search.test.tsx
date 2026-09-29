import { afterEach, beforeEach, describe, expect, it, type Mock, vi } from 'vitest';

import type { AssetFilters } from '../App';
import {
  type SiteBootstrap,
  SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
} from '../lib/siteBootstrap';
import { initTelemetry, resetTelemetryForTests } from '../lib/telemetry';
import { logEvent } from '../lib/vortexSink';
import type { Project } from '../shared';
import { renderWithProviders } from '../test/renderWithProviders';
import Search from './Search';

// Drive the page off a controllable `useSearch` return and stub the heavy
// results list — this test only cares about the `search_performed` effect.
const searchReturn = {
  data: [] as unknown[],
  total: 0 as number | undefined,
  isPending: false,
  isFetchingNextPage: false,
  hasNextPage: false,
  error: null as unknown,
  errorCode: undefined as string | undefined,
  errorMessage: undefined as string | undefined,
  fetchNextPage: vi.fn(),
};

vi.mock('../lib/vortexSink', () => ({
  logEvent: vi.fn(() => Promise.resolve()),
  flushVortex: vi.fn(() => Promise.resolve()),
}));

/** `flush` needs a bootstrap: there is nowhere else to send an event. */
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

vi.mock('../shared', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../shared')>();
  return {
    ...mod,
    useSearch: () => searchReturn,
    SearchResultsList: () => null,
  };
});

const EMPTY_FILTERS: AssetFilters = {
  resourceType: [],
  modelingLayer: [],
  materialization: [],
  pkg: [],
  tag: [],
};

const PROJECT = { name: 'demo' } as unknown as Project;

function renderSearch(query: string) {
  return renderWithProviders(
    <Search
      project={PROJECT}
      nodes={[]}
      query={query}
      filters={EMPTY_FILTERS}
      onSetFilters={vi.fn()}
      previewId={null}
      onPeek={vi.fn()}
    />,
  );
}

describe('Search — search_performed analytics', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(logEvent).mockClear();
    window.__DBT_DOCS__ = siteBootstrap();
    searchReturn.total = 0;
    searchReturn.isPending = false;
  });

  afterEach(() => {
    resetTelemetryForTests();
    vi.runOnlyPendingTimers();
    vi.useRealTimers();
    delete window.__DBT_DOCS__;
  });

  it('fires search_performed once with the trimmed query + result count', () => {
    initTelemetry({ is_logged_in: true, analytics_enabled: true });
    searchReturn.total = 7;
    renderSearch('  orders  ');
    vi.runAllTimers();

    expect(logEvent).toHaveBeenCalledTimes(1);
    const event = (logEvent as Mock).mock.calls[0][0] as Record<string, unknown>;
    expect(event).toMatchObject({
      event_type: 'search_performed',
      search_query: 'orders',
      result_count: 7,
    });
  });

  it('does not fire for an empty query', () => {
    initTelemetry({ is_logged_in: true, analytics_enabled: true });
    renderSearch('   ');
    vi.runAllTimers();

    expect(logEvent).not.toHaveBeenCalled();
  });

  it('does not fire while the query is still pending', () => {
    initTelemetry({ is_logged_in: true, analytics_enabled: true });
    searchReturn.isPending = true;
    renderSearch('orders');
    vi.runAllTimers();

    expect(logEvent).not.toHaveBeenCalled();
  });

  it('does not fire when telemetry is not initialised', () => {
    searchReturn.total = 3;
    renderSearch('orders');
    vi.runAllTimers();

    expect(logEvent).not.toHaveBeenCalled();
  });
});
