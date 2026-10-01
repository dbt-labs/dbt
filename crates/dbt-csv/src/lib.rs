//! Custom CSV reading for dbt seeds
//!
//! This crate is a fork of arrow-csv, designed for reading CSV files with
//! Python/dbt/agate-compatible inference and parsing logic.
//!
//! # Usage
//!
//! ```ignore
//! use dbt_csv::reader::{Format, ReaderBuilder};
//! use std::io::Cursor;
//!
//! let data = std::fs::read("path/to/file.csv")?;
//! let format = Format::new(b',', true); // delimiter, has_header
//! let (agate_schema, _) = format.infer_agate_schema(Cursor::new(&data), None)?;
//! let reader = ReaderBuilder::new(agate_schema)
//!     .with_header(true)
//!     .build(Cursor::new(data))?;
//! let batches: Vec<_> = reader.collect::<Result<Vec<_>, _>>()?;
//! ```
//!
//! # Type Inference
//!
//! Type inference follows Python's agate library behavior:
//! - Type priority: Integer > Number > Date > DateTime > ISODateTime > Boolean > Text
//! - Only "true"/"false" are valid booleans (not "yes"/"no", "1"/"0")
//! - Null values are "" and "null" (case-insensitive)

pub mod reader;
pub mod type_tester;

pub use self::reader::Format;
pub use self::reader::NullRegex;
pub use self::reader::Reader;
pub use self::reader::ReaderBuilder;
pub use self::type_tester::{AgateColumn, AgateSchema, AgateType};

use arrow_array::RecordBatch;
use arrow_schema::{ArrowError, SchemaRef};
use std::io::Cursor;
use std::path::Path;

fn map_csv_error(error: csv::Error) -> ArrowError {
    match error.kind() {
        csv::ErrorKind::Io(error) => ArrowError::CsvError(error.to_string()),
        csv::ErrorKind::Utf8 { pos, err } => ArrowError::CsvError(format!(
            "Encountered UTF-8 error while reading CSV file: {}{}",
            err,
            pos.as_ref()
                .map(|pos| format!(" at line {}", pos.line()))
                .unwrap_or_default(),
        )),
        csv::ErrorKind::UnequalLengths {
            pos,
            expected_len,
            len,
        } => ArrowError::CsvError(format!(
            "Encountered unequal lengths between records on CSV file. Expected {} \
                 records, found {} records{}",
            expected_len,
            len,
            pos.as_ref()
                .map(|pos| format!(" at line {}", pos.line()))
                .unwrap_or_default(),
        )),
        _ => ArrowError::CsvError("Error reading CSV file".to_string()),
    }
}

/// Options for custom CSV reading.
#[derive(Debug, Clone)]
pub struct CustomCsvOptions {
    /// Field delimiter
    pub delimiter: u8,
    // Should deduplicate headers share the same names
    pub disambiguate_header: bool,
    /// Max records to use for schema inference
    pub max_records_for_inference: Option<usize>,
    /// Column names to force as Text type (case-sensitive exact match).
    /// These columns skip type inference and are always treated as UTF8 strings.
    /// This matches agate's behavior when users specify column types in dbt's YAML config.
    pub text_columns: Vec<String>,
}

impl Default for CustomCsvOptions {
    fn default() -> Self {
        CustomCsvOptions {
            delimiter: b',',
            disambiguate_header: true,
            max_records_for_inference: None,
            text_columns: Vec::new(),
        }
    }
}

impl CustomCsvOptions {
    /// Set the field delimiter
    pub fn with_delimiter(mut self, delimiter: u8) -> Self {
        self.delimiter = delimiter;
        self
    }

    /// Set whether the CSV should deduplicate header names
    pub fn with_dedupe_header(mut self, dedupe_header: bool) -> Self {
        self.disambiguate_header = dedupe_header;
        self
    }

    /// Set column names to force as Text type (case-sensitive exact match).
    /// These columns skip type inference and are always treated as UTF8 strings.
    pub fn with_text_columns(mut self, text_columns: Vec<String>) -> Self {
        self.text_columns = text_columns;
        self
    }
}

/// Result of reading a CSV file into Arrow record batches.
#[derive(Debug)]
pub struct CsvReadResult {
    /// The Arrow schema (guaranteed even if batches is empty)
    pub schema: SchemaRef,
    /// The record batches containing the data
    pub batches: Vec<RecordBatch>,
    /// Column names from `text_columns` that didn't match any CSV header
    pub unmatched_text_columns: Vec<String>,
}

/// Check if the first line of data is empty (only contains line terminators).
/// This matches agate's behavior where an empty header line triggers letter name generation.
fn is_first_line_empty(data: &[u8]) -> bool {
    matches!(data.first(), Some(b'\n') | Some(b'\r'))
}

/// Skip an empty first line (starts with line terminator).
/// Only call when is_first_line_empty() returns true.
fn skip_empty_first_line(data: &[u8]) -> &[u8] {
    match data.first() {
        Some(b'\n') => &data[1..],
        Some(b'\r') if data.get(1) == Some(&b'\n') => &data[2..],
        Some(b'\r') => &data[1..],
        _ => data,
    }
}

pub fn read_to_arrow_records(
    path: &Path,
    options: &CustomCsvOptions,
) -> Result<CsvReadResult, ArrowError> {
    let data = std::fs::read(path).map_err(|e| ArrowError::CsvError(e.to_string()))?;

    // Determine if file has a header row (agate compatibility)
    // If first line is empty, eat the line and treat as no-header (autogen letter name)
    let (has_header, data_for_processing) = if is_first_line_empty(&data) {
        (false, skip_empty_first_line(&data))
    } else {
        (true, data.as_slice())
    };

    // Use Python-compatible type inference (returns AgateSchema)
    // Pass text_columns to force those columns as Text type
    let format = Format::new(options.delimiter, has_header, options.disambiguate_header)
        .with_truncated_rows(true);
    let (agate_schema, _, unmatched_text_columns) = format.infer_agate_schema_with_text_columns(
        Cursor::new(data_for_processing),
        options.max_records_for_inference,
        &options.text_columns,
    )?;

    // Get Arrow schema (available even if CSV has no data rows)
    let schema = agate_schema.to_arrow_schema_ref();

    // Read data with flexible row handling (matches Python csv module)
    // ReaderBuilder now takes AgateSchema and parses according to AgateType semantics
    let reader: reader::BufReader<std::io::BufReader<Cursor<&[u8]>>> =
        ReaderBuilder::new(agate_schema)
            .with_format(format)
            .build(Cursor::new(data_for_processing))?;

    let batches = reader.collect::<Result<Vec<_>, _>>()?;

    Ok(CsvReadResult {
        schema,
        batches,
        unmatched_text_columns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::cast::AsArray;
    use arrow_array::types::{Float64Type, Int64Type};
    use std::path::Path;

    /// Helper to verify empty header CSV produces letter names and correct data
    fn verify_empty_header_csv(path: &Path) {
        let options = CustomCsvOptions::default();
        let result = read_to_arrow_records(path, &options).unwrap();

        // Should have 3 columns with letter names
        assert_eq!(result.schema.fields().len(), 3);
        assert_eq!(result.schema.field(0).name(), "a");
        assert_eq!(result.schema.field(1).name(), "b");
        assert_eq!(result.schema.field(2).name(), "c");

        // Should have 1 batch with 2 rows
        assert_eq!(result.batches.len(), 1);
        let batch = &result.batches[0];
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 3);

        // Verify data values
        let col_a = batch.column(0).as_primitive::<Int64Type>();
        assert_eq!(col_a.value(0), 1);
        assert_eq!(col_a.value(1), 2);

        let col_b = batch.column(1).as_string::<i32>();
        assert_eq!(col_b.value(0), "hello");
        assert_eq!(col_b.value(1), "world");

        let col_c = batch.column(2).as_primitive::<Float64Type>();
        assert!((col_c.value(0) - 3.5).abs() < 0.001);
        assert!((col_c.value(1) - 2.5).abs() < 0.001);
    }

    #[test]
    fn test_read_csv_with_empty_header_line_lf() {
        // Test empty header line with \n terminator
        verify_empty_header_csv(Path::new("test/data/empty_header.csv"));
    }

    #[test]
    fn test_read_csv_with_empty_header_line_cr() {
        // Test empty header line with \r terminator
        verify_empty_header_csv(Path::new("test/data/empty_header_cr.csv"));
    }

    #[test]
    fn test_read_csv_with_empty_header_line_crlf() {
        // Test empty header line with \r\n terminator
        verify_empty_header_csv(Path::new("test/data/empty_header_crlf.csv"));
    }
}
