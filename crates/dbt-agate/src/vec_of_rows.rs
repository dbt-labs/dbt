//! The vec-of-rows representation of Agate tables.
//!
//!

use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow_schema::ArrowError;
use minijinja::Value;

use crate::flat_record_batch::FlatRecordBatch;

/// Internal table representation that stores rows as a vector of minijinja values.
#[derive(Debug)]
pub(crate) struct VecOfRows {
    rows: Vec<Value>,
}

impl VecOfRows {
    pub fn from_flat_record_batch(batch: &FlatRecordBatch) -> Result<Self, ArrowError> {
        let converters = batch.converters();
        let nrows = batch.num_rows();
        let ncols = batch.num_columns();
        let mut rows = Vec::with_capacity(nrows);
        for row_idx in 0..nrows {
            let mut row = Vec::with_capacity(ncols);
            for col_converter in converters.iter() {
                let value = (*col_converter).to_value(row_idx);
                row.push(value);
            }
            rows.push(Value::from(row));
        }

        let vec_of_rows = VecOfRows { rows };
        Ok(vec_of_rows)
    }

    /// Create a VecOfRows from an Arrow RecordBatch.
    #[allow(dead_code)]
    pub fn from_record_batch(batch: Arc<RecordBatch>) -> Result<Self, ArrowError> {
        let flat = FlatRecordBatch::try_new(batch)?;
        Self::from_flat_record_batch(&flat)
    }

    #[allow(dead_code)]
    pub fn with_single_column(&self, idx: usize) -> Result<VecOfRows, minijinja::Error> {
        let results = (|| -> Result<Vec<Value>, minijinja::Error> {
            let mut rows = Vec::new();
            for row in self.rows_ref() {
                row.get_item_by_index(idx).map(|value| rows.push(value))?;
            }
            Ok(rows)
        })();
        let rows = results?;
        let new_vec_of_rows = VecOfRows { rows };
        Ok(new_vec_of_rows)
    }

    /// Get the rows.
    pub fn rows_ref(&self) -> &Vec<Value> {
        &self.rows
    }
}
