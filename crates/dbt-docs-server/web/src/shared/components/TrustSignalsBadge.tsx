import { FC, useMemo } from 'react';
import { intersection } from 'lodash';
import { twMerge } from 'tailwind-merge';

import { Popover } from '../../components/ui/Popover';
import type { ResourceType } from '../../lib/resourceType';
import {
  ExposureHealthIssueType,
  HealthIssueType,
  SourceHealthIssueType,
} from '../typings/discoveryEnums';
import {
  HealthIssue,
  TrustSignals,
  trustSignalsSupportedResourceTypes,
} from '../typings/trustSignals';
import { toTitleCase } from '../util/string';
import {
  determineTrustState,
  generateMessages,
  TrustSignalMessage,
  TrustState,
} from '../util/trustSignals';
import { trustIconMap } from './constants';
import { TrustSignalDescription } from './TrustSignalDescription';

export { trustIconMap };

/** Matches sourdough's SizeType structurally (same string values) so it's
 *  still assignable everywhere that type was used -- TypeScript checks
 *  shape, not origin, for a plain string-literal union. */
type SourdoughSize = 'xs' | 'sm' | 'md' | 'lg' | 'xl' | '2xl' | '3xl' | '4xl';

type TrustSignalsBadgeDisplayProps = {
  trustState: TrustState;
  messages: TrustSignalMessage[];
  shouldRenderStateHeader: boolean;
  size?: SourdoughSize;
  className?: string;
  showPopover?: boolean;
};

const TrustSignalsBadgeDisplay: FC<TrustSignalsBadgeDisplayProps> = ({
  className,
  trustState,
  messages,
  shouldRenderStateHeader,
  size = 'md',
  showPopover = true,
}) => {
  const icon = trustIconMap(size)[trustState];
  if (showPopover) {
    return (
      <Popover
        labelledBy={trustState}
        content={
          <TrustSignalDescription
            trustState={trustState}
            messages={messages}
            size={size}
          />
        }
        zIndex={50}
      >
        <div
          className={twMerge('flex items-center', className)}
          data-testid="trust-signals-badge"
        >
          <span>{icon}</span>
          {shouldRenderStateHeader && (
            <span>
              <h3 className="font-body ml-2 text-fgMain">{toTitleCase(trustState)}</h3>
            </span>
          )}
        </div>
      </Popover>
    );
  }
  return <span>{icon}</span>;
};

export const errorStatuses: Record<
  HealthIssueType,
  { importance: number; message: string }
> = {
  // Model staleness is no longer surfaced — see `withoutStale` below
  Stale: { importance: -1, message: '' },
  CautionUpstreamSources: { importance: 0, message: 'Has stale upstream sources' },
  DegradedUpstreamSources: { importance: 0, message: 'Has degraded upstream sources' },
  LastRunFailed: {
    importance: 1,
    message: 'Was not built in its last run due to a failure',
  },
  NoTests: { importance: 2, message: 'Does not have any tests' },
  FailedTest: { importance: 3, message: 'At least one or more tests failed' },
  WarnedTest: { importance: 4, message: 'At least one or more tests warned' },
  NoDescription: { importance: 5, message: 'Is missing a description' },
  Unknown: {
    importance: -1,
    message: 'No data on this resource. To see health data, include it in a job run.',
  },
};

export const successStatuses: Record<
  HealthIssueType,
  { importance: number; message: string }
> = {
  // Model staleness is no longer surfaced — see `withoutStale` below
  Stale: { importance: -1, message: '' },
  CautionUpstreamSources: { importance: 0, message: 'No stale upstream sources' },
  DegradedUpstreamSources: { importance: 0, message: 'No degraded upstream sources' },
  LastRunFailed: {
    importance: 1,
    message: 'Successfully built or reused in its last run',
  },
  NoTests: { importance: 2, message: 'Has tests configured' },
  FailedTest: { importance: 3, message: 'Has no test failures' },
  WarnedTest: { importance: 4, message: 'Has no test warnings' },
  NoDescription: { importance: 5, message: 'Has a description' },
  Unknown: { importance: -1, message: '' },
};

export interface TrustSignalsBadgeParams {
  trustSignals: TrustSignals;
  shouldRenderStateHeader?: boolean;
  size?: SourdoughSize;
  className?: string;
  showPopover?: boolean;
}

export function TrustSignalsBadgeContainer({
  resourceType,
  trustSignals,
  shouldRenderStateHeader = false,
  size = 'md',
  className,
  showPopover = true,
}: TrustSignalsBadgeParams & { resourceType: ResourceType }) {
  if (!trustSignalsSupportedResourceTypes.includes(resourceType)) {
    return null;
  }
  if (resourceType === 'model') {
    return ModelTrustSignalsBadgeContainer({
      trustSignals,
      shouldRenderStateHeader,
      size,
      className,
      showPopover,
    });
  }
  if (resourceType === 'source') {
    return SourceTrustSignalsBadgeContainer({
      trustSignals,
      shouldRenderStateHeader,
      size,
      className,
      showPopover,
    });
  }
  return ExposureTrustSignalsBadgeContainer({
    trustSignals,
    shouldRenderStateHeader,
    size,
    className,
    showPopover,
  });
}

/**
 * Staleness is not a health signal for models: with state-aware orchestration a model can
 * be legitimately reused rather than rebuilt for 30+ days and still be healthy. Source
 * staleness is a separate signal and is unaffected — note both enums serialize to the
 * string `Stale`, so this must only ever be applied on the model path.
 */
const withoutStale = (healthIssues: HealthIssue[] | undefined) =>
  (healthIssues ?? []).filter((issue) => issue !== HealthIssueType.Stale);

function ModelTrustSignalsBadgeContainer({
  className,
  trustSignals: { healthIssues, additionalMetadata },
  shouldRenderStateHeader = false,
  size,
  showPopover,
}: TrustSignalsBadgeParams) {
  const { messages, trustState } = useMemo(() => {
    const modelHealthIssues = withoutStale(healthIssues);
    return {
      trustState: determineTrustState(modelHealthIssues),
      messages: generateMessages(
        'model',
        modelHealthIssues,
        errorStatuses,
        successStatuses,
        additionalMetadata,
      ),
    };
  }, [healthIssues, additionalMetadata]);

  if (!healthIssues) {
    return null;
  }

  return (
    <TrustSignalsBadgeDisplay
      trustState={trustState}
      messages={messages}
      shouldRenderStateHeader={shouldRenderStateHeader}
      size={size}
      className={className}
      showPopover={showPopover}
    />
  );
}

export const sourceErrorStatuses: Record<
  SourceHealthIssueType,
  { importance: number; message: string }
> = {
  Stale: { importance: 0, message: 'Has not been refreshed in the past 30 days' },
  MissingFreshness: {
    importance: 1,
    message: 'Does not have a freshness check configured',
  },
  FreshnessError: {
    importance: 2,
    message: 'Failed its last freshness check',
  },
  FreshnessWarn: {
    importance: 3,
    message: 'Received a warn in its last freshness check',
  },
  NoDescription: { importance: 4, message: 'Is missing a description' },
  Unknown: {
    importance: -1,
    message: 'No data on this resource. To see health data, include it in a job run.',
  },
};

export const sourceSuccessStatuses: Record<
  SourceHealthIssueType,
  { importance: number; message: string }
> = {
  Stale: { importance: 0, message: 'Has been refreshed in the past 30 days' },
  MissingFreshness: { importance: 1, message: 'Has a freshness check configured' },
  NoDescription: { importance: 3, message: 'Has a description' },
  // these are handled in the useMemo below
  FreshnessError: { importance: -1, message: '' },
  FreshnessWarn: { importance: -1, message: '' },
  Unknown: { importance: -1, message: '' },
};

function SourceTrustSignalsBadgeContainer({
  trustSignals: { healthIssues, additionalMetadata },
  shouldRenderStateHeader = false,
  size,
  className,
  showPopover,
}: TrustSignalsBadgeParams) {
  const messages = useMemo(() => {
    const generatedMessages = generateMessages(
      'source',
      healthIssues ?? [],
      sourceErrorStatuses,
      sourceSuccessStatuses,
      additionalMetadata,
    );
    if (
      intersection(
        [
          SourceHealthIssueType.Unknown,
          SourceHealthIssueType.FreshnessError,
          SourceHealthIssueType.FreshnessWarn,
          SourceHealthIssueType.MissingFreshness,
        ],
        healthIssues,
      ).length === 0
    ) {
      generatedMessages.push({
        type: TrustState.Healthy,
        importance: 2,
        text: 'Passed its freshness check',
      });
    }
    return generatedMessages;
  }, [additionalMetadata, healthIssues]);

  if (!healthIssues) {
    return null;
  }

  return (
    <TrustSignalsBadgeDisplay
      trustState={determineTrustState(healthIssues ?? [])}
      messages={messages}
      shouldRenderStateHeader={shouldRenderStateHeader}
      size={size}
      className={className}
      showPopover={showPopover}
    />
  );
}

export const exposureErrorStatuses: Record<
  ExposureHealthIssueType,
  { importance: number; message: string }
> = {
  LastRunFailedUpstreamModels: {
    importance: 0,
    message: 'At least one upstream model did not build successfully',
  },
  WarnedTestUpstreamModels: {
    importance: 1,
    message: "At least one of the upstream models' tests warned",
  },
  FailedTestUpstreamModels: {
    importance: 2,
    message: "At least one of the upstream models' tests failed",
  },
  CautionUpstreamSources: {
    importance: 3,
    message: 'At least one of the upstream sources may be stale',
  },
  DegradedUpstreamSources: {
    importance: 4,
    message: 'At least one of the upstream sources is stale',
  },
  Unknown: {
    importance: -1,
    message: 'No data on this resource. To see health data, include it in a job run.',
  },
};

export const exposureSuccessStatuses: Record<
  ExposureHealthIssueType,
  { importance: number; message: string }
> = {
  LastRunFailedUpstreamModels: {
    importance: 0,
    message: 'All upstream models built successfully',
  },
  // these are handled in the useMemo below
  WarnedTestUpstreamModels: {
    importance: -1,
    message: '',
  },
  FailedTestUpstreamModels: {
    importance: -1,
    message: '',
  },
  CautionUpstreamSources: {
    importance: -1,
    message: '',
  },
  DegradedUpstreamSources: { importance: -1, message: '' },
  Unknown: { importance: -1, message: '' },
};

function ExposureTrustSignalsBadgeContainer({
  trustSignals: { healthIssues },
  shouldRenderStateHeader = false,
  size,
  className,
  showPopover,
}: TrustSignalsBadgeParams) {
  const messages = useMemo(() => {
    const generatedMessages = generateMessages(
      'exposure',
      healthIssues ?? [],
      exposureErrorStatuses,
      exposureSuccessStatuses,
    );
    if (
      intersection(
        [
          ExposureHealthIssueType.Unknown,
          ExposureHealthIssueType.WarnedTestUpstreamModels,
          ExposureHealthIssueType.FailedTestUpstreamModels,
        ],
        healthIssues,
      ).length === 0
    ) {
      generatedMessages.push({
        type: TrustState.Healthy,
        importance: 1,
        text: 'All upstream models passed their tests',
      });
    }
    return generatedMessages;
  }, [healthIssues]);

  return (
    <TrustSignalsBadgeDisplay
      trustState={determineTrustState(healthIssues ?? [])}
      messages={messages}
      shouldRenderStateHeader={shouldRenderStateHeader}
      size={size}
      className={className}
      showPopover={showPopover}
    />
  );
}
