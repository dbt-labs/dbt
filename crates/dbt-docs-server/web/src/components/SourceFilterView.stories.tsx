import type { Meta, StoryObj } from '@storybook/react-vite';

import { makeFakeProject, type SourceSummary } from '../shared';
import {
  emptyStorySource,
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
  storyDataSource,
} from '../shared/testing/storySources';
import { SourceFilterView } from './SourceFilterView';

function source(
  name: string,
  sourceName: string,
  databaseName: string,
  schemaName: string,
): SourceSummary {
  return {
    uniqueId: `source.jaffle_shop.${sourceName}.${name}`,
    name,
    resourceType: 'source',
    description: null,
    packageName: 'jaffle_shop',
    tags: [],
    sourceName,
    databaseName,
    schemaName,
  };
}

const meta: Meta<typeof SourceFilterView> = {
  component: SourceFilterView,
  args: { project: makeFakeProject() },
};

export default meta;
type Story = StoryObj<typeof SourceFilterView>;

/**
 * Unlike the other list views, this one lists source *collections* rather than source
 * tables: it fetches every source and rolls them up by `sourceName`, counting tables.
 * So the row count here is smaller than the fetched row count.
 */
export const Default: Story = {};

/** Several collections across two databases, which is what makes the database and
 *  schema dropdowns meaningful — both are derived from the rolled-up rows. */
export const MultipleCollections: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchAssetList: async () => ({
          items: [
            source('customers', 'raw', 'raw', 'jaffle_shop'),
            source('orders', 'raw', 'raw', 'jaffle_shop'),
            source('products', 'raw', 'raw', 'jaffle_shop'),
            source('pages', 'events', 'analytics', 'segment'),
            source('clicks', 'events', 'analytics', 'segment'),
            source('exchange_rates', 'finance', 'analytics', 'finance'),
          ],
          nextCursor: null,
          totalCount: 6,
        }),
      }),
    },
  },
};

/**
 * `sourceName` is optional on the summary; when it is absent the collection name is
 * recovered from the third segment of the uniqueId. Both rows here land in the same
 * collection.
 */
export const SourceNameDerivedFromUniqueId: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchAssetList: async () => ({
          items: [
            { ...source('customers', 'raw', 'raw', 'jaffle_shop'), sourceName: null },
            { ...source('orders', 'raw', 'raw', 'jaffle_shop'), sourceName: null },
          ],
          nextCursor: null,
          totalCount: 2,
        }),
      }),
    },
  },
};

export const Empty: Story = {
  parameters: { docsApp: { source: emptyStorySource() } },
};

export const Loading: Story = {
  parameters: { docsApp: { source: loadingStorySource() } },
};

export const LoadError: Story = {
  parameters: { docsApp: { source: failingStorySource() } },
};

export const UnsupportedSurface: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
};
