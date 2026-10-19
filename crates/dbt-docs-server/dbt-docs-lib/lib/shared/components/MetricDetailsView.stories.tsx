import type { Meta, StoryObj } from '@storybook/react-vite';

import { MetricDetailsView } from './MetricDetailsView';

const meta: Meta<typeof MetricDetailsView> = {
  component: MetricDetailsView,
  args: {
    metric: {
      type: 'simple',
      measure: 'order_total',
      filters: ["{{ Dimension('order__status') }} = 'completed'"],
    },
  },
  decorators: [(Story) => <div className="max-w-2xl">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof MetricDetailsView>;

/** A simple metric: a measure plus a filter. Rows with no value are omitted, so each
 *  metric kind below renders a different set. */
export const Simple: Story = {};

export const Ratio: Story = {
  args: {
    metric: {
      type: 'ratio',
      numerator: 'revenue',
      denominator: 'order_count',
    },
  },
};

/** Cumulative metrics are the only kind with a window, and it is formatted as
 *  `count granularity` rather than shown raw. */
export const Cumulative: Story = {
  args: {
    metric: {
      type: 'cumulative',
      measure: 'revenue',
      window: { count: 28, granularity: 'day' },
      grainToDate: 'month',
    },
  },
};

export const Derived: Story = {
  args: {
    metric: {
      type: 'derived',
      expression: 'revenue - cost',
    },
  },
};

export const MultipleFilters: Story = {
  args: {
    metric: {
      type: 'simple',
      measure: 'order_total',
      filters: [
        "{{ Dimension('order__status') }} = 'completed'",
        "{{ Dimension('customer__region') }} in ('EMEA', 'APAC')",
        "{{ TimeDimension('metric_time', 'day') }} >= '2026-01-01'",
      ],
    },
  },
};

/** No filters drops the whole second section rather than showing an empty one. */
export const WithoutFilters: Story = {
  args: { metric: { type: 'simple', measure: 'order_total' } },
};

/** A null metric, and a metric with no `type`, take the same "no details" branch —
 *  `type` is what makes the record meaningful. */
export const NoMetric: Story = {
  args: { metric: null },
};

export const MetricWithoutType: Story = {
  args: { metric: { type: '' } },
};
