import { useState } from 'react';
import { MiniMap, type Node } from '@xyflow/react';
import { ChevronDown, Scan } from 'lucide-react';

import { cn } from '../../lib/utils';
import { DagGroupsMinimap } from './DagGroupsMinimap';
import { dagResourceColor } from './dagResourceColors';
import { GLASS_PANEL_CLASSES } from './glassPanel';

interface Props {
  /** xyflow's own `<MiniMap>` only means anything against a pan/zoom canvas --
   *  Groups view has no canvas (it's a plain scrollable div), so 'groups'
   *  swaps in DagGroupsMinimap instead of trying to render the real one
   *  against nothing. */
  view: 'groups' | 'dag';
  rootUniqueId: string;
}

export function DagMinimapPanel({ view, rootUniqueId }: Props) {
  const [collapsed, setCollapsed] = useState(false);

  return (
    <div className={cn('w-64 overflow-hidden', GLASS_PANEL_CLASSES)}>
      <div className="flex items-center justify-between px-3 py-2">
        <div className="flex items-center gap-1.5 text-sm text-fgMain">
          <Scan className="size-3.5" />
          Minimap
        </div>
        <button
          type="button"
          onClick={() => setCollapsed((c) => !c)}
          aria-label={collapsed ? 'Expand minimap' : 'Collapse minimap'}
          className="text-fgDecorative hover:text-fgMain"
        >
          <ChevronDown
            className={cn('size-3.5 transition-transform', collapsed && 'rotate-180')}
          />
        </button>
      </div>
      {!collapsed &&
        (view === 'groups' ? (
          <DagGroupsMinimap rootUniqueId={rootUniqueId} />
        ) : (
          <MiniMap
            nodeColor={(node: Node) =>
              dagResourceColor((node.data?.resourceType as string) ?? 'model')
            }
            maskColor="rgba(0, 0, 0, 0.55)"
            bgColor="transparent"
            pannable
            zoomable
            // MiniMap reads style.width/height as raw numbers for its internal
            // viewBox math, not as measured CSS layout -- must stay numeric (px),
            // matching the wrapper's fixed w-64 (256px) width.
            style={{ position: 'static', margin: 0, width: 256, height: 160 }}
          />
        ))}
    </div>
  );
}
