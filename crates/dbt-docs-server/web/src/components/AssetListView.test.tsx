import type { ComponentProps } from 'react';
import { fireEvent, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { AssetFilters } from '../App';
import type { Project } from '../shared';
import { renderWithProviders } from '../test/renderWithProviders';
import type { NodeSummary } from '../types';
import { AssetListView } from './AssetListView';

const PROJECT: Project = { name: 'jaffle_shop' };

const NODES: NodeSummary[] = [
  {
    unique_id: 'model.jaffle_shop.stg_orders',
    name: 'stg_orders',
    resource_type: 'model',
    package_name: 'jaffle_shop',
    materialized: 'view',
    original_file_path: 'models/staging/stg_orders.sql',
    schema_name: 'staging',
    description: 'Staged orders',
  },
  {
    unique_id: 'model.jaffle_shop.int_orders_joined',
    name: 'int_orders_joined',
    resource_type: 'model',
    package_name: 'jaffle_shop',
    materialized: 'ephemeral',
    original_file_path: 'models/intermediate/int_orders_joined.sql',
    schema_name: 'intermediate',
  },
  {
    unique_id: 'model.jaffle_shop.fct_orders',
    name: 'fct_orders',
    resource_type: 'model',
    package_name: 'jaffle_shop',
    materialized: 'table',
    original_file_path: 'models/marts/fct_orders.sql',
    schema_name: 'marts',
    description: 'Fact orders table',
  },
  {
    unique_id: 'model.dbt_utils.dim_customers',
    name: 'dim_customers',
    resource_type: 'model',
    package_name: 'dbt_utils',
    materialized: 'table',
    original_file_path: 'models/marts/dim_customers.sql',
    schema_name: 'marts',
  },
  {
    unique_id: 'source.jaffle_shop.raw.orders_tbl',
    name: 'orders_tbl',
    resource_type: 'source',
    package_name: 'jaffle_shop',
    schema_name: 'raw',
  },
  {
    unique_id: 'test.jaffle_shop.not_null_orders_id',
    name: 'not_null_orders_id',
    resource_type: 'test',
    package_name: 'jaffle_shop',
  },
];

function makeFilters(overrides: Partial<AssetFilters> = {}): AssetFilters {
  return {
    resourceType: [],
    modelingLayer: [],
    materialization: [],
    pkg: [],
    tag: [],
    ...overrides,
  };
}

function makeProps(
  overrides: Partial<ComponentProps<typeof AssetListView>> = {},
): ComponentProps<typeof AssetListView> {
  return {
    project: PROJECT,
    nodes: NODES,
    query: '',
    filters: makeFilters(),
    previewId: null,
    onPeek: vi.fn(),
    ...overrides,
  };
}

/** Rows in the tbody, excluding the header row. */
function bodyRows() {
  return screen.getAllByRole('row').slice(1);
}

describe('<AssetListView /> — header title', () => {
  it('shows "All assets" with no resourceType filter', () => {
    renderWithProviders(<AssetListView {...makeProps()} />);
    expect(screen.getByRole('heading', { name: 'All assets' })).toBeInTheDocument();
  });

  it('shows the resource type label when exactly one type is selected', () => {
    renderWithProviders(
      <AssetListView
        {...makeProps({ filters: makeFilters({ resourceType: ['model'] }) })}
      />,
    );
    expect(screen.getByRole('heading', { name: 'Models' })).toBeInTheDocument();
  });

  it('shows "N resource types" when more than one type is selected', () => {
    renderWithProviders(
      <AssetListView
        {...makeProps({ filters: makeFilters({ resourceType: ['model', 'source'] }) })}
      />,
    );
    expect(
      screen.getByRole('heading', { name: '2 resource types' }),
    ).toBeInTheDocument();
  });
});

describe('<AssetListView /> — filtering pipeline', () => {
  it('filters by resourceType', () => {
    renderWithProviders(
      <AssetListView
        {...makeProps({ filters: makeFilters({ resourceType: ['source'] }) })}
      />,
    );
    expect(screen.getByText('orders_tbl')).toBeInTheDocument();
    expect(screen.queryByText('stg_orders')).not.toBeInTheDocument();
    expect(bodyRows()).toHaveLength(1);
  });

  it('filters by modelingLayer (inferred from original_file_path)', () => {
    renderWithProviders(
      <AssetListView
        {...makeProps({ filters: makeFilters({ modelingLayer: ['Marts'] }) })}
      />,
    );
    expect(screen.getByText('fct_orders')).toBeInTheDocument();
    expect(screen.getByText('dim_customers')).toBeInTheDocument();
    expect(screen.queryByText('stg_orders')).not.toBeInTheDocument();
    expect(screen.queryByText('int_orders_joined')).not.toBeInTheDocument();
    expect(bodyRows()).toHaveLength(2);
  });

  it('filters by materialization', () => {
    renderWithProviders(
      <AssetListView
        {...makeProps({ filters: makeFilters({ materialization: ['table'] }) })}
      />,
    );
    expect(screen.getByText('fct_orders')).toBeInTheDocument();
    expect(screen.getByText('dim_customers')).toBeInTheDocument();
    expect(bodyRows()).toHaveLength(2);
  });

  it('filters by pkg', () => {
    renderWithProviders(
      <AssetListView
        {...makeProps({ filters: makeFilters({ pkg: ['dbt_utils'] }) })}
      />,
    );
    expect(screen.getByText('dim_customers')).toBeInTheDocument();
    expect(bodyRows()).toHaveLength(1);
  });

  it('filters by free-text query on name, case-insensitively', () => {
    renderWithProviders(<AssetListView {...makeProps({ query: 'FCT' })} />);
    expect(screen.getByText('fct_orders')).toBeInTheDocument();
    expect(bodyRows()).toHaveLength(1);
  });

  it('filters by free-text query matching unique_id but not name', () => {
    renderWithProviders(<AssetListView {...makeProps({ query: 'jaffle_shop.int' })} />);
    expect(screen.getByText('int_orders_joined')).toBeInTheDocument();
    expect(bodyRows()).toHaveLength(1);
  });

  it('combines multiple filters (pkg + modelingLayer)', () => {
    renderWithProviders(
      <AssetListView
        {...makeProps({
          filters: makeFilters({ pkg: ['jaffle_shop'], modelingLayer: ['Marts'] }),
        })}
      />,
    );
    expect(screen.getByText('fct_orders')).toBeInTheDocument();
    expect(screen.queryByText('dim_customers')).not.toBeInTheDocument();
    expect(bodyRows()).toHaveLength(1);
  });
});

describe('<AssetListView /> — sorting', () => {
  it('defaults to A→Z order', () => {
    renderWithProviders(<AssetListView {...makeProps()} />);
    const names = bodyRows().map((r) => within(r).getAllByRole('cell')[0].textContent);
    expect(names).toEqual([...names].sort((a, b) => (a ?? '').localeCompare(b ?? '')));
    expect(screen.getByRole('radio', { name: 'A→Z' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('radio', { name: 'Z→A' })).toHaveAttribute(
      'aria-checked',
      'false',
    );
  });

  it('reverses order when Z→A is clicked, and aria-checked follows', () => {
    renderWithProviders(<AssetListView {...makeProps()} />);
    fireEvent.click(screen.getByRole('radio', { name: 'Z→A' }));
    const names = bodyRows().map((r) => within(r).getAllByRole('cell')[0].textContent);
    const azSorted = [...names].sort((a, b) => (a ?? '').localeCompare(b ?? ''));
    expect(names).toEqual([...azSorted].reverse());
    expect(screen.getByRole('radio', { name: 'Z→A' })).toHaveAttribute(
      'aria-checked',
      'true',
    );
    expect(screen.getByRole('radio', { name: 'A→Z' })).toHaveAttribute(
      'aria-checked',
      'false',
    );
  });
});

describe('<AssetListView /> — columns', () => {
  it('omits the resource type column when a single type is selected', () => {
    renderWithProviders(
      <AssetListView
        {...makeProps({ filters: makeFilters({ resourceType: ['model'] }) })}
      />,
    );
    expect(
      screen.queryByRole('columnheader', { name: 'Resource type' }),
    ).not.toBeInTheDocument();
  });

  it('shows the resource type column otherwise', () => {
    renderWithProviders(<AssetListView {...makeProps()} />);
    expect(
      screen.getByRole('columnheader', { name: 'Resource type' }),
    ).toBeInTheDocument();
  });
});

describe('<AssetListView /> — empty state', () => {
  it('shows a query-specific message when a query yields no matches', () => {
    renderWithProviders(<AssetListView {...makeProps({ query: 'no-such-asset' })} />);
    expect(screen.getByText(/No matches for "no-such-asset"/)).toBeInTheDocument();
  });

  it('shows a filter-specific message when filters (no query) yield no matches', () => {
    renderWithProviders(
      <AssetListView
        {...makeProps({ filters: makeFilters({ pkg: ['does-not-exist'] }) })}
      />,
    );
    expect(screen.getByText(/No assets match the current filters/)).toBeInTheDocument();
  });
});

describe('<AssetListView /> — pagination', () => {
  const MANY_NODES: NodeSummary[] = Array.from({ length: 210 }, (_, i) => ({
    unique_id: `model.jaffle_shop.model_${String(i).padStart(3, '0')}`,
    name: `model_${String(i).padStart(3, '0')}`,
    resource_type: 'model',
    package_name: 'jaffle_shop',
  }));

  it('renders only the first 200 initially, with a "Showing 200 of 210" footer and a Load more button', () => {
    renderWithProviders(<AssetListView {...makeProps({ nodes: MANY_NODES })} />);
    expect(bodyRows()).toHaveLength(200);
    expect(screen.getByText('Showing 200 of 210')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Load 10 more/ })).toBeInTheDocument();
  });

  it('reveals the rest and hides the button when Load more is clicked', () => {
    renderWithProviders(<AssetListView {...makeProps({ nodes: MANY_NODES })} />);
    fireEvent.click(screen.getByRole('button', { name: /Load 10 more/ }));
    expect(bodyRows()).toHaveLength(210);
    expect(screen.getByText('Showing 210 of 210')).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: /Load.*more/ }),
    ).not.toBeInTheDocument();
  });
});

describe('<AssetListView /> — row interaction', () => {
  it('calls onPeek(unique_id) when a row is clicked', () => {
    const onPeek = vi.fn();
    renderWithProviders(<AssetListView {...makeProps({ onPeek })} />);
    const row = screen.getByText('stg_orders').closest('tr');
    if (!row) throw new Error('row not found');
    fireEvent.click(row);
    expect(onPeek).toHaveBeenCalledWith('model.jaffle_shop.stg_orders');
  });

  it('calls onPeek(unique_id) when Enter is pressed on a focused row', () => {
    const onPeek = vi.fn();
    renderWithProviders(<AssetListView {...makeProps({ onPeek })} />);
    const row = screen.getByText('stg_orders').closest('tr');
    if (!row) throw new Error('row not found');
    fireEvent.keyDown(row, { key: 'Enter' });
    expect(onPeek).toHaveBeenCalledWith('model.jaffle_shop.stg_orders');
  });

  it('calls onPeek(unique_id) when Space is pressed on a focused row', () => {
    const onPeek = vi.fn();
    renderWithProviders(<AssetListView {...makeProps({ onPeek })} />);
    const row = screen.getByText('stg_orders').closest('tr');
    if (!row) throw new Error('row not found');
    fireEvent.keyDown(row, { key: ' ' });
    expect(onPeek).toHaveBeenCalledWith('model.jaffle_shop.stg_orders');
  });

  it('marks the row matching previewId as active', () => {
    renderWithProviders(
      <AssetListView {...makeProps({ previewId: 'model.jaffle_shop.stg_orders' })} />,
    );
    const activeRow = screen.getByText('stg_orders').closest('tr');
    const otherRow = screen.getByText('fct_orders').closest('tr');
    expect(activeRow?.className).toContain('is-active');
    expect(otherRow?.className).not.toContain('is-active');
  });
});

describe('<AssetListView /> — ActiveFilters pills', () => {
  it('renders one pill per non-empty modelingLayer/materialization/pkg filter', () => {
    renderWithProviders(
      <AssetListView
        {...makeProps({
          filters: makeFilters({
            modelingLayer: ['Marts'],
            materialization: ['table'],
            pkg: ['jaffle_shop'],
            resourceType: ['model'], // should NOT produce a pill
          }),
          query: 'fct', // should NOT produce a pill
        })}
      />,
    );
    expect(screen.getByText('Modeling layer:')).toBeInTheDocument();
    expect(screen.getByText('Materialization:')).toBeInTheDocument();
    expect(screen.getByText('Package:')).toBeInTheDocument();
    expect(screen.queryByText('Resource type:')).not.toBeInTheDocument();
    expect(screen.queryByText('Query:')).not.toBeInTheDocument();
  });

  it('renders nothing when modelingLayer/materialization/pkg are all empty', () => {
    renderWithProviders(<AssetListView {...makeProps()} />);
    expect(screen.queryByText('Filters')).not.toBeInTheDocument();
  });
});
