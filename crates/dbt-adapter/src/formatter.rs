use chrono::{DateTime, SecondsFormat, Utc};
use dbt_adapter_core::AdapterType;
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};
use minijinja::Value;
use minijinja::value::ValueKind;
use minijinja_contrib::modules::py_datetime::date::PyDate;
use minijinja_contrib::modules::py_datetime::datetime::PyDateTime;

/// Formatter for SQL Literals.
///
/// Differences in SQL dialects are handled by matching on the [AdapterType].
pub struct SqlLiteralFormatter {
    adapter_type: AdapterType,
}

impl SqlLiteralFormatter {
    pub fn new(adapter_type: AdapterType) -> Self {
        Self { adapter_type }
    }

    pub fn format_bool(&self, b: bool) -> String {
        match self.adapter_type {
            AdapterType::Fabric | AdapterType::SqlServer => {
                if b {
                    "1".to_string()
                } else {
                    "0".to_string()
                }
            }
            _ => {
                if b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
        }
    }

    pub fn format_str(&self, l: &str) -> String {
        match self.adapter_type {
            AdapterType::Bigquery | AdapterType::Databricks => {
                // BigQuery and Databricks uses \ for string escapes
                // https://docs.databricks.com/aws/en/sql/language-manual/data-types/string-type
                let escaped_str = l.replace("'", "\\'");
                format!("'{escaped_str}'")
            }
            AdapterType::Snowflake => {
                let escaped_str = l.replace('\\', "\\\\").replace('\'', "''");
                format!("'{escaped_str}'")
            }
            _ => {
                // XXX: this of course not enough for all strings in any SQL dialect
                // but it's a start
                let escaped_str = l.replace("'", "''");
                format!("'{escaped_str}'")
            }
        }
    }

    /// Formats a string value using dbt's unit-test fixture escaping contract.
    pub fn format_unit_test_str(&self, value: &str) -> String {
        match self.adapter_type {
            AdapterType::Snowflake => format!("'{}'", value.replace('\'', "\\'")),
            _ => self.format_str(value),
        }
    }

    pub fn format_bytes(&self, bytes_value: &Value) -> String {
        assert!(bytes_value.kind() == ValueKind::Bytes);
        format!("'{bytes_value}'")
    }

    pub fn format_date(&self, l: PyDate) -> String {
        match self.adapter_type {
            // Exasol: typed DATE literal, independent of NLS_DATE_FORMAT.
            AdapterType::Exasol => format!("DATE '{}'", l.date.format("%Y-%m-%d")),
            _ => format!("'{}'", l.date.format("%Y-%m-%d")),
        }
    }

    pub fn format_datetime(&self, l: PyDateTime) -> String {
        match self.adapter_type {
            // Exasol: typed TIMESTAMP literal, NLS-independent. A quoted ISO
            // string would be cast via NLS_TIMESTAMP_FORMAT, which rejects 'T'.
            // TIMESTAMP literals also take no time-zone offset, so tz-aware
            // values are converted to UTC and formatted naive.
            AdapterType::Exasol => {
                use minijinja_contrib::modules::py_datetime::datetime::DateTimeState;
                let naive = match &l.state {
                    DateTimeState::Naive(ndt) => *ndt,
                    DateTimeState::Aware(adt) => adt.naive_utc(),
                    DateTimeState::FixedOffset(fdt) => fdt.naive_utc(),
                };
                let formatted = naive.format("%Y-%m-%d %H:%M:%S%.6f").to_string();
                let formatted = formatted
                    .strip_suffix(".000000")
                    .map(str::to_string)
                    .unwrap_or(formatted);
                format!("TIMESTAMP '{formatted}'")
            }
            _ => format!("'{}'", l.isoformat()),
        }
    }

    /// Format a UTC timestamp as a SQL literal for this adapter.
    ///
    /// RFC 3339 allows any number of fractional-second digits (`"." 1*DIGIT`); chrono
    /// emits nanoseconds by default because `DateTime<Utc>` is two `u32`s internally
    /// (seconds + nanoseconds). BigQuery's TIMESTAMP parser is stricter than the RFC
    /// and caps at microseconds, so we truncate to 6 digits to avoid a runtime parse error.
    pub fn format_timestamp(&self, ts: DateTime<Utc>) -> String {
        match self.adapter_type {
            AdapterType::Bigquery => ts.to_rfc3339_opts(SecondsFormat::Micros, true),
            _ => ts.to_rfc3339(),
        }
    }

    pub fn none_value(&self) -> String {
        "NULL".to_string()
    }
}

/// Append a single Jinja value to `result` as a SQL literal for
/// `adapter_type`'s dialect (destination-passing style: no per-value
/// allocation for the dispatch itself, e.g. the numeric fallback formats
/// straight into `result` via `Write` instead of `value.to_string()`).
///
/// Shared by [`format_sql_with_bindings`] (placeholder substitution) and by
/// callers that build literal SQL when driver-level parameter binding is
/// unavailable — both can be called once per cell in a large table, so a
/// `-> String` return here would mean one allocation per value.
pub fn push_value_as_sql_literal(adapter_type: AdapterType, value: &Value, result: &mut String) {
    let formatter = SqlLiteralFormatter::new(adapter_type);
    match value.kind() {
        ValueKind::String => result.push_str(&formatter.format_str(value.as_str().unwrap())),
        ValueKind::Bytes => result.push_str(&formatter.format_bytes(value)),
        ValueKind::None => result.push_str(&formatter.none_value()),
        ValueKind::Bool => result.push_str(&formatter.format_bool(value.is_true())),
        _ => {
            // TODO: handle the SQL escaping of more data types
            if let Some(date) = value.downcast_object::<PyDate>() {
                result.push_str(&formatter.format_date(date.as_ref().clone()));
            } else if let Some(datetime) = value.downcast_object::<PyDateTime>() {
                result.push_str(&formatter.format_datetime(datetime.as_ref().clone()));
            } else {
                use std::fmt::Write;
                let _ = write!(result, "{value}");
            }
        }
    }
}

/// Splits `sql` on the dialect's binding placeholder (`?` for Fabric, `%s`
/// otherwise) and interleaves it with `bindings`, formatted as SQL literals.
/// The split is naive text splitting, not SQL-aware parsing: the first chunk
/// (before any placeholder) is emitted as-is, then each subsequent chunk is
/// preceded by the next formatted binding value.
pub fn format_sql_with_bindings(
    adapter_type: AdapterType,
    sql: &str,
    bindings: &Value,
) -> AdapterResult<String> {
    let mut result = String::with_capacity(sql.len());
    // this placeholder char is seen from `get_binding_char` macro
    let binding_char = if matches!(adapter_type, AdapterType::Fabric | AdapterType::SqlServer) {
        "?"
    } else {
        "%s"
    };
    let mut parts = sql.split(binding_char);
    let mut binding_iter = bindings.as_object().unwrap().try_iter().unwrap();

    if let Some(first) = parts.next() {
        result.push_str(first);
    }

    for part in parts {
        match binding_iter.next() {
            Some(value) => push_value_as_sql_literal(adapter_type, &value, &mut result),
            None => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "Not enough bindings provided for SQL template".to_string(),
                ));
            }
        }
        result.push_str(part);
    }

    if binding_iter.next().is_some() {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "Too many bindings provided for SQL template".to_string(),
        ));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only convenience wrapper around [`push_value_as_sql_literal`].
    /// Not exposed outside tests: production call sites push into a shared
    /// buffer instead of allocating a `String` per value.
    fn format_value_as_sql_literal(adapter_type: AdapterType, value: &Value) -> String {
        let mut result = String::new();
        push_value_as_sql_literal(adapter_type, value, &mut result);
        result
    }

    #[test]
    fn format_timestamp_bigquery_truncates_to_microseconds() {
        // BigQuery rejects nanosecond-precision RFC 3339 strings at parse time.
        // Verify we emit exactly 6 fractional digits regardless of input precision.
        let ts = DateTime::from_timestamp(1_700_000_000, 999).unwrap(); // 999 ns
        let result = SqlLiteralFormatter::new(AdapterType::Bigquery).format_timestamp(ts);
        let frac = result.split('.').nth(1).unwrap();
        assert_eq!(
            &frac[..6],
            "000000",
            "expected 6 fractional digits, got: {result}"
        );
        assert!(
            !frac.chars().nth(6).is_some_and(|c| c.is_ascii_digit()),
            "must not emit sub-microsecond digits: {result}"
        );
    }

    #[test]
    fn format_timestamp_non_bigquery_preserves_nanoseconds() {
        // Non-BigQuery adapters must round-trip sub-microsecond precision;
        // truncating here would silently corrupt time windows on those platforms.
        let ts = DateTime::from_timestamp(1_700_000_000, 999).unwrap(); // 999 ns
        let result = SqlLiteralFormatter::new(AdapterType::Snowflake).format_timestamp(ts);
        assert!(
            result.contains("000000999"),
            "expected nanoseconds in output, got: {result}"
        );
    }

    #[test]
    fn format_timestamp_bigquery_uses_utc_z_suffix() {
        // BigQuery TIMESTAMP literals must carry an explicit UTC indicator.
        // to_rfc3339_opts with use_z=true produces "Z"; confirm we don't emit "+00:00".
        let ts = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let result = SqlLiteralFormatter::new(AdapterType::Bigquery).format_timestamp(ts);
        assert!(
            result.ends_with('Z'),
            "expected Z suffix for UTC, got: {result}"
        );
    }

    #[test]
    fn test_bigquery_format_str() {
        let formatter = SqlLiteralFormatter::new(AdapterType::Bigquery);
        assert_eq!(formatter.format_str("hello"), "'hello'");
        assert_eq!(formatter.format_str("it's"), "'it\\'s'");
        assert_eq!(formatter.format_str("it's a test's"), "'it\\'s a test\\'s'");
        assert_eq!(formatter.format_str(""), "''");
        assert_eq!(formatter.format_str("\\"), "'\\'");
        assert_eq!(formatter.format_str("\\'"), "'\\\\''");
    }

    #[test]
    fn test_databricks_format_str() {
        let formatter = SqlLiteralFormatter::new(AdapterType::Databricks);

        assert_eq!(formatter.format_str("hello"), "'hello'");
        assert_eq!(formatter.format_str("it's"), "'it\\'s'");
        assert_eq!(formatter.format_str("it's a test's"), "'it\\'s a test\\'s'");
        assert_eq!(formatter.format_str(""), "''");
        assert_eq!(formatter.format_str("\\"), "'\\'");
        assert_eq!(formatter.format_str("\\'"), "'\\\\''");
    }

    #[test]
    fn test_snowflake_format_str() {
        let f = SqlLiteralFormatter::new(AdapterType::Snowflake);

        assert_eq!(f.format_str(""), "''");
        assert_eq!(f.format_str("hello"), "'hello'");
        assert_eq!(f.format_str("Mom\\Baby"), "'Mom\\\\Baby'");
        assert_eq!(f.format_str("it's"), "'it''s'");
    }

    #[test]
    fn test_snowflake_format_unit_test_str() {
        let f = SqlLiteralFormatter::new(AdapterType::Snowflake);
        let nested_json = r#"{"Data":[{"Value":"[{\\"id\\":\\"128091\\"}]"}]}"#;

        assert_eq!(
            f.format_unit_test_str(nested_json),
            format!("'{nested_json}'")
        );
        assert_eq!(f.format_unit_test_str("they're"), r"'they\'re'");
    }

    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    use minijinja_contrib::modules::py_datetime::datetime::DateTimeState;

    /// Build a naive (no-timezone) PyDateTime directly from its public fields.
    fn naive_datetime(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> PyDateTime {
        let date = NaiveDate::from_ymd_opt(y, mo, d).unwrap();
        let time = NaiveTime::from_hms_opt(h, mi, s).unwrap();
        PyDateTime {
            state: DateTimeState::Naive(NaiveDateTime::new(date, time)),
            tzinfo: None,
        }
    }

    fn py_date(y: i32, mo: u32, d: u32) -> PyDate {
        PyDate::new(NaiveDate::from_ymd_opt(y, mo, d).unwrap())
    }

    #[test]
    fn test_exasol_format_datetime() {
        let f = SqlLiteralFormatter::new(AdapterType::Exasol);
        let out = f.format_datetime(naive_datetime(2024, 1, 1, 8, 0, 0));
        assert!(
            out.starts_with("TIMESTAMP '"),
            "expected TIMESTAMP literal, got {out}"
        );
        assert!(out.contains(' '), "expected a space separator, got {out}");
        // trim the keyword first — `TIMESTAMP` legitimately contains a 'T'
        let quoted = out.trim_start_matches("TIMESTAMP ");
        assert!(
            !quoted.contains('T'),
            "Exasol timestamp value must not use the ISO 'T' separator, got {out}"
        );
        assert_eq!(out, "TIMESTAMP '2024-01-01 08:00:00'");
    }

    #[test]
    fn test_exasol_format_datetime_tz_aware() {
        use chrono::{FixedOffset, TimeZone};
        let f = SqlLiteralFormatter::new(AdapterType::Exasol);
        // Exasol TIMESTAMP literals reject offsets ('Z', '+02:00'); aware
        // values must be converted to UTC and rendered naive.
        let fdt = FixedOffset::east_opt(2 * 3600)
            .unwrap()
            .with_ymd_and_hms(2024, 1, 1, 10, 0, 0)
            .unwrap();
        let dt = PyDateTime {
            state: DateTimeState::FixedOffset(fdt),
            tzinfo: None,
        };
        assert_eq!(f.format_datetime(dt), "TIMESTAMP '2024-01-01 08:00:00'");
    }

    #[test]
    fn test_exasol_format_date() {
        let f = SqlLiteralFormatter::new(AdapterType::Exasol);
        let out = f.format_date(py_date(2024, 1, 1));
        // Exasol emits a typed DATE literal.
        assert!(
            out.starts_with("DATE '"),
            "expected DATE literal, got {out}"
        );
        assert_eq!(out, "DATE '2024-01-01'");
    }

    #[test]
    fn test_postgres_format_datetime() {
        let f = SqlLiteralFormatter::new(AdapterType::Postgres);
        let out = f.format_datetime(naive_datetime(2024, 1, 1, 8, 0, 0));
        // Non-Exasol adapters keep the quoted ISO string with the 'T' separator.
        assert!(
            out.starts_with('\''),
            "expected a quoted literal, got {out}"
        );
        assert!(out.contains('T'), "expected ISO 'T' separator, got {out}");
        assert_eq!(out, "'2024-01-01T08:00:00'");
    }

    #[test]
    fn test_postgres_format_date() {
        let f = SqlLiteralFormatter::new(AdapterType::Postgres);
        let out = f.format_date(py_date(2024, 1, 1));
        // Non-Exasol adapters emit a plain quoted date string.
        assert_eq!(out, "'2024-01-01'");
    }

    #[test]
    fn value_as_sql_literal_quotes_and_escapes_strings() {
        assert_eq!(
            format_value_as_sql_literal(AdapterType::LakeCompute, &Value::from("hello")),
            "'hello'"
        );
        assert_eq!(
            format_value_as_sql_literal(AdapterType::LakeCompute, &Value::from("it's a test")),
            "'it''s a test'"
        );
    }

    #[test]
    fn value_as_sql_literal_formats_none_as_null() {
        assert_eq!(
            format_value_as_sql_literal(AdapterType::LakeCompute, &Value::from(())),
            "NULL"
        );
    }

    #[test]
    fn value_as_sql_literal_formats_booleans() {
        assert_eq!(
            format_value_as_sql_literal(AdapterType::LakeCompute, &Value::from(true)),
            "true"
        );
        assert_eq!(
            format_value_as_sql_literal(AdapterType::LakeCompute, &Value::from(false)),
            "false"
        );
    }

    #[test]
    fn value_as_sql_literal_formats_date() {
        let date = PyDate::new(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
        let value = Value::from_object(date);
        assert_eq!(
            format_value_as_sql_literal(AdapterType::LakeCompute, &value),
            "'2026-01-15'"
        );
    }

    #[test]
    fn value_as_sql_literal_falls_back_to_display_for_numbers() {
        assert_eq!(
            format_value_as_sql_literal(AdapterType::LakeCompute, &Value::from(42i64)),
            "42"
        );
    }
}
