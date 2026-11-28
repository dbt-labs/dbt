use std::cmp::Ordering;
use std::hint::unreachable_unchecked;
use std::io;
use std::rc::Rc;
use std::sync::Arc;

use arrow::array::StringViewBuilder;
use arrow::compute::TakeOptions;
use arrow::record_batch::RecordBatch;
use arrow_array::{Array, StringViewArray, UInt64Array};
use arrow_schema::{ArrowError, Schema};
use minijinja::arg_utils::ArgsIter;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{DynObject, Enumerator, Kwargs, Object, ValueMap, mutable_map::MutableMap};
use minijinja::{Error, ErrorKind, State, Value};

use crate::Tuple;
use crate::column::Column;
use crate::columns::*;
use crate::flat_record_batch::FlatRecordBatch;
use crate::grouper::Grouper;
use crate::join::JoinType;
use crate::print_table::TableDisplay;
use crate::rows::*;
use crate::table_repr::TableRepr;
use crate::table_set::{TableSet, TableSetRepr};

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
        Self::from_record_batch_impl(Arc::clone(&batch), Some(batch), row_names)
    }

    /// Create an AgateTable from an Arrow RecordBatch.
    pub fn from_record_batch(batch: Arc<RecordBatch>) -> Self {
        Self::from_record_batch_impl(Arc::clone(&batch), Some(batch), None)
    }

    /// Constructor for callers that do their own batch transformation but
    /// still need to keep the original [RecordBatch] around.
    pub fn from_record_batch_impl(
        batch: Arc<RecordBatch>,
        original: Option<Arc<RecordBatch>>,
        row_names: Option<Arc<StringViewArray>>,
    ) -> Self {
        let flat = FlatRecordBatch::try_new_impl(batch, original).unwrap();
        let repr = TableRepr::new(Arc::new(flat), None, row_names);
        Self::from_repr(Arc::new(repr))
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

    #[cfg(test)]
    pub(crate) fn repr(&self) -> &TableRepr {
        self.repr.as_ref()
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

    /// A new table with the first `n` rows: Python's `table.limit(n)`.
    ///
    /// Like the slice `rows[:n]`, a negative `n` drops the last `-n` rows.
    pub fn limit(&self, n: i64) -> Result<AgateTable, ArrowError> {
        self.slice(None, Some(n), None)
    }

    /// A new table with the rows `rows[start:stop:step]`, with Python's slice
    /// semantics. Row names, if any, are sliced the same way.
    pub fn slice(
        &self,
        start: Option<i64>,
        stop: Option<i64>,
        step: Option<i64>,
    ) -> Result<AgateTable, ArrowError> {
        let repr = self.repr.slice(start, stop, step)?;
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
        let column_indices = self
            .repr
            .column_indices(&column_names)
            .collect::<Vec<usize>>();

        let repr = self
            .repr
            .distinct(&column_indices)
            .map_err(|e| Error::new(ErrorKind::InvalidOperation, format!("Table.distinct: {e}")))?;

        Ok(AgateTable::from_repr(repr))
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

    /// Compare tables by identity
    ///
    /// `agate.Table` defines no `__eq__`, so Python falls back to
    /// `object.__eq__` and two tables holding identical data are never equal.
    /// Note, `agate.Row`/`Column` do define value equality, which makes the
    /// table case unintuitive
    fn custom_cmp(self: &Arc<Self>, other: &DynObject) -> Option<Ordering> {
        let this: *const AgateTable = Arc::as_ptr(self);
        let other: *const AgateTable = other.downcast_ref::<AgateTable>()?;
        Some(this.cmp(&other))
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        fn invalid_operation(method: &str, e: &ArrowError) -> Error {
            Error::new(ErrorKind::InvalidOperation, format!("{method}: {e}"))
        }

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
            //     Create a new table with fewer rows.
            //
            //     See also: Python's builtin :func:`slice`.
            //
            //     :param start_or_stop:
            //         If the only argument, then how many rows to include, otherwise,
            //         the index of the first row to include.
            //     :param stop:
            //         The index of the last row to include.
            //     :param step:
            //         The size of the jump between rows to include. (`step=2` will return
            //         every other row.)
            //     :returns:
            //         A new :class:`.Table`.
            //     """
            //     if stop or step:
            //         s = slice(start_or_stop, stop, step)
            //     else:
            //         s = slice(start_or_stop)
            // ```
            "limit" => {
                let iter = ArgsIter::new("Table.limit", &[], args);
                let start_or_stop = iter.next_kwarg::<Option<i64>>("start_or_stop")?;
                let stop = iter.next_kwarg::<Option<i64>>("stop")?;
                let step = iter.next_kwarg::<Option<i64>>("step")?;
                iter.finish()?;

                let is_truthy = |v: Option<i64>| v.is_some_and(|v| v != 0);
                let table = {
                    if is_truthy(stop) || is_truthy(step) {
                        self.slice(start_or_stop, stop, step)
                    } else {
                        self.slice(None, start_or_stop, None)
                    }
                }
                .map_err(|e| invalid_operation("Table.limit", &e))?;
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
            "__eq__" | "__ne__" => Err(Error::from(ErrorKind::UnknownMethod)),
            // TODO: add handling for `__[lt | le | gt | ge]__`
            other => unimplemented!("AgateTable::{}", other),
        }
    }
}

#[cfg(test)]
mod tests;
