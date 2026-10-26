import type { Meta, StoryObj } from '@storybook/react-vite';

import { TrustState } from '../util/trustSignals';
import { TrustSignalDescription } from './TrustSignalDescription';

const meta: Meta<typeof TrustSignalDescription> = {
  component: TrustSignalDescription,
  args: {
    trustState: TrustState.Caution,
    messages: [
      { type: TrustState.Caution, importance: 1, text: 'This model has no tests' },
      { type: TrustState.Healthy, importance: 2, text: 'All upstream sources healthy' },
      { type: TrustState.Healthy, importance: 3, text: 'Last run succeeded' },
    ],
  },
  decorators: [
    (Story) => (
      <div className="w-96 rounded-lg border border-borderMuted bg-bgMain p-4">
        {Story()}
      </div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof TrustSignalDescription>;

/** The tooltip body behind a trust-signals badge: an overall state, then the
 *  per-signal lines that produced it. */
export const Caution: Story = {};

export const Healthy: Story = {
  args: {
    trustState: TrustState.Healthy,
    messages: [
      { type: TrustState.Healthy, importance: 1, text: 'All tests passing' },
      { type: TrustState.Healthy, importance: 2, text: 'All upstream sources healthy' },
      { type: TrustState.Healthy, importance: 3, text: 'Has a description' },
    ],
  },
};

export const Degraded: Story = {
  args: {
    trustState: TrustState.Degraded,
    messages: [
      { type: TrustState.Degraded, importance: 1, text: 'One or more tests failed' },
      {
        type: TrustState.Caution,
        importance: 2,
        text: 'This model has no description',
      },
      { type: TrustState.Healthy, importance: 3, text: 'All upstream sources healthy' },
    ],
  },
};

export const Unknown: Story = {
  args: {
    trustState: TrustState.Unknown,
    messages: [
      {
        type: TrustState.Unknown,
        importance: 1,
        text: 'Health could not be determined for this resource',
      },
    ],
  },
};

/**
 * Messages sort by severity first (unknown → degraded → caution → healthy) and by
 * `importance` only within a tier. Passed here deliberately out of order so the sort
 * is what you are looking at.
 */
export const MessagesAreSorted: Story = {
  args: {
    trustState: TrustState.Degraded,
    messages: [
      { type: TrustState.Healthy, importance: 1, text: 'Healthy, importance 1' },
      { type: TrustState.Caution, importance: 9, text: 'Caution, importance 9' },
      { type: TrustState.Degraded, importance: 5, text: 'Degraded, importance 5' },
      { type: TrustState.Caution, importance: 2, text: 'Caution, importance 2' },
      { type: TrustState.Unknown, importance: 7, text: 'Unknown, importance 7' },
    ],
  },
};

/** A message can carry a router link, used to send the reader to the failing tests. */
export const WithLinkedMessage: Story = {
  args: {
    messages: [
      {
        type: TrustState.Caution,
        importance: 1,
        text: 'One or more upstream sources are stale',
        link: { to: '/sources', state: { filter: 'stale' } },
      },
      { type: TrustState.Healthy, importance: 2, text: 'All tests passing' },
    ],
  },
};

export const NoMessages: Story = {
  args: { messages: [] },
};

export const SmallIcons: Story = {
  args: { size: 'sm' },
};
