import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { MetadataDataProvider } from '../context/MetadataDataProvider';
import type { MetadataDataSource } from '../data-sources/MetadataDataSource';
import { createFakeDataSource } from '../testing/createFakeDataSource';
import { UNSUPPORTED_SURFACE_MESSAGE } from './unsupportedSurface';
import { useAssetList } from './useAssetList';
import { useLineage } from './useLineage';

function wrapperFor(source: MetadataDataSource) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      <MetadataDataProvider source={source}>{children}</MetadataDataProvider>
    </QueryClientProvider>
  );
}

/**
 * A data source advertises what it supports by which optional fetchers it
 * defines. These pin the *reported* consequence of an absent one, which is the
 * part that was wrong: the hooks correctly disabled themselves, but a disabled
 * query is indistinguishable from a loading one, so views rendered a forever
 * spinner or an empty list — beside a sidebar count, that reads as data loss.
 */
describe('an unsupported surface', () => {
  it('is reported rather than left looking empty', async () => {
    const { result } = renderHook(() => useAssetList({ filter: {} }), {
      wrapper: wrapperFor(createFakeDataSource()),
    });

    await waitFor(() => {
      expect(result.current.errorMessage).toBe(UNSUPPORTED_SURFACE_MESSAGE);
    });
    // Not pending: nothing is coming, so a spinner would never resolve.
    expect(result.current.isPending).toBe(false);
    expect(result.current.data).toEqual([]);
  });

  it('is distinguishable from a supported surface that returned nothing', async () => {
    const source = createFakeDataSource({
      fetchAssetList: async () => ({ items: [], nextCursor: null, totalCount: 0 }),
    });
    const { result } = renderHook(() => useAssetList({ filter: {} }), {
      wrapper: wrapperFor(source),
    });

    await waitFor(() => expect(result.current.isPending).toBe(false));
    // Genuinely empty, so no message — the empty state is the right answer here.
    expect(result.current.errorMessage).toBeNull();
  });

  it('leaves capability-gated queries un-pending so views can branch', async () => {
    const { result } = renderHook(
      () => useLineage({ uniqueId: 'model.a.b', resourceType: 'model' }),
      { wrapper: wrapperFor(createFakeDataSource()) },
    );

    await waitFor(() => expect(result.current.isSupported).toBe(false));
    expect(result.current.isPending).toBe(false);
    expect(result.current.data).toBeUndefined();
  });

  it('reports supported when the fetcher is present', async () => {
    const source = createFakeDataSource({
      fetchLineage: async () => ({ nodes: [], edges: [] }),
    });
    const { result } = renderHook(
      () => useLineage({ uniqueId: 'model.a.b', resourceType: 'model' }),
      { wrapper: wrapperFor(source) },
    );

    await waitFor(() => expect(result.current.isSupported).toBe(true));
    await waitFor(() => expect(result.current.data).toEqual({ nodes: [], edges: [] }));
  });
});
