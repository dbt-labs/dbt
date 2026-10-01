import type { ReactNode } from 'react';
import { MemoryRouter } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, test, vi } from 'vitest';

import {
  type Asset,
  createFakeDataSource,
  makeFakeModelAsset,
  MetadataDataProvider,
} from '../shared';
import { NodeLineagePanel } from './NodeLineagePanel';

function renderPanel(asset: Asset) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const fetchAsset = vi.fn().mockResolvedValue(asset);
  const source = createFakeDataSource({ fetchAsset });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      <MetadataDataProvider source={source}>
        <MemoryRouter>{children}</MemoryRouter>
      </MetadataDataProvider>
    </QueryClientProvider>
  );
  render(<NodeLineagePanel uniqueId={asset.uniqueId} onClose={() => {}} />, {
    wrapper,
  });
  return { fetchAsset };
}

describe('NodeLineagePanel', () => {
  test('renders the resolved asset on the general tab', async () => {
    renderPanel(
      makeFakeModelAsset({
        uniqueId: 'model.shop.customers',
        name: 'customers',
        description: 'The customers model.',
        columns: [
          {
            name: 'id',
            description: 'pk',
            dataType: 'integer',
            declaredType: null,
            catalogType: null,
            tags: [],
            meta: {},
          },
        ],
        dependsOn: ['source.shop.raw.orders'],
        referencedBy: ['model.shop.revenue'],
      }),
    );

    expect(await screen.findByText('customers')).toBeInTheDocument();
    expect(screen.getByText('The customers model.')).toBeInTheDocument();
  });

  test('dispatches fetchAsset with the resource type inferred from the unique_id', async () => {
    const { fetchAsset } = renderPanel(makeFakeModelAsset());
    await waitFor(() =>
      expect(fetchAsset).toHaveBeenCalledWith(
        expect.objectContaining({
          uniqueId: 'model.jaffle_shop.customers',
          resourceType: 'model',
        }),
      ),
    );
  });
});
