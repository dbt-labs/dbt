import type { Meta, StoryObj } from '@storybook/react-vite';

import { AnnotationBadge } from './AnnotationBadge';

const meta: Meta<typeof AnnotationBadge> = {
  component: AnnotationBadge,
};

export default meta;
type Story = StoryObj<typeof AnnotationBadge>;

/** `text` defaults to "Beta" — the reason this component exists. */
export const Default: Story = {};

export const CustomText: Story = {
  args: { text: 'Preview' },
};

/** It is `align-text-top` and sized to sit beside a heading, so the only useful way
 *  to review it is next to real text. */
export const BesideAHeading: Story = {
  render: () => (
    <h2 className="m-0 text-xl text-fgMain">
      Column lineage
      <AnnotationBadge />
    </h2>
  ),
};
