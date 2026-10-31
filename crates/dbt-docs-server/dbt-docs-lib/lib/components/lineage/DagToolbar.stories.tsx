import type { Meta, StoryObj } from '@storybook/react-vite';
import { action } from 'storybook/actions';

import { DagToolbar } from './DagToolbar';

const StoryComponent: typeof DagToolbar = (args) => (
  <div className="h-20">
    <DagToolbar {...args} />
  </div>
);

const meta: Meta<typeof DagToolbar> = {
  component: StoryComponent,
};

export default meta;

type Story = StoryObj<typeof DagToolbar>;

export const Component: Story = {
  args: {
    /**
     * The `toolbarItems` prop can be found on the `<Dag />`
     */
    toolbarItems: [
      {
        label: 'Full Screen',
        tooltip: 'Enter Full Screen',
        action: action('fullScreen'),
      },
      {
        label: 'My Button',
        tooltip: 'My Tooltip',
        isDisabled: true,
      },
      {
        tooltip: 'Reset',
        action: action('reset'),
      },
    ],
  },
};
