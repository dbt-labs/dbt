import type { Meta, StoryObj } from '@storybook/react-vite';

import { Badge, Badges } from './Badge';

const meta: Meta<typeof Badge> = {
  component: Badge,
  args: { children: 'daily' },
};

export default meta;
type Story = StoryObj<typeof Badge>;

export const Default: Story = {};

/** Tags are the usual content, and they are free text — long ones do not wrap
 *  (`whitespace-nowrap`), so this is what a verbose tag actually does to a row. */
export const LongLabel: Story = {
  args: { children: 'contains_pii_and_must_not_leave_the_warehouse' },
};

/** `Badges` renders a labelled list. This is the shape the test asserts on:
 *  `data-testid="test-badges"` with one `<li>` per entry. */
export const BadgeList: Story = {
  render: () => <Badges content={['unique', 'not_null', 'accepted_values']} />,
};

/** `Badges` returns `null` for an empty list rather than an empty `<ul>` — worth a
 *  story so the "renders nothing" contract is visible rather than only asserted. */
export const BadgeListEmpty: Story = {
  render: () => <Badges content={[]} />,
};
