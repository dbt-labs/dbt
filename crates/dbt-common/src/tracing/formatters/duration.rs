use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};

/// Format elapsed duration for final invocation summaries.
///
/// The formatting targets a compact single token suitable for `[<value>]` blocks,
/// matching the desired UX in the run result output.
pub fn format_duration_for_summary(duration: Duration) -> String {
    let total_secs = duration.as_secs_f64();

    if total_secs >= 3600.0 {
        let hours = (total_secs / 3600.0).floor() as u64;
        let minutes = ((total_secs % 3600.0) / 60.0).floor() as u64;
        let seconds = total_secs % 60.0;
        if seconds >= 1.0 {
            format!("{hours}h {minutes}m {seconds:.0}s")
        } else if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else if total_secs >= 60.0 {
        let minutes = (total_secs / 60.0).floor() as u64;
        let seconds = total_secs % 60.0;
        if seconds >= 1.0 {
            format!("{minutes}m {seconds:.0}s")
        } else {
            format!("{minutes}m")
        }
    } else if total_secs >= 1.0 {
        format!("{total_secs:.1}s")
    } else {
        let millis = duration.as_millis();
        if millis >= 1 {
            format!("{millis}ms")
        } else {
            let micros = duration.as_micros();
            if micros >= 1 {
                format!("{micros}us")
            } else {
                format!("{}ns", duration.as_nanos())
            }
        }
    }
}

/// Format duration with fixed width for alignment (7 characters total)
/// Supports ns, μs, ms, s, m, h for materializations
pub fn format_duration_fixed_width(duration: Duration) -> String {
    let total_secs = duration.as_secs_f64();

    if total_secs > 86400.0 {
        // > 1 day: show fixed indicator for very long operations
        "   >24h".to_string()
    } else if total_secs == 0.0 {
        "-------".to_string()
    } else if total_secs < 60.0 {
        format!("{total_secs:6.2}s")
    } else if total_secs < 3600.0 {
        let total_minutes = total_secs / 60.0;
        let minutes = total_minutes.floor();
        let seconds = (total_secs % 60.0).floor();
        format!(" {minutes:2.0}m{seconds:2.0}s")
    } else {
        let total_hours = total_secs / 3600.0;
        let hours = total_hours.floor();
        let minutes = ((total_secs % 3600.0) / 60.0).floor();
        format!(" {hours:2.0}h{minutes:2.0}m")
    }
}

/// Format a timestamp in ISO format with microseconds: HH:MM:SS.microseconds
///
/// This function will ignore time zone information and only output the time portion.
pub fn format_timestamp_time_only(time: SystemTime) -> String {
    let datetime: DateTime<Utc> = time.into();
    datetime.format("%H:%M:%S%.6f").to_string()
}

/// Format SystemTime as ISO 8601 timestamp string with Zulu timezone
/// for compatibility with dbt-core python logs
pub fn format_timestamp_utc_zulu(ts: SystemTime) -> String {
    // Convert to chrono UTC datetime
    let datetime: DateTime<Utc> = ts.into();
    datetime.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_fixed_width() {
        use std::time::Duration;
        assert_eq!(
            format_duration_fixed_width(Duration::from_secs(90061)),
            "   >24h"
        );
        assert_eq!(
            format_duration_fixed_width(Duration::from_secs(0)),
            "-------"
        );
        assert_eq!(
            format_duration_fixed_width(Duration::from_millis(760)),
            "  0.76s"
        );
        assert_eq!(
            format_duration_fixed_width(Duration::from_millis(52760)),
            " 52.76s"
        );
        assert_eq!(
            format_duration_fixed_width(Duration::from_secs(301)),
            "  5m 1s"
        );
        assert_eq!(
            format_duration_fixed_width(Duration::from_secs(342)),
            "  5m42s"
        );
        assert_eq!(
            format_duration_fixed_width(Duration::from_secs(3900)),
            "  1h 5m"
        );
        assert_eq!(
            format_duration_fixed_width(Duration::from_secs(83400)),
            " 23h10m"
        );
    }
}
