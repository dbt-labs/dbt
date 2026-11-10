import type { ResourceType } from '../../lib/resourceType';
import {
  ExposureHealthIssueType,
  HealthIssueType,
  SourceHealthIssueType,
} from './discoveryEnums';

export type HealthIssue =
  HealthIssueType | SourceHealthIssueType | ExposureHealthIssueType;

export const AllHealthIssues = [
  ...Object.values(HealthIssueType),
  ...Object.values(SourceHealthIssueType),
  ...Object.values(ExposureHealthIssueType),
] as const;

export const trustSignalsSupportedResourceTypes: readonly ResourceType[] = [
  'model',
  'source',
  'exposure',
] as const;

/** All required information to render trust signals for a resource */
export type TrustSignals = {
  healthIssues: HealthIssue[] | undefined;
  additionalMetadata?: AdditionalMetadata;
};

export type LinkMapping = {
  [k in HealthIssue]?: { to: string; state?: object };
};

/** Additional resource-specific metadata that affects the trust signals rendering */
export type AdditionalMetadata = {
  freshnessChecked?: boolean | null;
  /** Mapping of health issues to links */
  linkMapping?: LinkMapping;
};
