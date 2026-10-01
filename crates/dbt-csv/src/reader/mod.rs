//! CSV Reader: this module is a fork of arrow-csv's reader.
//!
//! This reader uses Python/agate-compatible type inference to match dbt behavior.
//!
//! # Basic Usage
//!
//! This CSV reader allows CSV files to be read into the Arrow memory model. Records are
//! loaded in batches and are then converted from row-based data to columnar data.
//!
//! Example:
//!
//! ```ignore
//! # use arrow_schema::*;
//! # use dbt_csv::reader::{Reader, ReaderBuilder, Format};
//! # use std::fs::File;
//! # use std::sync::Arc;
//!
//! let file = File::open("test/data/uk_cities.csv").unwrap();
//! let format = Format::new(b',', true);
//! let (schema, _) = format.infer_schema(&file, None).unwrap();
//!
//! let mut csv = ReaderBuilder::new(crate::type_tester::AgateSchema::from_arrow_schema(&schema)).build(file).unwrap();
//! let batch = csv.next().unwrap().unwrap();
//! ```
//!
//! # Type Inference
//!
//! Type inference follows Python's agate library behavior:
//! - Type priority: Integer > Number > Date > DateTime > ISODateTime > Boolean > Text
//! - Only "true"/"false" are valid booleans
//! - Null values are "" and "null" (case-insensitive)
mod records;

use arrow_array::*;
use arrow_cast::parse::string_to_datetime;
use arrow_schema::*;
use chrono::Utc;
use csv::StringRecord;
use regex::Regex;
use std::fmt::{self, Debug};
use std::io::{BufRead, BufReader as StdBufReader, Read};
use std::sync::Arc;

use crate::map_csv_error;
use crate::reader::records::{RecordDecoder, StringRecords};
use std::sync::LazyLock;

/// Default null regex matching Python agate's `null_values=("null", "")`.
/// Matches empty string OR "null" (case-insensitive).
static DEFAULT_NULL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^$|^(?i)null$").expect("Invalid default null regex"));

/// Null value checker matching Python agate's `null_values=("null", "")`.
///
/// Matches:
/// - Empty strings "" → NULL
/// - "null" (case-insensitive) → NULL
#[derive(Debug, Clone)]
pub struct NullRegex(Regex);

impl Default for NullRegex {
    fn default() -> Self {
        Self(DEFAULT_NULL_REGEX.clone())
    }
}

impl NullRegex {
    /// Returns true if the value should be considered as `NULL`.
    #[inline]
    pub fn is_null(&self, s: &str) -> bool {
        self.0.is_match(s)
    }
}

/// The format specification for the CSV file.
///
/// Uses Python/agate-compatible settings by default:
/// - truncated_rows(true) - allows rows with different numbers of fields
/// - quote("") - treats double quote as an escape character like Python's default
/// - Uses agate-style type inference
#[derive(Debug, Clone)]
pub struct Format {
    header: bool,
    disambiguate_header: bool,
    delimiter: Option<u8>,
    escape: Option<u8>,
    quote: Option<u8>,
    comment: Option<u8>,
    null_regex: NullRegex,
    truncated_rows: bool,
}

impl Default for Format {
    fn default() -> Self {
        Self {
            header: false,
            disambiguate_header: false,
            delimiter: Some(b','),
            escape: None,
            quote: Some(b'"'),
            comment: None,
            null_regex: NullRegex::default(),
            truncated_rows: true, // Python-compatible: flexible(true)
        }
    }
}

impl Format {
    /// Create a new Format with Python-compatible settings.
    ///
    /// This sets up options to match how Python/dbt/agate parse CSVs:
    /// - `flexible(true)` - allows rows with different numbers of fields
    /// - `quoting(true)` - handles quoted fields like Python
    /// - Uses agate-style type inference
    ///
    /// # Arguments
    /// * `delimiter` - The field delimiter character (e.g., b',')
    /// * `has_header` - Whether the first row is a header
    ///
    /// # Example
    /// ```ignore
    /// let format = Format::new(b',', true);
    /// let (schema, _) = format.infer_schema(reader, None)?;
    /// ```
    pub fn new(delimiter: u8, has_header: bool, disambiguate_header: bool) -> Self {
        Self {
            header: has_header,
            disambiguate_header,
            delimiter: Some(delimiter),
            escape: None,
            quote: Some(b'"'),
            comment: None,
            null_regex: NullRegex::default(),
            truncated_rows: true, // flexible(true) equivalent - matches Python
        }
    }

    /// Specify whether the CSV file has a header, defaults to `false`
    ///
    /// When `true`, the first row of the CSV file is treated as a header row
    pub fn with_header(mut self, has_header: bool) -> Self {
        self.header = has_header;
        self
    }

    /// Specify a custom delimiter character, defaults to comma `','`
    pub fn with_delimiter(mut self, delimiter: u8) -> Self {
        self.delimiter = Some(delimiter);
        self
    }

    /// Specify an escape character, defaults to `None`
    pub fn with_escape(mut self, escape: u8) -> Self {
        self.escape = Some(escape);
        self
    }

    /// Specify a custom quote character, defaults to double quote `'"'`
    pub fn with_quote(mut self, quote: u8) -> Self {
        self.quote = Some(quote);
        self
    }

    /// Specify a comment character, defaults to `None`
    ///
    /// Lines starting with this character will be ignored
    pub fn with_comment(mut self, comment: u8) -> Self {
        self.comment = Some(comment);
        self
    }

    /// Whether to allow truncated rows when parsing.
    ///
    /// By default this is set to `true` (Python-compatible) and will allow records
    /// with less than the expected number of columns, filling missing columns with nulls.
    /// When set to false, will error if the CSV rows have different lengths.
    /// If the record's schema is not nullable, then it will still return an error.
    pub fn with_truncated_rows(mut self, allow: bool) -> Self {
        self.truncated_rows = allow;
        self
    }

    /// Infer schema of CSV records from the provided `reader`
    ///
    /// Uses Python/agate-compatible type inference:
    /// - Type priority: Integer > Number > Date > DateTime > ISODateTime > Boolean > Text
    /// - Only "true"/"false" are valid booleans
    /// - Null values are "" and "null" (case-insensitive)
    ///
    /// If `max_records` is `None`, all records will be read, otherwise up to `max_records`
    /// records are read to infer the schema
    ///
    /// Returns inferred schema and number of records read
    pub fn infer_schema<R: Read>(
        &self,
        reader: R,
        max_records: Option<usize>,
    ) -> Result<(Schema, usize), ArrowError> {
        let (agate_schema, records_count) = self.infer_agate_schema(reader, max_records)?;
        Ok((agate_schema.to_arrow_schema(), records_count))
    }

    /// Infer an AgateSchema from the data in a reader.
    ///
    /// This returns the schema with AgateType per column, preserving the original
    /// type inference information. This is useful when you want to parse data
    /// directly according to AgateType semantics rather than Arrow types.
    ///
    /// If `max_records` is `None`, all records will be read, otherwise up to `max_records`
    /// records are read to infer the schema
    ///
    /// Returns inferred AgateSchema and number of records read
    pub fn infer_agate_schema<R: Read>(
        &self,
        reader: R,
        max_records: Option<usize>,
    ) -> Result<(crate::type_tester::AgateSchema, usize), ArrowError> {
        let (schema, records_count, _warnings) =
            self.infer_agate_schema_with_text_columns(reader, max_records, &[])?;
        Ok((schema, records_count))
    }

    /// Infer an AgateSchema from the data in a reader, with forced text columns.
    ///
    /// This is similar to `infer_agate_schema`, but allows specifying columns that
    /// should be forced to `Text` type without inspecting their data. This matches
    /// agate's behavior when users specify column types in dbt's YAML config.
    ///
    /// # Arguments
    /// * `text_columns` - Column names to force as Text type (case-sensitive exact match)
    pub fn infer_agate_schema_with_text_columns<R: Read>(
        &self,
        reader: R,
        max_records: Option<usize>,
        text_columns: &[String],
    ) -> Result<(crate::type_tester::AgateSchema, usize, Vec<String>), ArrowError> {
        use crate::type_tester::{AgateSchema, AgateType, TypeTester};

        let mut csv_reader = self.build_reader(reader);

        // get or create header names
        let headers: Vec<String> = if self.header {
            let headers = &csv_reader.headers().map_err(map_csv_error)?.clone();
            headers.iter().map(|s| s.to_string()).collect()
        } else {
            // No header row - generate letter names like agate does
            // This matches agate's behavior: tuple(utils.letter_name(i) for i in range(len(rows[0])))
            let first_record_count = csv_reader.headers().map_err(map_csv_error)?.len();
            (0..first_record_count).map(Self::letter_name).collect()
        };
        // Apply deduplication if enabled
        let headers = if self.disambiguate_header {
            Self::disambiguate_headers(headers)
        } else {
            headers
        };

        // Build a boolean mask for forced text columns (case-insensitive match using dbt-ident)
        // This handles dialect-specific normalization (e.g., Snowflake uppercases column_types keys)
        // Also track which text_columns weren't found for warning
        let mut is_force_text_col: Vec<bool> = vec![false; headers.len()];
        let mut missing_columns: Vec<String> = Vec::new();

        for text_col in text_columns {
            let text_col_ident = dbt_ident::Ident::new(text_col);
            if let Some(idx) = headers.iter().position(|h| text_col_ident.matches(h)) {
                is_force_text_col[idx] = true;
            } else {
                missing_columns.push(text_col.clone());
            }
        }

        let header_length = headers.len();
        // Use Python-compatible TypeTester for each column
        let mut type_testers: Vec<TypeTester> =
            (0..header_length).map(|_| TypeTester::new()).collect();

        let mut records_count = 0;

        let mut record = StringRecord::new();
        let max_records = max_records.unwrap_or(usize::MAX);
        while records_count < max_records {
            if !csv_reader.read_record(&mut record).map_err(map_csv_error)? {
                break;
            }
            records_count += 1;

            // Test each value against possible types (skip forced text columns)
            for (i, tester) in type_testers.iter_mut().enumerate() {
                // Skip type inference for forced text columns
                if is_force_text_col[i] {
                    continue;
                }
                if let Some(string) = record.get(i) {
                    tester.test(string);
                }
            }
        }

        // Build AgateSchema from inference results
        // For forced text columns, use Text type directly
        let types: Vec<_> = type_testers
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if is_force_text_col[i] {
                    AgateType::Text
                } else {
                    t.get_type()
                }
            })
            .collect();
        let agate_schema = AgateSchema::from_names_and_types(headers, types);

        Ok((agate_schema, records_count, missing_columns))
    }

    /// Deduplicate column names matching agate's utils.deduplicate behavior.
    /// - Empty/whitespace-only names are replaced with letter_name(i): "a", "b", etc.
    /// - Duplicate names get suffixes: ['abc', 'abc', 'cde', 'abc'] -> ['abc', 'abc_2', 'cde', 'abc_3']
    fn disambiguate_headers(headers: Vec<String>) -> Vec<String> {
        use std::collections::HashMap;

        // First pass: replace empty/whitespace-only names with letter_name(i)
        let headers: Vec<String> = headers
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                if name.trim().is_empty() {
                    Self::letter_name(i)
                } else {
                    name
                }
            })
            .collect();

        // Second pass: deduplicate duplicate names
        let mut counts: HashMap<&str, usize> = HashMap::new();

        headers
            .iter()
            .map(|name| {
                let count = counts.entry(name).or_insert(0);
                *count += 1;
                if *count == 1 {
                    name.clone()
                } else {
                    format!("{}_{}", name, *count)
                }
            })
            .collect()
    }

    /// Generate a letter name for a column index, matching agate's letter_name function.
    /// Index 0 -> "a", 1 -> "b", ..., 25 -> "z", 26 -> "aa", 27 -> "bb", ...
    /// Replicating agate/utils.py::letter_name
    fn letter_name(index: usize) -> String {
        const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
        let count = LETTERS.len();
        let letter = LETTERS[index % count] as char;
        let repeat = (index / count) + 1;
        std::iter::repeat_n(letter, repeat).collect()
    }

    /// Build a [`csv::Reader`] for this [`Format`]
    fn build_reader<R: Read>(&self, reader: R) -> csv::Reader<R> {
        let mut builder = csv::ReaderBuilder::new();
        builder.has_headers(self.header);
        builder.flexible(self.truncated_rows);

        if let Some(c) = self.delimiter {
            builder.delimiter(c);
        }
        builder.escape(self.escape);
        if let Some(c) = self.quote {
            builder.quote(c);
        }
        if let Some(comment) = self.comment {
            builder.comment(Some(comment));
        }
        builder.from_reader(reader)
    }

    /// Build a [`csv_core::Reader`] for this [`Format`]
    fn build_parser(&self) -> csv_core::Reader {
        let mut builder = csv_core::ReaderBuilder::new();
        builder.escape(self.escape);
        builder.comment(self.comment);

        if let Some(c) = self.delimiter {
            builder.delimiter(c);
        }
        if let Some(c) = self.quote {
            builder.quote(c);
        }
        builder.build()
    }
}

// optional bounds of the reader, of the form (min line, max line).
type Bounds = Option<(usize, usize)>;

/// CSV file reader using [`std::io::BufReader`]
pub type Reader<R> = BufReader<StdBufReader<R>>;

/// CSV file reader
pub struct BufReader<R> {
    /// File reader
    reader: R,

    /// The decoder
    decoder: Decoder,
}

impl<R> Debug for BufReader<R>
where
    R: BufRead,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reader")
            .field("decoder", &self.decoder)
            .finish()
    }
}

impl<R: Read> Reader<R> {
    /// Returns the schema of the reader, useful for getting the schema without reading
    /// record batches
    pub fn schema(&self) -> SchemaRef {
        match &self.decoder.projection {
            Some(projection) => {
                let fields = self.decoder.arrow_schema.fields();
                let projected = projection.iter().map(|i| fields[*i].clone());
                Arc::new(Schema::new(projected.collect::<Fields>()))
            }
            None => self.decoder.arrow_schema.clone(),
        }
    }
}

impl<R: BufRead> BufReader<R> {
    fn read(&mut self) -> Result<Option<RecordBatch>, ArrowError> {
        loop {
            let buf = self.reader.fill_buf()?;
            // uses `self.delimiter=csv_core::Reader` to perform 0-copy parsing
            // that handles delimiter, quote and escape characters,
            // load the data into self.decoder's internal buffer
            // called `data` with offsets for each row
            let decoded = self.decoder.decode(buf)?;
            self.reader.consume(decoded);
            // Yield if decoded no bytes or the decoder is full
            //
            // The capacity check avoids looping around and potentially
            // blocking reading data in fill_buf that isn't needed
            // to flush the next batch
            if decoded == 0 || self.decoder.capacity() == 0 {
                break;
            }
        }

        // convert internal buffer with offsets into StringRecords
        // call parse_agate on StringRecords
        self.decoder.flush()
    }
}

impl<R: BufRead> Iterator for BufReader<R> {
    type Item = Result<RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.read().transpose()
    }
}

impl<R: BufRead> RecordBatchReader for BufReader<R> {
    fn schema(&self) -> SchemaRef {
        self.decoder.arrow_schema.clone()
    }
}

/// A push-based interface for decoding CSV data from an arbitrary byte stream
///
/// See [`Reader`] for a higher-level interface for interface with [`Read`]
///
/// The push-based interface facilitates integration with sources that yield arbitrarily
/// delimited bytes ranges, such as [`BufRead`], or a chunked byte stream received from
/// object storage
///
/// ```ignore
/// # use std::io::BufRead;
/// # use arrow_array::RecordBatch;
/// # use arrow_csv::ReaderBuilder;
/// # use arrow_schema::{ArrowError, SchemaRef};
/// #
/// fn read_from_csv<R: BufRead>(
///     mut reader: R,
///     schema: SchemaRef,
///     batch_size: usize,
/// ) -> Result<impl Iterator<Item = Result<RecordBatch, ArrowError>>, ArrowError> {
///     let mut decoder = ReaderBuilder::new(crate::type_tester::AgateSchema::from_arrow_schema(&schema))
///         .with_batch_size(batch_size)
///         .build_decoder();
///
///     let mut next = move || {
///         loop {
///             let buf = reader.fill_buf()?;
///             let decoded = decoder.decode(buf)?;
///             if decoded == 0 {
///                 break;
///             }
///
///             // Consume the number of bytes read
///             reader.consume(decoded);
///         }
///         decoder.flush()
///     };
///     Ok(std::iter::from_fn(move || next().transpose()))
/// }
/// ```
#[derive(Debug)]
pub struct Decoder {
    /// AgateSchema for the CSV file - all parsing follows AgateType semantics
    agate_schema: crate::type_tester::AgateSchema,

    /// Cached Arrow schema (derived from agate_schema)
    arrow_schema: SchemaRef,

    /// Optional projection for which columns to load (zero-based column indices)
    projection: Option<Vec<usize>>,

    /// Number of records per batch
    batch_size: usize,

    /// Rows to skip
    to_skip: usize,

    /// Current line number
    line_number: usize,

    /// End line number
    end: usize,

    /// A decoder for [`StringRecords`]
    record_decoder: RecordDecoder,

    /// Check if the string matches this pattern for `NULL`.
    null_regex: NullRegex,
}

impl Decoder {
    /// Decode records from `buf` returning the number of bytes read
    ///
    /// This method returns once `batch_size` objects have been parsed since the
    /// last call to [`Self::flush`], or `buf` is exhausted. Any remaining bytes
    /// should be included in the next call to [`Self::decode`]
    ///
    /// There is no requirement that `buf` contains a whole number of records, facilitating
    /// integration with arbitrary byte streams, such as that yielded by [`BufRead`] or
    /// network sources such as object storage
    pub fn decode(&mut self, buf: &[u8]) -> Result<usize, ArrowError> {
        if self.to_skip != 0 {
            // Skip in units of `to_read` to avoid over-allocating buffers
            let to_skip = self.to_skip.min(self.batch_size);
            let (skipped, bytes) = self.record_decoder.decode(buf, to_skip)?;
            self.to_skip -= skipped;
            self.record_decoder.clear();
            return Ok(bytes);
        }

        let to_read = self.batch_size.min(self.end - self.line_number) - self.record_decoder.len();
        let (_, bytes) = self.record_decoder.decode(buf, to_read)?;
        Ok(bytes)
    }

    /// Flushes the currently buffered data to a [`RecordBatch`]
    ///
    /// This should only be called after [`Self::decode`] has returned `Ok(0)`,
    /// otherwise may return an error if part way through decoding a record
    ///
    /// Returns `Ok(None)` if no buffered data
    pub fn flush(&mut self) -> Result<Option<RecordBatch>, ArrowError> {
        if self.record_decoder.is_empty() {
            return Ok(None);
        }

        let rows = self.record_decoder.flush()?;

        // Parse using AgateType semantics
        let batch = parse_agate(
            &rows,
            &self.agate_schema,
            Some(self.arrow_schema.metadata.clone()),
            self.projection.as_ref(),
            self.line_number,
            &self.null_regex,
        )?;

        self.line_number += rows.len();
        Ok(Some(batch))
    }

    /// Returns the number of records that can be read before requiring a call to [`Self::flush`]
    pub fn capacity(&self) -> usize {
        self.batch_size - self.record_decoder.len()
    }
}

/// Parses a slice of [`StringRecords`] into a [RecordBatch] using AgateType semantics.
///
/// This function parses CSV data based on AgateType (Integer, Number, Date, DateTime,
/// ISODateTime, Boolean, Text) rather than Arrow DataType. This ensures consistent
/// parsing that matches Python/agate behavior.
fn parse_agate(
    rows: &StringRecords<'_>,
    agate_schema: &crate::type_tester::AgateSchema,
    metadata: Option<std::collections::HashMap<String, String>>,
    projection: Option<&Vec<usize>>,
    line_number: usize,
    null_regex: &NullRegex,
) -> Result<RecordBatch, ArrowError> {
    use crate::type_tester::AgateType;

    let projection: Vec<usize> = match projection {
        Some(v) => v.clone(),
        None => (0..agate_schema.len()).collect(),
    };

    let arrays: Result<Vec<ArrayRef>, _> = projection
        .iter()
        .map(|i| {
            let col_idx = *i;
            let column = agate_schema.column(col_idx);
            match column.agate_type {
                AgateType::Integer => build_integer_array(line_number, rows, col_idx, null_regex),
                AgateType::Number => build_number_array(line_number, rows, col_idx, null_regex),
                AgateType::Date => build_date_array(line_number, rows, col_idx, null_regex),
                AgateType::DateTime => build_datetime_array(line_number, rows, col_idx, null_regex),
                AgateType::ISODateTime => {
                    build_iso_datetime_array(line_number, rows, col_idx, null_regex)
                }
                AgateType::Boolean => build_boolean_array(line_number, rows, col_idx, null_regex),
                AgateType::Text => build_text_array(rows, col_idx, null_regex),
            }
        })
        .collect();

    // Build projected schema
    let projected_fields: Fields = projection
        .iter()
        .map(|i| agate_schema.column(*i).to_arrow_field())
        .collect();

    let projected_schema = Arc::new(match metadata {
        None => Schema::new(projected_fields),
        Some(metadata) => Schema::new_with_metadata(projected_fields, metadata),
    });

    arrays.and_then(|arr| {
        RecordBatch::try_new_with_options(
            projected_schema,
            arr,
            &RecordBatchOptions::new()
                .with_match_field_names(true)
                .with_row_count(Some(rows.len())),
        )
    })
}

// =============================================================================
// AgateType-specific array builders
// =============================================================================

/// Build an Int64 array from integer strings.
/// Trims whitespace and parses integers according to agate semantics.
/// Source: https://github.com/dbt-labs/dbt-common/blob/main/dbt_common/clients/agate_helper.py
fn build_integer_array(
    line_number: usize,
    rows: &StringRecords<'_>,
    col_idx: usize,
    null_regex: &NullRegex,
) -> Result<ArrayRef, ArrowError> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            let s = row.get(col_idx);
            let trimmed = s.trim();
            if null_regex.is_null(trimmed) {
                return Ok(None);
            }
            match trimmed.parse::<i64>() {
                Ok(v) => Ok(Some(v)),
                Err(_) => Err(ArrowError::ParseError(format!(
                    "Error parsing '{}' as Integer at column {} line {}",
                    s,
                    col_idx,
                    line_number + row_index
                ))),
            }
        })
        .collect::<Result<Int64Array, _>>()
        .map(|arr| Arc::new(arr) as ArrayRef)
}

/// Build a Float64 array from number strings.
/// Handles percentages, scientific notation, and special values (NaN, inf).
/// Source: https://github.com/wireservice/agate/blob/553e30ac3a856deffb64178e77a7024bec93028a/agate/data_types/number.py
fn build_number_array(
    line_number: usize,
    rows: &StringRecords<'_>,
    col_idx: usize,
    null_regex: &NullRegex,
) -> Result<ArrayRef, ArrowError> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            let s = row.get(col_idx);
            let trimmed = s.trim();
            if null_regex.is_null(trimmed) {
                return Ok(None);
            }
            match parse_float64(trimmed) {
                Some(v) => Ok(Some(v)),
                None => Err(ArrowError::ParseError(format!(
                    "Error parsing '{}' as Number at column {} line {}",
                    s,
                    col_idx,
                    line_number + row_index
                ))),
            }
        })
        .collect::<Result<Float64Array, _>>()
        .map(|arr| Arc::new(arr) as ArrayRef)
}

/// Build a Date32 array from date strings (YYYY-MM-DD format).
/// Source: https://github.com/wireservice/agate/blob/553e30ac3a856deffb64178e77a7024bec93028a/agate/data_types/date.py
fn build_date_array(
    line_number: usize,
    rows: &StringRecords<'_>,
    col_idx: usize,
    null_regex: &NullRegex,
) -> Result<ArrayRef, ArrowError> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            let s = row.get(col_idx);
            let trimmed = s.trim();
            if null_regex.is_null(trimmed) {
                return Ok(None);
            }
            // Parse YYYY-MM-DD format
            match chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
                Ok(date) => {
                    // Convert to days since epoch (1970-01-01)
                    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
                    let days = (date - epoch).num_days() as i32;
                    Ok(Some(days))
                }
                Err(_) => Err(ArrowError::ParseError(format!(
                    "Error parsing '{}' as Date (YYYY-MM-DD) at column {} line {}",
                    s,
                    col_idx,
                    line_number + row_index
                ))),
            }
        })
        .collect::<Result<Date32Array, _>>()
        .map(|arr| Arc::new(arr) as ArrayRef)
}

/// Build a Timestamp array from datetime strings (YYYY-MM-DD HH:MM:SS format).
/// Source: https://github.com/wireservice/agate/blob/553e30ac3a856deffb64178e77a7024bec93028a/agate/data_types/date_time.py
fn build_datetime_array(
    line_number: usize,
    rows: &StringRecords<'_>,
    col_idx: usize,
    null_regex: &NullRegex,
) -> Result<ArrayRef, ArrowError> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            let s = row.get(col_idx);
            let trimmed = s.trim();
            if null_regex.is_null(trimmed) {
                return Ok(None);
            }
            // Parse YYYY-MM-DD HH:MM:SS or YYYY-MM-DDTHH:MM:SS format
            let datetime = chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S"));

            match datetime {
                Ok(dt) => {
                    // Convert to microseconds since epoch
                    Ok(Some(dt.and_utc().timestamp_micros()))
                }
                Err(_) => Err(ArrowError::ParseError(format!(
                    "Error parsing '{}' as DateTime at column {} line {}",
                    s,
                    col_idx,
                    line_number + row_index
                ))),
            }
        })
        .collect::<Result<TimestampMicrosecondArray, _>>()
        .map(|arr| Arc::new(arr) as ArrayRef)
}

/// Build a Timestamp array from ISO 8601 datetime strings.
/// Supports standard and compact formats with optional timezone.
/// Source: https://github.com/dbt-labs/dbt-common/blob/main/dbt_common/clients/agate_helper.py
fn build_iso_datetime_array(
    line_number: usize,
    rows: &StringRecords<'_>,
    col_idx: usize,
    null_regex: &NullRegex,
) -> Result<ArrayRef, ArrowError> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            let s = row.get(col_idx);
            let trimmed = s.trim();
            if null_regex.is_null(trimmed) {
                return Ok(None);
            }

            // Normalize compact format if needed
            let normalized = normalize_compact_iso_datetime(trimmed);
            let to_parse = normalized.as_deref().unwrap_or(trimmed);

            // Try parsing with Arrow's string_to_datetime (handles timezones)
            match string_to_datetime(&Utc, to_parse) {
                Ok(dt) => {
                    // Convert to microseconds since epoch
                    Ok(Some(dt.timestamp_micros()))
                }
                Err(_) => Err(ArrowError::ParseError(format!(
                    "Error parsing '{}' as ISODateTime at column {} line {}",
                    s,
                    col_idx,
                    line_number + row_index
                ))),
            }
        })
        .collect::<Result<TimestampMicrosecondArray, _>>()
        .map(|arr| Arc::new(arr) as ArrayRef)
}

/// Build a Boolean array from boolean strings.
/// Only accepts "true" and "false" (case-insensitive) per agate semantics.
/// Source: https://github.com/wireservice/agate/blob/553e30ac3a856deffb64178e77a7024bec93028a/agate/data_types/boolean.py
fn build_boolean_array(
    line_number: usize,
    rows: &StringRecords<'_>,
    col_idx: usize,
    null_regex: &NullRegex,
) -> Result<ArrayRef, ArrowError> {
    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            let s = row.get(col_idx);
            let trimmed = s.trim();
            if null_regex.is_null(trimmed) {
                return Ok(None);
            }
            match parse_bool(trimmed) {
                Some(v) => Ok(Some(v)),
                None => Err(ArrowError::ParseError(format!(
                    "Error parsing '{}' as Boolean at column {} line {}",
                    s,
                    col_idx,
                    line_number + row_index
                ))),
            }
        })
        .collect::<Result<BooleanArray, _>>()
        .map(|arr| Arc::new(arr) as ArrayRef)
}

/// Build a Utf8 array from text strings.
/// Source: https://github.com/wireservice/agate/blob/553e30ac3a856deffb64178e77a7024bec93028a/agate/data_types/text.py
fn build_text_array(
    rows: &StringRecords<'_>,
    col_idx: usize,
    null_regex: &NullRegex,
) -> Result<ArrayRef, ArrowError> {
    Ok(Arc::new(
        rows.iter()
            .map(|row| {
                let s = row.get(col_idx);
                let trimmed = s.trim();
                (!null_regex.is_null(trimmed)).then_some(s)
            })
            .collect::<StringArray>(),
    ) as ArrayRef)
}

/// Parse a boolean value, trimming whitespace first.
/// Only accepts "true" and "false" (case-insensitive) to match Python/agate behavior.
/// Source: https://github.com/wireservice/agate/blob/553e30ac3a856deffb64178e77a7024bec93028a/agate/data_types/boolean.py
fn parse_bool(string: &str) -> Option<bool> {
    let trimmed = string.trim();
    if trimmed.eq_ignore_ascii_case("false") {
        Some(false)
    } else if trimmed.eq_ignore_ascii_case("true") {
        Some(true)
    } else {
        None
    }
}

/// Normalize compact ISO datetime formats to standard format that Arrow can parse.
///
/// Handles:
/// - Compact date: `20180806T11:33:29.320Z` -> `2018-08-06T11:33:29.320Z`
/// - Compact time: `2018-08-06T113329Z` -> `2018-08-06T11:33:29Z`
/// - Fully compact: `20180806T113329Z` -> `2018-08-06T11:33:29Z`
fn normalize_compact_iso_datetime(s: &str) -> Option<String> {
    let bytes = s.as_bytes();

    // Find the separator (T or space) position
    let sep_pos = bytes.iter().position(|&b| b == b'T' || b == b' ')?;

    // Check if date is compact (8 digits) or standard (10 chars with dashes)
    let date_part = &s[..sep_pos];
    let separator = s.chars().nth(sep_pos)?;
    let after_sep = &s[sep_pos + 1..];

    let is_compact_date = date_part.len() == 8 && date_part.chars().all(|c| c.is_ascii_digit());
    let is_standard_date = date_part.len() == 10
        && date_part.chars().nth(4) == Some('-')
        && date_part.chars().nth(7) == Some('-');

    if !is_compact_date && !is_standard_date {
        return None;
    }

    // Check if time is compact (6 digits at start) or standard (has colons)
    // Time part may be followed by fractional seconds (.xxx) or timezone (Z or +/-HH:MM)
    let time_digits: String = after_sep
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let is_compact_time = time_digits.len() == 6 && !after_sep.contains(':');
    let rest_after_time = &after_sep[time_digits.len()..];

    // If neither date nor time is compact, no normalization needed
    if !is_compact_date && !is_compact_time {
        return None;
    }

    // Build normalized string
    let normalized_date = if is_compact_date {
        format!(
            "{}-{}-{}",
            &date_part[..4],
            &date_part[4..6],
            &date_part[6..8]
        )
    } else {
        date_part.to_string()
    };

    let normalized_time = if is_compact_time {
        format!(
            "{}:{}:{}{}",
            &time_digits[..2],
            &time_digits[2..4],
            &time_digits[4..6],
            rest_after_time
        )
    } else {
        after_sep.to_string()
    };

    Some(format!(
        "{}{}{}",
        normalized_date, separator, normalized_time
    ))
}

/// Parse a Float64 value with extended support for:
/// - Leading/trailing whitespace
/// - Percentage signs (e.g., "50%" -> 50.0)
/// - Special values: NaN, inf, -inf
///
/// This matches the type_tester's test_number() behavior.
fn parse_float64(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Handle special values (Arrow's Float64Type::parse handles these)
    match trimmed {
        "NaN" | "nan" => return Some(f64::NAN),
        "inf" => return Some(f64::INFINITY),
        "-inf" => return Some(f64::NEG_INFINITY),
        _ => {}
    }

    // Strip percentage sign if present (type_tester allows this)
    let without_percent = trimmed.trim_end_matches('%');
    without_percent.parse::<f64>().ok()
}

/// CSV file reader builder
#[derive(Debug)]
pub struct ReaderBuilder {
    /// AgateSchema of the CSV file - all parsing follows AgateType semantics
    agate_schema: crate::type_tester::AgateSchema,
    /// Format of the CSV file
    format: Format,
    /// Batch size (number of records to load each time)
    ///
    /// The default batch size when using the `ReaderBuilder` is 1024 records
    batch_size: usize,
    /// The bounds over which to scan the reader. `None` starts from 0 and runs until EOF.
    bounds: Bounds,
    /// Optional projection for which columns to load (zero-based column indices)
    projection: Option<Vec<usize>>,
}

impl ReaderBuilder {
    /// Create a new builder for configuring CSV parsing options.
    ///
    /// To convert a builder into a reader, call `ReaderBuilder::build`
    ///
    /// # Example
    ///
    /// ```ignore
    /// # use arrow_csv::{Reader, ReaderBuilder};
    /// # use std::fs::File;
    /// # use std::io::Seek;
    /// # use std::sync::Arc;
    /// # use arrow_csv::reader::Format;
    /// #
    /// let mut file = File::open("test/data/uk_cities_with_headers.csv").unwrap();
    /// // Infer the schema with the first 100 records
    /// let (agate_schema, _) = Format::default().infer_agate_schema(&mut file, Some(100)).unwrap();
    /// file.rewind().unwrap();
    ///
    /// // create a builder
    /// ReaderBuilder::new(agate_schema).build(file).unwrap();
    /// ```
    pub fn new(agate_schema: crate::type_tester::AgateSchema) -> ReaderBuilder {
        Self {
            agate_schema,
            format: Format::default(),
            batch_size: 1024,
            bounds: None,
            projection: None,
        }
    }

    /// Set whether the CSV file has a header
    pub fn with_header(mut self, has_header: bool) -> Self {
        self.format.header = has_header;
        self
    }

    /// Set whether to dedupe conflicting header names
    pub fn with_disambiguate_header(mut self, disambiguate_header: bool) -> Self {
        self.format.disambiguate_header = disambiguate_header;
        self
    }
    /// Overrides the [Format] of this [ReaderBuilder]
    pub fn with_format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }

    /// Set the CSV file's column delimiter as a byte character
    pub fn with_delimiter(mut self, delimiter: u8) -> Self {
        self.format.delimiter = Some(delimiter);
        self
    }

    /// Set the given character as the CSV file's escape character
    pub fn with_escape(mut self, escape: u8) -> Self {
        self.format.escape = Some(escape);
        self
    }

    /// Set the given character as the CSV file's quote character, by default it is double quote
    pub fn with_quote(mut self, quote: u8) -> Self {
        self.format.quote = Some(quote);
        self
    }

    /// Provide a comment character, lines starting with this character will be ignored
    pub fn with_comment(mut self, comment: u8) -> Self {
        self.format.comment = Some(comment);
        self
    }

    /// Set the batch size (number of records to load at one time)
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Set the bounds over which to scan the reader.
    /// `start` and `end` are line numbers.
    pub fn with_bounds(mut self, start: usize, end: usize) -> Self {
        self.bounds = Some((start, end));
        self
    }

    /// Set the reader's column projection
    pub fn with_projection(mut self, projection: Vec<usize>) -> Self {
        self.projection = Some(projection);
        self
    }

    /// Whether to allow truncated rows when parsing.
    ///
    /// By default this is set to `false` and will error if the CSV rows have different lengths.
    /// When set to true then it will allow records with less than the expected number of columns
    /// and fill the missing columns with nulls. If the record's schema is not nullable, then it
    /// will still return an error.
    pub fn with_truncated_rows(mut self, allow: bool) -> Self {
        self.format.truncated_rows = allow;
        self
    }

    /// Create a new `Reader` from a non-buffered reader
    ///
    /// If `R: BufRead` consider using [`Self::build_buffered`] to avoid unnecessary additional
    /// buffering, as internally this method wraps `reader` in [`std::io::BufReader`]
    pub fn build<R: Read>(self, reader: R) -> Result<Reader<R>, ArrowError> {
        self.build_buffered(StdBufReader::new(reader))
    }

    /// Create a new `BufReader` from a buffered reader
    pub fn build_buffered<R: BufRead>(self, reader: R) -> Result<BufReader<R>, ArrowError> {
        Ok(BufReader {
            reader,
            decoder: self.build_decoder(),
        })
    }

    /// Builds a decoder that can be used to decode CSV from an arbitrary byte stream
    pub fn build_decoder(self) -> Decoder {
        let delimiter = self.format.build_parser();
        let record_decoder = RecordDecoder::new(
            delimiter,
            self.agate_schema.len(),
            self.format.truncated_rows,
        );

        let header = self.format.header as usize;

        let (start, end) = match self.bounds {
            Some((start, end)) => (start + header, end + header),
            None => (header, usize::MAX),
        };

        // Derive Arrow schema from AgateSchema
        let arrow_schema = self.agate_schema.to_arrow_schema_ref();

        Decoder {
            agate_schema: self.agate_schema,
            arrow_schema,
            to_skip: start,
            record_decoder,
            line_number: start,
            end,
            projection: self.projection,
            batch_size: self.batch_size,
            null_regex: self.format.null_regex,
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
#[allow(clippy::cognitive_complexity)]
mod tests {
    use super::*;

    use std::fs::File;
    use std::io::{Cursor, Seek, SeekFrom, Write};

    use arrow_array::cast::AsArray;
    use arrow_array::types::{Float64Type, Int64Type};

    #[test]
    fn test_csv() {
        // Using AgateSchema: Utf8 stays Utf8, Float64 stays Float64
        let schema = Schema::new(vec![
            Field::new("city", DataType::Utf8, true),
            Field::new("lat", DataType::Float64, true),
            Field::new("lng", DataType::Float64, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);

        let file = File::open("test/data/uk_cities.csv").unwrap();
        let mut csv = ReaderBuilder::new(agate_schema).build(file).unwrap();
        let batch = csv.next().unwrap().unwrap();
        assert_eq!(37, batch.num_rows());
        assert_eq!(3, batch.num_columns());

        // access data from a primitive array
        let lat = batch.column(1).as_primitive::<Float64Type>();
        assert_eq!(57.653484, lat.value(0));

        // access data from a string array (ListArray<u8>)
        let city = batch.column(0).as_string::<i32>();

        assert_eq!("Aberdeen, Aberdeen City, UK", city.value(13));
    }

    #[test]
    #[ignore] // Need investigation on test failure
    fn test_csv_reader_with_decimal() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("city", DataType::Utf8, false),
            Field::new("lat", DataType::Decimal128(38, 6), false),
            Field::new("lng", DataType::Decimal256(76, 6), false),
        ]));

        let file = File::open("test/data/decimal_test.csv").unwrap();

        let mut csv =
            ReaderBuilder::new(crate::type_tester::AgateSchema::from_arrow_schema(&schema))
                .build(file)
                .unwrap();
        let batch = csv.next().unwrap().unwrap();
        // access data from a primitive array
        let lat = batch
            .column(1)
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .unwrap();

        assert_eq!("57.653484", lat.value_as_string(0));
        assert_eq!("53.002666", lat.value_as_string(1));
        assert_eq!("52.412811", lat.value_as_string(2));
        assert_eq!("51.481583", lat.value_as_string(3));
        assert_eq!("12.123456", lat.value_as_string(4));
        assert_eq!("50.760000", lat.value_as_string(5));
        assert_eq!("0.123000", lat.value_as_string(6));
        assert_eq!("123.000000", lat.value_as_string(7));
        assert_eq!("123.000000", lat.value_as_string(8));
        assert_eq!("-50.760000", lat.value_as_string(9));

        let lng = batch
            .column(2)
            .as_any()
            .downcast_ref::<Decimal256Array>()
            .unwrap();

        assert_eq!("-3.335724", lng.value_as_string(0));
        assert_eq!("-2.179404", lng.value_as_string(1));
        assert_eq!("-1.778197", lng.value_as_string(2));
        assert_eq!("-3.179090", lng.value_as_string(3));
        assert_eq!("-3.179090", lng.value_as_string(4));
        assert_eq!("0.290472", lng.value_as_string(5));
        assert_eq!("0.290472", lng.value_as_string(6));
        assert_eq!("0.290472", lng.value_as_string(7));
        assert_eq!("0.290472", lng.value_as_string(8));
        assert_eq!("0.290472", lng.value_as_string(9));
    }

    #[test]
    #[ignore] // TODO: Need investigation on why test failure
    fn test_csv_reader_with_decimal_3264() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("city", DataType::Utf8, false),
            Field::new("lat", DataType::Decimal32(9, 6), false),
            Field::new("lng", DataType::Decimal64(16, 6), false),
        ]));

        let file = File::open("test/data/decimal_test.csv").unwrap();

        let mut csv =
            ReaderBuilder::new(crate::type_tester::AgateSchema::from_arrow_schema(&schema))
                .build(file)
                .unwrap();
        let batch = csv.next().unwrap().unwrap();
        // access data from a primitive array
        let lat = batch
            .column(1)
            .as_any()
            .downcast_ref::<Decimal32Array>()
            .unwrap();

        assert_eq!("57.653484", lat.value_as_string(0));
        assert_eq!("53.002666", lat.value_as_string(1));
        assert_eq!("52.412811", lat.value_as_string(2));
        assert_eq!("51.481583", lat.value_as_string(3));
        assert_eq!("12.123456", lat.value_as_string(4));
        assert_eq!("50.760000", lat.value_as_string(5));
        assert_eq!("0.123000", lat.value_as_string(6));
        assert_eq!("123.000000", lat.value_as_string(7));
        assert_eq!("123.000000", lat.value_as_string(8));
        assert_eq!("-50.760000", lat.value_as_string(9));

        let lng = batch
            .column(2)
            .as_any()
            .downcast_ref::<Decimal64Array>()
            .unwrap();

        assert_eq!("-3.335724", lng.value_as_string(0));
        assert_eq!("-2.179404", lng.value_as_string(1));
        assert_eq!("-1.778197", lng.value_as_string(2));
        assert_eq!("-3.179090", lng.value_as_string(3));
        assert_eq!("-3.179090", lng.value_as_string(4));
        assert_eq!("0.290472", lng.value_as_string(5));
        assert_eq!("0.290472", lng.value_as_string(6));
        assert_eq!("0.290472", lng.value_as_string(7));
        assert_eq!("0.290472", lng.value_as_string(8));
        assert_eq!("0.290472", lng.value_as_string(9));
    }

    #[test]
    fn test_csv_from_buf_reader() {
        let schema = Schema::new(vec![
            Field::new("city", DataType::Utf8, true),
            Field::new("lat", DataType::Float64, true),
            Field::new("lng", DataType::Float64, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);

        let file_with_headers = File::open("test/data/uk_cities_with_headers.csv").unwrap();
        let file_without_headers = File::open("test/data/uk_cities.csv").unwrap();
        let both_files = file_with_headers
            .chain(Cursor::new("\n".to_string()))
            .chain(file_without_headers);
        let mut csv = ReaderBuilder::new(agate_schema)
            .with_header(true)
            .build(both_files)
            .unwrap();
        let batch = csv.next().unwrap().unwrap();
        assert_eq!(74, batch.num_rows());
        assert_eq!(3, batch.num_columns());
    }

    #[test]
    fn test_csv_with_schema_inference() {
        let mut file = File::open("test/data/uk_cities_with_headers.csv").unwrap();

        // Use infer_agate_schema directly for agate-style parsing
        let (agate_schema, _) = Format::default()
            .with_header(true)
            .infer_agate_schema(&mut file, None)
            .unwrap();

        file.rewind().unwrap();
        let builder = ReaderBuilder::new(agate_schema).with_header(true);

        let mut csv = builder.build(file).unwrap();
        let expected_schema = Schema::new(vec![
            Field::new("city", DataType::Utf8, true),
            Field::new("lat", DataType::Float64, true),
            Field::new("lng", DataType::Float64, true),
        ]);
        assert_eq!(Arc::new(expected_schema), csv.schema());
        let batch = csv.next().unwrap().unwrap();
        assert_eq!(37, batch.num_rows());
        assert_eq!(3, batch.num_columns());

        // access data from a primitive array
        let lat = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(57.653484, lat.value(0));

        // access data from a string array (ListArray<u8>)
        let city = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();

        assert_eq!("Aberdeen, Aberdeen City, UK", city.value(13));
    }

    #[test]
    fn test_csv_with_schema_inference_no_headers() {
        let mut file = File::open("test/data/uk_cities.csv").unwrap();

        let (agate_schema, _) = Format::default()
            .infer_agate_schema(&mut file, None)
            .unwrap();
        file.rewind().unwrap();

        let mut csv = ReaderBuilder::new(agate_schema).build(file).unwrap();

        // csv field names should be letter names (agate behavior)
        let schema = csv.schema();
        assert_eq!("a", schema.field(0).name());
        assert_eq!("b", schema.field(1).name());
        assert_eq!("c", schema.field(2).name());
        let batch = csv.next().unwrap().unwrap();
        let batch_schema = batch.schema();

        assert_eq!(schema, batch_schema);
        assert_eq!(37, batch.num_rows());
        assert_eq!(3, batch.num_columns());

        // access data from a primitive array
        let lat = batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(57.653484, lat.value(0));

        // access data from a string array (ListArray<u8>)
        let city = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();

        assert_eq!("Aberdeen, Aberdeen City, UK", city.value(13));
    }

    #[test]
    fn test_csv_builder_with_bounds() {
        let mut file = File::open("test/data/uk_cities.csv").unwrap();

        // Set the bounds to the lines 0, 1 and 2.
        let (agate_schema, _) = Format::default()
            .infer_agate_schema(&mut file, None)
            .unwrap();
        file.rewind().unwrap();
        let mut csv = ReaderBuilder::new(agate_schema)
            .with_bounds(0, 2)
            .build(file)
            .unwrap();
        let batch = csv.next().unwrap().unwrap();

        // access data from a string array (ListArray<u8>)
        let city = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();

        // The value on line 0 is within the bounds
        assert_eq!("Elgin, Scotland, the UK", city.value(0));

        // The value on line 13 is outside of the bounds. Therefore
        // the call to .value() will panic.
        let result = std::panic::catch_unwind(|| city.value(13));
        assert!(result.is_err());
    }

    #[test]
    fn test_csv_with_projection() {
        let schema = Schema::new(vec![
            Field::new("city", DataType::Utf8, true),
            Field::new("lat", DataType::Float64, true),
            Field::new("lng", DataType::Float64, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);

        let file = File::open("test/data/uk_cities.csv").unwrap();

        let mut csv = ReaderBuilder::new(agate_schema)
            .with_projection(vec![0, 1])
            .build(file)
            .unwrap();

        let projected_schema = Arc::new(Schema::new(vec![
            Field::new("city", DataType::Utf8, true),
            Field::new("lat", DataType::Float64, true),
        ]));
        assert_eq!(projected_schema, csv.schema());
        let batch = csv.next().unwrap().unwrap();
        assert_eq!(projected_schema, batch.schema());
        assert_eq!(37, batch.num_rows());
        assert_eq!(2, batch.num_columns());
    }

    fn invalid_csv_helper(file_name: &str) -> String {
        let file = File::open(file_name).unwrap();
        let schema = Schema::new(vec![
            Field::new("c_int", DataType::UInt64, false),
            Field::new("c_float", DataType::Float32, false),
            Field::new("c_string", DataType::Utf8, false),
            Field::new("c_bool", DataType::Boolean, false),
        ]);

        let builder =
            ReaderBuilder::new(crate::type_tester::AgateSchema::from_arrow_schema(&schema))
                .with_header(true)
                .with_delimiter(b'|')
                .with_batch_size(512)
                .with_projection(vec![0, 1, 2, 3]);

        let mut csv = builder.build(file).unwrap();

        csv.next().unwrap().unwrap_err().to_string()
    }

    #[test]
    #[ignore] // Need investigation on test failure
    fn test_parse_invalid_csv_float() {
        let file_name = "test/data/various_invalid_types/invalid_float.csv";

        let error = invalid_csv_helper(file_name);
        assert_eq!(
            "Parser error: Error while parsing value '4.x4' as type 'Float32' for column 1 at line 4. Row data: '[4,4.x4,,false]'",
            error
        );
    }

    #[test]
    #[ignore] // Need investigation on test failure
    fn test_parse_invalid_csv_int() {
        let file_name = "test/data/various_invalid_types/invalid_int.csv";

        let error = invalid_csv_helper(file_name);
        assert_eq!(
            "Parser error: Error while parsing value '2.3' as type 'UInt64' for column 0 at line 2. Row data: '[2.3,2.2,2.22,false]'",
            error
        );
    }

    #[test]
    #[ignore] // Need investigation on test failure
    fn test_parse_invalid_csv_bool() {
        let file_name = "test/data/various_invalid_types/invalid_bool.csv";

        let error = invalid_csv_helper(file_name);
        assert_eq!(
            "Parser error: Error while parsing value 'none' as type 'Boolean' for column 3 at line 2. Row data: '[2,2.2,2.22,none]'",
            error
        );
    }

    #[test]
    fn test_bounded() {
        // With agate: UInt32 becomes Int64
        let schema = Schema::new(vec![Field::new("int", DataType::Int64, true)]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);
        let data = [
            vec!["0"],
            vec!["1"],
            vec!["2"],
            vec!["3"],
            vec!["4"],
            vec!["5"],
            vec!["6"],
        ];

        let data = data
            .iter()
            .map(|x| x.join(","))
            .collect::<Vec<_>>()
            .join("\n");
        let data = data.as_bytes();

        let reader = Cursor::new(data);

        let mut csv = ReaderBuilder::new(agate_schema)
            .with_batch_size(2)
            .with_projection(vec![0])
            .with_bounds(2, 6)
            .build_buffered(reader)
            .unwrap();

        let batch = csv.next().unwrap().unwrap();
        let a = batch.column(0);
        let a = a.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(a, &Int64Array::from(vec![2, 3]));

        let batch = csv.next().unwrap().unwrap();
        let a = batch.column(0);
        let a = a.as_any().downcast_ref::<Int64Array>().unwrap();
        assert_eq!(a, &Int64Array::from(vec![4, 5]));

        assert!(csv.next().is_none());
    }

    #[test]
    fn test_empty_projection() {
        // With agate: UInt32 becomes Int64
        let schema = Schema::new(vec![Field::new("int", DataType::Int64, true)]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);
        let data = [vec!["0"], vec!["1"]];

        let data = data
            .iter()
            .map(|x| x.join(","))
            .collect::<Vec<_>>()
            .join("\n");

        let mut csv = ReaderBuilder::new(agate_schema)
            .with_batch_size(2)
            .with_projection(vec![])
            .build_buffered(Cursor::new(data.as_bytes()))
            .unwrap();

        let batch = csv.next().unwrap().unwrap();
        assert_eq!(batch.columns().len(), 0);
        assert_eq!(batch.num_rows(), 2);

        assert!(csv.next().is_none());
    }

    #[test]
    fn test_non_std_quote() {
        let schema = Schema::new(vec![
            Field::new("text1", DataType::Utf8, true),
            Field::new("text2", DataType::Utf8, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);
        let builder = ReaderBuilder::new(agate_schema)
            .with_header(false)
            .with_quote(b'~'); // default is ", change to ~

        let mut csv_text = Vec::new();
        let mut csv_writer = Cursor::new(&mut csv_text);
        for index in 0..10 {
            let text1 = format!("id{index:}");
            let text2 = format!("value{index:}");
            csv_writer
                .write_fmt(format_args!("~{text1}~,~{text2}~\r\n"))
                .unwrap();
        }
        let mut csv_reader = Cursor::new(&csv_text);
        let mut reader = builder.build(&mut csv_reader).unwrap();
        let batch = reader.next().unwrap().unwrap();
        let col0 = batch.column(0);
        assert_eq!(col0.len(), 10);
        let col0_arr = col0.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(col0_arr.value(0), "id0");
        let col1 = batch.column(1);
        assert_eq!(col1.len(), 10);
        let col1_arr = col1.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(col1_arr.value(5), "value5");
    }

    #[test]
    fn test_non_std_escape() {
        let schema = Schema::new(vec![
            Field::new("text1", DataType::Utf8, true),
            Field::new("text2", DataType::Utf8, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);
        let builder = ReaderBuilder::new(agate_schema)
            .with_header(false)
            .with_escape(b'\\'); // default is None, change to \

        let mut csv_text = Vec::new();
        let mut csv_writer = Cursor::new(&mut csv_text);
        for index in 0..10 {
            let text1 = format!("id{index:}");
            let text2 = format!("value\\\"{index:}");
            csv_writer
                .write_fmt(format_args!("\"{text1}\",\"{text2}\"\r\n"))
                .unwrap();
        }
        let mut csv_reader = Cursor::new(&csv_text);
        let mut reader = builder.build(&mut csv_reader).unwrap();
        let batch = reader.next().unwrap().unwrap();
        let col0 = batch.column(0);
        assert_eq!(col0.len(), 10);
        let col0_arr = col0.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(col0_arr.value(0), "id0");
        let col1 = batch.column(1);
        assert_eq!(col1.len(), 10);
        let col1_arr = col1.as_any().downcast_ref::<StringArray>().unwrap();
        assert_eq!(col1_arr.value(5), "value\"5");
    }

    #[test]
    fn test_header_bounds() {
        let csv = "a,b\na,b\na,b\na,b\na,b\n";
        let tests = [
            (None, false, 5),
            (None, true, 4),
            (Some((0, 4)), false, 4),
            (Some((1, 4)), false, 3),
            (Some((0, 4)), true, 4),
            (Some((1, 4)), true, 3),
        ];
        let schema = Schema::new(vec![
            Field::new("a", DataType::Utf8, true),
            Field::new("b", DataType::Utf8, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);

        for (idx, (bounds, has_header, expected)) in tests.into_iter().enumerate() {
            let mut reader = ReaderBuilder::new(agate_schema.clone()).with_header(has_header);
            if let Some((start, end)) = bounds {
                reader = reader.with_bounds(start, end);
            }
            let b = reader
                .build_buffered(Cursor::new(csv.as_bytes()))
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            assert_eq!(b.num_rows(), expected, "{idx}");
        }
    }

    #[test]
    fn test_null_boolean() {
        let csv = "true,false\nFalse,True\n,True\nFalse,";
        let schema = Schema::new(vec![
            Field::new("a", DataType::Boolean, true),
            Field::new("b", DataType::Boolean, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);

        let b = ReaderBuilder::new(agate_schema)
            .build_buffered(Cursor::new(csv.as_bytes()))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert_eq!(b.num_rows(), 4);
        assert_eq!(b.num_columns(), 2);

        let c = b.column(0).as_boolean();
        assert_eq!(c.null_count(), 1);
        assert!(c.value(0));
        assert!(!c.value(1));
        assert!(c.is_null(2));
        assert!(!c.value(3));

        let c = b.column(1).as_boolean();
        assert_eq!(c.null_count(), 1);
        assert!(!c.value(0));
        assert!(c.value(1));
        assert!(c.value(2));
        assert!(c.is_null(3));
    }

    #[test]
    fn test_truncated_rows() {
        // With agate: Int32 becomes Int64
        let data = "a,b,c\n1,2,3\n4,5\n\n6,7,8";
        let schema = Schema::new(vec![
            Field::new("a", DataType::Int64, true),
            Field::new("b", DataType::Int64, true),
            Field::new("c", DataType::Int64, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);

        let reader = ReaderBuilder::new(agate_schema.clone())
            .with_header(true)
            .with_truncated_rows(true)
            .build(Cursor::new(data))
            .unwrap();

        let batches = reader.collect::<Result<Vec<_>, _>>();
        assert!(batches.is_ok());
        let batch = batches.unwrap().into_iter().next().unwrap();
        // Empty rows are preserved (agate-compatible behavior)
        // Data: 1,2,3 | 4,5 (truncated) | <empty> | 6,7,8 = 4 rows
        assert_eq!(batch.num_rows(), 4);

        let reader = ReaderBuilder::new(agate_schema)
            .with_header(true)
            .with_truncated_rows(false)
            .build(Cursor::new(data))
            .unwrap();

        let batches = reader.collect::<Result<Vec<_>, _>>();
        assert!(match batches {
            Err(ArrowError::CsvError(e)) => e.contains("incorrect number of fields"),
            _ => false,
        });
    }

    #[test]
    fn test_truncated_rows_csv() {
        // With agate: UInt32 becomes Int64, Date32 stays Date32
        let file = File::open("test/data/truncated_rows.csv").unwrap();
        let schema = Schema::new(vec![
            Field::new("Name", DataType::Utf8, true),
            Field::new("Age", DataType::Int64, true),
            Field::new("Occupation", DataType::Utf8, true),
            Field::new("DOB", DataType::Date32, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);
        let reader = ReaderBuilder::new(agate_schema)
            .with_header(true)
            .with_batch_size(24)
            .with_truncated_rows(true);
        let csv = reader.build(file).unwrap();
        let batches = csv.collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(batch.num_rows(), 7);
        assert_eq!(batch.num_columns(), 4);
        let name = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let age = batch
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        let occupation = batch
            .column(2)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        let dob = batch
            .column(3)
            .as_any()
            .downcast_ref::<Date32Array>()
            .unwrap();

        assert_eq!(name.value(0), "A1");
        assert_eq!(name.value(1), "B2");
        assert!(name.is_null(2));
        assert_eq!(name.value(3), "C3");
        assert!(name.is_null(4));
        assert_eq!(name.value(5), "D4");
        assert_eq!(name.value(6), "E5");

        assert_eq!(age.value(0), 34);
        assert_eq!(age.value(1), 29);
        assert!(age.is_null(2));
        assert_eq!(age.value(3), 45);
        assert!(age.is_null(4));
        assert!(age.is_null(5));
        assert_eq!(age.value(6), 31);

        assert_eq!(occupation.value(0), "Engineer");
        assert_eq!(occupation.value(1), "Doctor");
        assert!(occupation.is_null(2));
        assert_eq!(occupation.value(3), "Artist");
        assert!(occupation.is_null(4));
        assert!(occupation.is_null(5));
        assert!(occupation.is_null(6));

        assert_eq!(dob.value(0), 5675);
        assert!(dob.is_null(1));
        assert!(dob.is_null(2));
        assert_eq!(dob.value(3), -1858);
        assert!(dob.is_null(4));
        assert!(dob.is_null(5));
        assert!(dob.is_null(6));
    }

    #[test]
    #[ignore] // TODO: Need investigation on test failure
    fn test_truncated_rows_not_nullable_error() {
        // With agate: Int32 becomes Int64
        let data = "a,b,c\n1,2,3\n4,5";
        let schema = Schema::new(vec![
            Field::new("a", DataType::Int64, false),
            Field::new("b", DataType::Int64, false),
            Field::new("c", DataType::Int64, false),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);

        let reader = ReaderBuilder::new(agate_schema)
            .with_header(true)
            .with_truncated_rows(true)
            .build(Cursor::new(data))
            .unwrap();

        let batches = reader.collect::<Result<Vec<_>, _>>();
        assert!(match batches {
            Err(ArrowError::InvalidArgumentError(e)) => e.contains("contains null values"),
            _ => false,
        });
    }

    #[test]
    fn test_buffered() {
        let tests = [
            ("test/data/uk_cities.csv", false, 37),
            ("test/data/various_types.csv", true, 10),
            ("test/data/decimal_test.csv", false, 10),
        ];

        for (path, has_header, expected_rows) in tests {
            // Use infer_agate_schema for agate-style parsing
            let (agate_schema, _) = Format::default()
                .infer_agate_schema(File::open(path).unwrap(), None)
                .unwrap();

            for batch_size in [1, 4] {
                for capacity in [1, 3, 7, 100] {
                    let reader = ReaderBuilder::new(agate_schema.clone())
                        .with_batch_size(batch_size)
                        .with_header(has_header)
                        .build(File::open(path).unwrap())
                        .unwrap();

                    let expected = reader.collect::<Result<Vec<_>, _>>().unwrap();

                    assert_eq!(
                        expected.iter().map(|x| x.num_rows()).sum::<usize>(),
                        expected_rows
                    );

                    let buffered =
                        std::io::BufReader::with_capacity(capacity, File::open(path).unwrap());

                    let reader = ReaderBuilder::new(agate_schema.clone())
                        .with_batch_size(batch_size)
                        .with_header(has_header)
                        .build_buffered(buffered)
                        .unwrap();

                    let actual = reader.collect::<Result<Vec<_>, _>>().unwrap();
                    assert_eq!(expected, actual)
                }
            }
        }
    }

    fn err_test(csv: &[u8], expected: &str) {
        fn err_test_with_schema(csv: &[u8], expected: &str, schema: Arc<Schema>) {
            let buffer = std::io::BufReader::with_capacity(2, Cursor::new(csv));
            let b = ReaderBuilder::new(crate::type_tester::AgateSchema::from_arrow_schema(&schema))
                .with_batch_size(2)
                .build_buffered(buffer)
                .unwrap();
            let err = b.collect::<Result<Vec<_>, _>>().unwrap_err().to_string();
            assert_eq!(err, expected)
        }

        let schema_utf8 = Arc::new(Schema::new(vec![
            Field::new("text1", DataType::Utf8, true),
            Field::new("text2", DataType::Utf8, true),
        ]));
        err_test_with_schema(csv, expected, schema_utf8);

        let schema_utf8view = Arc::new(Schema::new(vec![
            Field::new("text1", DataType::Utf8View, true),
            Field::new("text2", DataType::Utf8View, true),
        ]));
        err_test_with_schema(csv, expected, schema_utf8view);
    }

    #[test]
    fn test_invalid_utf8() {
        err_test(
            b"sdf,dsfg\ndfd,hgh\xFFue\n,sds\nFalhghse,",
            "Csv error: Encountered invalid UTF-8 data for line 2 and field 2",
        );

        err_test(
            b"sdf,dsfg\ndksdk,jf\nd\xFFfd,hghue\n,sds\nFalhghse,",
            "Csv error: Encountered invalid UTF-8 data for line 3 and field 1",
        );

        err_test(
            b"sdf,dsfg\ndksdk,jf\ndsdsfd,hghue\n,sds\nFalhghse,\xFF",
            "Csv error: Encountered invalid UTF-8 data for line 5 and field 2",
        );

        err_test(
            b"\xFFsdf,dsfg\ndksdk,jf\ndsdsfd,hghue\n,sds\nFalhghse,\xFF",
            "Csv error: Encountered invalid UTF-8 data for line 1 and field 1",
        );
    }

    struct InstrumentedRead<R> {
        r: R,
        fill_count: usize,
        fill_sizes: Vec<usize>,
    }

    impl<R> InstrumentedRead<R> {
        fn new(r: R) -> Self {
            Self {
                r,
                fill_count: 0,
                fill_sizes: vec![],
            }
        }
    }

    impl<R: Seek> Seek for InstrumentedRead<R> {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.r.seek(pos)
        }
    }

    impl<R: BufRead> Read for InstrumentedRead<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.r.read(buf)
        }
    }

    impl<R: BufRead> BufRead for InstrumentedRead<R> {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            self.fill_count += 1;
            let buf = self.r.fill_buf()?;
            self.fill_sizes.push(buf.len());
            Ok(buf)
        }

        fn consume(&mut self, amt: usize) {
            self.r.consume(amt)
        }
    }

    #[test]
    fn test_io() {
        let schema = Schema::new(vec![
            Field::new("a", DataType::Utf8, true),
            Field::new("b", DataType::Utf8, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);
        let csv = "foo,bar\nbaz,foo\na,b\nc,d";
        let mut read = InstrumentedRead::new(Cursor::new(csv.as_bytes()));
        let reader = ReaderBuilder::new(agate_schema)
            .with_batch_size(3)
            .build_buffered(&mut read)
            .unwrap();

        let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].num_rows(), 3);
        assert_eq!(batches[1].num_rows(), 1);

        // Expect 4 calls to fill_buf
        // 1. Read first 3 rows
        // 2. Read final row
        // 3. Delimit and flush final row
        // 4. Iterator finished
        assert_eq!(&read.fill_sizes, &[23, 3, 0, 0]);
        assert_eq!(read.fill_count, 4);
    }

    #[test]
    fn test_record_length_mismatch_allowed_by_default() {
        // With Python-compatible settings, truncated rows are allowed by default
        let csv = "\
        a,b,c\n\
        1,2,3\n\
        4,5\n\
        6,7,8";
        let mut read = Cursor::new(csv.as_bytes());
        let result = Format::new(b',', true, true).infer_agate_schema(&mut read, None);
        // Should succeed now (truncated rows allowed by default)
        assert!(result.is_ok());
    }

    #[test]
    fn test_record_length_mismatch_error_when_disabled() {
        // When truncated_rows is disabled, should error
        let csv = "\
        a,b,c\n\
        1,2,3\n\
        4,5\n\
        6,7,8";
        let mut read = Cursor::new(csv.as_bytes());
        let result = Format::new(b',', true, true)
            .with_truncated_rows(false)
            .infer_agate_schema(&mut read, None);
        assert!(result.is_err());
        // Include line number in the error message to help locate and fix the issue
        assert_eq!(
            result.err().unwrap().to_string(),
            "Csv error: Encountered unequal lengths between records on CSV file. Expected 3 records, found 2 records at line 3"
        );
    }

    #[test]
    fn test_comment() {
        // With agate: Int8 becomes Int64
        let schema = Schema::new(vec![
            Field::new("a", DataType::Int64, true),
            Field::new("b", DataType::Int64, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);

        let csv = "# comment1 \n1,2\n#comment2\n11,22";
        let mut read = Cursor::new(csv.as_bytes());
        let reader = ReaderBuilder::new(agate_schema)
            .with_comment(b'#')
            .build(&mut read)
            .unwrap();

        let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(batches.len(), 1);
        let b = batches.first().unwrap();
        assert_eq!(b.num_columns(), 2);
        assert_eq!(
            b.column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values(),
            &vec![1i64, 11i64]
        );
        assert_eq!(
            b.column(1)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values(),
            &vec![2i64, 22i64]
        );
    }

    #[test]
    fn test_parse_string_view_single_column() {
        // Note: AgateSchema converts Utf8View to Text which becomes Utf8 (not Utf8View)
        let csv = ["foo", "something_cannot_be_inlined", "foobar"].join("\n");
        let schema = Schema::new(vec![Field::new("c1", DataType::Utf8, true)]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);

        let mut decoder = ReaderBuilder::new(agate_schema).build_decoder();

        let decoded = decoder.decode(csv.as_bytes()).unwrap();
        assert_eq!(decoded, csv.len());
        decoder.decode(&[]).unwrap();

        let batch = decoder.flush().unwrap().unwrap();
        assert_eq!(batch.num_columns(), 1);
        assert_eq!(batch.num_rows(), 3);
        let col = batch.column(0).as_string::<i32>();
        assert_eq!(col.data_type(), &DataType::Utf8);
        assert_eq!(col.value(0), "foo");
        assert_eq!(col.value(1), "something_cannot_be_inlined");
        assert_eq!(col.value(2), "foobar");
    }

    #[test]
    fn test_parse_string_view_multi_column() {
        // Note: AgateSchema converts Utf8View to Text which becomes Utf8 (not Utf8View)
        let csv = ["foo,", ",something_cannot_be_inlined", "foobarfoobar,bar"].join("\n");
        let schema = Schema::new(vec![
            Field::new("c1", DataType::Utf8, true),
            Field::new("c2", DataType::Utf8, true),
        ]);
        let agate_schema = crate::type_tester::AgateSchema::from_arrow_schema(&schema);

        let mut decoder = ReaderBuilder::new(agate_schema).build_decoder();

        let decoded = decoder.decode(csv.as_bytes()).unwrap();
        assert_eq!(decoded, csv.len());
        decoder.decode(&[]).unwrap();

        let batch = decoder.flush().unwrap().unwrap();
        assert_eq!(batch.num_columns(), 2);
        assert_eq!(batch.num_rows(), 3);
        let c1 = batch.column(0).as_string::<i32>();
        let c2 = batch.column(1).as_string::<i32>();
        assert_eq!(c1.data_type(), &DataType::Utf8);
        assert_eq!(c2.data_type(), &DataType::Utf8);

        assert!(!c1.is_null(0));
        assert!(c1.is_null(1));
        assert!(!c1.is_null(2));
        assert_eq!(c1.value(0), "foo");
        assert_eq!(c1.value(2), "foobarfoobar");

        assert!(c2.is_null(0));
        assert!(!c2.is_null(1));
        assert!(!c2.is_null(2));
        assert_eq!(c2.value(1), "something_cannot_be_inlined");
        assert_eq!(c2.value(2), "bar");
    }

    #[test]
    fn test_infer_agate_schema_with_text_columns() {
        use crate::type_tester::{AgateColumn, AgateSchema, AgateType};

        let mut file = File::open("test/data/text_columns_test.csv").unwrap();

        // Specify date_created and original_network_name as forced text columns
        let text_columns: Vec<String> = vec!["date_created", "original_network_name"]
            .into_iter()
            .map(String::from)
            .collect();
        let (agate_schema, records_count, missing_columns) = Format::default()
            .with_header(true)
            .infer_agate_schema_with_text_columns(&mut file, None, &text_columns)
            .unwrap();

        // Should have read 1 data row
        assert_eq!(records_count, 1);

        // No missing columns - all text_columns matched
        assert!(
            missing_columns.is_empty(),
            "Expected no missing columns, got: {:?}",
            missing_columns
        );

        // Expected schema:
        // - date_created: forced to Text (would fail date inference due to "4/30/2021" format)
        // - original_network_name: forced to Text
        // - mapped_network_name: inferred as Text (contains "Ad Extensions")
        // - is_paid_network: inferred as Boolean (contains "TRUE")
        let expected_schema = AgateSchema::new(vec![
            AgateColumn::new("date_created", AgateType::Text),
            AgateColumn::new("original_network_name", AgateType::Text),
            AgateColumn::new("mapped_network_name", AgateType::Text),
            AgateColumn::new("is_paid_network", AgateType::Boolean),
        ]);

        assert_eq!(agate_schema, expected_schema);
    }

    #[test]
    fn test_infer_agate_schema_with_text_columns_case_insensitive() {
        use crate::type_tester::{AgateColumn, AgateSchema, AgateType};

        let mut file = File::open("test/data/text_columns_test.csv").unwrap();

        // Specify uppercased column names (simulates Snowflake normalization)
        // Case-insensitive matching should still find them
        let text_columns: Vec<String> = vec![
            "DATE_CREATED",          // uppercase (matches date_created)
            "ORIGINAL_NETWORK_NAME", // uppercase (matches original_network_name)
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let (agate_schema, records_count, missing_columns) = Format::default()
            .with_header(true)
            .infer_agate_schema_with_text_columns(&mut file, None, &text_columns)
            .unwrap();

        // Should have read 1 data row
        assert_eq!(records_count, 1);

        // No missing columns - case-insensitive matching finds them
        assert!(
            missing_columns.is_empty(),
            "Expected no missing columns, got: {:?}",
            missing_columns
        );

        // Expected schema: both specified columns forced to Text
        let expected_schema = AgateSchema::new(vec![
            AgateColumn::new("date_created", AgateType::Text),
            AgateColumn::new("original_network_name", AgateType::Text),
            AgateColumn::new("mapped_network_name", AgateType::Text),
            AgateColumn::new("is_paid_network", AgateType::Boolean),
        ]);

        assert_eq!(agate_schema, expected_schema);
    }

    #[test]
    fn test_infer_agate_schema_with_text_columns_missing() {
        use crate::type_tester::{AgateColumn, AgateSchema, AgateType};

        let mut file = File::open("test/data/text_columns_test.csv").unwrap();

        // Specify misspelled column names (not just wrong case)
        let text_columns: Vec<String> = vec![
            "ORIGINAL_NETWORK",   // misspelled (actual: original_network_name)
            "nonexistent_column", // doesn't exist at all
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let (agate_schema, records_count, missing_columns) = Format::default()
            .with_header(true)
            .infer_agate_schema_with_text_columns(&mut file, None, &text_columns)
            .unwrap();

        // Should have read 1 data row
        assert_eq!(records_count, 1);

        // Both should be missing (misspelled, not just wrong case)
        assert_eq!(
            missing_columns.len(),
            2,
            "Expected 2 missing columns, got: {:?}",
            missing_columns
        );
        assert!(missing_columns.contains(&"ORIGINAL_NETWORK".to_string()));
        assert!(missing_columns.contains(&"nonexistent_column".to_string()));

        // Expected schema: all columns inferred normally (no forced text)
        let expected_schema = AgateSchema::new(vec![
            AgateColumn::new("date_created", AgateType::Date),
            AgateColumn::new("original_network_name", AgateType::Text),
            AgateColumn::new("mapped_network_name", AgateType::Text),
            AgateColumn::new("is_paid_network", AgateType::Boolean),
        ]);

        assert_eq!(agate_schema, expected_schema);
    }

    #[test]
    fn test_csv_with_duplicate_column_names() {
        // CSV with duplicate column name "col_1"
        let csv = "col_1,col_2,col_1\n1,text,3";

        // Use Format with disambiguate_header=true to infer schema with deduplication
        let format = Format::new(b',', true, true);
        let (agate_schema, _) = format
            .infer_agate_schema(Cursor::new(csv.as_bytes()), None)
            .unwrap();

        // Assert that column names were deduplicated (col_1, col_2, col_1_2)
        assert_eq!(agate_schema.column(0).name, "col_1");
        assert_eq!(agate_schema.column(1).name, "col_2");
        assert_eq!(agate_schema.column(2).name, "col_1_2");

        // Read the data using the inferred schema
        let batch = ReaderBuilder::new(agate_schema)
            .with_header(true)
            .build_buffered(Cursor::new(csv.as_bytes()))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        // Assert on row/column counts
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.num_columns(), 3);

        // Assert on schema/datatypes with deduplicated names
        let result_schema = batch.schema();
        assert_eq!(result_schema.field(0).name(), "col_1");
        assert_eq!(result_schema.field(0).data_type(), &DataType::Int64);
        assert_eq!(result_schema.field(1).name(), "col_2");
        assert_eq!(result_schema.field(1).data_type(), &DataType::Utf8);
        assert_eq!(result_schema.field(2).name(), "col_1_2");
        assert_eq!(result_schema.field(2).data_type(), &DataType::Int64);

        // Assert on values
        let col1_first = batch.column(0).as_primitive::<Int64Type>();
        assert_eq!(col1_first.value(0), 1);

        let col2 = batch.column(1).as_string::<i32>();
        assert_eq!(col2.value(0), "text");

        let col1_second = batch.column(2).as_primitive::<Int64Type>();
        assert_eq!(col1_second.value(0), 3);
    }

    #[test]
    fn test_csv_with_empty_column_names() {
        // CSV with trailing comma creates empty column name: "col_1," -> ["col_1", ""]
        // Matches agate behavior: empty names are replaced with letter_name(i)
        let csv = "col_1,\nhey,\nhi,\nhow\nare,\n";

        let format = Format::new(b',', true, true);
        let (agate_schema, _) = format
            .infer_agate_schema(Cursor::new(csv.as_bytes()), None)
            .unwrap();

        assert_eq!(agate_schema.column(0).name, "col_1");
        assert_eq!(agate_schema.column(1).name, "b"); // was empty "", now "b"

        let batch = ReaderBuilder::new(agate_schema)
            .with_header(true)
            .build_buffered(Cursor::new(csv.as_bytes()))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert_eq!(batch.num_rows(), 4);
        assert_eq!(batch.num_columns(), 2);
    }

    #[test]
    fn test_csv_with_whitespace_only_column_name() {
        // CSV with whitespace-only column name: "col_1,   " -> ["col_1", "   "]
        // Whitespace-only names should also be replaced with letter_name(i)
        let csv = "col_1,   \n1,2\n";

        let format = Format::new(b',', true, true);
        let (agate_schema, _) = format
            .infer_agate_schema(Cursor::new(csv.as_bytes()), None)
            .unwrap();

        assert_eq!(agate_schema.column(0).name, "col_1");
        assert_eq!(agate_schema.column(1).name, "b");
    }

    #[test]
    fn test_whitespace_only_value_is_null_for_integer() {
        use crate::type_tester::{AgateColumn, AgateSchema, AgateType};

        // Test that whitespace-only values are treated as NULL for integer columns
        let csv = "int_col,other_col\n1,\n2, \n3,\n";

        // Create schema with integer column and text column
        let agate_schema = AgateSchema::new(vec![
            AgateColumn::new("int_col", AgateType::Integer),
            AgateColumn::new("other_col", AgateType::Text),
        ]);

        // Read the data
        let batch = ReaderBuilder::new(agate_schema)
            .with_header(true)
            .build_buffered(Cursor::new(csv.as_bytes()))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert_eq!(batch.num_rows(), 3);

        // All rows in other_col should be NULL (empty or whitespace)
        let other_col = batch.column(1);
        assert!(other_col.is_null(0), "Empty value '' should be NULL");
        assert!(
            other_col.is_null(1),
            "Whitespace-only value ' ' should be NULL"
        );
        assert!(other_col.is_null(2), "Empty value '' should be NULL");

        // int_col should have actual integer values
        let int_col = batch.column(0).as_primitive::<Int64Type>();
        assert_eq!(int_col.value(0), 1);
        assert_eq!(int_col.value(1), 2);
        assert_eq!(int_col.value(2), 3);
    }

    #[test]
    fn test_letter_name() {
        // Index 0 -> "a" (first letter)
        assert_eq!(Format::letter_name(0), "a");

        // Index 25 -> "z" (last letter, 25 % 26 = 25, repeat = 1)
        assert_eq!(Format::letter_name(25), "z");

        // Index 26 -> "aa" (wraps to first letter, 26 % 26 = 0 -> 'a', repeat = 2)
        assert_eq!(Format::letter_name(26), "aa");

        // Index 51 -> "zz" (wraps to last letter, 51 % 26 = 25 -> 'z', repeat = 2)
        assert_eq!(Format::letter_name(51), "zz");
    }

    #[test]
    fn test_datetime_microsecond_precision() {
        use crate::type_tester::{AgateColumn, AgateSchema, AgateType};

        // DateTime type (no fractional seconds) should produce microsecond timestamps
        let csv = "ts\n2023-06-21 11:30:00\n2023-06-21 11:30:45\n";

        let agate_schema = AgateSchema::new(vec![AgateColumn::new("ts", AgateType::DateTime)]);

        let batch = ReaderBuilder::new(agate_schema)
            .with_header(true)
            .build_buffered(Cursor::new(csv.as_bytes()))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert_eq!(batch.num_rows(), 2);

        // Verify schema is Timestamp(Microsecond, None)
        let schema = batch.schema();
        let field = schema.field(0);
        assert_eq!(
            field.data_type(),
            &DataType::Timestamp(TimeUnit::Microsecond, None)
        );

        // Verify values (microseconds since epoch)
        let ts_col = batch
            .column(0)
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();

        // 2023-06-21 11:30:00 UTC = 1687347000 seconds = 1687347000000000 microseconds
        assert_eq!(ts_col.value(0), 1_687_347_000_000_000);
        // 2023-06-21 11:30:45 UTC = 1687347045 seconds = 1687347045000000 microseconds
        assert_eq!(ts_col.value(1), 1_687_347_045_000_000);
    }

    #[test]
    fn test_iso_datetime_microsecond_preservation() {
        use crate::type_tester::{AgateColumn, AgateSchema, AgateType};

        // ISODateTime with 6-digit fractional seconds should preserve microseconds exactly
        let csv = "ts\n2023-06-21T11:30:00.123456\n2023-06-21T11:30:00.999999\n2023-06-21T11:30:00.000001\n";

        let agate_schema = AgateSchema::new(vec![AgateColumn::new("ts", AgateType::ISODateTime)]);

        let batch = ReaderBuilder::new(agate_schema)
            .with_header(true)
            .build_buffered(Cursor::new(csv.as_bytes()))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert_eq!(batch.num_rows(), 3);

        // Verify schema is Timestamp(Microsecond, None)
        let schema = batch.schema();
        let field = schema.field(0);
        assert_eq!(
            field.data_type(),
            &DataType::Timestamp(TimeUnit::Microsecond, None)
        );

        let ts_col = batch
            .column(0)
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();

        // Base time: 2023-06-21 11:30:00 UTC = 1687347000 seconds
        let base_micros: i64 = 1_687_347_000_000_000;

        // .123456 -> 123456 microseconds added
        assert_eq!(ts_col.value(0), base_micros + 123456);
        // .999999 -> 999999 microseconds added
        assert_eq!(ts_col.value(1), base_micros + 999999);
        // .000001 -> 1 microsecond added
        assert_eq!(ts_col.value(2), base_micros + 1);
    }

    #[test]
    fn test_iso_datetime_nanosecond_truncation() {
        use crate::type_tester::{AgateColumn, AgateSchema, AgateType};

        // ISODateTime with 9-digit fractional seconds should truncate to microseconds
        // This matches Python agate behavior where datetime.microsecond truncates nanoseconds
        let csv = "ts\n2023-06-21T11:30:00.123456789\n2023-06-21T11:30:00.999999999\n2023-06-21T11:30:00.000000001\n";

        let agate_schema = AgateSchema::new(vec![AgateColumn::new("ts", AgateType::ISODateTime)]);

        let batch = ReaderBuilder::new(agate_schema)
            .with_header(true)
            .build_buffered(Cursor::new(csv.as_bytes()))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert_eq!(batch.num_rows(), 3);

        // Verify schema is Timestamp(Microsecond, None)
        let schema = batch.schema();
        let field = schema.field(0);
        assert_eq!(
            field.data_type(),
            &DataType::Timestamp(TimeUnit::Microsecond, None)
        );

        let ts_col = batch
            .column(0)
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap();

        // Base time: 2023-06-21 11:30:00 UTC = 1687347000 seconds
        let base_micros: i64 = 1_687_347_000_000_000;

        // .123456789 truncates to .123456 -> 123456 microseconds (last 3 digits lost)
        // Python agate: 2023-06-21T11:30:00.123456789 -> microsecond=123456
        assert_eq!(ts_col.value(0), base_micros + 123456);

        // .999999999 truncates to .999999 -> 999999 microseconds (last 3 digits lost)
        // Python agate: 2023-06-21T11:30:00.999999999 -> microsecond=999999
        assert_eq!(ts_col.value(1), base_micros + 999999);

        // .000000001 truncates to .000000 -> 0 microseconds (nanosecond completely lost)
        // Python agate: 2023-06-21T11:30:00.000000001 -> microsecond=0
        assert_eq!(ts_col.value(2), base_micros);
    }
}
