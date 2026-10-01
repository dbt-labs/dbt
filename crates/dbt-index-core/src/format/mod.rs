use arrow_array::RecordBatch;
use arrow_cast::display::{ArrayFormatter, FormatOptions};

// ---------------------------------------------------------------------------
// Shared batch utilities
// ---------------------------------------------------------------------------

/// Format options for Arrow display — no null sentinel, bare values.
pub static FMT_OPTS: FormatOptions<'static> = FormatOptions::new();

/// Extract a cell value as a display string from a RecordBatch.
pub fn cell_to_string(batch: &RecordBatch, row: usize, col: usize) -> String {
    let array = batch.column(col);
    if array.is_null(row) {
        return String::new();
    }
    let Ok(formatter) = ArrayFormatter::try_new(array.as_ref(), &FMT_OPTS) else {
        return String::new();
    };
    formatter.value(row).to_string()
}

/// Return the first batch that has at least one row, or `None`.
pub fn first_nonempty(batches: &[RecordBatch]) -> Option<&RecordBatch> {
    batches.iter().find(|b| b.num_rows() > 0)
}
