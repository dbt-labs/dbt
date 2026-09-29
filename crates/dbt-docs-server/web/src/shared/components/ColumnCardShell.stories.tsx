import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';

import { ColumnCardShell, type ColumnCardShellProps } from './ColumnCardShell';

const meta: Meta<typeof ColumnCardShell> = {
  component: ColumnCardShell,
  args: {
    name: 'customer_id',
    type: 'varchar',
    description: (
      <p className="mt-1 text-sm text-fgDecorative">Surrogate key for the customer.</p>
    ),
  },
  decorators: [(Story) => <ul className="max-w-2xl space-y-2">{Story()}</ul>],
};

export default meta;
type Story = StoryObj<typeof ColumnCardShell>;

export const Default: Story = {};

/** The key icon plus a literal "PK" — belt and braces, since the icon alone is easy
 *  to miss in a long column list. */
export const PrimaryKey: Story = {
  args: { isPrimaryKey: true },
};

/** Constraints render as a `Badges` list. `name ?? type` is the label, so a nameless
 *  constraint still shows its kind. */
export const WithConstraints: Story = {
  args: {
    constraints: [
      { name: 'not_null', type: 'not_null' },
      { name: null, type: 'unique' },
      { name: 'positive_amount', type: 'check' },
    ],
  },
};

/**
 * Expansion is fully controlled — the shell never holds the state, so a story that
 * wants a working caret has to own it. A real component rather than an inline `render`
 * closure, because hooks are not allowed in the latter.
 */
function ControlledCard(props: ColumnCardShellProps) {
  const [expanded, setExpanded] = useState(false);
  return (
    <ColumnCardShell
      {...props}
      expandable
      expanded={expanded}
      onToggleExpanded={() => setExpanded((v) => !v)}
      expandedBody={
        <div className="border-t border-borderMuted px-4 py-3 text-fgAlt">
          Upstream: stg_customers.customer_id
        </div>
      }
    />
  );
}

export const Expandable: Story = {
  render: (args) => (
    <ControlledCard
      {...args}
      toggleTooltip={{ open: 'Hide column lineage', closed: 'Show column lineage' }}
    />
  ),
};

/** The CLL badge is a second, more discoverable trigger for the same toggle. It only
 *  renders when the row is also `expandable`. */
export const WithCllBadge: Story = {
  render: (args) => <ControlledCard {...args} showCllBadge />,
};

/** `bodyRows` and `belowDescription` are the two slots explorer uses for test
 *  statuses and meta highlights. Both sit below the main row, before any expanded
 *  body. */
export const WithBodyRows: Story = {
  args: {
    belowDescription: (
      <div className="px-4 pb-2 text-xs text-fgBrand">Meta contains: pii</div>
    ),
    bodyRows: (
      <div className="border-t border-borderMuted px-4 py-2 text-fgAlt">
        2 tests passing
      </div>
    ),
  },
};

/** Everything at once — the densest a row gets, and the one worth checking for
 *  crowding at a narrow width. */
export const Everything: Story = {
  args: {
    isPrimaryKey: true,
    constraints: [{ name: 'not_null', type: 'not_null' }],
    expandable: true,
    showCllBadge: true,
    belowDescription: (
      <div className="px-4 pb-2 text-xs text-fgBrand">Meta contains: pii</div>
    ),
    bodyRows: (
      <div className="border-t border-borderMuted px-4 py-2 text-fgAlt">
        2 tests passing
      </div>
    ),
  },
};
