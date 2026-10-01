import type { Meta, StoryObj } from '@storybook/react-vite';

import { assetToMetadataProps } from '../mappers/assetToMetadataProps';
import {
  storyExposure,
  storyMacro,
  storyModel,
  storySavedQuery,
  storySource,
} from '../testing/storyFixtures';
import { AssetMetadata } from './AssetMetadata';

const meta: Meta<typeof AssetMetadata> = {
  component: AssetMetadata,
  args: assetToMetadataProps(storyModel()),
  decorators: [(Story) => <div className="max-w-2xl">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof AssetMetadata>;

/** Which rows appear is chosen per resource type, and within a type each row is
 *  omitted when its value is absent — so these stories are really about the
 *  `resourceType` switch. */
export const Model: Story = {};

/** Loader, database and schema instead of access/contract/materialization. */
export const Source: Story = {
  args: assetToMetadataProps(storySource()),
};

export const Exposure: Story = {
  args: assetToMetadataProps(storyExposure()),
};

/** Macros get only package and file — there is no relation to describe. */
export const Macro: Story = {
  args: assetToMetadataProps(storyMacro()),
};

export const SavedQuery: Story = {
  args: assetToMetadataProps(storySavedQuery()),
};

/** An unrecognised resource type falls through to the default: package only. Worth a
 *  story because new resource types land here silently rather than erroring. */
export const UnknownResourceType: Story = {
  args: {
    resourceType: 'some_future_type',
    uniqueId: 'some_future_type.jaffle_shop.thing',
    packageName: 'jaffle_shop',
    relation: null,
  },
};

/** With no relation object but a pre-formatted `relationName`, the row renders as
 *  plain text instead of the copyable `RelationName`. */
export const RelationNameFallback: Story = {
  args: {
    ...assetToMetadataProps(storyModel()),
    relation: null,
    relationName: 'analytics.dbt.customers',
  },
};

/**
 * `userState` adds the query-history upsell as a table footer. Only on
 * model/seed/snapshot — the row is meaningless for a macro. `via-catalog` resolves to
 * hidden copy, so that state renders no row even though the prop is set.
 */
export const WithQueryHistoryUpsell: Story = {
  args: { ...assetToMetadataProps(storyModel()), userState: 'core' },
};

export const WithQueryHistoryUpsellSuppressed: Story = {
  args: { ...assetToMetadataProps(storyModel()), userState: 'via-catalog' },
};

/** `compact` is the panel form: tighter rows, no outer dividers. */
export const Compact: Story = {
  args: { ...assetToMetadataProps(storyModel()), compact: true },
};

export const Loading: Story = {
  args: { ...assetToMetadataProps(storyModel()), isLoading: true },
};

/** Nothing but a uniqueId — every optional row drops and the table renders nothing.
 *  `hasAssetMetadata` exists so a caller can avoid wrapping this in an empty section. */
export const EmptyRendersNothing: Story = {
  args: {
    resourceType: 'model',
    uniqueId: 'model.jaffle_shop.customers',
    packageName: null,
    relation: null,
  },
};
