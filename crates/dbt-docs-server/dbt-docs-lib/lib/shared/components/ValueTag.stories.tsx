import type { Meta, StoryObj } from '@storybook/react-vite';
import type { CellContext } from '@tanstack/react-table';

import { ValueCell, ValueTag } from './ValueTag';

const meta: Meta<typeof ValueTag> = {
  component: ValueTag,
  args: { children: 'incremental' },
};

export default meta;
type Story = StoryObj<typeof ValueTag>;

export const Default: Story = {};

/** The negative vertical margin exists so the tag does not grow a table row. Two
 *  tags stacked show whether that still holds. */
export const InRows: Story = {
  render: () => (
    <div className="w-64 divide-y divide-borderMuted">
      <div className="px-3 py-2">
        <ValueTag>table</ValueTag>
      </div>
      <div className="px-3 py-2">
        <ValueTag>view</ValueTag>
      </div>
    </div>
  ),
};

/** `ValueCell` is the react-table adapter: it reads the value out of the cell
 *  context rather than from children. */
export const AsTableCell: Story = {
  render: () => (
    <ValueCell
      {...({ getValue: () => 'materialized_view' } as unknown as CellContext<
        unknown,
        string | undefined
      >)}
    />
  ),
};

/** `getValue` is typed as possibly-undefined, and an unset column renders as an
 *  empty tag rather than as nothing at all. */
export const AsTableCellEmpty: Story = {
  render: () => (
    <ValueCell
      {...({ getValue: () => undefined } as unknown as CellContext<
        unknown,
        string | undefined
      >)}
    />
  ),
};
