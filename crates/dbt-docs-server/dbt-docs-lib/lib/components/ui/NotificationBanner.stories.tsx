import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';

import { Code } from './Code';
import { NotificationBanner } from './NotificationBanner';

/**
 * An inline message above the content it concerns. It carries `role="alert"`, so a
 * banner that appears after an action is announced immediately — which is the reason to
 * use this rather than a styled `<div>`, and the reason not to mount one that is
 * already on screen at first paint if it isn't urgent.
 *
 * Everything comes in as a single `notification` object with an `id`, so callers can
 * hold a list of them in state; the component itself renders exactly one.
 */
const meta: Meta<typeof NotificationBanner> = {
  component: NotificationBanner,
  args: {
    notification: {
      id: 'search-failed',
      type: 'info',
      header: 'Showing cached results while the index reloads.',
    },
  },
  decorators: [(Story) => <div className="w-[520px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof NotificationBanner>;

/** `info` is the default when `type` is omitted. */
export const Info: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // The role is the accessibility contract worth pinning.
    await expect(canvas.getByRole('alert')).toBeVisible();
  },
};

export const Warning: Story = {
  args: {
    notification: {
      id: 'search-bad-request',
      type: 'warning',
      header: 'That query could not be parsed. Showing unfiltered results.',
    },
  },
};

export const Error: Story = {
  args: {
    notification: {
      id: 'search-error',
      type: 'error',
      header: 'Search failed. Check your connection and try again.',
    },
  },
};

/** `header` is a node, so a banner can carry inline code or a link — useful when the
 *  message needs to name a file or a flag. */
export const RichHeader: Story = {
  args: {
    notification: {
      id: 'no-column-lineage',
      type: 'info',
      header: (
        <>
          Column lineage is missing because the last compile did not run with{' '}
          <Code>--static-analysis strict</Code>.
        </>
      ),
    },
  },
};

/** Several conditions at once, stacked — how the search page renders a bad request, a
 *  filter conflict and a failure without deciding which one wins. */
export const Stacked: Story = {
  render: () => (
    <div className="flex flex-col gap-2">
      <NotificationBanner
        notification={{
          id: 'a',
          type: 'warning',
          header: 'Two filters conflict; the narrower one was applied.',
        }}
      />
      <NotificationBanner
        notification={{
          id: 'b',
          type: 'info',
          header: 'Modeling layer only applies to models.',
        }}
      />
      <NotificationBanner
        notification={{ id: 'c', type: 'error', header: 'Search failed.' }}
      />
    </div>
  ),
};

/** Long copy wraps rather than truncating — banners are the one place in the app where
 *  a full sentence of explanation is expected. */
export const LongMessage: Story = {
  args: {
    notification: {
      id: 'long',
      type: 'warning',
      header:
        'This docs site was generated without lineage, so the lineage tab and the ' +
        'column-level lineage view will be empty. Re-run `dbt docs generate` after a ' +
        'compile with `--write-index` to populate them.',
    },
  },
};
