import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  type SiteBootstrap,
  SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
} from '../lib/siteBootstrap';
import { createQueryWrapper } from '../test/renderWithProviders';
import type { Identity } from '../types';
import { useIdentity } from './useIdentity';

const CONSENT_DENIED: Identity = { is_logged_in: false, analytics_enabled: false };

function siteBootstrap(overrides: Partial<SiteBootstrap> = {}): SiteBootstrap {
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
    ...overrides,
  };
}

describe('useIdentity', () => {
  afterEach(() => {
    delete window.__DBT_DOCS__;
    vi.restoreAllMocks();
  });

  it('reads consent and login state from the site bootstrap', async () => {
    window.__DBT_DOCS__ = siteBootstrap();

    const { result } = renderHook(() => useIdentity(), {
      wrapper: createQueryWrapper(),
    });

    await waitFor(() =>
      expect(result.current.data).toEqual({
        is_logged_in: true,
        analytics_enabled: true,
      }),
    );
  });

  it('passes through a denied consent flag', async () => {
    window.__DBT_DOCS__ = siteBootstrap({
      is_logged_in: true,
      telemetry: { ...siteBootstrap().telemetry, enabled: false },
    });

    const { result } = renderHook(() => useIdentity(), {
      wrapper: createQueryWrapper(),
    });

    await waitFor(() =>
      expect(result.current.data).toEqual({
        is_logged_in: true,
        analytics_enabled: false,
      }),
    );
  });

  it('fails closed when there is no bootstrap to read', async () => {
    const { result } = renderHook(() => useIdentity(), {
      wrapper: createQueryWrapper(),
    });

    await waitFor(() => expect(result.current.data).toEqual(CONSENT_DENIED));
  });

  it('fails closed when the bootstrap schema is unrecognized', async () => {
    // The payload claims consent, but its shape is from a version this build does not
    // know — so `readSiteBootstrap` rejects it and consent must not be inferred.
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    window.__DBT_DOCS__ = siteBootstrap({ schema_version: 99 });

    const { result } = renderHook(() => useIdentity(), {
      wrapper: createQueryWrapper(),
    });

    await waitFor(() => expect(result.current.data).toEqual(CONSENT_DENIED));
    expect(warn).toHaveBeenCalled();
  });
});
