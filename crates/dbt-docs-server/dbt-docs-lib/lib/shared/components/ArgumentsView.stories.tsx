import type { Meta, StoryObj } from '@storybook/react-vite';

import { storyMacro } from '../testing/storyFixtures';
import { ArgumentsView } from './ArgumentsView';

const meta: Meta<typeof ArgumentsView> = {
  component: ArgumentsView,
  args: { macroArguments: storyMacro().arguments },
};

export default meta;
type Story = StoryObj<typeof ArgumentsView>;

export const Default: Story = {};

/** A macro whose arguments are declared but undocumented — common, since `arguments:`
 *  in a `.yml` is optional and often only names are filled in. */
export const Undocumented: Story = {
  args: {
    macroArguments: [
      { name: 'column_name', type: null, description: null },
      { name: 'scale', type: null, description: null },
    ],
  },
};

/** No arguments renders flat copy, not an empty list. */
export const NoArguments: Story = {
  args: { macroArguments: [] },
};

/** `null` is distinct from `[]` at the call site (unknown vs known-empty) but renders
 *  the same — worth pinning so a future change to that has to be deliberate. */
export const NullArguments: Story = {
  args: { macroArguments: null },
};
