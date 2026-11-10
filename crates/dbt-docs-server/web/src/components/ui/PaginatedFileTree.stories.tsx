import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';

import { buildFileTreeItems } from '../../lib/fileTree';
import { storyFiles } from '../../shared/testing/storyFixtures';
import {
  type FileTreeItemType,
  PaginatedFileTree,
  type PaginatedFileTreeProps,
} from './PaginatedFileTree';

/**
 * The tree is built from a *flat* list plus `parent` pointers, not from nesting, and
 * `rootNodeName` names the parent whose children are the top-level rows — so the
 * project row below is itself an item (`parent: 'root'`), the same shape
 * `buildFileTreeItems` produces for the locate pane.
 */
const ROOT = 'root';
const PROJECT = 'jaffle_shop';

function dir(id: string, parent: string, name?: string): FileTreeItemType {
  return { id, parent, data: { pathType: 'directory', ...(name && { name }) } };
}

function file(
  id: string,
  parent: string,
  resourceType: string,
  info?: string,
): FileTreeItemType {
  return {
    id,
    parent,
    data: {
      pathType: 'file',
      ...(info && { info: { text: info } }),
    },
  };
}

const ITEMS: FileTreeItemType[] = [
  dir(PROJECT, ROOT),
  // Listed out of order on purpose: the component sorts directories before files and
  // then alphabetically, so the input order should not matter.
  file(`${PROJECT}/dbt_project.yml`, PROJECT, 'doc'),
  dir(`${PROJECT}/seeds`, PROJECT),
  dir(`${PROJECT}/models`, PROJECT),
  dir(`${PROJECT}/macros`, PROJECT),
  file(`${PROJECT}/macros/cents_to_dollars.sql`, `${PROJECT}/macros`, 'macro'),
  dir(`${PROJECT}/models/marts`, `${PROJECT}/models`),
  dir(`${PROJECT}/models/staging`, `${PROJECT}/models`),
  file(
    `${PROJECT}/models/marts/customers.sql`,
    `${PROJECT}/models/marts`,
    'model',
    '2 tests',
  ),
  file(`${PROJECT}/models/marts/orders.sql`, `${PROJECT}/models/marts`, 'model'),
  file(
    `${PROJECT}/models/staging/stg_customers.sql`,
    `${PROJECT}/models/staging`,
    'model',
  ),
  // A YAML file rendered as a folder, with one leaf per resource defined inside it.
  // The leaf carries an explicit `name` because its path segment is the resource name.
  dir(`${PROJECT}/models/staging/_sources.yml`, `${PROJECT}/models/staging`),
  {
    id: `${PROJECT}/models/staging/_sources.yml/raw_customers`,
    parent: `${PROJECT}/models/staging/_sources.yml`,
    data: {
      pathType: 'file',
      name: 'raw_customers',
    },
  },
  file(`${PROJECT}/seeds/raw_customers.csv`, `${PROJECT}/seeds`, 'seed'),
];

const ALL_DIRECTORIES = ITEMS.filter((i) => i.data.pathType === 'directory').map(
  (i) => i.id,
);

const meta: Meta<typeof PaginatedFileTree> = {
  component: PaginatedFileTree,
  args: {
    items: ITEMS,
    rootNodeName: ROOT,
    openDirectories: [PROJECT, `${PROJECT}/models`],
    setOpenDirectories: fn(),
    onFileSelect: fn(),
    onFolderSelect: fn(),
  },
  decorators: [
    (Story) => (
      <div className="w-[320px] rounded-lg border border-borderMuted p-2">
        {Story()}
      </div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof PaginatedFileTree>;

/** Partly expanded, which is the normal steady state: the project and its `models`
 *  folder open, everything below still closed. Note the ordering — `macros`, `models`
 *  and `seeds` come before `dbt_project.yml` because directories sort first. */
export const Default: Story = {};

/** Nothing open. Only the rows parented to `rootNodeName` render, so this is the
 *  first paint of the Files tab before the user expands anything. */
export const Collapsed: Story = {
  args: { openDirectories: [] },
};

/** Every folder open, including the YAML-as-folder row whose child is a source
 *  defined inline rather than a file on disk. */
export const FullyExpanded: Story = {
  args: { openDirectories: ALL_DIRECTORIES },
};

/** A selected file. Only rows under an open folder exist, so a deep selection is only
 *  visible if its ancestors are open — the caller is responsible for expanding them. */
export const WithSelectedFile: Story = {
  args: {
    openDirectories: ALL_DIRECTORIES,
    selectedFile: `${PROJECT}/models/marts/customers.sql`,
  },
};

/** A selected folder, highlighted the same way as a selected file. */
export const WithSelectedFolder: Story = {
  args: {
    openDirectories: [PROJECT, `${PROJECT}/models`],
    selectedFolder: `${PROJECT}/models/marts`,
  },
};

/** `maxHeight` caps the scroll container rather than the content, so a fully expanded
 *  tree scrolls inside the pane instead of pushing the layout. */
export const ConstrainedHeight: Story = {
  args: { openDirectories: ALL_DIRECTORIES, maxHeight: 160 },
};

/** `onSort` replaces the whole comparator — including the directories-first grouping,
 *  which is why `dbt_project.yml` now leads. It receives ids, not items. */
export const CustomSort: Story = {
  args: {
    openDirectories: [PROJECT],
    onSort: (a: string, b: string) => b.localeCompare(a),
  },
};

/** No items: the tree still renders its `role="tree"` container, so a caller that
 *  wants an empty-state message has to supply one itself. */
export const Empty: Story = {
  args: { items: [], openDirectories: [] },
};

/** A `rootNodeName` that matches nothing renders the same empty tree — the usual cause
 *  of a blank tree from a non-empty item list. */
export const UnmatchedRootNodeName: Story = {
  args: { rootNodeName: 'nope' },
};

/** A real component rather than an inline `render` closure, because hooks are not
 *  allowed in the latter. The tree is controlled: it hands back a reducer-style
 *  updater that may return `undefined`, which the caller coerces to `[]`. */
function InteractiveTree(props: PaginatedFileTreeProps) {
  const [openDirectories, setOpenDirectories] = useState(props.openDirectories);
  return (
    <PaginatedFileTree
      {...props}
      openDirectories={openDirectories}
      setOpenDirectories={(updater) =>
        setOpenDirectories((prev) => updater(prev) ?? [])
      }
    />
  );
}

/** Drilling down from the project root to a file, which is the interaction the Files
 *  tab exists for. */
export const Interactive: Story = {
  args: { openDirectories: [] },
  render: (args) => <InteractiveTree {...args} />,
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);

    await userEvent.click(canvas.getByRole('button', { name: PROJECT }));
    await userEvent.click(canvas.getByRole('button', { name: 'models' }));
    await userEvent.click(canvas.getByRole('button', { name: 'marts' }));

    const target = canvas.getByRole('button', { name: /customers\.sql/ });
    await userEvent.click(target);
    await expect(args.onFileSelect).toHaveBeenCalledWith(
      `${PROJECT}/models/marts/customers.sql`,
      expect.anything(),
      expect.objectContaining({ id: `${PROJECT}/models/marts/customers.sql` }),
    );
  },
};

// The wire-shaped fixture, built the way `LocatePane` builds it: one synthetic root
// named after the project, with every package nested under it. For the `jaffle_shop`
// package that repeats the segment (`jaffle_shop/jaffle_shop/models/...`) — the root is
// the project and the folder is the package, which happen to share a name.
const wireItems = buildFileTreeItems(storyFiles(), PROJECT).items;
const wireDirectories = wireItems
  .filter((i) => i.data.pathType === 'directory')
  .map((i) => i.id);

/** Built by `buildFileTreeItems` from the same `FileEntry[]` fixture the app-level
 *  stories use — the multi-package layout, resource-type icons and YAML-as-folder rows
 *  all come out of that transform rather than being hand-written here. */
export const FromFileEntries: Story = {
  args: {
    items: wireItems,
    rootNodeName: ROOT,
    openDirectories: wireDirectories,
    maxHeight: 320,
  },
};
