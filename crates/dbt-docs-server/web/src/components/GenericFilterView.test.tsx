import type { ColumnDef } from '@tanstack/react-table';
import { screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { ModelSummary } from '../shared';
import { renderWithProviders } from '../test/renderWithProviders';
import { listSource } from '../test/wireFixtures';
import { GenericFilterView } from './GenericFilterView';

/** Local mirror of the REST list envelope this test mocks (the adapter maps it
 *  into the domain shape). */
interface ModelListResponse {
  data: Record<string, unknown>[];
  page_info: {
    total_count: number;
    start_cursor: string | null;
    end_cursor: string | null;
    has_next_page: boolean;
  };
}

vi.mock('../shared', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../shared')>();
  return {
    ...mod,
    useResourceLink: () => ({ home: () => '/' }),
    SimpleLinkBreadcrumbs: ({ breadcrumbs }: { breadcrumbs: { text: string }[] }) => (
      <nav>{breadcrumbs.map((b) => b.text).join(' / ')}</nav>
    ),
  };
});

// Capture the sort-related props GenericFilterView forwards.
vi.mock('./ResourceFilterTable', () => ({
  ResourceFilterTable: ({
    isSortable,
    initialSortColumn,
    initialSortDesc,
  }: {
    isSortable?: boolean;
    initialSortColumn?: string;
    initialSortDesc?: boolean;
  }) => (
    <div
      data-testid="rft"
      data-sortable={String(isSortable)}
      data-initial-col={initialSortColumn ?? ''}
      data-initial-desc={String(initialSortDesc)}
    />
  ),
}));

const EMPTY_LIST: ModelListResponse = {
  data: [],
  page_info: {
    total_count: 0,
    has_next_page: false,
    end_cursor: null,
    start_cursor: null,
  },
};

const columns: ColumnDef<ModelSummary>[] = [
  { id: 'name', accessorFn: (row) => row.name },
];

describe('<GenericFilterView />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('forwards sort props to ResourceFilterTable', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() =>
        Promise.resolve({ ok: true, json: () => Promise.resolve(EMPTY_LIST) }),
      ),
    );
    renderWithProviders(
      <GenericFilterView<ModelSummary>
        label="Models"
        project={{ name: 'p' }}
        resourceType="model"
        columns={columns}
        isSortable
        initialSortColumn="executed_at"
        initialSortDesc
      />,
    );
    const rft = await screen.findByTestId('rft');
    expect(rft.dataset.sortable).toBe('true');
    expect(rft.dataset.initialCol).toBe('executed_at');
    expect(rft.dataset.initialDesc).toBe('true');
  });

  it('threads sort into the list request', async () => {
    // Was an assertion on `sort=` in the URL. The sort now travels as a `ListSort` on
    // the source call, so that is what this checks — same intent, no transport.
    const fetchAssetList = vi.fn(
      async (_args: { filter?: Record<string, unknown>; sort?: unknown }) => ({
        items: [],
        nextCursor: null,
        totalCount: 0,
      }),
    );
    renderWithProviders(
      <GenericFilterView<ModelSummary>
        label="Models"
        project={{ name: 'p' }}
        resourceType="model"
        columns={columns}
        sort={{ field: 'executed_at', desc: true }}
      />,
      { source: listSource('model', EMPTY_LIST, { fetchAssetList } as never) },
    );
    await waitFor(() =>
      expect(
        fetchAssetList.mock.calls.some(
          (c) =>
            (c[0] as { sort?: { field?: string; desc?: boolean } })?.sort?.field ===
              'executed_at' &&
            (c[0] as { sort?: { desc?: boolean } })?.sort?.desc === true,
        ),
      ).toBe(true),
    );
  });
});
