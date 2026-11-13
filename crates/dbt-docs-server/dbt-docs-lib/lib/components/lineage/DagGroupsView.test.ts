import { describe, expect, it } from 'vitest';

import { channelX } from './DagGroupsView';

// The gap the connectors cross: the right edge of a group-card column and the left
// edge of the root card, roughly what the real layout produces.
const FROM = 712;
const TO = 763;

describe('channelX', () => {
  it('puts a lone connector where xyflow would have', () => {
    // One connector cannot overlap anything, so the midpoint of the gap -- xyflow's
    // own default -- is still the right answer.
    expect(channelX(FROM, TO, 0, 1)).toBe((FROM + TO) / 2);
  });

  it('gives every connector in a column its own vertical channel', () => {
    // Cards in a column share a right edge and all target the same point on the
    // root, so a shared channel would draw their vertical runs on top of each other
    // and several connectors would read as one brace.
    const channels = [0, 1, 2].map((i) => channelX(FROM, TO, i, 3));
    expect(new Set(channels).size).toBe(3);
    // Top-to-bottom order in, left-to-right channels out.
    expect(channels).toEqual([...channels].sort((a, b) => a - b));
  });

  it('keeps every turn clear of the cards on both sides', () => {
    // A channel on either edge would put the corner flush against a card.
    for (const count of [1, 2, 3, 5]) {
      for (let i = 0; i < count; i++) {
        const x = channelX(FROM, TO, i, count);
        expect(x).toBeGreaterThan(FROM);
        expect(x).toBeLessThan(TO);
      }
    }
  });

  it('works right-to-left too, for the downstream column', () => {
    // Downstream connectors run root→card, so the gap is traversed the same way but
    // starts at the root's right edge.
    const channels = [0, 1].map((i) => channelX(TO, TO + 51, i, 2));
    expect(channels.every((x) => x > TO && x < TO + 51)).toBe(true);
    expect(new Set(channels).size).toBe(2);
  });
});
