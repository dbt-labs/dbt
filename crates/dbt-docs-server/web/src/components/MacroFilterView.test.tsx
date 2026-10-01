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
import { MacroFilterView } from './MacroFilterView';

describe('<MacroFilterView />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('loads macros and shows total count', async () => {
    renderWithProviders(
      <MacroFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('macro', {
          data: [
            {
              unique_id: 'macro.pkg.generate_schema',
              name: 'generate_schema',
              resource_type: 'macro',
              package_name: 'pkg',
            },
          ],
          page_info: { total_count: 1, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() => expect(screen.getByText('Loaded 1 of 1')).toBeInTheDocument());
    expect(screen.getByText('Macros')).toBeInTheDocument();
  });

  it('shows empty state when no macros', async () => {
    renderWithProviders(
      <MacroFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { source: listSource('macro', { data: [] }) },
    );
    await waitFor(() =>
      expect(screen.getByText('No macros found.')).toBeInTheDocument(),
    );
  });

  it('renders package facet options and filters on selection', async () => {
    // This used to assert on the request URL. The mechanism is now a source call, so
    // the assertion moves to the filter the view passes down — same intent, one less
    // layer of indirection.
    const fetchAssetList = vi.fn(
      async (_args: { filter?: Record<string, unknown>; sort?: unknown }) => ({
        items: [],
        nextCursor: null,
        totalCount: 0,
      }),
    );
    const source = listSource('macro', { data: [] }, {
      fetchAssetList,
      fetchFacets: async () => ({ packages: [{ value: 'jaffle_shop', count: 5 }] }),
    } as never);

    renderWithProviders(
      <MacroFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      { source },
    );
    await waitFor(() =>
      expect(
        screen.getByRole('option', { name: 'jaffle_shop (5)' }),
      ).toBeInTheDocument(),
    );

    fireEvent.change(screen.getByTestId('filter-Package'), {
      target: { value: 'jaffle_shop' },
    });

    await waitFor(() =>
      expect(
        fetchAssetList.mock.calls.some((c) =>
          (c[0] as { filter?: { packages?: string[] } })?.filter?.packages?.includes(
            'jaffle_shop',
          ),
        ),
      ).toBe(true),
    );
  });
});
