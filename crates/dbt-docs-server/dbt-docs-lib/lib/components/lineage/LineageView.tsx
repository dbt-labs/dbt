import { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { Maximize2 } from 'lucide-react';

import { useLineageData } from '../../hooks/useLineageData';
import { paths } from '../../routes';
import { Spinner } from '../ui/Spinner';
import { UNSUPPORTED_SURFACE_MESSAGE } from '../ui/UnsupportedSurfaceMessage';
import { NoLineageFallback } from './NoLineageFallback';
import { Button } from './../ui/Button';
import { BaseDag } from './BaseDag';

interface LineageViewProps {
  rootUniqueId: string;
  modelName: string;
}

export function LineageView({ rootUniqueId, modelName }: LineageViewProps) {
  const navigate = useNavigate();
  const { data, error, dagNodes, isSupported } = useLineageData(rootUniqueId, 1);

  if (error) {
    return (
      <div className="err">
        Failed to load lineage: <code className="inline">{error.message}</code>
      </div>
    );
  }
  // Distinct from "no lineage in the data": nothing is loading and nothing is
  // coming, and `NoLineageFallback` would advise rerunning with
  // `--write-lineage`, which would not help.
  if (!isSupported) {
    return (
      <p className="muted" style={{ fontSize: 13 }}>
        {UNSUPPORTED_SURFACE_MESSAGE}
      </p>
    );
  }
  if (!data) {
    return (
      <p className="muted flex items-center gap-2" style={{ fontSize: 13 }}>
        <Spinner /> Loading lineage…
      </p>
    );
  }
  if (dagNodes.length <= 1 && data.edges.length === 0) {
    return <NoLineageFallback modelName={modelName} />;
  }

  return (
    <div className="lineage-frame">
      <div className="absolute inset-0">
        <BaseDag
          rootUniqueId={rootUniqueId}
          topBarLeft={
            <Button
              variant="outline"
              size="sm"
              icon={<Maximize2 className="size-3.5" />}
              text="Expand"
              ariaLabel="Open fullscreen lineage"
              tooltip="Open fullscreen lineage"
              onClick={() => navigate(paths.lineage(rootUniqueId))}
              className="h-9"
            />
          }
          onNodeClick={(uniqueId) => navigate(paths.details(uniqueId))}
        />
      </div>
    </div>
  );
}

export { LineageView as LineageViewV2 };
