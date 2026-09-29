import { HelmetProvider } from 'react-helmet-async';
import { MemoryRouter } from 'react-router-dom';
import { QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';

import App from './App';
import { BootstrapProvider } from './lib/bootstrapContext';
import { LinkPrefixProvider, MetadataDataProvider } from './shared';
import type { BootstrapData } from './shared/data-sources/duckdb/bootstrap';
import { DETAIL_REGISTRY } from './shared/data-sources/duckdb/details';
import {
  fromCapabilities,
  fromDistribution,
} from './shared/data-sources/mappers/fromWire';
import type { MetadataDataSource } from './shared/data-sources/MetadataDataSource';
import { createFakeDataSource } from './shared/testing/createFakeDataSource';
import { makeTestQueryClient } from './test/renderWithProviders';

/**
 * The shell's data source, built from the same fixtures the fetch stub described.
 *
 * This test used to stub `fetch` and let the REST source read it. The fixtures are
 * still the right description of the shell's data, so they feed a fake source instead.
 */
function shellSource(overrides: Partial<MetadataDataSource> = {}): MetadataDataSource {
  return createFakeDataSource(
    {
      fetchProject: async () => ({
        name: 'demo_project',
        description: 'A demo project description.',
        dbtVersion: null,
        adapterType: null,
        git: null,
      }),
      fetchCapabilities: async () => fromCapabilities({ has_column_lineage: false }),
      fetchDistribution: async () =>
        fromDistribution({ name: 'oss', version: '0.0.0', is_logged_in: false }),
      fetchFiles: async () => [],
      fetchAssetCounts: async () => ({ model: 1 }),
      ...overrides,
    } as never,
    { full: true },
  );
}

const testDataSource = shellSource();

/**
 * The first-paint read `main.tsx` starts before mounting.
 *
 * The shell renders nothing until this settles — it is where the node list, and so the
 * sidebar, file tree, and resource-type resolver, come from.
 */
function bootstrapRead(nodes: BootstrapData['nodes'] = []): Promise<BootstrapData> {
  return Promise.resolve({
    nodes,
    project: {
      name: 'demo_project',
      description: 'A demo project description.',
      dbtVersion: null,
      adapterType: null,
    },
    generation: null,
  } as unknown as BootstrapData);
}

function renderApp(
  source: MetadataDataSource = testDataSource,
  bootstrap: Promise<BootstrapData> = bootstrapRead(),
) {
  return render(
    <QueryClientProvider client={makeTestQueryClient()}>
      <HelmetProvider>
        <MemoryRouter initialEntries={['/']}>
          <BootstrapProvider value={bootstrap}>
            <MetadataDataProvider source={source}>
              <App />
            </MetadataDataProvider>
          </BootstrapProvider>
        </MemoryRouter>
      </HelmetProvider>
    </QueryClientProvider>,
  );
}

describe('<App />', () => {
  it('renders topbar and loading state', () => {
    // A read that never settles: the shell must show its chrome and a loading state
    // rather than an empty project.
    renderApp(testDataSource, new Promise<BootstrapData>(() => {}));

    expect(screen.getByRole('searchbox')).toBeVisible();
    expect(screen.getByText('Loading…')).toBeVisible();
  });

  describe('overview page', () => {
    it('renders the project-authored overview at /', async () => {
      renderApp(
        shellSource({
          fetchOverview: async () => ({
            uniqueId: 'doc.demo.__overview__',
            packageName: 'demo',
            blockContents: '# Authored overview\n\nFrom the project.',
          }),
        } as never),
      );
      await waitFor(() => {
        expect(screen.getByText('Authored overview', { selector: 'h1' })).toBeVisible();
      });
      expect(screen.getByText('From the project.')).toBeVisible();
    });

    it('falls back to the dashboard default when no package defines one', async () => {
      // `fetchOverview` resolving null is the real "not defined" answer, not an
      // error — the landing page must still render something.
      renderApp(shellSource({ fetchOverview: async () => null } as never));
      await waitFor(() => {
        expect(screen.getByText('Welcome!', { selector: 'h1' })).toBeVisible();
      });
    });

    it('falls back to the dashboard default when the overview read fails', async () => {
      // An unreadable dbt.docs_blocks must not blank the landing page.
      renderApp(
        shellSource({
          fetchOverview: async () => {
            throw new Error('dbt.docs_blocks is missing');
          },
        } as never),
      );
      await waitFor(() => {
        expect(screen.getByText('Welcome!', { selector: 'h1' })).toBeVisible();
      });
    });
  });

  describe('detail panel', () => {
    it('renders model detail when navigating to a detail route', async () => {
      // The node index carries the resource type, which is what routes the detail
      // panel to the model view; the detail body itself comes from the source.
      const detailSource = shellSource({
        fetchAsset: async () =>
          DETAIL_REGISTRY.model!.map({
            unique_id: 'model.demo.dim_customers',
            name: 'dim_customers',
            resource_type: 'model',
            package_name: 'demo',
            description: 'Customer dimension table',
            tags: [],
            fqn: ['demo', 'dim_customers'],
            columns: [],
            depends_on: [],
            referenced_by: [],
          }),
      } as never);

      render(
        <QueryClientProvider client={makeTestQueryClient()}>
          <HelmetProvider>
            <MemoryRouter initialEntries={['/details/model.demo.dim_customers']}>
              <LinkPrefixProvider prefix="/">
                <BootstrapProvider
                  value={bootstrapRead([
                    {
                      unique_id: 'model.demo.dim_customers',
                      name: 'dim_customers',
                      resource_type: 'model',
                      package_name: 'demo',
                    },
                  ])}
                >
                  <MetadataDataProvider source={detailSource}>
                    <App />
                  </MetadataDataProvider>
                </BootstrapProvider>
              </LinkPrefixProvider>
            </MemoryRouter>
          </HelmetProvider>
        </QueryClientProvider>,
      );

      await waitFor(() => {
        expect(screen.getByRole('heading', { name: 'dim_customers' })).toBeVisible();
      });
    });
  });
});
