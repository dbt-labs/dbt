import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  dataBaseUrl,
  hasSiteBootstrap,
  readSiteBootstrap,
  type SiteBootstrap,
  SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
} from './siteBootstrap';

function bootstrap(overrides: Partial<SiteBootstrap> = {}): SiteBootstrap {
  return {
    schema_version: SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
    generated_at: '2026-08-08T18:00:00Z',
    dbt_version: '2.0.0',
    distribution: 'dbt',
    is_logged_in: true,
    duckdb_cdn_base: 'https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.32.0',
    data_dir: 'index/',
    telemetry: {
      enabled: false,
      dbt_cloud_account_identifier: '',
      dbt_cloud_project_id: '',
      dbt_cloud_environment_id: '',
    },
    ...overrides,
  };
}

afterEach(() => {
  delete window.__DBT_DOCS__;
  vi.restoreAllMocks();
});

describe('readSiteBootstrap', () => {
  it('returns null when absent', () => {
    expect(readSiteBootstrap()).toBeNull();
    expect(hasSiteBootstrap()).toBe(false);
  });

  it('returns the payload on a generated site', () => {
    window.__DBT_DOCS__ = bootstrap();
    expect(readSiteBootstrap()?.distribution).toBe('dbt');
    expect(hasSiteBootstrap()).toBe(true);
  });

  it('ignores an unrecognized schema version rather than reading moved fields', () => {
    // A stale index.html served next to fresh assets should read as absent rather
    // than misread a payload whose shape changed.
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    window.__DBT_DOCS__ = bootstrap({ schema_version: 99 });

    expect(readSiteBootstrap()).toBeNull();
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('schema_version 99'));
  });

  it('carries no column-lineage flag', () => {
    // Availability comes from whether the artifact loaded; a second copy here
    // could disagree with the data beside it.
    window.__DBT_DOCS__ = bootstrap();
    expect(readSiteBootstrap()).not.toHaveProperty('has_column_lineage');
  });
});

describe('dataBaseUrl', () => {
  it('resolves against the document base so subpath hosting works', () => {
    // A GitLab Pages project site lives under /<group>/<project>/.
    window.__DBT_DOCS__ = bootstrap();
    vi.spyOn(document, 'baseURI', 'get').mockReturnValue(
      'https://acme.gitlab.io/data/dbt-docs/',
    );
    expect(dataBaseUrl()).toBe('https://acme.gitlab.io/data/dbt-docs/index/');
  });

  it('resolves correctly from an explicit index.html', () => {
    window.__DBT_DOCS__ = bootstrap();
    vi.spyOn(document, 'baseURI', 'get').mockReturnValue(
      'https://acme.gitlab.io/data/dbt-docs/index.html',
    );
    expect(dataBaseUrl()).toBe('https://acme.gitlab.io/data/dbt-docs/index/');
  });

  it('is unaffected by the hash, which is what keeps deep links working', () => {
    // Hash routing is what makes a relative data path sound: the document's own
    // path never changes as the user navigates.
    window.__DBT_DOCS__ = bootstrap();
    vi.spyOn(document, 'baseURI', 'get').mockReturnValue(
      'https://acme.gitlab.io/docs/#/details/model.a.b/',
    );
    expect(dataBaseUrl()).toBe('https://acme.gitlab.io/docs/index/');
  });

  it('honors the directory the exporter chose', () => {
    // The site is written to the dbt target directory, where `data/` already
    // belongs to stored test failures — so the name has to come from the build
    // rather than be hardcoded here.
    window.__DBT_DOCS__ = bootstrap({ data_dir: 'somewhere_else/' });
    vi.spyOn(document, 'baseURI', 'get').mockReturnValue(
      'https://acme.gitlab.io/docs/',
    );
    expect(dataBaseUrl()).toBe('https://acme.gitlab.io/docs/somewhere_else/');
  });

  it('falls back to the default when there is no readable bootstrap', () => {
    vi.spyOn(document, 'baseURI', 'get').mockReturnValue(
      'https://acme.gitlab.io/docs/',
    );
    expect(dataBaseUrl()).toBe('https://acme.gitlab.io/docs/index/');
  });
});
