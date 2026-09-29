import type { Meta, StoryObj } from '@storybook/react-vite';

import { makeFakeProject } from '../shared';
import {
  storyExposure,
  storyMacro,
  storyModel,
  storyNodes,
  storySource,
} from '../shared/testing/storyFixtures';
import { PreviewDrawer } from './PreviewDrawer';

const summary = storyNodes()[0];

const meta: Meta<typeof PreviewDrawer> = {
  component: PreviewDrawer,
  args: {
    project: makeFakeProject(),
    previewId: 'model.jaffle_shop.customers',
    summary,
    detail: storyModel(),
    onClose: () => {},
    onOpenFull: () => {},
  },
  // A right-docked drawer with a backdrop: it needs the viewport, not a padded canvas.
  parameters: { layout: 'fullscreen' },
};

export default meta;
type Story = StoryObj<typeof PreviewDrawer>;

/** The peek drawer, fully resolved. */
export const Default: Story = {};

/**
 * The interesting state: the summary is already in memory from the node list, so the
 * drawer paints name, type, schema and materialization *immediately*, and only the
 * fields that need the detail fetch show as loading. Opening a peek should never be a
 * blank panel.
 */
export const SummaryOnlyWhileDetailLoads: Story = {
  args: { detail: null },
};

/** Neither summary nor detail — the drawer falls all the way back to the uniqueId as
 *  its title. Rare, but reachable from a deep link to an id not in the node list. */
export const NothingKnownYet: Story = {
  args: { summary: null, detail: null },
};

/** Detail arrives without a summary, e.g. a peek opened before the node list settled. */
export const DetailWithoutSummary: Story = {
  args: { summary: null },
};

export const SourceAsset: Story = {
  args: {
    previewId: 'source.jaffle_shop.raw.customers',
    summary: storyNodes().find((n) => n.resource_type === 'source') ?? null,
    detail: storySource(),
  },
};

export const ExposureAsset: Story = {
  args: {
    previewId: 'exposure.jaffle_shop.weekly_metrics',
    summary: storyNodes().find((n) => n.resource_type === 'exposure') ?? null,
    detail: storyExposure(),
  },
};

/** A macro has no relation, so the database/schema cells stay empty rather than
 *  showing placeholders. */
export const MacroAsset: Story = {
  args: {
    previewId: 'macro.jaffle_shop.cents_to_dollars',
    summary: storyNodes().find((n) => n.resource_type === 'macro') ?? null,
    detail: storyMacro(),
  },
};

/** No description: the paragraph is omitted rather than left blank. */
export const WithoutDescription: Story = {
  args: {
    summary: summary ? { ...summary, description: null } : null,
    detail: storyModel({ description: null }),
  },
};

/** A long name in the title row, which truncates through `PageHeading`. */
export const LongName: Story = {
  args: {
    detail: storyModel({
      name: 'int_order_items_joined_to_customers_and_products_and_locations',
    }),
    summary: summary
      ? {
          ...summary,
          name: 'int_order_items_joined_to_customers_and_products_and_locations',
        }
      : null,
  },
};

/** An unrecognised resource type falls back to its own name for both the singular and
 *  the pluralised breadcrumb label. */
export const UnknownResourceType: Story = {
  args: {
    previewId: 'future_type.jaffle_shop.thing',
    summary: summary
      ? { ...summary, resource_type: 'future_type', name: 'thing' }
      : null,
    detail: null,
  },
};
