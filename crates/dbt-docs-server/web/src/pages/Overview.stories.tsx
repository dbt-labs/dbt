import type { Meta, StoryObj } from '@storybook/react-vite';

import { storyOverview } from '../shared/testing/storyFixtures';
import {
  failingStorySource,
  loadingStorySource,
  minimalStorySource,
  storyDataSource,
} from '../shared/testing/storySources';
import Overview from './Overview';

const meta: Meta<typeof Overview> = {
  component: Overview,
};

export default meta;
type Story = StoryObj<typeof Overview>;

/** A project that authored a `{% docs __overview__ %}` block. */
export const Authored: Story = {};

/**
 * No authored block. The dashboard default renders — icon cards, copyable commands,
 * and live resource counts. This is the most common landing page in practice.
 */
export const UnconfiguredDefault: Story = {
  parameters: {
    docsApp: { source: storyDataSource({ fetchOverview: async () => null }) },
  },
};

/**
 * A whitespace-only block also falls through to the default rather than rendering an
 * empty page.
 */
export const BlankAuthoredBlock: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchOverview: async () => storyOverview({ blockContents: '   \n\n ' }),
      }),
    },
  },
};

/**
 * A failed read falls through to the dashboard default too: an unreadable `dbt.docs`
 * must not blank the landing page, and the built-in dashboard is a correct answer
 * rather than a degraded one.
 */
export const ReadErrorFallsBackToDefault: Story = {
  parameters: { docsApp: { source: failingStorySource() } },
};

/**
 * Deliberately a spinner rather than the default while pending — flashing the built-in
 * copy and then swapping it for the project's own is worse than a brief wait.
 */
export const Loading: Story = {
  parameters: { docsApp: { source: loadingStorySource() } },
};

/** A source with no `fetchOverview` at all skips the pending state entirely and goes
 *  straight to the default. */
export const UnsupportedSurface: Story = {
  parameters: { docsApp: { source: minimalStorySource() } },
};

/** A long authored overview, to check the 768px measure and the heading rhythm. */
export const LongAuthoredOverview: Story = {
  parameters: {
    docsApp: {
      source: storyDataSource({
        fetchOverview: async () =>
          storyOverview({
            blockContents: [
              '# Jaffle Shop',
              '',
              'The canonical example project.',
              '',
              '## Layers',
              '',
              '### Staging',
              '',
              'One model per source table, materialized as views.',
              '',
              '### Marts',
              '',
              '- `customers` — one row per customer',
              '- `orders` — one row per order',
              '',
              '#### Conventions',
              '',
              '1. Staging models are prefixed `stg_`',
              '2. Marts are tables',
              '',
              '| Layer | Materialization |',
              '| --- | --- |',
              '| staging | view |',
              '| marts | table |',
              '',
              '```sql',
              "select * from {{ ref('customers') }}",
              '```',
              '',
              '> Sources refresh hourly.',
              '',
              'See [the runbook](https://example.com/runbook).',
            ].join('\n'),
          }),
      }),
    },
  },
};
