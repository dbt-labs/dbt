import { useCallback, useEffect, useMemo, useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { X } from 'lucide-react';

import { useLineageData } from '../../hooks/useLineageData';
import { inferResourceType } from '../../lib/inferResourceType';
import { decorateOutboundHref } from '../../lib/outboundReferrer';
import { isTelemetryInitialized, trackLineageViewed } from '../../lib/telemetry';
import { paths } from '../../routes';
import { LineageEmptyState, Spinner } from '../../shared';
import { UNSUPPORTED_SURFACE_MESSAGE } from '../../shared/hooks/unsupportedSurface';
import { Button } from './../ui/Button';
import { BaseDag } from './BaseDag';
import { DagNodePanel } from './DagNodePanel';

export default function FullLineagePage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const rootUniqueId = searchParams.get('uniqueId') ?? '';
  const panelId = searchParams.get('panel');
  const navigate = useNavigate();
  const { data, error, isSupported } = useLineageData(rootUniqueId, 1);

  // Analytics: `lineage_viewed` (fullscreen) once the graph resolves for the
  // current root. Re-fires when the root changes (refetch → data null → data).
  useEffect(() => {
    if (!isTelemetryInitialized() || !data || !rootUniqueId) return;
    const rootType =
      data.nodes.find((n) => n.uniqueId === rootUniqueId)?.resourceType ??
      inferResourceType(rootUniqueId);
    trackLineageViewed({
      lineage_type: 'fullscreen',
      resource_type: rootType,
      resource_id: rootUniqueId,
    });
  }, [rootUniqueId, data]);

  const updateParams = useCallback(
    (mut: (p: URLSearchParams) => void) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          mut(next);
          return next;
        },
        { replace: true },
      );
    },
    [setSearchParams],
  );

  // Lives here, not in DagNodePanel, because the canvas's own width (below)
  // has to react to it too.
  const [panelCollapsed, setPanelCollapsed] = useState(false);

  const closePanel = useCallback(() => {
    updateParams((p) => p.delete('panel'));
    setPanelCollapsed(false);
  }, [updateParams]);

  const onClose = useCallback(() => {
    navigate(paths.details(rootUniqueId));
  }, [navigate, rootUniqueId]);

  // Groups pill click -> that resource becomes the new root. BaseDag resets
  // hops to 1+/+1 on its own whenever rootUniqueId changes.
  const onRecenter = useCallback(
    (uniqueId: string) => {
      updateParams((p) => p.set('uniqueId', uniqueId));
    },
    [updateParams],
  );

  // Canvas node click -> reveal its detail drawer. Un-collapses too, so
  // clicking a node while the rail is showing brings back the full panel
  // rather than silently swapping which node the collapsed rail points at.
  const onNodeClick = useCallback(
    (uniqueId: string) => {
      updateParams((p) => p.set('panel', uniqueId));
      setPanelCollapsed(false);
    },
    [updateParams],
  );

  // Breadcrumb stops at project (package) — current resource lives in the
  // panel/details page, not the breadcrumb. unique_id shape:
  // `<resource>.<package>.<...path>.<name>`.
  const rootProject = useMemo(() => {
    if (!rootUniqueId) return null;
    return rootUniqueId.split('.')[1] ?? null;
  }, [rootUniqueId]);

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
                href={decorateOutboundHref(
                  'https://docs.getdbt.com/reference/node-selection/syntax',
                )}
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
              onRecenter={onRecenter}
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
