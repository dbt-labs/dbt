import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import { CollapsibleSection } from './CollapsibleSection';

const meta: Meta<typeof CollapsibleSection> = {
  component: CollapsibleSection,
  args: {
    children: <span className="text-fgMain">customers</span>,
    toExpand: (
      <div className="ml-6 border-l border-borderMuted py-2 pl-4 text-fgAlt">
        customer_id, first_order_date, number_of_orders, lifetime_value
      </div>
    ),
    closeAltText: 'Collapse customers',
    expandAltText: 'Expand customers',
  },
};

export default meta;
type Story = StoryObj<typeof CollapsibleSection>;

/** Collapsed. The whole header is a transparent full-bleed button, so clicking
 *  anywhere on the row toggles — including the caret. */
export const Collapsed: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // The toggle has no visible label — its accessible name is the sr-only text, which
    // is the only thing a screen reader has to go on. Querying by it therefore also
    // asserts that the label is present and correct.
    const toggle = canvas.getByRole('button', { name: 'Expand customers' });
    await expect(canvas.queryByText(/customer_id, first_order_date/)).toBeNull();

    await userEvent.click(toggle);
    await expect(canvas.getByText(/customer_id, first_order_date/)).toBeVisible();
    // The label flips with the state, so the control keeps describing what it does.
    await expect(
      canvas.getByRole('button', { name: 'Collapse customers' }),
    ).toBeInTheDocument();

    await userEvent.click(canvas.getByRole('button', { name: 'Collapse customers' }));
    await expect(canvas.queryByText(/customer_id, first_order_date/)).toBeNull();
  },
};

export const InitiallyOpen: Story = {
  args: { isOpen: true },
};

/** `disable` removes the caret and the button entirely: a leaf row that looks like
 *  the others but cannot be opened. Note it also suppresses `toExpand` regardless of
 *  `isOpen`. */
export const Disabled: Story = {
  args: { disable: true, isOpen: true },
};

/** With `shouldIndent={false}` a disabled row loses the spacer that keeps its label
 *  aligned with expandable siblings — the two side by side show why the default is
 *  on. */
export const DisabledWithoutIndent: Story = {
  args: { disable: true, shouldIndent: false },
};

export const Nested: Story = {
  args: {
    isOpen: true,
    children: <span className="text-fgMain">models</span>,
    toExpand: (
      <div className="ml-6">
        <CollapsibleSection
          closeAltText="Collapse marts"
          expandAltText="Expand marts"
          toExpand={<div className="ml-6 py-1 text-fgAlt">customers.sql</div>}
        >
          <span className="text-fgMain">marts</span>
        </CollapsibleSection>
        <CollapsibleSection
          disable
          closeAltText="Collapse staging"
          expandAltText="Expand staging"
          toExpand={null}
        >
          <span className="text-fgMain">staging</span>
        </CollapsibleSection>
      </div>
    ),
  },
};
