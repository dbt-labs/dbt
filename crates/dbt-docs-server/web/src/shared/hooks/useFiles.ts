import { useMetadataDataSource } from '../context/MetadataDataProvider';
import { filesKey } from '../util/queryKeys';
import { useSourceQuery } from './useSourceQuery';

/**
 * Fetch the active {@link MetadataDataSource}'s flat file list — the rows a
 * file tree is built from. Disabled when the source advertises no `fetchFiles`.
 */
export function useFiles() {
  const source = useMetadataDataSource();
  return useSourceQuery({
    method: 'fetchFiles',
    queryKey: filesKey(source.id),
    call: (s) => s.fetchFiles!(),
    staleTime: 5 * 60_000,
  });
}
