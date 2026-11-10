import type { Meta, StoryObj } from '@storybook/react-vite';

import type { ResourceTypeExplorer } from '../../lib/resourceType';
import { ResourceChip } from './ResourceChip';

const RESOURCE_TYPES: ResourceTypeExplorer[] = [
  'model',
  'source',
  'seed',
  'snapshot',
  'test',
  'unit_test',
  'exposure',
  'metric',
  'semantic_model',
  'saved_query',
  'macro',
  'group',
  'analysis',
  'function',
  'column',
  'project',
];

const meta: Meta<typeof ResourceChip> = {
  component: ResourceChip,
  args: { resourceType: 'model' },
};

export default meta;
type Story = StoryObj<typeof ResourceChip>;

export const Default: Story = {};

/** Every resource type at once. Each has its own background from dbt-dag's
 *  `backgroundColors`, so this is the story that catches a type whose colour or
 *  capitalized name was never mapped. */
export const AllResourceTypes: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      {RESOURCE_TYPES.map((type) => (
        <ResourceChip key={type} resourceType={type} />
      ))}
    </div>
  ),
};

/** Icon only. Used where the label would be redundant, e.g. inside a table cell
 *  that already has a name column. */
export const IconOnly: Story = {
  args: { showText: false },
};

/** A warehouse type routes to `DataPlatformChip` instead — same call site, a
 *  different component underneath. */
export const WarehousePlatform: Story = {
  args: { resourceType: 'snowflake' },
};

export const AllWarehousePlatforms: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      {(['snowflake', 'databricks', 'bigquery', 'redshift'] as const).map((type) => (
        <ResourceChip key={type} resourceType={type} />
      ))}
    </div>
  ),
};
