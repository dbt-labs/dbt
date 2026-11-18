import type { Meta, StoryObj } from '@storybook/react-vite';
import { Search, X } from 'lucide-react';
import { expect, fn, userEvent, within } from 'storybook/test';

import { Input } from './Input';

/**
 * A text input with optional leading and trailing icons. It forwards every native
 * input prop, so `value`/`onChange`, `placeholder`, `disabled` and `type` behave
 * normally.
 *
 * Two details to know:
 *
 * - `id` falls back to `name`, and the label's `htmlFor` uses that — so a labelled
 *   input needs one of the two, or the label won't be associated with it.
 * - The label renders *inside* the `flex items-center` row, which places a visible
 *   label to the left of the field rather than above it. For a stacked label, use
 *   `labelIsHidden` plus your own `<label>`, or wrap the pair yourself.
 */
const meta: Meta<typeof Input> = {
  component: Input,
  args: { placeholder: 'Search models, sources, tests…', onChange: fn() },
  decorators: [(Story) => <div className="w-[360px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof Input>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.type(canvas.getByRole('textbox'), 'customers');
    await expect(canvas.getByRole('textbox')).toHaveValue('customers');
  },
};

/** The topbar search: `type="search"` plus a leading magnifying glass. The icon shifts
 *  the input's padding automatically (`pl-8`), so text never sits under it. */
export const SearchWithIcon: Story = {
  args: {
    type: 'search',
    name: 'search',
    startIcon: { icon: <Search className="size-3" /> },
    'aria-label': 'Search jaffle_shop',
  },
};

/** A visible label. Note it lands to the *left* of the field, since both are children
 *  of the same flex row. */
export const WithVisibleLabel: Story = {
  args: { name: 'model-name', label: 'Model' },
};

/** `labelIsHidden` keeps the label in the accessibility tree as `sr-only` — the right
 *  choice when the surrounding UI already makes the field's purpose obvious. */
export const WithHiddenLabel: Story = {
  args: { name: 'search', label: 'Search assets', labelIsHidden: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // Invisible, but still the input's accessible name.
    await expect(canvas.getByLabelText('Search assets')).toBeInTheDocument();
  },
};

/** A trailing icon with an `onClick` renders as a real button — a clear affordance
 *  rather than decoration. Without `onClick` the same slot renders an inert span. */
export const WithClearButton: Story = {
  args: {
    name: 'search',
    defaultValue: 'customers',
    startIcon: { icon: <Search className="size-3" /> },
    endIcon: { icon: <X className="size-3" />, onClick: fn() },
    'aria-label': 'Search',
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // The icon button has no accessible name of its own — worth knowing if this
    // pattern spreads. Query by role and position instead.
    await userEvent.click(canvas.getByRole('button'));
  },
};

/** Both slots filled, which is where the double padding is easiest to check. */
export const WithBothIcons: Story = {
  args: {
    name: 'search',
    startIcon: { icon: <Search className="size-3" /> },
    endIcon: { icon: <X className="size-3" /> },
  },
};

export const Disabled: Story = {
  args: { disabled: true, defaultValue: 'customers' },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole('textbox')).toBeDisabled();
  },
};

/** `inputClassName` targets the field itself, while `className` targets the wrapper —
 *  the distinction matters when adjusting width versus adjusting the control. */
export const CustomWidth: Story = {
  args: { className: 'w-[200px]', inputClassName: 'h-7 text-xs' },
};
