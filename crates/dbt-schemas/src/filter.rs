use chrono::{DateTime, Duration, Utc};
use dbt_common::{ErrorCode, FsResult, fs_err};
use serde::{Deserialize, Serialize};

/// The filter options for a dbt run
#[derive(Debug, Clone, Default)]
pub struct RunFilter {
    pub empty: bool,
    pub sample: Option<Sample>,
}

/// The sample window for a dbt run
#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}

impl Default for Sample {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            start: Some(now),
            end: Some(now),
        }
    }
}

impl RunFilter {
    /// Returns true if any of the filters are active
    pub fn enabled(&self) -> bool {
        self.empty || self.sample.is_some()
    }

    // Convenience fn to access the sample start & end times
    pub fn sample_times(&self) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
        let sample = self.sample.as_ref();
        let sample_start = sample.and_then(|s| s.start);
        let sample_end = sample.and_then(|s| s.end);
        (sample_start, sample_end)
    }

    /// Validate then collect the filter options into a [RunFilter] struct
    pub fn try_from(empty: bool, sample: Option<String>) -> FsResult<Self> {
        let parsed_sample = if let Some(sample_str) = sample {
            Some(Self::parse_sample(&sample_str)?)
        } else {
            None
        };

        Ok(Self {
            empty,
            sample: parsed_sample,
        })
    }

    /// Parse the sample string into a Sample struct
    ///
    /// Allows either as a relative range from now "3 days", "6 hours"
    /// or as a absolute range, for example "{'start': '2024-07-01', 'end': '2024-07-08 18:00:00'}"
    ///
    /// reference: https://docs.getdbt.com/docs/build/sample-flag#examples
    fn parse_sample(sample_str: &str) -> FsResult<Sample> {
        if let Ok(sample) = Self::try_parse_abs_range(sample_str) {
            Ok(sample)
        } else {
            Self::try_parse_relative_range(sample_str)
        }
    }

    fn try_parse_abs_range(sample_str: &str) -> FsResult<Sample> {
        let normalized = sample_str.replace('\'', "\"");

        #[derive(Deserialize)]
        struct SampleJson {
            start: Option<String>,
            end: Option<String>,
        }
        let sample_json: SampleJson = serde_json::from_str(&normalized)?;

        // Parse the datetime strings
        let start = if let Some(start_str) = sample_json.start {
            Some(Self::parse_datetime_string(&start_str)?)
        } else {
            None
        };

        let end = if let Some(end_str) = sample_json.end {
            Some(Self::parse_datetime_string(&end_str)?)
        } else {
            None
        };

        Ok(Sample { start, end })
    }

    fn parse_datetime_string(datetime_str: &str) -> FsResult<DateTime<Utc>> {
        // TODO: verify the formats below are all supported by dbt-core
        // reference: https://github.com/dbt-labs/dbt-core/blob/98711cec7550d93fb7f6210e3693715fa54c030b/core/dbt/cli/option_types.py#L112
        static ALLOWED_FORMATS: [&str; 6] = [
            "%Y-%m-%d",
            "%Y-%m-%d %H:%M:%S",
            "%Y-%m-%d %H:%M",
            "%Y-%m-%dT%H:%M:%S",
            "%Y-%m-%dT%H:%M:%SZ",
            "%Y-%m-%dT%H:%M:%S%.fZ",
        ];

        for format in ALLOWED_FORMATS {
            if let Ok(naive_dt) = chrono::NaiveDateTime::parse_from_str(datetime_str, format) {
                return Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive_dt, Utc));
            }

            // For date-only formats, try parsing as NaiveDate and convert to datetime
            if format == "%Y-%m-%d"
                && let Ok(naive_date) = chrono::NaiveDate::parse_from_str(datetime_str, format)
            {
                let naive_dt = naive_date.and_hms_opt(0, 0, 0).unwrap();
                return Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive_dt, Utc));
            }
        }

        Err(fs_err!(
            ErrorCode::Generic,
            "Unable to parse datetime string: {}",
            datetime_str
        ))
    }

    /// reference: https://github.com/dbt-labs/dbt-core/blob/5f873da929dfeb8d70a46d6f2cde0f54a8a556bb/core/dbt/event_time/sample_window.py#L33
    fn try_parse_relative_range(sample_str: &str) -> FsResult<Sample> {
        let duration = humantime::parse_duration(sample_str).map_err(|e| {
            fs_err!(
                ErrorCode::Generic,
                "Failed to parse duration '{sample_str}': {e}",
            )
        })?;

        let duration = Duration::from_std(duration)
            .map_err(|e| fs_err!(ErrorCode::Generic, "Failed to convert duration: {}", e))?;

        let end = Utc::now();
        let start = end - duration;
        Ok(Sample {
            start: Some(start),
            end: Some(end),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_sample() {
        use chrono::NaiveDate;

        // Test JSON format with single quotes
        let json_sample = "{'start': '2024-07-01', 'end': '2024-07-08 18:00:00'}";
        let result = RunFilter::try_from(true, Some(json_sample.to_string()));
        assert!(result.is_ok());
        let config = result.unwrap();
        let sample = config.sample.as_ref().unwrap();

        // Check start date (2024-07-01 00:00:00 UTC)
        let expected_start = NaiveDate::from_ymd_opt(2024, 7, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(sample.start.unwrap().naive_utc(), expected_start);

        // Check end date (2024-07-08 18:00:00 UTC)
        let expected_end = NaiveDate::from_ymd_opt(2024, 7, 8)
            .unwrap()
            .and_hms_opt(18, 0, 0)
            .unwrap();
        assert_eq!(sample.end.unwrap().naive_utc(), expected_end);

        // Test JSON format with double quotes
        let json_sample = r#"{"start": "2024-07-01", "end": "2024-07-08 18:00:00"}"#;
        let result = RunFilter::try_from(true, Some(json_sample.to_string()));
        assert!(result.is_ok());
        let config = result.unwrap();
        let sample = config.sample.as_ref().unwrap();
        assert_eq!(sample.start.unwrap().naive_utc(), expected_start);
        assert_eq!(sample.end.unwrap().naive_utc(), expected_end);
    }

    #[test]
    fn test_parse_duration_sample() {
        // Test duration format
        let duration_sample = "3 days";
        let result = RunFilter::try_from(true, Some(duration_sample.to_string()));
        assert!(result.is_ok());
        let config = result.unwrap();
        let sample = config.sample.as_ref().unwrap();

        // Verify that start is 3 days before end
        let start = sample.start.unwrap();
        let end = sample.end.unwrap();
        let duration = end.signed_duration_since(start);
        assert_eq!(duration.num_days(), 3);

        // Verify end time is approximately now (within a few seconds)
        let now = Utc::now();
        let diff = (now - end).num_seconds().abs();
        assert!(
            diff < 5,
            "End time should be close to current time, diff: {diff} seconds"
        );

        // Test hours format
        let duration_sample = "6 hours";
        let result = RunFilter::try_from(true, Some(duration_sample.to_string()));
        assert!(result.is_ok());
        let config = result.unwrap();
        let sample = config.sample.as_ref().unwrap();

        // Verify that start is 6 hours before end
        let start = sample.start.unwrap();
        let end = sample.end.unwrap();
        let duration = end.signed_duration_since(start);
        assert_eq!(duration.num_hours(), 6);
    }

    #[test]
    fn test_invalid_sample_format() {
        // Test invalid format
        let invalid_sample = "invalid format";
        let result = RunFilter::try_from(true, Some(invalid_sample.to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn test_no_sample() {
        // Test with no sample
        let result = RunFilter::try_from(true, None);
        assert!(result.is_ok());
        let config = result.unwrap();
        assert!(config.sample.is_none());
    }
}
