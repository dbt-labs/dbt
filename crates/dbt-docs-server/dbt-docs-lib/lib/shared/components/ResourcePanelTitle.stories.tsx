import type { Meta, StoryObj } from '@storybook/react-vite';

import { HealthIssueType } from '../typings/discoveryEnums';
import { ResourcePanelTitle } from './ResourcePanelTitle';

const meta: Meta<typeof ResourcePanelTitle> = {
  component: ResourcePanelTitle,
  args: {
    name: 'customers',
    packageName: 'jaffle_shop',
    resourceType: 'model',
  },
  decorators: [(Story) => <div className="w-[420px] p-4">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof ResourcePanelTitle>;

export const Default: Story = {};

/** A recognised access level adds the padlock/globe icon before the name. */
export const WithAccessLevel: Story = {
  args: { access: 'public' },
};

export const AllAccessLevels: Story = {
  render: () => (
    <div className="space-y-4">
      {['public', 'protected', 'private'].map((access) => (
        <ResourcePanelTitle
          key={access}
          name={`customers (${access})`}
          packageName="jaffle_shop"
          resourceType="model"
          access={access}
        />
      ))}
    </div>
  ),
};

/** An unrecognised access string resolves to no icon rather than a broken one. */
export const UnknownAccessLevel: Story = {
  args: { access: 'something_else' },
};

/** Trust signals only render for model, source and exposure — the types
 *  `trustSignalsSupportedResourceTypes` allows. */
export const WithTrustSignals: Story = {
  args: {
    trustSignals: { healthIssues: [HealthIssueType.NoDescription] },
  },
};

/** Same props on an unsupported type: the badge is suppressed, not empty. */
export const TrustSignalsOnUnsupportedType: Story = {
  args: {
    resourceType: 'macro',
    trustSignals: { healthIssues: [HealthIssueType.NoDescription] },
  },
};

/** Long names truncate with a tooltip; the package line below does not. */
export const LongName: Story = {
  args: {
    name: 'int_order_items_joined_to_customers_and_products_and_locations',
  },
};

/** Both name and package are optional at the type level — a panel opened before its
 *  fetch resolves passes neither. */
export const WithoutNameOrPackage: Story = {
  args: { name: undefined, packageName: null },
};
