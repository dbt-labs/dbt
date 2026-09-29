import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';

import { Pill, type PillData } from './Pill';

/**
 * A removable chip. The remove button only renders when `onClickRemove` is passed, so
 * the same component covers both a static tag and a dismissible filter.
 *
 * Removal is not self-managed: the handler receives the pill's `{ id, value }` and the
 * parent is expected to respond by no longer rendering it. That is what lets the search
 * page keep its filter state in one place — see `FilterChips` in `pages/Search.tsx`.
 */
const meta: Meta<typeof Pill> = {
  component: Pill,
  args: { id: 'resource_type:model', value: 'Models' },
};

export default meta;
type Story = StoryObj<typeof Pill>;

/** No `onClickRemove` — a plain, static chip with no button inside it. */
export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByRole('button')).toBeNull();
  },
};

/** With the remove affordance. The button's accessible name interpolates the value, so
 *  several pills in a row remain individually addressable. */
export const Removable: Story = {
  args: { onClickRemove: fn() },
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('button', { name: 'Remove Models' }));
    // The whole pill comes back, not just the id — callers need the label to report
    // what was dismissed.
    await expect(args.onClickRemove).toHaveBeenCalledWith({
      id: 'resource_type:model',
      value: 'Models',
    });
  },
};

/** Long values do not truncate or wrap inside the pill, so a wide value widens the
 *  row. Worth knowing before putting user-authored text in one. */
export const LongValue: Story = {
  args: {
    value: 'modeling_layer: intermediate_and_marts_only',
    onClickRemove: fn(),
  },
  decorators: [(Story) => <div className="w-[240px]">{Story()}</div>],
};

/** A real component rather than an inline `render` closure, because hooks are not
 *  allowed in the latter. */
function FilterChips() {
  const [pills, setPills] = useState<PillData[]>([
    { id: 'resource_type:model', value: 'Models' },
    { id: 'tag:nightly', value: 'nightly' },
    { id: 'package:jaffle_shop', value: 'jaffle_shop' },
    { id: 'materialized:incremental', value: 'incremental' },
  ]);

  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {pills.map((pill) => (
        <Pill
          key={pill.id}
          {...pill}
          onClickRemove={(removed) =>
            setPills((prev) => prev.filter((p) => p.id !== removed.id))
          }
        />
      ))}
      {pills.length === 0 && (
        <span className="text-xs text-fgDecorative">No filters applied</span>
      )}
    </div>
  );
}

/**
 * The applied-filters row, wired the way the search page wires it: the parent owns the
 * list and drops the pill on removal.
 */
export const FilterRow: Story = {
  render: () => <FilterChips />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('button', { name: 'Remove nightly' }));
    await expect(canvas.queryByText('nightly')).toBeNull();
    // Its neighbours are untouched — removal is keyed on id, not index.
    await expect(canvas.getByText('Models')).toBeVisible();
  },
};
