import { useQuery } from '@tanstack/react-query';

import { useBootstrapData } from '../lib/bootstrapContext';
import type { NodeSummary } from '../types';

export interface UseAllNodesResult {
  /** Null until the bootstrap read resolves. */
  nodes: NodeSummary[] | null;
  total: number | null;
  isPending: boolean;
  error: Error | null;
}

/**
 * Every node in the project, from one parquet read.
 *
 * `dbt.nodes_index` is exactly the nine columns this needs, so the whole project
 * arrives in a single request that `main.tsx` started before React mounted. This
 * used to page `GET /api/v1/nodes` a thousand rows at a time and auto-fetch the
 * next as each landed, which is why callers still see a `total` — it is now just
 * `nodes.length`, and there is no partial state to report progress on.
 */
export function useAllNodes(): UseAllNodesResult {
  const bootstrap = useBootstrapData();
  const query = useQuery({
    queryKey: ['bootstrapNodes'],
    // The promise is created once per page load, so this resolves immediately on any
    // refetch — react-query is just how the result reaches the component tree.
    queryFn: () => bootstrap,
    staleTime: Infinity,
  });

  return {
    nodes: query.data?.nodes ?? null,
    total: query.data?.nodes.length ?? null,
    isPending: query.isPending,
    error: query.error,
  };
}
