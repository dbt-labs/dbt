export type Capabilities = {
  hasColumnLineage: boolean;
  hasQueryHistory: boolean;
  hasCostInsights: boolean;
  hasPerformance: boolean;
  hasRecommendations: boolean;
  hasHealthSignals: boolean;
  hasAutoExposures: boolean;
  hasMultiProject: boolean;
  hasMesh: boolean;
  hasRunResults: boolean;
  hasCatalogStats: boolean;
  /** dbt State is active for this project (server `has_dbt_state`). Distinct
   *  from {@link hasRunResults}, which is run-artifact availability. */
  hasDbtState: boolean;
};
