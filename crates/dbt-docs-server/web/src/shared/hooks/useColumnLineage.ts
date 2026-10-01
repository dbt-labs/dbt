import { useQuery } from '@tanstack/react-query';

import { useMetadataDataSource } from '../context/MetadataDataProvider';
import type { ColumnLineageArgs } from '../typings/args';
import { columnLineageKey } from '../util/queryKeys';

/**
 * Fetch column-level lineage for an asset via the active
 * {@link MetadataDataSource}. Returns a {@link ColumnLineageResult} so a gated
 * backend stays distinct from an empty graph.
 *
 * Supports lazy loading: pass `{ enabled: false }` and call `refetch()` on
 * first need (e.g. expanding a column row), matching the dbt-docs-v2 Columns
 * tab. Disabled when `args` is null or the source has no `fetchColumnLineage`.
 */
export function useColumnLineage(
  args: ColumnLineageArgs | null,
  opts: { enabled?: boolean } = {},
) {
  const source = useMetadataDataSource();
  const supported = 'fetchColumnLineage' in source;
  const enabled = (opts.enabled ?? true) && args !== null && supported;

  return useQuery({
    queryKey: args
      ? columnLineageKey(source.id, args)
      : [source.id, 'columnLineage', 'none'],
    queryFn: () => source.fetchColumnLineage!(args!),
    enabled,
    staleTime: 30_000,
  });
}
