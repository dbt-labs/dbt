import { screen, waitFor } from '@testing-library/react';
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
  };
});
import { SavedQueryFilterView } from './SimpleFilterViews';

describe('<SavedQueryFilterView />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('loads saved queries and shows total count', async () => {
    renderWithProviders(
      <SavedQueryFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('saved_query', {
          data: [
            {
              unique_id: 'saved_query.pkg.revenue_sq',
              name: 'revenue_sq',
              resource_type: 'saved_query',
              package_name: 'pkg',
            },
          ],
          page_info: { total_count: 1, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() => expect(screen.getByText('Loaded 1 of 1')).toBeInTheDocument());
    expect(screen.getByText('Saved queries')).toBeInTheDocument();
  });

  it('shows empty state when no saved queries', async () => {
    renderWithProviders(
      <SavedQueryFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('saved_query', {
          data: [],
          page_info: { total_count: 0, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() =>
      expect(screen.getByText('No saved queries found.')).toBeInTheDocument(),
    );
  });
});
