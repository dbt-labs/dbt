import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, within } from 'storybook/test';

import type { AssetFilters } from '../App';
import { makeFakeProject } from '../shared';
import { storyNodes, storySearchHits } from '../shared/testing/storyFixtures';
import {
  emptyStorySource,
  loadingStorySource,
  minimalStorySource,
  storyDataSource,
} from '../shared/testing/storySources';
import Search from './Search';

const NO_FILTERS: AssetFilters = {
  resourceType: [],
  modelingLayer: [],
  materialization: [],
  pkg: [],
  tag: [],
};

const meta: Meta<typeof Search> = {
  component: Search,
  args: {
    project: makeFakeProject(),
    nodes: storyNodes(),
    query: 'customer',
    filters: NO_FILTERS,
    previewId: null,
    onSetFilters: () => {},
    onPeek: () => {},
  },
};

export default meta;
type Story = StoryObj<typeof Search>;

/** Cross-type search results — the hits span models, a source, a test and an
 *  exposure, each with a different matched field. */
export const Default: Story = {};

/** Active filters render as removable pills above the results. */
export const WithFilters: Story = {
  args: {
    filters: { ...NO_FILTERS, resourceType: ['model'], tag: ['daily'] },
  },
};

/** More results than one page, so the load-more footer appears. */
export const ManyResults: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchSearch: async () => ({
          kind: 'ok',
          page: {
            items: Array.from({ length: 50 }, (_, i) => {
              const hits = storySearchHits();
              const hit = hits[i % hits.length]!;
              return { ...hit, uniqueId: `${hit.uniqueId}_${i}` };
            }),
            nextCursor: '50',
            totalCount: 137,
          },
        }),
      }),
    },
  },
};

/** A query that matched nothing. */
export const NoResults: Story = {
  parameters: { docsApp: { source: emptyStorySource() } },
};

/** No query typed yet. */
export const EmptyQuery: Story = {
  args: { query: '' },
};

export const Loading: Story = {
  parameters: { docsApp: { source: loadingStorySource() } },
};

/**
 * A structured rejection. The search contract returns
 * `{ kind: 'error', code, message }` rather than throwing, precisely so the four
 * documented codes can be turned into actionable copy instead of a stack trace.
 */
export const QueryTooLong: Story = {
  args: { query: 'x'.repeat(400) },
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchSearch: async () => ({
          kind: 'error',
          code: 'query_too_long',
          message: 'query exceeds maximum length',
        }),
      }),
    },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // The actionable copy replaces the raw server message, which is the point of
    // mapping the codes at all.
    await expect(
      await canvas.findByText(/Your search query is too long/),
    ).toBeInTheDocument();
    await expect(canvas.queryByText('query exceeds maximum length')).toBeNull();
  },
};

export const InvalidTypeFilter: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchSearch: async () => ({
          kind: 'error',
          code: 'invalid_type',
          message: 'unrecognised resource type',
        }),
      }),
    },
  },
};

export const ExpiredCursor: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchSearch: async () => ({
          kind: 'error',
          code: 'invalid_cursor',
          message: 'cursor expired',
        }),
      }),
    },
  },
};

/** An unrecognised code falls through to the raw message, so a new server-side code
 *  still gives the reader something rather than nothing. */
export const UnknownErrorCode: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchSearch: async () => ({
          kind: 'error',
          code: 'something_new',
          message: 'The search index is being rebuilt. Try again shortly.',
        }),
      }),
    },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // Falls through to the raw message rather than swallowing it, so a code added
    // server-side still tells the reader something.
    await expect(
      await canvas.findByText(/The search index is being rebuilt/),
    ).toBeInTheDocument();
  },
};

/** A source with no `fetchSearch` at all. */
export const UnsupportedSurface: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
};
