// Revisit whether or not we want to do this
import { intersection } from 'lodash';

import type { ResourceType } from '../../lib/resourceType';
import {
  ExposureHealthIssueType,
  HealthIssueType,
  SourceHealthIssueType,
} from '../typings/discoveryEnums';
import { AdditionalMetadata, HealthIssue } from '../typings/trustSignals';

const ALL_UNKNOWN_ISSUES = [
  HealthIssueType.Unknown,
  SourceHealthIssueType.Unknown,
  ExposureHealthIssueType.Unknown,
];

const ALL_UPSTREAM_SOURCE_ISSUES = [
  HealthIssueType.CautionUpstreamSources,
  HealthIssueType.DegradedUpstreamSources,
  ExposureHealthIssueType.CautionUpstreamSources,
  ExposureHealthIssueType.DegradedUpstreamSources,
];

export enum TrustState {
  Unknown = 'unknown',
  Degraded = 'degraded',
  Caution = 'caution',
  Healthy = 'healthy',
}

export interface TrustSignalMessage {
  type: TrustState;
  importance: number;
  text: string;
  link?: {
    to: string;
    state?: object;
  };
}

function isHealthIssue(issue: HealthIssue): issue is HealthIssueType {
  return issue in healthIssueToTrustState;
}

function isSourceHealthIssue(issue: HealthIssue): issue is SourceHealthIssueType {
  return issue in sourceHealthIssueToTrustState;
}

function isExposureHealthIssue(issue: HealthIssue): issue is ExposureHealthIssueType {
  return issue in exposureHealthIssueToTrustState;
}

export const getTrustState = (healthIssue: HealthIssue): TrustState => {
  if (isHealthIssue(healthIssue)) {
    return healthIssueToTrustState[healthIssue];
  }
  if (isSourceHealthIssue(healthIssue)) {
    return sourceHealthIssueToTrustState[healthIssue];
  }
  if (isExposureHealthIssue(healthIssue)) {
    return exposureHealthIssueToTrustState[healthIssue];
  }
  return TrustState.Unknown;
};

export const determineTrustState = (healthIssues: HealthIssue[]): TrustState => {
  if (healthIssues.length === 0) return TrustState.Healthy;

  const mappedStates = healthIssues.map((healthIssue: HealthIssue) =>
    getTrustState(healthIssue),
  );

  return mappedStates.includes(TrustState.Unknown)
    ? TrustState.Unknown
    : mappedStates.includes(TrustState.Degraded)
      ? TrustState.Degraded
      : TrustState.Caution;
};

const healthIssueToTrustState: Record<HealthIssueType, TrustState> = {
  [HealthIssueType.Unknown]: TrustState.Unknown,
  [HealthIssueType.LastRunFailed]: TrustState.Degraded,
  [HealthIssueType.NoTests]: TrustState.Caution,
  [HealthIssueType.FailedTest]: TrustState.Degraded,
  [HealthIssueType.WarnedTest]: TrustState.Caution,
  [HealthIssueType.NoDescription]: TrustState.Caution,
  [HealthIssueType.Stale]: TrustState.Caution,
  [HealthIssueType.CautionUpstreamSources]: TrustState.Caution,
  [HealthIssueType.DegradedUpstreamSources]: TrustState.Degraded,
};

const sourceHealthIssueToTrustState: Record<SourceHealthIssueType, TrustState> = {
  [SourceHealthIssueType.NoDescription]: TrustState.Caution,
  [SourceHealthIssueType.Unknown]: TrustState.Unknown,
  [SourceHealthIssueType.Stale]: TrustState.Caution,
  [SourceHealthIssueType.MissingFreshness]: TrustState.Caution,
  [SourceHealthIssueType.FreshnessError]: TrustState.Degraded,
  [SourceHealthIssueType.FreshnessWarn]: TrustState.Caution,
};

const exposureHealthIssueToTrustState: Record<ExposureHealthIssueType, TrustState> = {
  [ExposureHealthIssueType.Unknown]: TrustState.Unknown,
  [ExposureHealthIssueType.LastRunFailedUpstreamModels]: TrustState.Degraded,
  [ExposureHealthIssueType.WarnedTestUpstreamModels]: TrustState.Caution,
  [ExposureHealthIssueType.FailedTestUpstreamModels]: TrustState.Degraded,
  [ExposureHealthIssueType.CautionUpstreamSources]: TrustState.Caution,
  [ExposureHealthIssueType.DegradedUpstreamSources]: TrustState.Degraded,
};

/**
 * When a more-severe issue is present, suppress the less-severe counterpart.
 */
const SEVERITY_SUPPRESSION_RULES: { when: HealthIssue; suppress: HealthIssue }[] = [
  {
    when: HealthIssueType.DegradedUpstreamSources,
    suppress: HealthIssueType.CautionUpstreamSources,
  },
  {
    when: ExposureHealthIssueType.DegradedUpstreamSources,
    suppress: ExposureHealthIssueType.CautionUpstreamSources,
  },
];

export const filterSuppressedIssues = (healthIssues: HealthIssue[]): HealthIssue[] => {
  const suppressed = new Set<HealthIssue>();

  for (const rule of SEVERITY_SUPPRESSION_RULES) {
    if (healthIssues.includes(rule.when)) {
      suppressed.add(rule.suppress);
    }
  }

  return healthIssues.filter((issue) => !suppressed.has(issue));
};

export const generateMessages = (
  resourceType: ResourceType,
  healthIssues: HealthIssue[],
  errorMessages: { [key: string]: { importance: number; message: string } },
  successMessages: { [key: string]: { importance: number; message: string } },
  additionalMetadata?: AdditionalMetadata,
): TrustSignalMessage[] => {
  if (intersection(healthIssues, ALL_UNKNOWN_ISSUES).length > 0) {
    return [
      {
        type: TrustState.Unknown,
        importance: 1,
        text: errorMessages[HealthIssueType.Unknown].message,
      },
    ];
  }

  const filteredHealthIssues = filterSuppressedIssues(healthIssues);

  const messages: TrustSignalMessage[] = filteredHealthIssues
    .filter((healthIssue: HealthIssue) => {
      // if the NoTests health issue is reported, it does not make sense
      // to provide information on tests succeeding or failing.
      return !(
        ((healthIssue === HealthIssueType.FailedTest ||
          healthIssue === HealthIssueType.WarnedTest) &&
          healthIssues.includes(HealthIssueType.NoTests)) ||
        healthIssue === HealthIssueType.Unknown
      );
    })
    .map((issue) => ({
      type: getTrustState(issue),
      importance: errorMessages[issue]?.importance,
      text: errorMessages[issue]?.message,
      ...(issue === HealthIssueType.CautionUpstreamSources && {
        link: additionalMetadata?.linkMapping?.[issue],
      }),
    }))
    .filter((message) => message.text);

  const healthyUpstreamSources =
    intersection(healthIssues, ALL_UPSTREAM_SOURCE_ISSUES).length === 0;
  if (healthyUpstreamSources && resourceType !== 'source') {
    messages.push({
      type: TrustState.Healthy,
      importance: 2,
      text: 'All upstream sources healthy',
    });
  }

  const freshnessIgnored = additionalMetadata?.freshnessChecked === null;

  Object.entries(successMessages)
    .filter(
      ([healthIssue]) =>
        // If no tests, do not render test status issues
        !(
          healthIssues.includes(HealthIssueType.NoTests) &&
          (healthIssue === HealthIssueType.FailedTest ||
            healthIssue === HealthIssueType.WarnedTest)
        ),
    )
    .filter(
      ([healthIssue]) =>
        !(
          // If all healthy, don't render individual success messages
          healthyUpstreamSources &&
          ALL_UPSTREAM_SOURCE_ISSUES.includes(
            healthIssue as HealthIssueType | ExposureHealthIssueType,
          )
        ) && // If freshness block is set to null, don't render success messages related to freshness
        !(
          freshnessIgnored &&
          (healthIssue === SourceHealthIssueType.Stale ||
            healthIssue === SourceHealthIssueType.MissingFreshness)
        ),
    )
    .forEach(([issue, info]) => {
      if (!healthIssues.includes(issue as HealthIssue) && info.message) {
        messages.push({
          type: TrustState.Healthy,
          importance: info.importance,
          text: info.message,
        });
      }
    });

  return messages;
};
