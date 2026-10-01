import { type RefObject } from 'react';
import { Group, RotateCcw, Share2 } from 'lucide-react';

import { cn } from '../../lib/utils';
import { Button } from '../ui/Button';
import { type Segment, SegmentedButton } from '../ui/SegmentedButton';
import { DagLensesDropdown } from './DagLensesDropdown';
import { DagZoomControl } from './DagZoomControl';
import { GLASS_PANEL_CLASSES } from './glassPanel';

const VIEW_SEGMENTS: Segment[] = [
  { label: 'Groups', value: 'groups', startIcon: <Group className="size-3.5" /> },
  { label: 'DAG', value: 'dag', startIcon: <Share2 className="size-3.5" /> },
];

export interface DagBottomBarProps {
  view: 'groups' | 'dag';
  onViewChange: (view: 'groups' | 'dag') => void;
  onRefresh: () => void;
  fullscreenTarget?: RefObject<HTMLElement | null>;
}

export function DagBottomBar({
  view,
  onViewChange,
  onRefresh,
  fullscreenTarget,
}: DagBottomBarProps) {
  return (
    <div
      className={cn(
        'flex items-center justify-between gap-3 overflow-x-auto px-3 py-2',
        GLASS_PANEL_CLASSES,
      )}
    >
      <DagLensesDropdown />

      <div className="flex flex-none items-center gap-3">
        <SegmentedButton
          segments={VIEW_SEGMENTS}
          selectedValue={view}
          onSelect={(value) => onViewChange(value as 'groups' | 'dag')}
          itemClassName="h-9 font-normal"
        />
        <DagZoomControl fullscreenTarget={fullscreenTarget} />
        <div className="flex items-center rounded-md border border-borderMain">
          <Button
            variant="ghost"
            size="icon-sm"
            icon={<RotateCcw className="size-4" />}
            ariaLabel="Reset to default view"
            tooltip="Reset to default view (1+ / +1)"
            onClick={onRefresh}
            className="h-9 w-9"
          />
        </div>
      </div>
    </div>
  );
}
