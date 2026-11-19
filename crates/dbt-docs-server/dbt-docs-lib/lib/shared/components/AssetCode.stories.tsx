import type { Meta, StoryObj } from '@storybook/react-vite';

import { storyModel } from '../testing/storyFixtures';
import { AssetCode } from './AssetCode';

const model = storyModel();

const meta: Meta<typeof AssetCode> = {
  component: AssetCode,
  args: { rawCode: model.rawCode, compiledCode: model.compiledCode },
};

export default meta;
type Story = StoryObj<typeof AssetCode>;

/** Both tabs available: the authored Jinja and the compiled SQL. */
export const Default: Story = {};

/** Without compiled code the preview has nothing to toggle to — an index written
 *  without a successful compile. */
export const RawOnly: Story = {
  args: { compiledCode: null },
};

/** Renders nothing at all when there is no raw code, so a resource type with no code
 *  (a group, an exposure) does not get an empty card. */
export const NoCodeRendersNothing: Story = {
  args: { rawCode: null },
};

export const Python: Story = {
  args: {
    rawCode:
      'def model(dbt, session):\n' +
      '    dbt.config(materialized="table")\n' +
      '    orders = dbt.ref("stg_orders")\n' +
      '    return orders.groupby("customer_id").agg({"amount": "sum"})',
    compiledCode: null,
  },
};
