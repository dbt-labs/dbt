import { fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { createFakeDataSource } from '../shared/testing/createFakeDataSource';
import { renderWithProviders } from '../test/renderWithProviders';
import { pageFromWire } from '../test/wireFixtures';

/** Local mirror of the REST `/models` list envelope this test mocks (the
 *  adapter maps it into the domain shape). */
interface ModelListResponse {
  data: Record<string, unknown>[];
  page_info: {
    total_count: number;
    start_cursor: string | null;
    end_cursor: string | null;
    has_next_page: boolean;
  };
}

/** Shape of `GET /api/v1/models/facets` the REST adapter maps. */
interface ModelFacetsBody {
  modeling_layers: { value: string; count: number | null }[];
  owners: { value: string; count: number | null }[];
  packages: { value: string; count: number | null }[];
}

vi.mock('../shared', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../shared')>();
  return {
    ...mod,
    useResourceLink: () => ({ home: () => '/' }),
    SimpleLinkBreadcrumbs: ({ breadcrumbs }: { breadcrumbs: { text: string }[] }) => (
      <nav>{breadcrumbs.map((b) => b.text).join(' / ')}</nav>
    ),
    FilterDropdown: ({
      name,
      onChange,
      defaultOption,
      options,
    }: {
      name: string;
      onChange: (opt: { value: string }) => void;
      defaultOption: { label: string; value: string };
      options: { label: string; value: string }[];
    }) => (
      <select
        data-testid={`filter-${name}`}
        value={defaultOption.value}
        onChange={(e) => onChange({ value: e.target.value })}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    ),
  };
});

vi.mock('./ResourceFilterTable', () => ({
  ResourceFilterTable: ({
    columns,
    data,
    total,
    onChangeSort,
  }: {
    columns: { id?: string; accessorFn?: (row: any) => unknown }[];

    data: any[];
    total: number | null;
    onChangeSort?: (sortBy: { id: string; desc: boolean }[]) => void;
  }) => (
    <div>
      {data.map((row, ri) =>
        columns.map((col, ci) => {
          const val = col.accessorFn ? col.accessorFn(row) : undefined;
          return val != null && val !== '' ? (
            <span key={`${ri}-${ci}`}>{String(val)}</span>
          ) : null;
        }),
      )}
      {/* Stand-in for a sortable header click — drives the view's onChangeSort. */}
      <button
        type="button"
        data-testid="sort-name"
        onClick={() => onChangeSort?.([{ id: 'name', desc: false }])}
      >
        sort name
      </button>
      {total != null && <span>{`Loaded ${data.length} of ${total}`}</span>}
    </div>
  ),
}));

const emptyFacets: ModelFacetsBody = {
  modeling_layers: [],
  owners: [],
  packages: [],
};

/**
 * A source serving the same two fixtures the fetch stub used to.
 *
 * Returns the `fetchAssetList` spy alongside it: the tests that asserted on request
 * URLs now assert on the filter and sort the view passes down, which is the same intent
 * with the transport removed.
 */
function makeSource(
  modelRes: ModelListResponse,
  facets: ModelFacetsBody = emptyFacets,
) {
  const fetchAssetList = vi.fn(async (args: unknown) => {
    void args;
    return pageFromWire('model', modelRes as never);
  });
  const source = createFakeDataSource(
    {
      fetchAssetList,
      fetchFacets: async () => ({
        owners: facets.owners ?? [],
        modelingLayers: facets.modeling_layers ?? [],
        ...(facets as {
          materializations?: unknown[];
          accesses?: unknown[];
          packages?: unknown[];
        }),
      }),
    } as never,
    { full: true },
  );
  return { source, fetchAssetList };
}

const EMPTY_LIST: ModelListResponse = {
  data: [],
  page_info: {
    total_count: 0,
    has_next_page: false,
    end_cursor: null,
    start_cursor: null,
  },
};

import { ModelFilterView } from './ModelFilterView';

describe('<ModelFilterView />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('loads models and shows total count', async () => {
    const { source } = makeSource({
      data: [
        {
          unique_id: 'model.pkg.orders',
          name: 'orders',
          package_name: 'pkg',
          original_file_path: 'models/orders.sql',
          modeling_layer: 'mart',
          access_level: 'public',
          contract_enforced: true,
          owner: 'alice',
          executed_at: null,
          has_catalog: false,
        },
      ],
      page_info: {
        total_count: 1,
        has_next_page: false,
        end_cursor: null,
        start_cursor: null,
      },
    });
    renderWithProviders(
      <ModelFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { source },
    );
    await waitFor(() => expect(screen.getByText('Loaded 1 of 1')).toBeInTheDocument());
    expect(screen.getByText('Models')).toBeInTheDocument();
  });

  it('renders formatted row count when catalog stat present', async () => {
    const { source } = makeSource({
      data: [
        {
          unique_id: 'model.pkg.big',
          name: 'big',
          package_name: 'pkg',
          original_file_path: 'models/big.sql',
          modeling_layer: null,
          access_level: null,
          contract_enforced: null,
          owner: null,
          executed_at: null,
          // Flat, as the model list projection emits them — the nested `catalog`
          // object is assembled by the mapper, not supplied to it.
          has_catalog: true,
          row_count_stat: 1234567,
          bytes_stat: null,
          last_modified_stat: null,
        },
      ],
      page_info: {
        total_count: 1,
        has_next_page: false,
        end_cursor: null,
        start_cursor: null,
      },
    });
    renderWithProviders(
      <ModelFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { source },
    );
    // The cell uses locale-aware `toLocaleString()`, so build the expected grouped
    // string the same way rather than hardcoding en-US's '1,234,567'.
    await waitFor(() =>
      expect(screen.getByText((1234567).toLocaleString())).toBeInTheDocument(),
    );
  });

  it('renders empty cell when catalog is null', async () => {
    const { source } = makeSource({
      data: [
        {
          unique_id: 'model.pkg.no_catalog',
          name: 'no_catalog',
          package_name: 'pkg',
          original_file_path: 'models/no_catalog.sql',
          modeling_layer: null,
          access_level: null,
          contract_enforced: null,
          owner: null,
          executed_at: null,
          has_catalog: false,
        },
      ],
      page_info: {
        total_count: 1,
        has_next_page: false,
        end_cursor: null,
        start_cursor: null,
      },
    });
    renderWithProviders(
      <ModelFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { source },
    );
    await waitFor(() => expect(screen.getByText('no_catalog')).toBeInTheDocument());
    expect(screen.queryByText(/\d{1,3}(,\d{3})+/)).not.toBeInTheDocument();
  });

  it('carries the modeling_layer deep-link param into the list fetch', async () => {
    const { source, fetchAssetList } = makeSource({
      data: [],
      page_info: {
        total_count: 0,
        has_next_page: false,
        end_cursor: null,
        start_cursor: null,
      },
    });
    renderWithProviders(
      <ModelFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { initialEntries: ['/models?modeling_layer=Marts'], source },
    );
    await waitFor(() =>
      expect(
        fetchAssetList.mock.calls.some((c) =>
          (
            c[0] as { filter?: { modelingLayers?: string[] } }
          )?.filter?.modelingLayers?.includes('Marts'),
        ),
      ).toBe(true),
    );
  });

  it('refetches with the mapped sort field when a sortable header is clicked', async () => {
    const { source, fetchAssetList } = makeSource({
      data: [],
      page_info: {
        total_count: 0,
        has_next_page: false,
        end_cursor: null,
        start_cursor: null,
      },
    });
    renderWithProviders(
      <ModelFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { source },
    );
    // Wait for the initial (unsorted) models fetch.
    await waitFor(() => expect(fetchAssetList.mock.calls.length > 0).toBe(true));

    fireEvent.click(screen.getByTestId('sort-name'));

    await waitFor(() =>
      expect(
        fetchAssetList.mock.calls.some(
          (c) => (c[0] as { sort?: { field?: string } })?.sort?.field === 'name',
        ),
      ).toBe(true),
    );
  });

  it('renders facet dropdown options with counts', async () => {
    const { source } = makeSource(EMPTY_LIST, {
      modeling_layers: [{ value: 'Marts', count: 3 }],
      owners: [{ value: 'alice', count: 2 }],
      packages: [{ value: 'jaffle_shop', count: 5 }],
    });
    renderWithProviders(
      <ModelFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { source },
    );
    await waitFor(() =>
      expect(screen.getByRole('option', { name: 'alice (2)' })).toBeInTheDocument(),
    );
    expect(screen.getByRole('option', { name: 'Marts (3)' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'jaffle_shop (5)' })).toBeInTheDocument();
  });

  it('re-issues the list with owner= when an owner is selected', async () => {
    const { source, fetchAssetList } = makeSource(EMPTY_LIST, {
      modeling_layers: [],
      owners: [{ value: 'alice', count: 2 }],
      packages: [],
    });
    renderWithProviders(
      <ModelFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { source },
    );
    await waitFor(() =>
      expect(screen.getByRole('option', { name: 'alice (2)' })).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByTestId('filter-Owner'), {
      target: { value: 'alice' },
    });

    await waitFor(() =>
      expect(
        fetchAssetList.mock.calls.some((c) =>
          (c[0] as { filter?: { owners?: string[] } })?.filter?.owners?.includes(
            'alice',
          ),
        ),
      ).toBe(true),
    );
  });
});
