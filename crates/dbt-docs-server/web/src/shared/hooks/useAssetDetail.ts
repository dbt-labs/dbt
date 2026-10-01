import { useQuery } from '@tanstack/react-query';

import { useMetadataDataSource } from '../context/MetadataDataProvider';
import type { AssetArgs } from '../typings/args';
import type { Asset } from '../typings/domain/asset';
import type { ExecutionInfo } from '../typings/domain/executionInfo';
import { assetKey } from '../util/queryKeys';

/** Poll cadence (ms) while an asset is mid-run. */
const IN_FLIGHT_POLL_MS = 5_000;

/**
 * Whether the asset's latest execution is still in progress. The domain `Asset`
 * union doesn't carry `executionInfo`, but adapters may attach it; read it
 * defensively so polling works without coupling the union to it.
 */
function isAssetInFlight(data: Asset | null | undefined): boolean {
  const exec = (data as { executionInfo?: ExecutionInfo | null } | null | undefined)
    ?.executionInfo;
  if (!exec) return false;
  const status = exec.node?.status ?? exec.job?.status;
  return status === 'running' || status === 'queued';
}

/**
 * Fetch a single asset via the active {@link MetadataDataSource}, polling while
 * its execution is in flight and settling once the run completes.
 */
export function useAssetDetail(args: AssetArgs | null) {
  const source = useMetadataDataSource();

  return useQuery({
    queryKey: args ? assetKey(source.id, args) : [source.id, 'asset', 'none'],
    queryFn: () => source.fetchAsset(args!),
    enabled: args !== null,
    staleTime: 30_000,
    // This hook owns retry; `wrapDataSource`'s retry is for non-hook call sites.
    retry: 2,
    refetchInterval: (query) =>
      isAssetInFlight(query.state.data) ? IN_FLIGHT_POLL_MS : false,
  });
}
