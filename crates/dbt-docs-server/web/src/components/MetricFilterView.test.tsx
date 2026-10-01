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
import { MetricFilterView } from './SimpleFilterViews';

describe('<MetricFilterView />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('loads metrics and shows total count', async () => {
    renderWithProviders(
      <MetricFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('metric', {
          data: [
            {
              unique_id: 'metric.pkg.revenue',
              name: 'revenue',
              resource_type: 'metric',
              package_name: 'pkg',
            },
          ],
          page_info: { total_count: 1, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() => expect(screen.getByText('Loaded 1 of 1')).toBeInTheDocument());
    expect(screen.getByText('Metrics')).toBeInTheDocument();
  });

  it('shows empty state when no metrics', async () => {
    renderWithProviders(
      <MetricFilterView project={{ name: 'test_project' }} onPeek={vi.fn()} />,
      {
        source: listSource('metric', {
          data: [],
          page_info: { total_count: 0, has_next_page: false, end_cursor: null },
        }),
      },
    );
    await waitFor(() =>
      expect(screen.getByText('No metrics found.')).toBeInTheDocument(),
    );
  });
});
