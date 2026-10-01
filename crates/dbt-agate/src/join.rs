use arrow::record_batch::RecordBatch;
use arrow_array::{Array, ArrayRef, RecordBatchOptions, StringViewArray, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use minijinja::{Error, ErrorKind, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::AgateTable;
use crate::grouper::{Grouper, SipHash128};

#[derive(Default)]
pub enum JoinType {
    #[default]
    LeftOuter,
    Inner,
    FullOuter,
}

impl JoinType {
    pub fn is_inner(&self) -> bool {
        matches!(self, JoinType::Inner)
    }

    pub fn is_full_outer(&self) -> bool {
        matches!(self, JoinType::FullOuter)
    }
}

pub enum JoinKey {
    /// Sequential join: the key of a row is its row number.
    RowNumbers,
    /// The key of a row is the tuple of values in these columns.
    Columns(Vec<usize>),
}

impl JoinKey {
    /// The key column indices -- empty when rows are keyed by row number.
    pub fn as_slice(&self) -> &[usize] {
        match self {
            JoinKey::RowNumbers => &[],
            JoinKey::Columns(indices) => indices.as_slice(),
        }
    }

    /// The join key of a row of `table`, rendered the way Python renders `left_value`.
    ///
    /// Only used in error messages -- rows are matched by their hashes.
    pub fn key_of_row(&self, table: &AgateTable, row_idx: usize) -> String {
        match self {
            JoinKey::RowNumbers => row_idx.to_string(),
            JoinKey::Columns(indices) => {
                let mut values = indices.iter().map(|&col_idx| {
                    table
                        .cell(row_idx as isize, col_idx as isize)
                        .unwrap_or_default()
                        .to_string()
                });
                if indices.len() == 1 {
                    values.next().unwrap_or_default()
                } else {
                    format!("({})", values.collect::<Vec<String>>().join(", "))
                }
            }
        }
    }

    /// Build a [Grouper] that hashes the rows of `table` keyd by this key.
    ///
    /// Panics if a key column has a type that cannot be hashed (e.g. a list).
    pub fn to_grouper(&self, table: &AgateTable) -> Grouper {
        match self {
            JoinKey::RowNumbers => {
                let row_numbers = UInt64Array::from_iter_values(0..table.num_rows() as u64);
                let schema = Arc::new(Schema::new(vec![Field::new(
                    "row_number",
                    DataType::UInt64,
                    false,
                )]));
                let batch = RecordBatch::try_new(schema, vec![Arc::new(row_numbers)])
                    .expect("a single UInt64 column always matches its own schema");
                Grouper::from_record_batch_columns(&batch, &[0])
                    .expect("UInt64 columns are hashable")
            }
            JoinKey::Columns(indices) => {
                Grouper::from_record_batch_columns(&table.to_record_batch(), indices.as_slice())
                    .expect("join key columns are hashable")
            }
        }
    }
}

/// Parse the `columns` argument of the `Table.join` method.
///
// ```
// :param columns:
//     A sequence of column names from :code:`right_table` to include in
//     the final output table. Defaults to all columns not in
//     :code:`right_key`. Ignored when :code:`full_outer` is :code:`True`.
// ```
//
// `None` means "all columns not in right_key" (the default), `Some([])` means
// "no columns", and `Some([name1, name2, ...])` means "only these columns".
fn selected_right_table_columns(columns: Option<&Value>) -> Result<Option<Vec<String>>, Error> {
    let Some(columns) = columns else {
        return Ok(None);
    };
    let iter = columns.try_iter().map_err(|e| {
        Error::new(
            ErrorKind::InvalidArgument,
            format!("Table.join: columns must be a sequence of column names: {e}"),
        )
    })?;
    let mut names = Vec::new();
    for column in iter {
        match column.as_str() {
            Some(name) => names.push(name.to_string()),
            None => {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    format!(
                        "Table.join: columns must be a sequence of column names: {column} found instead"
                    ),
                ));
            }
        }
    }
    Ok(Some(names))
}

impl AgateTable {
    pub fn join(
        &self,
        right_table: &AgateTable,
        left_key: Option<&Value>,
        right_key: Option<&Value>,
        join_type: JoinType,
        require_match: bool,
        columns: Option<&Value>,
    ) -> Result<AgateTable, Error> {
        let full_outer = join_type.is_full_outer();
        let columns = selected_right_table_columns(columns)?;

        let (left_key, right_key) = match (left_key, right_key) {
            // Sequential join: both sides are keyed by row number.
            (None, _) => (JoinKey::RowNumbers, JoinKey::RowNumbers),
            // Left join: the left key is used for both sides.
            (Some(inner_key), None) => (
                JoinKey::Columns(self.column_indices_of(inner_key)?),
                JoinKey::Columns(right_table.column_indices_of(inner_key)?),
            ),
            (Some(left_key), Some(right_key)) => (
                JoinKey::Columns(self.column_indices_of(left_key)?),
                JoinKey::Columns(right_table.column_indices_of(right_key)?),
            ),
        };

        let right_projection_columns = (0..right_table.num_columns())
            .filter(|&i| {
                if full_outer {
                    return true;
                }
                match &columns {
                    // by default, every right-hand column but the key columns
                    None => !right_key.as_slice().contains(&i),
                    // `columns` names the right-hand columns to include
                    Some(columns) => right_table
                        .column_name(i as isize)
                        .is_some_and(|name| columns.contains(name)),
                }
            })
            .collect::<Vec<usize>>();

        // We use groupers to fingerprint the rows indexed by the left and right keys.
        // Then comparing these fingerprints is equivalent to comparing the tuples.
        let left_tuple_hasher = left_key.to_grouper(self);
        let right_tuple_hasher = right_key.to_grouper(right_table);

        // ```python
        //     right_hash = {}
        //
        //     for i, value in enumerate(right_data):
        //         if value not in right_hash:
        //             right_hash[value] = []
        //
        //         right_hash[value].append(right_table._rows[i])
        // ```
        //
        // Python implementation maps the key value to the right-hand rows themselves;
        // here the rows are kept as indices into the right table operand.
        let mut right_hash: HashMap<SipHash128, Vec<usize>> = HashMap::new();
        for row_idx in 0..right_table.num_rows() {
            right_hash
                .entry(right_tuple_hasher.hash_row(row_idx))
                .or_insert_with(|| Vec::with_capacity(1))
                .push(row_idx);
        }

        // ```python
        //     # Collect new rows
        //     rows = []
        //     ...
        //     # Iterate over left column
        //     for left_index, left_value in enumerate(left_data):
        //         matching_rows = right_hash.get(left_value, None)
        //
        //         if require_match and matching_rows is None:
        //             raise ValueError('Left key "%s" does not have a matching right key.' % left_value)
        //
        //         # Rows with matches
        //         if matching_rows:
        //             for right_row in matching_rows:
        //                 ...
        //                 rows.append(Row(new_row, column_names))
        //         # Rows without matches
        //         elif not inner:
        //             ...
        //             rows.append(Row(new_row, column_names))
        // ```
        //
        // Instead of materializing the rows, the joined table is described as pairs of
        // row indices into the left and the right table. `None` on either side means the
        // row has no counterpart there and the columns of that side are NULL.
        let mut joined_rows: Vec<(Option<usize>, Option<usize>)> = Vec::new();
        for left_row_idx in 0..self.num_rows() {
            match right_hash.get(&left_tuple_hasher.hash_row(left_row_idx)) {
                Some(right_row_indices) => {
                    // one joined row per matching right row
                    for &right_row_idx in right_row_indices {
                        joined_rows.push((Some(left_row_idx), Some(right_row_idx)));
                    }
                }
                None => {
                    if require_match {
                        return Err(Error::new(
                            ErrorKind::InvalidOperation,
                            format!(
                                "Left key \"{}\" does not have a matching right key.",
                                left_key.key_of_row(self, left_row_idx)
                            ),
                        ));
                    }
                    if !join_type.is_inner() {
                        joined_rows.push((Some(left_row_idx), None));
                    }
                }
            }
        }

        // ```python
        //     # Full outer join
        //     if full_outer:
        //         left_set = set(left_data)
        //
        //         for right_index, right_value in enumerate(right_data):
        //             if right_value in left_set:
        //                 continue
        //
        //             new_row = ([None] * len(self._columns)) + list(right_table.rows[right_index])
        //
        //             rows.append(Row(new_row, column_names))
        // ```
        if full_outer {
            let left_keys = (0..self.num_rows())
                .map(|row_idx| left_tuple_hasher.hash_row(row_idx))
                .collect::<HashSet<SipHash128>>();
            for right_row_idx in 0..right_table.num_rows() {
                if !left_keys.contains(&right_tuple_hasher.hash_row(right_row_idx)) {
                    joined_rows.push((None, Some(right_row_idx)));
                }
            }
        }

        // Use take (aka gather, select) with the selection vectors produced above
        // to materialize the joined result set.
        let left_row_indices = joined_rows
            .iter()
            .map(|&(left_row_idx, _)| left_row_idx.map(|idx| idx as u64))
            .collect::<UInt64Array>();
        let right_row_indices = joined_rows
            .iter()
            .map(|&(_, right_row_idx)| right_row_idx.map(|idx| idx as u64))
            .collect::<UInt64Array>();

        let left_batch = self.to_record_batch();
        let right_batch = right_table.to_record_batch();
        let mut fields =
            Vec::with_capacity(left_batch.num_columns() + right_projection_columns.len());
        let mut arrays = Vec::with_capacity(fields.capacity());
        let gather = |array: &ArrayRef, indices: &UInt64Array| {
            arrow::compute::take(array.as_ref(), indices, None)
                .map_err(|e| Error::new(ErrorKind::InvalidOperation, format!("Table.join: {e}")))
        };

        for (i, field) in left_batch.schema_ref().fields().iter().enumerate() {
            // unmatched right rows make every left column nullable
            fields.push(field.as_ref().clone().with_nullable(true));
            arrays.push(gather(left_batch.column(i), &left_row_indices)?);
        }
        // ```python
        //         if name in self.column_names:
        //             column_names.append('%s2' % name)
        //         else:
        //             column_names.append(name)
        // ```
        for &i in &right_projection_columns {
            let field = right_batch.schema_ref().field(i);
            let name = if self.column_names_iter().any(|n| n == field.name()) {
                format!("{}2", field.name())
            } else {
                field.name().clone()
            };
            fields.push(Field::new(name, field.data_type().clone(), true));
            arrays.push(gather(right_batch.column(i), &right_row_indices)?);
        }

        let options = RecordBatchOptions::default().with_row_count(Some(joined_rows.len()));
        let batch =
            RecordBatch::try_new_with_options(Arc::new(Schema::new(fields)), arrays, &options)
                .map_err(|e| Error::new(ErrorKind::InvalidOperation, format!("Table.join: {e}")))?;

        // The row names of the joined table are the left row names of every joined row.
        let row_names = match (self.row_names_array(), full_outer) {
            (Some(row_names), false) => {
                let gathered = gather(&(Arc::clone(row_names) as ArrayRef), &left_row_indices)?;
                let gathered = gathered
                    .as_any()
                    .downcast_ref::<StringViewArray>()
                    .expect("row names")
                    .clone();
                Some(Arc::new(gathered))
            }
            _ => None,
        };

        Ok(AgateTable::new(Arc::new(batch), row_names))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::*;
    use arrow_array::{Int32Array, Int64Array, StringArray};

    fn rows_of(table: &AgateTable) -> Vec<String> {
        stringly_rows_of(table, "|")
    }

    /// | id | name |
    /// | 1  | a    |
    /// | 2  | b    |
    /// | 3  | c    |
    fn left_table() -> AgateTable {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new("name", DataType::Utf8, true),
        ]));
        let id: ArrayRef = Arc::new(Int32Array::from(vec![1, 2, 3]));
        let name: ArrayRef = Arc::new(StringArray::from(vec!["a", "b", "c"]));
        AgateTable::from_record_batch(Arc::new(
            RecordBatch::try_new(schema, vec![id, name]).unwrap(),
        ))
    }

    /// | id | score |
    /// | 2  | 20    |
    /// | 3  | 30    |
    /// | 3  | 31    |
    /// | 4  | 40    |
    fn right_table() -> AgateTable {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new("score", DataType::Int32, true),
        ]));
        let id: ArrayRef = Arc::new(Int32Array::from(vec![2, 3, 3, 4]));
        let score: ArrayRef = Arc::new(Int32Array::from(vec![20, 30, 31, 40]));
        AgateTable::from_record_batch(Arc::new(
            RecordBatch::try_new(schema, vec![id, score]).unwrap(),
        ))
    }

    #[test]
    fn sequential_join_matches_on_row_number() {
        let joined = left_table()
            .join(&right_table(), None, None, JoinType::LeftOuter, false, None)
            .unwrap();

        // no key columns to drop, so "id" collides with the left-hand one
        assert_eq!(joined.column_names(), vec!["id", "name", "id2", "score"]);
        // the 4th right row is dangling and left out
        assert_eq!(rows_of(&joined), vec!["1|a|2|20", "2|b|3|30", "3|c|3|31"]);
    }

    /// Join on the `fk` column of the left table and the `id` column of the right one --
    /// the key pair every test of the PR uses unless stated otherwise.
    fn join_on_fk(
        left: &AgateTable,
        right: &AgateTable,
        join_type: JoinType,
        require_match: bool,
    ) -> Result<AgateTable, Error> {
        left.join(
            right,
            Some(&Value::from("fk")),
            Some(&Value::from("id")),
            join_type,
            require_match,
            None,
        )
    }

    #[test]
    fn join_exposes_join_type_and_indices() {
        let left = main_table(
            &[1, 2],
            &["Sherlock Holmes", "Hercule Poirot"],
            &some(&[10, 20]),
        );
        let right = related_table(&some(&[10, 30]), &["The Sign of the Four", "Nemesis"]);
        let joined = join_on_fk(&left, &right, JoinType::FullOuter, false).unwrap();

        // left_indices  = [Some(0), Some(1), None]
        // right_indices = [Some(0), None,    Some(1)]
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|10|The Sign of the Four",
                "2|Hercule Poirot|20|None|None",
                "None|None|None|30|Nemesis",
            ]
        );
    }

    #[test]
    fn join_left_outer_left_probe_right_build() {
        let left = main_table(
            &[1, 2, 3, 4],
            &[
                "Sherlock Holmes",
                "John Watson",
                "Hercule Poirot",
                "Benoit Blanc",
            ],
            &some(&[10, 10, 20, 30]),
        );
        let right = related_table(
            &some(&[20, 10]),
            &["The Mysterious Affair at Styles", "The Sign of the Four"],
        );
        let joined = join_on_fk(&left, &right, JoinType::LeftOuter, false).unwrap();

        // [(0, 1), (1, 1), (2, 0), (3, None)]
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|The Sign of the Four",
                "2|John Watson|10|The Sign of the Four",
                "3|Hercule Poirot|20|The Mysterious Affair at Styles",
                "4|Benoit Blanc|30|None",
            ]
        );
    }

    #[test]
    fn join_left_outer_left_build_right_probe() {
        let left = main_table(
            &[1, 2, 3],
            &["Sherlock Holmes", "John Watson", "Benoit Blanc"],
            &some(&[10, 10, 30]),
        );
        let right = related_table(
            &some(&[10, 20, 40, 50]),
            &[
                "The Sign of the Four",
                "The Mysterious Affair at Styles",
                "Dresden Files",
                "Nemesis",
            ],
        );
        let joined = join_on_fk(&left, &right, JoinType::LeftOuter, false).unwrap();

        // [(0, 0), (1, 0), (2, None)]
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|The Sign of the Four",
                "2|John Watson|10|The Sign of the Four",
                "3|Benoit Blanc|30|None",
            ]
        );
    }

    #[test]
    fn join_inner() {
        let left = main_table(
            &[1, 2, 3, 4],
            &[
                "Sherlock Holmes",
                "John Watson",
                "Hercule Poirot",
                "Benoit Blanc",
            ],
            &some(&[10, 10, 20, 30]),
        );
        let right = related_table(
            &some(&[10, 20]),
            &["The Sign of the Four", "The Mysterious Affair at Styles"],
        );
        let joined = join_on_fk(&left, &right, JoinType::Inner, false).unwrap();

        // [(0, 0), (1, 0), (2, 1)]
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|The Sign of the Four",
                "2|John Watson|10|The Sign of the Four",
                "3|Hercule Poirot|20|The Mysterious Affair at Styles",
            ]
        );
    }

    #[test]
    fn join_full_outer() {
        let left = main_table(
            &[1, 2, 3, 4],
            &[
                "Sherlock Holmes",
                "John Watson",
                "Hercule Poirot",
                "Benoit Blanc",
            ],
            &some(&[10, 10, 20, 30]),
        );
        let right = related_table(
            &some(&[10, 20, 40]),
            &[
                "The Sign of the Four",
                "The Mysterious Affair at Styles",
                "Dresden Files",
            ],
        );
        let joined = join_on_fk(&left, &right, JoinType::FullOuter, false).unwrap();

        // [(0, 0), (1, 0), (2, 1), (3, None), (None, 2)]
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|10|The Sign of the Four",
                "2|John Watson|10|10|The Sign of the Four",
                "3|Hercule Poirot|20|20|The Mysterious Affair at Styles",
                "4|Benoit Blanc|30|None|None",
                "None|None|None|40|Dresden Files",
            ]
        );
    }

    #[test]
    fn join_full_outer_with_several_dangling_right_rows() {
        let left = main_table(
            &[1, 2, 3],
            &["Sherlock Holmes", "John Watson", "Benoit Blanc"],
            &some(&[10, 10, 30]),
        );
        let right = related_table(
            &some(&[10, 20, 40, 50]),
            &[
                "The Sign of the Four",
                "The Mysterious Affair at Styles",
                "Dresden Files",
                "Nemesis",
            ],
        );
        let joined = join_on_fk(&left, &right, JoinType::FullOuter, false).unwrap();

        // [(0, 0), (1, 0), (2, None), (None, 1), (None, 2), (None, 3)]
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|10|The Sign of the Four",
                "2|John Watson|10|10|The Sign of the Four",
                "3|Benoit Blanc|30|None|None",
                "None|None|None|20|The Mysterious Affair at Styles",
                "None|None|None|40|Dresden Files",
                "None|None|None|50|Nemesis",
            ]
        );
    }

    #[test]
    fn join_left_outer_with_null_join_key() {
        let left = main_table(
            &[1, 2, 3],
            &["Sherlock Holmes", "John Watson", "Benoit Blanc"],
            &[Some(10), None, Some(30)],
        );
        let right = related_table(
            &[Some(10), None],
            &["The Sign of the Four", "The Mysterious Affair at Styles"],
        );
        let joined = join_on_fk(&left, &right, JoinType::LeftOuter, false).unwrap();

        // [(0, 0), (1, 1), (2, None)] -- a null key matches a null key
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|The Sign of the Four",
                "2|John Watson|None|The Mysterious Affair at Styles",
                "3|Benoit Blanc|30|None",
            ]
        );
    }

    #[test]
    fn join_fan_out_left_outer_on_duplicate_right_key() {
        let left = main_table(
            &[1, 2],
            &["Sherlock Holmes", "Benoit Blanc"],
            &some(&[10, 20]),
        );
        let right = related_table(
            &some(&[10, 10, 30]),
            &[
                "The Sign of the Four",
                "The Hound of the Baskervilles",
                "The Mysterious Affair at Styles",
            ],
        );
        let joined = join_on_fk(&left, &right, JoinType::LeftOuter, false).unwrap();

        // [(0, 0), (0, 1), (1, None)]
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|The Sign of the Four",
                "1|Sherlock Holmes|10|The Hound of the Baskervilles",
                "2|Benoit Blanc|20|None",
            ]
        );
    }

    #[test]
    fn join_fan_out_inner_on_duplicate_right_key() {
        let left = main_table(
            &[1, 2],
            &["Sherlock Holmes", "Benoit Blanc"],
            &some(&[10, 20]),
        );
        let right = related_table(
            &some(&[10, 10, 30]),
            &[
                "The Sign of the Four",
                "The Hound of the Baskervilles",
                "The Mysterious Affair at Styles",
            ],
        );
        let joined = join_on_fk(&left, &right, JoinType::Inner, false).unwrap();

        // [(0, 0), (0, 1)]
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|The Sign of the Four",
                "1|Sherlock Holmes|10|The Hound of the Baskervilles",
            ]
        );
    }

    #[test]
    fn join_fan_out_full_outer_on_duplicate_right_key() {
        let left = main_table(
            &[1, 2],
            &["Sherlock Holmes", "Benoit Blanc"],
            &some(&[10, 20]),
        );
        let right = related_table(
            &some(&[10, 10, 30]),
            &[
                "The Sign of the Four",
                "The Hound of the Baskervilles",
                "The Mysterious Affair at Styles",
            ],
        );
        let joined = join_on_fk(&left, &right, JoinType::FullOuter, false).unwrap();

        // [(0, 0), (0, 1), (1, None), (None, 2)]
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|10|The Sign of the Four",
                "1|Sherlock Holmes|10|10|The Hound of the Baskervilles",
                "2|Benoit Blanc|20|None|None",
                "None|None|None|30|The Mysterious Affair at Styles",
            ]
        );
    }

    #[test]
    fn join_require_match_errors_on_miss() {
        let left = main_table(
            &[1, 2],
            &["Sherlock Holmes", "Harry Dresden"],
            &some(&[10, 20]),
        );
        let right = related_table(&some(&[10]), &["The Sign of the Four"]);

        for join_type in [JoinType::LeftOuter, JoinType::Inner, JoinType::FullOuter] {
            let err = join_on_fk(&left, &right, join_type, true).unwrap_err();
            // agate names the unmatched key in the message
            assert!(
                err.to_string()
                    .contains(r#"Left key "20" does not have a matching right key."#),
                "{err}"
            );
        }
    }

    #[test]
    fn join_require_match_succeeds_without_miss() {
        let left = main_table(
            &[1, 2],
            &["Sherlock Holmes", "Harry Dresden"],
            &some(&[10, 20]),
        );
        let right = related_table(&some(&[10, 20]), &["The Sign of the Four", "Twelve Months"]);

        for join_type in [JoinType::LeftOuter, JoinType::Inner] {
            let joined = join_on_fk(&left, &right, join_type, true).unwrap();
            assert_eq!(
                rows_of(&joined),
                vec![
                    "1|Sherlock Holmes|10|The Sign of the Four",
                    "2|Harry Dresden|20|Twelve Months",
                ]
            );
        }
    }

    #[test]
    fn join_full_outer_require_match_ignores_right_only_miss() {
        // require_match only cares about *left* rows lacking a match -- an unmatched
        // *right* row under full_outer must not trigger it.
        let left = main_table(&[1], &["Sherlock Holmes"], &some(&[10]));
        let right = related_table(&some(&[10, 20]), &["The Sign of the Four", "Twelve Months"]);
        let joined = join_on_fk(&left, &right, JoinType::FullOuter, true).unwrap();

        // [(0, 0), (None, 1)]
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|10|The Sign of the Four",
                "None|None|None|20|Twelve Months",
            ]
        );
    }

    /// The left key is `(fk, name)` and the right key is `(id, name)`.
    fn join_on_fk_and_name(
        left: &AgateTable,
        right: &AgateTable,
        join_type: JoinType,
        require_match: bool,
    ) -> Result<AgateTable, Error> {
        left.join(
            right,
            Some(&Value::from_iter(["fk", "name"])),
            Some(&Value::from_iter(["id", "name"])),
            join_type,
            require_match,
            None,
        )
    }

    #[test]
    fn join_multi_column_key() {
        let left = main_table(
            &[1, 2],
            &["Sherlock Holmes", "Hercule Poirot"],
            &some(&[10, 20]),
        );
        let right = related_table(
            &some(&[20, 10]),
            &["The Mysterious Affair at Styles", "Sherlock Holmes"],
        );

        // [(0, 1), (1, None)] -- only row 0 matches on both columns
        let joined = join_on_fk_and_name(&left, &right, JoinType::LeftOuter, false).unwrap();
        assert_eq!(
            rows_of(&joined),
            vec!["1|Sherlock Holmes|10", "2|Hercule Poirot|20"]
        );

        // [(0, 1)]
        let joined = join_on_fk_and_name(&left, &right, JoinType::Inner, false).unwrap();
        assert_eq!(rows_of(&joined), vec!["1|Sherlock Holmes|10"]);

        // [(0, 1), (1, None), (None, 0)]
        let joined = join_on_fk_and_name(&left, &right, JoinType::FullOuter, false).unwrap();
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|10|Sherlock Holmes",
                "2|Hercule Poirot|20|None|None",
                "None|None|None|20|The Mysterious Affair at Styles",
            ]
        );

        let err = join_on_fk_and_name(&left, &right, JoinType::LeftOuter, true).unwrap_err();
        assert!(
            err.to_string()
                .contains("does not have a matching right key"),
            "{err}"
        );
    }

    #[test]
    fn join_empty_right_table_left_outer_null_pads_every_left_row() {
        let left = main_table(
            &[1, 2],
            &["Sherlock Holmes", "Hercule Poirot"],
            &some(&[10, 20]),
        );
        let right = related_table(&[], &[]);
        let joined = join_on_fk(&left, &right, JoinType::LeftOuter, false).unwrap();

        // [(0, None), (1, None)]
        assert_eq!(
            rows_of(&joined),
            vec!["1|Sherlock Holmes|10|None", "2|Hercule Poirot|20|None"]
        );
    }

    #[test]
    fn join_empty_left_table_inner_produces_no_rows() {
        let left = main_table(&[], &[], &[]);
        let right = related_table(
            &some(&[20, 10]),
            &["The Mysterious Affair at Styles", "Sherlock Holmes"],
        );
        let joined = join_on_fk(&left, &right, JoinType::Inner, false).unwrap();

        assert_eq!(joined.num_rows(), 0);
        assert_eq!(rows_of(&joined), Vec::<String>::new());
    }

    #[test]
    fn join_columns_filter() {
        let left = main_table(
            &[1, 2, 3],
            &["Sherlock Holmes", "Benoit Blanc", "Bilbo Baggins"],
            &some(&[10, 30, 20]),
        );
        let right = genre_table(
            &[20, 10],
            &["The Lord of the Rings", "The Sign of the Four"],
            &["Fantasy", "Fiction"],
        );
        let joined = left
            .join(
                &right,
                Some(&Value::from("fk")),
                Some(&Value::from("id")),
                JoinType::LeftOuter,
                false,
                Some(&Value::from_iter(["genre"])),
            )
            .unwrap();

        assert_eq!(joined.column_names(), vec!["id", "name", "fk", "genre"]);
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|Fiction",
                "2|Benoit Blanc|30|None",
                "3|Bilbo Baggins|20|Fantasy",
            ]
        );
    }

    #[test]
    fn join_columns_filter_is_noop_under_full_outer() {
        let left = main_table(
            &[1, 2, 3],
            &["Sherlock Holmes", "Benoit Blanc", "Bilbo Baggins"],
            &some(&[10, 30, 20]),
        );
        let right = genre_table(
            &[20, 10],
            &["The Lord of the Rings", "The Sign of the Four"],
            &["Fantasy", "Fiction"],
        );
        let joined = left
            .join(
                &right,
                Some(&Value::from("fk")),
                Some(&Value::from("id")),
                JoinType::FullOuter,
                false,
                Some(&Value::from_iter(["genre"])),
            )
            .unwrap();

        assert_eq!(
            joined.column_names(),
            vec!["id", "name", "fk", "id2", "name2", "genre"]
        );
    }

    #[test]
    fn join_fan_out_repeats_the_left_values() {
        let left = main_table(&[1], &["Sherlock Holmes"], &some(&[10]));
        let right = related_table(
            &some(&[10, 10]),
            &["The Sign of the Four", "The Hound of the Baskervilles"],
        );
        let joined = join_on_fk(&left, &right, JoinType::LeftOuter, false).unwrap();

        assert_eq!(joined.column_names(), vec!["id", "name", "fk", "name2"]);
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|The Sign of the Four",
                "1|Sherlock Holmes|10|The Hound of the Baskervilles",
            ]
        );
    }

    #[test]
    fn join_composite_key_carries_non_key_right_column_with_correct_alignment() {
        let left = main_table(
            &[1, 2, 3],
            &["Sherlock Holmes", "Frodo Baggins", "Hercule Poirot"],
            &some(&[10, 30, 20]),
        );
        // | id | name | protagonist |
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, false),
                Field::new("protagonist", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(Int64Array::from(vec![20, 10, 30])),
                Arc::new(StringArray::from(vec![
                    "The Mysterious Affair at Styles",
                    "The Sign of the Four",
                    "The Hobbit",
                ])),
                Arc::new(StringArray::from(vec![
                    "Hercule Poirot",
                    "Sherlock Holmes",
                    "Bilbo Baggins",
                ])),
            ],
        )
        .unwrap();
        let right = AgateTable::from_record_batch(Arc::new(batch));

        // the key is (fk, name) on the left and (id, protagonist) on the right
        let joined = left
            .join(
                &right,
                Some(&Value::from_iter(["fk", "name"])),
                Some(&Value::from_iter(["id", "protagonist"])),
                JoinType::LeftOuter,
                false,
                None,
            )
            .unwrap();

        // the non-key right column is the only one carried over
        assert_eq!(joined.column_names(), vec!["id", "name", "fk", "name2"]);
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|The Sign of the Four",
                // (30, "Frodo Baggins") does not match (30, "Bilbo Baggins")
                "2|Frodo Baggins|30|None",
                "3|Hercule Poirot|20|The Mysterious Affair at Styles",
            ]
        );
    }

    #[test]
    fn columns_filter_does_not_exclude_a_listed_right_key() {
        let left = main_table(
            &[1, 2, 3],
            &["Sherlock Holmes", "Benoit Blanc", "Bilbo Baggins"],
            &some(&[10, 30, 20]),
        );
        let right = genre_table(
            &[20, 10],
            &["The Lord of the Rings", "The Sign of the Four"],
            &["Fantasy", "Fiction"],
        );
        let joined = left
            .join(
                &right,
                Some(&Value::from("fk")),
                Some(&Value::from("id")),
                JoinType::LeftOuter,
                false,
                Some(&Value::from_iter(["id", "genre"])),
            )
            .unwrap();

        // the PR asserts ["id", "name", "fk", "genre"] here
        assert_eq!(
            joined.column_names(),
            vec!["id", "name", "fk", "id2", "genre"]
        );
        assert_eq!(
            rows_of(&joined),
            vec![
                "1|Sherlock Holmes|10|10|Fiction",
                "2|Benoit Blanc|30|None|None",
                "3|Bilbo Baggins|20|20|Fantasy",
            ]
        );
    }

    #[test]
    fn a_doubly_colliding_right_name_loses_the_left_column() {
        // The single-pass suffix rule renames the right-hand "name" to "name2", which
        // collides again with the left-hand "name2". agate resolves that when the joined
        // table is created: it warns (DuplicateColumnWarning) and renames the second one
        // to "name2_2", so both columns survive as ("id", "name", "name2", "name2_2").
        // Here the duplicate "name2" is dropped when the joined batch is flattened, so
        // the *left* "name2" is the one that disappears.
        let left = three_column_table(
            ("id", &[1, 2]),
            ("name", &["a", "b"]),
            ("name2", &["a2", "b2"]),
        );
        let right = two_column_table(("id", &[1, 2]), ("name", &["x", "y"]));
        let joined = left
            .join(
                &right,
                Some(&Value::from("id")),
                None,
                JoinType::LeftOuter,
                false,
                Some(&Value::from_iter(["name"])),
            )
            .unwrap();

        // the PR asserts ["id", "name", "name2", "name2"] on the raw RecordBatch
        assert_eq!(joined.column_names(), vec!["id", "name", "name2"]);
        assert_eq!(rows_of(&joined), vec!["1|a|x", "2|b|y"]);
    }

    #[test]
    fn keys_of_different_arity_do_not_match() {
        let left = main_table(
            &[1, 2],
            &["Sherlock Holmes", "Hercule Poirot"],
            &some(&[10, 20]),
        );
        let right = related_table(
            &some(&[20, 10]),
            &["The Mysterious Affair at Styles", "The Sign of the Four"],
        );
        let joined = left
            .join(
                &right,
                // comparing a 2-tuple against a 1-tuple never matches
                Some(&Value::from_iter(["fk", "name"])),
                Some(&Value::from("id")),
                JoinType::LeftOuter,
                false,
                None,
            )
            .unwrap();

        assert_eq!(
            rows_of(&joined),
            vec!["1|Sherlock Holmes|10|None", "2|Hercule Poirot|20|None"]
        );
    }

    #[test]
    fn keys_of_different_types_do_not_match() {
        let left = main_table(
            &[1, 2],
            &["Sherlock Holmes", "Hercule Poirot"],
            &some(&[10, 20]),
        );
        let right = related_table(
            &some(&[10, 20]),
            &["The Sign of the Four", "The Mysterious Affair at Styles"],
        );
        let joined = left
            .join(
                &right,
                // "name" is Utf8 on the left, "id" is Int64 on the right
                Some(&Value::from("name")),
                Some(&Value::from("id")),
                JoinType::LeftOuter,
                false,
                None,
            )
            .unwrap();

        assert_eq!(
            rows_of(&joined),
            vec!["1|Sherlock Holmes|10|None", "2|Hercule Poirot|20|None"]
        );
    }
}
