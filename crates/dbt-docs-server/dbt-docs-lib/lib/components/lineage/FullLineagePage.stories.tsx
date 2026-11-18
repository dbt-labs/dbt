import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';

import {
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
} from '../../shared/testing/storySources';
import FullLineagePage, { type FullLineagePageProps } from './FullLineagePage';

const ROOT = 'model.jaffle_shop.customers';

/** Keeps the page's state the way the docs app's route does, so clicking a node or a
 *  groups pill updates the story instead of only logging the callback. */
function Stateful(props: FullLineagePageProps) {
  const [rootUniqueId, setRootUniqueId] = useState(props.rootUniqueId);
  const [panelId, setPanelId] = useState(props.panelId);
  const [tab, setTab] = useState(props.tab);
  return (
    <FullLineagePage
      rootUniqueId={rootUniqueId}
      panelId={panelId}
      tab={tab}
      onRootChange={(uniqueId) => {
        setRootUniqueId(uniqueId);
        props.onRootChange(uniqueId);
      }}
      onPanelChange={(uniqueId) => {
        setPanelId(uniqueId);
        props.onPanelChange(uniqueId);
      }}
      onTabChange={(next) => {
        setTab(next);
        props.onTabChange(next);
      }}
    />
  );
}

const meta: Meta<typeof FullLineagePage> = {
  component: FullLineagePage,
  // Keyed so editing a control restarts from the new value.
  render: (args) => (
    <Stateful key={`${args.rootUniqueId}|${args.panelId}|${args.tab}`} {...args} />
  ),
  args: {
    rootUniqueId: ROOT,
    panelId: null,
    tab: null,
    onRootChange: fn(),
    onPanelChange: fn(),
    onTabChange: fn(),
  },
  // The page is `h-screen w-screen` — it owns the viewport, so it should not be
  // padded into a story canvas.
  parameters: { layout: 'fullscreen' },
};

export default meta;
type Story = StoryObj<typeof FullLineagePage>;

/** Three hops each way, with zoom controls, a materialization lens, and per-node
 *  context menus. */
export const Default: Story = {};

/**
 * With no root there is nothing to draw, so the page renders the empty state
 * pointing at selector syntax. This is what a bare `#/lineage` link lands on.
 */
export const EmptyState: Story = {
  args: { rootUniqueId: '' },
};

/**
 * `panelId` drives the slide-in detail panel, and the canvas insets by 450px to make
 * room. The docs app keeps it in the URL, which is what makes a lineage view with a
 * node selected shareable.
 */
export const WithPanelOpen: Story = {
  args: { panelId: ROOT },
};

export const Loading: Story = {
  parameters: { docsApp: { source: loadingStorySource() } },
};

export const LoadError: Story = {
  parameters: { docsApp: { source: failingStorySource('lineage read failed') } },
};

/** A source with no `fetchLineage`. Note the unsupported message only appears when
 *  there is a root to draw — without one, the empty state wins. */
export const UnsupportedSurface: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
};

/** Rooted on a source rather than a model, so the graph is downstream-heavy. */
export const RootedOnASource: Story = {
  args: { rootUniqueId: 'source.jaffle_shop.raw.customers' },
};
