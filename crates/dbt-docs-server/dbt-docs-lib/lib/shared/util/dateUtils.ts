import {
  differenceInHours,
  differenceInMinutes,
  differenceInSeconds,
  isAfter,
} from 'date-fns';
import { formatInTimeZone } from 'date-fns-tz';

/**
 * Format a date in the local timezone
 * @param date - The date to format
 * @param fmt - The format string (date-fns format)
 * @returns A formatted date string
 */
export const formatLocalDate = (date: Date, fmt: string) => {
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  try {
    return formatInTimeZone(date, timeZone, fmt);
  } catch (e: any | Error) {
    return e.message;
  }
};

/**
 * Formats a date as YYYY-MM-DD for CSV export (standard ISO format).
 * This format is universally recognized by spreadsheet applications and doesn't require escaping.
 * Uses UTC methods to ensure consistent output regardless of local timezone.
 *
 * @param date - The date to format
 * @returns A string in YYYY-MM-DD format
 */
export const formatUtcDate = (date: Date): string => date.toUTCString();

export const formatAbsoluteLocalDate = (date: Date): string =>
  formatLocalDate(date, 'PP, p z');

export const formatRelativeDate = (date: Date): string => {
  const now = new Date();
  const isInFuture = isAfter(date, now);
  const diffInHours = Math.abs(differenceInHours(now, date));
  const diffInSeconds = Math.abs(differenceInSeconds(now, date));
  const diffInMinutes = Math.abs(differenceInMinutes(now, date));
  let relativeTime = '';
  if (diffInHours < 1) {
    relativeTime = `${diffInMinutes}m ${diffInSeconds - 60 * diffInMinutes}s`;
  } else {
    relativeTime = `${diffInHours}h ${diffInMinutes - 60 * diffInHours}m`;
  }
  return isInFuture ? `in ${relativeTime}` : `${relativeTime} ago`;
};

export interface CommonDateFormat {
  humanized?: string;
  utc?: string;
}

export const formatDateCommon = (date: Date): CommonDateFormat => {
  const now = new Date();
  const diffInHours = Math.abs(differenceInHours(now, date));
  const humanized =
    diffInHours < 6 ? formatRelativeDate(date) : formatAbsoluteLocalDate(date);
  return { humanized, utc: formatUtcDate(date) };
};

export const formatDateForCsv = (date: Date): string => {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
};
