/**
 * Enums transcribed from the dbt platform Discovery API schema.
 *
 * Upstream, `metadata-shared` gets these from its GraphQL codegen artifact
 * (`src/__generated__/graphql.ts`, ~7.2k generated lines). This server talks REST,
 * so only the handful of enums the forked components actually reference are
 * transcribed here rather than carrying the whole generated schema.
 *
 * Keep the members in sync with the Discovery API schema if any of this data
 * starts being served from the REST API.
 */

export enum HealthIssueType {
  CautionUpstreamSources = 'CautionUpstreamSources',
  DegradedUpstreamSources = 'DegradedUpstreamSources',
  FailedTest = 'FailedTest',
  LastRunFailed = 'LastRunFailed',
  NoDescription = 'NoDescription',
  NoTests = 'NoTests',
  Stale = 'Stale',
  Unknown = 'Unknown',
  WarnedTest = 'WarnedTest',
}

export enum SourceHealthIssueType {
  FreshnessError = 'FreshnessError',
  FreshnessWarn = 'FreshnessWarn',
  MissingFreshness = 'MissingFreshness',
  NoDescription = 'NoDescription',
  Stale = 'Stale',
  Unknown = 'Unknown',
}

export enum ExposureHealthIssueType {
  CautionUpstreamSources = 'CautionUpstreamSources',
  DegradedUpstreamSources = 'DegradedUpstreamSources',
  FailedTestUpstreamModels = 'FailedTestUpstreamModels',
  LastRunFailedUpstreamModels = 'LastRunFailedUpstreamModels',
  Unknown = 'Unknown',
  WarnedTestUpstreamModels = 'WarnedTestUpstreamModels',
}

export enum RunStatus {
  Error = 'error',
  Reused = 'reused',
  Skipped = 'skipped',
  Success = 'success',
  Warn = 'warn',
}
