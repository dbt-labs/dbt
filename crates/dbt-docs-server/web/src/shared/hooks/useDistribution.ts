import { useMetadataDataSource } from '../context/MetadataDataProvider';
import { distributionKey } from '../util/queryKeys';
import { useSourceQuery } from './useSourceQuery';

/**
 * Fetch the active {@link MetadataDataSource}'s build distribution (dbt v2 vs
 * Core, login state). Disabled when the source advertises no
 * `fetchDistribution`.
 */
export function useDistribution() {
  const source = useMetadataDataSource();
  return useSourceQuery({
    method: 'fetchDistribution',
    queryKey: distributionKey(source.id),
    call: (s) => s.fetchDistribution!(),
    staleTime: 5 * 60_000,
  });
}
