import { createElement } from 'react';
import type { Node as ReactFlowNode } from '@xyflow/react';

import {
  type ResourceGroup,
  useLineageGroupPartitions,
} from '../../hooks/useLineageGroupPartitions';
import { iconForType, RESOURCE_TYPE_LABEL } from '../../lib/resourceType';
import { useLineageStore } from '../../stores/lineageStore';
import { Tooltip } from '../ui/Tooltip';
import type { DagNodeData } from './DagNode';
import { dagResourceColor } from './dagResourceColors';

type GraphNode = ReactFlowNode<DagNodeData>;

// Every square -- root and group alike -- is this same fixed size. Sizing by
// count (area-proportional) was tried first but read as messy with a mix of
// big and small squares; the count badge already carries the magnitude, so
// the squares themselves don't need to.
const SQUARE_SIZE = 32;

function GroupSquare({ group }: { group: ResourceGroup }) {
  const count = group.items.length;
  const label = RESOURCE_TYPE_LABEL[group.resourceType] ?? group.resourceType;
  return (
    <Tooltip content={`${count} ${label}`}>
      <div
        className="relative flex flex-none items-center justify-center rounded-sm text-bgMain"
        style={{
          width: SQUARE_SIZE,
          height: SQUARE_SIZE,
          backgroundColor: dagResourceColor(group.resourceType),
        }}
      >
        {createElement(iconForType(group.resourceType), { size: 16 })}
        <span className="absolute -right-1.5 -top-1.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-bgMain px-1 text-[10px] font-medium text-fgMain ring-1 ring-borderMain">
          {count}
        </span>
      </div>
    </Tooltip>
  );
}

/** The one node in the middle everything else is relative to -- no count badge
 *  (it's a single resource, not a group) and a ring so it doesn't read as just
 *  another group square. */
function RootSquare({ resourceType, name }: { resourceType: string; name: string }) {
  return (
    <Tooltip content={name}>
      <div
        className="flex flex-none items-center justify-center rounded-sm text-bgMain ring-2 ring-fgMain ring-offset-2 ring-offset-bgMain"
        style={{
          width: SQUARE_SIZE,
          height: SQUARE_SIZE,
          backgroundColor: dagResourceColor(resourceType),
        }}
      >
        {createElement(iconForType(resourceType), { size: 16 })}
      </div>
    </Tooltip>
  );
}

/** One column of squares, stacked and gapped the same as the cards in
 *  DagGroupsView -- `align` pulls the column against the root square (end for
 *  upstream on the left, start for downstream on the right) so the two sides
 *  read as flanking it rather than floating independently. */
function GroupColumn({
  groups,
  align,
}: {
  groups: ResourceGroup[];
  align: 'end' | 'start';
}) {
  return (
    <div
      className="flex w-14 flex-col gap-1.5"
      style={{ alignItems: align === 'end' ? 'flex-end' : 'flex-start' }}
    >
      {groups.map((group) => (
        <GroupSquare key={group.resourceType} group={group} />
      ))}
    </div>
  );
}

/** Groups view has no pan/zoom canvas, so xyflow's own `<MiniMap>` has nothing
 *  to represent there -- this is the substitute. Root resource centered, with
 *  its upstream/downstream groups flanking it left/right the same way the DAG
 *  canvas itself reads (upstream feeds in from the left, downstream flows out
 *  to the right) -- position carries the direction instead of text labels.
 *  No connecting lines to the root: this is a compact composition summary, not
 *  a literal mini graph -- reads the same partitions DagGroupsView renders as
 *  cards, off the same shared hook, so the two never disagree. */
export function DagGroupsMinimap({ rootUniqueId }: { rootUniqueId: string }) {
  const nodes = useLineageStore((s) => s.nodes) as GraphNode[];
  const edges = useLineageStore((s) => s.edges);
  const root = nodes.find((n) => n.id === rootUniqueId);
  const { upstream, downstream } = useLineageGroupPartitions(
    rootUniqueId,
    nodes,
    edges,
  );

  if (!root) return null;

  return (
    <div className="flex items-center justify-center gap-2 p-3">
      <GroupColumn groups={upstream} align="end" />
      <RootSquare resourceType={root.data.resourceType} name={root.data.name} />
      <GroupColumn groups={downstream} align="start" />
    </div>
  );
}
