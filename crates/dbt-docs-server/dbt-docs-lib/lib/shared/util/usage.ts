import { Grouping } from '../typings/costInsights';
import { UsageDataPoint } from '../typings/usage';
import { COLUMN_LABELS } from './columnLabels';
import { downloadCsv } from './csv';
import { formatDateForCsv } from './dateUtils';

/**
 * Downloads usage data as a CSV file.
 * The CSV includes raw numeric values for proper spreadsheet compatibility.
 *
 * @param data - The usage data to export
 * @param grouping - The time grouping applied to the data
 * @param filename - Optional custom filename (without .csv extension). Defaults to "usage-{grouping}"
 */
export const downloadUsageCsv = (
  data: UsageDataPoint[],
  grouping: Grouping,
  filename?: string,
): void => {
  const headers = [
    COLUMN_LABELS.date,
    COLUMN_LABELS.modelBuilds,
    COLUMN_LABELS.modelReused,
    `${COLUMN_LABELS.runDuration} (seconds)`,
    COLUMN_LABELS.computeExecuted,
    COLUMN_LABELS.computeAvoided,
    COLUMN_LABELS.cost,
    COLUMN_LABELS.costSaved,
  ];

  const rows = data.map((item) => [
    formatDateForCsv(new Date(item.date)),
    item.builds.toString(),
    item.reused.toString(),
    item.runDuration.toString(),
    item.creditsUsed.toFixed(2),
    item.creditsSaved.toFixed(2),
    item.cost.toFixed(2),
    item.costSavings.toFixed(2),
  ]);

  downloadCsv(headers, rows, filename || `usage-${grouping}`);
};
