import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import { Badge } from './Badge';
import { Code } from './Code';
import { Popover } from './Popover';

/**
 * A hover card: a panel that opens on hover or focus and can hold real content, unlike
 * `Tooltip`, which is for a line of text. Built on Radix's hover-card, so it has a
 * deliberate open delay (~700ms) — long enough that passing over the trigger on the way
 * somewhere else doesn't flash the panel.
 *
 * `children` must be a single element (it is the trigger, via `asChild`).
 * `labelledBy` becomes the panel's `aria-label`, so pass something descriptive rather
 * than an id. Content is portalled to `<body>`; `zIndex` is there for the cases where
 * that still lands under something.
 */
const meta: Meta<typeof Popover> = {
  component: Popover,
  args: {
    labelledBy: 'Freshness details',
    content: (
      <div className="flex max-w-xs flex-col gap-1">
        <strong className="text-fgMain">Fresh</strong>
        <span className="text-fgAlt">
          Built 3 hours ago, inside the 24 hour warn threshold.
        </span>
      </div>
    ),
    children: <Badge text="fresh" variant="default" />,
  },
  decorators: [(Story) => <div className="flex justify-center p-12">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof Popover>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.hover(canvas.getByText('fresh'));
    // Portalled to <body>, behind hover-card's ~700ms open delay.
    await within(document.body).findByText(
      /inside the 24 hour warn threshold/,
      undefined,
      {
        timeout: 5000,
      },
    );
  },
};

/** What the trust-signals badge does with it: a definition list explaining how a
 *  status was derived. This is the case `Tooltip` cannot serve. */
export const RichContent: Story = {
  args: {
    labelledBy: 'Trust signals',
    content: (
      <dl className="m-0 grid max-w-xs grid-cols-[auto,1fr] gap-x-3 gap-y-1 text-xs">
        <dt className="text-fgDecorative">Freshness</dt>
        <dd className="m-0 text-fgMain">3h ago</dd>
        <dt className="text-fgDecorative">Tests</dt>
        <dd className="m-0 text-fgMain">12 passed, 0 failed</dd>
        <dt className="text-fgDecorative">Owner</dt>
        <dd className="m-0 text-fgMain">analytics</dd>
      </dl>
    ),
  },
};

/** The panel is interactive — unlike a tooltip, the pointer can travel into it, so it
 *  can hold a link. */
export const WithInteractiveContent: Story = {
  args: {
    labelledBy: 'Column lineage unavailable',
    content: (
      <div className="flex max-w-xs flex-col gap-2 text-xs">
        <span className="text-fgAlt">
          Column lineage needs a compile with <Code>--static-analysis strict</Code>.
        </span>
        <a className="text-fgBrand underline" href="https://docs.getdbt.com">
          How to enable it
        </a>
      </div>
    ),
    children: <Badge text="no column lineage" variant="outline" />,
  },
};

/** Any single element can be the trigger, not just a badge — here a bare text span in a
 *  sentence. */
export const TextTrigger: Story = {
  args: {
    labelledBy: 'Selector explanation',
    content: (
      <span className="text-xs text-fgAlt">
        Three generations of parents and children around the focused node.
      </span>
    ),
    children: (
      <span className="cursor-help underline decoration-dotted">3+customers+3</span>
    ),
  },
};

/** `zIndex` raises the portalled panel above surfaces with their own stacking context
 *  — the lineage canvas and its controls being the reason the prop exists. */
export const RaisedZIndex: Story = {
  args: { zIndex: 100 },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.hover(canvas.getByText('fresh'));
    const panel = await within(document.body).findByLabelText(
      'Freshness details',
      undefined,
      {
        timeout: 5000,
      },
    );
    await expect(panel).toHaveStyle({ zIndex: '100' });
  },
};
