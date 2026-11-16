import { useCallback } from 'react';
import { useSearchParams } from 'react-router-dom';
import { FullLineagePageV2, PROJECT_LINEAGE_ROOT } from 'dbt-docs-lib';

/**
 * The full-screen lineage route. Its state lives in the search params, so a view with
 * a node selected is shareable: `uniqueId` is the graph's root (or `scope=project`
 * for the whole project, which has none), `panel` the resource whose detail panel is
 * open, and `tab` that panel's tab.
 */
export default function Lineage() {
  const [searchParams, setSearchParams] = useSearchParams();

  // One update for several keys: react-router hands each functional update the
  // params of the last render, so two calls in a row would drop the first.
  const setParams = useCallback(
    (changes: Record<string, string | null>) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          for (const [key, value] of Object.entries(changes)) {
            if (value === null) next.delete(key);
            else next.set(key, value);
          }
          return next;
        },
        { replace: true },
      );
    },
    [setSearchParams],
  );

  return (
    <FullLineagePageV2
      rootUniqueId={
        searchParams.get('scope') === 'project'
          ? PROJECT_LINEAGE_ROOT
          : (searchParams.get('uniqueId') ?? '')
      }
      panelId={searchParams.get('panel')}
      tab={searchParams.get('tab')}
      // Recentering on a node leaves the whole-project view.
      onRootChange={(uniqueId) => setParams({ scope: null, uniqueId })}
      onPanelChange={(uniqueId) => setParams({ panel: uniqueId })}
      onTabChange={(tab) => setParams({ tab })}
    />
  );
}
