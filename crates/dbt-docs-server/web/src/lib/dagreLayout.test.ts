import type { Edge, Node as ReactFlowNode } from '@xyflow/react';
import { describe, expect, it } from 'vitest';

import { applyDagreLayout, NODE_HEIGHT } from './dagreLayout';

const ROOT = 'model.jaffle_shop.customers';

/** A card of a given measured size. Widths vary because a card is as wide as the name
 *  it renders, which is the whole reason ranks come out ragged. */
function card(id: string, width: number, height = NODE_HEIGHT): ReactFlowNode {
  return {
    id,
    position: { x: 0, y: 0 },
    data: {},
    measured: { width, height },
  };
}

/** Root with three upstream cards of different widths and two downstream. */
function graph(): { nodes: ReactFlowNode[]; edges: Edge[] } {
  return {
    nodes: [
      card('up_wide', 600),
      card('up_mid', 400),
      card('up_narrow', 245),
      card(ROOT, 300),
      card('down_wide', 580),
      card('down_narrow', 245),
    ],
    edges: [
      { id: 'e1', source: 'up_wide', target: ROOT },
      { id: 'e2', source: 'up_mid', target: ROOT },
      { id: 'e3', source: 'up_narrow', target: ROOT },
      { id: 'e4', source: ROOT, target: 'down_wide' },
      { id: 'e5', source: ROOT, target: 'down_narrow' },
    ],
  };
}

function byId(nodes: ReactFlowNode[]) {
  return new Map(
    nodes.map((n) => [
      n.id,
      {
        left: n.position.x,
        right: n.position.x + (n.measured?.width ?? 0),
        top: n.position.y,
        bottom: n.position.y + (n.measured?.height ?? 0),
      },
    ]),
  );
}

describe('applyDagreLayout rank alignment', () => {
  it('flushes upstream ranks right and downstream ranks left', () => {
    const { nodes, edges } = graph();
    const box = byId(applyDagreLayout(nodes, edges, { rankdir: 'LR' }, ROOT));

    // Upstream of the root: the edge facing the root is the right one.
    const upstreamRight = box.get('up_wide')!.right;
    expect(box.get('up_mid')!.right).toBe(upstreamRight);
    expect(box.get('up_narrow')!.right).toBe(upstreamRight);
    // ...and only that edge. Left edges stay ragged, since the cards differ in width.
    expect(box.get('up_narrow')!.left).not.toBe(box.get('up_wide')!.left);

    // Downstream: the left edge is the one facing the root.
    const downstreamLeft = box.get('down_wide')!.left;
    expect(box.get('down_narrow')!.left).toBe(downstreamLeft);
  });

  it('moves cards toward the root without disturbing the rank they sit in', () => {
    const { nodes, edges } = graph();
    const centred = byId(applyDagreLayout(nodes, edges, { rankdir: 'LR' }));
    const flushed = byId(applyDagreLayout(nodes, edges, { rankdir: 'LR' }, ROOT));

    // The widest card in a rank defines the band, so it does not move at all.
    expect(flushed.get('up_wide')!.left).toBe(centred.get('up_wide')!.left);
    expect(flushed.get('down_wide')!.left).toBe(centred.get('down_wide')!.left);

    // Everything else moves toward the root, never away from it and never past the
    // widest card's edge -- so no card leaves the band dagre reserved for its rank.
    for (const id of ['up_mid', 'up_narrow']) {
      expect(flushed.get(id)!.left).toBeGreaterThan(centred.get(id)!.left);
      expect(flushed.get(id)!.right).toBeLessThanOrEqual(flushed.get('up_wide')!.right);
    }
    expect(flushed.get('down_narrow')!.left).toBeLessThan(
      centred.get('down_narrow')!.left,
    );
    expect(flushed.get('down_narrow')!.left).toBeGreaterThanOrEqual(
      flushed.get('down_wide')!.left,
    );

    // Rank separation survives: the whole upstream rank still clears the root, and
    // the root still clears the whole downstream rank.
    for (const id of ['up_wide', 'up_mid', 'up_narrow']) {
      expect(flushed.get(id)!.right).toBeLessThan(flushed.get(ROOT)!.left);
    }
    for (const id of ['down_wide', 'down_narrow']) {
      expect(flushed.get(id)!.left).toBeGreaterThan(flushed.get(ROOT)!.right);
    }

    // The cross axis is dagre's business, not ours.
    for (const id of flushed.keys()) {
      expect(flushed.get(id)!.top).toBe(centred.get(id)!.top);
    }
  });

  it('leaves the root rank centred, and every rank centred with no root', () => {
    const { nodes, edges } = graph();
    const centred = byId(applyDagreLayout(nodes, edges, { rankdir: 'LR' }));
    const flushed = byId(applyDagreLayout(nodes, edges, { rankdir: 'LR' }, ROOT));
    expect(flushed.get(ROOT)).toEqual(centred.get(ROOT));

    // A root that is not in the graph is not a root to align toward.
    expect(
      byId(applyDagreLayout(nodes, edges, { rankdir: 'LR' }, 'model.nope')),
    ).toEqual(centred);
  });

  it('aligns along the rank axis, which is vertical under TB', () => {
    const { edges } = graph();
    // Heights vary here instead of widths -- under TB that is the rank axis.
    const nodes = [
      card('up_wide', 245, 200),
      card('up_mid', 245, 150),
      card('up_narrow', 245, 108),
      card(ROOT, 245, 108),
      card('down_wide', 245, 200),
      card('down_narrow', 245, 108),
    ];
    const box = byId(applyDagreLayout(nodes, edges, { rankdir: 'TB' }, ROOT));

    // Above the root, the bottom edges line up; below it, the top edges do.
    expect(box.get('up_mid')!.bottom).toBe(box.get('up_wide')!.bottom);
    expect(box.get('up_narrow')!.bottom).toBe(box.get('up_wide')!.bottom);
    expect(box.get('down_narrow')!.top).toBe(box.get('down_wide')!.top);
  });
});
