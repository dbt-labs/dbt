import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, waitFor, within } from 'storybook/test';

import { DropdownButton, type DropdownOption } from './DropdownButton';

/**
 * A single-select dropdown built on Radix's select. The list is portalled to `<body>`,
 * so tests and any parent with `overflow: hidden` see it outside their own subtree.
 *
 * Selection is *not* controlled: `defaultOption` seeds it and `onChange` reports every
 * change, so the parent holds the value it cares about but cannot push a new one in.
 * Note also that the trigger renders no placeholder — without `defaultOption` it starts
 * visually empty, so pass one (the app's filters use a leading "All" option for this).
 *
 * `value` may be a number; it is stringified for the DOM and matched back to the
 * original option before `onChange` fires, so callers get their own object back.
 */
const LAYERS: DropdownOption[] = [
  { label: 'All', value: '' },
  { label: 'Staging', value: 'staging' },
  { label: 'Intermediate', value: 'intermediate' },
  { label: 'Marts', value: 'marts' },
];

const meta: Meta<typeof DropdownButton> = {
  component: DropdownButton,
  args: {
    options: LAYERS,
    defaultOption: LAYERS[0],
    onChange: fn(),
  },
  // The list pops out of flow; without headroom it is clipped by the story canvas.
  decorators: [(Story) => <div className="h-64">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof DropdownButton>;

/**
 * Open the menu and hand back the portalled listbox.
 *
 * The click is retried until the trigger reports itself expanded. Vitest's browser mode
 * clicks by real coordinates, so a click fired while Storybook still has its
 * story-preparing overlay up lands on the overlay instead of the trigger and is simply
 * lost — rare alone, reproducible when this file runs after a slow one.
 */
async function openMenu(trigger: HTMLElement) {
  await waitFor(
    async () => {
      await userEvent.click(trigger);
      await expect(trigger).toHaveAttribute('aria-expanded', 'true');
    },
    { timeout: 5000 },
  );
  // Radix portals the list to <body>, outside the story canvas.
  return within(document.body);
}

/** Open it, pick something, and the whole option comes back — not just the value,
 *  since the caller needs the label to keep its own trigger text in sync. */
export const Default: Story = {
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    const menu = await openMenu(canvas.getByRole('combobox'));
    await userEvent.click(await menu.findByRole('option', { name: 'Marts' }));

    await expect(args.onChange).toHaveBeenCalledWith({
      label: 'Marts',
      value: 'marts',
    });
  },
};

/** `name` prefixes the trigger with a dimmed label, so a row of filters reads
 *  "Layer: Marts" without a separate `<label>` element. */
export const WithName: Story = {
  args: { name: 'Layer', defaultOption: LAYERS[3] },
};

/** A selection already applied — what a deep-linked filter looks like on first paint. */
export const WithSelection: Story = {
  args: { defaultOption: LAYERS[1] },
};

/** Disabled. The app disables a filter whose dimension the current data source cannot
 *  answer, rather than hiding it, so the filter row stays stable. */
export const Disabled: Story = {
  args: { isDisabled: true, name: 'Layer' },
};

/** Numeric values, stringified for the DOM and mapped back on the way out. */
export const NumericValues: Story = {
  args: {
    name: 'Page size',
    options: [
      { label: '25', value: 25 },
      { label: '50', value: 50 },
      { label: '100', value: 100 },
    ],
    defaultOption: { label: '25', value: 25 },
  },
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('combobox'));
    await userEvent.click(
      await within(document.body).findByRole('option', { name: '100' }),
    );

    // A number, not '100' — the option object survives the round trip.
    await expect(args.onChange).toHaveBeenCalledWith({ label: '100', value: 100 });
  },
};

/** A long list. The viewport scrolls rather than growing past the window. */
export const ManyOptions: Story = {
  args: {
    name: 'Package',
    options: Array.from({ length: 20 }, (_, i) => ({
      label: `package_${i + 1}`,
      value: `package_${i + 1}`,
    })),
    defaultOption: { label: 'package_1', value: 'package_1' },
  },
};

/** Two of them side by side, which is how the search page renders its filter row. */
export const FilterRow: Story = {
  render: (args) => (
    <div className="flex items-center gap-2">
      <DropdownButton {...args} name="Layer" />
      <DropdownButton
        {...args}
        name="Materialization"
        options={[
          { label: 'All', value: '' },
          { label: 'table', value: 'table' },
          { label: 'view', value: 'view' },
          { label: 'incremental', value: 'incremental' },
        ]}
        defaultOption={{ label: 'All', value: '' }}
      />
    </div>
  ),
};
