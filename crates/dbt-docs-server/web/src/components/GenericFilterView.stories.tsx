import type { Meta, StoryObj } from '@storybook/react-vite';
import type { ColumnDef } from '@tanstack/react-table';
import { expect, within } from 'storybook/test';

import { type AssetSummary, makeFakeProject } from '../shared';
import { FilterDropdown } from '../shared';
import { UNSUPPORTED_SURFACE_MESSAGE } from '../shared/hooks/unsupportedSurface';
import {
  emptyStorySource,
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
} from '../shared/testing/storySources';
import { GenericFilterView } from './GenericFilterView';

// Only fields common to every `AssetSummary`: this component is generic over the
// union, so per-type columns belong in the per-type views that wrap it.
const COLUMNS: ColumnDef<AssetSummary>[] = [
  { id: 'name', header: 'Name', accessorFn: (r) => r.name, enableSorting: true },
  { id: 'package', header: 'Package', accessorFn: (r) => r.packageName },
  { id: 'description', header: 'Description', accessorFn: (r) => r.description ?? '' },
];

const meta: Meta<typeof GenericFilterView> = {
  component: GenericFilterView,
  args: {
    label: 'Models',
    project: makeFakeProject(),
    resourceType: 'model',
    columns: COLUMNS,
  },
};

export default meta;
type Story = StoryObj<typeof GenericFilterView>;

/**
 * The deep module every per-type list view wraps: breadcrumb, heading, optional filter
 * toolbar, and the table — plus the `useAssetList` fetch. The per-type views supply
 * only columns and facet controls.
 */
export const Default: Story = {};

/** `filterControls` is the toolbar slot. The per-type views put their facet dropdowns
 *  here; the shell does not care what they are. */
export const WithFilterControls: Story = {
  args: {
    filterControls: (
      <FilterDropdown
        name="Modeling layer"
        options={[
          { label: 'All', value: '' },
          { label: 'Marts (30)', value: 'marts' },
        ]}
        defaultOption={{ label: 'All', value: '' }}
        onChange={() => {}}
      />
    ),
  },
};

export const Sortable: Story = {
  args: { isSortable: true, initialSortColumn: 'name', onChangeSort: () => {} },
};

/** Filters that match nothing. */
export const Empty: Story = {
  parameters: { docsApp: { source: emptyStorySource() } },
};

export const EmptyWithCustomMessage: Story = {
  args: { emptyMessage: 'No models match the current filters.' },
  parameters: { docsApp: { source: emptyStorySource() } },
};

export const Loading: Story = {
  parameters: { docsApp: { source: loadingStorySource() } },
};

export const LoadError: Story = {
  parameters: { docsApp: { source: failingStorySource() } },
};

/**
 * A source that never implemented `fetchAssetList`. The hooks surface that as an
 * *error* rather than an empty table — beside a sidebar count of 142, "no models"
 * would read as data loss.
 */
export const UnsupportedSurface: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // The invariant worth locking down: an unsupported surface must not read as an
    // empty project. Asserted against the shared constant so the copy can be
    // reworded without breaking the test.
    await expect(
      await canvas.findByText(UNSUPPORTED_SURFACE_MESSAGE),
    ).toBeInTheDocument();
    await expect(
      canvas.queryByText(/No resources match the current filters/),
    ).toBeNull();
  },
};

/** A different resource type through the same shell, to show that nothing here is
 *  model-specific. */
export const OtherResourceType: Story = {
  args: { label: 'Snapshots', resourceType: 'snapshot' },
};
