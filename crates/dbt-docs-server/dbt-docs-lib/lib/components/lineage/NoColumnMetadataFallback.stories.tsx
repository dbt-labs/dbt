import type { Meta, StoryObj } from '@storybook/react-vite';

import { NoColumnMetadataFallback } from './NoColumnMetadataFallback';

const meta: Meta<typeof NoColumnMetadataFallback> = {
  component: NoColumnMetadataFallback,
  decorators: [
    (Story) => (
      <div className="w-[560px] rounded-lg border border-borderMuted">{Story()}</div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof NoColumnMetadataFallback>;

/**
 * Column metadata needs either an authored `schemas.yml` or a
 * catalog read, and a plain `docs generate` has neither — so this is the *common* path,
 * not an error, and has to read as deliberate.
 */
export const Default: Story = {};

/** Narrow: the explanatory line is a wrapping flex row mixing text and an inline
 *  `Code` element, which is where it would break badly if it did. */
export const Narrow: Story = {
  decorators: [
    (Story) => (
      <div className="w-72 rounded-lg border border-borderMuted">{Story()}</div>
    ),
  ],
};
