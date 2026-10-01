import { useMetadataDataSource } from '../context/MetadataDataProvider';
import { projectKey } from '../util/queryKeys';
import { useSourceQuery } from './useSourceQuery';

/**
 * Fetch the active {@link MetadataDataSource}'s project identity (name,
 * dbt/adapter versions, git state). Disabled when the source advertises no
 * `fetchProject`.
 */
export function useProject() {
  const source = useMetadataDataSource();
  return useSourceQuery({
    method: 'fetchProject',
    queryKey: projectKey(source.id),
    call: (s) => s.fetchProject!(),
    staleTime: 5 * 60_000,
  });
}
