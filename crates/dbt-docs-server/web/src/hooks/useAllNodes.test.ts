import { createElement, type ReactNode } from 'react';
import { renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { BootstrapProvider } from '../lib/bootstrapContext';
import type { BootstrapData } from '../shared/data-sources/duckdb/bootstrap';
import { createQueryWrapper } from '../test/renderWithProviders';
import type { NodeSummary } from '../types';
import { useAllNodes } from './useAllNodes';

const node = (id: string): NodeSummary => ({
  unique_id: `seed.pkg.${id}`,
  name: id,
  resource_type: 'seed',
  package_name: 'pkg',
  description: null,
});

function bootstrapData(nodes: NodeSummary[]): BootstrapData {
  return {
    nodes,
    project: { name: 'pkg', description: null, dbtVersion: null, adapterType: null },
    generation: null,
  } as unknown as BootstrapData;
}

/** The provider stack the hook expects: a QueryClient plus the in-flight read. */
function wrapper(read: Promise<BootstrapData>) {
  const QueryWrapper = createQueryWrapper();
  return ({ children }: { children: ReactNode }) =>
    createElement(
      QueryWrapper,
      null,
      createElement(BootstrapProvider, {
        value: read,
        children,
      }),
    );
}

describe('useAllNodes', () => {
  it('resolves the whole project from the bootstrap read', async () => {
    const { result } = renderHook(() => useAllNodes(), {
      wrapper: wrapper(Promise.resolve(bootstrapData([node('a'), node('b')]))),
    });

    await waitFor(() => expect(result.current.nodes).toHaveLength(2));
    expect(result.current.total).toBe(2);
    expect(result.current.error).toBeNull();
  });

  it('reports pending until the read settles', () => {
    const { result } = renderHook(() => useAllNodes(), {
      wrapper: wrapper(new Promise<BootstrapData>(() => {})),
    });

    expect(result.current.isPending).toBe(true);
    expect(result.current.nodes).toBeNull();
  });

  it('surfaces a failed read as an error rather than an empty project', async () => {
    const { result } = renderHook(() => useAllNodes(), {
      wrapper: wrapper(Promise.reject(new Error('boom'))),
    });

    await waitFor(() => expect(result.current.error).not.toBeNull());
    // An empty list here would render as "this project has no nodes", which is a
    // different and much more alarming claim than "the read failed".
    expect(result.current.nodes).toBeNull();
  });
});
