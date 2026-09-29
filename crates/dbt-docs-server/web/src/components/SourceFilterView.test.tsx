import React from 'react';
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
vi.mock('react-router-dom', async (importOriginal) => {
  const mod = await importOriginal<typeof import('react-router-dom')>();
  return {
    ...mod,
    Link: ({
      to,
      children,
      className,
    }: {
      to: string;
      children: React.ReactNode;
      className?: string;
    }) => (
      <a href={to} className={className}>
        {children}
      </a>
    ),
  };
});

const twoSources = {
  data: [
    {
      unique_id: 'source.pkg.raw.orders',
      name: 'orders',
      resource_type: 'source',
      package_name: 'pkg',
      database_name: 'prod',
      schema_name: 'public',
    },
    {
      unique_id: 'source.pkg.raw.users',
      name: 'users',
      resource_type: 'source',
      package_name: 'pkg',
      database_name: 'dev',
      schema_name: 'staging',
    },
  ],
  page_info: { total_count: 2, has_next_page: false, end_cursor: null },
};

import { SourceFilterView } from './SourceFilterView';

describe('<SourceFilterView />', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('renders database and schema filter dropdowns', async () => {
    renderWithProviders(<SourceFilterView project={{ name: 'test_project' }} />, {
      source: listSource('source', twoSources),
    });
    await waitFor(() =>
      expect(screen.getByTestId('filter-Database')).toBeInTheDocument(),
    );
    expect(screen.getByTestId('filter-Schema')).toBeInTheDocument();
  });

  it('loads sources and groups into collections', async () => {
    renderWithProviders(<SourceFilterView project={{ name: 'test_project' }} />, {
      source: listSource('source', twoSources),
    });
    // Two sources with same sourceName 'raw' → 1 collection → header count shows 1
    await waitFor(() => expect(screen.getByText('Loaded 1 of 1')).toBeInTheDocument());
    expect(screen.getByText('Sources')).toBeInTheDocument();
  });

  it('shows empty state when no sources', async () => {
    renderWithProviders(<SourceFilterView project={{ name: 'test_project' }} />, {
      source: listSource('source', {
        data: [],
        page_info: { total_count: 0, has_next_page: false, end_cursor: null },
      }),
    });
    await waitFor(() =>
      expect(screen.getByText('No sources found.')).toBeInTheDocument(),
    );
  });
});
