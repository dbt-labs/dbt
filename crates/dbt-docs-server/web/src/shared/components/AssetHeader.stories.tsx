import type { Meta, StoryObj } from '@storybook/react-vite';
import { Info, Key, Waypoints } from 'lucide-react';

import { assetToHeaderProps } from '../mappers/assetToHeaderProps';
import { storyExposure, storyModel, storySource } from '../testing/storyFixtures';
import { AssetHeader } from './AssetHeader';

const meta: Meta<typeof AssetHeader> = {
  component: AssetHeader,
  args: assetToHeaderProps(storyModel()),
};

export default meta;
type Story = StoryObj<typeof AssetHeader>;

/** Breadcrumbs, the dbt chip and the name. Built through `assetToHeaderProps`, the
 *  same mapper the detail page uses, so the story cannot drift from it. */
export const Default: Story = {};

/** `headerIcons` is the row of facts under the heading. `Contents` beats `text` when
 *  both are set, and a `href` wraps the pair in a link. */
export const WithHeaderIcons: Story = {
  args: {
    headerIcons: [
      { icon: <Key className="size-3 align-middle" />, text: 'customer_id' },
      {
        icon: <Waypoints className="size-3 align-middle" />,
        text: '2 upstream',
        tooltip: 'Direct parents',
      },
      {
        icon: <Info className="size-3 align-middle" />,
        text: 'Contract enforced',
        href: 'https://docs.getdbt.com/docs/collaborate/govern/model-contracts',
      },
    ],
  },
};

export const WithLastRunLabel: Story = {
  args: { lastRunLabel: 'Last run: Feb 11, 2026 at 4:12 PM · succeeded in 18s' },
};

/** `headingOverride` replaces the title only — the breadcrumb still uses `name`, which
 *  is the point: a version dropdown must not rewrite the trail. */
export const WithHeadingOverride: Story = {
  args: {
    headingOverride: (
      <span className="flex items-baseline gap-2">
        customers
        <span className="text-base text-fgDecorative">v3</span>
      </span>
    ),
  },
};

/** `actions` pins to the right of the heading row. */
export const WithActions: Story = {
  args: {
    actions: (
      <button
        type="button"
        className="rounded border border-borderMuted px-3 py-1.5 text-sm text-fgMain"
      >
        View lineage
      </button>
    ),
  },
};

export const Source: Story = {
  args: assetToHeaderProps(storySource()),
};

export const Exposure: Story = {
  args: assetToHeaderProps(storyExposure()),
};

/** A long name plus a long package: the heading truncates and the breadcrumb trail
 *  collapses. Narrow container so both actually trigger. */
export const LongNameTruncates: Story = {
  args: {
    ...assetToHeaderProps(
      storyModel({
        name: 'int_order_items_joined_to_customers_and_products_and_locations',
        packageName: 'jaffle_shop_analytics_platform',
      }),
    ),
  },
  decorators: [(Story) => <div className="w-[420px]">{Story()}</div>],
};

/** No package name falls back to an em dash rather than an empty crumb. */
export const WithoutPackage: Story = {
  args: { packageName: null },
};
