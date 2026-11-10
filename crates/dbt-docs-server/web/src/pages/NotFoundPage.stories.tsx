import type { Meta, StoryObj } from '@storybook/react-vite';

import NotFoundPage from './NotFoundPage';

const meta: Meta<typeof NotFoundPage> = {
  component: NotFoundPage,
};

export default meta;
type Story = StoryObj<typeof NotFoundPage>;

/** The catch-all route. Routing is hash-based, so this is reached by a bad fragment
 *  rather than by the host returning a 404. */
export const Default: Story = {};
