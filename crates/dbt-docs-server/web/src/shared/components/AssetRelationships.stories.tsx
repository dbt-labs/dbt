import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';

import { AssetRelationships, type RelationshipItem } from './AssetRelationships';

const DEPENDS_ON: RelationshipItem[] = [
  {
    uniqueId: 'model.jaffle_shop.stg_customers',
    name: 'stg_customers',
    resourceType: 'model',
  },
  {
    uniqueId: 'model.jaffle_shop.stg_orders',
    name: 'stg_orders',
    resourceType: 'model',
  },
  {
    uniqueId: 'source.jaffle_shop.raw.customers',
    name: 'raw.customers',
    resourceType: 'source',
  },
];

const REFERENCED_BY: RelationshipItem[] = [
  {
    uniqueId: 'exposure.jaffle_shop.weekly_metrics',
    name: 'weekly_metrics',
    resourceType: 'exposure',
  },
  { uniqueId: 'metric.jaffle_shop.revenue', name: 'revenue', resourceType: 'metric' },
];

const meta: Meta<typeof AssetRelationships> = {
  component: AssetRelationships,
  args: { dependsOn: DEPENDS_ON, referencedBy: REFERENCED_BY },
  decorators: [(Story) => <div className="max-w-md">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof AssetRelationships>;

/** Both tabs present; "Depends on" wins the initial selection. */
export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await expect(canvas.getByText('stg_customers')).toBeVisible();
    await expect(canvas.queryByText('weekly_metrics')).toBeNull();

    await userEvent.click(canvas.getByText('Referenced by'));

    await expect(canvas.getByText('weekly_metrics')).toBeVisible();
    await expect(canvas.queryByText('stg_customers')).toBeNull();
  },
};

/** `onSelect` turns each row into a button — this is how the lineage panel hands a
 *  click back up to the page. Without it the rows are inert text. */
export const Selectable: Story = {
  args: { onSelect: fn() },
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);
    // The uniqueId is handed back, not the display name — the caller navigates by id.
    await userEvent.click(canvas.getByRole('button', { name: /stg_customers/ }));
    await expect(args.onSelect).toHaveBeenCalledWith('model.jaffle_shop.stg_customers');
  },
};

/** With no upstreams the initial tab falls through to "Referenced by", and the empty
 *  tab is not rendered at all. */
export const OnlyReferencedBy: Story = {
  args: { dependsOn: [] },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByText('Depends on')).toBeNull();
    await expect(canvas.getByText('weekly_metrics')).toBeVisible();
  },
};

/** A leaf node — the usual shape for an exposure. */
export const OnlyDependsOn: Story = {
  args: { referencedBy: [] },
};

/** Neither direction: a single italic line, no tab bar. */
export const NoRelationships: Story = {
  args: { dependsOn: [], referencedBy: [] },
};

/** An unrecognised resource type resolves to the `unknown` icon rather than
 *  throwing — worth pinning, since `dependsOn` carries whatever the manifest says. */
export const UnknownResourceType: Story = {
  args: {
    dependsOn: [
      { uniqueId: 'future.jaffle_shop.thing', name: 'thing', resourceType: 'future' },
    ],
    referencedBy: [],
  },
};
