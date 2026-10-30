export type Grouping = 'daily' | 'weekly' | 'monthly';

export type CostInsightsTableMode = 'all' | 'jobs';

export type CostInsight = {
  date: Date;
  executionCount: number;
  reusedCount: number;
  executionTime: number;
  executionComputeUnits: number;
  executionCost: number;
  executionTimeSaved: number;
  executionComputeUnitsSaved: number;
  executionCostSaved: number;
  isCostProcessed: boolean;
};

export type CostInsightByJob = {
  date: Date;
  jobDefinitionId: number;
  jobName: string | null;
  executionCount: number;
  reusedCount: number;
  executionTime: number;
  executionComputeUnits: number;
  executionCost: number;
  executionTimeSaved: number;
  executionComputeUnitsSaved: number;
  executionCostSaved: number;
  isCostProcessed: boolean;
};

export type CostInsightsConnectionTestStatusErrorType =
  'unknown' | 'no_access' | 'no_access_optional' | 'no_data';

export type CostInsightsConnectionTestStatus =
  'unknown' | 'never_triggered' | 'testing' | 'succeeded' | 'failed';

export interface CostInsightsConnectionTestStatusErrorDetails {
  table_name: string;
  error_type: CostInsightsConnectionTestStatusErrorType;
  error_details?: string | null;
}

export interface CostInsightsConnectionTestStatusOverview {
  account_id: string;
  connection_id: string;
  status: CostInsightsConnectionTestStatus;
  errors: CostInsightsConnectionTestStatusErrorDetails[];
  updated_at: string;
}
