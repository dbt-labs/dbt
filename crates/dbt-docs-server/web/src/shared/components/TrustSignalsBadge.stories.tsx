import type { Meta, StoryObj } from '@storybook/react-vite';

import {
  ExposureHealthIssueType,
  HealthIssueType,
  SourceHealthIssueType,
} from '../typings/discoveryEnums';
import { TrustSignalsBadgeContainer } from './TrustSignalsBadge';

const meta: Meta<typeof TrustSignalsBadgeContainer> = {
  component: TrustSignalsBadgeContainer,
  args: {
    resourceType: 'model',
    trustSignals: { healthIssues: [] },
  },
  // Room below for the hover popover, which is where the actual detail lives.
  decorators: [(Story) => <div className="p-4 pb-64">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof TrustSignalsBadgeContainer>;

/** No issues resolves to healthy. Hover the badge for the breakdown. */
export const ModelHealthy: Story = {};

/** Caution-tier issues only. */
export const ModelCaution: Story = {
  args: {
    trustSignals: {
      healthIssues: [HealthIssueType.NoTests, HealthIssueType.NoDescription],
    },
  },
};

/** A single degraded issue pulls the whole state to degraded. */
export const ModelDegraded: Story = {
  args: {
    trustSignals: {
      healthIssues: [HealthIssueType.FailedTest, HealthIssueType.NoDescription],
    },
  },
};

/** `Unknown` outranks everything: no data at all is not the same as bad data. */
export const ModelUnknown: Story = {
  args: {
    trustSignals: {
      healthIssues: [HealthIssueType.Unknown, HealthIssueType.FailedTest],
    },
  },
};

/**
 * Model staleness is deliberately dropped: with state-aware orchestration a model can
 * go 30+ days without a rebuild and still be perfectly healthy. So this renders as
 * healthy despite carrying a `Stale` issue.
 */
export const ModelStaleIsIgnored: Story = {
  args: { trustSignals: { healthIssues: [HealthIssueType.Stale] } },
};

/** With a degraded upstream present, the caution counterpart is suppressed rather
 *  than listed alongside it. Hover to confirm only one upstream line appears. */
export const ModelUpstreamSuppression: Story = {
  args: {
    trustSignals: {
      healthIssues: [
        HealthIssueType.DegradedUpstreamSources,
        HealthIssueType.CautionUpstreamSources,
      ],
    },
  },
};

/** `NoTests` suppresses the failed/warned-test lines — they would be nonsense next to
 *  "has no tests". */
export const ModelNoTestsSuppressesTestResults: Story = {
  args: {
    trustSignals: {
      healthIssues: [
        HealthIssueType.NoTests,
        HealthIssueType.FailedTest,
        HealthIssueType.WarnedTest,
      ],
    },
  },
};

/** Sources have their own vocabulary — freshness rather than tests. Unlike models,
 *  `Stale` *is* a signal here, which is why the model filter is model-only. */
export const SourceHealthy: Story = {
  args: { resourceType: 'source', trustSignals: { healthIssues: [] } },
};

export const SourceFreshnessError: Story = {
  args: {
    resourceType: 'source',
    trustSignals: { healthIssues: [SourceHealthIssueType.FreshnessError] },
  },
};

export const SourceStale: Story = {
  args: {
    resourceType: 'source',
    trustSignals: { healthIssues: [SourceHealthIssueType.Stale] },
  },
};

/** `freshnessChecked: null` means freshness is intentionally not tracked, which
 *  suppresses the freshness success lines. */
export const SourceFreshnessIgnored: Story = {
  args: {
    resourceType: 'source',
    trustSignals: {
      healthIssues: [],
      additionalMetadata: { freshnessChecked: null },
    },
  },
};

/** Exposures report on their upstream models rather than on themselves. */
export const ExposureHealthy: Story = {
  args: { resourceType: 'exposure', trustSignals: { healthIssues: [] } },
};

export const ExposureDegraded: Story = {
  args: {
    resourceType: 'exposure',
    trustSignals: {
      healthIssues: [ExposureHealthIssueType.FailedTestUpstreamModels],
    },
  },
};

/** Only model, source and exposure are supported; anything else renders nothing. */
export const UnsupportedResourceType: Story = {
  args: { resourceType: 'macro', trustSignals: { healthIssues: [] } },
};

/** `healthIssues: undefined` means "not loaded yet" and renders nothing on the model
 *  and source paths — distinct from `[]`, which means "checked, all clear". */
export const NotLoadedRendersNothing: Story = {
  args: { trustSignals: { healthIssues: undefined } },
};

/** The badge can carry its state as text, for surfaces with room for it. */
export const WithStateHeader: Story = {
  args: {
    shouldRenderStateHeader: true,
    trustSignals: { healthIssues: [HealthIssueType.NoTests] },
  },
};

/** Icon only, no popover — used in dense table rows where a hover card would fight
 *  the row's own tooltip. */
export const WithoutPopover: Story = {
  args: { showPopover: false },
};

export const Sizes: Story = {
  render: () => (
    <div className="flex items-center gap-4">
      {(['sm', 'md', 'lg'] as const).map((size) => (
        <TrustSignalsBadgeContainer
          key={size}
          resourceType="model"
          size={size}
          shouldRenderStateHeader
          trustSignals={{ healthIssues: [HealthIssueType.NoTests] }}
        />
      ))}
    </div>
  ),
};
