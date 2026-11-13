import type { Meta, StoryObj } from '@storybook/react-vite';

import {
  type SharedUpstreamSource,
  UpstreamSourcesSection,
} from './UpstreamSourcesSection';

function source(name: string, freshnessStatus: string | null): SharedUpstreamSource {
  return { name, uniqueId: `source.jaffle_shop.${name}`, freshnessStatus };
}

const details = (
  <div className="border-t border-borderMuted p-4 text-sm text-fgAlt">
    Per-source freshness rows go here.
  </div>
);

const meta: Meta<typeof UpstreamSourcesSection> = {
  component: UpstreamSourcesSection,
  args: {
    sources: [source('raw.customers', 'pass'), source('raw.orders', 'pass')],
    toExpand: details,
  },
  decorators: [
    (Story) => (
      <div className="w-[560px] rounded-md border border-borderMuted">{Story()}</div>
    ),
  ],
};

export default meta;
type Story = StoryObj<typeof UpstreamSourcesSection>;

/** All fresh. The row rolls the upstreams up to their *worst* status. */
export const AllFresh: Story = {};

/** One error drags the whole roll-up to error, however many sources are passing. */
export const OneError: Story = {
  args: {
    sources: [
      source('raw.customers', 'pass'),
      source('raw.orders', 'error'),
      source('raw.products', 'warn'),
    ],
  },
};

export const OneWarning: Story = {
  args: {
    sources: [source('raw.customers', 'pass'), source('raw.orders', 'warn')],
  },
};

/** `unconfigured` and `outdated` share a severity rank — both mean "freshness is not
 *  telling us anything useful", which is a caution rather than a failure. */
export const UnconfiguredAndOutdated: Story = {
  args: {
    sources: [
      source('raw.customers', 'unconfigured'),
      source('raw.orders', 'outdated'),
      source('raw.products', 'pass'),
    ],
  },
};

/** A null status is treated as `unknown` rather than skipped. */
export const NullStatus: Story = {
  args: {
    sources: [source('raw.customers', null), source('raw.orders', 'pass')],
  },
};

/** Sorted worst-first, then alphabetically. Expand to check it. */
export const SortedBySeverity: Story = {
  args: {
    sources: [
      source('zzz_passing', 'pass'),
      source('aaa_skipped', 'skipped'),
      source('mmm_warning', 'warn'),
      source('bbb_error', 'error'),
      source('aaa_outdated', 'outdated'),
    ],
  },
};

export const InitiallyOpen: Story = {
  args: { isOpen: true },
};

export const NotExpandable: Story = {
  args: { toExpand: undefined },
};

/** An empty list renders nothing — a model with no sources gets no row at all, rather
 *  than a row claiming "no upstream sources". Same for `undefined`. */
export const NoSourcesRendersNothing: Story = {
  args: { sources: [] },
};

export const UndefinedRendersNothing: Story = {
  args: { sources: undefined },
};
