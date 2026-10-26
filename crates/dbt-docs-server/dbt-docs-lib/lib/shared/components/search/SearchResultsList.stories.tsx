import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';

import { SearchResultsList } from './SearchResultsList';
import type { SearchResultDisplayData } from './types';

const RESULTS: SearchResultDisplayData[] = [
  {
    matchedField: 'name',
    highlight: '<b>customer</b>s',
    hit: {
      name: 'customers',
      uniqueId: 'model.jaffle_shop.customers',
      resourceType: 'model',
      fqn: ['jaffle_shop', 'marts', 'customers'],
    },
  },
  {
    matchedField: 'column',
    highlight: '<b>customer</b>_id',
    hit: {
      name: 'orders',
      uniqueId: 'model.jaffle_shop.orders',
      resourceType: 'model',
    },
  },
  {
    matchedField: 'description',
    highlight: 'raw <b>customer</b> records as landed by the loader',
    hit: {
      name: 'customers',
      uniqueId: 'source.jaffle_shop.raw.customers',
      resourceType: 'source',
    },
  },
  {
    matchedField: 'tag',
    highlight: '<b>customer</b>_facing',
    hit: {
      name: 'weekly_metrics',
      uniqueId: 'exposure.jaffle_shop.weekly_metrics',
      resourceType: 'exposure',
    },
  },
];

const meta: Meta<typeof SearchResultsList> = {
  component: SearchResultsList,
  args: {
    query: 'customer',
    data: RESULTS,
    pageSize: 4,
    isLoadingMore: false,
    hasMoreResults: false,
    fetchMore: () => {},
    getResourceHref: (uniqueId) => `#/resource/${uniqueId}`,
  },
  decorators: [(Story) => <div className="w-[760px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof SearchResultsList>;

/** The default variant — one compact row per hit. */
export const Default: Story = {};

/** With more pages, a load-more footer appears below the rows. */
export const WithMoreResults: Story = {
  args: { hasMoreResults: true, fetchMore: fn() },
  play: async ({ args, canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole('button', { name: /load more/i }));
    await expect(args.fetchMore).toHaveBeenCalled();
  },
};

/** Loading a further page appends `pageSize` skeletons *after* the existing rows,
 *  rather than replacing them. */
export const LoadingMore: Story = {
  args: { hasMoreResults: true, isLoadingMore: true },
};

/** The initial load: skeletons only, because there is no data yet. */
export const InitialSkeleton: Story = {
  args: { data: undefined, skeleton: true },
};

/** An empty array is a real "no results" answer and gets copy; `undefined` means "not
 *  loaded" and renders nothing at all. */
export const NoResults: Story = {
  args: { data: [] },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText('No results were found')).toBeVisible();
  },
};

export const NotLoadedRendersNothing: Story = {
  args: { data: undefined },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // Not the "no results" copy: nothing has been asked yet, so claiming there are no
    // results would be a different — and wrong — statement.
    await expect(canvas.queryByText('No results were found')).toBeNull();
  },
};

/** The `rich` variant swaps every row for the card presentation, and needs its own
 *  metadata and highlight resolvers to have anything extra to show. */
export const RichVariant: Story = {
  args: {
    variant: 'rich',
    getRichMetadata: (hit) => ({
      projectName: hit.uniqueId.split('.')[1],
      environmentType: 'Production',
      numColumns: 4,
    }),
    getRichHighlights: (data) =>
      data.highlight ? { [data.matchedField]: [data.highlight] } : undefined,
  },
};

/** The rich variant's skeleton is taller than the default one, matching its card. */
export const RichVariantSkeleton: Story = {
  args: { variant: 'rich', data: undefined, skeleton: true },
};

/** Optional per-row builders: a lineage CTA for hits that carry an `fqn`, and column
 *  deep links. */
export const WithLineageAndColumnLinks: Story = {
  args: {
    getLineageHref: (uniqueId) => `#/lineage?select=${uniqueId}`,
    getColumnHref: (uniqueId, column) => `#/resource/${uniqueId}?column=${column}`,
  },
};
