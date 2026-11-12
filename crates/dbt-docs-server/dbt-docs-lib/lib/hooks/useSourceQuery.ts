import { useQuery } from '@tanstack/react-query';

import { useMetadataDataSource } from '../context/MetadataDataProvider';
import type { MetadataDataSource } from '../data-sources/MetadataDataSource';

/**
 * Shared substrate for the capability-gated read hooks (`useLineage`,
 * `useFacets`, `useCapabilities`, `useDistribution`). Owns the one piece of
 * logic they all repeat: gate the query on the source advertising `method`
 * (optional fetchers are absent when unsupported), then run `call` against the
 * active {@link MetadataDataSource}. `enabled` layers an extra per-hook gate
 * (e.g. null args) on top of the capability gate.
 *
 * Not for the infinite (`useAssetList`) or lazy/refetch (`useColumnLineage`)
 * hooks — their query semantics differ.
 */
export function useSourceQuery<T>(opts: {
  /** Optional fetcher whose presence on the source gates this query. */
  method: keyof MetadataDataSource;
  queryKey: readonly unknown[];
  call: (source: MetadataDataSource) => Promise<T>;
  /** Extra gate ANDed with the capability gate. Defaults to true. */
  enabled?: boolean;
  staleTime?: number;
}) {
  const source = useMetadataDataSource();
  const supported = opts.method in source;
  const query = useQuery({
    queryKey: opts.queryKey,
    queryFn: () => opts.call(source),
    enabled: supported && (opts.enabled ?? true),
    staleTime: opts.staleTime,
  });

  // A disabled query stays `isPending` forever with `data` undefined, which is
  // indistinguishable from "still loading" — so views render a spinner or an
  // empty state that misattributes the cause. `isSupported` lets them say the
  // honest thing instead. `isPending` is forced false because nothing is coming.
  return {
    ...query,
    isSupported: supported,
    isPending: supported ? query.isPending : false,
  };
}
