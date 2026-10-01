import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';

import {
  createFakeDataSource,
  type LineageGraph,
  MetadataDataProvider,
} from '../shared';
import { useLineageData } from './useLineageData';

const GRAPH: LineageGraph = {
  nodes: [
    {
      uniqueId: 'model.shop.customers',
      name: 'customers',
      resourceType: 'model',
      description: null,
      packageName: 'shop',
      tags: [],
      materialized: 'incremental',
    },
    {
      uniqueId: 'source.shop.raw.orders',
      name: 'orders',
      resourceType: 'source',
      description: null,
      packageName: 'shop',
      tags: [],
      materialized: null,
    },
  ],
  edges: [
    {
      upstreamUniqueId: 'source.shop.raw.orders',
      downstreamUniqueId: 'model.shop.customers',
    },
  ],
};

function render() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const fetchLineage = vi.fn().mockResolvedValue(GRAPH);
  const source = createFakeDataSource({ fetchLineage });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      <MetadataDataProvider source={source}>{children}</MetadataDataProvider>
    </QueryClientProvider>
  );
  return { wrapper, fetchLineage };
}

describe('useLineageData', () => {
  test('builds dagNodes with parents and materializationType from the shared hook', async () => {
    const { wrapper, fetchLineage } = render();
    const { result } = renderHook(() => useLineageData('model.shop.customers', 3), {
      wrapper,
    });

    await waitFor(() => expect(result.current.dagNodes).toHaveLength(2));

    // Resource type + depth forwarded to the shared fetcher.
    expect(fetchLineage).toHaveBeenCalledWith({
      uniqueId: 'model.shop.customers',
      resourceType: 'model',
      depth: 3,
    });

    const customers = result.current.dagNodes.find(
      (n) => n.id === 'model.shop.customers',
    )!;
    expect(customers.materializationType).toBe('incremental');
    expect(customers.parents).toEqual(['source.shop.raw.orders']);

    const orders = result.current.dagNodes.find(
      (n) => n.id === 'source.shop.raw.orders',
    )!;
    expect(orders.materializationType).toBeNull();
    expect(orders.parents).toEqual([]);
  });
});
