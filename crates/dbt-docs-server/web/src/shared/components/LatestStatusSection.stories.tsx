import type { Meta, StoryObj } from '@storybook/react-vite';
import { BadgeCheck } from 'lucide-react';

import {
  freshnessStatusResultIconMap,
  LatestStatusSectionDisplay,
  ResourceStatusResult,
  runStatusResultIconMap,
} from './LatestStatusSection';

const meta: Meta<typeof LatestStatusSectionDisplay> = {
  component: LatestStatusSectionDisplay,
  args: {
    header: 'Build succeeded',
    status: ResourceStatusResult.pass,
    statusIcon: BadgeCheck,
    tooltip: 'The most recent build of this model succeeded.',
    checkCompletedAt: 'Feb 11, 2026, 4:12 PM',
    checkCompletedAtUtc: '2026-02-11 16:12:18 UTC',
  },
  decorators: [(Story) => <div className="max-w-2xl">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof LatestStatusSectionDisplay>;

/** Section heading, an "as of" timestamp beside it, and one status row. */
export const Default: Story = {};

/** `viewRunUrl` adds a right-aligned external link out to the run. */
export const WithViewRunLink: Story = {
  args: { viewRunUrl: 'https://cloud.getdbt.com/deploy/1/runs/4821' },
};

/** Every run status, each with the icon `runStatusResultIconMap` assigns it. */
export const AllRunStatuses: Story = {
  render: () => (
    <div className="space-y-8">
      {(
        [
          [ResourceStatusResult.pass, 'Build succeeded'],
          [ResourceStatusResult.warn, 'Build warned'],
          [ResourceStatusResult.error, 'Build failed'],
          [ResourceStatusResult.skipped, 'Build skipped'],
          [ResourceStatusResult.reused, 'Build reused'],
          [ResourceStatusResult.unknown, 'Build status unknown'],
        ] as const
      ).map(([status, header]) => (
        <LatestStatusSectionDisplay
          key={status}
          header={header}
          status={status}
          statusIcon={runStatusResultIconMap[status]}
          tooltip={header}
        />
      ))}
    </div>
  ),
};

/** The freshness vocabulary uses a different icon set for the same result codes —
 *  which is the reason there are two maps rather than one. */
export const AllFreshnessStatuses: Story = {
  render: () => (
    <div className="space-y-8">
      {(
        [
          [ResourceStatusResult.pass, 'Source is fresh'],
          [ResourceStatusResult.warn, 'Source is stale'],
          [ResourceStatusResult.error, 'Freshness check failed'],
          [ResourceStatusResult.unknown, 'Freshness not configured'],
        ] as const
      ).map(([status, header]) => (
        <LatestStatusSectionDisplay
          key={status}
          header={header}
          status={status}
          statusIcon={freshnessStatusResultIconMap[status]}
          tooltip={header}
        />
      ))}
    </div>
  ),
};

/** `children` are appended inside the same bordered box, below the status row — where
 *  the test-results rows go on a model page. */
export const WithChildren: Story = {
  args: {
    children: (
      <div className="p-4 text-sm text-fgAlt">
        4 tests passed, 1 warned on this model.
      </div>
    ),
  },
};

/** Without a timestamp the "as of" accessory disappears but the heading stays. */
export const WithoutTimestamp: Story = {
  args: { checkCompletedAt: undefined, checkCompletedAtUtc: undefined },
};

/** `shouldIndent` aligns the row with expandable siblings when it sits in a list that
 *  has carets. */
export const Indented: Story = {
  args: { shouldIndent: true },
};
