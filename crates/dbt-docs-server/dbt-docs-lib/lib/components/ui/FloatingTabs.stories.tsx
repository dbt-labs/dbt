import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';

import { FloatingTabs } from './FloatingTabs';

/**
 * A tab *bar*, not a tab panel set: it renders Radix's tab list and nothing else, so the
 * caller owns whatever the selected tab reveals. That is deliberate — the detail pages
 * swap large, independently-loaded sections rather than pre-rendering every panel.
 *
 * Controlled, via `value` / `onValueChange`. Tabs are declared as `FloatingTabs.Tab`
 * children (also exported as `FloatingTab`), each with an `id` that `value` is compared
 * against.
 *
 * Because there are no `Tabs.Content` elements, each trigger's `aria-controls` points at
 * a panel that does not exist. Worth knowing if the accessibility panel is consulted.
 */
const meta: Meta<typeof FloatingTabs> = {
  component: FloatingTabs,
  args: {
    value: 'details',
    onValueChange: fn(),
    children: (
      <>
        <FloatingTabs.Tab id="details" text="Details" />
        <FloatingTabs.Tab id="columns" text="Columns" />
        <FloatingTabs.Tab id="lineage" text="Lineage" />
        <FloatingTabs.Tab id="code" text="Code" />
      </>
    ),
  },
  decorators: [(Story) => <div className="w-[520px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof FloatingTabs>;

export const Default: Story = {
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    await expect(canvas.getByRole('tab', { name: 'Details' })).toHaveAttribute(
      'aria-selected',
      'true',
    );

    await userEvent.click(canvas.getByRole('tab', { name: 'Columns' }));
    await expect(args.onValueChange).toHaveBeenCalledWith('columns');
  },
};

/** `count` renders a pill after the label — for a tab whose contents are countable, so
 *  the number is visible without opening it. `0` renders too; only `undefined` hides
 *  the pill. */
export const WithCounts: Story = {
  args: {
    children: (
      <>
        <FloatingTabs.Tab id="columns" text="Columns" count={14} />
        <FloatingTabs.Tab id="tests" text="Tests" count={0} />
        <FloatingTabs.Tab id="children" text="Children" count={132} />
        <FloatingTabs.Tab id="details" text="Details" />
      </>
    ),
    value: 'columns',
  },
};

/** `children` on a tab is an alternative to `text`, for a label that needs its own
 *  markup. `text` wins when both are passed. */
export const RichTabLabel: Story = {
  args: {
    value: 'sql',
    children: (
      <>
        <FloatingTabs.Tab id="sql">
          <span className="font-mono">compiled.sql</span>
        </FloatingTabs.Tab>
        <FloatingTabs.Tab id="yml">
          <span className="font-mono">schema.yml</span>
        </FloatingTabs.Tab>
      </>
    ),
  },
};

/** Labels never wrap, so a wide set overflows its container rather than stacking. Worth
 *  checking against the narrowest pane a tab bar lives in. */
export const ManyTabs: Story = {
  args: {
    children: (
      <>
        <FloatingTabs.Tab id="details" text="Details" />
        <FloatingTabs.Tab id="columns" text="Columns" count={14} />
        <FloatingTabs.Tab id="lineage" text="Lineage" />
        <FloatingTabs.Tab id="code" text="Code" />
        <FloatingTabs.Tab id="tests" text="Tests" count={12} />
        <FloatingTabs.Tab id="freshness" text="Freshness" />
        <FloatingTabs.Tab id="deps" text="Dependencies" count={132} />
      </>
    ),
    value: 'details',
  },
  decorators: [(Story) => <div className="w-[320px] overflow-x-auto">{Story()}</div>],
};

/** A real component rather than an inline `render` closure, because hooks are not
 *  allowed in the latter. Shows the missing half — the bar plus the caller's own panel. */
function TabbedPanel() {
  const [tab, setTab] = useState('details');
  return (
    <div>
      <FloatingTabs value={tab} onValueChange={setTab}>
        <FloatingTabs.Tab id="details" text="Details" />
        <FloatingTabs.Tab id="columns" text="Columns" count={14} />
        <FloatingTabs.Tab id="lineage" text="Lineage" />
      </FloatingTabs>
      <div className="p-4 text-sm text-fgMain">
        {tab === 'details' && <p>One row per customer, refreshed nightly.</p>}
        {tab === 'columns' && <p>14 columns, 3 with descriptions.</p>}
        {tab === 'lineage' && <p>4 parents, 2 children.</p>}
      </div>
    </div>
  );
}

/** Wired to its own content, the way a detail page uses it. */
export const Interactive: Story = {
  render: () => <TabbedPanel />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('tab', { name: /Columns/ }));
    await expect(canvas.getByText('14 columns, 3 with descriptions.')).toBeVisible();

    // Arrow keys move between tabs — Radix's roving tabindex, free with the primitive.
    await userEvent.keyboard('{ArrowRight}');
    await expect(canvas.getByText('4 parents, 2 children.')).toBeVisible();
  },
};
