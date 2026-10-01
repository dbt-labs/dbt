/**
 * Shared column header labels used in both the ChartTable UI
 * and CSV export functions. Define labels here to keep them
 * consistent across table display and downloads.
 */
export const COLUMN_LABELS = {
  date: 'Date',
  modelBuilds: 'Asset builds',
  modelReused: 'Reused assets',
  runDuration: 'Run duration',
  computeExecuted: 'Compute executed',
  computeAvoided: 'Compute avoided',
  cost: 'Cost',
  costSaved: 'Cost saved',
} as const;
