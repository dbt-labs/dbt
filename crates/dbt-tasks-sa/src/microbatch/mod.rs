//! Microbatch incremental strategy implementation.
//!
//! This module provides the infrastructure for executing models with the microbatch
//! incremental strategy, which processes data in time-based windows (batches).

use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc};
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::common::DbtBatchSize;
use serde::{Deserialize, Serialize};

/// Context for a single batch within a microbatch execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchContext {
    /// Unique identifier for the batch (e.g., "20240115", "2024011512")
    pub id: String,
    /// Start of the batch window (inclusive)
    pub event_time_start: DateTime<Utc>,
    /// End of the batch window (exclusive)
    pub event_time_end: DateTime<Utc>,
    /// Index of this batch (0-based)
    pub batch_index: usize,
    /// Total number of batches
    pub total_batches: usize,
}

impl BatchContext {
    /// Create a new batch context.
    pub fn new(
        id: String,
        event_time_start: DateTime<Utc>,
        event_time_end: DateTime<Utc>,
        batch_index: usize,
        total_batches: usize,
    ) -> Self {
        Self {
            id,
            event_time_start,
            event_time_end,
            batch_index,
            total_batches,
        }
    }

    /// Returns true if this is the first batch.
    pub fn is_first(&self) -> bool {
        self.batch_index == 0
    }

    /// Returns true if this is the last batch.
    pub fn is_last(&self) -> bool {
        self.batch_index == self.total_batches.saturating_sub(1)
    }
}

/// Builder for creating batch windows for microbatch execution.
///
/// Follows the dbt-core pattern for batch window calculation,
/// supporting hour, day, month, and year granularities.
#[derive(Debug, Clone)]
pub struct MicrobatchBuilder {
    /// The granularity of batches
    batch_size: DbtBatchSize,
    /// The earliest time to process (from model config `begin`)
    begin: DateTime<Utc>,
    /// Number of previous batches to reprocess (from model config `lookback`)
    lookback: i32,
}

impl MicrobatchBuilder {
    /// Create a new MicrobatchBuilder.
    ///
    /// # Arguments
    /// * `batch_size` - The granularity of batches (hour, day, month, year)
    /// * `begin` - The earliest time to process
    /// * `lookback` - Number of previous batches to reprocess
    pub fn new(batch_size: DbtBatchSize, begin: DateTime<Utc>, lookback: i32) -> Self {
        Self {
            batch_size,
            begin,
            lookback,
        }
    }

    /// Create a MicrobatchBuilder from model configuration.
    ///
    /// # Arguments
    /// * `batch_size` - The batch size from model config
    /// * `begin_str` - The begin date string from model config (format: "YYYY-MM-DD")
    /// * `lookback` - The lookback value from model config (default: 1)
    pub fn from_config(
        batch_size: Option<DbtBatchSize>,
        begin_str: Option<&str>,
        lookback: Option<i32>,
    ) -> FsResult<Self> {
        let batch_size = batch_size.ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "Microbatch models require `batch_size` configuration"
            )
        })?;

        let begin_str = begin_str.ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "Microbatch models require `begin` configuration"
            )
        })?;

        let begin = Self::parse_begin_date(begin_str)?;
        let lookback = lookback.unwrap_or(1);

        Ok(Self::new(batch_size, begin, lookback))
    }

    /// Parse a begin date string into a DateTime.
    fn parse_begin_date(begin_str: &str) -> FsResult<DateTime<Utc>> {
        if let Ok(date) = NaiveDate::parse_from_str(begin_str, "%Y-%m-%d") {
            let datetime = date.and_hms_opt(0, 0, 0).ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Invalid begin date: {}",
                    begin_str
                )
            })?;
            return Ok(DateTime::<Utc>::from_naive_utc_and_offset(datetime, Utc));
        }

        [
            "%Y-%m-%d %H:%M:%S",
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%d %H:%M:%S%.f",
            "%Y-%m-%dT%H:%M:%S%.f",
        ]
        .iter()
        .find_map(|fmt| chrono::NaiveDateTime::parse_from_str(begin_str, fmt).ok())
        .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
        .ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "Unable to parse begin date '{}'. Expected format: YYYY-MM-DD or YYYY-MM-DD HH:MM:SS",
                begin_str
            )
        })
    }

    /// Calculate the start time for batch processing.
    ///
    /// If a checkpoint is provided, use it (with lookback adjustment).
    /// Otherwise, use the configured `begin` time.
    pub fn build_start_time(
        &self,
        checkpoint: Option<DateTime<Utc>>,
        event_start_time: Option<String>,
        is_incremental: bool,
    ) -> FsResult<DateTime<Utc>> {
        if let Some(start_str) = event_start_time {
            let parsed_start = Self::parse_begin_date(&start_str)?;
            return Ok(self.truncate_timestamp(parsed_start));
        }

        let Some(base_time) = checkpoint.filter(|_| is_incremental) else {
            return Ok(self.truncate_timestamp(self.begin));
        };

        // Mirror dbt-core's `MicrobatchBuilder.build_start_time`: offset back from
        // the checkpoint by `lookback` batches. When the checkpoint sits exactly on
        // a batch boundary the final batch ends at the checkpoint, so we must look
        // back one extra batch to include the current batch — otherwise we emit
        // `lookback` batches instead of the expected `lookback + 1`. Since the
        // checkpoint is `build_end_time`'s ceiling'd end, it is normally on a
        // boundary and the `+ 1` applies.
        let lookback = if self.truncate_timestamp(base_time) == base_time {
            self.lookback + 1
        } else {
            self.lookback
        };
        let lookback_time = self.offset_timestamp(self.truncate_timestamp(base_time), -lookback);
        let clamped_time = std::cmp::max(self.begin, lookback_time);

        Ok(self.truncate_timestamp(clamped_time))
    }

    /// Calculate the end time for batch processing.
    ///
    /// Returns the current time truncated to the batch boundary.
    pub fn build_end_time(&self, event_end_time: Option<String>) -> FsResult<DateTime<Utc>> {
        let end_time = match event_end_time {
            Some(s) => Self::parse_begin_date(&s)?,
            None => Utc::now(),
        };

        Ok(self.ceiling_timestamp(end_time))
    }

    /// Build all batch contexts between start and end times.
    pub fn build_batches(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<BatchContext> {
        let mut batches = Vec::new();
        let mut current = self.truncate_timestamp(start);
        let end = self.truncate_timestamp(end);

        // First pass: collect all batch boundaries
        let mut boundaries = Vec::new();
        while current < end {
            let next = self.offset_timestamp(current, 1);
            boundaries.push((current, next));
            current = next;
        }

        let total_batches = boundaries.len();

        // Second pass: create batch contexts
        for (index, (batch_start, batch_end)) in boundaries.into_iter().enumerate() {
            let id = self.batch_id(batch_start);
            batches.push(BatchContext::new(
                id,
                batch_start,
                batch_end,
                index,
                total_batches,
            ));
        }

        batches
    }

    /// Truncate a timestamp to the batch boundary (floor).
    ///
    /// For example, with batch_size=Day, "2024-01-15 14:30:00" becomes "2024-01-15 00:00:00".
    pub fn truncate_timestamp(&self, ts: DateTime<Utc>) -> DateTime<Utc> {
        match self.batch_size {
            DbtBatchSize::Hour => ts
                .with_minute(0)
                .and_then(|t| t.with_second(0))
                .and_then(|t| t.with_nanosecond(0))
                .unwrap_or(ts),
            DbtBatchSize::Day => ts
                .with_hour(0)
                .and_then(|t| t.with_minute(0))
                .and_then(|t| t.with_second(0))
                .and_then(|t| t.with_nanosecond(0))
                .unwrap_or(ts),
            DbtBatchSize::Month => {
                let naive = NaiveDate::from_ymd_opt(ts.year(), ts.month(), 1)
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .unwrap_or_else(|| ts.naive_utc());
                DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
            }
            DbtBatchSize::Year => {
                let naive = NaiveDate::from_ymd_opt(ts.year(), 1, 1)
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .unwrap_or_else(|| ts.naive_utc());
                DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
            }
        }
    }

    /// Round up a timestamp to the next batch boundary (ceiling).
    ///
    /// If the timestamp is already at a boundary, it remains unchanged.
    pub fn ceiling_timestamp(&self, ts: DateTime<Utc>) -> DateTime<Utc> {
        let truncated = self.truncate_timestamp(ts);
        if truncated == ts {
            ts
        } else {
            self.offset_timestamp(truncated, 1)
        }
    }

    /// Offset a timestamp by a number of batches.
    ///
    /// Positive offset moves forward, negative offset moves backward.
    pub fn offset_timestamp(&self, ts: DateTime<Utc>, offset: i32) -> DateTime<Utc> {
        match self.batch_size {
            DbtBatchSize::Hour => ts + Duration::hours(offset as i64),
            DbtBatchSize::Day => ts + Duration::days(offset as i64),
            DbtBatchSize::Month => {
                let total_months = ts.year() * 12 + ts.month() as i32 - 1 + offset;
                let year = total_months.div_euclid(12);
                let month = (total_months.rem_euclid(12) + 1) as u32;
                let day = ts.day().min(days_in_month(year, month));
                let naive = NaiveDate::from_ymd_opt(year, month, day)
                    .and_then(|d| d.and_hms_opt(ts.hour(), ts.minute(), ts.second()))
                    .unwrap_or_else(|| ts.naive_utc());
                DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
            }
            DbtBatchSize::Year => {
                let year = ts.year() + offset;
                let month = ts.month();
                let day = ts.day().min(days_in_month(year, month));
                let naive = NaiveDate::from_ymd_opt(year, month, day)
                    .and_then(|d| d.and_hms_opt(ts.hour(), ts.minute(), ts.second()))
                    .unwrap_or_else(|| ts.naive_utc());
                DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
            }
        }
    }

    /// Generate a batch ID for a given timestamp.
    ///
    /// The format depends on the batch size:
    /// - Hour: "YYYYMMDDHH"
    /// - Day: "YYYYMMDD"
    /// - Month: "YYYYMM"
    /// - Year: "YYYY"
    pub fn batch_id(&self, ts: DateTime<Utc>) -> String {
        match self.batch_size {
            DbtBatchSize::Hour => ts.format("%Y%m%d%H").to_string(),
            DbtBatchSize::Day => ts.format("%Y%m%d").to_string(),
            DbtBatchSize::Month => ts.format("%Y%m").to_string(),
            DbtBatchSize::Year => ts.format("%Y").to_string(),
        }
    }

    /// Get the batch size.
    pub fn batch_size(&self) -> &DbtBatchSize {
        &self.batch_size
    }

    /// Get the begin time.
    pub fn begin(&self) -> &DateTime<Utc> {
        &self.begin
    }

    /// Get the lookback value.
    pub fn lookback(&self) -> i32 {
        self.lookback
    }
}

/// Helper function to get the number of days in a month.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 30, // fallback
    }
}

/// Helper function to check if a year is a leap year.
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_datetime(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
        let naive = NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, 0, 0)
            .unwrap();
        DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
    }

    #[test]
    fn test_truncate_timestamp_hour() {
        let builder = MicrobatchBuilder::new(DbtBatchSize::Hour, make_datetime(2024, 1, 1, 0), 0);
        let ts = make_datetime(2024, 1, 15, 14);
        let ts = ts.with_minute(30).unwrap();
        let truncated = builder.truncate_timestamp(ts);
        assert_eq!(truncated, make_datetime(2024, 1, 15, 14));
    }

    #[test]
    fn test_truncate_timestamp_day() {
        let builder = MicrobatchBuilder::new(DbtBatchSize::Day, make_datetime(2024, 1, 1, 0), 0);
        let ts = make_datetime(2024, 1, 15, 14);
        let truncated = builder.truncate_timestamp(ts);
        assert_eq!(truncated, make_datetime(2024, 1, 15, 0));
    }

    #[test]
    fn test_truncate_timestamp_month() {
        let builder = MicrobatchBuilder::new(DbtBatchSize::Month, make_datetime(2024, 1, 1, 0), 0);
        let ts = make_datetime(2024, 1, 15, 14);
        let truncated = builder.truncate_timestamp(ts);
        assert_eq!(truncated, make_datetime(2024, 1, 1, 0));
    }

    #[test]
    fn test_truncate_timestamp_year() {
        let builder = MicrobatchBuilder::new(DbtBatchSize::Year, make_datetime(2024, 1, 1, 0), 0);
        let ts = make_datetime(2024, 6, 15, 14);
        let truncated = builder.truncate_timestamp(ts);
        assert_eq!(truncated, make_datetime(2024, 1, 1, 0));
    }

    #[test]
    fn test_offset_timestamp_day() {
        let builder = MicrobatchBuilder::new(DbtBatchSize::Day, make_datetime(2024, 1, 1, 0), 0);
        let ts = make_datetime(2024, 1, 15, 0);

        // Forward
        let offset = builder.offset_timestamp(ts, 3);
        assert_eq!(offset, make_datetime(2024, 1, 18, 0));

        // Backward
        let offset = builder.offset_timestamp(ts, -3);
        assert_eq!(offset, make_datetime(2024, 1, 12, 0));
    }

    #[test]
    fn test_offset_timestamp_month() {
        let builder = MicrobatchBuilder::new(DbtBatchSize::Month, make_datetime(2024, 1, 1, 0), 0);
        let ts = make_datetime(2024, 1, 1, 0);

        // Forward
        let offset = builder.offset_timestamp(ts, 3);
        assert_eq!(offset, make_datetime(2024, 4, 1, 0));

        // Backward across year boundary
        let offset = builder.offset_timestamp(ts, -2);
        assert_eq!(offset, make_datetime(2023, 11, 1, 0));
    }

    #[test]
    fn test_batch_id() {
        let builder_hour =
            MicrobatchBuilder::new(DbtBatchSize::Hour, make_datetime(2024, 1, 1, 0), 0);
        let builder_day =
            MicrobatchBuilder::new(DbtBatchSize::Day, make_datetime(2024, 1, 1, 0), 0);
        let builder_month =
            MicrobatchBuilder::new(DbtBatchSize::Month, make_datetime(2024, 1, 1, 0), 0);
        let builder_year =
            MicrobatchBuilder::new(DbtBatchSize::Year, make_datetime(2024, 1, 1, 0), 0);

        let ts = make_datetime(2024, 1, 15, 14);

        assert_eq!(builder_hour.batch_id(ts), "2024011514");
        assert_eq!(builder_day.batch_id(ts), "20240115");
        assert_eq!(builder_month.batch_id(ts), "202401");
        assert_eq!(builder_year.batch_id(ts), "2024");
    }

    #[test]
    fn test_build_batches() {
        let builder = MicrobatchBuilder::new(DbtBatchSize::Day, make_datetime(2024, 1, 1, 0), 0);

        let start = make_datetime(2024, 1, 1, 0);
        let end = make_datetime(2024, 1, 4, 0);

        let batches = builder.build_batches(start, end);

        assert_eq!(batches.len(), 3);

        assert_eq!(batches[0].id, "20240101");
        assert_eq!(batches[0].batch_index, 0);
        assert!(batches[0].is_first());
        assert!(!batches[0].is_last());

        assert_eq!(batches[1].id, "20240102");
        assert_eq!(batches[1].batch_index, 1);
        assert!(!batches[1].is_first());
        assert!(!batches[1].is_last());

        assert_eq!(batches[2].id, "20240103");
        assert_eq!(batches[2].batch_index, 2);
        assert!(!batches[2].is_first());
        assert!(batches[2].is_last());
    }

    #[test]
    fn test_build_start_time_with_lookback() {
        let builder = MicrobatchBuilder::new(
            DbtBatchSize::Day,
            make_datetime(2024, 1, 1, 0),
            2, // lookback of 2 days
        );

        // Checkpoint Jan 10 is on a batch boundary (the final batch ends at Jan 10),
        // so lookback=2 reaches back 3 batches to Jan 7 — yielding batches for
        // Jan 7, 8, 9 (current batch + 2 previous). Matches dbt-core.
        let start = builder
            .build_start_time(Some(make_datetime(2024, 1, 10, 0)), None, true)
            .expect("successful start time build");
        assert_eq!(start, make_datetime(2024, 1, 7, 0));

        // If lookback would go before begin, use begin instead
        let start = builder
            .build_start_time(Some(make_datetime(2024, 1, 2, 0)), None, true)
            .expect("successful start time build");
        assert_eq!(start, make_datetime(2024, 1, 1, 0));
    }

    #[test]
    fn test_lookback_produces_n_plus_one_batches() {
        // Regression for dbt-core#15477: a `lookback` of N must produce N + 1
        // day-batches (the current batch plus N previous), matching dbt-core and
        // the microbatch docs. `end` is `build_end_time`'s ceiling of a mid-day
        // 2024-10-01 run, i.e. the exclusive window end 2024-10-02.
        let builder = MicrobatchBuilder::new(DbtBatchSize::Day, make_datetime(2024, 1, 1, 0), 3);
        let end = make_datetime(2024, 10, 2, 0);

        let start = builder
            .build_start_time(Some(end), None, true)
            .expect("successful start time build");
        assert_eq!(start, make_datetime(2024, 9, 28, 0));

        let ids: Vec<String> = builder
            .build_batches(start, end)
            .into_iter()
            .map(|b| b.id)
            .collect();
        assert_eq!(ids, ["20240928", "20240929", "20240930", "20241001"]);
    }

    #[test]
    fn test_parse_begin_date() {
        // Date only
        let result = MicrobatchBuilder::parse_begin_date("2024-01-15");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), make_datetime(2024, 1, 15, 0));

        // Date with time (space separator)
        let result = MicrobatchBuilder::parse_begin_date("2024-01-15 14:30:00");
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            NaiveDate::from_ymd_opt(2024, 1, 15)
                .unwrap()
                .and_hms_opt(14, 30, 0)
                .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
                .unwrap()
        );

        // ISO 8601 with T separator
        let result = MicrobatchBuilder::parse_begin_date("2024-01-15T14:30:00");
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            NaiveDate::from_ymd_opt(2024, 1, 15)
                .unwrap()
                .and_hms_opt(14, 30, 0)
                .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
                .unwrap()
        );

        // Space separator with fractional seconds (Python str(datetime) output)
        let result = MicrobatchBuilder::parse_begin_date("2023-06-01 09:31:47.317452");
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            NaiveDate::from_ymd_opt(2023, 6, 1)
                .unwrap()
                .and_hms_micro_opt(9, 31, 47, 317452)
                .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
                .unwrap()
        );

        // ISO 8601 with T separator and fractional seconds (isoformat() output)
        let result = MicrobatchBuilder::parse_begin_date("2023-06-01T09:31:47.317452");
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            NaiveDate::from_ymd_opt(2023, 6, 1)
                .unwrap()
                .and_hms_micro_opt(9, 31, 47, 317452)
                .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
                .unwrap()
        );

        // Fractional seconds with fewer digits
        let result = MicrobatchBuilder::parse_begin_date("2024-01-15T14:30:00.123");
        assert!(result.is_ok());

        // Invalid: wrong date order
        let result = MicrobatchBuilder::parse_begin_date("01-15-2024");
        assert!(result.is_err());

        // Invalid: completely bogus
        let result = MicrobatchBuilder::parse_begin_date("not-a-date");
        assert!(result.is_err());

        // Invalid: empty string
        let result = MicrobatchBuilder::parse_begin_date("");
        assert!(result.is_err());

        // Invalid: date with timezone offset (we parse as naive/UTC, not arbitrary zones)
        let result = MicrobatchBuilder::parse_begin_date("2024-01-15T14:30:00+05:00");
        assert!(result.is_err());
    }
}
