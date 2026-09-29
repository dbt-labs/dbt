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
import { SeedFilterView } from './SimpleFilterViews';

describe('<SeedFilterView />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('loads seeds and shows total count', async () => {
    renderWithProviders(
      <SeedFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('seed', {
          data: [
            {
              unique_id: 'seed.pkg.raw',
              name: 'raw',
              resource_type: 'seed',
              package_name: 'pkg',
            },
          ],
          page_info: { total_count: 1, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() => expect(screen.getByText('Loaded 1 of 1')).toBeInTheDocument());
    expect(screen.getByText('Seeds')).toBeInTheDocument();
  });

  it('shows empty state when no seeds', async () => {
    renderWithProviders(
      <SeedFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('seed', {
          data: [],
          page_info: { total_count: 0, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() =>
      expect(screen.getByText('No seeds found.')).toBeInTheDocument(),
    );
  });
});
