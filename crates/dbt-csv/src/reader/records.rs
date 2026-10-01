// RecordDecoder: forked from arrow-csv

use arrow_schema::ArrowError;
use csv_core::{ReadRecordResult, Reader};

/// The estimated length of a field in bytes
const AVERAGE_FIELD_SIZE: usize = 8;

/// The minimum amount of data in a single read
const MIN_CAPACITY: usize = 1024;

/// State machine for tracking empty records (consecutive terminators)
///
/// csv_core skips empty lines by default. This state machine tracks
/// terminators to correctly count and emit empty records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminatorState {
    /// Normal state - at start of record or parsing fields
    #[default]
    AtStart,
    /// A record just ended with `\r`, waiting to see if `\n` follows
    /// The `\n` would complete the record's CRLF terminator (not an empty line)
    RecordEndedWithCR,
    /// Saw `\r` at the start of a record (potential empty line)
    /// Waiting to see if `\n` follows (\r\n = 1 empty) or not (\r alone = 1 empty)
    PendingEmptyCR,
}

/// [`RecordDecoder`] provides a push-based interface to decoder [`StringRecords`]
#[derive(Debug)]
pub struct RecordDecoder {
    delimiter: Reader,

    /// The expected number of fields per row
    num_columns: usize,

    /// The current line number
    line_number: usize,

    /// Offsets delimiting field start positions
    offsets: Vec<usize>,

    /// The current offset into `self.offsets`
    ///
    /// We track this independently of Vec to avoid re-zeroing memory
    offsets_len: usize,

    /// The number of fields read for the current record
    /// At 0, it can either be csv_core has not processed any field
    /// OR it is currently processing the first field, no delimiter found yet
    current_field: usize,

    /// The number of rows buffered
    num_rows: usize,

    /// Decoded field data
    data: Vec<u8>,

    /// Offsets into data
    ///
    /// We track this independently of Vec to avoid re-zeroing memory
    data_len: usize,

    /// Whether rows with less than expected columns are considered valid
    ///
    /// Default value is false
    /// When enabled fills in missing columns with null
    truncated_rows: bool,

    /// State machine for tracking terminators to emit empty records
    terminator_state: TerminatorState,

    /// Whether csv_core has partial field data waiting for a terminator.
    /// Set when read_record returns InputEmpty with bytes consumed but no fields completed.
    /// When true, the next decode() should NOT pre-check for empty lines because
    /// csv_core has buffered data and we are in the middle of processing a csv field.
    has_partial_field: bool,
}

impl RecordDecoder {
    pub fn new(delimiter: Reader, num_columns: usize, truncated_rows: bool) -> Self {
        Self {
            delimiter,
            num_columns,
            line_number: 1,
            offsets: vec![],
            offsets_len: 1, // The first offset is always 0
            current_field: 0,
            data_len: 0,
            data: vec![],
            num_rows: 0,
            truncated_rows,
            terminator_state: TerminatorState::default(),
            has_partial_field: false,
        }
    }

    /// Called when a record is emitted. Updates terminator state based on
    /// whether the record ended with \r (potential CRLF).
    fn on_record_emitted(&mut self, last_input_byte: Option<u8>) {
        if last_input_byte == Some(b'\r') {
            // Record ended with \r, might be followed by \n
            self.terminator_state = TerminatorState::RecordEndedWithCR;
        } else {
            self.terminator_state = TerminatorState::AtStart;
        }
    }

    /// Returns true if we are at a clean record boundary with no pending csv_core state.
    ///
    /// State matrix for (current_field, has_partial_field):
    ///   (0, false) = Clean record boundary, csv_core has no buffered data
    ///   (0, true)  = csv_core is parsing first field, no delimiter found yet
    ///   (>0, true) = Mid-record, some fields done, more expected
    ///   (>0, false) = Unlikely in practice (would require csv_core bug)
    ///
    /// Only returns true for (0, false) - truly at record boundary.
    fn at_record_start(&self) -> bool {
        debug_assert!(
            self.current_field == 0 || self.has_partial_field,
            "self.current_field={}, self.has_partial_field =false. This should not happen in the state machine",
            self.current_field
        );
        self.current_field == 0 && !self.has_partial_field
    }

    /// Insert an empty record (all fields are empty strings).
    fn insert_empty_record(&mut self) {
        // Ensure we have space for offsets
        debug_assert!(
            self.offsets_len + self.num_columns <= self.offsets.len(),
            "self.offsets does not have enough room to allocate empty records!"
        );

        // Offset denotes the end position of a field relative to the start of the row
        // For an empty record, all field offsets are 0 (relative to this record's data start)
        // which flush() later converts to absolute positions.

        // self.offsets work hand-in-hand with self.data (containing output data)
        // each element in self.offsets denote the end index in self.data of the
        // current csv field.
        // Example: csv data "a,b\n\nx,y\n"
        // After csv_core::Reader finished reading the first record,
        //       self.data = [a, b]
        //       self.offsets = [0, 1, 2]
        //       	0: a single sentinel for the entire file
        //       	1: field "a" ends at index 1
        //          2: field "b" ends at index 2
        // Remaining data buffer: \nx,y\n
        // We detect a new line and inject an empty record as follows:
        //       self.offsets = [0, 1, 2]
        //       for every column in the next row, append 0 (relative offset to start of row)
        //       self.offsets = [0, 1, 2, 0, 0]
        // csv_core::Reader  reads "x,y\n"
        // 		self.data = [a, b, x, y]
        // 		self.offsets = [0, 1, 2, 0, 0, 1, 2]
        for _ in 0..self.num_columns {
            self.offsets[self.offsets_len] = 0;
            self.offsets_len += 1;
        }

        self.num_rows += 1;
        self.line_number += 1;
    }

    /// Handle terminator state from previous decode() call (buffer boundary cases).
    /// Returns (input_offset, records_read) after processing any pending empty records.
    ///
    /// This handles two cases:
    /// - RecordEndedWithCR: Previous record ended with \r. If input starts with \n,
    ///   skip it (completing \r\n terminator). NOT an empty line.
    /// - PendingEmptyCR: Record buffer started with \r at a record boundary.
    ///   Emit the pending empty record and skip \n if present.
    fn complete_pending_terminator(&mut self, input: &[u8], to_read: usize) -> (usize, usize) {
        let mut input_offset = 0;
        let mut read = 0;

        match self.terminator_state {
            TerminatorState::RecordEndedWithCR => {
                // Previous record ended with \r, check if this buffer starts with \n
                // If so, skip it (completing \r\n terminator). NOT an empty line.
                if !input.is_empty() && input[0] == b'\n' {
                    input_offset += 1;
                }
                self.terminator_state = TerminatorState::AtStart;
            }
            TerminatorState::PendingEmptyCR => {
                // We are at the start of record and there is an extra \r.
                // Emit the empty record and skip \n if present
                self.terminator_state = TerminatorState::AtStart;
                if read < to_read {
                    self.insert_empty_record();
                    read += 1;
                }
                if !input.is_empty() && input[0] == b'\n' {
                    input_offset += 1;
                }
            }
            TerminatorState::AtStart => {}
        }

        (input_offset, read)
    }

    /// Insert empty records for terminator tokens at record boundaries.
    ///
    /// When at a clean record boundary (no partial field data), this consumes leading
    /// terminator tokens (\n, \r, \r\n) and inserts empty records for each.
    /// Also handles RecordEndedWithCR state where the previous record ended with \r
    /// and we need to check if the current buffer starts with \n (completing \r\n).
    ///
    /// Returns updated (input_offset, records_read).
    fn insert_empty_records_at_boundary(
        &mut self,
        input: &[u8],
        mut input_offset: usize,
        mut read: usize,
        to_read: usize,
    ) -> (usize, usize) {
        // Last record ended with \r. Current input begins with \n.
        // We find \r\n, skip \n in the current buffer and reset
        // TerminatorState to AtStart
        if self.terminator_state == TerminatorState::RecordEndedWithCR {
            if input_offset < input.len() && input[input_offset] == b'\n' {
                input_offset += 1;
            }
            self.terminator_state = TerminatorState::AtStart;
        }

        // If buffer starts with terminator tokens and we are at a clean record
        // boundary, consume these terminator tokens instead of passing them to
        // csv_core::Reader (which will silently consume them without giving us
        // empty records). Inject empty records appropriately.
        while read < to_read && self.at_record_start() && input_offset < input.len() {
            let remaining = &input[input_offset..];
            match remaining[0] {
                b'\n' => {
                    // \n = empty line
                    self.insert_empty_record();
                    read += 1;
                    input_offset += 1;
                }
                b'\r' => {
                    if remaining.len() >= 2 {
                        if remaining[1] == b'\n' {
                            // \r\n = empty line
                            self.insert_empty_record();
                            read += 1;
                            input_offset += 2;
                        } else {
                            // \r followed by non-\n = standalone \r = empty line
                            self.insert_empty_record();
                            read += 1;
                            input_offset += 1;
                        }
                    } else {
                        // Only \r at end of buffer - set state for next decode()
                        self.terminator_state = TerminatorState::PendingEmptyCR;
                        input_offset += 1;
                        // this will fall through to the caller which will stop
                        // consuming and return
                        break;
                    }
                }
                _ => break,
            }
        }

        (input_offset, read)
    }

    /// Decodes records from `input` returning the number of records and bytes read
    ///
    /// Note: this expects to be called with an empty `input` to signal EOF
    pub fn decode(&mut self, input: &[u8], to_read: usize) -> Result<(usize, usize), ArrowError> {
        if to_read == 0 {
            return Ok((0, 0));
        }

        // Reserve sufficient capacity in offsets (extra for potential empty records)
        self.offsets
            .resize(self.offsets_len + to_read * self.num_columns, 0);

        // Handle terminator state from previous decode() call (buffer boundary cases)
        let (mut input_offset, mut read) = self.complete_pending_terminator(input, to_read);

        loop {
            // Reserve necessary space in output data based on best estimate
            let remaining_rows = to_read - read;
            let capacity = remaining_rows * self.num_columns * AVERAGE_FIELD_SIZE;
            let estimated_data = capacity.max(MIN_CAPACITY);
            self.data.resize(self.data_len + estimated_data, 0);

            // Try to read a record
            loop {
                // Handle empty records at record boundaries before passing to csv_core
                (input_offset, read) =
                    self.insert_empty_records_at_boundary(input, input_offset, read, to_read);

                if read >= to_read {
                    return Ok((read, input_offset));
                }

                // Don't return early if we have partial field data - need to call csv_core
                // to potentially finalize the record on empty input (EOF signal)
                if input_offset >= input.len() && !self.has_partial_field {
                    return Ok((read, input_offset));
                }

                // bytes_read: include terminator tokens read from input buffer
                // bytes written: terminator token is NOT written to the buffer
                // end_positions: number of csv fields in the row consumed on this read
                let (result, bytes_read, bytes_written, end_positions) =
                    self.delimiter.read_record(
                        &input[input_offset..],
                        &mut self.data[self.data_len..],
                        &mut self.offsets[self.offsets_len..],
                    );

                self.current_field += end_positions;
                self.offsets_len += end_positions;
                input_offset += bytes_read;
                self.data_len += bytes_written;

                // if any work waas done in this iteration, we are mid-field
                // otherwise, we persist previous state of has_partial_field
                // which may have come from previous iterations or previous decode calls
                if bytes_read > 0 || bytes_written > 0 {
                    self.has_partial_field = true;
                }

                match result {
                    ReadRecordResult::End => {
                        debug_assert!(
                            self.terminator_state != TerminatorState::PendingEmptyCR,
                            "PendingEmptyCR should be handled at start of decode()"
                        );
                        return Ok((read, input_offset));
                    }
                    ReadRecordResult::InputEmpty => {
                        // Reached end of input and csv_core needs more data to construct record
                        // has_partial_field already set above
                        return Ok((read, input_offset));
                    }
                    // Need to allocate more capacity
                    ReadRecordResult::OutputFull => break,
                    ReadRecordResult::OutputEndsFull => {
                        return Err(ArrowError::CsvError(format!(
                            "incorrect number of fields for line {}, expected {} got more than {}",
                            self.line_number, self.num_columns, self.current_field
                        )));
                    }
                    ReadRecordResult::Record => {
                        // Finished reading a full row. Make a record.
                        // Handle row-truncation if current number of fields read differ from num_columns expected
                        if self.current_field != self.num_columns {
                            if self.truncated_rows && self.current_field < self.num_columns {
                                // If the number of fields is less than expected, pad with nulls
                                let fill_count = self.num_columns - self.current_field;
                                let fill_value = self.offsets[self.offsets_len - 1];
                                debug_assert!(
                                    self.offsets_len + fill_count <= self.offsets.len(),
                                    "self.offsets does not have enough room to allocate records!"
                                );
                                self.offsets[self.offsets_len..self.offsets_len + fill_count]
                                    .fill(fill_value);
                                self.offsets_len += fill_count;
                            } else {
                                return Err(ArrowError::CsvError(format!(
                                    "incorrect number of fields for line {}, expected {} got {}",
                                    self.line_number, self.num_columns, self.current_field
                                )));
                            }
                        }
                        read += 1;
                        self.current_field = 0;
                        self.line_number += 1;
                        self.num_rows += 1;
                        self.has_partial_field = false; // Record complete

                        // Update terminator state for next iteration
                        if bytes_read > 0 {
                            let last_byte = input[input_offset - 1];
                            self.on_record_emitted(Some(last_byte));
                        }

                        if read == to_read {
                            // Read sufficient rows
                            return Ok((read, input_offset));
                        }

                        if input.len() == input_offset {
                            // Input exhausted, need to read more
                            // Without this read_record will interpret the empty input
                            // byte array as indicating the end of the file
                            return Ok((read, input_offset));
                        }
                    }
                }
            }
        }
    }

    /// Returns the current number of buffered records
    pub fn len(&self) -> usize {
        self.num_rows
    }

    /// Returns true if the decoder is empty
    pub fn is_empty(&self) -> bool {
        self.num_rows == 0
    }

    /// Clears the current contents of the decoder
    pub fn clear(&mut self) {
        // This does not reset current_field to allow clearing part way through a record
        self.offsets_len = 1;
        self.data_len = 0;
        self.num_rows = 0;
    }

    /// Flushes the current contents of the reader
    pub fn flush(&mut self) -> Result<StringRecords<'_>, ArrowError> {
        if self.current_field != 0 {
            return Err(ArrowError::CsvError(
                "Cannot flush part way through record".to_string(),
            ));
        }

        // csv_core::Reader writes end offsets relative to the start of the row
        // Therefore scan through and offset these based on the cumulative row offsets
        let mut row_offset = 0;
        self.offsets[1..self.offsets_len]
            .chunks_exact_mut(self.num_columns)
            .for_each(|row| {
                let offset = row_offset;
                row.iter_mut().for_each(|x| {
                    *x += offset;
                    row_offset = *x;
                });
            });

        // Need to truncate data to the actual amount of data read
        let data = std::str::from_utf8(&self.data[..self.data_len]).map_err(|e| {
            let valid_up_to = e.valid_up_to();

            // We can't use binary search because of empty fields
            let idx = self.offsets[..self.offsets_len]
                .iter()
                .rposition(|x| *x <= valid_up_to)
                .unwrap();

            let field = idx % self.num_columns + 1;
            let line_offset = self.line_number - self.num_rows;
            let line = line_offset + idx / self.num_columns;

            ArrowError::CsvError(format!(
                "Encountered invalid UTF-8 data for line {line} and field {field}"
            ))
        })?;

        let offsets = &self.offsets[..self.offsets_len];
        let num_rows = self.num_rows;

        // Reset state
        self.offsets_len = 1;
        self.data_len = 0;
        self.num_rows = 0;

        Ok(StringRecords {
            num_rows,
            num_columns: self.num_columns,
            offsets,
            data,
        })
    }
}

/// A collection of parsed, UTF-8 CSV records
#[derive(Debug)]
pub struct StringRecords<'a> {
    num_columns: usize,
    num_rows: usize,
    offsets: &'a [usize],
    data: &'a str,
}

impl<'a> StringRecords<'a> {
    fn get(&self, index: usize) -> StringRecord<'a> {
        let field_idx = index * self.num_columns;
        StringRecord {
            data: self.data,
            offsets: &self.offsets[field_idx..field_idx + self.num_columns + 1],
        }
    }

    pub fn len(&self) -> usize {
        self.num_rows
    }

    pub fn iter(&self) -> impl Iterator<Item = StringRecord<'a>> + '_ {
        (0..self.num_rows).map(|x| self.get(x))
    }
}

/// A single parsed, UTF-8 CSV record
#[derive(Debug, Clone, Copy)]
pub struct StringRecord<'a> {
    data: &'a str,
    offsets: &'a [usize],
}

impl<'a> StringRecord<'a> {
    pub fn get(&self, index: usize) -> &'a str {
        let end = self.offsets[index + 1];
        let start = self.offsets[index];

        // SAFETY:
        // Parsing produces offsets at valid byte boundaries
        unsafe { self.data.get_unchecked(start..end) }
    }
}

impl std::fmt::Display for StringRecord<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let num_fields = self.offsets.len() - 1;
        write!(f, "[")?;
        for i in 0..num_fields {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{}", self.get(i))?;
        }
        write!(f, "]")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::reader::records::RecordDecoder;
    use csv_core::Reader;
    use std::io::{BufRead, BufReader, Cursor};

    /// Marker for an empty row. Use in expected data arrays
    const EMPTY: &[&str] = &[];

    const BUFFER_SIZES: &[usize] = &[1, 2, 3, 4, 5, 6, 7, 8, 10, 20];

    /// Helper to assert decoded records match expected 2D data.
    fn assert_records_eq(decoder: &mut RecordDecoder, num_cols: usize, expected: &[&[&str]]) {
        let batch = decoder.flush().unwrap();

        let records: Vec<Vec<&str>> = batch
            .iter()
            .map(|r| (0..num_cols).map(|j| r.get(j)).collect())
            .collect();

        assert_eq!(
            records.len(),
            expected.len(),
            "Row count mismatch: got {}, expected {}",
            records.len(),
            expected.len()
        );

        for (i, (actual, exp)) in records.iter().zip(expected.iter()).enumerate() {
            // Expand EMPTY marker to vec of empty strings
            let expected_row: Vec<&str> = if exp.is_empty() {
                vec![""; num_cols]
            } else {
                exp.to_vec()
            };

            assert_eq!(
                actual, &expected_row,
                "Row {} mismatch:\n  actual:   {:?}\n  expected: {:?}",
                i, actual, expected_row
            );
        }
    }

    /// Helper to run a stress test with various buffer capacities
    /// This will aggressively test the terminator state transition
    /// since buffers are small and we increase the likelihood of spill-over terminator
    /// tokens cutting \r\n into two consecutive decode calls
    fn stress_test_with_capacities(
        csv: &[u8],
        num_cols: usize,
        expected_rows: usize,
        expected: &[&[&str]],
    ) {
        for &capacity in BUFFER_SIZES {
            let mut reader = BufReader::with_capacity(capacity, Cursor::new(csv));
            let mut decoder = RecordDecoder::new(Reader::new(), num_cols, true);
            let mut total_read = 0;

            loop {
                let buf = reader.fill_buf().unwrap();
                assert!(buf.len() <= capacity);
                if buf.is_empty() {
                    // Signal EOF with empty input to finalize any pending records
                    let (read, _) = decoder.decode(&[], expected_rows).unwrap();
                    total_read += read;
                    break;
                }

                let (read, bytes) = decoder.decode(buf, expected_rows).unwrap();
                reader.consume(bytes);
                total_read += read;
            }

            assert_eq!(
                total_read, expected_rows,
                "capacity {}: expected {} records, got {}",
                capacity, expected_rows, total_read
            );

            assert_records_eq(&mut decoder, num_cols, expected);
        }
    }

    #[test]
    fn test_basic() {
        let csv = "foo,bar,baz\na,b,c\n12,3,5\n\"asda\"\"asas\",\"sdffsnsd\", as\n";

        let mut decoder = RecordDecoder::new(Reader::new(), 3, false);
        let (read, bytes) = decoder.decode(csv.as_bytes(), 4).unwrap();
        assert_eq!(read, 4);
        assert_eq!(bytes, csv.len());

        assert_records_eq(
            &mut decoder,
            3,
            &[
                &["foo", "bar", "baz"],
                &["a", "b", "c"],
                &["12", "3", "5"],
                &["asda\"asas", "sdffsnsd", " as"],
            ],
        );
    }

    #[test]
    fn test_basic_buffered() {
        // Test with small buffer to exercise buffered reading
        let csv = [
            "foo,bar,baz",
            "a,b,c",
            "12,3,5",
            "\"asda\"\"asas\",\"sdffsnsd\", as",
        ]
        .join("\n");

        let mut expected = vec![
            vec!["foo", "bar", "baz"],
            vec!["a", "b", "c"],
            vec!["12", "3", "5"],
            vec!["asda\"asas", "sdffsnsd", " as"],
        ]
        .into_iter();

        let mut reader = BufReader::with_capacity(3, Cursor::new(csv.as_bytes()));
        let mut decoder = RecordDecoder::new(Reader::new(), 3, false);

        loop {
            let to_read = 3;
            let mut read = 0;
            loop {
                let buf = reader.fill_buf().unwrap();
                let (records, bytes) = decoder.decode(buf, to_read - read).unwrap();

                reader.consume(bytes);
                read += records;

                if read == to_read || bytes == 0 {
                    break;
                }
            }
            if read == 0 {
                break;
            }

            let b = decoder.flush().unwrap();
            b.iter().zip(&mut expected).for_each(|(record, expected)| {
                let actual = (0..3)
                    .map(|field_idx| record.get(field_idx))
                    .collect::<Vec<_>>();
                assert_eq!(actual, expected)
            });
        }
        assert!(expected.next().is_none());
    }

    #[test]
    fn test_invalid_fields() {
        let csv = "a,b\nb,c\na\n";
        let mut decoder = RecordDecoder::new(Reader::new(), 2, false);
        let err = decoder.decode(csv.as_bytes(), 4).unwrap_err().to_string();

        let expected = "Csv error: incorrect number of fields for line 3, expected 2 got 1";

        assert_eq!(err, expected);

        // Test with initial skip
        let mut decoder = RecordDecoder::new(Reader::new(), 2, false);
        let (skipped, bytes) = decoder.decode(csv.as_bytes(), 1).unwrap();
        assert_eq!(skipped, 1);
        decoder.clear();

        let remaining = &csv.as_bytes()[bytes..];
        let err = decoder.decode(remaining, 3).unwrap_err().to_string();
        assert_eq!(err, expected);
    }

    #[test]
    fn test_decode_fewer_rows_than_requested() {
        // Request 3 rows but only 2 available - should return 2
        let csv = "a\nv\n";
        let mut decoder = RecordDecoder::new(Reader::new(), 1, false);
        let (read, bytes) = decoder.decode(csv.as_bytes(), 3).unwrap();
        assert_eq!(read, 2);
        assert_eq!(bytes, csv.len());

        assert_records_eq(&mut decoder, 1, &[&["a"], &["v"]]);
    }

    #[test]
    fn test_truncated_rows() {
        let csv = "a,b\nv\n,1\n,2\n,3\n";
        let mut decoder = RecordDecoder::new(Reader::new(), 2, true);
        let (read, bytes) = decoder.decode(csv.as_bytes(), 5).unwrap();
        assert_eq!(read, 5);
        assert_eq!(bytes, csv.len());

        assert_records_eq(
            &mut decoder,
            2,
            &[&["a", "b"], &["v", ""], &["", "1"], &["", "2"], &["", "3"]],
        );
    }

    #[test]
    fn test_cr_line_endings() {
        // CSV with standalone \r (carriage return) as line endings (classic Mac OS style)
        let csv = "foo,bar,baz\ra,b,c\r12,3,5\r";

        let mut decoder = RecordDecoder::new(Reader::new(), 3, true);
        let (read, bytes) = decoder.decode(csv.as_bytes(), 3).unwrap();
        assert_eq!(read, 3);
        assert_eq!(bytes, csv.len());

        assert_records_eq(
            &mut decoder,
            3,
            &[&["foo", "bar", "baz"], &["a", "b", "c"], &["12", "3", "5"]],
        );
    }

    #[test]
    fn test_cr_empty_lines() {
        // CSV with standalone \r line endings and empty lines in the middle
        let csv = "a,b,c\r\rfoo,bar,baz\r";

        let mut decoder = RecordDecoder::new(Reader::new(), 3, true);
        let (read, bytes) = decoder.decode(csv.as_bytes(), 3).unwrap();
        assert_eq!(read, 3);
        assert_eq!(bytes, csv.len());

        assert_records_eq(
            &mut decoder,
            3,
            &[&["a", "b", "c"], EMPTY, &["foo", "bar", "baz"]],
        );
    }

    #[test]
    fn test_crlf_split_across_buffers() {
        // Test case: \r\n is split across buffer boundaries
        // CSV cut into 2 buffers: "a,b,c\r  |   \nx,y,z\r\n"
        // Should produce 2 rows, NOT 3

        let csv = "a,b,c\r\nx,y,z\r\n";
        let split_point = 6; // "a,b,c\r" = 6 bytes

        let mut decoder = RecordDecoder::new(Reader::new(), 3, true);

        let (read1, bytes1) = decoder.decode(&csv.as_bytes()[..split_point], 2).unwrap();
        assert_eq!(read1, 1);
        assert_eq!(bytes1, split_point);

        let buf2 = &csv.as_bytes()[split_point..];
        let (read2, bytes2) = decoder.decode(buf2, 1).unwrap();
        assert_eq!(read2, 1);

        // Consume any remaining bytes (trailing \n handled by next decode call)
        let mut total_bytes2 = bytes2;
        if bytes2 < buf2.len() {
            let (read3, bytes3) = decoder.decode(&buf2[bytes2..], 1).unwrap();
            assert_eq!(read3, 0); // No more records (just terminator cleanup)
            total_bytes2 += bytes3;
        }
        assert_eq!(total_bytes2, buf2.len());

        assert_records_eq(&mut decoder, 3, &[&["a", "b", "c"], &["x", "y", "z"]]);
    }

    #[test]
    fn test_buffer_boundary_after_crlf() {
        // Test case: buffer cuts cleanly after \r\n
        // CSV cut into 2 buffers: "a,b,c\r\n  |   x,y,z\r\n"
        let csv = "a,b,c\r\nx,y,z\r\n";
        let split_point = 7; // "a,b,c\r\n" = 7 bytes

        let mut decoder = RecordDecoder::new(Reader::new(), 3, true);

        let (read1, bytes1) = decoder.decode(&csv.as_bytes()[..split_point], 2).unwrap();
        assert_eq!(read1, 1);
        assert_eq!(bytes1, split_point);

        let buf2 = &csv.as_bytes()[split_point..];
        let (read2, bytes2) = decoder.decode(buf2, 1).unwrap();
        assert_eq!(read2, 1);

        // Consume any remaining bytes (trailing \n handled by next decode call)
        let mut total_bytes2 = bytes2;
        if bytes2 < buf2.len() {
            let (read3, bytes3) = decoder.decode(&buf2[bytes2..], 1).unwrap();
            assert_eq!(read3, 0); // No more records (just terminator cleanup)
            total_bytes2 += bytes3;
        }
        assert_eq!(total_bytes2, buf2.len());

        assert_records_eq(&mut decoder, 3, &[&["a", "b", "c"], &["x", "y", "z"]]);
    }

    #[test]
    fn test_crlf_split_with_empty_line() {
        // Test case: \r\n split with an actual empty line following
        // CSV cut into  "a,b,c\r  |  \n\r\nx,y,z\r\n"
        let csv = "a,b,c\r\n\r\nx,y,z\r\n";
        let split_point = 6; // "a,b,c\r" = 6 bytes

        let mut decoder = RecordDecoder::new(Reader::new(), 3, true);

        let (read1, bytes1) = decoder.decode(&csv.as_bytes()[..split_point], 3).unwrap();
        // read "a,b,c\r"
        assert_eq!(read1, 1);
        assert_eq!(bytes1, split_point);

        let buf2 = &csv.as_bytes()[split_point..];
        let (read2, bytes2) = decoder.decode(buf2, 2).unwrap();
        // read \r\n
        // read x,y,z\r
        assert_eq!(read2, 2);

        // Consume any remaining bytes (trailing \n handled by next decode call)
        let mut total_bytes2 = bytes2;
        if bytes2 < buf2.len() {
            let (read3, bytes3) = decoder.decode(&buf2[bytes2..], 1).unwrap();
            assert_eq!(read3, 0); // No more records (just terminator cleanup)
            total_bytes2 += bytes3;
        }
        assert_eq!(total_bytes2, buf2.len());

        assert_records_eq(
            &mut decoder,
            3,
            &[&["a", "b", "c"], EMPTY, &["x", "y", "z"]],
        );
    }

    #[test]
    fn test_standalone_cr_at_buffer_boundary() {
        // Test case: standalone \r (empty line) at buffer boundary
        // CSV: "a,b,c\n\rx,y,z\n" split as "a,b,c\n\r" | "x,y,z\n"
        // Row 1: a,b,c (terminated by \n)
        // Row 2: empty (standalone \r)
        // Row 3: x,y,z (terminated by \n)
        let csv = "a,b,c\n\rx,y,z\n";
        let split_point = 7; // "a,b,c\n\r" = 7 bytes

        let mut decoder = RecordDecoder::new(Reader::new(), 3, true);

        // First buffer: "a,b,c\n\r"
        // - csv_core reads "a,b,c", sees \n, returns Record
        let (read1, bytes1) = decoder.decode(&csv.as_bytes()[..split_point], 3).unwrap();
        assert_eq!(read1, 1, "Should read 1 row from first buffer");
        assert_eq!(bytes1, split_point);

        // Second buffer: "x,y,z\n"
        let (read2, bytes2) = decoder.decode(&csv.as_bytes()[split_point..], 2).unwrap();
        assert_eq!(read2, 2, "Should read empty line + x,y,z");
        assert_eq!(bytes2, csv.len() - split_point);

        assert_records_eq(
            &mut decoder,
            3,
            &[&["a", "b", "c"], EMPTY, &["x", "y", "z"]],
        );
    }

    #[test]
    fn test_pending_empty_cr_at_eof() {
        // CSV: "a\n\r" | "" (EOF) - the \r is an empty line at end of file
        let csv = b"a\n\r";

        let mut decoder = RecordDecoder::new(Reader::new(), 1, true);

        let (read1, bytes1) = decoder.decode(csv, 10).unwrap();
        assert_eq!(bytes1, csv.len());

        // Signal EOF with empty input
        let (read2, _) = decoder.decode(&[], 10 - read1).unwrap();

        assert_eq!(read1 + read2, 2, "expected 2 records (a, empty)");
        assert_records_eq(&mut decoder, 1, &[&["a"], EMPTY]);
    }

    #[test]
    fn test_record_ended_with_cr_then_data() {
        // The \r was a standalone terminator
        // CSV: "a,b\r" | "c,d\r\n"
        let csv = b"a,b\rc,d\r\n";
        let split = 4; // "a,b\r" | "c,d\r\n"

        let mut decoder = RecordDecoder::new(Reader::new(), 2, true);

        let (read1, _) = decoder.decode(&csv[..split], 10).unwrap();
        let (read2, _) = decoder.decode(&csv[split..], 10 - read1).unwrap();

        assert_eq!(read1 + read2, 2, "expected 2 records");
        assert_records_eq(&mut decoder, 2, &[&["a", "b"], &["c", "d"]]);
    }

    #[test]
    fn test_record_ended_with_cr_then_empty_cr() {
        // CSV: "a\r" | "\rb\n" - first \r ends record, second \r is empty line
        let csv = b"a\r\rb\n";
        let expected = &[&["a"], EMPTY, &["b"]];
        stress_test_with_capacities(csv, 1, expected.len(), expected);
    }

    #[test]
    fn test_multiple_cr_at_boundary() {
        // Test multiple \r characters at buffer boundary
        let csv = b"a\r\r\rb\n";
        let expected = &[&["a"], EMPTY, EMPTY, &["b"]];
        stress_test_with_capacities(csv, 1, expected.len(), expected);
    }

    #[test]
    fn test_all_terminator_types_with_splits() {
        // Test all terminator types (\n, \r\n, \r) with systematic buffer splits
        // CSV: "a\n" "b\r\n" "c\r" "d\n" = 4 records
        let csv = b"a\nb\r\nc\rd\n";
        let expected: &[&[&str]] = &[&["a"], &["b"], &["c"], &["d"]];

        stress_test_with_capacities(csv, 1, expected.len(), expected);
    }

    #[test]
    fn test_empty_line_at_every_position() {
        // CSV with empty lines at start, middle, and end
        // "\n" "a\n" "\n" "b\n" "\n"
        let csv = b"\na\n\nb\n\n";
        let expected = &[EMPTY, &["a"], EMPTY, &["b"], EMPTY];

        stress_test_with_capacities(csv, 1, expected.len(), expected);
    }

    #[test]
    fn test_consecutive_crlf_empty_lines() {
        // Multiple \r\n empty lines in a row
        // CSV: "a\r\n" "\r\n" "\r\n" "b\r\n"
        let csv = b"a\r\n\r\n\r\nb\r\n";
        let expected = &[&["a"], EMPTY, EMPTY, &["b"]];

        stress_test_with_capacities(csv, 1, expected.len(), expected);
    }

    #[test]
    fn test_mixed_line_endings_with_empty_lines() {
        let csv = b"a,b,c\r\n\n\rx,y,z\r\n\r\n\n1,2,3\n";
        let expected = &[
            &["a", "b", "c"], // row 0 (CRLF)
            EMPTY,            // row 1: empty (\n)
            EMPTY,            // row 2: empty (\r standalone)
            &["x", "y", "z"], // row 3 (CRLF)
            EMPTY,            // row 4: empty (\r\n)
            EMPTY,            // row 5: empty (\n)
            &["1", "2", "3"], // row 6 (LF)
        ];
        stress_test_with_capacities(csv, 3, expected.len(), expected);
    }

    #[test]
    fn test_alternating_terminators() {
        // Tests all three terminator types (\n, \r\n, \r) interleaved with data
        // Pattern: a\n + \r\n (empty) + b\r + \r\n (empty) + c\n
        let csv = b"a\n\r\nb\r\r\nc\n";
        let expected: &[&[&str]] = &[&["a"], EMPTY, &["b"], EMPTY, &["c"]];

        stress_test_with_capacities(csv, 1, expected.len(), expected);
    }

    #[test]
    fn test_empty_lines_at_start() {
        // Pattern: \n + \r\n + \r + a,b,c\n
        let csv = b"\n\r\n\ra,b,c\n";
        let expected: &[&[&str]] = &[EMPTY, EMPTY, EMPTY, &["a", "b", "c"]];

        stress_test_with_capacities(csv, 3, expected.len(), expected);
    }

    #[test]
    fn test_empty_lines_at_end() {
        // Pattern: a,b,c\n + \n + \r\n + \r (EOF)
        let csv = b"a,b,c\n\n\r\n\r";
        let expected: &[&[&str]] = &[&["a", "b", "c"], EMPTY, EMPTY, EMPTY];

        stress_test_with_capacities(csv, 3, expected.len(), expected);
    }

    #[test]
    fn test_many_consecutive_empty_lines() {
        // Pattern: a\n + \n + \n + \r\n + \r\n + \r + \r + b\n
        let csv = b"a\n\n\n\r\n\r\n\r\rb\n";
        let expected: &[&[&str]] = &[&["a"], EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, &["b"]];

        stress_test_with_capacities(csv, 1, expected.len(), expected);
    }

    #[test]
    fn test_multicolumn_with_empty_lines() {
        let csv = b"foo,bar,baz\r\n\r\n1,2,3\n\nx,y,z\r";
        let expected: &[&[&str]] = &[
            &["foo", "bar", "baz"],
            EMPTY,
            &["1", "2", "3"],
            EMPTY,
            &["x", "y", "z"],
        ];

        stress_test_with_capacities(csv, 3, expected.len(), expected);
    }

    #[test]
    fn test_quoted_fields_with_empty_lines() {
        // Tests that terminator state is NOT affected by commas/quotes inside fields
        // csv_core handles quoted fields; we only track terminators at record boundaries
        // Pattern: "a,b",c,d\r\n + \n (empty) + "x""y",z,w\n
        let csv = b"\"a,b\",c,d\r\n\n\"x\"\"y\",z,w\n";
        let expected: &[&[&str]] = &[&["a,b", "c", "d"], EMPTY, &["x\"y", "z", "w"]];

        stress_test_with_capacities(csv, 3, expected.len(), expected);
    }

    #[test]
    fn test_all_empty_file() {
        // Pattern: \n + \r\n + \r\n + \r (EOF)
        let csv = b"\n\r\n\r\n\r";
        let expected: &[&[&str]] = &[EMPTY, EMPTY, EMPTY, EMPTY];

        stress_test_with_capacities(csv, 1, expected.len(), expected);
    }

    #[test]
    fn test_quoted_field_spanning_buffer_boundaries() {
        let csv = b"\"A\r\nB\r\nC\r\nD\",simple\n\nx,\"Y\rZ\"\nz\n";

        let expected: &[&[&str]] = &[
            &["A\r\nB\r\nC\r\nD", "simple"],
            EMPTY,
            &["x", "Y\rZ"],
            &["z", ""], // missing field padded with empty
        ];

        stress_test_with_capacities(csv, 2, expected.len(), expected);
    }
}
