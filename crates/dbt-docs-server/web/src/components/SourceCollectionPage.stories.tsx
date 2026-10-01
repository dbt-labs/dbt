import type { Meta, StoryObj } from '@storybook/react-vite';

import { storyNodes } from '../shared/testing/storyFixtures';
import { loadingStorySource, storyDataSource } from '../shared/testing/storySources';
import type { NodeSummary } from '../types';
import { SourceCollectionPage } from './SourceCollectionPage';

function sourceNode(name: string, collection: string): NodeSummary {
  return {
    unique_id: `source.jaffle_shop.${collection}.${name}`,
    name,
    resource_type: 'source',
    package_name: 'jaffle_shop',
    description: `Raw ${name} table.`,
    database_name: 'raw',
    schema_name: 'jaffle_shop',
    original_file_path: 'models/staging/_sources.yml',
  };
}

const NODES: NodeSummary[] = [
  ...storyNodes(),
  sourceNode('customers', 'raw'),
  sourceNode('orders', 'raw'),
  sourceNode('products', 'raw'),
  // A different collection, to prove the page filters to the one in the route.
  sourceNode('pages', 'events'),
];

const meta: Meta<typeof SourceCollectionPage> = {
  component: SourceCollectionPage,
  args: { nodes: NODES, onSelect: () => {} },
  // The collection name comes from the route, so every story needs one.
  parameters: { docsApp: { initialEntries: ['/sources/raw'] } },
};

export default meta;
type Story = StoryObj<typeof SourceCollectionPage>;

/**
 * One source collection's tables. The collection is matched on the third segment of
 * each source's uniqueId, so `raw` here picks up three tables and excludes the
 * `events` one.
 */
export const Default: Story = {};

/** A collection with a single table. */
export const SingleTable: Story = {
  parameters: { docsApp: { initialEntries: ['/sources/events'] } },
};

/** A collection name that matches nothing — a stale link, or a source removed from
 *  the project since the page was bookmarked. */
export const UnknownCollection: Story = {
  parameters: { docsApp: { initialEntries: ['/sources/does_not_exist'] } },
};

/** No sources in the project at all. */
export const NoSources: Story = {
  args: { nodes: storyNodes().filter((n) => n.resource_type !== 'source') },
};

/** Per-table detail (freshness, columns) is fetched per row, so this is the state
 *  while those land. */
export const DetailsLoading: Story = {
  parameters: {
    docsApp: {
      source: loadingStorySource(),
      initialEntries: ['/sources/raw'],
    },
  },
};

/** A detail fetch that resolves to nothing — the table rows still render from the
 *  node index. */
export const DetailsNotFound: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({ fetchAsset: async () => null }),
      initialEntries: ['/sources/raw'],
    },
  },
};
