import type { Meta, StoryObj } from '@storybook/react-vite';
import type { CellContext } from '@tanstack/react-table';

import {
  RightAlignedTruncatedCell,
  TimestampCell,
  TruncatedCell,
  TruncatedCopyLinkCell,
} from './TruncatedCell';

/**
 * These are react-table cell renderers, so the only prop that matters is the cell
 * context's `getValue`. Faking just that keeps the stories about the rendering.
 *
 * Typed `never` in the value position so one helper serves every renderer here: they
 * each declare a different `TValue` (`ReactElement`, `string`, a timestamp union) and
 * `never` is assignable to all of them.
 */
function cell(value: unknown): CellContext<object, never> {
  return { getValue: () => value } as unknown as CellContext<object, never>;
}

/** Cells only truncate inside a constrained parent, so every story needs one — a bare
 *  cell would just render its full text and prove nothing. */
const inCell = (node: React.ReactNode) => (
  <div className="w-56 border border-borderMuted p-2">{node}</div>
);

const meta: Meta<typeof TruncatedCell> = {
  component: TruncatedCell,
};

export default meta;
type Story = StoryObj<typeof TruncatedCell>;

/** Short enough to fit: no tooltip, since it only appears once the text is clipped. */
export const Default: Story = {
  render: () => inCell(<TruncatedCell {...cell('customers')} />),
};

/** Clipped, so hovering reveals the full value. */
export const Truncated: Story = {
  render: () =>
    inCell(
      <TruncatedCell {...cell('int_order_items_joined_to_customers_and_products')} />,
    ),
};

/** The right-aligned variant, for numeric columns. Pair with `meta.align="right"` on
 *  the header so the two line up. */
export const RightAligned: Story = {
  render: () => inCell(<RightAlignedTruncatedCell {...cell('128,450')} />),
};

/** `TimestampCell` renders the local date with the UTC time in a tooltip. Unlike
 *  `TruncatedCell` that tooltip always shows on hover — it carries information the
 *  cell itself does not. */
export const Timestamp: Story = {
  render: () => inCell(<TimestampCell {...cell('2026-02-11T16:12:18Z')} />),
};

export const TimestampFromDate: Story = {
  render: () => inCell(<TimestampCell {...cell(new Date('2026-02-11T16:12:18Z'))} />),
};

/**
 * Every non-date a timestamp column can hold — null, empty string, and an
 * unparseable string — renders as an em dash. Models that never ran produce all
 * three, so an exception here would break a whole list page.
 */
export const TimestampEmptyAndInvalid: Story = {
  render: () => (
    <div className="space-y-2">
      {inCell(<TimestampCell {...cell(null)} />)}
      {inCell(<TimestampCell {...cell('')} />)}
      {inCell(<TimestampCell {...cell('not a date')} />)}
    </div>
  ),
};

/** A copy affordance in a cell: clicking copies the full value and toasts. */
export const CopyLink: Story = {
  render: () => inCell(<TruncatedCopyLinkCell {...cell('analytics.dbt.customers')} />),
};

export const CopyLinkTruncated: Story = {
  render: () =>
    inCell(
      <TruncatedCopyLinkCell
        {...cell('analytics_production.dbt_marts_finance.int_order_items_joined')}
      />,
    ),
};

/** A cell value can be an element rather than a string, in which case it is rendered
 *  as-is and used as its own tooltip content. */
export const ElementValue: Story = {
  render: () =>
    inCell(
      <TruncatedCell
        {...cell(<span className="text-fgBrand">customers (public, contracted)</span>)}
      />,
    ),
};
