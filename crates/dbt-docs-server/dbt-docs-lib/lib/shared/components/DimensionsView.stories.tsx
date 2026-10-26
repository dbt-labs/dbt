import type { Meta, StoryObj } from '@storybook/react-vite';

import { storySemanticModel } from '../testing/storyFixtures';
import { DimensionsView } from './DimensionsView';

const meta: Meta<typeof DimensionsView> = {
  component: DimensionsView,
  args: { dimensions: storySemanticModel().dimensions },
};

export default meta;
type Story = StoryObj<typeof DimensionsView>;

/** Both dimension types — `time` and `categorical` — since the type is rendered as
 *  the card's type chip. */
export const Default: Story = {};

export const Undocumented: Story = {
  args: {
    dimensions: [
      { name: 'ordered_at', type: 'time', description: null },
      { name: 'status', type: 'categorical', description: null },
    ],
  },
};

export const NoDimensions: Story = {
  args: { dimensions: [] },
};
