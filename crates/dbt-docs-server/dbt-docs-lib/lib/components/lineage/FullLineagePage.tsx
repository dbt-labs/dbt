import { useCallback, useMemo, useState } from 'react';
import { X } from 'lucide-react';

import { useNavigation } from '../../context/NavigationProvider';
import { useLineageData } from '../../hooks/useLineageData';
import { LineageEmptyState, type TabType } from '../../shared';
import { isProjectLineageRoot } from '../../utils/projectLineage';
import { Spinner } from '../ui/Spinner';
import { UNSUPPORTED_SURFACE_MESSAGE } from '../ui/UnsupportedSurfaceMessage';
import { Button } from './../ui/Button';
import { BaseDag } from './BaseDag';
import { DagNodePanel } from './DagNodePanel';

/**
 * Controlled: the host keeps this state wherever it likes (the docs app keeps it in
 * the URL, so a view with a node selected is shareable) and passes it back in.
 */
export interface FullLineagePageProps {
  /** Resource the graph is rooted on, or `PROJECT_LINEAGE_ROOT` for the whole
   *  project. Empty renders the empty state. */
  rootUniqueId: string;
  /** Resource whose detail panel is open, or null for none. */
  panelId: string | null;
  /** The detail panel's tab; see `DagNodePanel`. */
  tab: string | null;
  onRootChange: (uniqueId: string) => void;
  onPanelChange: (uniqueId: string | null) => void;
  onTabChange: (tab: TabType) => void;
}

export default function FullLineagePage({
  rootUniqueId,
  panelId,
  tab,
  onRootChange,
  onPanelChange,
  onTabChange,
}: FullLineagePageProps) {
  const { navigate, paths } = useNavigation();
  const { data, error, isSupported } = useLineageData(rootUniqueId, 1);

  // Lives here, not in DagNodePanel, because the canvas's own width (below)
  // has to react to it too.
  const [panelCollapsed, setPanelCollapsed] = useState(false);

  const closePanel = useCallback(() => {
    onPanelChange(null);
    setPanelCollapsed(false);
  }, [onPanelChange]);

  const onClose = useCallback(() => {
    navigate(
      isProjectLineageRoot(rootUniqueId) ? paths.home() : paths.details(rootUniqueId),
    );
  }, [navigate, paths, rootUniqueId]);

  // Canvas node click -> reveal its detail drawer. Un-collapses too, so
  // clicking a node while the rail is showing brings back the full panel
  // rather than silently swapping which node the collapsed rail points at.
  const onNodeClick = useCallback(
    (uniqueId: string) => {
      onPanelChange(uniqueId);
      setPanelCollapsed(false);
    },
    [onPanelChange],
  );

  // Breadcrumb stops at project (package) — current resource lives in the
  // panel/details page, not the breadcrumb. unique_id shape:
  // `<resource>.<package>.<...path>.<name>`.
  const rootProject = useMemo(() => {
    if (!rootUniqueId) return null;
    // The whole-project root is a stand-in, so read the package off a real node.
    const id = isProjectLineageRoot(rootUniqueId)
      ? data?.nodes[0]?.uniqueId
      : rootUniqueId;
    return id?.split('.')[1] ?? null;
  }, [rootUniqueId, data]);

  return (
    <div className="relative h-screen w-screen overflow-hidden bg-bgMain">
      {error && (
        <div className="err m-4">
          Failed to load lineage: <code className="inline">{error.message}</code>
        </div>
      )}
      {!isSupported && rootUniqueId && (
        <p className="muted" style={{ fontSize: 13, padding: 16 }}>
          {UNSUPPORTED_SURFACE_MESSAGE}
        </p>
      )}
      {isSupported && !data && !error && rootUniqueId && (
        <p
          className="muted flex items-center gap-2"
          style={{ fontSize: 13, padding: 16 }}
        >
          <Spinner /> Loading lineage…
        </p>
      )}
      {!rootUniqueId && (
        <LineageEmptyState
          description={
            <>
              Search for the lineage you wish to see by
              <br />
              <a
                className="inline-block underline"
                href={'https://docs.getdbt.com/reference/node-selection/syntax'}
                target="_blank"
                rel="noreferrer"
              >
                using selector syntax
              </a>{' '}
              or navigate to a node&apos;s detail page
              <br />
              and open lineage from there.
            </>
          }
        />
      )}
      {data && (
        <>
          <div
            className={`absolute bottom-0 left-0 top-0 transition-[right] duration-300 motion-reduce:duration-0 ${
              panelId ? (panelCollapsed ? 'right-14' : 'right-[450px]') : 'right-0'
            }`}
          >
            <BaseDag
              rootUniqueId={rootUniqueId}
              projectName={rootProject}
              // Groups pill click -> that resource becomes the new root. BaseDag
              // resets hops to 1+/+1 on its own whenever rootUniqueId changes.
              onRecenter={onRootChange}
              onNodeClick={onNodeClick}
              topBarLeft={
                <>
                  <Button
                    variant="outline"
                    size="icon-sm"
                    icon={<X className="size-4" />}
                    ariaLabel="Close full lineage"
                    tooltip="Close full lineage"
                    onClick={onClose}
                    className="h-9 w-9"
                  />
                  {rootProject && (
                    <button
                      type="button"
                      onClick={() => navigate(paths.home())}
                      className="h-9 rounded-md border border-borderMain bg-bgMain px-4 text-sm text-fgMain hover:bg-bgMainHover"
                    >
                      {rootProject}
                    </button>
                  )}
                </>
              }
            />
          </div>
          <DagNodePanel
            uniqueId={panelId}
            tab={tab}
            onTabChange={onTabChange}
            onClose={closePanel}
            collapsed={panelCollapsed}
            onToggleCollapse={() => setPanelCollapsed((v) => !v)}
          />
        </>
      )}
    </div>
  );
}
export { FullLineagePage as FullLineagePageV2 };
