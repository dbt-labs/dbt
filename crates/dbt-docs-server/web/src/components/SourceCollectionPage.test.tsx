import { Route, Routes } from 'react-router-dom';
import { screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { DETAIL_REGISTRY } from '../shared/data-sources/duckdb/details';
import { createFakeDataSource } from '../shared/testing/createFakeDataSource';
import { renderWithProviders } from '../test/renderWithProviders';
import type { NodeSummary } from '../types';
import { SourceCollectionPage } from './SourceCollectionPage';

const SOURCE_NODE: NodeSummary = {
  unique_id: 'source.proj.my_source.tbl1',
  name: 'tbl1',
  resource_type: 'source',
  package_name: 'proj',
  database_name: 'db',
  schema_name: 'sch',
};

/** Shape of `GET /api/v1/sources/:id` consumed by the REST adapter's
 *  `fromSourceDetail` — freshness lives in a nested object. */
const SOURCE_DETAIL = {
  unique_id: SOURCE_NODE.unique_id,
  name: 'tbl1',
  resource_type: 'source',
  package_name: 'proj',
  database_name: 'db',
  schema_name: 'sch',
  identifier: 'tbl1',
  loader: 'fivetran',
  meta: null,
  tags: [],
  fqn: [],
  referenced_by: [],
  columns: [],
  freshness: { status: 'pass', max_loaded_at: '2026-06-01T12:00:00Z' },
  catalog: null,
};

describe('<SourceCollectionPage />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('renders freshness from the per-source detail', async () => {
    // Was a stubbed `/api/v1/sources/:id` response. The fixture is still the right
    // description of the data, so it goes through the same source mapper the app uses.
    const source = createFakeDataSource(
      {
        fetchAsset: async () =>
          DETAIL_REGISTRY.source!.map(
            SOURCE_DETAIL as unknown as Record<string, unknown>,
          ),
      } as never,
      { full: true },
    );

    renderWithProviders(
      <Routes>
        <Route
          path="/sources/:sourceName"
          element={<SourceCollectionPage nodes={[SOURCE_NODE]} onSelect={() => {}} />}
        />
      </Routes>,
      { initialEntries: ['/sources/my_source'], source },
    );

    // The header's "last loaded at" is derived from the per-source detail's
    // `freshnessMaxLoadedAt`. Its presence proves the freshness data now flows
    // from the adapter's `fetchAsset` (the table cell itself is virtualized and
    // doesn't render in jsdom). Match the year to stay timezone-agnostic.
    expect(await screen.findByText(/2026/)).toBeInTheDocument();
  });
});
