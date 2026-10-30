import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { Moon, Sun, SunMoon } from 'lucide-react';
import { expect, fn, userEvent, within } from 'storybook/test';

import { SegmentedButton } from './SegmentedButton';

/**
 * A one-of-N switch for a small, stable set of views — the locate pane's
 * Assets/Files/Filter switcher and the theme picker. For more than about four options,
 * or for options that change at runtime, use `DropdownButton` instead.
 *
 * It is controlled: `selectedValue` in, `onSelect` out. Clicking the already-selected
 * segment is swallowed (Radix reports an empty value for a deselect, and the component
 * drops it), so the control can never end up with nothing selected.
 */
const meta: Meta<typeof SegmentedButton> = {
  component: SegmentedButton,
  args: {
    segments: [
      { label: 'Assets', value: 'assets' },
      { label: 'Files', value: 'files' },
      { label: 'Filter', value: 'filter' },
    ],
    selectedValue: 'assets',
    onSelect: fn(),
  },
};

export default meta;
type Story = StoryObj<typeof SegmentedButton>;

export const Default: Story = {
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByText('Files'));
    await expect(args.onSelect).toHaveBeenCalledWith('files');
  },
};

/** Clicking the current segment reports nothing, so a parent that mirrors `onSelect`
 *  into state can't be talked into an empty selection. */
export const ReselectIsIgnored: Story = {
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByText('Assets'));
    await expect(args.onSelect).not.toHaveBeenCalled();
  },
};

/** With icons. `startIcon` / `endIcon` are nodes now, so the caller sizes them —
 *  `size-3` is what keeps segments at a consistent height. */
export const WithIcons: Story = {
  args: {
    segments: [
      { label: 'Light', value: 'light', startIcon: <Sun className="size-3" /> },
      { label: 'Dark', value: 'dark', startIcon: <Moon className="size-3" /> },
      { label: 'System', value: 'system', startIcon: <SunMoon className="size-3" /> },
    ],
    selectedValue: 'dark',
  },
};

/** `variant="stretch"` makes the control fill its container and share the width evenly
 *  between segments — for a switcher pinned to the top of a sidebar. */
export const Stretch: Story = {
  args: { variant: 'stretch' },
  decorators: [(Story) => <div className="w-[300px]">{Story()}</div>],
};

/** The two sizes. `sm` is the default; `md` is for a standalone control rather than one
 *  packed into a toolbar. */
export const Sizes: Story = {
  render: (args) => (
    <div className="flex flex-col items-start gap-3">
      <SegmentedButton {...args} size="sm" />
      <SegmentedButton {...args} size="md" />
    </div>
  ),
};

/** Nothing selected. Legal, and how a control looks before its state resolves — but not
 *  reachable by clicking, per `ReselectIsIgnored`. */
export const NoSelection: Story = {
  args: { selectedValue: undefined },
};

/** A real component rather than an inline `render` closure, because hooks are not
 *  allowed in the latter. */
function ThemeSwitcher() {
  const [theme, setTheme] = useState('system');
  return (
    <div className="flex flex-col items-start gap-3">
      <SegmentedButton
        segments={[
          { label: 'Light', value: 'light', startIcon: <Sun className="size-3" /> },
          { label: 'Dark', value: 'dark', startIcon: <Moon className="size-3" /> },
          {
            label: 'System',
            value: 'system',
            startIcon: <SunMoon className="size-3" />,
          },
        ]}
        selectedValue={theme}
        onSelect={setTheme}
      />
      <p className="text-sm text-fgAlt">Selected: {theme}</p>
    </div>
  );
}

/** Wired up, as the locate pane's theme row wires it. */
export const Interactive: Story = {
  render: () => <ThemeSwitcher />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByText('Light'));
    await expect(canvas.getByText('Selected: light')).toBeVisible();

    await userEvent.click(canvas.getByText('Dark'));
    await expect(canvas.getByText('Selected: dark')).toBeVisible();
  },
};
