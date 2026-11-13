import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';

import { PaginationFooter } from './PaginationFooter';

/**
 * The "Load more" footer for cursor-style pagination, where pages accumulate in the
 * list instead of replacing each other. Compare `Pagination`, which is the numbered
 * Previous/Next form for a table that swaps its rows.
 *
 * It renders `null` unless `hasMorePages` is true, so a caller can mount it
 * unconditionally at the end of a list and let it disappear on the last page.
 */
const meta: Meta<typeof PaginationFooter> = {
  component: PaginationFooter,
  args: { hasMorePages: true, onLoadMore: fn() },
  decorators: [(Story) => <div className="w-[360px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof PaginationFooter>;

/** More to fetch. */
export const Default: Story = {
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: 'Load more' }));
    await expect(args.onLoadMore).toHaveBeenCalled();
  },
};

/** In flight: the label changes and the button disables, so a double click can't fire
 *  two fetches for the same cursor. */
export const Loading: Story = {
  args: { isPageLoading: true },
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    const button = canvas.getByRole('button', { name: 'Loading…' });
    await expect(button).toBeDisabled();
    // `disabled:pointer-events-none` means the click is refused by the browser rather
    // than swallowed by the handler, so asserting the style is the real guarantee — a
    // `userEvent.click` here throws instead of being a no-op.
    await expect(button).toHaveStyle({ pointerEvents: 'none' });
    await expect(args.onLoadMore).not.toHaveBeenCalled();
  },
};

/** The last page: nothing renders. */
export const NoMorePages: Story = {
  args: { hasMorePages: false },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByRole('button')).toBeNull();
  },
};

/** A real component rather than an inline `render` closure, because hooks are not
 *  allowed in the latter. Appends a page per click and then removes itself, which is
 *  the whole lifecycle a caller has to wire. */
function LoadMoreList() {
  const [count, setCount] = useState(3);
  const total = 7;

  return (
    <div className="rounded-md border border-borderMuted">
      <ul className="m-0 list-none p-0">
        {Array.from({ length: count }).map((_, i) => (
          <li
            key={i}
            className="border-b border-borderMuted px-3 py-2 text-sm text-fgMain last:border-0"
          >
            stg_model_{i + 1}
          </li>
        ))}
      </ul>
      <PaginationFooter
        hasMorePages={count < total}
        onLoadMore={() => setCount((c) => Math.min(total, c + 2))}
      />
    </div>
  );
}

/** Clicking through to the end, at which point the footer unmounts on its own. */
export const Interactive: Story = {
  render: () => <LoadMoreList />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('button', { name: 'Load more' }));
    await expect(canvas.getByText('stg_model_5')).toBeVisible();

    await userEvent.click(canvas.getByRole('button', { name: 'Load more' }));
    await expect(canvas.getByText('stg_model_7')).toBeVisible();
    // Everything is loaded, so the footer is gone.
    await expect(canvas.queryByRole('button')).toBeNull();
  },
};
