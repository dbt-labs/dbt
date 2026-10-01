import {
  createElement,
  type RefObject,
  useLayoutEffect,
  useRef,
  useState,
} from 'react';
import { getSmoothStepPath, type Node as ReactFlowNode, Position } from '@xyflow/react';

import {
  type ResourceGroup,
  useLineageGroupPartitions,
} from '../../hooks/useLineageGroupPartitions';
import { iconForType } from '../../lib/resourceType';
import { useLineageStore } from '../../stores/lineageStore';
import { Badge } from '../ui/Badge';
import type { DagNodeData } from './DagNode';
import { DagResourceBadge } from './DagResourceBadge';

type GraphNode = ReactFlowNode<DagNodeData>;

/** How far a connector runs straight out of a card before it is allowed to turn.
 *  Zero, deliberately. The columns sit about 50px apart, and xyflow's default 20px
 *  reserve at *each* end leaves only ~10px in the middle for the turn itself -- the
 *  corner radius is then clamped to a couple of pixels and what should be a right
 *  angle renders as a curl. The channels below keep every turn clear of both cards
 *  without reserving anything up front. */
const CONNECTOR_OFFSET = 0;

/**
 * Where the `index`-th of `count` connectors makes its vertical run, spread evenly
 * across the gap between the two columns.
 *
 * Left to xyflow, every connector turns at the midpoint of the gap. That is fine for
 * one, but every card in a column shares a right edge and every connector ends on the
 * same point of the root, so a second connector turns at exactly the same x, its
 * vertical run lands on top of the first, and the two read as a single brace rather
 * than two connectors. Giving each its own channel separates them.
 *
 * Exported for tests only.
 */
export function channelX(
  from: number,
  to: number,
  index: number,
  count: number,
): number {
  return from + ((to - from) * (index + 1)) / (count + 1);
}

/** The exact same right-angle-with-rounded-corners path the DAG canvas's own
 *  edges use (they're `type: 'smoothstep'`, see useLineageData.ts) -- xyflow
 *  exports its path math directly, so this is the real thing, not a
 *  hand-rolled lookalike. Horizontal in/out (Right→Left) matches how these
 *  connectors sit relative to the cards either side of them. */
function connectorPath(
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  centerX: number,
): string {
  const [path] = getSmoothStepPath({
    sourceX: x1,
    sourceY: y1,
    sourcePosition: Position.Right,
    targetX: x2,
    targetY: y2,
    targetPosition: Position.Left,
    centerX,
    offset: CONNECTOR_OFFSET,
  });
  return path;
}

type ConnectorPath = { id: string; d: string };

/** Measures the root card and every registered group-card ref (relative to
 *  `containerRef`) and produces one bezier path per upstream-card→root and
 *  root→downstream-card connection. Recomputes on layout changes (a
 *  ResizeObserver on the container) and whenever the groups themselves
 *  change shape -- pass the same memoized upstream/downstream/root values
 *  the caller already has as `deps`, not a new array literal each render. */
function useGroupConnectors(
  containerRef: RefObject<HTMLDivElement | null>,
  rootRef: RefObject<HTMLDivElement | null>,
  groupRefs: RefObject<Map<string, HTMLDivElement>>,
  deps: [ResourceGroup[], ResourceGroup[], GraphNode | undefined],
): ConnectorPath[] {
  const [paths, setPaths] = useState<ConnectorPath[]>([]);

  useLayoutEffect(() => {
    const container = containerRef.current;
    const root = rootRef.current;
    if (!container || !root) return;

    const recompute = () => {
      const containerRect = container.getBoundingClientRect();
      const local = (rect: DOMRect) => ({
        left: rect.left - containerRect.left,
        right: rect.right - containerRect.left,
        centerY: rect.top - containerRect.top + rect.height / 2,
      });
      const rootLocal = local(root.getBoundingClientRect());
      const cards = Array.from(groupRefs.current, ([key, el]) => ({
        key,
        isUpstream: key.startsWith('up-'),
        rect: local(el.getBoundingClientRect()),
      }));

      const next: ConnectorPath[] = [];
      for (const isUpstream of [true, false]) {
        // Top to bottom, so which channel a card gets follows where it sits on screen
        // rather than the order its ref callback happened to fire in.
        const column = cards
          .filter((card) => card.isUpstream === isUpstream)
          .sort((a, b) => a.rect.centerY - b.rect.centerY);
        column.forEach(({ key, rect }, index) => {
          // Upstream runs card→root, downstream root→card; either way the connector
          // leaves a right edge and arrives at a left one, and the gap it crosses is
          // the same for every card in the column.
          const from = isUpstream
            ? { x: rect.right, y: rect.centerY }
            : { x: rootLocal.right, y: rootLocal.centerY };
          const to = isUpstream
            ? { x: rootLocal.left, y: rootLocal.centerY }
            : { x: rect.left, y: rect.centerY };
          next.push({
            id: key,
            d: connectorPath(
              from.x,
              from.y,
              to.x,
              to.y,
              channelX(from.x, to.x, index, column.length),
            ),
          });
        });
      }
      setPaths(next);
    };

    recompute();
    const observer = new ResizeObserver(recompute);
    observer.observe(container);
    return () => observer.disconnect();
  }, deps);

  return paths;
}

interface DagGroupsViewProps {
  rootUniqueId: string;
  /** Bubbles a clicked pill up to the caller, which is expected to make it the
   *  new root (and, per BaseDag, reset hops to 1+/+1) and switch back to the
   *  DAG view. Absent in contexts that don't support recentering (e.g. the
   *  small embedded preview) -- pills still render, they just don't do
   *  anything when clicked. */
  onSelectResource?: (uniqueId: string) => void;
}

export function DagGroupsView({ rootUniqueId, onSelectResource }: DagGroupsViewProps) {
  const nodes = useLineageStore((s) => s.nodes) as GraphNode[];
  const edges = useLineageStore((s) => s.edges);
  const root = nodes.find((n) => n.id === rootUniqueId);
  const { upstream, downstream } = useLineageGroupPartitions(
    rootUniqueId,
    nodes,
    edges,
  );

  const containerRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  // Stable across renders -- populated by each card's ref callback below, read
  // back by useGroupConnectors once they're all mounted. Passed as the ref
  // object itself (not unwrapped here) so nothing reads `.current` during
  // render -- only inside the ref callbacks and the layout effect, neither
  // of which run at render time.
  const groupRefs = useRef(new Map<string, HTMLDivElement>());
  const paths = useGroupConnectors(containerRef, rootRef, groupRefs, [
    upstream,
    downstream,
    root,
  ]);

  if (!root) return null;

  return (
    <div className="dag-groups-view h-full w-full overflow-auto">
      {/* pt-24/pb-24: the hop bar and bottom bar are absolutely positioned
       *  siblings, not part of this scroll flow -- without this, the first
       *  and last rows of groups render underneath them. */}
      <div
        ref={containerRef}
        className="relative mx-auto flex w-fit min-w-full items-start justify-center gap-6 p-6 pb-24 pt-24"
      >
        {/* Behind the cards (first in paint order, no z-index needed since
         *  the cards that follow are opaque anyway) and sized to the content,
         *  not the viewport, so it scrolls together with everything else. */}
        <svg className="pointer-events-none absolute inset-0 h-full w-full overflow-visible">
          {paths.map((path) => (
            <path
              key={path.id}
              d={path.d}
              stroke="#ffffff"
              strokeWidth={2}
              fill="none"
            />
          ))}
        </svg>

        <div className="flex w-[340px] flex-none flex-col gap-4">
          {upstream.map((group) => (
            <DagGroupCard
              key={group.resourceType}
              direction="Upstream"
              group={group}
              onSelectResource={onSelectResource}
              registerRef={(el) => {
                const key = `up-${group.resourceType}`;
                if (el) groupRefs.current.set(key, el);
                else groupRefs.current.delete(key);
              }}
            />
          ))}
        </div>

        <div className="flex-none pt-1" ref={rootRef}>
          <DagGroupsRootCard node={root} />
        </div>

        <div className="flex w-[340px] flex-none flex-col gap-4">
          {downstream.map((group) => (
            <DagGroupCard
              key={group.resourceType}
              direction="Downstream"
              group={group}
              onSelectResource={onSelectResource}
              registerRef={(el) => {
                const key = `down-${group.resourceType}`;
                if (el) groupRefs.current.set(key, el);
                else groupRefs.current.delete(key);
              }}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

function DagGroupsRootCard({ node }: { node: GraphNode }) {
  const { name, resourceType } = node.data;
  return (
    <div className="dag-node w-[220px]" data-resource-type={resourceType}>
      <div className="dag-node__header">
        <span className="dag-node__name">{name}</span>
      </div>
      <div className="dag-node__body">
        <DagResourceBadge resourceType={resourceType} />
      </div>
    </div>
  );
}

function DagGroupCard({
  direction,
  group,
  onSelectResource,
  registerRef,
}: {
  direction: 'Upstream' | 'Downstream';
  group: ResourceGroup;
  onSelectResource?: (uniqueId: string) => void;
  registerRef: (el: HTMLDivElement | null) => void;
}) {
  return (
    <div ref={registerRef} className="rounded-lg border border-borderMuted bg-bgMain">
      <div className="flex items-center gap-2 border-b border-borderMuted px-4 py-3">
        <span className="text-sm font-semibold text-fgMain">{direction}</span>
        <DagResourceBadge resourceType={group.resourceType} />
        <Badge text={String(group.items.length)} variant="secondary" />
      </div>
      <div className="grid grid-cols-2 gap-2 p-3">
        {group.items.map((item) => (
          <button
            key={item.id}
            type="button"
            onClick={() => onSelectResource?.(item.id)}
            disabled={!onSelectResource}
            className="flex items-center gap-2 overflow-hidden rounded-md border border-borderMain px-3 py-2 text-left text-sm text-fgMain hover:bg-bgMainHover disabled:cursor-default disabled:hover:bg-transparent"
          >
            <DagGroupPillIcon resourceType={item.data.resourceType} />
            <span className="truncate">{item.data.name}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function DagGroupPillIcon({ resourceType }: { resourceType: string }) {
  return (
    <span
      data-resource-type={resourceType}
      className="flex-none"
      style={{ color: 'var(--dag-node-type-color)' }}
    >
      {createElement(iconForType(resourceType), { size: 16 })}
    </span>
  );
}
