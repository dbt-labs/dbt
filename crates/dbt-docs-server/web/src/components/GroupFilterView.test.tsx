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
import { GroupFilterView } from './SimpleFilterViews';

describe('<GroupFilterView />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('loads groups and shows total count', async () => {
    renderWithProviders(
      <GroupFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('group', {
          data: [
            {
              unique_id: 'group.pkg.finance',
              name: 'finance',
              resource_type: 'group',
              package_name: 'pkg',
            },
          ],
          page_info: { total_count: 1, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() => expect(screen.getByText('Loaded 1 of 1')).toBeInTheDocument());
    expect(screen.getByText('Groups')).toBeInTheDocument();
  });

  it('shows empty state when no groups', async () => {
    renderWithProviders(
      <GroupFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('group', {
          data: [],
          page_info: { total_count: 0, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() =>
      expect(screen.getByText('No groups found.')).toBeInTheDocument(),
    );
  });
});
