//! Python/agate-compatible type tester module for CSV schema inference.
//!
//! This module implements type inference that matches Python's dbt/agate behavior.
//! Types are tested in priority order, and the first matching type is used.
//!
//! # Type Priority (from agate_helper.py's build_type_tester):
//! 0. **Null** - checked FIRST for all types (matches Python's null_values parameter)
//! 1. Integer → DataType::Int64
//! 2. Number (decimal) → DataType::Float64
//! 3. Date (format "%Y-%m-%d") → DataType::Date32 (date-only, days since epoch)
//! 4. DateTime (format "%Y-%m-%d %H:%M:%S") → DataType::Timestamp(Microsecond, None)
//! 5. ISODateTime → DataType::Timestamp(Microsecond, None) (ISO 8601 with optional TZ)
//! 6. Boolean → DataType::Boolean
//! 7. Text → DataType::Utf8
//!
//! # Arrow Type Mapping Rationale
//! - Date (YYYY-MM-DD only) → Date32: Preserves date-only semantics
//! - DateTime/ISODateTime → Timestamp(Microsecond)
//!
//! # Null Handling
//! Every type in Python agate has `null_values=("null", "")`. This means:
//! - Empty strings and "null" (case-insensitive) are treated as NULL
//! - Null values pass the test for ANY type (they don't eliminate type candidates)
//! - The same null handling is used for both inference AND parsing
//!
//! The `NullRegex` from the reader module is used for null checking, ensuring
//! consistency between type inference and parsing.

use crate::reader::NullRegex;
use arrow_schema::DataType;
use regex::Regex;
use std::sync::LazyLock;

// =============================================================================
// Null Handling
// =============================================================================

/// Default NullRegex for type testing.
/// This is the same as the decoder uses, ensuring consistency.
static DEFAULT_NULL_REGEX: LazyLock<NullRegex> = LazyLock::new(NullRegex::default);

/// Convenience function to check if a value is null using the default NullRegex.
/// Matches Python agate's `null_values=("null", "")`.
#[inline]
pub fn is_null_value(s: &str) -> bool {
    DEFAULT_NULL_REGEX.is_null(s)
}

// =============================================================================
// Type Matchers (regex-based, for determining if a string matches a type)
// =============================================================================

/// Regex for matching integers.
/// Matches Python's behavior: optional whitespace, optional sign, digits only.
/// Does NOT match floats (no decimal point).
static INTEGER_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*-?\d+\s*$").expect("Invalid integer regex"));

/// Regex for matching decimal numbers.
/// Matches Python's Number type behavior - handles:
/// - Optional leading/trailing whitespace
/// - Optional sign
/// - Decimal point with digits before and/or after
/// - Scientific notation (e.g., 1.5e10, 2E-3)
/// - Percentage signs (stripped in Python)
/// - Currency symbols are handled separately in Python but we'll be simpler here
static NUMBER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*-?(\d+\.?\d*|\d*\.?\d+)([eE][-+]?\d+)?%?\s*$").expect("Invalid number regex")
});

/// Regex for matching Date format "%Y-%m-%d" (YYYY-MM-DD).
/// This matches agate.data_types.Date with date_format="%Y-%m-%d".
static DATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\d{4}-\d{2}-\d{2}\s*$").expect("Invalid date regex"));

/// Regex for matching DateTime format "%Y-%m-%d %H:%M:%S".
/// This matches agate.data_types.DateTime with datetime_format="%Y-%m-%d %H:%M:%S".
static DATETIME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}\s*$").expect("Invalid datetime regex")
});

/// Regex for matching ISO 8601 datetime strings.
/// This matches dbt's ISODateTime which uses isodate.parse_datetime.
///
/// Supports both standard and compact formats for date AND time:
/// - Standard: `2018-08-06T11:33:29.320Z` (YYYY-MM-DD HH:MM:SS)
/// - Compact date: `20180806T11:33:29.320Z` (YYYYMMDD HH:MM:SS)
/// - Fully compact: `20180806T113329Z` (YYYYMMDD HHMMSS)
///
/// Optional parts:
/// - Fractional seconds: `.ffffff`
/// - Timezone: `Z` or `+HH:MM` / `-HH:MM`
static ISO_DATETIME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // Date: YYYY-MM-DD or YYYYMMDD
    // Time: HH:MM:SS or HHMMSS
    Regex::new(
        r"^\s*(\d{4}-\d{2}-\d{2}|\d{8})[T ](\d{2}:\d{2}:\d{2}|\d{6})(\.\d+)?(Z|[+-]\d{2}:?\d{2})?\s*$",
    )
    .expect("Invalid ISO datetime regex")
});

/// Boolean true values matching Python agate_helper:
/// `true_values=("true",)` - only "true" (case-insensitive)
pub const BOOLEAN_TRUE_VALUES: &[&str] = &["true"];

/// Boolean false values matching Python agate_helper:
/// `false_values=("false",)` - only "false" (case-insensitive)
pub const BOOLEAN_FALSE_VALUES: &[&str] = &["false"];

// =============================================================================
// Type Testing Functions
// =============================================================================

/// Test if a string value matches the Integer type.
/// Returns true if the value is null or can be parsed as an integer.
pub fn test_integer(s: &str) -> bool {
    if is_null_value(s) {
        return true;
    }
    if !INTEGER_REGEX.is_match(s) {
        return false;
    }
    // Also verify it can actually be parsed (handles overflow)
    s.trim().parse::<i64>().is_ok()
}

/// Test if a string value matches the Number (decimal) type.
/// Returns true if the value is null or can be parsed as a decimal number.
pub fn test_number(s: &str) -> bool {
    if is_null_value(s) {
        return true;
    }
    // Special float values
    let trimmed = s.trim();
    if trimmed == "NaN" || trimmed == "nan" || trimmed == "inf" || trimmed == "-inf" {
        return true;
    }
    if !NUMBER_REGEX.is_match(s) {
        return false;
    }
    // Also verify it can actually be parsed
    let trimmed = trimmed.trim_end_matches('%');
    trimmed.parse::<f64>().is_ok()
}

/// Test if a string value matches the Date type (format "%Y-%m-%d").
/// Maps to DataType::Date32 (days since epoch).
pub fn test_date(s: &str) -> bool {
    if is_null_value(s) {
        return true;
    }
    DATE_REGEX.is_match(s)
}

/// Test if a string value matches the DateTime type (format "%Y-%m-%d %H:%M:%S").
/// Maps to DataType::Timestamp(Microsecond, None).
pub fn test_datetime(s: &str) -> bool {
    if is_null_value(s) {
        return true;
    }
    DATETIME_REGEX.is_match(s)
}

/// Test if a string value matches the ISODateTime type.
/// Maps to DataType::Timestamp(Microsecond, None) - datetime with optional timezone.
pub fn test_iso_datetime(s: &str) -> bool {
    if is_null_value(s) {
        return true;
    }
    ISO_DATETIME_REGEX.is_match(s)
}

/// Test if a string value matches the Boolean type.
/// Only accepts "true" and "false" (case-insensitive) per agate_helper config.
pub fn test_boolean(s: &str) -> bool {
    if is_null_value(s) {
        return true;
    }
    let lower = s.trim().to_lowercase();
    BOOLEAN_TRUE_VALUES.contains(&lower.as_str()) || BOOLEAN_FALSE_VALUES.contains(&lower.as_str())
}

/// Test if a string value matches the Text type.
/// Always returns true - Text is the fallback type.
pub fn test_text(_s: &str) -> bool {
    true
}

// =============================================================================
// Type Tester (like Python's agate TypeTester)
// =============================================================================

/// Represents a type that can be tested and inferred.
/// Matches the behavior for types defined in dbt-common/dbt_common/clients/agate_helper.py
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgateType {
    /// Integer type - maps to Int64
    Integer,
    /// Number (decimal) type - maps to Float64
    Number,
    /// Date type (YYYY-MM-DD) - maps to Date32 (days since epoch)
    Date,
    /// DateTime type (YYYY-MM-DD HH:MM:SS) - maps to Timestamp(Microsecond, None)
    DateTime,
    /// ISO 8601 DateTime - maps to Timestamp(Microsecond, None)
    ISODateTime,
    /// Boolean type - maps to Boolean
    Boolean,
    /// Text type (fallback) - maps to Utf8
    Text,
}

impl AgateType {
    /// Get the priority order of types (lower = higher priority).
    /// Matches the order in dbt-common/dbt_common/clients/agate_helper.py::build_type_tester
    pub fn priority(&self) -> u8 {
        match self {
            AgateType::Integer => 0,
            AgateType::Number => 1,
            AgateType::Date => 2,
            AgateType::DateTime => 3,
            AgateType::ISODateTime => 4,
            AgateType::Boolean => 5,
            AgateType::Text => 6,
        }
    }

    /// Convert to Arrow DataType.
    pub fn to_arrow_type(&self) -> DataType {
        use arrow_schema::TimeUnit;
        match self {
            AgateType::Integer => DataType::Int64,
            AgateType::Number => DataType::Float64,
            AgateType::Date => DataType::Date32,
            AgateType::DateTime => DataType::Timestamp(TimeUnit::Microsecond, None),
            AgateType::ISODateTime => DataType::Timestamp(TimeUnit::Microsecond, None),
            AgateType::Boolean => DataType::Boolean,
            AgateType::Text => DataType::Utf8,
        }
    }

    /// Infer AgateType from Arrow DataType.
    ///
    /// This is useful for creating AgateSchema from existing Arrow schemas.
    /// Note: Multiple Arrow types may map to the same AgateType, so this
    /// is not a perfect round-trip.
    pub fn from_arrow_type(data_type: &DataType) -> Self {
        use arrow_schema::TimeUnit;
        match data_type {
            // Integer types
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64 => AgateType::Integer,

            // Number types
            DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Decimal32(_, _)
            | DataType::Decimal64(_, _)
            | DataType::Decimal128(_, _)
            | DataType::Decimal256(_, _) => AgateType::Number,

            // Date types
            DataType::Date32 | DataType::Date64 => AgateType::Date,

            // Timestamp types → ISODateTime (most flexible)
            DataType::Timestamp(TimeUnit::Second, _)
            | DataType::Timestamp(TimeUnit::Millisecond, _)
            | DataType::Timestamp(TimeUnit::Microsecond, _)
            | DataType::Timestamp(TimeUnit::Nanosecond, _) => AgateType::ISODateTime,

            // Boolean
            DataType::Boolean => AgateType::Boolean,

            // Text (and fallback for other types)
            _ => AgateType::Text,
        }
    }

    /// Test if a string value matches this type.
    pub fn test(&self, s: &str) -> bool {
        match self {
            AgateType::Integer => test_integer(s),
            AgateType::Number => test_number(s),
            AgateType::Date => test_date(s),
            AgateType::DateTime => test_datetime(s),
            AgateType::ISODateTime => test_iso_datetime(s),
            AgateType::Boolean => test_boolean(s),
            AgateType::Text => test_text(s),
        }
    }

    /// Get all types in priority order.
    pub fn all_in_priority_order() -> &'static [AgateType] {
        &[
            AgateType::Integer,
            AgateType::Number,
            AgateType::Date,
            AgateType::DateTime,
            AgateType::ISODateTime,
            AgateType::Boolean,
            AgateType::Text,
        ]
    }
}

// =============================================================================
// AgateSchema - Schema with AgateType columns
// =============================================================================

/// A column in an AgateSchema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgateColumn {
    /// Column name
    pub name: String,
    /// Column type (AgateType)
    pub agate_type: AgateType,
}

impl AgateColumn {
    /// Create a new column.
    pub fn new(name: impl Into<String>, agate_type: AgateType) -> Self {
        Self {
            name: name.into(),
            agate_type,
        }
    }

    /// Convert to Arrow Field.
    pub fn to_arrow_field(&self) -> arrow_schema::Field {
        arrow_schema::Field::new(&self.name, self.agate_type.to_arrow_type(), true)
    }
}

/// Schema that stores column names and AgateType per column.
///
/// This is used during CSV parsing to preserve the original type inference
/// information and parse data directly according to AgateType semantics,
/// rather than converting to Arrow types and using Arrow's parsers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgateSchema {
    /// Columns in the schema
    columns: Vec<AgateColumn>,
}

impl AgateSchema {
    /// Create a new schema from columns.
    pub fn new(columns: Vec<AgateColumn>) -> Self {
        Self { columns }
    }

    /// Create from column names and types.
    pub fn from_names_and_types(names: Vec<String>, types: Vec<AgateType>) -> Self {
        assert_eq!(names.len(), types.len());
        let columns = names
            .into_iter()
            .zip(types)
            .map(|(name, agate_type)| AgateColumn { name, agate_type })
            .collect();
        Self { columns }
    }

    /// Create from an Arrow Schema by inferring AgateType from DataType.
    ///
    /// This is useful for tests and backward compatibility. The mapping is:
    /// - Int8/16/32/64, UInt8/16/32/64 → Integer
    /// - Float32/64, Decimal* → Number  
    /// - Date32/64 → Date
    /// - Timestamp → ISODateTime
    /// - Boolean → Boolean
    /// - Utf8/LargeUtf8/Utf8View → Text
    /// - Other types → Text (fallback)
    pub fn from_arrow_schema(schema: &arrow_schema::Schema) -> Self {
        let columns = schema
            .fields()
            .iter()
            .map(|field| {
                let agate_type = AgateType::from_arrow_type(field.data_type());
                AgateColumn::new(field.name(), agate_type)
            })
            .collect();
        Self { columns }
    }

    /// Get the number of columns.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Check if schema is empty.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// Get a column by index.
    pub fn column(&self, index: usize) -> &AgateColumn {
        &self.columns[index]
    }

    /// Get all columns.
    pub fn columns(&self) -> &[AgateColumn] {
        &self.columns
    }

    /// Iterate over columns.
    pub fn iter(&self) -> impl Iterator<Item = &AgateColumn> {
        self.columns.iter()
    }

    /// Convert to Arrow Schema.
    pub fn to_arrow_schema(&self) -> arrow_schema::Schema {
        let fields: Vec<arrow_schema::Field> =
            self.columns.iter().map(|c| c.to_arrow_field()).collect();
        arrow_schema::Schema::new(fields)
    }

    /// Convert to Arrow SchemaRef (Arc<Schema>).
    pub fn to_arrow_schema_ref(&self) -> arrow_schema::SchemaRef {
        std::sync::Arc::new(self.to_arrow_schema())
    }
}

/// Type tester that maintains possible types for a column and narrows them down.
/// Similar to Python's agate TypeTester but simplified for our use case.
#[derive(Debug, Clone)]
pub struct TypeTester {
    /// Possible types remaining for this column
    possible_types: Vec<AgateType>,
}

impl Default for TypeTester {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeTester {
    /// Create a new TypeTester with all types possible.
    pub fn new() -> Self {
        Self {
            possible_types: AgateType::all_in_priority_order().to_vec(),
        }
    }

    /// Test a value and narrow down possible types.
    /// Removes types that don't match the given value.
    pub fn test(&mut self, value: &str) {
        self.possible_types.retain(|t| t.test(value));
    }

    /// Get the best (highest priority) type from remaining possibilities.
    /// Returns Text as fallback if no other types match.
    pub fn get_type(&self) -> AgateType {
        self.possible_types
            .iter()
            .min_by_key(|t| t.priority())
            .copied()
            .unwrap_or(AgateType::Text)
    }

    /// Get the Arrow DataType.
    pub fn get_arrow_type(&self) -> DataType {
        self.get_type().to_arrow_type()
    }

    /// Check if only one type remains.
    pub fn is_determined(&self) -> bool {
        self.possible_types.len() == 1
    }
}

/// Infer types for multiple columns from rows of data.
/// This is the main entry point for type inference.
pub fn infer_column_types<'a, I, R>(
    rows: I,
    num_columns: usize,
    limit: Option<usize>,
) -> Vec<AgateType>
where
    I: IntoIterator<Item = R>,
    R: AsRef<[&'a str]>,
{
    let mut testers: Vec<TypeTester> = (0..num_columns).map(|_| TypeTester::new()).collect();
    let max_rows = limit.unwrap_or(usize::MAX);

    for row in rows.into_iter().take(max_rows) {
        let row_ref = row.as_ref();
        for (i, tester) in testers.iter_mut().enumerate() {
            if i < row_ref.len() {
                tester.test(row_ref[i]);
            }
        }
    }

    testers.iter().map(|t| t.get_type()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // =============================================================================
    // Integer Tests
    // =============================================================================

    #[test]
    fn test_integer_valid() {
        assert!(test_integer("123"));
        assert!(test_integer("-456"));
        assert!(test_integer(" 789 "));
        assert!(test_integer("0"));
        assert!(test_integer("-0"));
    }

    #[test]
    fn test_integer_invalid() {
        assert!(!test_integer("12.34"));
        assert!(!test_integer("abc"));
        assert!(!test_integer("12abc"));
        assert!(!test_integer("1.0"));
    }

    #[test]
    fn test_integer_null() {
        assert!(test_integer(""));
        assert!(test_integer("null"));
        assert!(test_integer("NULL"));
    }

    #[test]
    fn test_integer_overflow() {
        // Should fail for values that overflow i64
        assert!(!test_integer("9999999999999999999999"));
    }

    // =============================================================================
    // Number Tests
    // =============================================================================

    #[test]
    fn test_number_valid() {
        assert!(test_number("12.34"));
        assert!(test_number("-56.78"));
        assert!(test_number(".5"));
        assert!(test_number("5."));
        assert!(test_number("1e10"));
        assert!(test_number("1.5E-3"));
        assert!(test_number(" 12.34 "));
        assert!(test_number("50%"));
    }

    #[test]
    fn test_number_special_values() {
        assert!(test_number("NaN"));
        assert!(test_number("nan"));
        assert!(test_number("inf"));
        assert!(test_number("-inf"));
    }

    #[test]
    fn test_number_null() {
        assert!(test_number(""));
        assert!(test_number("null"));
    }

    #[test]
    fn test_number_integers_also_valid() {
        // Integers are also valid numbers
        assert!(test_number("123"));
        assert!(test_number("-456"));
    }

    // =============================================================================
    // Date Tests
    // =============================================================================

    #[test]
    fn test_date_valid() {
        assert!(test_date("2024-01-15"));
        assert!(test_date(" 2024-01-15 "));
        assert!(test_date("1970-01-01"));
    }

    #[test]
    fn test_date_invalid() {
        assert!(!test_date("2024-1-15")); // Single digit month
        assert!(!test_date("2024/01/15")); // Wrong separator
        assert!(!test_date("01-15-2024")); // Wrong order
        assert!(!test_date("2024-01-15 10:30:00")); // Has time
    }

    #[test]
    fn test_date_null() {
        assert!(test_date(""));
        assert!(test_date("null"));
    }

    // =============================================================================
    // DateTime Tests
    // =============================================================================

    #[test]
    fn test_datetime_valid() {
        assert!(test_datetime("2024-01-15 10:30:00"));
        assert!(test_datetime("2024-01-15T10:30:00"));
        assert!(test_datetime(" 2024-01-15 10:30:00 "));
    }

    #[test]
    fn test_datetime_invalid() {
        assert!(!test_datetime("2024-01-15")); // No time
        assert!(!test_datetime("2024-01-15 10:30")); // Missing seconds
        assert!(!test_datetime("2024-01-15 10:30:00.123")); // Has fractional seconds
    }

    #[test]
    fn test_datetime_null() {
        assert!(test_datetime(""));
        assert!(test_datetime("null"));
    }

    // =============================================================================
    // ISODateTime Tests
    // =============================================================================

    #[test]
    fn test_iso_datetime_valid() {
        assert!(test_iso_datetime("2024-01-15T10:30:00"));
        assert!(test_iso_datetime("2024-01-15T10:30:00Z"));
        assert!(test_iso_datetime("2024-01-15T10:30:00+05:30"));
        assert!(test_iso_datetime("2024-01-15T10:30:00.123456"));
        assert!(test_iso_datetime("2024-01-15 10:30:00"));
    }

    #[test]
    fn test_iso_datetime_null() {
        assert!(test_iso_datetime(""));
        assert!(test_iso_datetime("null"));
    }

    // =============================================================================
    // Boolean Tests
    // =============================================================================

    #[test]
    fn test_boolean_valid() {
        assert!(test_boolean("true"));
        assert!(test_boolean("TRUE"));
        assert!(test_boolean("True"));
        assert!(test_boolean("false"));
        assert!(test_boolean("FALSE"));
        assert!(test_boolean("False"));
        assert!(test_boolean(" true "));
    }

    #[test]
    fn test_boolean_invalid() {
        // Per agate_helper config, only "true" and "false" are valid
        // "yes", "no", "1", "0" are NOT valid
        assert!(!test_boolean("yes"));
        assert!(!test_boolean("no"));
        assert!(!test_boolean("1"));
        assert!(!test_boolean("0"));
        assert!(!test_boolean("t"));
        assert!(!test_boolean("f"));
    }

    #[test]
    fn test_boolean_null() {
        assert!(test_boolean(""));
        assert!(test_boolean("null"));
    }

    // =============================================================================
    // TypeTester Tests
    // =============================================================================

    #[test]
    fn test_type_tester_integer() {
        let mut tester = TypeTester::new();
        tester.test("123");
        tester.test("456");
        tester.test("-789");
        assert_eq!(tester.get_type(), AgateType::Integer);
    }

    #[test]
    fn test_type_tester_number() {
        let mut tester = TypeTester::new();
        tester.test("12.34");
        tester.test("56.78");
        assert_eq!(tester.get_type(), AgateType::Number);
    }

    #[test]
    fn test_type_tester_integer_to_number() {
        let mut tester = TypeTester::new();
        tester.test("123"); // Could be integer or number
        tester.test("45.67"); // Only number
        // Integer has higher priority, but it doesn't match "45.67"
        assert_eq!(tester.get_type(), AgateType::Number);
    }

    #[test]
    fn test_type_tester_date() {
        let mut tester = TypeTester::new();
        tester.test("2024-01-15");
        tester.test("2024-02-20");
        assert_eq!(tester.get_type(), AgateType::Date);
    }

    #[test]
    fn test_type_tester_boolean() {
        let mut tester = TypeTester::new();
        tester.test("true");
        tester.test("false");
        assert_eq!(tester.get_type(), AgateType::Boolean);
    }

    #[test]
    fn test_type_tester_text_fallback() {
        let mut tester = TypeTester::new();
        tester.test("hello");
        tester.test("world");
        assert_eq!(tester.get_type(), AgateType::Text);
    }

    #[test]
    fn test_type_tester_with_nulls() {
        let mut tester = TypeTester::new();
        tester.test("123");
        tester.test(""); // null
        tester.test("456");
        tester.test("null"); // null
        // Nulls should not eliminate any type
        assert_eq!(tester.get_type(), AgateType::Integer);
    }

    #[test]
    fn test_type_tester_mixed_causes_text() {
        let mut tester = TypeTester::new();
        tester.test("123");
        tester.test("abc"); // Not a number
        assert_eq!(tester.get_type(), AgateType::Text);
    }

    // =============================================================================
    // Column Inference Tests
    // =============================================================================

    #[test]
    fn test_infer_column_types() {
        let rows: Vec<Vec<&str>> = vec![
            vec!["1", "10.5", "2024-01-15", "true", "hello"],
            vec!["2", "20.0", "2024-02-20", "false", "world"],
            vec!["3", "30.5", "2024-03-25", "true", "test"],
        ];

        // Convert to the expected format
        let row_refs: Vec<&[&str]> = rows.iter().map(|r| r.as_slice()).collect();
        let types = infer_column_types(row_refs, 5, None);

        assert_eq!(types[0], AgateType::Integer);
        assert_eq!(types[1], AgateType::Number);
        assert_eq!(types[2], AgateType::Date);
        assert_eq!(types[3], AgateType::Boolean);
        assert_eq!(types[4], AgateType::Text);
    }

    #[test]
    fn test_agate_type_to_arrow() {
        use arrow_schema::TimeUnit;

        assert_eq!(AgateType::Integer.to_arrow_type(), DataType::Int64);
        assert_eq!(AgateType::Number.to_arrow_type(), DataType::Float64);
        // Date (YYYY-MM-DD) maps to Date32 - preserves date-only semantics
        assert_eq!(AgateType::Date.to_arrow_type(), DataType::Date32);
        // DateTime maps to Timestamp(Microsecond)
        assert_eq!(
            AgateType::DateTime.to_arrow_type(),
            DataType::Timestamp(TimeUnit::Microsecond, None)
        );
        // ISODateTime also maps to Timestamp(Microsecond) - ISO 8601 with optional TZ
        assert_eq!(
            AgateType::ISODateTime.to_arrow_type(),
            DataType::Timestamp(TimeUnit::Microsecond, None)
        );
        assert_eq!(AgateType::Boolean.to_arrow_type(), DataType::Boolean);
        assert_eq!(AgateType::Text.to_arrow_type(), DataType::Utf8);
    }
}
