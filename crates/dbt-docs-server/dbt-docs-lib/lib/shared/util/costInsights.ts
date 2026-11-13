import { endOfWeek, startOfMonth, startOfWeek } from 'date-fns';

import { CostInsight, CostInsightByJob, Grouping } from '../typings/costInsights';
import { COLUMN_LABELS } from './columnLabels';
import { downloadCsv } from './csv';
import { formatDateForCsv, formatLocalDate } from './dateUtils';
import { formatMinutes as formatMinutesUtil } from './duration';

/**
 * Reduces an array of CostInsight objects by summing all numeric fields.
 */
const reduceCostInsights = (items: CostInsight[]): CostInsight => {
  if (items.length === 0) {
    throw new Error('Cannot reduce empty array');
  }

  const firstItem = items[0];
  const result: CostInsight = {
    date: firstItem.date,
    executionCount: 0,
    reusedCount: 0,
    executionTime: 0,
    executionComputeUnits: 0,
    executionCost: 0,
    executionTimeSaved: 0,
    executionComputeUnitsSaved: 0,
    executionCostSaved: 0,
    isCostProcessed: true,
  };

  // Sum all numeric fields
  for (const item of items) {
    result.executionCount += item.executionCount;
    result.reusedCount += item.reusedCount;
    result.executionTime += item.executionTime;
    result.executionComputeUnits += item.executionComputeUnits;
    result.executionCost += item.executionCost;
    result.executionTimeSaved += item.executionTimeSaved;
    result.executionComputeUnitsSaved += item.executionComputeUnitsSaved;
    result.executionCostSaved += item.executionCostSaved;
  }

  result.isCostProcessed = items.every((item) => item.isCostProcessed);

  return result;
};

/**
 * Groups cost insight data by a time period using a function to determine the period start.
 * Groups data starting from the first item's period.
 * If the first item starts mid-period, only includes items for that period, then moves to the next period.
 *
 * All numeric fields are summed within each group.
 *
 * Note: This function sorts the input data chronologically before grouping to ensure correct aggregation.
 */
const groupByPeriod = (
  data: CostInsight[],
  getPeriodStart: (date: Date) => Date,
): CostInsight[] => {
  // Sort data chronologically to ensure items from the same period are grouped together
  const sortedData = [...data].sort((a, b) => a.date.getTime() - b.date.getTime());

  const grouped: CostInsight[] = [];
  let currentGroup: CostInsight[] = [];
  let currentPeriodStart: Date | null = null;

  for (const item of sortedData) {
    const periodStart = getPeriodStart(item.date);

    // Initialize first group
    if (currentPeriodStart === null) {
      currentPeriodStart = periodStart;
      currentGroup = [item];
      continue;
    }

    // If item belongs to current period, add to current group
    if (periodStart.getTime() === currentPeriodStart.getTime()) {
      currentGroup.push(item);
    } else {
      // Start a new period group
      if (currentGroup.length > 0) {
        grouped.push(reduceCostInsights(currentGroup));
      }
      currentPeriodStart = periodStart;
      currentGroup = [item];
    }
  }

  // Add the last group
  if (currentGroup.length > 0) {
    grouped.push(reduceCostInsights(currentGroup));
  }

  return grouped;
};

/**
 * Groups cost insight data by time grouping (daily, weekly, or monthly).
 *
 * - Daily: Returns data as-is (sorted chronologically)
 * - Weekly: Groups data by week starting from the first item's week.
 *   If the first item starts mid-week, only includes items for that week, then moves to the next week.
 * - Monthly: Groups data by month starting from the first item's month.
 *   If the first item starts mid-month, only includes items for that month, then moves to the next month.
 *
 * All numeric fields are summed within each group.
 *
 * Note: Input data does not need to be pre-sorted; the function handles sorting internally for weekly and monthly groupings.
 */
export const groupDataByTimeGrouping = (
  data: CostInsight[],
  grouping: Grouping,
): CostInsight[] => {
  if (data.length === 0) {
    return [];
  }

  // Daily grouping: return data sorted chronologically
  if (grouping === 'daily') {
    return [...data].sort((a, b) => a.date.getTime() - b.date.getTime());
  }

  // Weekly grouping
  if (grouping === 'weekly') {
    return groupByPeriod(data, (date) => startOfWeek(date, { weekStartsOn: 0 })); // Sunday = 0
  }

  // Monthly grouping
  if (grouping === 'monthly') {
    return groupByPeriod(data, startOfMonth);
  }

  return data;
};

/**
 * Formats minutes for display in chart labels/tooltips.
 * Converts minutes to a human-readable format:
 * - Less than 1 minute: displays as seconds (e.g., "30 seconds")
 * - 1 minute or more: displays as minutes with comma formatting (e.g., "45 minutes", "1,440 minutes")
 *
 * @param minutes - The number of minutes to format (can be null)
 * @returns A formatted string representation of the time
 */
export const formatMinutes = formatMinutesUtil;

/**
 * Formats a currency value for display.
 * @param value - The numeric value to format
 * @returns A formatted string with dollar sign and comma formatting (e.g., "$1,234")
 */
export const formatCurrency = (value: number): string => {
  return `$${value.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
};

/**
 * Formats a number for display with comma separators.
 * @param value - The numeric value to format (can be null or undefined)
 * @returns A formatted string with comma formatting (e.g., "1,234") or "0" for null/undefined
 */
export const formatNumber = (value: number | null | undefined): string => {
  if (value === null || value === undefined) {
    return '0';
  }
  return Math.round(value).toLocaleString('en-US');
};

/**
 * Formats a credit value for display with 2 decimal places and comma separators.
 * @param value - The numeric value to format (can be null or undefined)
 * @returns A formatted string with 2 decimal places (e.g., "1,234.56") or "0.00" for null/undefined
 */
export const formatCredits = (value: number | null | undefined): string => {
  if (value === null || value === undefined) {
    return '0.00';
  }
  return value.toLocaleString('en-US', {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
};

/**
 * Formats the date based on the grouping for table display.
 * @param date - The date to format
 * @param grouping - The grouping to use
 * @returns The formatted date string
 */
export const formatDateByGrouping = (date: Date, grouping: Grouping): string => {
  if (grouping === 'daily') {
    return formatLocalDate(date, 'MMM d, yyyy');
  }

  if (grouping === 'weekly') {
    const weekStart = startOfWeek(date, { weekStartsOn: 0 });
    const weekEnd = endOfWeek(date, { weekStartsOn: 0 });
    const startFormatted = formatLocalDate(weekStart, 'MMM d');
    const endFormatted = formatLocalDate(weekEnd, 'MMM d, yyyy');
    return `${startFormatted} - ${endFormatted}`;
  }

  if (grouping === 'monthly') {
    return formatLocalDate(date, 'MMMM yyyy');
  }

  return formatLocalDate(date, 'MMM d, yyyy');
};

/**
 * Returns the appropriate time period label based on the grouping option.
 *
 * @param grouping - The time grouping option ('daily', 'weekly', or 'monthly')
 * @returns A string representing the time period (e.g., "by day", "by week", "by month")
 */
export const getTimePeriodLabel = (grouping: Grouping): string => {
  switch (grouping) {
    case 'daily':
      return 'by day';
    case 'weekly':
      return 'by week';
    case 'monthly':
      return 'by month';
    default:
      return 'by day';
  }
};

const FORTY_EIGHT_HOURS_MS = 48 * 60 * 60 * 1000;
const THIRTY_MINUTES_MS = 30 * 60 * 1000;

/**
 * True when a row reflects real build activity (runs or reuse). These fields are
 * sourced from run metadata and are populated before warehouse cost extraction —
 * unlike the cost/compute fields, which stay 0 while cost is still "preparing".
 * They're therefore the reliable signal for "there is data" independent of whether
 * cost has been processed yet.
 */
const hasBuildActivity = (item: CostInsight): boolean =>
  item.executionCount > 0 || item.reusedCount > 0 || item.executionTime > 0;

/**
 * Checks whether cost insights should show the "preparing" state: there is genuine
 * build activity in the range but its cost has not been processed yet, indicating
 * initial cost calculation is still in progress for a newly enabled account.
 *
 * Returns false when there's no data to prepare — an empty array OR rows that carry
 * no build activity at all (all zero). Those are the "no data" state, not
 * "preparing", so callers fall through to their empty state instead of showing the
 * spinner indefinitely.
 *
 * The activity must also be recent (within 48 h, matching `isRowCostPending`).
 * Cost extraction runs shortly after a build; if activity is older than that and
 * still unprocessed, extraction has stalled (broken/unsupported connection, a
 * failed cron, etc.) rather than being in flight — so we stop showing the spinner
 * and let callers render the real run/reuse data instead of a skeleton forever.
 *
 * @param now - Injectable timestamp for testing; defaults to Date.now()
 */
export const isInitialCostProcessing = (
  data: CostInsight[],
  now: number = Date.now(),
): boolean => {
  return (
    data.length > 0 &&
    data.every((item) => !item.isCostProcessed) &&
    data.some(
      (item) =>
        hasBuildActivity(item) && now - item.date.getTime() < FORTY_EIGHT_HOURS_MS,
    )
  );
};

/**
 * Returns true when a daily row's warehouse-sourced cost columns should show
 * the "--" pending placeholder instead of real values.
 *
 * Always returns false for weekly/monthly groupings — the period start date is
 * not "yesterday", so the 48 h recency guard does not apply.
 *
 * @param now - Injectable timestamp for testing; defaults to Date.now()
 */
export const isRowCostPending = ({
  grouping,
  rowDate,
  isCostProcessed,
  connectionTestUpdatedAt,
  now = Date.now(),
}: {
  grouping: Grouping;
  rowDate: Date;
  isCostProcessed: boolean;
  connectionTestUpdatedAt: Date | undefined;
  now?: number;
}): boolean => {
  if (grouping !== 'daily') return false;
  if (now - rowDate.getTime() >= FORTY_EIGHT_HOURS_MS) return false;
  if (
    connectionTestUpdatedAt !== undefined &&
    now - connectionTestUpdatedAt.getTime() >= FORTY_EIGHT_HOURS_MS
  )
    return false;

  // Extraction queries can be slow — keep showing -- for 30 min after the cron fires
  const withinExtractionBuffer =
    connectionTestUpdatedAt !== undefined &&
    now - connectionTestUpdatedAt.getTime() < THIRTY_MINUTES_MS;

  return !isCostProcessed || withinExtractionBuffer;
};

/**
 * Downloads cost insights data as a CSV file.
 * The CSV includes raw numeric values for proper spreadsheet compatibility.
 * Spreadsheet applications will handle their own formatting of numbers, currency, and dates.
 *
 * @param data - The cost insight data to export
 * @param grouping - The time grouping applied to the data
 * @param filename - Optional custom filename (without .csv extension). Defaults to "cost-insights-{grouping}"
 */
export const downloadCostInsightsCsv = (
  data: CostInsight[],
  grouping: Grouping,
  filename?: string,
): void => {
  const headers = [
    COLUMN_LABELS.date,
    COLUMN_LABELS.modelBuilds,
    COLUMN_LABELS.modelReused,
    `${COLUMN_LABELS.runDuration} (minutes)`,
    COLUMN_LABELS.computeExecuted,
    COLUMN_LABELS.computeAvoided,
    COLUMN_LABELS.cost,
    COLUMN_LABELS.costSaved,
  ];

  const rows = data.map((item) => [
    formatDateForCsv(item.date),
    item.executionCount.toString(),
    item.reusedCount.toString(),
    ((item.executionTime || 0) / 60).toFixed(2), // Convert from seconds to minutes
    item.executionComputeUnits.toFixed(2),
    item.executionComputeUnitsSaved.toFixed(2),
    item.executionCost.toFixed(2),
    item.executionCostSaved.toFixed(2),
  ]);

  downloadCsv(headers, rows, filename || `cost-insights-${grouping}`);
};

/**
 * Groups CostInsightByJob data by (jobDefinitionId, time period), summing metrics within each group.
 * Parallel to groupDataByTimeGrouping but preserves the job dimension.
 */
export const groupDataByJobAndTimeGrouping = (
  data: CostInsightByJob[],
  grouping: Grouping,
): CostInsightByJob[] => {
  if (data.length === 0) return [];

  const getPeriodStart = (date: Date): Date => {
    if (grouping === 'weekly') return startOfWeek(date, { weekStartsOn: 0 });
    if (grouping === 'monthly') return startOfMonth(date);
    return date;
  };

  const groupMap = new Map<string, CostInsightByJob[]>();

  for (const item of data) {
    const periodStart = getPeriodStart(item.date);
    const key = `${item.jobDefinitionId}::${periodStart.getTime()}`;
    const existing = groupMap.get(key);
    if (existing) {
      existing.push(item);
    } else {
      groupMap.set(key, [item]);
    }
  }

  return Array.from(groupMap.values())
    .map((group) => {
      const first = group[0];
      const result: CostInsightByJob = {
        date: getPeriodStart(first.date),
        jobDefinitionId: first.jobDefinitionId,
        jobName: group.find((item) => item.jobName !== null)?.jobName ?? null,
        executionCount: 0,
        reusedCount: 0,
        executionTime: 0,
        executionComputeUnits: 0,
        executionCost: 0,
        executionTimeSaved: 0,
        executionComputeUnitsSaved: 0,
        executionCostSaved: 0,
        isCostProcessed: true,
      };
      for (const item of group) {
        result.executionCount += item.executionCount;
        result.reusedCount += item.reusedCount;
        result.executionTime += item.executionTime;
        result.executionComputeUnits += item.executionComputeUnits;
        result.executionCost += item.executionCost;
        result.executionTimeSaved += item.executionTimeSaved;
        result.executionComputeUnitsSaved += item.executionComputeUnitsSaved;
        result.executionCostSaved += item.executionCostSaved;
      }
      result.isCostProcessed = group.every((item) => item.isCostProcessed);
      return result;
    })
    .sort((a, b) => {
      const dateDiff = a.date.getTime() - b.date.getTime();
      return dateDiff !== 0 ? dateDiff : a.jobDefinitionId - b.jobDefinitionId;
    });
};

/**
 * Downloads cost insights by job data as a CSV file.
 * Each row represents one (period, job) pair with the same metrics as the all-jobs CSV.
 */
export const downloadCostInsightsByJobCsv = (
  data: CostInsightByJob[],
  grouping: Grouping,
  filename?: string,
): void => {
  const headers = [
    COLUMN_LABELS.date,
    'Job',
    COLUMN_LABELS.modelBuilds,
    COLUMN_LABELS.modelReused,
    `${COLUMN_LABELS.runDuration} (minutes)`,
    COLUMN_LABELS.computeExecuted,
    COLUMN_LABELS.computeAvoided,
    COLUMN_LABELS.cost,
    COLUMN_LABELS.costSaved,
  ];

  const sorted = [...data].sort((a, b) => a.date.getTime() - b.date.getTime());

  const rows = sorted.map((item) => [
    formatDateForCsv(item.date),
    item.jobName ?? `Job ${item.jobDefinitionId}`,
    item.executionCount.toString(),
    item.reusedCount.toString(),
    ((item.executionTime || 0) / 60).toFixed(2),
    item.executionComputeUnits.toFixed(2),
    item.executionComputeUnitsSaved.toFixed(2),
    item.executionCost.toFixed(2),
    item.executionCostSaved.toFixed(2),
  ]);

  downloadCsv(headers, rows, filename || `cost-insights-by-job-${grouping}`);
};
