import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import { Collapsible, CollapsibleContent, CollapsibleTrigger } from './Collapsible';

/**
 * `Collapsible` is a thin re-export of Radix's collapsible primitives, so it carries
 * no styling of its own — every story below supplies its own classes, which is how the
 * app uses it (see `FileTreeNode`). The one behaviour worth knowing is that
 * `CollapsibleContent` *unmounts* while closed rather than hiding: a closed section
 * costs nothing to render, but anything with state inside it loses that state.
 */
const meta: Meta<typeof Collapsible> = {
  component: Collapsible,
  args: {
    children: (
      <>
        <CollapsibleTrigger className="w-full rounded px-2 py-1 text-left text-sm text-fgMain hover:bg-bgMainHover">
          Compiled SQL
        </CollapsibleTrigger>
        <CollapsibleContent className="mt-1 rounded bg-bgNeutralMuted p-2 text-xs text-fgAlt">
          select customer_id, count(*) from orders group by 1
        </CollapsibleContent>
      </>
    ),
  },
  decorators: [(Story) => <div className="w-[420px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof Collapsible>;

/** Uncontrolled and closed — the trigger owns the open state. */
export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Closed means absent, not hidden, so query rather than get.
    await expect(canvas.queryByText(/select customer_id/)).toBeNull();

    await userEvent.click(canvas.getByRole('button', { name: 'Compiled SQL' }));
    await expect(canvas.getByText(/select customer_id/)).toBeVisible();

    await userEvent.click(canvas.getByRole('button', { name: 'Compiled SQL' }));
    await expect(canvas.queryByText(/select customer_id/)).toBeNull();
  },
};

/** `defaultOpen` seeds the uncontrolled state — open on first paint, still toggleable. */
export const DefaultOpen: Story = {
  args: { defaultOpen: true },
};

/** `open` without `onOpenChange` pins the section open: the trigger renders and can be
 *  clicked, but nothing moves. This is the form `FileTreeNode` uses — it drives
 *  `open` from the `openDirectories` array it is handed and toggles that array itself,
 *  rather than letting Radix hold the state. */
export const ControlledOpen: Story = {
  args: { open: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: 'Compiled SQL' }));
    // Still open: the parent owns the state and did not change it.
    await expect(canvas.getByText(/select customer_id/)).toBeVisible();
  },
};

/** Disabled: the trigger is inert and marked `disabled` for assistive tech. A closed
 *  disabled section cannot be opened by any means. */
export const Disabled: Story = {
  args: { disabled: true },
};

/** A real component rather than an inline `render` closure, because hooks are not
 *  allowed in the latter. */
function ControlledCollapsible() {
  const [open, setOpen] = useState(false);
  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className="w-full rounded px-2 py-1 text-left text-sm text-fgMain hover:bg-bgMainHover">
        {open ? 'Hide' : 'Show'} compiled SQL
      </CollapsibleTrigger>
      <CollapsibleContent className="mt-1 rounded bg-bgNeutralMuted p-2 text-xs text-fgAlt">
        select customer_id, count(*) from orders group by 1
      </CollapsibleContent>
    </Collapsible>
  );
}

/** The fully controlled pairing: `open` plus `onOpenChange`, so the trigger works and
 *  the parent can also open the section from elsewhere. Label reacts to the state. */
export const Controlled: Story = {
  render: () => <ControlledCollapsible />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('button', { name: 'Show compiled SQL' }));
    await expect(canvas.getByText(/select customer_id/)).toBeVisible();
    await expect(
      canvas.getByRole('button', { name: 'Hide compiled SQL' }),
    ).toBeInTheDocument();
  },
};

/** With `asChild` the trigger renders *as* the child element instead of wrapping it in
 *  its own `<button>`, which is how a tree row makes its caret the trigger without
 *  nesting one button inside another. */
export const TriggerAsChild: Story = {
  args: {
    defaultOpen: true,
    children: (
      <>
        <CollapsibleTrigger asChild>
          <button
            type="button"
            aria-label="Toggle marts"
            className="rounded px-2 py-1 text-sm text-fgMain hover:bg-bgMainHover"
          >
            marts
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent className="pl-4 text-sm text-fgAlt">
          customers.sql
        </CollapsibleContent>
      </>
    ),
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // Exactly one button: the child, not a wrapper plus the child.
    await expect(canvas.getAllByRole('button')).toHaveLength(1);
    await userEvent.click(canvas.getByRole('button', { name: 'Toggle marts' }));
    await expect(canvas.queryByText('customers.sql')).toBeNull();
  },
};
