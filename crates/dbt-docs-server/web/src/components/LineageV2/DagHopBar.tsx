import type { ReactNode } from 'react';

import { cn } from '../../lib/utils';
import { DagHopControl } from './DagHopControl';
import { GLASS_PANEL_CLASSES } from './glassPanel';

export interface DagHopBarProps {
  /** The current lineage root, e.g. "fishtown_internal_analytics.marts.sales.dim_customers" */
  path: string;
  upstreamHops: number;
  downstreamHops: number;
  onUpstreamChange: (hops: number) => void;
  onDownstreamChange: (hops: number) => void;
  /** Rendered on the far left, inside the same holder -- mirrors DagBottomBar's
   *  Lenses-on-the-left / cluster-on-the-right layout. FullLineagePage passes its
   *  close+breadcrumb here; LineageView's embedded panel has none, so the hop
   *  cluster just sits alone on the right. */
  leftContent?: ReactNode;
}

/** Search on the center path is deliberately out of scope for now -- it's a
 *  plain label, not the Figma states' editable/typeahead field. */
export function DagHopBar({
  path,
  upstreamHops,
  downstreamHops,
  onUpstreamChange,
  onDownstreamChange,
  leftContent,
}: DagHopBarProps) {
  return (
    <div
      className={cn(
        'flex items-center justify-between gap-3 overflow-x-auto px-3 py-2',
        GLASS_PANEL_CLASSES,
      )}
    >
      <div className="flex flex-none items-center gap-2">{leftContent}</div>
      <div className="flex min-w-0 flex-1 items-center justify-end gap-2">
        <DagHopControl
          direction="upstream"
          hops={upstreamHops}
          onChange={onUpstreamChange}
        />
        {/* flex-1 + max-w: the path is the one element that should give up space
         *  first when the bar is squeezed (e.g. on a narrower window with the
         *  detail panel open) -- the hop controls on either side stay full size
         *  since they're the actionable controls, not just a label. */}
        <div className="flex h-9 min-w-0 max-w-[350px] flex-1 items-center justify-center overflow-hidden rounded-md border border-borderMain bg-bgMain px-3 text-sm text-fgMain">
          <span className="truncate">{path}</span>
        </div>
        <DagHopControl
          direction="downstream"
          hops={downstreamHops}
          onChange={onDownstreamChange}
        />
      </div>
    </div>
  );
}
