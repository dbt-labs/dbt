import type { ResourceTypeExplorer } from '../../lib/resourceType';

export const capitalizedResourceNames: Record<ResourceTypeExplorer, string> = {
  analysis: 'Analysis',
  column: 'Column',
  exposure: 'Exposure',
  function: 'Function',
  group: 'Group',
  macro: 'Macro',
  metric: 'Metric',
  model: 'Model',
  project: 'Project',
  saved_query: 'Saved Query',
  seed: 'Seed',
  semantic_model: 'Semantic Model',
  snapshot: 'Snapshot',
  source: 'Source',
  test: 'Test',
  unit_test: 'Unit Test',
};
