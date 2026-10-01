import { useMetadataDataSource } from '../context/MetadataDataProvider';
import { overviewKey } from '../util/queryKeys';
import { useSourceQuery } from './useSourceQuery';

/**
 * Fetch the project's `{% docs __overview__ %}` block. Resolves `null` when the
 * project defines none — the caller renders its own default rather than an
 * empty page. Disabled when the source advertises no `fetchOverview`.
 */
export function useProjectOverview() {
  const source = useMetadataDataSource();
  return useSourceQuery({
    method: 'fetchOverview',
    queryKey: overviewKey(source.id),
    call: (s) => s.fetchOverview!(),
    staleTime: 5 * 60_000,
  });
}
