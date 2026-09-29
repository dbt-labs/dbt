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
import { SnapshotFilterView } from './SimpleFilterViews';

describe('<SnapshotFilterView />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('loads snapshots and shows total count', async () => {
    renderWithProviders(
      <SnapshotFilterView project={{ name: 'pkg' }} onPeek={vi.fn()} />,
      {
        source: listSource('snapshot', {
          data: [
            {
              unique_id: 'snapshot.pkg.orders_snap',
              name: 'orders_snap',
              resource_type: 'snapshot',
              package_name: 'pkg',
            },
          ],
          page_info: { total_count: 1, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() => expect(screen.getByText('Loaded 1 of 1')).toBeInTheDocument());
    expect(screen.getByText('Snapshots')).toBeInTheDocument();
  });

  it('shows empty state when no snapshots', async () => {
    renderWithProviders(
      <SnapshotFilterView project={{ name: 'pkg' }} onPeek={vi.fn()} />,
      {
        source: listSource('snapshot', {
          data: [],
          page_info: { total_count: 0, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() =>
      expect(screen.getByText('No snapshots found.')).toBeInTheDocument(),
    );
  });
});
