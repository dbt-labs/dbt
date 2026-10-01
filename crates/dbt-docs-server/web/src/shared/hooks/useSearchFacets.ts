import { useMetadataDataSource } from '../context/MetadataDataProvider';
import { searchFacetsKey } from '../util/queryKeys';
import { useSourceQuery } from './useSourceQuery';

/**
 * Fetch project-wide distinct facet values for the cross-type search/filter
 * surface from the active {@link MetadataDataSource}. Disabled when the source
 * advertises no `fetchSearchFacets`.
 */
export function useSearchFacets() {
  const source = useMetadataDataSource();
  return useSourceQuery({
    method: 'fetchSearchFacets',
    queryKey: searchFacetsKey(source.id),
    call: (s) => s.fetchSearchFacets!(),
    staleTime: 5 * 60_000,
  });
}
