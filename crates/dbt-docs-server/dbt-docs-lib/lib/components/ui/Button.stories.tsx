import type { Meta, StoryObj } from '@storybook/react-vite';
import { Copy, Expand, Waypoints, X } from 'lucide-react';
import { expect, fn, userEvent, within } from 'storybook/test';

import { Button } from './Button';

/**
 * The app's button. Two things are worth knowing before reaching for it:
 *
 * - It takes `text` (a node) rather than children, and renders `type="button"` always,
 *   so it never submits a form.
 * - There is no `disabled` prop. The variant classes include `disabled:` styling, but
 *   nothing passes the attribute through — a button that needs to be disabled has to
 *   be a plain `<button>` or an `InvisibleButton` today.
 */
const meta: Meta<typeof Button> = {
  component: Button,
  args: { text: 'Run', onClick: fn() },
};

export default meta;
type Story = StoryObj<typeof Button>;

/** The default variant is `outline` — the app uses it for almost every control, and
 *  reserves the filled `default` variant for the one primary action on a surface. */
export const Default: Story = {
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: 'Run' }));
    await expect(args.onClick).toHaveBeenCalled();
  },
};

export const AllVariants: Story = {
  render: () => (
    <div className="flex flex-wrap items-center gap-2">
      <Button text="default" variant="default" />
      <Button text="outline" variant="outline" />
      <Button text="ghost" variant="ghost" />
    </div>
  ),
};

/** `xs` and `sm` are the text sizes. The `icon-*` sizes swap the horizontal padding for
 *  square padding and are meant for a button with no text at all. */
export const AllSizes: Story = {
  render: () => (
    <div className="flex flex-wrap items-center gap-2">
      <Button text="xs" size="xs" />
      <Button text="sm" size="sm" />
      <Button
        icon={<Copy className="size-3" />}
        size="icon-xs"
        ariaLabel="Copy (icon-xs)"
      />
      <Button
        icon={<Copy className="size-3" />}
        size="icon-sm"
        ariaLabel="Copy (icon-sm)"
      />
      <Button
        icon={<Copy className="size-4" />}
        size="icon-lg"
        ariaLabel="Copy (icon-lg)"
      />
    </div>
  ),
};

/** Icon plus text. The gap comes from the size variant, so it stays consistent across
 *  buttons of the same size — but the icon's own size does not: `icon` is a node, so
 *  the caller picks it (`size-3` alongside `xs`/`sm` text, `size-4` for `icon-lg`). */
export const WithIcon: Story = {
  args: { text: 'View lineage', icon: <Waypoints className="size-3" /> },
};

/**
 * Icon-only. `ariaLabel` is not optional in practice here: with no `text` the button
 * has no accessible name at all, and a screen reader announces "button".
 */
export const IconOnly: Story = {
  args: {
    text: undefined,
    icon: <Expand className="size-3" />,
    size: 'icon-sm',
    ariaLabel: 'Expand',
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole('button', { name: 'Expand' })).toBeInTheDocument();
  },
};

/** Passing `tooltip` wraps the button in `Tooltip` for you — the usual pairing for an
 *  icon-only control, where the label is invisible by definition. The bubble renders in
 *  a portal, so it is outside the story canvas in the DOM. */
export const WithTooltip: Story = {
  args: {
    text: undefined,
    icon: <X className="size-3" />,
    size: 'icon-sm',
    ariaLabel: 'Close full lineage',
    tooltip: 'Close full lineage',
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.hover(canvas.getByRole('button', { name: 'Close full lineage' }));
    // Portalled to <body>, and behind a 200ms open delay.
    await within(document.body).findByText('Close full lineage', undefined, {
      timeout: 3000,
    });
  },
};

/** A toolbar: one filled primary action, the rest outline or ghost. */
export const InToolbar: Story = {
  render: () => (
    <div className="flex items-center gap-2 rounded-md border border-borderMuted p-2">
      <Button text="Refresh" variant="default" size="xs" />
      <Button
        text="Copy selector"
        variant="outline"
        size="xs"
        icon={<Copy className="size-3" />}
      />
      <Button
        icon={<X className="size-3" />}
        variant="ghost"
        size="icon-xs"
        ariaLabel="Dismiss"
        className="ml-auto"
      />
    </div>
  ),
};

/** `text` is a node, not a string, so a button can carry richer content — here a count
 *  styled differently from the label. */
export const RichText: Story = {
  args: {
    text: (
      <>
        Results <span className="text-fgDecorative">142</span>
      </>
    ),
  },
};
