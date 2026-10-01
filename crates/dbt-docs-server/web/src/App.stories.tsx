import type { Meta, StoryObj } from '@storybook/react-vite';

import App from './App';
import { storyCapabilities } from './shared/testing/storyFixtures';
import {
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
  storyDataSource,
} from './shared/testing/storySources';

const meta: Meta<typeof App> = {
  component: App,
  // The app owns the viewport: topbar, resizable sidebar, main area, peek drawer.
  parameters: { layout: 'fullscreen' },
};

export default meta;
type Story = StoryObj<typeof App>;

/**
 * The whole shell, with every capability on and every artifact present.
 *
 * Telemetry stays off here without any special wiring — `useIdentity` reads consent
 * from `window.__DBT_DOCS__`, which Storybook does not inject, and an unreadable
 * bootstrap resolves to consent-denied. The site fails closed, so browsing these
 * stories emits nothing.
 */
export const Home: Story = {};

/**
 * A source reporting no column lineage, no query history and no health signals. This
 * is the common path for a plain `dbt docs generate`, so it has to look deliberate
 * rather than stripped.
 */
export const CoreTier: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchCapabilities: async () => storyCapabilities({ hasColumnLineage: false }),
        fetchDistribution: async () => ({ isProprietary: false, isLoggedIn: false }),
        fetchColumnLineage: async () => ({ kind: 'gated' }),
      }),
    },
  },
};

/** dbt v2 installed but not logged in — the `dbt login` upsell state. */
export const FusionNotLoggedIn: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchCapabilities: async () => storyCapabilities({ hasColumnLineage: false }),
        fetchDistribution: async () => ({ isProprietary: true, isLoggedIn: false }),
        fetchColumnLineage: async () => ({ kind: 'gated' }),
      }),
    },
  },
};

export const ResourceList: Story = {
  parameters: { docsApp: { initialEntries: ['/list/model'] } },
};

export const ResourceDetail: Story = {
  parameters: {
    docsApp: { initialEntries: ['/details/model.jaffle_shop.customers'] },
  },
};

/** The `/search` route, which also switches the sidebar into Filter mode. */
export const SearchRoute: Story = {
  parameters: { docsApp: { initialEntries: ['/search'] } },
};

/** The file-tree sidebar mode, selected by `?view=files`. */
export const FileTreeMode: Story = {
  parameters: { docsApp: { initialEntries: ['/?view=files'] } },
};

/**
 * `/lineage` takes over the whole viewport — no topbar, no sidebar — because the DAG
 * needs the room. It is the one route the shell short-circuits for.
 */
export const FullLineageRoute: Story = {
  parameters: {
    docsApp: {
      initialEntries: ['/lineage?uniqueId=model.jaffle_shop.customers'],
    },
  },
};

/**
 * First paint. `project`, `capabilities`, `distribution` and the node list are the
 * blocking set, so until they land the shell is a topbar and a loading line — the
 * topbar renders immediately by design, so the page never looks broken.
 */
export const Loading: Story = {
  parameters: { docsApp: { source: loadingStorySource() } },
};

/** A failed read of the blocking set surfaces as a server error inside the shell
 *  rather than a blank page. */
export const LoadError: Story = {
  parameters: { docsApp: { source: failingStorySource() } },
};

/**
 * A source implementing only `fetchAsset`. The blocking set never resolves, so this
 * sits in the loading state — worth pinning, because it is the shape a partially
 * ported data source would take.
 */
export const MinimalSource: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
};

/**
 * Files, facets and counts are best-effort rather than blocking: they default to
 * empty and the chrome still renders. The sidebar loses its per-type counts and the
 * file tree is empty, but nothing else changes.
 */
export const WithoutBestEffortData: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchFiles: async () => {
          throw new Error('no files artifact');
        },
        fetchSearchFacets: async () => {
          throw new Error('no facets');
        },
        fetchAssetCounts: async () => {
          throw new Error('no counts');
        },
      }),
    },
  },
};
