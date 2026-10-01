export type ExecutionState = 'none' | 'ran' | 'skipped' | 'error';
export type RunStatus =
  'success' | 'error' | 'running' | 'queued' | 'skipped' | 'reused';

/**
 * Per-node execution from the most recent run that touched this asset.
 * Distinct from {@link JobExecutionInfo}, which describes the parent job.
 */
export type NodeExecutionInfo = {
  state: ExecutionState;
  status: RunStatus | null;
  startedAt: string | null;
  completedAt: string | null;
};

/**
 * Most recent job (dbt invocation) that produced execution data for the asset.
 * Use for "last run" headers; use {@link NodeExecutionInfo} for per-node detail.
 */
export type JobExecutionInfo = {
  jobId: string | null;
  status: RunStatus | null;
  startedAt: string | null;
  completedAt: string | null;
};

export type ExecutionInfo = {
  job: JobExecutionInfo | null;
  node: NodeExecutionInfo | null;
};
