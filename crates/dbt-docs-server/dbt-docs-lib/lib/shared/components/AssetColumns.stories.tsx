import type { Meta, StoryObj } from '@storybook/react-vite';

import { storyColumns } from '../testing/storyFixtures';
import { AssetColumns } from './AssetColumns';

const meta: Meta<typeof AssetColumns> = {
  component: AssetColumns,
  args: { columns: storyColumns() },
};

export default meta;
type Story = StoryObj<typeof AssetColumns>;

/** Description and type are collapsed into one cell as `description (type)`. The
 *  fixture's undocumented column shows the `—` placeholder that stands in for a
 *  missing description. */
export const Default: Story = {};

/** No types available, as when the catalog was never read. The parenthesised type is
 *  dropped entirely rather than rendered empty. */
export const WithoutTypes: Story = {
  args: {
    columns: storyColumns().map((column) => ({ ...column, dataType: null })),
  },
};

export const Loading: Story = {
  args: { columns: [], isLoading: true },
};

export const NoColumns: Story = {
  args: { columns: [] },
};
