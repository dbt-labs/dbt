import type { Meta, StoryObj } from '@storybook/react-vite';

import { LineageEmptyState } from './LineageEmptyState';

const meta: Meta<typeof LineageEmptyState> = {
  component: LineageEmptyState,
  args: {
    description: 'Search with a selector above, or pick a starting point.',
  },
  // Absolutely positioned and centred on its nearest positioned ancestor — it is meant
  // to float over the empty lineage canvas, so it needs one to centre in.
  decorators: [
    (Story) => (
      <div className="relative h-96 w-full rounded-lg border border-borderMuted">
        {Story()}
      </div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof LineageEmptyState>;

/** What the full-lineage page shows before a selector is entered. */
export const Default: Story = {};

/** Quick links are pre-baked selectors — the fast path out of the empty state. */
export const WithQuickLinks: Story = {
  args: {
    quickLinks: [
      { selector: 'tag:daily+', label: 'Daily models' },
      { selector: '+exposure:*', label: 'Everything feeding exposures' },
    ],
  },
};

/** `description` is a node, not a string, so a caller can put markup in it. */
export const RichDescription: Story = {
  args: {
    description: (
      <>
        Try <code className="text-fgMain">customers+</code> for everything downstream of
        a model, or press Enter to load the whole project.
      </>
    ),
  },
};

/** The box does not wrap (`whitespace-nowrap`), so long copy widens it rather than
 *  reflowing — worth seeing against a narrow canvas. */
export const LongDescription: Story = {
  args: {
    description:
      'No nodes matched that selector. Check the spelling, or widen it with a + on either side.',
    quickLinks: [{ selector: '+customers+', label: 'Reset to customers' }],
  },
};
