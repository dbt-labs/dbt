import { createElement } from 'react';

import { iconForType } from '../../lib/resourceType';
import { TYPE_LABEL } from './DagNode';

/**
 * Same badge as a DAG node's own resource-type pill (reuses its `.dag-node__type*`
 * CSS and the shared `--fgViz*`-based color scheme via `data-resource-type`), for
 * spots outside the canvas that need the identical look -- the panel header (via
 * ResourcePanelHeader's `chip` override) and the collapsed rail. Deliberately not
 * `ResourceChip`: that component's `--bgDag*` palette renders noticeably paler
 * than this one, which is the one confirmed against Jess's reference swatch.
 */
export function DagResourceBadge({
  resourceType,
  showText = true,
}: {
  resourceType: string;
  showText?: boolean;
}) {
  const label = TYPE_LABEL[resourceType] ?? resourceType;
  return (
    <span className="dag-node__type" data-resource-type={resourceType}>
      {createElement(iconForType(resourceType), {
        className: 'dag-node__type-icon',
        size: 16,
      })}
      {showText && <span className="dag-node__type-label">{label}</span>}
    </span>
  );
}
