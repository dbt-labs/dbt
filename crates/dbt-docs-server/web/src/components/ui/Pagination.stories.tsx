import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';

import { Pagination, type PaginationProps } from './Pagination';

/**
 * Previous / Next page control for a page-based table.
 *
 * The two numeric props are easy to mix up: `currentPage` is 0-indexed (so page 1 in
 * the label is `0`), and `totalRows` is the index of the last row, from which the
 * component derives the page count as `ceil((totalRows + 1) / rowsPerPage)`. Pass a
 * plain row *count* and the last page can come out one short.
 *
 * `rowsPerPage` defaults to 1, which is almost never what a caller wants — every real
 * use site passes it (see `PaginatedTable`).
 */
const meta: Meta<typeof Pagination> = {
  component: Pagination,
  args: {
    currentPage: 0,
    totalRows: 99,
    rowsPerPage: 25,
    onPageChange: fn(),
  },
};

export default meta;
type Story = StoryObj<typeof Pagination>;

/** First of four pages: Previous is disabled, Next is live. */
export const FirstPage: Story = {
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    await expect(canvas.getByText('Page 1 of 4')).toBeVisible();
    await expect(canvas.getByRole('button', { name: 'Previous' })).toBeDisabled();

    await userEvent.click(canvas.getByRole('button', { name: 'Next' }));
    // It reports the page it wants, and the parent is expected to re-render with it.
    await expect(args.onPageChange).toHaveBeenCalledWith(1);
  },
};

/** Mid-range: both controls live. */
export const MiddlePage: Story = {
  args: { currentPage: 1 },
};

/** Last page: Next is disabled. */
export const LastPage: Story = {
  args: { currentPage: 3 },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText('Page 4 of 4')).toBeVisible();
    await expect(canvas.getByRole('button', { name: 'Next' })).toBeDisabled();
  },
};

/** One page of results renders nothing at all — no empty control bar to lay out. */
export const SinglePage: Story = {
  args: { totalRows: 12, rowsPerPage: 25 },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByRole('button')).toBeNull();
  },
};

/** Many pages: the label carries the count, so there is no pill row to overflow. */
export const ManyPages: Story = {
  args: { totalRows: 5_000, rowsPerPage: 25, currentPage: 42 },
};

/** A real component rather than an inline `render` closure, because hooks are not
 *  allowed in the latter. */
function PaginationHarness(props: PaginationProps) {
  const [page, setPage] = useState(props.currentPage);
  return (
    <div className="flex flex-col items-center gap-3">
      <p className="text-sm text-fgAlt">Showing rows for page index {page}</p>
      <Pagination {...props} currentPage={page} onPageChange={setPage} />
    </div>
  );
}

/** Controlled, the way a table would drive it — paging forward and back. */
export const Interactive: Story = {
  render: (args) => <PaginationHarness {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('button', { name: 'Next' }));
    await userEvent.click(canvas.getByRole('button', { name: 'Next' }));
    await expect(canvas.getByText('Page 3 of 4')).toBeVisible();

    await userEvent.click(canvas.getByRole('button', { name: 'Previous' }));
    await expect(canvas.getByText('Page 2 of 4')).toBeVisible();
  },
};
