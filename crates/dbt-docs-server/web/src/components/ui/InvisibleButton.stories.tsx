import type { Meta, StoryObj } from '@storybook/react-vite';
import { ChevronRight } from 'lucide-react';
import { expect, fn, userEvent, within } from 'storybook/test';

import { Badge } from './Badge';
import { InvisibleButton } from './InvisibleButton';

/**
 * A `<button>` stripped of every default: no background, no border, no padding, and
 * left-aligned text. Use it when a whole region needs to be clickable but must not look
 * like a button — a table row, a tree row, a card. It exists so those regions are real
 * buttons (focusable, keyboard-activatable, announced as buttons) instead of a `<div>`
 * with an `onClick`.
 *
 * It forwards every native button attribute, so `disabled`, `aria-*`, `type` and the
 * rest work as usual.
 */
const meta: Meta<typeof InvisibleButton> = {
  component: InvisibleButton,
  args: { children: 'customers', onClick: fn() },
};

export default meta;
type Story = StoryObj<typeof InvisibleButton>;

/** Bare. It inherits the surrounding font and colour, which is the point. */
export const Default: Story = {
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: 'customers' }));
    await expect(args.onClick).toHaveBeenCalled();
  },
};

/** Hover and focus styling is the caller's job — `className` is merged after the
 *  reset, so nothing has to be un-set first. */
export const WithHoverStyling: Story = {
  args: {
    className:
      'w-full rounded px-2 py-1 text-sm text-fgMain hover:bg-bgMainHover focus-visible:outline focus-visible:outline-2 focus-visible:outline-bgBrand',
  },
  decorators: [(Story) => <div className="w-[260px]">{Story()}</div>],
};

/** A whole row as one click target, with its own internal layout. This is the shape a
 *  list row takes: icon, label, trailing metadata. */
export const AsListRow: Story = {
  args: {
    className:
      'flex w-full items-center gap-2 rounded px-2 py-1.5 hover:bg-bgMainHover',
    children: (
      <>
        <ChevronRight className="size-3" />
        <span className="text-sm text-fgMain">stg_customers</span>
        <Badge text="view" size="xs" className="ml-auto" />
      </>
    ),
  },
  decorators: [(Story) => <div className="w-[280px]">{Story()}</div>],
};

/** Because the visual affordance is gone, a control whose content is only an icon or
 *  only decoration needs an explicit accessible name. */
export const WithAriaLabel: Story = {
  args: {
    'aria-label': 'Expand customers',
    children: <ChevronRight className="size-3" />,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      canvas.getByRole('button', { name: 'Expand customers' }),
    ).toBeInTheDocument();
  },
};

/** `disabled` passes straight through to the element. Note the reset carries no
 *  disabled styling, so a disabled invisible button looks identical — pair it with a
 *  `className` if the state needs to be visible. */
export const Disabled: Story = {
  args: { disabled: true },
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: 'customers' }));
    await expect(args.onClick).not.toHaveBeenCalled();
  },
};
