import { useMetadataDataSource } from '../context/MetadataDataProvider';
import { capabilitiesKey } from '../util/queryKeys';
import { useSourceQuery } from './useSourceQuery';

/**
 * Fetch the active {@link MetadataDataSource}'s feature capabilities. Disabled
 * when the source advertises no `fetchCapabilities`.
 */
export function useCapabilities() {
  const source = useMetadataDataSource();
  return useSourceQuery({
    method: 'fetchCapabilities',
    queryKey: capabilitiesKey(source.id),
    call: (s) => s.fetchCapabilities!(),
    staleTime: 5 * 60_000,
  });
}
