import { useMetadataDataSource } from '../../context/MetadataDataProvider';
import { useSourceQuery } from '../../hooks/useSourceQuery';
import { assetCountsKey } from '../../utils/queryKeys';

/**
 * Fetch project-wide per-resource-type asset counts from the active
 * {@link MetadataDataSource}. Disabled when the source advertises no
 * `fetchAssetCounts`.
 */
export function useAssetCounts() {
  const source = useMetadataDataSource();
  return useSourceQuery({
    method: 'fetchAssetCounts',
    queryKey: assetCountsKey(source.id),
    call: (s) => s.fetchAssetCounts!(),
    staleTime: 5 * 60_000,
  });
}
