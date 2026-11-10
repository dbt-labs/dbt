import { fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { renderWithProviders } from '../test/renderWithProviders';
import { listSource } from '../test/wireFixtures';

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
const TESTS_RESPONSE = {
  data: [
    {
      unique_id: 'test.pkg.not_null_id',
      name: 'not_null_id',
      resource_type: 'test',
      package_name: 'pkg',
      tested_node_unique_id: 'model.pkg.customers',
      tested_column: 'id',
      execution_info: { status: 'pass' },
    },
  ],
  page_info: { total_count: 1, has_next_page: false, end_cursor: null },
};

/** Branches on `/facets` so the list and facet endpoints get distinct bodies. */

const EMPTY_LIST = {
  data: [],
  page_info: { total_count: 0, has_next_page: false, end_cursor: null },
};

import { TestFilterView } from './TestFilterView';

describe('<TestFilterView />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('loads tests and shows total count', async () => {
    renderWithProviders(
      <TestFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { source: listSource('test', TESTS_RESPONSE) },
    );
    await waitFor(() => expect(screen.getByText('Loaded 1 of 1')).toBeInTheDocument());
    expect(screen.getByText('Tests')).toBeInTheDocument();
  });

  it('shows empty state when no tests', async () => {
    renderWithProviders(
      <TestFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { source: listSource('test', EMPTY_LIST) },
    );
    await waitFor(() =>
      expect(screen.getByText('No tests found.')).toBeInTheDocument(),
    );
  });

  it('renders capitalized result/test-type facet options', async () => {
    renderWithProviders(
      <TestFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('test', EMPTY_LIST, {
          fetchFacets: async () => ({
            results: [{ value: 'pass', count: 10 }],
            testTypes: [{ value: 'data', count: 7 }],
          }),
        } as never),
      },
    );
    await waitFor(() =>
      expect(screen.getByRole('option', { name: 'Pass (10)' })).toBeInTheDocument(),
    );
    expect(screen.getByRole('option', { name: 'Data (7)' })).toBeInTheDocument();
  });

  it('re-issues the list with result= when a result is selected', async () => {
    // Was an assertion on the request URL; the mechanism is a source call now, so it
    // asserts on the filter the view passes down instead.
    const fetchAssetList = vi.fn(
      async (_args: { filter?: Record<string, unknown>; sort?: unknown }) => ({
        items: [],
        nextCursor: null,
        totalCount: 0,
      }),
    );
    renderWithProviders(
      <TestFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('test', EMPTY_LIST, {
          fetchAssetList,
          fetchFacets: async () => ({
            results: [{ value: 'pass', count: 10 }],
            testTypes: [],
          }),
        } as never),
      },
    );
    await waitFor(() =>
      expect(screen.getByRole('option', { name: 'Pass (10)' })).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByTestId('filter-Test result'), {
      target: { value: 'pass' },
    });

    await waitFor(() =>
      expect(
        fetchAssetList.mock.calls.some((c) =>
          (c[0] as { filter?: { results?: string[] } })?.filter?.results?.includes(
            'pass',
          ),
        ),
      ).toBe(true),
    );
  });
});
