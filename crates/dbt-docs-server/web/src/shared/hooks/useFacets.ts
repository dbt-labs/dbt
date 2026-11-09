import { useMetadataDataSource } from 'dbt-docs-lib';
import type { ResourceType } from 'dbt-docs-lib/domain';

import { facetsKey } from '../util/queryKeys';
import { useSourceQuery } from './useSourceQuery';

/**
 * Fetch the list filter facet options for a resource type from the active
 * {@link MetadataDataSource}. Disabled when the source advertises no
 * `fetchFacets`.
 */
export function useFacets(resourceType: ResourceType) {
  const source = useMetadataDataSource();
  return useSourceQuery({
    method: 'fetchFacets',
    queryKey: facetsKey(source.id, resourceType),
    call: (s) => s.fetchFacets!({ resourceType }),
    staleTime: 5 * 60_000,
  });
}
