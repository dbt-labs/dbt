import type { Meta, StoryObj } from '@storybook/react-vite';

import { Spinner } from './Spinner';

const meta: Meta<typeof Spinner> = {
  component: Spinner,
};

export default meta;
type Story = StoryObj<typeof Spinner>;

/** The animation is gated on `motion-safe`, so this is a static glyph for anyone
 *  browsing with `prefers-reduced-motion`. */
export const Default: Story = {};
