import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';

import type { DropdownOption } from '../../components/ui/DropdownButton';
import { FilterDropdown } from './FilterDropdown';

/** Shaped the way `lib/facetOptions.ts` builds them: a leading "All" option with an
 *  empty value, then `value (count)` labels. */
const LAYER_OPTIONS: DropdownOption[] = [
  { label: 'All', value: '' },
  { label: 'Staging (80)', value: 'staging' },
  { label: 'Intermediate (32)', value: 'intermediate' },
  { label: 'Marts (30)', value: 'marts' },
];

const meta: Meta<typeof FilterDropdown> = {
  component: FilterDropdown,
  args: {
    name: 'Modeling layer',
    options: LAYER_OPTIONS,
    defaultOption: LAYER_OPTIONS[0] as DropdownOption,
    onChange: () => {},
  },
  // The list pops out of flow; without headroom it is clipped by the story canvas.
  decorators: [(Story) => <div className="h-72 pt-2">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof FilterDropdown>;

/** Unfiltered — the "All" option selected, which is how every list page starts. */
export const Default: Story = {
  args: { onChange: fn() },
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('button', { name: /modeling layer/i }));
    await userEvent.click(await canvas.findByText('Marts (30)'));

    // The whole option is handed back, not just its value — the caller needs the
    // label to keep the trigger in sync.
    await expect(args.onChange).toHaveBeenCalledWith(
      expect.objectContaining({ value: 'marts' }),
    );
  },
};

/** A selection already applied. In the app this is what a `?modeling_layer=Marts`
 *  deep link produces on first paint. */
export const WithSelection: Story = {
  args: { defaultOption: LAYER_OPTIONS[3] as DropdownOption },
};

/** A source that does not honour a filter field disables its control rather than
 *  hiding it, so the set of filters stays stable across resource types. */
export const Disabled: Story = {
  args: { isDisabled: true },
};

/** Facets with no values collapse to just "All" — a project where no model declares
 *  the field. */
export const OnlyAllOption: Story = {
  args: {
    options: [{ label: 'All', value: '' }],
    defaultOption: { label: 'All', value: '' },
  },
};

/** Long values are the norm for owners and packages. */
export const LongLabels: Story = {
  args: {
    name: 'Owner',
    options: [
      { label: 'All', value: '' },
      { label: 'data-platform-and-analytics-engineering (74)', value: 'dpae' },
      { label: 'finance-analytics (38)', value: 'finance-analytics' },
    ],
    defaultOption: { label: 'All', value: '' },
  },
};

/** The row of them a list page actually renders. */
export const FilterBar: Story = {
  render: () => (
    <div className="flex flex-wrap gap-3">
      <FilterDropdown
        name="Modeling layer"
        options={LAYER_OPTIONS}
        defaultOption={LAYER_OPTIONS[0] as DropdownOption}
        onChange={() => {}}
      />
      <FilterDropdown
        name="Owner"
        options={[
          { label: 'All', value: '' },
          { label: 'data-platform (74)', value: 'data-platform' },
        ]}
        defaultOption={{ label: 'All', value: '' }}
        onChange={() => {}}
      />
      <FilterDropdown
        name="Package"
        options={[
          { label: 'All', value: '' },
          { label: 'jaffle_shop (142)', value: 'jaffle_shop' },
          { label: 'dbt_utils (18)', value: 'dbt_utils' },
        ]}
        defaultOption={{ label: 'All', value: '' }}
        onChange={() => {}}
      />
    </div>
  ),
};
