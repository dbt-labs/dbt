import { createElement } from 'react';
import { Handle, type Node, type NodeProps, Position } from '@xyflow/react';
import { Columns3 } from 'lucide-react';

import { iconForType } from '../../lib/resourceType';
import { isCardCompact, useLineageStore } from '../../stores/lineageStore';
import { Tooltip } from '../ui/Tooltip';
import { lensBadgeFor } from './lensBadges';

/** Registered node type. Nodes must carry `type: DAG_NODE_TYPE` to render as this. */
export const DAG_NODE_TYPE = 'dagNode';

/** Kept in step with NODE_WIDTH / NODE_HEIGHT in `lib/dagreLayout.ts`, which is what
 *  dagre spaces the graph by, and with the `.dag-node` rule that actually sizes it. */
export const DAG_NODE_WIDTH = 245;
export const DAG_NODE_HEIGHT = 108;

export type DagNodeData = {
  /** Display name — the resource's own name, not its unique_id. */
  name: string;
  resourceType: string;
  /** Column count for the column-lineage chip. The chip is hidden when this is
   *  null or undefined, which is the no-column-lineage state. */
  columnCount?: number | null;
  /** Fired when the column-count chip is clicked. Not wired to real navigation
   *  yet -- the intent is to open the node's detail drawer on its Columns tab. */
  onColumnsClick?: () => void;
  id?: string;
} & Record<string, unknown>;

export type DagNodeType = Node<DagNodeData, typeof DAG_NODE_TYPE>;

/** Type labels. Hand-rolled rather than imported so this component owes nothing to
 *  dbt-dag; unknown types fall through to the raw string. Exported for
 *  DagResourceBadge, which reuses this alongside the badge's own CSS/color
 *  scheme so the panel header and collapsed rail read exactly like a node. */
export const TYPE_LABEL: Record<string, string> = {
  analysis: 'Analysis',
  exposure: 'Exposure',
  function: 'Function',
  group: 'Group',
  macro: 'Macro',
  metric: 'Metric',
  model: 'Model',
  saved_query: 'Saved query',
  seed: 'Seed',
  semantic_model: 'Semantic model',
  snapshot: 'Snapshot',
  source: 'Source',
  test: 'Test',
  unit_test: 'Unit test',
};

export function DagNode({
  data,
  selected,
  sourcePosition = Position.Right,
  targetPosition = Position.Left,
}: NodeProps<DagNodeType>) {
  const { name, resourceType, columnCount, onColumnsClick, id } = data;
  const label = TYPE_LABEL[resourceType] ?? resourceType;
  const hasColumnLineage = columnCount != null;
  const rootUniqueId = useLineageStore((s) => s.rootUniqueId);
  const nodeIsRoot = rootUniqueId != null && rootUniqueId == id;
  const isCompact = useLineageStore(isCardCompact);
  const activeLens = useLineageStore((s) => s.activeLens);
  // `data` carries whatever the lineage payload put on it beyond DagNodeData's
  // own typed fields (see the `& Record<string, unknown>` on the type) --
  // these casts are just naming which of those extras this card cares about.
  const lensBadge = lensBadgeFor(activeLens, {
    materialized: data.materialized as string | null | undefined,
    originalFilePath: data.originalFilePath as string | null | undefined,
    lastRunStatus: data.lastRunStatus as string | null | undefined,
    lastTestStatus: data.lastTestStatus as string | null | undefined,
    queryCount: data.queryCount as number | null | undefined,
  });

  // Below the zoom threshold, every unselected node collapses to just its resource
  // badge -- the selected node (if any) keeps the full card so there's still
  // something to actually read detail off of at that zoom level.
  if (isCompact && !selected) {
    return (
      <div className="dag-node dag-node--compact">
        <Handle type="target" position={targetPosition} isConnectable={false} />
        <span className="dag-node__type" data-resource-type={resourceType}>
          {createElement(iconForType(resourceType), {
            className: 'dag-node__type-icon',
            size: 16,
          })}
          <span className="dag-node__type-label">{label}</span>
        </span>
        <Handle type="source" position={sourcePosition} isConnectable={false} />
      </div>
    );
  }

  return (
    <div
      className={`dag-node${selected || nodeIsRoot ? ' dag-node--active' : ''}`}
      data-resource-type={resourceType}
    >
      {/* Handle positions follow the layout direction rather than being pinned
          left/right, so a top-to-bottom graph attaches its edges correctly too. */}
      <Handle type="target" position={targetPosition} isConnectable={false} />

      <div className="dag-node__header">
        {/* Names are long and the header truncates, so the full name has to be
            recoverable — `displayOnlyWhenTruncated` means the tooltip appears only for
            the ones that actually got clipped. Note this mounts one Radix provider per
            node; if the graph grows past a few hundred, hoist a single provider above
            the canvas instead. */}
        <Tooltip
          content={name}
          displayOnlyWhenTruncated
          className="dag-node__name-wrap"
        >
          {(ref) => (
            <span ref={ref} className="dag-node__name">
              {name}
            </span>
          )}
        </Tooltip>
      </div>

      <div className="dag-node__body">
        {lensBadge && (
          <span
            className="dag-node__lens-badge"
            style={{ backgroundColor: lensBadge.color }}
          >
            <lensBadge.icon className="dag-node__lens-badge-icon" size={16} />
            <span className="dag-node__lens-badge-label">{lensBadge.label}</span>
          </span>
        )}

        <Tooltip
          content={label}
          displayOnlyWhenTruncated
          className="dag-node__type-wrap"
        >
          {(ref) => (
            <span className="dag-node__type" data-resource-type={resourceType}>
              {createElement(iconForType(resourceType), {
                className: 'dag-node__type-icon',
                size: 16,
              })}
              <span ref={ref} className="dag-node__type-label">
                {label}
              </span>
            </span>
          )}
        </Tooltip>

        {hasColumnLineage && (
          <Tooltip content={`${columnCount} columns with lineage`}>
            <button
              type="button"
              className="dag-node__columns"
              aria-label={`${columnCount} columns with lineage`}
              onClick={onColumnsClick}
            >
              <span className="dag-node__columns-icon">
                <Columns3 size={16} />
              </span>
              <span className="dag-node__columns-count">{columnCount}</span>
            </button>
          </Tooltip>
        )}
      </div>

      <Handle type="source" position={sourcePosition} isConnectable={false} />
    </div>
  );
}

/** Hoisted so it is one stable object: React Flow re-creates every node when the
 *  `nodeTypes` identity changes. */
export const DAG_NODE_TYPES = { [DAG_NODE_TYPE]: DagNode };
