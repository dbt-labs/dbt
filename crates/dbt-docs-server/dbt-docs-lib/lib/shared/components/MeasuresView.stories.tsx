import type { Meta, StoryObj } from '@storybook/react-vite';

import { storySemanticModel } from '../testing/storyFixtures';
import { MeasuresView } from './MeasuresView';

const meta: Meta<typeof MeasuresView> = {
  component: MeasuresView,
  args: { measures: storySemanticModel().measures },
};

export default meta;
type Story = StoryObj<typeof MeasuresView>;

/** The aggregation (`sum`, `count`) takes the type-chip slot that `DimensionsView`
 *  uses for the dimension type. */
export const Default: Story = {};

export const ManyAggregations: Story = {
  args: {
    measures: (
      ['sum', 'count', 'count_distinct', 'average', 'min', 'max', 'median'] as const
    ).map((agg) => ({
      name: `${agg}_of_amount`,
      agg,
      expr: 'amount',
      description: `The ${agg} of order amount.`,
    })),
  },
};

export const NoMeasures: Story = {
  args: { measures: [] },
};
