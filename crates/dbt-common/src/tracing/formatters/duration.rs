use std::{
    fmt,
    time::{Duration, SystemTime},
};

use chrono::{DateTime, Utc};

/// Displays elapsed duration for final invocation summaries.
///
/// The formatting targets a compact single token suitable for `[<value>]` blocks,
/// matching the desired UX in the run result output.
#[derive(Clone, Copy)]
pub struct SummaryDuration(pub Duration);

impl fmt::Display for SummaryDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let duration = self.0;
        let total_secs = duration.as_secs_f64();

        if total_secs >= 3600.0 {
            let hours = (total_secs / 3600.0).floor() as u64;
            let minutes = ((total_secs % 3600.0) / 60.0).floor() as u64;
            let seconds = total_secs % 60.0;
            if seconds >= 1.0 {
                write!(f, "{hours}h {minutes}m {seconds:.0}s")
            } else if minutes > 0 {
                write!(f, "{hours}h {minutes}m")
            } else {
                write!(f, "{hours}h")
            }
        } else if total_secs >= 60.0 {
            let minutes = (total_secs / 60.0).floor() as u64;
            let seconds = total_secs % 60.0;
            if seconds >= 1.0 {
                write!(f, "{minutes}m {seconds:.0}s")
            } else {
                write!(f, "{minutes}m")
            }
        } else if total_secs >= 1.0 {
            write!(f, "{total_secs:.1}s")
        } else {
            let millis = duration.as_millis();
            if millis >= 1 {
                write!(f, "{millis}ms")
            } else {
                let micros = duration.as_micros();
                if micros >= 1 {
                    write!(f, "{micros}us")
                } else {
                    write!(f, "{}ns", duration.as_nanos())
                }
            }
        }
    }
}

/// Displays a duration with fixed width for alignment (7 characters total).
/// Supports ns, μs, ms, s, m, h for materializations
#[derive(Clone, Copy)]
pub struct FixedWidthDuration(pub Duration);

impl fmt::Display for FixedWidthDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_secs = self.0.as_secs_f64();

        if total_secs > 86400.0 {
            // > 1 day: show fixed indicator for very long operations
            f.write_str("   >24h")
        } else if total_secs == 0.0 {
            f.write_str("-------")
        } else if total_secs < 60.0 {
            write!(f, "{total_secs:6.2}s")
        } else if total_secs < 3600.0 {
            let total_minutes = total_secs / 60.0;
            let minutes = total_minutes.floor();
            let seconds = (total_secs % 60.0).floor();
            write!(f, " {minutes:2.0}m{seconds:2.0}s")
        } else {
            let total_hours = total_secs / 3600.0;
            let hours = total_hours.floor();
            let minutes = ((total_secs % 3600.0) / 60.0).floor();
            write!(f, " {hours:2.0}h{minutes:2.0}m")
        }
    }
}

/// Displays a timestamp in ISO format with microseconds: HH:MM:SS.microseconds
///
/// Time zone information is ignored and only the time portion is written.
#[derive(Clone, Copy)]
pub struct TimeOnlyTimestamp(pub SystemTime);

impl fmt::Display for TimeOnlyTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let datetime: DateTime<Utc> = self.0.into();
        // Formatting the naive time skips the offset string `DateTime::format` allocates.
        datetime.naive_utc().format("%H:%M:%S%.6f").write_to(f)
    }
}

/// Displays a timestamp as ISO 8601 with microseconds and Zulu timezone
/// for compatibility with dbt-core python logs, e.g. `2026-01-02T03:04:05.678901Z`.
#[derive(Clone, Copy)]
pub struct UtcZuluTimestamp(pub SystemTime);

impl fmt::Display for UtcZuluTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let datetime: DateTime<Utc> = self.0.into();
        // Same as `to_rfc3339_opts(SecondsFormat::Micros, true)` for years 0 to 9999.
        // Formatting the naive time skips the offset string `DateTime::format` allocates.
        datetime
            .naive_utc()
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .write_to(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_fixed_width() {
        use std::time::Duration;
        assert_eq!(
            FixedWidthDuration(Duration::from_secs(90061)).to_string(),
            "   >24h"
        );
        assert_eq!(
            FixedWidthDuration(Duration::from_secs(0)).to_string(),
            "-------"
        );
        assert_eq!(
            FixedWidthDuration(Duration::from_millis(760)).to_string(),
            "  0.76s"
        );
        assert_eq!(
            FixedWidthDuration(Duration::from_millis(52760)).to_string(),
            " 52.76s"
        );
        assert_eq!(
            FixedWidthDuration(Duration::from_secs(301)).to_string(),
            "  5m 1s"
        );
        assert_eq!(
            FixedWidthDuration(Duration::from_secs(342)).to_string(),
            "  5m42s"
        );
        assert_eq!(
            FixedWidthDuration(Duration::from_secs(3900)).to_string(),
            "  1h 5m"
        );
        assert_eq!(
            FixedWidthDuration(Duration::from_secs(83400)).to_string(),
            " 23h10m"
        );
    }
}
