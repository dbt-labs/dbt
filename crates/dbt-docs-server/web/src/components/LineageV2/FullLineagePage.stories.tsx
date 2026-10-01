import type { Meta, StoryObj } from '@storybook/react-vite';

import {
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
} from '../../shared/testing/storySources';
import FullLineagePage from './FullLineagePage';

const ROOT = 'model.jaffle_shop.customers';

const meta: Meta<typeof FullLineagePage> = {
  component: FullLineagePage,
  // The page is `h-screen w-screen` — it owns the viewport, so it should not be
  // padded into a story canvas.
  parameters: {
    layout: 'fullscreen',
    docsApp: { initialEntries: [`/lineage?uniqueId=${ROOT}`] },
  },
};

export default meta;
type Story = StoryObj<typeof FullLineagePage>;

/** Three hops each way, with zoom controls, a materialization lens, and per-node
 *  context menus. */
export const Default: Story = {};

/**
 * With no `uniqueId` there is nothing to draw, so the page renders the empty state
 * pointing at selector syntax. This is what a bare `#/lineage` link lands on.
 */
export const EmptyState: Story = {
  parameters: { docsApp: { initialEntries: ['/lineage'] } },
};

/**
 * The `panel` search param drives the slide-in detail panel, and the canvas insets by
 * 450px to make room. Keeping it in the URL is what makes a lineage view with a node
 * selected shareable.
 */
export const WithPanelOpen: Story = {
  parameters: {
    docsApp: {
      initialEntries: [`/lineage?uniqueId=${ROOT}&panel=${ROOT}`],
    },
  },
};

export const Loading: Story = {
  parameters: {
    docsApp: {
      source: loadingStorySource(),
      initialEntries: [`/lineage?uniqueId=${ROOT}`],
    },
  },
};

export const LoadError: Story = {
  parameters: {
    docsApp: {
      source: failingStorySource('lineage read failed'),
      initialEntries: [`/lineage?uniqueId=${ROOT}`],
    },
  },
};

/** A source with no `fetchLineage`. Note the unsupported message only appears when
 *  there is a root to draw — without one, the empty state wins. */
export const UnsupportedSurface: Story = {
  parameters: {
    docsApp: {
      source: minimalStorySource(),
      initialEntries: [`/lineage?uniqueId=${ROOT}`],
    },
  },
};

/** Rooted on a source rather than a model, so the graph is downstream-heavy. */
export const RootedOnASource: Story = {
  parameters: {
    docsApp: {
      initialEntries: ['/lineage?uniqueId=source.jaffle_shop.raw.customers'],
    },
  },
};
