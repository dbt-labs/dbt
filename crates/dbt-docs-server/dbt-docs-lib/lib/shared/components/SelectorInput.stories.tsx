import type { Meta, StoryObj } from '@storybook/react-vite';

import { SelectorInput } from './SelectorInput';

const meta: Meta<typeof SelectorInput> = {
  component: SelectorInput,
  decorators: [(Story) => <div className="w-[640px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof SelectorInput>;

/** The default placeholder is long — it has to teach selector syntax — so this is
 *  mostly a check that it is not clipped at a realistic width. */
export const Default: Story = {};

/** The trailing info icon opens the node-selection docs in a new tab. */
export const WithoutInfoIcon: Story = {
  args: { endIconEnabled: false },
};

export const WithValue: Story = {
  args: { value: 'tag:daily+', onChange: () => {} },
};

export const CustomPlaceholder: Story = {
  args: { placeholder: 'Filter this lineage graph' },
};

/** Narrow container: the label is visually hidden and the two icons are inset, so
 *  the usable text area shrinks fast. */
export const Narrow: Story = {
  decorators: [(Story) => <div className="w-64">{Story()}</div>],
};
