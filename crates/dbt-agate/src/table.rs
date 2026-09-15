use crate::column::Column;
use crate::columns::*;
use crate::converters::ArrayConverter;
use crate::flat_record_batch::FlatRecordBatch;
use crate::grouper::Grouper;
use crate::join::JoinType;
use crate::print_table::TableDisplay;
use crate::row::Row;
use crate::rows::*;
use crate::table_set::{TableSet, TableSetRepr};
use crate::vec_of_rows::VecOfRows;
use crate::{Tuple, adjusted_index};

use arrow::array::StringViewBuilder;
use arrow::compute::TakeOptions;
use arrow::record_batch::RecordBatch;
use arrow_array::{Array, StringViewArray, UInt64Array};
use arrow_schema::{ArrowError, Schema};
use minijinja::arg_utils::ArgsIter;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Enumerator, Kwargs, Object, ValueMap, mutable_map::MutableMap};
use minijinja::{Error, ErrorKind, State, Value};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::hint::unreachable_unchecked;
use std::io;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

/// Internal table representation.
///
/// An AgateTable can be internally represented as an Arrow RecordBatch and,
/// optionally, a vector of Jinja objects -- one iterable per row.
///
/// Both representations are immutable.
#[derive(Debug)]
pub(crate) struct TableRepr {
    /// Arrow representation of the table.
    flat: Arc<FlatRecordBatch>,
    /// Lazy-computed representation of the table as a vector of rows.
    row_table: OnceLock<Result<Arc<VecOfRows>, Arc<ArrowError>>>,
    /// Optional row names array (same length as number of rows).
    row_names: Option<Arc<StringViewArray>>,
}

impl TableRepr {
    fn new(
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

    /// Return a single-column table with the distinct values in this column.
    pub fn column_distinct(&self, col_idx: isize) -> Arc<Self> {
        let _col = self.single_column_table(col_idx).unwrap();
        todo!("column_distinct")
    }

    pub fn column_without_nulls(&self, col_idx: isize) -> Arc<Self> {
        let _col = self.single_column_table(col_idx).unwrap();
        todo!("column_without_nulls")
    }

    pub fn column_sorted(&self, col_idx: isize) -> Arc<Self> {
        let _col = self.single_column_table(col_idx).unwrap();
        todo!("column_sorted")
    }

    pub fn column_without_nulls_sorted(&self, col_idx: isize) -> Arc<Self> {
        let _col = self.single_column_table(col_idx).unwrap();
        todo!("column_without_nulls_sorted")
    }

    pub fn count_occurrences_of_value_in_column(&self, _needle: &Value, col_idx: isize) -> usize {
        let _col = self.single_column_table(col_idx).unwrap();
        todo!("count_occurrences_of_value_in_column")
    }

    pub fn index_of_value_in_column(&self, _needle: &Value, col_idx: isize) -> Option<usize> {
        let _col = self.single_column_table(col_idx).unwrap();
        todo!("index_of_value_in_column")
    }

    fn with_renamed_columns(&self, renamed_columns: Vec<String>) -> Arc<Self> {
        debug_assert!(renamed_columns.len() == self.num_columns());
        let new_batch = self.flat.with_renamed_columns(&renamed_columns);
        let new_vec_of_rows = self.peek_row_table().map(Arc::clone);
        let row_names = self.row_names.as_ref().map(Arc::clone);
        let repr = TableRepr::new(new_batch, new_vec_of_rows, row_names);
        Arc::new(repr)
    }

    fn with_renamed_rows(&self, renamed_columns: Arc<StringViewArray>) -> Arc<Self> {
        debug_assert!(renamed_columns.len() == self.num_rows());
        let new_batch = Arc::clone(&self.flat);
        let new_vec_of_rows = self.peek_row_table().map(Arc::clone);
        let row_named = Some(renamed_columns);
        let repr = TableRepr::new(new_batch, new_vec_of_rows, row_named);
        Arc::new(repr)
    }

    fn grouper(&self, column_indices: &[usize]) -> Result<Grouper, ArrowError> {
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

    pub(crate) fn limit(&self, n: i64) -> Result<TableRepr, ArrowError> {
        if n < 0 {
            return Err(ArrowError::ComputeError(
                "Table.limit: count must be non-negative".to_string(),
            ));
        }
        let take = (n as u64).min(self.num_rows() as u64);
        let indices = UInt64Array::from_iter_values(0..take);
        self.select_rows(&indices, None)
    }

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

/// The AgateTable object.
///
/// Tables are immutable. Instead of modifying the data, various methods can be used to
/// create new, derivative tables.
///
/// Tables are not themselves iterable, but the columns of the table can be
/// accessed via [`AgateTable::columns`] and the rows via [`AgateTable::rows`]. Both
/// sequences can be accessed either by numeric index or by name. (In the case of
/// rows, row names are optional.)
#[derive(Debug, Clone)]
pub struct AgateTable {
    /// The internal representation of the table.
    repr: Arc<TableRepr>,
}

impl AgateTable {
    /// Create an [AgateTable] from an Arrow [RecordBatch].
    ///
    /// `row_names` is an optional array of strings with the same length as the number
    /// of rows in the `RecordBatch`.
    pub fn new(batch: Arc<RecordBatch>, row_names: Option<Arc<StringViewArray>>) -> Self {
        let flat = FlatRecordBatch::try_new(batch).unwrap();
        let repr = TableRepr::new(Arc::new(flat), None, row_names);
        Self::from_repr(Arc::new(repr))
    }

    /// Create an AgateTable from an Arrow RecordBatch.
    pub fn from_record_batch(batch: Arc<RecordBatch>) -> Self {
        Self::new(batch, None)
    }

    /// Create an [AgateTable] from an Arrow [RecordBatch] using a single row name for all rows.
    ///
    /// This is one of the possible ways to create row names for the table
    /// that comes from Python Agate:
    ///
    /// > row_names – Specifies unique names for each row. This parameter is optional.
    /// > If specified it may be 1) the name of a single column that contains a unique
    /// > identifier for each row, 2) a key function that takes a Row and returns a
    /// > unique identifier or 3) a sequence of unique identifiers of the same length
    /// > as the sequence of rows. The uniqueness of resulting identifiers is not
    /// > validated, so be certain the values you provide are truly unique.
    pub fn new_with_single_row_name(batch: Arc<RecordBatch>, row_name: &str) -> Self {
        let num_rows = batch.num_rows();

        // We can buid the StringView array very efficiently by having all values
        // point to the same buffer that only has to contain the row_name.
        let row_names = {
            let mut builder = StringViewBuilder::with_capacity(num_rows)
                .with_fixed_block_size(row_name.len() as u32);
            let block = builder.append_block(row_name.as_bytes().into());
            for _ in 0..num_rows {
                // SAFETY: 0 and row_name.len() are valid start and end for the block
                unsafe {
                    builder.append_view_unchecked(block, 0, row_name.len() as u32);
                }
            }
            Arc::new(builder.finish())
        };

        Self::new(batch, Some(row_names))
    }

    pub(crate) fn from_repr(repr: Arc<TableRepr>) -> Self {
        Self { repr }
    }

    pub fn to_value(&self) -> Value {
        Value::from_object(Self::from_repr(Arc::clone(&self.repr)))
    }

    pub fn into_value(self) -> Value {
        Value::from_object(self)
    }

    /// Returns the original Arrow [RecordBatch] used to create this Agate table.
    ///
    /// Some Agate operations like [TableRepr::single_column_table] may create new tables
    /// that do not have to go through the flattening process, so this function will simply
    /// return the flat [RecordBatch] in those cases.
    pub fn original_record_batch(&self) -> Arc<RecordBatch> {
        match self.repr.flat.original() {
            Some(original) => Arc::clone(original),
            None => self.repr.to_record_batch(),
        }
    }

    /// Returns the underlying Arrow [RecordBatch] backing this Agate table.
    ///
    /// This will return the [RecordBatch] produced at construction time after
    /// the flattening process of nested columns (Structs, Lists, etc). For the
    /// original, unflattened [RecordBatch], use [AgateTable::original_record_batch].
    pub fn to_record_batch(&self) -> Arc<RecordBatch> {
        self.repr.to_record_batch()
    }

    /// Get the internal representation of the table.
    pub fn cell(&self, row_idx: isize, col_idx: isize) -> Option<Value> {
        self.repr.cell(row_idx, col_idx)
    }

    // Columns ----------------------------------------------------------------

    /// Get the number of columns.
    pub fn num_columns(&self) -> usize {
        self.repr.num_columns()
    }

    /// Get the columns.
    pub fn columns(&self) -> Columns {
        self.repr.columns()
    }

    /// Get a single column name.
    pub fn column_name(&self, idx: isize) -> Option<&String> {
        self.repr.column_name(idx)
    }

    /// Get the column names as a zero-copy iterator.
    pub fn column_names_iter(&self) -> impl Iterator<Item = &String> + '_ {
        self.repr.column_names()
    }

    pub(crate) fn column_types_as_tuple(&self) -> ColumnTypesAsTuple {
        ColumnTypesAsTuple::of_table(&self.repr)
    }

    pub(crate) fn column_names_as_tuple(&self) -> ColumnNamesAsTuple {
        ColumnNamesAsTuple::of_table(&self.repr)
    }

    /// Get the column types as a slice.
    pub fn column_types(&self) -> &[crate::DataType] {
        self.repr.column_types()
    }

    /// Get the column names as a newly allocated [Vec<String>].
    pub fn column_names(&self) -> Vec<String> {
        self.column_names_iter().map(|s| s.to_string()).collect()
    }

    /// Indices of the columns identified by a Jinja value.
    ///
    /// See [TableRepr::column_indices_of].
    pub fn column_indices_of(&self, keys: &Value) -> Result<Vec<usize>, Error> {
        self.repr.column_indices_of(keys)
    }

    /// Create a new table with only the specified columns.
    pub fn select(&self, keys: &[String]) -> AgateTable {
        let indices = self.repr.column_indices(keys);
        let repr = self.repr.select(indices);
        AgateTable::from_repr(repr)
    }

    /// Return a new table containing the first `n` rows.
    pub fn limit(&self, n: i64) -> Result<AgateTable, ArrowError> {
        let repr = self.repr.limit(n)?;
        Ok(AgateTable::from_repr(Arc::new(repr)))
    }

    pub fn grouper(&self, column_names: &[String]) -> Result<Grouper, ArrowError> {
        let indices = self
            .repr
            .column_indices(column_names)
            .collect::<Vec<usize>>();
        self.repr.grouper(indices.as_slice())
    }

    // Rows -------------------------------------------------------------------

    /// Get the number of rows.
    pub fn num_rows(&self) -> usize {
        self.repr.num_rows()
    }

    /// Get the rows as Jinja value.
    pub fn rows(&self) -> Rows {
        self.repr.rows()
    }

    /// Get the row names.
    pub fn row_names(&self) -> Option<Tuple> {
        self.repr.row_names()
    }

    /// Get the row names as the Arrow array backing them.
    pub(crate) fn row_names_array(&self) -> Option<&Arc<StringViewArray>> {
        self.repr.row_names.as_ref()
    }

    // Rest of API ------------------------------------------------------------

    pub fn print_table(
        &self,
        max_rows: usize,
        max_columns: usize,
        max_column_width: usize,
        output: Option<&mut dyn io::Write>,
    ) -> Result<(), Error> {
        crate::print_table::print_table(self, max_rows, max_columns, max_column_width, output)
            .map_err(|e| {
                Error::new(
                    ErrorKind::InvalidOperation,
                    format!("Table.print_table: I/O error: {e}"),
                )
            })
    }

    pub fn print_table_to_string(
        &self,
        max_rows: usize,
        max_columns: usize,
        max_column_width: usize,
    ) -> Result<String, Error> {
        let mut output = Vec::new();
        self.print_table(max_rows, max_columns, max_column_width, Some(&mut output))?;
        // SAFETY: print_table() only writes valid UTF-8, it's not necessary to validate again
        let s = unsafe { String::from_utf8_unchecked(output) };
        Ok(s)
    }

    pub fn display<'a>(&'a self) -> TableDisplay<'a> {
        TableDisplay::new(self)
    }

    /// Internal `rename` implementation that handles dynamic minijinja values.
    fn rename_iternal(
        &self,
        column_names: Option<&Value>, // array or map
        row_names: Option<&Value>,    // array or map
        slug_columns: bool,
        slug_rows: bool,
        _kwargs: &Kwargs,
    ) -> Result<AgateTable, Error> {
        // Renaming of columns
        let renamed_columns = column_names
            .map(|v| {
                let old = self.column_names();
                macro_rules! rename_columns_by_map {
                    ($map:expr) => {{
                        let mut renamed = old.clone();
                        for (key, value) in $map {
                            for (i, col) in old.iter().enumerate() {
                                if key.as_str().is_some_and(|k| k == col) {
                                    renamed[i] = value.to_string();
                                }
                            }
                        }
                        renamed
                    }};
                }
                if let Some(map) = v.downcast_object_ref::<ValueMap>() {
                    Ok(rename_columns_by_map!(map))
                } else if let Some(map) = v.downcast_object_ref::<MutableMap>() {
                    let map: ValueMap = map.clone().into();
                    Ok(rename_columns_by_map!(map))
                } else {
                    // Try to treat it as a generic iterable
                    let iter = match v.try_iter() {
                        Ok(iter) => iter,
                        Err(_) => {
                            return Err(Error::new(
                                ErrorKind::InvalidArgument,
                                "Table.rename: column_names must be a map or an array",
                            ));
                        }
                    };
                    let mut renamed = old;
                    for (i, value) in iter.enumerate() {
                        if i >= renamed.len() {
                            break;
                        }
                        if let Some(s) = value.as_str() {
                            renamed[i] = s.to_string();
                        } else {
                            return Err(Error::new(
                                ErrorKind::InvalidArgument,
                                format!(
                                    "Table.rename: column_names array must contain only strings, found: {}",
                                    value
                                ),
                            ));
                        }
                    }
                    Ok(renamed)
                }
            })
            .transpose()?;

        // Renaming of rows
        let old_row_name = |i| -> Option<&str> {
            self.repr.row_names.as_ref().and_then(|names| {
                if names.as_ref().is_valid(i) {
                    Some(names.value(i))
                } else {
                    None
                }
            })
        };
        let renamed_rows = row_names
            .map(|v| {
                let mut renamed = StringViewBuilder::with_capacity(self.num_rows());
                macro_rules! rename_rows_by_map {
                    ($map:expr) => {{
                        for i in 0..self.num_rows() {
                            if let Some(old_name) = old_row_name(i) {
                                let old_name_value = Value::from(old_name);
                                if let Some(new_name_value) = $map.get(&old_name_value) {
                                    // we append a NULL if the value is not a byte/string
                                    renamed.append_option(new_name_value.as_str());
                                } else {
                                    renamed.append_value(old_name);
                                }
                            } else {
                                renamed.append_null();
                            }
                        }
                        Arc::new(renamed.finish())
                    }};
                }
                if let Some(map) = v.downcast_object_ref::<ValueMap>() {
                    Ok(rename_rows_by_map!(map))
                } else if let Some(map) = v.downcast_object_ref::<MutableMap>() {
                    Ok(rename_rows_by_map!(map))
                } else {
                    // Try to treat it as a generic iterable
                    let iter = match v.try_iter() {
                        Ok(iter) => iter,
                        Err(_) => {
                            return Err(Error::new(
                                ErrorKind::InvalidArgument,
                                "Table.rename: row_names must be a map or an array",
                            ));
                        }
                    };

                    // Collect the iterator values
                    let values: Vec<_> = iter.collect();

                    for i in 0..self.num_rows() {
                        if let Some(value) = values.get(i) {
                            if let Some(s) = value.as_str() {
                                renamed.append_value(s);
                            } else {
                                return Err(Error::new(
                                    ErrorKind::InvalidArgument,
                                    format!(
                                        "Table.rename: row_names array must contain only strings, found: {}",
                                        value
                                    ),
                                ));
                            }
                        } else {
                            renamed.append_option(old_row_name(i));
                        }
                    }
                    Ok(Arc::new(renamed.finish()))
                }
            })
            .transpose()?;

        self.rename(renamed_columns, renamed_rows, slug_columns, slug_rows)
    }

    /// Rename columns and/or rows.
    ///
    /// PRECONDITION:
    /// - if `renamed_columns` is `Some`, its length must be equal to
    ///   the number of columns in the table.
    /// - if `renamed_rows` is `Some`, its length must be equal to
    ///   the number of rows in the table.
    pub fn rename(
        &self,
        renamed_columns: Option<Vec<String>>,
        renamed_rows: Option<Arc<StringViewArray>>,
        slug_columns: bool,
        slug_rows: bool,
    ) -> Result<AgateTable, Error> {
        if let Some(ref columns) = renamed_columns {
            if columns.len() != self.num_columns() {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    format!(
                        "Table.rename: renamed_columns length ({}) does not match number of columns ({})",
                        columns.len(),
                        self.num_columns()
                    ),
                ));
            }
        }
        if let Some(ref rows) = renamed_rows {
            if rows.len() != self.num_rows() {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    format!(
                        "Table.rename: renamed_rows length ({}) does not match number of rows ({})",
                        rows.len(),
                        self.num_rows()
                    ),
                ));
            }
        }

        if slug_columns || slug_rows {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "Table.rename: slugging columns or rows is not implemented yet",
            ));
        }

        let repr = if let Some(renamed_columns) = renamed_columns {
            self.repr.with_renamed_columns(renamed_columns)
        } else {
            Arc::clone(&self.repr)
        };
        let repr = if let Some(renamed_rows) = renamed_rows {
            repr.with_renamed_rows(renamed_rows)
        } else {
            repr
        };

        Ok(AgateTable::from_repr(repr))
    }

    fn group_by_key(
        &self,
        key: &str,
        key_name: &str,
        key_type: Option<crate::DataType>,
    ) -> Result<TableSet, Error> {
        let column = self
            .column_names_iter()
            .position(|n| n == key)
            .map(|idx| Column::new(idx, Arc::clone(&self.repr)));

        let key_type = key_type.or_else(|| column.as_ref().and_then(|c| c.data_type().cloned()));

        // TODO: cast the values in `column` according to `key_type`, create a new
        // table with the casted column, and use that table to create the grouper
        let grouper = self.grouper(&[key.to_string()]).map_err(|e| {
            Error::new(
                ErrorKind::InvalidOperation,
                format!("Table.group_by_key: error creating grouper for key '{key}': {e}"),
            )
        })?;

        // Each vec contains the row indices for each group.
        let mut groups: Vec<Vec<u64>> = Vec::new();
        for (row_idx, group_id) in grouper.iter().enumerate() {
            match group_id.cmp(&groups.len()) {
                Ordering::Less => groups[group_id].push(row_idx as u64),
                Ordering::Equal => groups.push(vec![row_idx as u64]),
                Ordering::Greater => {
                    // SAFETY: new group ids are always created with increments of 1, so
                    // we either see a new group id equal to the current length of groups,
                    // or an existing group in this loop
                    unsafe { unreachable_unchecked() }
                }
            }
        }
        // The key values for a group are the same, so we can just pick the
        // first row index for each group to address the value of group key.
        let mut key_index_per_group = Vec::with_capacity(groups.len());
        for row_indices in groups.iter() {
            match row_indices.first() {
                Some(&i) => key_index_per_group.push(i),
                None => {
                    // SAFETY: the loop above guarantees that each group has at least one index
                    unsafe { unreachable_unchecked() }
                }
            }
        }
        let keys = match column {
            Some(col) => col.select_values(&key_index_per_group),
            None => vec![Value::from(()); groups.len()],
        };

        #[allow(clippy::needless_update)]
        let tables = groups
            .into_iter()
            .map(|indices| {
                // Move the Vec into an Arrow UInt64Array so we can use the arrow::compute functions
                let indices = UInt64Array::new(indices.into(), None);
                let table = self
                    .repr
                    .select_rows(
                        &indices,
                        Some(TakeOptions {
                            check_bounds: false, // groups only contain valid indices
                            ..Default::default()
                        }),
                    )
                    .map(|repr| {
                        let table = AgateTable::from_repr(Arc::new(repr));
                        Arc::new(table)
                    })?;
                Ok(table) as Result<Arc<AgateTable>, ArrowError>
            })
            .collect::<Result<Vec<Arc<AgateTable>>, ArrowError>>()
            .map_err(|e| {
                Error::new(
                    ErrorKind::InvalidOperation,
                    format!("Table.group_by_key: error selecting table rows: {e}"),
                )
            })?;

        let key_name = Some(key_name.to_string());
        let is_fork = true; // skip validations
        let repr = TableSetRepr::try_new(tables, keys, key_name, key_type, is_fork)?;
        Ok(TableSet::from_repr(repr))
    }

    fn distinct(&self, key: Option<Vec<String>>) -> Result<AgateTable, Error> {
        let column_names = match key {
            Some(keys) => self
                .column_names()
                .iter()
                .filter_map(|key| {
                    if keys.contains(key) {
                        Some(key.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            None => self.column_names(),
        };
        // TODO: cast the values in `column` according to `key_type`, create a new
        // table with the casted column, and use that table to create the grouper
        let grouper = self.grouper(&column_names).map_err(|e| {
            Error::new(
                ErrorKind::InvalidOperation,
                format!("Table.distinct: error creating grouper: {e}"),
            )
        })?;

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

        let repr = self
            .repr
            .select_rows(&selection_vector, None)
            .map_err(|e| {
                Error::new(
                    ErrorKind::InvalidOperation,
                    format!("Table.distinct: error selecting rows: {e}"),
                )
            })?;
        Ok(AgateTable::from_repr(repr.into()))
    }
}

impl Default for AgateTable {
    fn default() -> Self {
        let batch = RecordBatch::new_empty(Arc::new(Schema::empty()));
        Self::from_record_batch(Arc::new(batch))
    }
}

// TODO(felipecrv): implement the AgateTable Python API
// https://github.com/wireservice/agate/blob/master/agate/table/__init__.py#L34
impl Object for AgateTable {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        // TODO(venka): update state to be aware of phase so we don't duplicate functions for each
        // phase with minor differences
        // This is to implement 'for row in table' enumeration
        if let Some(idx) = key.as_i64() {
            return self.repr.row_by_index(idx as isize);
        }
        match key.as_str()? {
            "columns" => {
                let columns = self.columns();
                Some(Value::from_object(columns))
            }
            "column_types" => {
                let tuple = ColumnTypesAsTuple::of_table(&self.repr).into_tuple();
                Some(Value::from_object(tuple))
            }
            "column_names" => {
                let tuple = ColumnNamesAsTuple::of_table(&self.repr).into_tuple();
                Some(Value::from_object(tuple))
            }
            "top_level_column_names" => {
                Some(Value::from_iter(self.repr.flat.top_level_column_names()))
            }
            "rows" => {
                let rows = self.rows();
                Some(Value::from_object(rows))
            }
            "row_names" => {
                let names = self.row_names()?;
                Some(Value::from_object(names))
            }
            // TODO(venkaa28, felipecrv): return NoOp only at Parsetime
            _ => Some(Value::UNDEFINED),
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Seq(self.num_rows())
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        match name {
            // TODO: print_csv
            // TODO: print_json
            "print_table" => {
                // Parse arguments or use defaults matching Python implementation:
                //
                //     def print_table(self, max_rows=20, max_columns=6,
                //         output=sys.stdout, max_column_width=20, locale=None,
                //         max_precision=3):
                //
                // TODO: implement output, locale and max_precision
                let iter = ArgsIter::new("Table.print_table", &[], args);
                let max_rows = iter.next_kwarg::<Option<i64>>("max_rows")?.unwrap_or(20) as usize;
                let max_columns =
                    iter.next_kwarg::<Option<i64>>("max_columns")?.unwrap_or(6) as usize;
                let _output = iter.next_kwarg::<Option<&Value>>("output")?;
                let max_column_width = iter
                    .next_kwarg::<Option<i64>>("max_column_width")?
                    .unwrap_or(20) as usize;
                let _locale = iter.next_kwarg::<Option<&Value>>("locale")?;
                let _max_precision = iter.next_kwarg::<Option<&Value>>("max_precision")?;
                iter.finish()?;

                let s = self.print_table_to_string(max_rows, max_columns, max_column_width)?;
                Ok(Value::from(s))
            }
            "select" => {
                // ```python
                // def select(self, key):
                //     """
                //     Create a new table with only the specified columns.
                //
                //     :param key:
                //         Either the name of a single column to include or a sequence of such
                //         names.
                //     :returns:
                //         A new :class:`.Table`.
                //     """
                // ```
                let iter = ArgsIter::new("Table.select", &["key"], args);
                let key = iter.next_arg::<&Value>()?;
                iter.finish()?;

                let keys = if let Some(single_key) = key.as_str() {
                    Vec::from([single_key.to_string()])
                } else {
                    let iter = match key.try_iter() {
                        Ok(iter) => iter,
                        Err(e) => {
                            return Err(Error::new(
                                ErrorKind::InvalidArgument,
                                format!(
                                    "Table.select: key must be a string or an array of strings: {e}"
                                ),
                            ));
                        }
                    };
                    let mut keys = Vec::new();
                    for v in iter {
                        if let Some(s) = v.as_str() {
                            keys.push(s.to_string());
                        } else {
                            return Err(Error::new(
                                ErrorKind::InvalidArgument,
                                format!(
                                    "Table.select: key must be a string or an array of strings: {v} found instead"
                                ),
                            ));
                        }
                    }
                    keys
                };
                let table = self.select(keys.as_slice());
                Ok(Value::from_object(table))
            }
            "rename" => {
                //     def rename(column_names=None, row_names=None,
                //                slug_columns=False, slug_rows=False,
                //                **kwargs)
                //
                //     column_names: array | dict | None
                //     row_names:    array | dict | None
                //     slug_columns: bool
                //     slug_rows:    bool
                let iter = ArgsIter::new("Table.rename", &[], args);
                let column_names = iter.next_kwarg::<Option<&Value>>("column_names")?;
                let row_names = iter.next_kwarg::<Option<&Value>>("row_names")?;
                let slug_columns = iter
                    .next_kwarg::<Option<bool>>("slug_columns")?
                    .unwrap_or(false);
                let slug_rows = iter
                    .next_kwarg::<Option<bool>>("slug_rows")?
                    .unwrap_or(false);
                let kwargs = iter.trailing_kwargs()?;

                let table = self.as_ref().rename_iternal(
                    column_names,
                    row_names,
                    slug_columns,
                    slug_rows,
                    kwargs,
                )?;
                Ok(Value::from_object(table))
            }
            // ```python
            // def group_by(self, key, key_name=None, key_type=None):
            //     """
            //     Create a :class:`.TableSet` with a table for each unique key.
            //
            //     Note that group names will always be coerced to a string, regardless of the
            //     format of the input column.
            //
            //     :param key:
            //         Either the name of a column from the this table to group by, or a
            //         :class:`function` that takes a row and returns a value to group by.
            //     :param key_name:
            //         A name that describes the grouped properties. Defaults to the
            //         column name that was grouped on or "group" if grouping with a key
            //         function. See :class:`.TableSet` for more.
            //     :param key_type:
            //         An instance of any subclass of :class:`.DataType`. If not provided
            //         it will default to a :class`.Text`.
            //     :returns:
            //         A :class:`.TableSet` mapping where the keys are unique values from
            //         the :code:`key` and the values are new :class:`.Table` instances
            //         containing the grouped rows.
            //     """
            // ```
            "group_by" => {
                let iter = ArgsIter::new("Table.group_by", &["key"], args);
                let key = iter.next_arg::<&Value>()?;
                let key_name = iter.next_kwarg::<Option<&Value>>("key_name")?;
                let key_type = iter.next_kwarg::<Option<&Value>>("key_type")?;
                iter.finish()?;

                let key = match key.as_str() {
                    Some(s) => s,
                    None => unimplemented!("group_by with function key"),
                };
                let key_name = match key_name {
                    Some(v) => match v.as_str() {
                        Some(s) => s,
                        None => unimplemented!("group_by with non-string key_name"),
                    },
                    None => "group",
                };
                let key_type = match key_type {
                    Some(ty) => match ty.downcast_object_ref::<crate::DataType>() {
                        Some(dt) => Some(dt.clone()),
                        None => {
                            // TODO: support DataType class instances
                            unimplemented!("group_by with non-string key_type")
                        }
                    },
                    None => None,
                };
                let table_set = self
                    .as_ref()
                    .group_by_key(key, key_name, key_type)
                    .map_err(|e| {
                        Error::new(ErrorKind::InvalidOperation, format!("Table.group_by: {e}"))
                    })?;
                Ok(Value::from_object(table_set))
            }
            // ```python
            // def join(self, right_table, left_key=None, right_key=None, inner=False,
            //         full_outer=False, require_match=False, columns=None):
            //     """
            //     Create a new table by joining two table's on common values. This method
            //     implements most varieties of SQL join, in addition to some unique features.
            //
            //     If :code:`left_key` and :code:`right_key` are both :code:`None` then this
            //     method will perform a "sequential join", which is to say it will join on row
            //     number. The :code:`inner` and :code:`full_outer` arguments will determine
            //     whether dangling left-hand and right-hand rows are included, respectively.
            //
            //     If :code:`left_key` is specified, then a "left outer join" will be
            //     performed. This will combine columns from the :code:`right_table` anywhere
            //     that :code:`left_key` and :code:`right_key` are equal. Unmatched rows from
            //     the left table will be included with the right-hand columns set to
            //     :code:`None`.
            //
            //     If :code:`inner` is :code:`True` then an "inner join" will be performed.
            //     Unmatched rows from either table will be left out.
            //
            //     If :code:`full_outer` is :code:`True` then a "full outer join" will be
            //     performed. Unmatched rows from both tables will be included, with the
            //     columns in the other table set to :code:`None`.
            //
            //     In all cases, if :code:`right_key` is :code:`None` then it :code:`left_key`
            //     will be used for both tables.
            //
            //     If :code:`left_key` and :code:`right_key` are column names, the right-hand
            //     identifier column will not be included in the output table.
            //
            //     If :code:`require_match` is :code:`True` unmatched rows will raise an
            //     exception. This is like an "inner join" except any row that doesn't have a
            //     match will raise an exception instead of being dropped. This is useful for
            //     enforcing expectations about datasets that should match.
            //
            //     Column names from the right table which also exist in this table will
            //     be suffixed "2" in the new table.
            //
            //     A subset of columns from the right-hand table can be included in the joined
            //     table using the :code:`columns` argument.
            //
            //     :param right_table:
            //         The "right" table to join to.
            //     :param left_key:
            //         Either the name of a column from the this table to join on, the index
            //         of a column, a sequence of such column identifiers, a
            //         :class:`function` that takes a row and returns a value to join on, or
            //         :code:`None` in which case the tables will be joined on row number.
            //     :param right_key:
            //         Either the name of a column from :code:table` to join on, the index of
            //         a column, a sequence of such column identifiers, or a :class:`function`
            //         that takes a ow and returns a value to join on. If :code:`None` then
            //         :code:`left_key` will be used for both. If :code:`left_key` is
            //         :code:`None` then this value is ignored.
            //     :param inner:
            //         Perform a SQL-style "inner join" instead of a left outer join. Rows
            //         which have no match for :code:`left_key` will not be included in
            //         the output table.
            //     :param full_outer:
            //         Perform a SQL-style "full outer" join rather than a left or a right.
            //         May not be used in combination with :code:`inner`.
            //     :param require_match:
            //         If true, an exception will be raised if there is a left_key with no
            //         matching right_key.
            //     :param columns:
            //         A sequence of column names from :code:`right_table` to include in
            //         the final output table. Defaults to all columns not in
            //         :code:`right_key`. Ignored when :code:`full_outer` is :code:`True`.
            //     :returns:
            //         A new :class:`.Table`.
            //     """
            // ```
            "join" => {
                let iter = ArgsIter::new("Table.join", &["right_table"], args);
                let right_table = iter.next_arg::<&Value>()?;
                let left_key = iter.next_kwarg::<Option<&Value>>("left_key")?;
                let right_key = iter.next_kwarg::<Option<&Value>>("right_key")?;
                let inner = iter.next_kwarg::<Option<bool>>("inner")?.unwrap_or(false);
                let full_outer = iter
                    .next_kwarg::<Option<bool>>("full_outer")?
                    .unwrap_or(false);
                let require_match = iter
                    .next_kwarg::<Option<bool>>("require_match")?
                    .unwrap_or(false);
                let columns = iter.next_kwarg::<Option<&Value>>("columns")?;
                iter.finish()?;

                let right_table = match right_table.downcast_object_ref::<AgateTable>() {
                    Some(table) => table,
                    None => {
                        return Err(Error::new(
                            ErrorKind::InvalidArgument,
                            format!(
                                "Table.join: right_table must be a Table: {right_table} found instead"
                            ),
                        ));
                    }
                };

                let join_type = if inner {
                    if full_outer {
                        return Err(Error::new(
                            ErrorKind::InvalidArgument,
                            "A join can not be both \"inner\" and \"full_outer\".",
                        ));
                    }
                    JoinType::Inner
                } else if full_outer {
                    JoinType::FullOuter
                } else {
                    JoinType::LeftOuter
                };

                let table = self.join(
                    right_table,
                    left_key,
                    right_key,
                    join_type,
                    require_match,
                    columns,
                )?;
                Ok(Value::from_object(table))
            }
            // ```python
            // def limit(self, start_or_stop=None, stop=None, step=None):
            //     """
            //     Filter data to a subset of all rows.
            //     """
            // ```
            //
            // Only the single-arg `limit(n)` form is supported on the rust port.
            "limit" => {
                let iter = ArgsIter::new("Table.limit", &["n"], args);
                let n = iter.next_arg::<i64>()?;
                iter.finish()?;
                let table = self.as_ref().limit(n).map_err(|e| {
                    Error::new(ErrorKind::InvalidOperation, format!("Table.limit: {e}"))
                })?;
                Ok(Value::from_object(table))
            }
            // ```python
            // def distinct(self, key=None):
            //     """
            //     Create a new table with only unique rows.
            //
            //     :param key:
            //         Either the name of a single column to use to identify unique rows, a
            //         sequence of such column names, a :class:`function` that takes a
            //         row and returns a value to identify unique rows, or `None`, in
            //         which case the entire row will be checked for uniqueness.
            //     :returns:
            //     A new :class:`.Table`.
            //     """
            // ```
            "distinct" => {
                let iter = ArgsIter::new("Table.distinct", &[], args);
                let key = iter.next_kwarg::<Option<&Value>>("key")?;
                iter.finish()?;

                let key = match key {
                    Some(v) => {
                        if let Some(s) = v.as_str() {
                            Some(vec![s.to_string()])
                        } else if let Ok(iter) = v.try_iter() {
                            let mut keys = Vec::new();
                            for key in iter {
                                match key.as_str() {
                                    Some(s) => keys.push(s.to_string()),
                                    None => unimplemented!("distinct with non-string keys"),
                                }
                            }
                            Some(keys)
                        } else {
                            None
                        }
                    }
                    None => None,
                };

                let result = self.as_ref().distinct(key).map_err(|e| {
                    Error::new(ErrorKind::InvalidOperation, format!("Table.distinct: {e}"))
                })?;
                Ok(Value::from_object(result))
            }
            other => unimplemented!("AgateTable::{}", other),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::flat_record_batch::FlatRecordBatch;
    use crate::test_fixtures::*;
    use crate::*;
    use arrow::array::{
        ArrayRef, BooleanBuilder, DictionaryArray, Float64Builder, Int32Array, Int32Builder,
        ListBuilder, StringBuilder, StringViewBuilder, StructBuilder,
    };
    use arrow::array::{GenericListArray, StringArray};
    use arrow::csv::reader::ReaderBuilder;
    use arrow::datatypes::{DataType, Field, Int32Type, Schema};
    use arrow::record_batch::RecordBatch;
    use arrow_array::{Array, ListArray, RecordBatchOptions, UInt64Array};
    use arrow_schema::Fields;
    use minijinja::value::Kwargs;
    use minijinja::value::ValueMap;
    use minijinja::value::mutable_map::MutableMap;
    use minijinja::{Environment, context};
    use minijinja_contrib::testing::jinja_assert;
    use std::io;
    use std::sync::Arc;

    fn simple_record_batch() -> Arc<RecordBatch> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new("country", DataType::Utf8, true),
        ]));
        let id_array: ArrayRef = Arc::new(Int32Array::from(vec![Some(42), Some(43), Some(44)]));
        let country_array: ArrayRef = Arc::new(StringArray::from(vec![
            Some("Brazil"),
            Some("USA"),
            Some("Canada"),
        ]));
        let batch = RecordBatch::try_new(schema, vec![id_array, country_array]).unwrap();
        Arc::new(batch)
    }

    #[test]
    fn jinja_sequence_value_matrix() {
        let table = Arc::new(main_table(&[1, 2], &["a", "b"], &[None, Some(1)]));
        let columns = Value::from_object(table.columns());
        let rows = Value::from_object(table.rows());
        let values = [
            ("columns", columns.clone(), 3),
            ("column", columns.get_item(&Value::from(0)).unwrap(), 2),
            ("rows", rows.clone(), 2),
            ("row", rows.get_item(&Value::from(0)).unwrap(), 3),
            (
                "tuple",
                Value::from_object(table.column_names_as_tuple().into_tuple()),
                3,
            ),
        ];
        let env = Environment::new();

        for (name, value, expected_len) in values {
            assert_eq!(value.kind(), ValueKind::Seq, "{name}");
            assert_eq!(
                env.render_str(
                    "{{ value | list | length }}",
                    context! { value => value },
                    &[],
                )
                .unwrap(),
                expected_len.to_string(),
                "{name}",
            );
        }
    }

    #[test]
    fn test_columns() {
        let batch = simple_record_batch();
        let table = Arc::new(AgateTable::from_record_batch(batch));

        // there are 2 columns
        let columns = table.columns();
        let values = columns.values();
        assert_eq!(values.len(), 2);

        let id = values.get(0).unwrap();
        let country = values.get(1).unwrap();

        let id = id.as_object().unwrap();
        let country = country.as_object().unwrap();

        // each column contains 3 values
        assert_eq!(id.enumerator_len().unwrap(), 3);
        assert_eq!(country.enumerator_len().unwrap(), 3);
    }

    #[test]
    fn test_select() {
        let batch = simple_record_batch();
        let table = AgateTable::from_record_batch(batch).into_value();

        let env = Environment::new();
        let state = env.empty_state();
        let select = |table: &Value, args: &[Value]| -> Result<Value, minijinja::Error> {
            table.call_method(&state, "select", args, &[])
        };

        let selected = select(
            &table,
            &[Value::from_iter([
                Value::from("country"),
                Value::from("id"),
                Value::from("country"),
            ])],
        )
        .unwrap()
        .downcast_object::<AgateTable>()
        .unwrap();

        assert_eq!(selected.num_columns(), 3);
        assert_eq!(selected.num_rows(), 3);

        assert_eq!(selected.column_name(0).unwrap(), "country");
        assert_eq!(selected.column_name(1).unwrap(), "id");
        assert_eq!(selected.column_name(2).unwrap(), "country");

        let cols = selected.columns().values();
        let country = cols.get(2).unwrap();
        assert_eq!(country.len(), Some(3));
        assert_eq!(
            country.get_item_by_index(0).unwrap().as_str().unwrap(),
            "Brazil"
        );
        assert_eq!(
            country.get_item_by_index(1).unwrap().as_str().unwrap(),
            "USA"
        );
        assert_eq!(
            country.get_item_by_index(2).unwrap().as_str().unwrap(),
            "Canada"
        );

        // select 0 columns
        let selected = select(&table, &[Value::from_iter([] as [Value; 0])])
            .unwrap()
            .downcast_object::<AgateTable>()
            .unwrap();
        // result has 0 columns and 3 rows
        assert_eq!(selected.num_columns(), 0);
        assert_eq!(selected.num_rows(), 3);
    }

    #[test]
    fn test_rows() {
        let table = AgateTable::from_record_batch(simple_record_batch());
        let rows = table.rows();
        let values = rows.values();
        assert_eq!(values.len(), 3);
    }

    fn country_at(table: &AgateTable, idx: usize) -> String {
        let table_arc = Arc::new(table.clone());
        let cols = table_arc.columns().values();
        let country = cols.get(1).unwrap();
        country
            .get_item_by_index(idx)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn test_limit_zero_preserves_schema() {
        let table = AgateTable::from_record_batch(simple_record_batch());
        assert_eq!(table.num_rows(), 3);
        let empty = table.limit(0).unwrap();
        assert_eq!(empty.num_rows(), 0);
        assert_eq!(empty.num_columns(), 2);
        assert_eq!(
            empty.column_names(),
            vec!["id".to_string(), "country".to_string()]
        );
        assert_eq!(empty.column_types().len(), 2);
    }

    #[test]
    fn test_limit_single_arg_takes_first_n() {
        let table = AgateTable::from_record_batch(simple_record_batch());
        let limited = table.limit(2).unwrap();
        assert_eq!(limited.num_rows(), 2);
        assert_eq!(country_at(&limited, 0), "Brazil");
        assert_eq!(country_at(&limited, 1), "USA");
    }

    #[test]
    fn test_limit_exceeds_returns_all_rows() {
        let table = AgateTable::from_record_batch(simple_record_batch());
        let limited = table.limit(100).unwrap();
        assert_eq!(limited.num_rows(), 3);
    }

    #[test]
    fn test_limit_negative_errors() {
        let table = AgateTable::from_record_batch(simple_record_batch());
        let err = table.limit(-1).unwrap_err();
        assert!(err.to_string().contains("non-negative"));
    }

    #[test]
    fn test_limit_via_jinja_call_method() {
        let table = AgateTable::from_record_batch(simple_record_batch()).into_value();

        let env = Environment::new();
        let state = env.empty_state();

        // Single positional: limit(0) -> empty
        let limited = table
            .call_method(&state, "limit", &[Value::from(0)], &[])
            .unwrap()
            .downcast_object::<AgateTable>()
            .unwrap();
        assert_eq!(limited.num_rows(), 0);
        assert_eq!(limited.num_columns(), 2);

        // Single positional: limit(2) -> first two rows
        let limited = table
            .call_method(&state, "limit", &[Value::from(2)], &[])
            .unwrap()
            .downcast_object::<AgateTable>()
            .unwrap();
        assert_eq!(limited.num_rows(), 2);

        // Keyword: limit(n=2)
        let limited = table
            .call_method(
                &state,
                "limit",
                &[Kwargs::from_iter([("n", Value::from(2))]).into()],
                &[],
            )
            .unwrap()
            .downcast_object::<AgateTable>()
            .unwrap();
        assert_eq!(limited.num_rows(), 2);

        // Multi-arg form is not supported on the rust port.
        let err = table
            .call_method(&state, "limit", &[Value::from(1), Value::from(3)], &[])
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TooManyArguments);
    }

    #[test]
    fn test_table_repr_single_column_and_select_rows_bounds() {
        let table = AgateTable::from_record_batch(simple_record_batch());

        let country = table.repr.single_column_table(1).unwrap();
        assert_eq!(country.num_columns(), 1);
        assert_eq!(country.num_rows(), 3);
        assert_eq!(country.column_name(0).unwrap(), "country");
        assert!(table.repr.single_column_table(99).is_none());

        let selected_indices = UInt64Array::new(vec![0, 2].into(), None);
        let selected = table.repr.select_rows(&selected_indices, None).unwrap();
        assert_eq!(selected.num_rows(), 2);
        assert_eq!(selected.cell(0, 1), Some(Value::from("Brazil")));
        assert_eq!(selected.cell(1, 1), Some(Value::from("Canada")));

        let out_of_bounds = UInt64Array::new(vec![99].into(), None);
        assert!(table.repr.select_rows(&out_of_bounds, None).is_err());
    }

    #[test]
    fn test_table_columns_and_object_properties() {
        let table = AgateTable::new_with_single_row_name(simple_record_batch(), "row-name");
        let table_value = table.into_value();
        let env = Environment::new();
        let state = env.empty_state();

        let column_names = table_value.get_attr("column_names").unwrap();
        assert_eq!(column_names.len(), Some(2));
        assert_eq!(
            column_names.get_item_by_index(0).unwrap(),
            Value::from("id")
        );
        assert_eq!(
            column_names.get_item_by_index(1).unwrap(),
            Value::from("country")
        );

        let column_types = table_value.get_attr("column_types").unwrap();
        assert_eq!(column_types.len(), Some(2));
        assert_eq!(
            column_types
                .get_item_by_index(0)
                .unwrap()
                .downcast_object_ref::<crate::DataType>()
                .unwrap()
                .type_name(),
            "Number"
        );
        assert_eq!(
            column_types
                .get_item_by_index(1)
                .unwrap()
                .downcast_object_ref::<crate::DataType>()
                .unwrap()
                .type_name(),
            "Text"
        );

        let columns = table_value.get_attr("columns").unwrap();
        assert_eq!(columns.len(), Some(2));
        let keys = columns.call_method(&state, "keys", &[], &[]).unwrap();
        assert_eq!(keys.get_item_by_index(1).unwrap(), Value::from("country"));
        let id_column = columns.get_item_by_index(0).unwrap();
        assert_eq!(id_column.get_attr("index").unwrap(), Value::from(0usize));
        assert_eq!(id_column.get_attr("name").unwrap(), Value::from("id"));
        assert_eq!(id_column.len(), Some(3));
        assert_eq!(id_column.get_item_by_index(0).unwrap(), Value::from(42));
        assert_eq!(id_column.get_item_by_index(2).unwrap(), Value::from(44));
        assert_eq!(
            id_column
                .get_attr("data_type")
                .unwrap()
                .downcast_object_ref::<crate::DataType>()
                .unwrap()
                .type_name(),
            "Number"
        );
        let columns_items = columns.call_method(&state, "items", &[], &[]).unwrap();
        assert_eq!(columns_items.len(), Some(2));
        assert_eq!(
            columns_items
                .get_item_by_index(0)
                .unwrap()
                .get_item_by_index(0)
                .unwrap(),
            Value::from("id")
        );
        let country_column = columns
            .call_method(&state, "get", &[Value::from("country")], &[])
            .unwrap();
        assert_eq!(
            country_column.get_attr("name").unwrap(),
            Value::from("country")
        );
    }

    #[test]
    fn test_table_rows_and_row_name_properties() {
        let table = AgateTable::new_with_single_row_name(simple_record_batch(), "row-name");
        let table_value = table.into_value();
        let env = Environment::new();
        let state = env.empty_state();

        let rows = table_value.get_attr("rows").unwrap();
        assert_eq!(rows.len(), Some(3));
        let row_keys = rows.call_method(&state, "keys", &[], &[]).unwrap();
        assert_eq!(
            row_keys.get_item_by_index(0).unwrap(),
            Value::from("row-name")
        );
        let first_row = rows.get_item_by_index(0).unwrap();
        assert_eq!(first_row.get_attr("id").unwrap(), Value::from(42));
        assert_eq!(
            first_row.get_attr("country").unwrap(),
            Value::from("Brazil")
        );
        let rows_items = rows.call_method(&state, "items", &[], &[]).unwrap();
        assert_eq!(rows_items.len(), Some(3));
        assert_eq!(
            rows_items
                .get_item_by_index(0)
                .unwrap()
                .get_item_by_index(0)
                .unwrap(),
            Value::from("row-name")
        );

        let row_names = table_value.get_attr("row_names").unwrap();
        assert_eq!(row_names.len(), Some(3));
        assert_eq!(
            row_names.get_item_by_index(2).unwrap(),
            Value::from("row-name")
        );
    }

    #[test]
    fn test_table_with_single_row_name() {
        let table = AgateTable::new_with_single_row_name(simple_record_batch(), "The Row Name");
        let row_names: Tuple = table.row_names().unwrap();
        for i in 0..table.num_rows() {
            let name = row_names.get(i as isize).unwrap();
            assert_eq!(name.as_str().unwrap(), "The Row Name");
        }
        let the_row_name = Value::from("The Row Name");
        let not_the_row_name = Value::from("Not The Row Name");
        assert_eq!(row_names.count(&the_row_name), table.num_rows());
        assert_eq!(row_names.count(&not_the_row_name), 0);
    }

    #[test]
    fn test_table_with_multiple_row_names() {
        let row_names = {
            let mut builder = StringViewBuilder::with_capacity(3);
            builder.append_value("Row 1");
            builder.append_value("Row 2");
            builder.append_value("Row 3");
            Arc::new(builder.finish())
        };
        let table = AgateTable::new(simple_record_batch(), Some(row_names));
        let row_names: Tuple = table.row_names().unwrap();
        for i in 0..table.num_rows() {
            let name = row_names.get(i as isize).unwrap();
            assert_eq!(name.as_str().unwrap(), format!("Row {}", i + 1));
        }
        for i in 0..table.num_rows() {
            let name = Value::from(format!("Row {}", i + 1));
            assert_eq!(row_names.count(&name), 1);
        }
        let row_2_name = Value::from("Row 2");
        assert_eq!(row_names.count(&row_2_name), 1);

        // Now get the rows via the Jinja API
        let table = table.into_value();
        let row_names = table.get_attr("row_names").unwrap();
        row_names
            .try_iter()
            .unwrap()
            .enumerate()
            .for_each(|(i, name)| {
                assert_eq!(name.as_str().unwrap(), format!("Row {}", i + 1));
            });

        // We can also get it as a property from the table object
        let row_names_prop = table.get_attr("row_names").unwrap();
        assert_eq!(row_names_prop, row_names);
    }

    #[test]
    fn test_agate_table_from_value() {
        let file = io::Cursor::new(
            "grantee,privilege_type\n\
 dbt_test_user_1,SELECT\n\
 dbt_test_user_2,SELECT\n\
 dbt_test_user_3,SELECT\n",
        );
        let csv_schema = Schema::new(vec![
            Field::new("grantee", DataType::Utf8, true),
            Field::new("privilege_type", DataType::Utf8, true),
        ]);
        let mut reader = ReaderBuilder::new(Arc::new(csv_schema))
            .with_header(true)
            .build(file)
            .unwrap();
        let batch = reader.next().unwrap().unwrap();
        let table = AgateTable::from_record_batch(Arc::new(batch)).into_value();

        let downcasted = table.downcast_object::<AgateTable>().unwrap();
        assert_eq!(downcasted.num_columns(), 2);
        assert_eq!(downcasted.num_rows(), 3);
        let record_batch = downcasted.original_record_batch();
        assert_eq!(record_batch.num_columns(), 2);
        assert_eq!(record_batch.num_rows(), 3);
    }

    /// Create a nested record batch with different data types.
    ///
    /// NOTE: other tests may use a JSON->Arrow parser to create record batches more
    /// easily, but let's keep this one as an example on how to use builders to create
    /// record batches imperatively.
    ///
    /// The data in the record batch is what the following SQL would generate:
    ///
    /// ```sql
    /// INSERT INTO user_events (id, user_name, event_tags, event_meta, groups) VALUES
    ///   (1, 'alice',   ARRAY['login', 'mobile'],   '{"device": "iPhone", "success": true}',
    ///     ARRAY[
    ///       ARRAY[1, 2, 3],
    ///       ARRAY[4, 5],
    ///       ARRAY[6]
    ///     ]),
    ///   (2, 'bob',     ARRAY['purchase'],          '{"item_id": 1234, "amount": 49.99}',
    ///     ARRAY[
    ///       ARRAY[10, 20],
    ///       ARRAY[30, 40, 50],
    ///       ARRAY[60, 70],
    ///       ARRAY[80]
    ///     ]),
    ///   (3, 'charlie', ARRAY['logout', 'timeout'], '{"duration_sec": 300}',
    ///     ARRAY[
    ///       ARRAY[7],
    ///       NULL,
    ///       ARRAY[8, 9]
    ///     ]),
    ///   (4, 'dana',    ARRAY[]::TEXT[],            '{"device": "desktop"}',
    ///     ARRAY[]::INTEGER[][]),  -- Empty outer list
    ///   (5, 'eve',     NULL,                       '{"success": false}',
    ///     NULL)
    ///   );
    /// ```
    fn nested_record_batch() -> RecordBatch {
        const CAPACITY: usize = 5;
        // all the missing fields become NULL in the record batch
        let event_type_fields = Fields::from(vec![
            Field::new("device", DataType::Utf8, true),
            Field::new("item_id", DataType::Int32, true),
            Field::new("amount", DataType::Float64, true),
            Field::new("duration_sec", DataType::Int32, true),
            Field::new("success", DataType::Boolean, true),
        ]);
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
            Field::new(
                "event_tags",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
                true,
            ),
            Field::new(
                "event_meta",
                DataType::Struct(event_type_fields.clone()),
                false,
            ),
            Field::new(
                "groups",
                DataType::List(Arc::new(Field::new(
                    "item",
                    DataType::List(Arc::new(Field::new("item", DataType::Int32, true))),
                    true,
                ))),
                true,
            ),
        ]));
        let id_array: ArrayRef = Arc::new(Int32Array::from(vec![
            Some(1),
            Some(2),
            Some(3),
            Some(4),
            Some(5),
        ]));
        let user_name_array: ArrayRef = Arc::new(StringArray::from(vec![
            Some("alice"),
            Some("bob"),
            Some("charlie"),
            Some("dana"),
            Some("eve"),
        ]));
        let event_tags_array = {
            let mut event_tags_builder = {
                let values_builder = StringBuilder::with_capacity(CAPACITY, CAPACITY * 10);
                ListBuilder::<StringBuilder>::with_capacity(values_builder, CAPACITY)
            };
            event_tags_builder.append_value(vec![Some("login"), Some("mobile")]);
            event_tags_builder.append_value(vec![Some("purchase")]);
            event_tags_builder.append_value(vec![Some("logout"), Some("timeout")]);
            event_tags_builder.append_value(Vec::<Option<String>>::new());
            event_tags_builder.append_null();

            let list_array = event_tags_builder.finish();
            // re-create the list array with a non-nullable field because finish()
            // doesn't let us specify the nullability of the list field
            let new_list_field = Field::new_list_field(
                list_array.values().data_type().clone(),
                false, // the values are non-nullable!
            );
            let event_tags_array = GenericListArray::new(
                Arc::new(new_list_field),
                list_array.offsets().clone(),
                list_array.values().clone(),
                None,
            );
            Arc::new(event_tags_array)
        };

        let events_array = {
            let mut event_builder = StructBuilder::from_fields(event_type_fields, CAPACITY);
            let mut append = |device: Option<&str>,
                              item_id: Option<i32>,
                              amount: Option<f64>,
                              duration_sec: Option<i32>,
                              success: Option<bool>| {
                event_builder
                    .field_builder::<StringBuilder>(0)
                    .unwrap()
                    .append_option(device.to_owned());
                event_builder
                    .field_builder::<Int32Builder>(1)
                    .unwrap()
                    .append_option(item_id);
                event_builder
                    .field_builder::<Float64Builder>(2)
                    .unwrap()
                    .append_option(amount);
                event_builder
                    .field_builder::<Int32Builder>(3)
                    .unwrap()
                    .append_option(duration_sec);
                event_builder
                    .field_builder::<BooleanBuilder>(4)
                    .unwrap()
                    .append_option(success);
                event_builder.append(true);
            };
            append(Some("iPhone"), None, None, None, Some(true));
            append(None, Some(1234), Some(49.99), None, None);
            append(None, None, None, Some(300), None);
            append(Some("Desktop"), None, None, None, None);
            append(None, None, None, None, Some(false));
            Arc::new(event_builder.finish())
        };

        let groups_array = {
            let mut groups_builder = {
                let inner_values_builder = Int32Builder::new();
                let inner_list_builder = ListBuilder::<Int32Builder>::new(inner_values_builder);
                ListBuilder::<ListBuilder<Int32Builder>>::with_capacity(
                    inner_list_builder,
                    CAPACITY,
                )
            };
            let inner_list = groups_builder.values();
            inner_list.append_value(vec![Some(1), Some(2), Some(3)]);
            inner_list.append_value(vec![Some(4), Some(5)]);
            inner_list.append_value(vec![Some(6)]);
            groups_builder.append(true); // groups 0

            let inner_list = groups_builder.values();
            inner_list.append_value(vec![Some(10), Some(20)]);
            inner_list.append_value(vec![Some(30), Some(40), Some(50)]);
            inner_list.append_value(vec![Some(60), Some(70)]);
            inner_list.append_value(vec![Some(80)]);
            groups_builder.append(true); // groups 1

            let inner_list = groups_builder.values();
            inner_list.append_value(vec![Some(7)]);
            inner_list.append_null();
            inner_list.append_value(vec![Some(8), Some(9)]);
            groups_builder.append(true); // groups 2

            // []   -- Empty list of groups (non-NULL)
            groups_builder.append(true); // groups 3

            // NULL -- Null list of groups
            groups_builder.append(false); // groups 4

            Arc::new(groups_builder.finish())
        };

        let columns = vec![
            id_array,
            user_name_array,
            event_tags_array,
            events_array,
            groups_array,
        ];
        RecordBatch::try_new(schema, columns).unwrap()
    }

    #[test]
    fn test_record_batch_flattening() {
        let batch = nested_record_batch();
        let _batch = FlatRecordBatch::try_new(Arc::new(batch)).unwrap();
        // TODO(felipcrv); implement CSV serialization to assert here
    }

    /// Take a 5-element column and make a dictionary-encoded version of it
    /// using the first two elements as dictionary values.
    fn dict_encoded_example(col: &ArrayRef) -> ArrayRef {
        let dictionary_values = col.slice(0, 2);
        let indices_array = Int32Array::from(vec![Some(0), Some(1), Some(0), Some(1), Some(0)]);
        let dict_array =
            DictionaryArray::<Int32Type>::try_new(indices_array, dictionary_values).unwrap();
        Arc::new(dict_array) as ArrayRef
    }

    fn batch_with_replaced_column(
        batch: &RecordBatch,
        col_idx: usize,
        new_col: ArrayRef,
    ) -> RecordBatch {
        let new_columns = batch
            .columns()
            .iter()
            .enumerate()
            .map(|(i, col)| {
                if i == col_idx {
                    new_col.clone()
                } else {
                    col.clone()
                }
            })
            .collect::<Vec<_>>();
        let new_schema = {
            let old_schema = batch.schema();
            let fields = new_columns
                .iter()
                .enumerate()
                .map(|(i, col)| {
                    old_schema
                        .field(i)
                        .clone()
                        .with_data_type(col.data_type().clone())
                })
                .collect::<Vec<_>>();
            Arc::new(Schema::new(fields))
        };
        RecordBatch::try_new(new_schema, new_columns).unwrap()
    }

    #[test]
    fn test_record_batch_flattening_with_dict_encoded_struct() {
        let batch = nested_record_batch();

        let event_meta = batch.column(3);
        assert!(matches!(event_meta.data_type(), DataType::Struct(_)));

        // Build a dictionary-encoded version of the "events" column
        let dict_event_meta = dict_encoded_example(event_meta);

        let new_batch = batch_with_replaced_column(&batch, 3, dict_event_meta);
        let _flat_batch = FlatRecordBatch::try_new(Arc::new(new_batch)).unwrap();
        // TODO(felipcrv); implement CSV serialization to assert here
    }

    #[test]
    fn test_record_batch_flattening_with_dict_encoded_list() {
        let batch = nested_record_batch();

        let event_tags = batch.column(2);
        assert!(matches!(event_tags.data_type(), DataType::List(_)));

        // Build a dictionary-encoded version of the "event_tags" column
        let dict_event_tags = dict_encoded_example(event_tags);

        let new_batch = batch_with_replaced_column(&batch, 2, dict_event_tags);
        let _flat_batch = FlatRecordBatch::try_new(Arc::new(new_batch)).unwrap();
        // TODO(felipcrv); implement CSV serialization to assert here
    }

    #[test]
    fn test_record_batch_flattening_with_nested_dict_encoded() {
        let batch = nested_record_batch();

        let event_meta = batch.column(3);
        assert!(matches!(event_meta.data_type(), DataType::Struct(_)));

        // Build a dictionary-encoded version of the "events" column...
        let dict_event_meta = dict_encoded_example(event_meta);
        // ...and dictionary-encode it again.
        let dict_dict_event_meta = dict_encoded_example(&dict_event_meta);

        let new_batch = batch_with_replaced_column(&batch, 3, dict_dict_event_meta);
        let _flat_batch = FlatRecordBatch::try_new(Arc::new(new_batch)).unwrap();
        // TODO(felipcrv); implement CSV serialization to assert here
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
    fn test_empty_batch_column_types_and_names() {
        let contact_fields = Fields::from(vec![
            Field::new("first", DataType::Utf8, true),
            Field::new("last", DataType::Utf8, true),
        ]);
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, false),
            Field::new(
                "event_tags",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
                true,
            ),
            Field::new("contact", DataType::Struct(contact_fields.clone()), true),
        ]));
        let mut contact_builder = StructBuilder::from_fields(contact_fields, 0);
        let opts = RecordBatchOptions::default().with_row_count(Some(0));
        let batch = RecordBatch::try_new_with_options(
            schema,
            vec![
                Arc::new(Int32Array::new_null(0)) as ArrayRef,
                Arc::new(StringArray::new_null(0)) as ArrayRef,
                Arc::new(ListArray::new_null(
                    Arc::new(Field::new_list_field(
                        DataType::Utf8,
                        false, // the values are non-nullable!
                    )),
                    0,
                )) as ArrayRef,
                Arc::new(contact_builder.finish()) as ArrayRef,
            ],
            &opts,
        )
        .unwrap();
        let table = AgateTable::from_record_batch(Arc::new(batch));
        let column_types = table.column_types();
        assert_eq!(
            column_types,
            vec![
                crate::DataType::new("Number".to_string()),
                crate::DataType::new("Text".to_string()),
                crate::DataType::new("Text".to_string()),
                crate::DataType::new("Text".to_string()),
                crate::DataType::new("Text".to_string()),
            ]
        );
        let column_names = table.column_names();
        assert_eq!(
            column_names,
            vec![
                "id",
                "name",
                "event_tags.0",
                "contact/first",
                "contact/last",
            ]
        );
        assert_eq!(
            table
                .repr
                .flat
                .top_level_column_names()
                .collect::<Vec<&String>>(),
            vec!["id", "name", "event_tags", "contact"]
        );
        jinja_assert(
            table.clone(),
            "{{ obj.top_level_column_names | join(',') }}",
            "id,name,event_tags,contact",
        );

        let column_types = table.column_types_as_tuple();
        assert_eq!(column_types.len(), 5);

        // column_types now returns DataType objects, not strings
        let dt0 = column_types.get_item_by_index(0).unwrap();
        let dt0_obj = dt0.downcast_object_ref::<crate::DataType>().unwrap();
        assert_eq!(dt0_obj.type_name(), "Number");

        let dt1 = column_types.get_item_by_index(1).unwrap();
        let dt1_obj = dt1.downcast_object_ref::<crate::DataType>().unwrap();
        assert_eq!(dt1_obj.type_name(), "Text");

        let dt2 = column_types.get_item_by_index(2).unwrap();
        let dt2_obj = dt2.downcast_object_ref::<crate::DataType>().unwrap();
        assert_eq!(dt2_obj.type_name(), "Text");

        let dt3 = column_types.get_item_by_index(3).unwrap();
        let dt3_obj = dt3.downcast_object_ref::<crate::DataType>().unwrap();
        assert_eq!(dt3_obj.type_name(), "Text");

        let dt4 = column_types.get_item_by_index(4).unwrap();
        let dt4_obj = dt4.downcast_object_ref::<crate::DataType>().unwrap();
        assert_eq!(dt4_obj.type_name(), "Text");

        assert_eq!(column_types.count_occurrences_of(&Value::from("Number")), 1);
        assert_eq!(column_types.count_occurrences_of(&Value::from("Text")), 4);
        assert_eq!(
            column_types.count_occurrences_of(&Value::from("DateTime")),
            0
        );
        assert_eq!(column_types.index_of(&Value::from("Number")), Some(0));
        assert_eq!(column_types.index_of(&Value::from("Text")), Some(1));
        assert_eq!(column_types.index_of(&Value::from("DateTime")), None);
        let column_types2 = table.column_types_as_tuple();
        assert!(column_types.eq_repr(&column_types2 as &dyn TupleRepr));

        let column_names = table.column_names_as_tuple();
        assert_eq!(column_names.len(), 5);
        assert_eq!(
            column_names.get_item_by_index(0).unwrap().as_str().unwrap(),
            "id"
        );
        assert_eq!(
            column_names.get_item_by_index(1).unwrap().as_str().unwrap(),
            "name"
        );
        assert_eq!(
            column_names.get_item_by_index(2).unwrap().as_str().unwrap(),
            "event_tags.0"
        );
        assert_eq!(
            column_names.get_item_by_index(3).unwrap().as_str().unwrap(),
            "contact/first"
        );
        assert_eq!(
            column_names.get_item_by_index(4).unwrap().as_str().unwrap(),
            "contact/last"
        );
        assert_eq!(column_names.count_occurrences_of(&Value::from("id")), 1);
        assert_eq!(column_names.count_occurrences_of(&Value::from("name")), 1);
        assert_eq!(
            column_names.count_occurrences_of(&Value::from("event_tags.0")),
            1
        );
        assert_eq!(
            column_names.count_occurrences_of(&Value::from("contact/first")),
            1
        );
        assert_eq!(
            column_names.count_occurrences_of(&Value::from("contact/last")),
            1
        );
        assert_eq!(
            column_names.count_occurrences_of(&Value::from("nonexistent")),
            0
        );
        assert_eq!(column_names.index_of(&Value::from("id")), Some(0));
        assert_eq!(column_names.index_of(&Value::from("name")), Some(1));
        assert_eq!(column_names.index_of(&Value::from("event_tags.0")), Some(2));
        assert_eq!(
            column_names.index_of(&Value::from("contact/first")),
            Some(3)
        );
        assert_eq!(column_names.index_of(&Value::from("contact/last")), Some(4));
        assert_eq!(column_names.index_of(&Value::from("nonexistent")), None);
        let column_names2 = table.column_names_as_tuple();
        assert!(column_names.eq_repr(&column_names2 as &dyn TupleRepr));
    }

    #[test]
    fn test_column_renaming() {
        let batch = Arc::new(nested_record_batch());
        let agate_table = AgateTable::from_record_batch(batch);
        let col_names = agate_table.column_names();
        let table = agate_table.into_value();

        let env = Environment::new();
        let state = env.empty_state();
        let rename = |table: &Value, args: &[Value]| -> Result<Value, minijinja::Error> {
            table.call_method(&state, "rename", args, &[])
        };

        // Original column names:
        //   "id", "name", "event_tags.0", "event_tags.1", "event_meta/device",
        //   "event_meta/item_id", "event_meta/amount", "event_meta/duration_sec",
        //   "event_meta/success", "groups.0", "groups.1", "groups.2", "groups.3"

        // Renaming with a map
        let map = ValueMap::from_iter([
            ("groups.0.0".into(), "first_group_cell".into()),
            ("groups.1.0".into(), "second_group_cell".into()),
            ("groups.2.0".into(), "third_group_cell".into()),
            ("groups.3.0".into(), "fourth_group_cell".into()),
            ("nonexistent".into(), "should_not_exist".into()),
        ]);
        let new_names = rename(&table, &[Value::from_object(map.clone())])
            .unwrap()
            .downcast_object::<AgateTable>()
            .unwrap()
            .column_names();
        assert_eq!(
            new_names[0..new_names.len() - 12],
            col_names[0..col_names.len() - 12]
        );
        static EXPECTED_12_LAST_COL_NAMES: [&str; 12] = [
            "first_group_cell",
            "groups.0.1",
            "groups.0.2",
            "second_group_cell",
            "groups.1.1",
            "groups.1.2",
            "third_group_cell",
            "groups.2.1",
            "groups.2.2",
            "fourth_group_cell",
            "groups.3.1",
            "groups.3.2",
        ];
        assert_eq!(
            new_names[new_names.len() - 12..],
            EXPECTED_12_LAST_COL_NAMES
        );

        // Renaming with a mutable map
        let map: MutableMap = map.into();
        let new_names = rename(&table, &[Value::from_object(map)])
            .unwrap()
            .downcast_object::<AgateTable>()
            .unwrap()
            .column_names();
        assert_eq!(
            new_names[0..new_names.len() - 12],
            col_names[0..col_names.len() - 12]
        );
        assert_eq!(
            new_names[new_names.len() - 12..],
            EXPECTED_12_LAST_COL_NAMES
        );

        // Renaming with an array
        let array = {
            let mut array = col_names[0..col_names.len() - 12].to_vec();
            array.extend_from_slice(EXPECTED_12_LAST_COL_NAMES.map(|s| s.to_string()).as_slice());
            Value::from_object(array)
        };
        let new_names = rename(&table, &[array])
            .unwrap()
            .downcast_object::<AgateTable>()
            .unwrap()
            .column_names();
        assert_eq!(
            new_names[0..new_names.len() - 12],
            col_names[0..col_names.len() - 12]
        );
        assert_eq!(
            new_names[new_names.len() - 12..],
            EXPECTED_12_LAST_COL_NAMES
        );
    }

    #[test]
    fn test_row_renaming() {
        let batch = simple_record_batch();
        let table = AgateTable::from_record_batch(batch).into_value();

        let env = Environment::new();
        let state = env.empty_state();
        let rename = |table: &Value, args: &[Value]| -> Result<Value, minijinja::Error> {
            table.call_method(&state, "rename", args, &[])
        };

        // Original row names are undefined
        let original_row_names = table.get_attr("row_names").unwrap();
        assert!(original_row_names.get_item_by_index(0).is_err());

        // Renaming with an array
        let array = Value::from_object(vec![
            "Row 1".to_string(),
            "Row 2".to_string(),
            "Row 3".to_string(),
        ]);
        let table = rename(&table, &[Value::from(()), array]).unwrap();
        let new_names = table
            .downcast_object::<AgateTable>()
            .unwrap()
            .row_names()
            .unwrap();
        assert_eq!(new_names.len(), 3);
        assert_eq!(new_names.get(0).unwrap().as_str().unwrap(), "Row 1");
        assert_eq!(new_names.get(1).unwrap().as_str().unwrap(), "Row 2");
        assert_eq!(new_names.get(2).unwrap().as_str().unwrap(), "Row 3");

        // Renaming with a map
        let map = ValueMap::from_iter([
            ("Row 1".into(), "First Row".into()),
            ("Nonexistent".into(), "Should Not Exist".into()),
        ]);
        let table = rename(&table, &[Value::from(()), Value::from_object(map)]).unwrap();
        let new_names = table
            .downcast_object::<AgateTable>()
            .unwrap()
            .row_names()
            .unwrap();
        assert_eq!(new_names.len(), 3);
        assert_eq!(new_names.get(0).unwrap().as_str().unwrap(), "First Row");
        assert_eq!(new_names.get(1).unwrap().as_str().unwrap(), "Row 2");
        assert_eq!(new_names.get(2).unwrap().as_str().unwrap(), "Row 3");
    }

    #[test]
    fn test_column_renaming_with_map_filter() {
        // Test case for the dbt_profiler issue: using map filter to rename columns
        let batch = simple_record_batch();
        let table_val = AgateTable::from_record_batch(batch).into_value();

        let env = Environment::new();
        let state = env.empty_state();

        // Simulate what happens in dbt_profiler:
        // {% set information_schema_columns = information_schema_columns.rename(information_schema_columns.column_names | map('lower') | list) %}
        // Get the table and its column_names
        let column_names = table_val.get_attr("column_names").unwrap();

        // The map filter returns an iterable that should work with rename
        // We'll use call_method directly instead of template rendering
        let rename_args = {
            // Create the mapped column names list by iterating the tuple
            let mut mapped_names = Vec::new();
            let iter = column_names.try_iter().unwrap();
            for name in iter {
                if let Some(s) = name.as_str() {
                    mapped_names.push(Value::from(s.to_lowercase()));
                }
            }
            Value::from_iter(mapped_names)
        };

        // Call rename with the mapped column names (this should not error)
        let result = table_val.call_method(&state, "rename", &[rename_args], &[]);

        // Verify it doesn't throw an error about "column_names must be a map or an array"
        assert!(
            result.is_ok(),
            "rename should accept iterable from map filter"
        );

        // Verify the columns were renamed
        let renamed_table = result.unwrap().downcast_object::<AgateTable>().unwrap();
        assert_eq!(renamed_table.column_name(0).unwrap(), "id");
        assert_eq!(renamed_table.column_name(1).unwrap(), "country");
    }

    #[test]
    fn test_rename_rejects_each_slug_flag_independently() {
        let table = AgateTable::from_record_batch(simple_record_batch());
        let err = table.rename(None, None, true, false).unwrap_err();
        assert!(err.to_string().contains("slugging columns or rows"));

        let err = table.rename(None, None, false, true).unwrap_err();
        assert!(err.to_string().contains("slugging columns or rows"));
    }

    fn color_table() -> AgateTable {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, true),
            Field::new("color", DataType::Utf8, true),
            Field::new("value", DataType::Int32, true),
        ]));
        let id_array: ArrayRef = Arc::new(Int32Array::new(vec![1, 2, 3, 4, 5, 6].into(), None));
        let color_array: ArrayRef = Arc::new(StringArray::from(vec![
            Some("red"),
            Some("blue"),
            Some("red"),
            Some("green"),
            Some("blue"),
            Some("red"),
        ]));
        let value_array: ArrayRef =
            Arc::new(Int32Array::new(vec![10, 20, 30, 40, 50, 60].into(), None));
        let batch = RecordBatch::try_new(schema, vec![id_array, color_array, value_array]).unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    fn nullable_and_wide_table() -> AgateTable {
        let schema = Arc::new(Schema::new(vec![
            Field::new("n", DataType::Utf8, true),
            Field::new("word", DataType::Utf8, true),
        ]));
        let nullable_array: ArrayRef = Arc::new(StringArray::from(vec![None, Some("plain")]));
        let word_array: ArrayRef = Arc::new(StringArray::from(vec![Some("violet"), Some("green")]));
        let batch = RecordBatch::try_new(schema, vec![nullable_array, word_array]).unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    #[test]
    fn test_grouper() {
        let table = color_table();
        let grouper = table.grouper(&["color".to_string()]).unwrap();
        let mut groups = grouper.iter();
        assert_eq!(groups.next().unwrap(), 0); // red
        assert_eq!(groups.next().unwrap(), 1); // blue
        assert_eq!(groups.next().unwrap(), 0); // red
        assert_eq!(groups.next().unwrap(), 2); // green
        assert_eq!(groups.next().unwrap(), 1); // blue
        assert_eq!(groups.next().unwrap(), 0); // red
        assert_eq!(groups.next(), None);
    }

    #[test]
    fn test_print_table_truncates_rows_columns_and_width() {
        let table = color_table();
        let printed = table.print_table_to_string(2, 2, 5).unwrap();
        assert_eq!(
            printed,
            "\
| id | color | ... |
| -- | ----- | --- |
|  1 | red   | ... |
|  2 | blue  | ... |
| ... | ...   | ... |
"
        );

        assert_eq!(
            table
                .display()
                .with_max_rows(2)
                .with_max_columns(2)
                .with_max_column_width(5)
                .to_string(),
            printed
        );

        let mut output = Vec::new();
        table.print_table(1, 1, 4, Some(&mut output)).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("| id | ... |"));
        assert!(output.contains("|  1 | ... |"));
    }

    #[test]
    fn test_print_table_keeps_boundary_limits_and_formats_nulls() {
        let table = color_table();
        let printed = table.print_table_to_string(6, 3, 20).unwrap();
        assert_eq!(
            printed,
            "\
| id | color | value |
| -- | ----- | ----- |
|  1 | red   |    10 |
|  2 | blue  |    20 |
|  3 | red   |    30 |
|  4 | green |    40 |
|  5 | blue  |    50 |
|  6 | red   |    60 |
"
        );
        assert!(!printed.contains("..."));

        let printed = nullable_and_wide_table()
            .print_table_to_string(2, 2, 5)
            .unwrap();
        // Both `n` and `word` are text columns, so their headers are now
        // left-justified like their data (see #13635) rather than
        // unconditionally right-justified.
        assert_eq!(
            printed,
            "\
| n     | word  |
| ----- | ----- |
|       | vi... |
| plain | green |
"
        );
        assert!(!printed.contains("none"));
    }

    #[test]
    fn test_group_by() {
        let env = Environment::new();
        let state = env.empty_state();
        let table = Value::from_object(color_table());

        let table_set = table
            .call_method(&state, "group_by", &[Value::from("color")], &[])
            .unwrap();
        let keys = table_set.call_method(&state, "keys", &[], &[]).unwrap();
        assert_eq!(keys.get_item_by_index(0).unwrap(), Value::from("red"));
        assert_eq!(keys.get_item_by_index(1).unwrap(), Value::from("blue"));
        assert_eq!(keys.get_item_by_index(2).unwrap(), Value::from("green"));

        let mut groups = table_set.try_iter().unwrap();
        // each group Value is an AgateTable
        let (red, blue, green) = (
            groups.next().unwrap(),
            groups.next().unwrap(),
            groups.next().unwrap(),
        );
        let mut iter = red.try_iter().unwrap();
        assert_eq!(
            "<agate.Row: (1, red, 10)>",
            iter.next().unwrap().to_string()
        );
        assert_eq!(
            "<agate.Row: (3, red, 30)>",
            iter.next().unwrap().to_string()
        );
        assert_eq!(
            "<agate.Row: (6, red, 60)>",
            iter.next().unwrap().to_string()
        );
        assert!(iter.next().is_none());
        let mut iter = blue.try_iter().unwrap();
        assert_eq!(
            "<agate.Row: (2, blue, 20)>",
            iter.next().unwrap().to_string()
        );
        assert_eq!(
            "<agate.Row: (5, blue, 50)>",
            iter.next().unwrap().to_string()
        );
        assert!(iter.next().is_none());
        let mut iter = green.try_iter().unwrap();
        assert_eq!(
            "<agate.Row: (4, green, 40)>",
            iter.next().unwrap().to_string()
        );
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_distinct() {
        let env = Environment::new();
        let state = env.empty_state();
        let table = Value::from_object(color_table());

        let result = table
            .call_method(&state, "distinct", &[Value::from("color")], &[])
            .unwrap();

        let distinct_table = result.downcast_object::<AgateTable>().unwrap();

        // Verify all columns are returned
        assert_eq!(distinct_table.column_name(0).unwrap(), "id");
        assert_eq!(distinct_table.column_name(1).unwrap(), "color");
        assert_eq!(distinct_table.column_name(2).unwrap(), "value");

        // Verify that there are only three rows: red, blue, and green
        let cols = distinct_table.columns().values();
        let color = cols.get(1).unwrap();
        assert_eq!(color.len(), Some(3));
        assert_eq!(color.get_item_by_index(0).unwrap().as_str().unwrap(), "red");
        assert_eq!(
            color.get_item_by_index(1).unwrap().as_str().unwrap(),
            "blue"
        );
        assert_eq!(
            color.get_item_by_index(2).unwrap().as_str().unwrap(),
            "green"
        );

        let result = table
            .call_method(
                &state,
                "distinct",
                &[Value::from_iter(vec!["id", "color", "value"])],
                &[],
            )
            .unwrap();

        let distinct_table = result.downcast_object::<AgateTable>().unwrap();

        assert_eq!(distinct_table.num_rows(), 6);

        let result = table
            .call_method(&state, "distinct", &[Value::NONE], &[])
            .unwrap();

        let distinct_table = result.downcast_object::<AgateTable>().unwrap();

        assert_eq!(distinct_table.num_rows(), 6);
    }

    #[test]
    fn column_indices_of_accepts_names_indices_and_sequences() {
        // columns: 0 = "id", 1 = "country"
        let table = AgateTable::from_record_batch(simple_record_batch());
        let indices = |keys: Value| table.column_indices_of(&keys);

        // single column name and single column index
        assert_eq!(indices(Value::from("country")).unwrap(), vec![1]);
        assert_eq!(indices(Value::from(1)).unwrap(), vec![1]);
        // negative indices count from the end
        assert_eq!(indices(Value::from(-1)).unwrap(), vec![1]);

        // sequences keep the order of the keys, not the order of the columns
        assert_eq!(
            indices(Value::from_iter(["country", "id"])).unwrap(),
            vec![1, 0]
        );
        assert_eq!(indices(Value::from_iter([1, 0])).unwrap(), vec![1, 0]);
        // names and indices can be mixed
        assert_eq!(
            indices(Value::from_iter([Value::from("country"), Value::from(0)])).unwrap(),
            vec![1, 0]
        );

        // unknown names and out-of-range indices are skipped
        assert_eq!(
            indices(Value::from_iter(["country", "nope"])).unwrap(),
            vec![1]
        );
        assert_eq!(indices(Value::from_iter([0, 7])).unwrap(), vec![0]);
        assert_eq!(indices(Value::from("nope")).unwrap(), Vec::<usize>::new());

        // anything that is not a column name or index is an error
        let err = indices(Value::from(1.5)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    }

    /// The `Table.join` method as a macro would call it: through `call_method`, which
    /// parses the arguments and turns `inner`/`full_outer` into a [JoinType].
    fn call_join(
        table: &Value,
        args: &[Value],
        kwargs: &[(&str, Value)],
    ) -> Result<Value, minijinja::Error> {
        let env = Environment::new();
        let state = env.empty_state();
        let args = if kwargs.is_empty() {
            args.to_vec()
        } else {
            let mut args = args.to_vec();
            args.push(Value::from(Kwargs::from_iter(kwargs.to_vec())));
            args
        };
        table.call_method(&state, "join", args.as_slice(), &[])
    }

    fn joined_rows(result: Result<Value, minijinja::Error>) -> Vec<String> {
        let table = result.unwrap().downcast_object::<AgateTable>().unwrap();
        stringly_rows_of(&table, "|")
    }

    #[test]
    fn call_join_accepts_keyword_arguments() {
        let left = main_table(&[1, 2], &["a", "b"], &[Some(10), Some(20)]).into_value();
        let right = related_table(&[Some(10)], &["The Sign of the Four"]).into_value();

        let joined = call_join(
            &left,
            &[right],
            &[
                ("left_key", Value::from("fk")),
                ("right_key", Value::from("id")),
            ],
        );
        assert_eq!(
            joined_rows(joined),
            vec!["1|a|10|The Sign of the Four", "2|b|20|None"]
        );
    }

    #[test]
    fn call_join_accepts_positional_arguments() {
        let left = main_table(&[1, 2], &["a", "b"], &[Some(10), Some(20)]).into_value();
        let right = related_table(&[Some(10)], &["The Sign of the Four"]).into_value();

        // right_table, left_key, right_key, inner
        let joined = call_join(
            &left,
            &[
                right,
                Value::from("fk"),
                Value::from("id"),
                Value::from(true),
            ],
            &[],
        );
        assert_eq!(joined_rows(joined), vec!["1|a|10|The Sign of the Four"]);
    }

    #[test]
    fn call_join_rejects_a_join_that_is_both_inner_and_full_outer() {
        let left = main_table(&[1], &["a"], &[Some(10)]).into_value();
        let right = related_table(&[Some(10)], &["The Sign of the Four"]).into_value();

        let err = call_join(
            &left,
            &[right],
            &[
                ("left_key", Value::from("fk")),
                ("right_key", Value::from("id")),
                ("inner", Value::from(true)),
                ("full_outer", Value::from(true)),
            ],
        )
        .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
        assert!(
            err.to_string()
                .contains(r#"A join can not be both "inner" and "full_outer"."#),
            "{err}"
        );
    }

    #[test]
    fn call_join_rejects_a_right_table_that_is_not_a_table() {
        let left = main_table(&[1], &["a"], &[Some(10)]).into_value();

        let err = call_join(&left, &[Value::from("not a table")], &[]).unwrap_err();

        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
        assert!(
            err.to_string()
                .contains("Table.join: right_table must be a Table"),
            "{err}"
        );
    }
}
