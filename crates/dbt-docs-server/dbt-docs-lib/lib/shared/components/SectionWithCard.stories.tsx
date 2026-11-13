import type { Meta, StoryObj } from '@storybook/react-vite';

import { Badge } from './Badge';
import { DetailsSection } from './SectionWithCard';

const meta: Meta<typeof DetailsSection> = {
  component: DetailsSection,
  args: {
    heading: 'Columns',
    children: (
      <div className="p-4 text-fgMain">Four columns, three of them documented.</div>
    ),
  },
};

export default meta;
type Story = StoryObj<typeof DetailsSection>;

export const Default: Story = {};

/** `withHeading` sits inline after the title — in the app this is where a count or
 *  a "view all" link goes. */
export const WithHeadingAccessory: Story = {
  args: { withHeading: <Badge className="ml-2">4</Badge> },
};

/** `withCard={false}` drops the surface and border, for sections whose children
 *  bring their own card. */
export const WithoutCard: Story = {
  args: { withCard: false },
};

export const WithoutHeading: Story = {
  args: { heading: undefined },
};

/** The whole section collapses to nothing when it has no children — that is how the
 *  detail page hides sections a resource type does not have, rather than rendering an
 *  empty card. */
export const NoChildrenRendersNothing: Story = {
  args: { children: undefined },
};
