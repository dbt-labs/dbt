//! Table fixtures shared by the tests of this crate.

use std::sync::Arc;

use arrow::record_batch::RecordBatch;
use arrow_array::{Int64Array, StringArray};
use arrow_schema::{DataType, Field, Schema};

use crate::AgateTable;

/// | id: i64 NOT NULL | name: str NOT NULL | fk: i64 |
pub fn main_table(id: &[i64], name: &[&str], fk: &[Option<i64>]) -> AgateTable {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("fk", DataType::Int64, true),
        ])),
        vec![
            Arc::new(Int64Array::from(id.to_vec())),
            Arc::new(StringArray::from(name.to_vec())),
            Arc::new(Int64Array::from(fk.to_vec())),
        ],
    )
    .unwrap();
    AgateTable::from_record_batch(Arc::new(batch))
}

/// | id: i64 | name: str NOT NULL |
pub fn related_table(id: &[Option<i64>], name: &[&str]) -> AgateTable {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, true),
            Field::new("name", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(id.to_vec())),
            Arc::new(StringArray::from(name.to_vec())),
        ],
    )
    .unwrap();
    AgateTable::from_record_batch(Arc::new(batch))
}

/// | id: i64 NOT NULL | name: str NOT NULL | genre: str NOT NULL |
pub fn genre_table(id: &[i64], name: &[&str], genre: &[&str]) -> AgateTable {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("genre", DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(id.to_vec())),
            Arc::new(StringArray::from(name.to_vec())),
            Arc::new(StringArray::from(genre.to_vec())),
        ],
    )
    .unwrap();
    AgateTable::from_record_batch(Arc::new(batch))
}

pub fn some(values: &[i64]) -> Vec<Option<i64>> {
    values.iter().map(|&v| Some(v)).collect()
}

pub fn two_column_table(id: (&str, &[i64]), name: (&str, &[&str])) -> AgateTable {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new(id.0, DataType::Int64, false),
            Field::new(name.0, DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(id.1.to_vec())),
            Arc::new(StringArray::from(name.1.to_vec())),
        ],
    )
    .unwrap();
    AgateTable::from_record_batch(Arc::new(batch))
}

pub fn three_column_table(
    id: (&str, &[i64]),
    name: (&str, &[&str]),
    other: (&str, &[&str]),
) -> AgateTable {
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new(id.0, DataType::Int64, false),
            Field::new(name.0, DataType::Utf8, false),
            Field::new(other.0, DataType::Utf8, false),
        ])),
        vec![
            Arc::new(Int64Array::from(id.1.to_vec())),
            Arc::new(StringArray::from(name.1.to_vec())),
            Arc::new(StringArray::from(other.1.to_vec())),
        ],
    )
    .unwrap();
    AgateTable::from_record_batch(Arc::new(batch))
}

/// The rows of `table` rendered as strings, for compact assertions.
pub fn stringly_rows_of(table: &AgateTable, sep: &str) -> Vec<String> {
    (0..table.num_rows())
        .map(|row_idx| {
            (0..table.num_columns())
                .map(|col_idx| {
                    table
                        .cell(row_idx as isize, col_idx as isize)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect::<Vec<String>>()
                .join(sep)
        })
        .collect()
}
