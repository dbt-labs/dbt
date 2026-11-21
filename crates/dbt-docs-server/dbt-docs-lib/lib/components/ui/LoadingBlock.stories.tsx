import type { Meta, StoryObj } from '@storybook/react-vite';

import { Card } from './Card';
import { LoadingBlock } from './LoadingBlock';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from './Table';

/**
 * A pulsing skeleton rectangle. Width is a number of pixels, or `-1` for "fill the
 * parent" — `undefined` also fills, so `-1` is only needed to say so explicitly in a
 * props object that is being spread.
 */
const meta: Meta<typeof LoadingBlock> = {
  component: LoadingBlock,
  decorators: [(Story) => <div className="w-[360px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof LoadingBlock>;

/** No props: full width, 16px tall — a single line of body text. */
export const Default: Story = {};

/** A fixed pixel width, for a skeleton standing in for something of known length like
 *  a label or a count. */
export const FixedWidth: Story = {
  args: { width: 140 },
};

/** `-1` is the explicit form of "fill the available width". */
export const FillWidth: Story = {
  args: { width: -1 },
};

/** Taller blocks stand in for non-text content — a chart, a code block, a thumbnail. */
export const CustomHeight: Story = {
  args: { height: 96 },
};

/** Staggered widths read as a paragraph rather than a stack of identical bars, which is
 *  the difference between a skeleton that looks intentional and one that looks broken. */
export const ParagraphSkeleton: Story = {
  render: () => (
    <div className="flex flex-col gap-2">
      <LoadingBlock />
      <LoadingBlock />
      <LoadingBlock width={240} />
    </div>
  ),
};

/** Inside a card, standing in for a heading plus body. */
export const InCard: Story = {
  render: () => (
    <Card>
      <div className="flex flex-col gap-3">
        <LoadingBlock width={120} height={20} />
        <LoadingBlock />
        <LoadingBlock width={280} />
      </div>
    </Card>
  ),
};

/** The table loading state: one block per cell, so the header stays readable and the
 *  row height does not jump when real data replaces it. This is what `PaginatedTable`
 *  renders while its first page is in flight. */
export const TableSkeleton: Story = {
  render: () => (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Type</TableHead>
          <TableHead>Rows</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {Array.from({ length: 4 }).map((_, i) => (
          <TableRow key={i}>
            <TableCell>
              <LoadingBlock />
            </TableCell>
            <TableCell>
              <LoadingBlock width={64} />
            </TableCell>
            <TableCell>
              <LoadingBlock width={40} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  ),
};

/** `twMerge` lets `className` replace the shape — a circle for an avatar slot, say. */
export const ClassNameOverride: Story = {
  args: { width: 32, height: 32, className: 'rounded-full' },
};
