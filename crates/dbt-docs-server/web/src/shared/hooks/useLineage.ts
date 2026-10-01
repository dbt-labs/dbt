import { useMetadataDataSource } from '../context/MetadataDataProvider';
import type { LineageArgs } from '../typings/args';
import { lineageKey } from '../util/queryKeys';
import { useSourceQuery } from './useSourceQuery';

/**
 * Fetch node-level lineage for an asset via the active {@link MetadataDataSource}.
 * Disabled when `args` is null or the source advertises no `fetchLineage`.
 */
export function useLineage(args: LineageArgs | null) {
  const source = useMetadataDataSource();
  return useSourceQuery({
    method: 'fetchLineage',
    queryKey: args ? lineageKey(source.id, args) : [source.id, 'lineage', 'none'],
    call: (s) => s.fetchLineage!(args!),
    enabled: args !== null,
    staleTime: 30_000,
  });
}
