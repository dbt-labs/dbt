import type { Meta, StoryObj } from '@storybook/react-vite';

import { CopyCommandSnippet } from './CopyCommandSnippet';

const meta: Meta<typeof CopyCommandSnippet> = {
  component: CopyCommandSnippet,
  args: { command: 'dbt login' },
};

export default meta;
type Story = StoryObj<typeof CopyCommandSnippet>;

/**
 * The CTA for the column-lineage upsell when dbt v2 is installed but not
 * authenticated: rather than a button to somewhere, the thing the user needs is a
 * command. Clicking copy swaps the icon to a checkmark for 1.5s and announces the
 * copy through a live region.
 */
export const Default: Story = {};

export const LongerCommand: Story = {
  args: { command: 'dbt compile --write-index --static-analysis strict' },
};

/** It is `inline-flex shrink-0`, so it does not wrap — worth seeing beside text at a
 *  realistic width. */
export const InlineWithText: Story = {
  render: () => (
    <div className="flex w-[560px] items-center justify-between gap-3">
      <span className="min-w-0 flex-1 truncate text-sm text-fgDecorative">
        Column-level lineage is available in dbt v2
      </span>
      <CopyCommandSnippet command="dbt login" />
    </div>
  ),
};
