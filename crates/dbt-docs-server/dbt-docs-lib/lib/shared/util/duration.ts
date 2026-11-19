/**
 * Formats a duration in seconds to a human-readable string.
 * Supports hours, minutes, and seconds with appropriate precision.
 *
 * Examples:
 * - 45 seconds → "45s"
 * - 125 seconds → "2m 5s"
 * - 3665 seconds → "1h 1m 5s"
 *
 * @param seconds - The duration in seconds
 * @returns A formatted string representation of the duration
 */
export const formatDuration = (seconds: number): string => {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = seconds % 60;

  if (hours > 0) {
    return `${hours}h ${minutes}m ${secs}s`;
  } else if (minutes > 0) {
    return `${minutes}m ${secs}s`;
  }
  return `${secs}s`;
};

/**
 * Formats minutes for display in chart labels/tooltips.
 * Converts minutes to a human-readable format:
 * - Less than 1 minute: displays as seconds (e.g., "30 seconds")
 * - 1 minute to less than 60 minutes: displays as minutes (e.g., "45 minutes")
 * - 60 minutes or more: displays as hours with remaining minutes (e.g., "1h", "2h", "1h 30 minutes")
 *
 * @param minutes - The number of minutes to format (can be null)
 * @returns A formatted string representation of the time
 */
export const formatMinutes = (minutes: number | null): string => {
  if (minutes === null || minutes === undefined || minutes === 0) return '0m';
  if (minutes < 1) {
    const seconds = Math.round(minutes * 60);
    return seconds === 1 ? '1s' : `${seconds}s`;
  }
  const roundedMinutes = Math.round(minutes);

  // Convert to days if >= 1440 minutes (24 hours)
  if (roundedMinutes >= 1440) {
    const days = Math.floor(roundedMinutes / 1440);
    const remainingAfterDays = roundedMinutes % 1440;
    const hours = Math.floor(remainingAfterDays / 60);
    const mins = remainingAfterDays % 60;

    if (hours === 0 && mins === 0) return `${days}d`;
    if (mins === 0) return `${days}d ${hours}h`;
    if (hours === 0) return `${days}d ${mins}m`;
    return `${days}d ${hours}h ${mins}m`;
  }

  // Convert to hours if >= 60 minutes
  if (roundedMinutes >= 60) {
    const hours = Math.floor(roundedMinutes / 60);
    const remainingMinutes = roundedMinutes % 60;

    if (remainingMinutes === 0) {
      return `${hours}h`;
    }
    const minuteText = remainingMinutes === 1 ? '1m' : `${remainingMinutes}m`;
    return `${hours}h ${minuteText}`;
  }

  if (roundedMinutes === 1) return '1m';
  return `${roundedMinutes.toLocaleString('en-US')}m`;
};
