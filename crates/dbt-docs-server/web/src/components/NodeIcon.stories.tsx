import type { Meta, StoryObj } from '@storybook/react-vite';

import { NodeIcon } from './NodeIcon';

const RESOURCE_TYPES = [
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
  'operation',
];

const meta: Meta<typeof NodeIcon> = {
  component: NodeIcon,
  args: { resourceType: 'model' },
};

export default meta;
type Story = StoryObj<typeof NodeIcon>;

export const Default: Story = {};

export const Medium: Story = {
  args: { size: 'md' },
};

/**
 * The per-type background colours come from `.type-pill` classes in `app.css`, so this
 * is the story that catches a resource type with no rule — it renders unstyled rather
 * than erroring.
 */
export const AllResourceTypes: Story = {
  render: () => (
    <div className="flex flex-wrap items-center gap-2">
      {RESOURCE_TYPES.map((type) => (
        <NodeIcon key={type} resourceType={type} />
      ))}
    </div>
  ),
};

export const AllResourceTypesMedium: Story = {
  render: () => (
    <div className="flex flex-wrap items-center gap-2">
      {RESOURCE_TYPES.map((type) => (
        <NodeIcon key={type} resourceType={type} size="md" />
      ))}
    </div>
  ),
};

/** Two snake_case types get shortened labels so they fit the sidebar; everything else
 *  renders its raw `resource_type`. */
export const AbbreviatedLabels: Story = {
  render: () => (
    <div className="flex items-center gap-2">
      <NodeIcon resourceType="semantic_model" />
      <NodeIcon resourceType="saved_query" />
    </div>
  ),
};

/** An unknown type falls through to its own name — new resource types appear
 *  unstyled but readable rather than blank. */
export const UnknownType: Story = {
  args: { resourceType: 'future_type' },
};
