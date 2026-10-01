import type { Meta, StoryObj } from '@storybook/react-vite';

import { storySavedQuery } from '../testing/storyFixtures';
import { SavedQueryParamsView } from './SavedQueryParamsView';

const meta: Meta<typeof SavedQueryParamsView> = {
  component: SavedQueryParamsView,
  args: { params: storySavedQuery().queryParams },
  decorators: [(Story) => <div className="max-w-2xl">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof SavedQueryParamsView>;

/** Each populated segment is its own collapsible row; `Where` renders its entries as
 *  code, the others as plain text. Click a row to expand it. */
export const Default: Story = {};

/** Empty segments are not rendered at all, so a metrics-only query is a single row.
 *  The limit row is never collapsible — there is nothing to reveal. */
export const MetricsOnly: Story = {
  args: {
    params: { metrics: ['revenue'], groupBy: [], where: [] },
  },
};

export const WithLimitOnly: Story = {
  args: {
    params: { metrics: [], groupBy: [], where: [], limit: 100 },
  },
};

export const AllSegments: Story = {
  args: {
    params: {
      metrics: ['revenue', 'order_count', 'aov'],
      groupBy: ['metric_time__week', 'customer__region', 'order__status'],
      where: [
        "{{ Dimension('order__status') }} = 'completed'",
        "{{ TimeDimension('metric_time', 'day') }} >= '2026-01-01'",
      ],
      orderBy: ['metric_time__week', 'customer__region'],
      limit: 5000,
    },
  },
};

/** A params object with every field empty takes the same "no parameters" branch as
 *  `null` — an empty saved query should not render an empty shell. */
export const AllSegmentsEmpty: Story = {
  args: { params: { metrics: [], groupBy: [], where: [], orderBy: [], limit: null } },
};

export const NoParams: Story = {
  args: { params: null },
};
