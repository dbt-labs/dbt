import type { Meta, StoryObj } from '@storybook/react-vite';

import { Card } from './Card';
import { Heading } from './Heading';
import { LoadingBlock } from './LoadingBlock';

/**
 * A bordered surface with a shadow. It brings no layout of its own beyond padding, so
 * children own their own spacing; `twMerge` means a `className` can override the
 * padding or the border rather than fighting it.
 */
const meta: Meta<typeof Card> = {
  component: Card,
  args: {
    children: (
      <p className="text-sm text-fgMain">
        Total revenue by customer, refreshed nightly.
      </p>
    ),
  },
  decorators: [(Story) => <div className="w-[420px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof Card>;

/** Default padding (`p-4`). */
export const Default: Story = {};

/** `isCompact` drops to `p-2`, for cards that sit in a dense grid or a sidebar. */
export const Compact: Story = {
  args: { isCompact: true },
};

/** The usual composition: a heading, then body content. The card adds no gap, so the
 *  heading needs its own bottom margin. */
export const WithHeading: Story = {
  args: {
    children: (
      <>
        <Heading size="5" className="mb-2">
          customers
        </Heading>
        <p className="text-sm text-fgAlt">
          One row per customer, with lifetime value and first order date.
        </p>
      </>
    ),
  },
};

/** As a skeleton container while its contents load. */
export const Loading: Story = {
  args: {
    children: (
      <div className="flex flex-col gap-2">
        <LoadingBlock width={140} />
        <LoadingBlock />
        <LoadingBlock width={220} />
      </div>
    ),
  },
};

/** A stat tile — the card is the frame, the child is the whole layout. */
export const StatTile: Story = {
  args: {
    isCompact: true,
    children: (
      <div className="flex flex-col gap-1">
        <span className="text-xs uppercase text-fgDecorative">Models</span>
        <span className="text-2xl font-semibold text-fgMain">142</span>
      </div>
    ),
  },
  decorators: [(Story) => <div className="w-[180px]">{Story()}</div>],
};

/** `twMerge` lets `className` replace the built-in padding and border rather than
 *  stacking a second, conflicting value on top of them. */
export const ClassNameOverride: Story = {
  args: { className: 'border-borderBrand bg-bgNeutralMuted p-6' },
};
