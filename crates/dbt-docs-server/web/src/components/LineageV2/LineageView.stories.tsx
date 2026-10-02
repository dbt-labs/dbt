import { Route, Routes } from 'react-router-dom';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import { paths, ROUTES } from '../../routes';
import { storyLineage } from '../../shared/testing/storyFixtures';
import {
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
  storyDataSource,
} from '../../shared/testing/storySources';
import { useLineageStore } from '../../stores/lineageStore';
import FullLineagePage from './FullLineagePage';
import { LineageView } from './LineageView';

const meta: Meta<typeof LineageView> = {
  component: LineageView,
  args: {
    rootUniqueId: 'model.jaffle_shop.customers',
    modelName: 'customers',
  },
  // The DAG canvas is absolutely positioned inside `.lineage-frame`, so it needs a
  // sized parent to render into at all.
  decorators: [(Story) => <div className="h-[520px] w-full">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof LineageView>;

/** The inline lineage panel on a resource detail page: one hop each way, with a
 *  toolbar showing the selector and a fullscreen escape hatch. */
export const Default: Story = {};

/** A root with no edges at all falls through to `NoLineageFallback`, which offers the
 *  command that would populate lineage — the expected path for an index written
 *  without it, not an error. */
export const NoLineage: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchLineage: async () => ({
          nodes: [
            {
              uniqueId: 'model.jaffle_shop.customers',
              name: 'customers',
              resourceType: 'model',
              description: null,
              packageName: 'jaffle_shop',
              tags: [],
            },
          ],
          edges: [],
        }),
      }),
    },
  },
};

/** A wider graph, to check the layout and zoom controls with more than a handful of
 *  nodes. */
export const WideGraph: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchLineage: async () => {
          const base = storyLineage();
          const extra = Array.from({ length: 10 }, (_, i) => ({
            uniqueId: `model.jaffle_shop.downstream_${i}`,
            name: `downstream_${i}`,
            resourceType: 'model' as const,
            description: null,
            packageName: 'jaffle_shop',
            tags: [],
            materialized: 'view',
          }));
          return {
            nodes: [...base.nodes, ...extra],
            edges: [
              ...base.edges,
              ...extra.map((n) => ({
                upstreamUniqueId: 'model.jaffle_shop.customers',
                downstreamUniqueId: n.uniqueId,
              })),
            ],
          };
        },
      }),
    },
  },
};

export const Loading: Story = {
  parameters: { docsApp: { source: loadingStorySource() } },
};

export const LoadError: Story = {
  parameters: { docsApp: { source: failingStorySource('lineage read failed') } },
};

/**
 * A source with no `fetchLineage` at all. Deliberately *not* the
 * `NoLineageFallback` copy: that would tell the reader to re-run with
 * `--write-lineage`, which would not help, because nothing is going to load.
 */
export const UnsupportedSurface: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
};

/** Expansion and closing remount the DAG for the same root. Its scope should stay
 *  intact in both directions, including the unlimited hop preset. */
export const PreservesHopsOnExpand: Story = {
  parameters: {
    docsApp: {
      initialEntries: [paths.details('model.jaffle_shop.customers')],
    },
  },
  beforeEach: () => {
    useLineageStore.getState().reset();
  },
  render: (args) => (
    <Routes>
      <Route path={ROUTES.details} element={<LineageView {...args} />} />
      <Route path={ROUTES.lineage} element={<FullLineagePage />} />
    </Routes>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const page = within(canvasElement.ownerDocument.body);
    await userEvent.click(await canvas.findByRole('button', { name: '1+' }));
    await userEvent.click(await page.findByRole('menuitemradio', { name: '3+' }));
    await userEvent.click(canvas.getByRole('button', { name: '+1' }));
    await userEvent.click(await page.findByRole('menuitemradio', { name: '+6' }));
    await userEvent.click(
      canvas.getByRole('button', { name: 'Open fullscreen lineage' }),
    );
    await canvas.findByRole('button', { name: 'Close full lineage' });
    await expect(canvas.getByRole('button', { name: '3+' })).toBeVisible();
    await expect(canvas.getByRole('button', { name: '+6' })).toBeVisible();

    await userEvent.click(canvas.getByRole('button', { name: '3+' }));
    await userEvent.click(await page.findByRole('menuitemradio', { name: 'max+' }));
    await userEvent.click(canvas.getByRole('button', { name: '+6' }));
    await userEvent.click(await page.findByRole('menuitemradio', { name: '+max' }));
    await userEvent.click(canvas.getByRole('button', { name: 'Close full lineage' }));
    await canvas.findByRole('button', { name: 'Open fullscreen lineage' });
    await expect(canvas.getByRole('button', { name: 'max+' })).toBeVisible();
    await expect(canvas.getByRole('button', { name: '+max' })).toBeVisible();
  },
};
