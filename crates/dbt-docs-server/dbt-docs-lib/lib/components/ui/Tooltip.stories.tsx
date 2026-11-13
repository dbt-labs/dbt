import type { Meta, StoryObj } from '@storybook/react-vite';
import { Info } from 'lucide-react';
import { expect, userEvent, within } from 'storybook/test';

import { Badge } from './Badge';
import { Button } from './Button';
import { Tooltip, type TooltipPlacement } from './Tooltip';

/**
 * Hover/focus tooltip. The bubble is portalled to `<body>`, so a story's assertions
 * have to look outside the canvas for it, and there is a 200ms open delay.
 *
 * The trigger is wrapped in a `<span>` with `asChild`, which means the child must be a
 * single element that can take a ref. `children` also accepts a *function* — it
 * receives a ref callback, which is how `displayOnlyWhenTruncated` measures the element
 * to decide whether a tooltip is warranted at all.
 */
const meta: Meta<typeof Tooltip> = {
  component: Tooltip,
  args: {
    content: 'Materialized as an incremental model',
    children: <Badge text="incremental" />,
  },
  // Tooltips need room around the trigger or they open against the canvas edge.
  decorators: [(Story) => <div className="flex justify-center p-12">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof Tooltip>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.hover(canvas.getByText('incremental'));
    // Portalled out of the canvas, behind a 200ms delay.
    await within(document.body).findByText(
      'Materialized as an incremental model',
      undefined,
      {
        timeout: 3000,
      },
    );
  },
};

/** Keyboard users get it too: the trigger is focusable through the child, and Radix
 *  opens on focus as well as hover. */
export const OpensOnFocus: Story = {
  args: { children: <Button text="Refresh" ariaLabel="Refresh" /> },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.tab();
    await expect(canvas.getByRole('button', { name: 'Refresh' })).toHaveFocus();
    await within(document.body).findByText(
      'Materialized as an incremental model',
      undefined,
      {
        timeout: 3000,
      },
    );
  },
};

const PLACEMENTS: TooltipPlacement[] = ['top', 'right', 'bottom', 'left'];

/** The four sides. `placement` also accepts `-start` / `-end` suffixes, which map onto
 *  Radix's `align` — `top-start` is side `top`, align `start`. */
export const Placements: Story = {
  render: (args) => (
    <div className="grid grid-cols-2 gap-10">
      {PLACEMENTS.map((placement) => (
        <Tooltip key={placement} {...args} placement={placement} content={placement}>
          <Badge text={placement} variant="outline" />
        </Tooltip>
      ))}
    </div>
  ),
};

/** Aligned variants, for a tooltip on an element near the edge of its container. */
export const AlignedPlacements: Story = {
  render: (args) => (
    <div className="flex gap-10">
      <Tooltip {...args} placement="top-start" content="top-start">
        <Badge text="top-start" variant="outline" />
      </Tooltip>
      <Tooltip {...args} placement="top-end" content="top-end">
        <Badge text="top-end" variant="outline" />
      </Tooltip>
    </div>
  ),
};

/** Content is a node, so a tooltip can carry a small block of explanation. It is capped
 *  at `max-w-xs` and wraps. */
export const RichContent: Story = {
  args: {
    content: (
      <div className="flex flex-col gap-1">
        <strong>Freshness</strong>
        <span>Last built 3 hours ago, within the 24h threshold.</span>
      </div>
    ),
    children: <Badge text="fresh" icon={<Info className="size-3" />} />,
  },
};

/**
 * `displayOnlyWhenTruncated` suppresses the tooltip unless the measured element is
 * actually clipped — so a table of short names stays quiet and only the long ones
 * explain themselves. This requires the function form of `children`, to hand the ref to
 * the element that does the truncating.
 */
export const OnlyWhenTruncated: Story = {
  args: { content: 'int_order_items_joined_to_customers_and_products' },
  render: (args) => (
    <div className="flex flex-col gap-4">
      <Tooltip {...args} displayOnlyWhenTruncated>
        {(ref) => (
          <span ref={ref} className="block w-[160px] truncate text-sm text-fgMain">
            int_order_items_joined_to_customers_and_products
          </span>
        )}
      </Tooltip>
      <Tooltip {...args} content="orders" displayOnlyWhenTruncated>
        {(ref) => (
          <span ref={ref} className="block w-[160px] truncate text-sm text-fgMain">
            orders
          </span>
        )}
      </Tooltip>
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = within(document.body);

    // The clipped one explains itself.
    await userEvent.hover(canvas.getByText(/int_order_items_joined/));
    await body.findByText(
      'int_order_items_joined_to_customers_and_products',
      undefined,
      {
        timeout: 3000,
      },
    );

    // The short one stays quiet. Waited out well past the 200ms open delay, so this is
    // a real absence rather than a race.
    await userEvent.unhover(canvas.getByText(/int_order_items_joined/));
    await userEvent.hover(canvas.getByText('orders'));
    await new Promise((resolve) => setTimeout(resolve, 600));
    await expect(body.queryByText('orders', { selector: 'div' })).toBeNull();
  },
};
