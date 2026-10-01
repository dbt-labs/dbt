import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { ResourceFilterTable } from './ResourceFilterTable';

const baseProps = {
  columns: [],
  data: [],
  isLoading: false,
  hasMore: false,
  onLoadMore: vi.fn(),
  total: null,
  shownCount: 0,
};

describe('<ResourceFilterTable />', () => {
  it('shows custom emptyMessage when no data and not loading', () => {
    render(<ResourceFilterTable {...baseProps} emptyMessage="Nothing here" />);
    expect(screen.getByText('Nothing here')).toBeInTheDocument();
  });

  it('defaults to generic empty message', () => {
    render(<ResourceFilterTable {...baseProps} />);
    expect(
      screen.getByText('No resources match the current filters.'),
    ).toBeInTheDocument();
  });

  it('shows error message when error prop set', () => {
    render(<ResourceFilterTable {...baseProps} error="Failed to load models." />);
    expect(screen.getByText('Failed to load models.')).toBeInTheDocument();
  });

  it('error takes precedence over empty state', () => {
    render(
      <ResourceFilterTable
        {...baseProps}
        error="Failed to load models."
        emptyMessage="Nothing here"
      />,
    );
    expect(screen.getByText('Failed to load models.')).toBeInTheDocument();
    expect(screen.queryByText('Nothing here')).not.toBeInTheDocument();
  });

  it('shows count in footer', () => {
    render(
      <ResourceFilterTable
        {...baseProps}
        data={[{ id: '1' }]}
        total={10}
        shownCount={1}
      />,
    );
    expect(screen.getByText(/Loaded 1 of 10/)).toBeInTheDocument();
  });
});
