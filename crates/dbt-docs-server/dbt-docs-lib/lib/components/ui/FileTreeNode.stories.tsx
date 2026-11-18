import { type ComponentProps, useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';

import { FileTreeNode } from './FileTreeNode';
import type { FileTreeItemType } from './PaginatedFileTree';

/** One row of the file tree. Ids are full paths — the component derives the label from
 *  the last segment (or `data.name`) and takes its indentation from `depth`, so these
 *  fixtures mirror what `buildFileTreeItems` emits. */
const MARTS = 'jaffle_shop/models/marts';

const customers: FileTreeItemType = {
  id: `${MARTS}/customers.sql`,
  parent: MARTS,
  data: { pathType: 'file' },
};

const orders: FileTreeItemType = {
  id: `${MARTS}/orders.sql`,
  parent: MARTS,
  data: { pathType: 'file' },
};

const marts: FileTreeItemType = {
  id: MARTS,
  parent: 'jaffle_shop/models',
  data: { pathType: 'directory' },
};

const childrenByParent = new Map<string, FileTreeItemType[]>([
  [MARTS, [customers, orders]],
]);

const meta: Meta<typeof FileTreeNode> = {
  component: FileTreeNode,
  args: {
    item: customers,
    depth: 0,
    childrenByParent,
    openDirectories: [],
    setOpenDirectories: fn(),
    onFileSelect: fn(),
    onFolderSelect: fn(),
  },
  // Rows are full-bleed buttons, so they need a bounded parent to show their
  // truncation and their right-aligned info text.
  decorators: [(Story) => <div className="w-[320px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof FileTreeNode>;

/** A file row: a single button, so the whole row is one tab stop and one click
 *  target. `onFileSelect` gets the id, the event and the item — callers need the item
 *  to map a path back to a resource unique_id. */
export const File: Story = {
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('button', { name: /customers\.sql/ }));
    await expect(args.onFileSelect).toHaveBeenCalledWith(
      customers.id,
      expect.anything(),
      customers,
    );
  },
};

/** The selected file. `selectedFile` is matched against the item id — the full path,
 *  not the display name, so two `schema.yml` rows in different folders can't collide. */
export const SelectedFile: Story = {
  args: { selectedFile: customers.id },
};

/** `data.info.text` is pushed to the trailing edge with `ml-auto`. Used for per-row
 *  counts; it is `shrink-0`, so it wins space over the name. */
export const WithInfoText: Story = {
  args: {
    item: {
      ...customers,
      data: { ...customers.data, info: { text: '2 tests' } },
    },
  },
};

/** `data.name` overrides the label derived from the path. This is how a YAML file that
 *  has been turned into a folder lists the resources defined inside it — the leaf's
 *  path segment is the resource name already, but an explicit name survives ids that
 *  don't end in something readable. */
export const ExplicitDisplayName: Story = {
  args: {
    item: {
      id: 'jaffle_shop/models/staging/_sources.yml/raw_customers',
      parent: 'jaffle_shop/models/staging/_sources.yml',
      data: {
        pathType: 'file',
        name: 'raw_customers',
      },
    },
  },
};

/** Long names truncate rather than wrap or widen the row — file trees live in a narrow
 *  pane, and a wrapped row would break the fixed row height the indentation relies on. */
export const LongFileName: Story = {
  args: {
    item: {
      ...customers,
      id: `${MARTS}/int_order_items_joined_to_customers_and_products.sql`,
    },
  },
};

/** `depth` is the only thing driving indentation (16px per level); the component does
 *  not infer it from the id. A row rendered at the wrong depth looks misparented. */
export const Nested: Story = {
  args: { depth: 3 },
};

/** A collapsed folder. Two controls, deliberately: the caret toggles open/closed, and
 *  the name is a separate button so a tree can treat "select this folder" and "expand
 *  this folder" as different intents. Children are unmounted while closed. */
export const FolderCollapsed: Story = {
  args: { item: marts },
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    await expect(canvas.queryByText('customers.sql')).toBeNull();
    await expect(canvas.getByRole('button', { name: 'Expand' })).toBeInTheDocument();

    await userEvent.click(canvas.getByRole('button', { name: 'Expand' }));
    // The caret reports itself as an expand click, so a caller can distinguish it from
    // a click on the folder name and skip navigating.
    await expect(args.onFolderSelect).toHaveBeenCalledWith(
      marts.id,
      expect.anything(),
      true,
    );
    await expect(args.setOpenDirectories).toHaveBeenCalled();
  },
};

/** Open, with its children mounted. The caret's accessible name flips to Collapse, so
 *  screen-reader users get the state and not just the control. */
export const FolderExpanded: Story = {
  args: { item: marts, openDirectories: [MARTS] },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await expect(canvas.getByText('customers.sql')).toBeVisible();
    await expect(canvas.getByText('orders.sql')).toBeVisible();
    await expect(canvas.getByRole('button', { name: 'Collapse' })).toBeInTheDocument();
  },
};

/** The selected folder, matched on id like `selectedFile`. A folder can be selected
 *  and closed at the same time — selection and expansion are independent. */
export const SelectedFolder: Story = {
  args: { item: marts, openDirectories: [MARTS], selectedFolder: MARTS },
};

/** A real component rather than an inline `render` closure, because hooks are not
 *  allowed in the latter. Holds `openDirectories` so the toggle stories show the row
 *  actually opening and closing instead of just reporting the intent. */
function InteractiveNode(props: ComponentProps<typeof FileTreeNode>) {
  const [openDirectories, setOpenDirectories] = useState(props.openDirectories);
  return (
    <FileTreeNode
      {...props}
      openDirectories={openDirectories}
      setOpenDirectories={(updater) =>
        setOpenDirectories((prev) => updater(prev) ?? [])
      }
    />
  );
}

/**
 * Clicking the folder *name* only ever opens. That asymmetry is deliberate: the name
 * means "show me this folder", and a second click on an already-open folder should not
 * close what the user just asked to see.
 */
export const RowClickOnlyOpens: Story = {
  args: { item: marts },
  render: (args) => <InteractiveNode {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('button', { name: 'marts' }));
    await expect(canvas.getByText('customers.sql')).toBeVisible();

    // Clicking the name again leaves it open.
    await userEvent.click(canvas.getByRole('button', { name: 'marts' }));
    await expect(canvas.getByText('customers.sql')).toBeVisible();

    // The caret is the way back to closed.
    await userEvent.click(canvas.getByRole('button', { name: 'Collapse' }));
    await expect(canvas.queryByText('customers.sql')).toBeNull();
  },
};

/**
 * `enableCloseFolderOnSecondClick` opts into the other behaviour: a name click closes
 * the folder, but only when it is both open *and* already the selected folder — so the
 * first click that selects a folder never also closes it.
 */
export const CloseFolderOnSecondClick: Story = {
  args: {
    item: marts,
    openDirectories: [MARTS],
    selectedFolder: MARTS,
    enableCloseFolderOnSecondClick: true,
  },
  render: (args) => <InteractiveNode {...args} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await expect(canvas.getByText('customers.sql')).toBeVisible();
    await userEvent.click(canvas.getByRole('button', { name: 'marts' }));
    await expect(canvas.queryByText('customers.sql')).toBeNull();
  },
};
