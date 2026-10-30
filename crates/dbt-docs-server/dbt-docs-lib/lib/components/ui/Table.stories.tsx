import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';

import { Badge } from './Badge';
import { LoadingBlock } from './LoadingBlock';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from './Table';

/**
 * Thin styled wrappers over the native table elements — one per tag, composed by the
 * caller. They add borders, padding and a hover row highlight, and `Table` itself wraps
 * the `<table>` in a horizontally scrolling div, so a wide table scrolls inside its
 * column instead of stretching the page.
 *
 * There is no sorting, selection or virtualization here; `PaginatedTable` in
 * `shared/components` layers TanStack Table on top of these for that.
 *
 * Every cell is `whitespace-nowrap`, which is what keeps rows aligned — long values
 * widen the table and scroll rather than wrapping.
 */
const meta: Meta<typeof Table> = { component: Table };

export default meta;
type Story = StoryObj<typeof Table>;

const MODELS = [
  { name: 'customers', materialization: 'table', rows: '1,204' },
  { name: 'orders', materialization: 'incremental', rows: '48,102' },
  { name: 'stg_customers', materialization: 'view', rows: '1,204' },
];

/** The standard composition: header row of `TableHead`, body rows of `TableCell`. */
export const Default: Story = {
  render: () => (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Model</TableHead>
          <TableHead>Materialization</TableHead>
          <TableHead>Rows</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {MODELS.map((model) => (
          <TableRow key={model.name}>
            <TableCell className="font-medium text-fgMain">{model.name}</TableCell>
            <TableCell>
              <Badge text={model.materialization} size="xs" />
            </TableCell>
            <TableCell className="text-fgAlt">{model.rows}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // Real table semantics come free with the native elements — that's the point of
    // these wrappers over a grid of divs.
    await expect(canvas.getByRole('table')).toBeInTheDocument();
    await expect(canvas.getAllByRole('row')).toHaveLength(4);
    await expect(
      canvas.getByRole('columnheader', { name: 'Materialization' }),
    ).toBeInTheDocument();
  },
};

/** Numeric columns read better right-aligned, which is a `className` on the head and
 *  cell rather than a prop. */
export const NumericAlignment: Story = {
  render: () => (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Model</TableHead>
          <TableHead className="text-right">Rows</TableHead>
          <TableHead className="text-right">Bytes</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow>
          <TableCell>customers</TableCell>
          <TableCell className="text-right tabular-nums">1,204</TableCell>
          <TableCell className="text-right tabular-nums">184 kB</TableCell>
        </TableRow>
        <TableRow>
          <TableCell>orders</TableCell>
          <TableCell className="text-right tabular-nums">48,102</TableCell>
          <TableCell className="text-right tabular-nums">7.2 MB</TableCell>
        </TableRow>
      </TableBody>
    </Table>
  ),
};

/** The loading state: the header stays, cells hold skeletons. Keeping the real row
 *  count means the table does not resize when data lands. */
export const Loading: Story = {
  render: () => (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Model</TableHead>
          <TableHead>Materialization</TableHead>
          <TableHead>Rows</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {Array.from({ length: 3 }).map((_, i) => (
          <TableRow key={i}>
            <TableCell>
              <LoadingBlock />
            </TableCell>
            <TableCell>
              <LoadingBlock width={72} />
            </TableCell>
            <TableCell>
              <LoadingBlock width={48} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  ),
};

/** Empty: the wrappers render no empty state of their own, so a caller has to supply
 *  one — a full-width cell is the simplest form. */
export const Empty: Story = {
  render: () => (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Model</TableHead>
          <TableHead>Materialization</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow>
          <TableCell colSpan={2} className="py-6 text-center text-fgDecorative">
            No models match these filters.
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  ),
};

/** More columns than fit: the wrapper scrolls horizontally rather than squeezing
 *  columns or wrapping cell text. */
export const HorizontalScroll: Story = {
  decorators: [(Story) => <div className="w-[420px]">{Story()}</div>],
  render: () => (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Unique ID</TableHead>
          <TableHead>Materialization</TableHead>
          <TableHead>Owner</TableHead>
          <TableHead>Last built</TableHead>
          <TableHead>Rows</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow>
          <TableCell>model.jaffle_shop.int_order_items_joined</TableCell>
          <TableCell>incremental</TableCell>
          <TableCell>analytics-engineering</TableCell>
          <TableCell>2026-08-24 03:12 UTC</TableCell>
          <TableCell>48,102</TableCell>
        </TableRow>
      </TableBody>
    </Table>
  ),
};
