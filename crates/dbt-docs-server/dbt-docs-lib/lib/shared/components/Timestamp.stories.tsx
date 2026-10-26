import type { Meta, StoryObj } from '@storybook/react-vite';

import { TimestampContainer, TimestampDisplay } from './Timestamp';

const meta: Meta<typeof TimestampDisplay> = {
  component: TimestampDisplay,
  args: {
    timestamp: 'Feb 11, 2026, 4:12 PM',
    timestampUtc: '2026-02-11 16:12:00 UTC',
  },
};

export default meta;
type Story = StoryObj<typeof TimestampDisplay>;

/** The local rendering, with the UTC value behind a tooltip — the split exists so a
 *  reader in any timezone can still resolve what "4:12 PM" meant. */
export const Default: Story = {};

export const WithPrependedText: Story = {
  args: { prependedText: 'Last run ' },
};

/** Renders nothing without a timestamp. Guarded on `timestamp` only, so a missing
 *  local value hides the element even if the UTC one is present. */
export const NoTimestampRendersNothing: Story = {
  args: { timestamp: undefined },
};

/** `TimestampContainer` takes a `Date` and does the formatting itself — the form most
 *  call sites use. Fixed date so the story does not change between runs. */
export const FromDate: Story = {
  render: () => <TimestampContainer date={new Date('2026-02-11T16:12:00Z')} />,
};

/** An absent date is the normal case for a resource that has never been built. */
export const FromUndefinedDate: Story = {
  render: () => <TimestampContainer date={undefined} />,
};
