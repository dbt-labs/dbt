use std::collections::HashSet;
use std::num::NonZero;
use std::sync::{Arc, OnceLock};

use arrow::compute::{LexicographicalComparator, SortColumn, SortOptions, TakeOptions};
use arrow::record_batch::RecordBatch;
use arrow_array::{Array, BooleanArray, StringViewArray, UInt64Array};
use arrow_schema::ArrowError;
use minijinja::{Error, ErrorKind, Value};

use crate::column::Column;
use crate::columns::*;
use crate::converters::ArrayConverter;
use crate::flat_record_batch::FlatRecordBatch;
use crate::grouper::Grouper;
use crate::row::Row;
use crate::rows::*;
use crate::vec_of_rows::VecOfRows;
use crate::{Tuple, adjusted_index};

/// Prepare the Arrow array to sort it like Python does sorting.
///
/// `None`, `bool`, numbers, `str`, `bytes`, `date`, `datetime` or `timedelta`,
/// are types that Python can compare. Everything else (maps, unions, lists,
/// structs, run-end encoded arrays) is rejected even when arrow could order it:
/// Python can't compare dicts, and mixed or nested values would raise `TypeError`
/// or order differently.
fn safe_to_sort(array: Arc<dyn Array>) -> Result<Arc<dyn Array>, ArrowError> {
    use arrow::compute::CastOptions;
    use arrow::compute::cast_with_options;
    use arrow_schema::DataType::*;

    let is_sortable_like_python = match array.data_type() {
        Null | Boolean => true,
        Int8 | Int16 | Int32 | Int64 | UInt8 | UInt16 | UInt32 | UInt64 => true,
        Float16 | Float32 | Float64 => true,
        Decimal32(_, _) | Decimal64(_, _) | Decimal128(_, _) | Decimal256(_, _) => true,
        Date32 | Date64 | Timestamp(_, _) => true,
        Time32(_) | Time64(_) | Duration(_) | Interval(_) => true,
        Utf8 | LargeUtf8 | Utf8View => true,
        Binary | LargeBinary | BinaryView | FixedSizeBinary(_) => true,
        Dictionary(_, value_type) => {
            // Dictionary-encoded arrays are not handled by the sort_to_indices()
            // function yet, so we must decompress them before sorting.
            let cast_options = CastOptions {
                safe: false,
                ..Default::default()
            };
            let values = cast_with_options(array.as_ref(), value_type.as_ref(), &cast_options)?;
            return safe_to_sort(values);
        }
        // XXX: consider supporting REE arrays in the future.
        RunEndEncoded(_, _) => false,
        List(_) | LargeList(_) | ListView(_) | LargeListView(_) | FixedSizeList(_, _) => false,
        Struct(_) | Union(_, _) | Map(_, _) => false,
    };

    if is_sortable_like_python {
        Ok(array)
    } else {
        Err(ArrowError::ComputeError(format!(
            "Sort not supported for data type {}",
            array.data_type()
        )))
    }
}

/// The indices selected by the Python slice `[start:stop:step]` of a sequence
/// of `len` elements.
///
/// This follows CPython's `slice.indices()`: missing bounds default to the
/// whole sequence in the direction of `step`, negative bounds count from the
/// end, and out-of-range bounds are clamped.
fn slice_indices(
    len: usize,
    start: Option<i64>,
    stop: Option<i64>,
    step: Option<NonZero<i64>>,
) -> UInt64Array {
    // SAFETY: 1 is non-zero
    let step = step.unwrap_or(unsafe { NonZero::new_unchecked(1) }).get();
    let len = len as i64;
    // Bounds of valid slice positions: [0, len] going forward and
    // [-1, len - 1] going backward (-1 is "before the first element").
    let (lower, upper) = if step > 0 { (0, len) } else { (-1, len - 1) };
    let clamp = |bound: Option<i64>, default: i64| match bound {
        None => default,
        Some(b) if b < 0 => b.saturating_add(len).max(lower),
        Some(b) => b.min(upper),
    };
    let (start, stop) = if step > 0 {
        (clamp(start, lower), clamp(stop, upper))
    } else {
        (clamp(start, upper), clamp(stop, lower))
    };

    let mut indices = Vec::new();
    let mut i = start;
    while (step > 0 && i < stop) || (step < 0 && i > stop) {
        indices.push(i as u64);
        i = match i.checked_add(step) {
            Some(next) => next,
            None => break,
        };
    }
    UInt64Array::from(indices)
}

/// Internal table representation.
///
/// An AgateTable can be internally represented as an Arrow RecordBatch and,
/// optionally, a vector of Jinja objects -- one iterable per row.
///
/// Both representations are immutable.
#[derive(Debug)]
pub(crate) struct TableRepr {
    /// Arrow representation of the table.
    pub(crate) flat: Arc<FlatRecordBatch>,
    /// Lazy-computed representation of the table as a vector of rows.
    row_table: OnceLock<Result<Arc<VecOfRows>, Arc<ArrowError>>>,
    /// Optional row names array (same length as number of rows).
    pub(crate) row_names: Option<Arc<StringViewArray>>,
}

impl TableRepr {
    pub(crate) fn new(
        flat: Arc<FlatRecordBatch>,
        row_table: Option<Arc<VecOfRows>>,
        row_names: Option<Arc<StringViewArray>>,
    ) -> Self {
        let row_table = match row_table {
            Some(vec_of_rows) => OnceLock::from(Ok(vec_of_rows)),
            None => OnceLock::new(),
        };
        Self {
            flat,
            row_table,
            row_names,
        }
    }

    /// Force the lazy initialization of table as a [VecOfRows].
    ///
    /// We try to delay the conversion from the Arrow-based [FlatRecordBatch] representation
    /// to [VecOfRows] until we actually need it. This means we can work with the Arrow-based
    /// representation for as long as possible, which is more efficient and structured.
    ///
    /// Reasons to call this function:
    /// - We don't want or don't have the time to implement the functionality against the
    ///   Arrow-based representation.
    /// - We must have the values as Jinja objects (e.g., for passing values to a Jinja
    ///   template)
    ///
    /// It's OK to call this function multiple times, it will only convert the table once.
    ///
    /// We *always* have the Arrow-based representation, so if you can implement Agate
    /// operations delegating to arrow-compute or some custom Arrow-based logic, you should
    /// do so.
    #[allow(dead_code)]
    pub fn force_row_table(&self) -> Result<&Arc<VecOfRows>, Error> {
        let res = self.row_table.get_or_init(|| {
            let vec_of_rows = VecOfRows::from_flat_record_batch(&self.flat)?;
            Ok(Arc::new(vec_of_rows))
        });
        match res {
            Ok(table) => Ok(table),
            Err(e) => {
                let e = Error::new(ErrorKind::InvalidOperation, e.to_string());
                Err(e)
            }
        }
    }

    /// Peek at the row table without forcing its initialization.
    pub fn peek_row_table(&self) -> Option<&Arc<VecOfRows>> {
        self.row_table.get().and_then(|res| res.as_ref().ok())
    }

    pub fn to_record_batch(&self) -> Arc<RecordBatch> {
        Arc::clone(self.flat.inner())
    }

    fn adjusted_column_index(&self, idx: isize) -> Option<usize> {
        adjusted_index(idx, self.num_columns())
    }

    fn adjusted_row_index(&self, idx: isize) -> Option<usize> {
        adjusted_index(idx, self.num_rows())
    }

    // Columns ----------------------------------------------------------------

    pub fn num_columns(&self) -> usize {
        self.flat.num_columns()
    }

    pub fn get_column(self: &Arc<Self>, idx: isize) -> Option<Column> {
        let idx = self.adjusted_column_index(idx)?;
        let col = Column::new(idx, Arc::clone(self));
        Some(col)
    }

    pub fn column_name(&self, idx: isize) -> Option<&String> {
        let idx = self.adjusted_column_index(idx)?;
        Some(self.flat.column_name(idx))
    }

    pub fn column_type(&self, idx: isize) -> Option<&crate::DataType> {
        let idx = self.adjusted_column_index(idx)?;
        Some(self.flat.column_type(idx))
    }

    pub fn columns(self: &Arc<Self>) -> Columns {
        Columns::new(Arc::clone(self))
    }

    pub fn column_types(&self) -> &[crate::DataType] {
        self.flat.column_types()
    }

    pub fn column_names(&self) -> impl Iterator<Item = &String> + '_ {
        self.flat.column_names()
    }

    /// Indices of the columns with the given names.
    ///
    /// If a name is not found, it is simply skipped.
    pub fn column_indices<'a>(&'a self, keys: &'a [String]) -> impl Iterator<Item = usize> + 'a {
        let fields = self.flat.schema_ref().as_ref().fields();
        let iter = keys
            .iter()
            .filter_map(|k| fields.iter().position(|f| f.name() == k));
        iter
    }

    /// Index of the column with the given name or at the given index.
    ///
    /// `None` if the name is not found or the index is out of range.
    fn column_index_of(&self, key: &Value) -> Result<Option<usize>, Error> {
        if let Some(name) = key.as_str() {
            let fields = self.flat.schema_ref().as_ref().fields();
            Ok(fields.iter().position(|f| f.name() == name))
        } else if let Some(idx) = key.as_i64() {
            Ok(adjusted_index(idx as isize, self.num_columns()))
        } else {
            Err(Error::new(
                ErrorKind::InvalidArgument,
                format!(
                    "column identifier must be a column name or a column index: {key} found instead"
                ),
            ))
        }
    }

    /// Indices of the columns identified by a Jinja value.
    ///
    /// `keys` may be a single column name, a single column index, or a sequence of
    /// either (the two can be mixed).
    ///
    /// If a key is not found, it is simply skipped,
    pub fn column_indices_of(&self, keys: &Value) -> Result<Vec<usize>, Error> {
        // Strings are iterable in Jinja (by character), so single identifiers have to
        // be handled before falling back to the sequence case below.
        if keys.as_str().is_some() || keys.as_i64().is_some() {
            return Ok(self.column_index_of(keys)?.into_iter().collect());
        }
        let iter = keys.try_iter().map_err(|e| {
            Error::new(
                ErrorKind::InvalidArgument,
                format!(
                    "column identifiers must be a column name, a column index, or a sequence of either: {e}"
                ),
            )
        })?;
        let mut indices = Vec::new();
        for key in iter {
            if let Some(idx) = self.column_index_of(&key)? {
                indices.push(idx);
            }
        }
        Ok(indices)
    }

    pub fn select<'a>(&'a self, indices: impl Iterator<Item = usize> + 'a) -> Arc<Self> {
        // get a new FlatRecordBatch with only the selected columns
        let flat = self.flat.select(indices);
        // row names remain the same when selecting columns
        let row_names = self.row_names.as_ref().map(Arc::clone);
        let repr = TableRepr::new(flat, None, row_names);
        Arc::new(repr)
    }

    pub fn single_column_table(&self, idx: isize) -> Option<Arc<TableRepr>> {
        let idx = self.adjusted_column_index(idx)?;
        let flat_with_single_column = self.flat.with_single_column(idx);
        let row_names = self.row_names.as_ref().map(Arc::clone);
        let repr = TableRepr::new(flat_with_single_column, None, row_names);
        Some(Arc::new(repr))
    }

    pub fn column_distinct(&self, col_idx: isize) -> Result<Option<Arc<Self>>, ArrowError> {
        self.single_column_table(col_idx)
            .map(|repr| repr.distinct(&[0]))
            .transpose()
    }

    pub fn column_without_nulls(&self, col_idx: isize) -> Result<Option<Arc<Self>>, ArrowError> {
        let Some(col) = self.single_column_table(col_idx) else {
            return Ok(None);
        };
        let arr = col.flat.inner().column(0);

        // logical_nulls() also covers Null, Dictionary, RunEndEncoded and Union arrays.
        let validity_filter = match arr.logical_nulls() {
            Some(nulls) if nulls.null_count() > 0 => {
                // Create a BooleanArray based on the logical nulls bitmap of the column.
                BooleanArray::new(nulls.into_inner(), None)
            }
            // Nothing to remove: reuse the column as is.
            _ => return Ok(Some(col)),
        };
        let filtered = arrow::compute::filter(arr.as_ref(), &validity_filter)?;

        let repr = {
            let schema = col.flat.inner().schema();
            let batch = RecordBatch::try_new(schema, vec![filtered])?;
            let flat = FlatRecordBatch::_from_flattened_record_batch(Arc::new(batch), None)?;
            let repr = TableRepr::new(Arc::new(flat), None, None);
            Arc::new(repr)
        };
        Ok(Some(repr))
    }

    /// The column values sorted according to `options`.
    pub fn column_sorted(
        &self,
        col_idx: isize,
        options: SortOptions,
    ) -> Result<Option<Arc<Self>>, ArrowError> {
        let Some(col) = self.single_column_table(col_idx) else {
            return Ok(None);
        };
        let indices = col.sort_to_indices(&[(0, options)])?;
        Ok(Some(Arc::new(col.select_rows(&indices, None)?)))
    }

    pub fn count_occurrences_of_value_in_column(&self, needle: &Value, col_idx: isize) -> usize {
        let Some(col_idx) = self.adjusted_column_index(col_idx) else {
            return 0;
        };
        (0..self.num_rows())
            .filter(|&r| {
                self.cell(r as isize, col_idx as isize)
                    .is_some_and(|v| v == *needle)
            })
            .count()
    }

    pub fn index_of_value_in_column(&self, needle: &Value, col_idx: isize) -> Option<usize> {
        let col_idx = self.adjusted_column_index(col_idx)?;
        (0..self.num_rows()).find(|&r| {
            self.cell(r as isize, col_idx as isize)
                .is_some_and(|v| v == *needle)
        })
    }

    pub(crate) fn with_renamed_columns(&self, renamed_columns: Vec<String>) -> Arc<Self> {
        debug_assert!(renamed_columns.len() == self.num_columns());
        let new_batch = self.flat.with_renamed_columns(&renamed_columns);
        let new_vec_of_rows = self.peek_row_table().map(Arc::clone);
        let row_names = self.row_names.as_ref().map(Arc::clone);
        let repr = TableRepr::new(new_batch, new_vec_of_rows, row_names);
        Arc::new(repr)
    }

    pub(crate) fn with_renamed_rows(&self, renamed_columns: Arc<StringViewArray>) -> Arc<Self> {
        debug_assert!(renamed_columns.len() == self.num_rows());
        let new_batch = Arc::clone(&self.flat);
        let new_vec_of_rows = self.peek_row_table().map(Arc::clone);
        let row_named = Some(renamed_columns);
        let repr = TableRepr::new(new_batch, new_vec_of_rows, row_named);
        Arc::new(repr)
    }

    pub(crate) fn grouper(&self, column_indices: &[usize]) -> Result<Grouper, ArrowError> {
        Grouper::from_record_batch_columns(self.flat.as_ref(), column_indices)
    }

    pub(crate) fn column_converter(&self, index: usize) -> &dyn ArrayConverter {
        self.flat.column_converter(index)
    }

    // Rows -------------------------------------------------------------------

    pub fn num_rows(&self) -> usize {
        self.flat.num_rows()
    }

    pub fn row_by_index(self: &Arc<Self>, idx: isize) -> Option<Value> {
        self.adjusted_row_index(idx).map(|i| {
            let row = Row::new(i, Arc::clone(self));
            Value::from_object(row)
        })
    }

    pub fn rows(self: &Arc<Self>) -> Rows {
        Rows::new(Arc::clone(self))
    }

    pub fn row_names(&self) -> Option<Tuple> {
        self.row_names.as_ref().map(|names| {
            let repr = RowNamesAsTuple::new(Arc::clone(names));
            let tuple = Tuple(Box::new(repr));
            tuple
        })
    }

    pub fn count_occurrences_of_row(&self, _needle: &Value) -> usize {
        todo!("count_occurrences_of_row")
    }

    pub fn index_of_row(&self, _needle: &Value) -> Option<usize> {
        todo!("index_of_row")
    }

    pub fn count_occurrences_of_value_in_row(
        self: &Arc<Self>,
        _needle: &Value,
        row_idx: isize,
    ) -> usize {
        let _row = self.row_by_index(row_idx).unwrap();
        todo!("count_occurrences_of_value_in_row")
    }

    pub fn index_of_value_in_row(
        self: &Arc<Self>,
        _needle: &Value,
        row_idx: isize,
    ) -> Option<usize> {
        let _row = self.row_by_index(row_idx).unwrap();
        todo!("index_of_value_in_row")
    }

    /// The rows selected by the Python slice `rows[start:stop:step]`.
    pub(crate) fn slice(
        &self,
        start: Option<i64>,
        stop: Option<i64>,
        step: Option<i64>,
    ) -> Result<TableRepr, ArrowError> {
        let step = step
            .map(|s| {
                NonZero::new(s).ok_or_else(|| {
                    ArrowError::InvalidArgumentError("slice step cannot be zero".to_owned())
                })
            })
            .transpose()?;
        let indices = slice_indices(self.num_rows(), start, stop, step);
        self.select_rows(&indices, None)
    }

    #[inline(never)]
    pub(crate) fn select_rows(
        &self,
        indices: &UInt64Array,
        take_options: Option<TakeOptions>,
    ) -> Result<TableRepr, ArrowError> {
        let row_names = self
            .row_names
            .as_ref()
            .map(|row_names| {
                let selected =
                    arrow::compute::take(row_names.as_ref(), indices, take_options.clone())?;
                let casted = selected
                    .as_ref()
                    .as_any()
                    .downcast_ref::<StringViewArray>()
                    .unwrap() // take preserves the input type
                    .clone(); // clone is cheap and necessary for the Arc
                Ok(Arc::new(casted)) as Result<Arc<StringViewArray>, ArrowError>
            })
            .transpose()?;

        let columns = self.flat.inner().columns();
        let mut new_columns = Vec::with_capacity(columns.len());
        match columns {
            [] => (),
            [first, rest @ ..] => {
                #[allow(clippy::needless_update)]
                let mut take_opts = take_options.unwrap_or_else(|| TakeOptions {
                    check_bounds: true,
                    ..Default::default()
                });
                let c0 = arrow::compute::take(first.as_ref(), indices, Some(take_opts.clone()))?;
                new_columns.push(c0);

                take_opts.check_bounds = false; // already checked for the first column
                for col in rest {
                    let c = arrow::compute::take(col.as_ref(), indices, Some(take_opts.clone()))?;
                    new_columns.push(c);
                }
            }
        }
        let schema = Arc::clone(self.flat.schema_ref());
        let row_count = new_columns.first().map_or(indices.len(), |c| c.len());
        // SAFETY: the schema doesn't change after selecting rows and data types of the
        // resulting arrays remain the same.
        let batch = unsafe { RecordBatch::new_unchecked(schema, new_columns, row_count) };
        // The filtered columns come from flat columns so they remain flat
        let flat = FlatRecordBatch::_from_flattened_record_batch(Arc::new(batch), None)?;
        let repr = TableRepr::new(Arc::new(flat), None, row_names);
        Ok(repr)
    }

    /// Row indices that sort the whole table by the `keys` columns.
    ///
    /// Rows are compared lexicographically: by the first key column and its
    /// [SortOptions] (ascending/descending, nulls first/last), ties broken by
    /// the next key, and so on. The sort is stable, like Python's `sorted()`,
    /// so rows that compare equal on every key keep their original order.
    ///
    /// Stability matters because rows can have different names in a table and
    /// a non-stable sorter could create non-determinism in the names of the
    /// sorted rows.
    #[inline(never)]
    pub(crate) fn sort_to_indices(
        &self,
        keys: &[(usize, SortOptions)],
    ) -> Result<Box<UInt64Array>, ArrowError> {
        let columns = self.flat.inner().columns();
        let sort_columns = keys
            .iter()
            .map(|&(idx, options)| {
                let values = safe_to_sort(Arc::clone(&columns[idx]))?;
                let sort_column = SortColumn {
                    values,
                    options: Some(options),
                };
                Ok(sort_column)
            })
            .collect::<Result<Vec<_>, ArrowError>>()?;

        let num_rows = self.num_rows();
        let indices = {
            let mut indices = (0..num_rows as u64).collect::<Vec<_>>();
            if !sort_columns.is_empty() {
                let comparator = LexicographicalComparator::try_new(&sort_columns)?;
                indices.sort_by(|&a, &b| comparator.compare(a as usize, b as usize));
            }
            UInt64Array::from(indices)
        };
        Ok(Box::new(indices))
    }

    #[inline(never)]
    pub(crate) fn distinct(&self, column_indices: &[usize]) -> Result<Arc<Self>, ArrowError> {
        // TODO: cast the values in `column` according to `key_type`, create a new
        // table with the casted column, and use that table to create the grouper
        let grouper = self.grouper(column_indices)?;

        // Builds a selection vector with the first `row_idx` of every group.
        // A "group" is the set of rows that are identical, so whenever the
        // grouper emits a `group_id` we've already seen, we know we shouldn't
        // emit that `row_idx` because it's not the index of a distinct row.
        let mut seen_groups = HashSet::new();
        let indices: Vec<u64> = grouper
            .iter()
            .enumerate()
            .filter_map(|(row_idx, group_id)| {
                if seen_groups.insert(group_id) {
                    Some(row_idx as u64)
                } else {
                    None
                }
            })
            .collect();
        let selection_vector = UInt64Array::new(indices.into(), None);

        let repr = self.select_rows(&selection_vector, None)?;
        Ok(Arc::new(repr))
    }

    // Cells ------------------------------------------------------------------

    pub fn cell(&self, row_idx: isize, col_idx: isize) -> Option<Value> {
        let row_idx = self.adjusted_row_index(row_idx)?;
        let col_idx = self.adjusted_column_index(col_idx)?;
        self.peek_row_table().map_or_else(
            || {
                let value = self.flat.column_converter(col_idx).to_value(row_idx);
                Some(value)
            },
            |vec_of_rows| {
                let row: &Value = vec_of_rows.rows_ref().get(row_idx)?;
                match row.get_item_by_index(col_idx) {
                    Ok(value) => Some(value),
                    Err(e) => {
                        debug_assert!(false, "Unexpected error: {e}");
                        None
                    }
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgateTable;
    use crate::test_fixtures::*;

    #[test]
    fn test_table_repr_single_column_and_select_rows_bounds() {
        let table = AgateTable::from_record_batch(simple_record_batch());

        let country = table.repr().single_column_table(1).unwrap();
        assert_eq!(country.num_columns(), 1);
        assert_eq!(country.num_rows(), 3);
        assert_eq!(country.column_name(0).unwrap(), "country");
        assert!(table.repr().single_column_table(99).is_none());

        let selected_indices = UInt64Array::new(vec![0, 2].into(), None);
        let selected = table.repr().select_rows(&selected_indices, None).unwrap();
        assert_eq!(selected.num_rows(), 2);
        assert_eq!(selected.cell(0, 1), Some(Value::from("Brazil")));
        assert_eq!(selected.cell(1, 1), Some(Value::from("Canada")));

        let out_of_bounds = UInt64Array::new(vec![99].into(), None);
        assert!(table.repr().select_rows(&out_of_bounds, None).is_err());
    }

    #[test]
    fn test_index_of_and_count_occurrences_in_column() {
        let table = main_table(&[1, 2, 2, 3], &["one", "two", "two", "three"], &[None; 4]);
        let repr = table.repr();

        assert_eq!(repr.index_of_value_in_column(&Value::from(2), 0), Some(1));
        assert_eq!(repr.index_of_value_in_column(&Value::from(99), 0), None);
        assert_eq!(
            repr.index_of_value_in_column(&Value::from("two"), -2),
            Some(1)
        );

        assert_eq!(
            repr.count_occurrences_of_value_in_column(&Value::from(2), 0),
            2
        );
        assert_eq!(
            repr.count_occurrences_of_value_in_column(&Value::from("two"), -2),
            2
        );
        assert_eq!(
            repr.count_occurrences_of_value_in_column(&Value::from("missing"), 1),
            0
        );
        assert_eq!(
            repr.count_occurrences_of_value_in_column(&Value::from(2), 99),
            0
        );
        assert_eq!(repr.index_of_value_in_column(&Value::from(2), 99), None);
    }
}
