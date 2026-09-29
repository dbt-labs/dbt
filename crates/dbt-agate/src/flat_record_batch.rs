//! Arrow record batch with only flat columns
//!
//!

use arrow::array::PrimitiveBuilder;
use arrow::array::{
    Array, DictionaryArray, GenericListArray, OffsetSizeTrait, RecordBatch, RecordBatchOptions,
    StructArray,
};
use arrow::compute::CastOptions;
use arrow::datatypes::Int64Type;
use arrow_data::ArrayData;
use arrow_schema::{ArrowError, DataType, Field, Schema};
use core::fmt;
use std::collections::HashSet;
use std::ops::Deref;
use std::sync::Arc;

use crate::converters::{ArrayConverter, make_array_converter};

/// Metadata key used to store the Agate data type associated with an Arrow array.
///
/// "Text"          Data representing text.
/// "Number"        Data representing numbers.
/// "Boolean"       Data representing true and false.
/// "Date"          Data representing dates alone.
/// "DateTime"      Data representing dates with times.
/// "TimeDelta"     Data representing the interval between two dates and/or times.
const AGATE_DTYPE_METADATA_KEY: &str = "AGATE:dtype";

/// Takes an Arrow struct array and flattens nested columns into separate columns.
///
/// Example:
///
///     (col0: int64, col1: struct<a: utf8, b: bool>) ->
///       (col0: int64, col1/a: utf8, col1/b: bool)
///
/// This follows the way Agate tables convert structs and arrays into columns of
/// primitive types [1].
///
/// [1] https://agate.readthedocs.io/en/latest/api/table.html#agate.Table.from_object
fn flatten_record_batch_columns(batch: &RecordBatch) -> RecordBatch {
    let state = FlattenRecordBatchState::new(batch);
    state.try_finalize().unwrap()
}

struct FlattenRecordBatchState {
    new_fields: Vec<Field>,
    new_columns: Vec<Arc<dyn Array>>,
    /// Stack for recursively flattening nested columns.
    stack: Vec<(Field, Arc<dyn Array>)>,
}

impl FlattenRecordBatchState {
    pub fn new(batch: &RecordBatch) -> Self {
        let ncols = batch.num_columns();
        let mut state = Self {
            new_fields: Vec::with_capacity(ncols),
            new_columns: Vec::with_capacity(ncols),
            stack: Vec::with_capacity(ncols),
        };

        // initial push of batch columns to be processed onto the stack
        let schema = batch.schema();
        for i in (0..batch.num_columns()).rev() {
            let field = schema.field(i).clone();
            let array = batch.column(i).clone();
            state.stack.push((field, array));
        }
        state
    }

    pub fn from_single_column(field: Field, column: Arc<dyn Array>) -> Self {
        let mut state = Self {
            new_fields: Vec::with_capacity(1),
            new_columns: Vec::with_capacity(1),
            stack: Vec::with_capacity(1),
        };

        // initial push of column to be processed onto the stack
        state.stack.push((field, column));
        state
    }

    /// Push a flat column to the new fields and columns.
    pub fn emit_flat_col(
        &mut self,
        field: Field,
        agate_dtype_name: Option<&str>,
        column: Arc<dyn Array>,
    ) {
        let mut field = field;
        if let Some(agate_dtype_name) = agate_dtype_name {
            let mut new_field_metadata = field.metadata().clone();
            new_field_metadata.insert(
                AGATE_DTYPE_METADATA_KEY.to_string(),
                agate_dtype_name.to_string(),
            );
            field.set_metadata(new_field_metadata);
        }
        self.new_fields.push(field);
        self.new_columns.push(column);
    }

    fn flatten_list_column<O: OffsetSizeTrait + fmt::Display>(
        &mut self,
        field: &Field,
        inner_field: &Field,
        list_array: &GenericListArray<O>,
    ) {
        let (min_size, mut max_size) = {
            if list_array.is_empty() {
                (O::zero(), O::zero())
            } else {
                Self::list_size_range::<O>(list_array)
            }
        };
        // If max_size is 0, we push an empty or all-NULL collumn of the inner type.
        if max_size == O::zero() {
            max_size = O::one();
        }
        let mut i = max_size.sub(O::one());
        while i >= O::zero() {
            let nullable = inner_field.is_nullable() || i >= min_size;
            let sub_field = inner_field
                .clone()
                .with_name(format!("{}.{}", field.name(), i))
                .with_nullable(nullable);
            let (ith_field, ith_column) = Self::ith_column_from_list::<O>(sub_field, list_array, i);
            // push to the recursion stack since flattening is recursive
            self.stack.push((ith_field, ith_column));
            i = i.sub(O::one());
        }
    }

    /// Take one field from the stack, flatten it, and push the flattened
    /// fields/columns to the new fields/columns.
    ///
    /// pre-condition: stack is not empty
    pub fn iterate(&mut self) {
        let (field, column) = self.stack.pop().unwrap();
        match column.data_type() {
            // XXX: agate doesn't have the type Null, so we default to "Text" (i.e. an all-NULL text column)
            DataType::Null => self.emit_flat_col(field, Some("Text"), column.clone()),
            DataType::Boolean => self.emit_flat_col(field, Some("Boolean"), column.clone()),
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Decimal32(_, _)
            | DataType::Decimal64(_, _)
            | DataType::Decimal128(_, _)
            | DataType::Decimal256(_, _) => {
                self.emit_flat_col(field, Some("Number"), column.clone())
            }
            DataType::Timestamp(_, _) => {
                self.emit_flat_col(field, Some("DateTime"), column.clone())
            }
            DataType::Date32 | DataType::Date64 => {
                self.emit_flat_col(field, Some("Date"), column.clone())
            }
            DataType::Time32(_)
            | DataType::Time64(_)
            | DataType::Duration(_)
            | DataType::Interval(_) => self.emit_flat_col(field, Some("TimeDelta"), column.clone()),
            // XXX: "Text" is used for binary and string types because agate doesn't have "Binary"
            DataType::Binary
            | DataType::FixedSizeBinary(_)
            | DataType::LargeBinary
            | DataType::BinaryView
            | DataType::Utf8
            | DataType::LargeUtf8
            | DataType::Utf8View => self.emit_flat_col(field, Some("Text"), column.clone()),
            // List-typed columns are flattened into multiple columns (one for each element in the
            // list value). Since not all list values have the same number of elements, we pad the
            // shorter ones with NULLs.
            DataType::List(inner_field) => {
                let list_array = column
                    .as_any()
                    .downcast_ref::<GenericListArray<i32>>()
                    .unwrap();
                self.flatten_list_column::<i32>(&field, inner_field, list_array);
            }
            DataType::LargeList(inner_field) => {
                let list_array = column
                    .as_any()
                    .downcast_ref::<GenericListArray<i64>>()
                    .unwrap();
                self.flatten_list_column::<i64>(&field, inner_field, list_array);
            }
            // TODO: list-views and fixed-size-list should be handled just like lists
            DataType::ListView(_) | DataType::LargeListView(_) | DataType::FixedSizeList(_, _) => {
                self.emit_flat_col(field, None, column.clone())
            }
            // Each struct field is flattened into its own column.
            DataType::Struct(fields) => {
                let struct_array = column.as_any().downcast_ref::<StructArray>().unwrap();
                for i in (0..fields.len()).rev() {
                    let sub_field = fields[i].as_ref().clone().with_name(format!(
                        "{}/{}",
                        field.name(),
                        fields[i].name()
                    ));
                    let sub_array = struct_array.column(i);
                    self.stack.push((sub_field, sub_array.clone()));
                }
            }
            // Flattening union columns is impossible, forward them as flat columns
            DataType::Union(_, _) => self.emit_flat_col(field, None, column.clone()),
            DataType::Dictionary(indices_type, value_type) => {
                self.flatten_dict_encoded_column(field, indices_type, value_type, &column)
            }
            // No way to flatten map columns, forward them as flat columns
            DataType::Map(_, _) => self.emit_flat_col(field, None, column.clone()),
            // REE arrays are not very common yet, so we just forward them as flat columns
            DataType::RunEndEncoded(_, _) => self.emit_flat_col(field, None, column.clone()),
        }
    }

    fn flatten_dict_encoded_column(
        &mut self,
        field: Field,
        _indices_type: &DataType,
        value_type: &DataType,
        column: &Arc<dyn Array>,
    ) {
        match value_type {
            // XXX: agate doesn't have the type Null, so we default to "Text" (i.e. an all-NULL text column)
            DataType::Null => self.emit_flat_col(field, Some("Text"), column.clone()),
            DataType::Boolean => self.emit_flat_col(field, Some("Boolean"), column.clone()),
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Float16
            | DataType::Float32
            | DataType::Float64
            | DataType::Decimal32(_, _)
            | DataType::Decimal64(_, _)
            | DataType::Decimal128(_, _)
            | DataType::Decimal256(_, _) => {
                self.emit_flat_col(field, Some("Number"), column.clone())
            }
            DataType::Timestamp(_, _) => {
                self.emit_flat_col(field, Some("DateTime"), column.clone())
            }
            DataType::Date32 | DataType::Date64 => {
                self.emit_flat_col(field, Some("Date"), column.clone())
            }
            DataType::Time32(_)
            | DataType::Time64(_)
            | DataType::Duration(_)
            | DataType::Interval(_) => self.emit_flat_col(field, Some("TimeDelta"), column.clone()),
            // XXX: "Text" is used for binary and string types because agate doesn't have "Binary"
            DataType::Binary
            | DataType::FixedSizeBinary(_)
            | DataType::LargeBinary
            | DataType::BinaryView
            | DataType::Utf8
            | DataType::LargeUtf8
            | DataType::Utf8View => self.emit_flat_col(field, Some("Text"), column.clone()),
            // Dictionary-encoded nested types are flattened and then re-encoded as
            // dictionary-encoded columns with the same indices as the original column.
            DataType::List(_)
            | DataType::LargeList(_)
            | DataType::ListView(_)
            | DataType::LargeListView(_)
            | DataType::FixedSizeList(_, _)
            | DataType::Struct(_)
            | DataType::Union(_, _)
            | DataType::Map(_, _)
            | DataType::RunEndEncoded(_, _) => {
                // Get the array of the dictionary values
                let dictionary_values =
                    arrow::array::make_array(column.to_data().child_data()[0].clone());

                // Flatten the batch made of a single column -- the dictionary values.
                // The column name is the same name as the original dictionary-encoded
                // column.
                let inner_batch = {
                    let state = FlattenRecordBatchState::from_single_column(
                        field.with_data_type(dictionary_values.data_type().clone()),
                        dictionary_values.clone(),
                    );
                    state.try_finalize().unwrap()
                };

                // Now for every column in the flattened inner batch, we need to
                // rebuild a dictionary-encoded column using the same indices as
                // the original column.
                let inner_schema = inner_batch.schema();
                let original_data = column.to_data();
                for i in (0..inner_batch.num_columns()).rev() {
                    // Get a new dictionary-encoded column with the same indices
                    let inner_column =
                        dict_encoded_with_same_indices(&original_data, inner_batch.column(i))
                            .unwrap();
                    // Use the dictionary-encoded column data type for the new field
                    let inner_field = inner_schema
                        .field(i)
                        .clone()
                        .with_data_type(inner_column.data_type().clone());
                    // Keep recursively flattening. This will eliminate possible
                    // nested dictionary-encoded columns.
                    self.stack.push((inner_field, inner_column));
                }
            }
            // When we see a dictionary-encoded dictionary, we must expand the inner
            // dictionary and avoid nested dictionary-encoded columns.
            DataType::Dictionary(_, inner_value_type) => {
                // Decode the dictionary-encoded column to get the dictionary values...
                let decoded = {
                    let cast_options = CastOptions::default();
                    arrow::compute::cast_with_options(
                        column.as_ref(),
                        inner_value_type.as_ref(),
                        &cast_options,
                    )
                    .unwrap()
                };
                // ...and recursively flatten the decoded column.
                let field = field.with_data_type(decoded.data_type().clone());
                self.stack.push((field, decoded));
            }
        }
    }

    pub fn try_finalize(self) -> Result<RecordBatch, ArrowError> {
        let mut state = self;
        // consume the stack and push flattened fields/columns to new_fields/new_columns
        while !state.stack.is_empty() {
            state.iterate();
        }

        // Agate column names are expected to be unique (the last ones override the previous ones).
        //
        // A pre-existing flat "a/b" column will be overridden by a struct column "a" with a field
        // "b" (and vice-versa). So we need to remove duplicates from the new fields/columns
        // *after* the flattening process.
        let len = state.new_fields.len();
        let mut seen: HashSet<String> = HashSet::new();
        for i in (0..len).rev() {
            let name = state.new_fields[i].name();
            if seen.contains(name) {
                state.new_fields.remove(i);
                state.new_columns.remove(i);
            } else {
                seen.insert(name.to_string());
            }
        }

        let new_schema = Arc::new(Schema::new(state.new_fields));
        let row_count = state
            .new_columns
            .first()
            .map(|array| array.len())
            .unwrap_or(0);
        let options = RecordBatchOptions::default().with_row_count(Some(row_count));
        RecordBatch::try_new_with_options(new_schema, state.new_columns, &options)
    }

    /// Minimum and maximum size of the list elements in a list array.
    ///
    /// PRE-CONDITION: the list array is not empty.
    fn list_size_range<O: OffsetSizeTrait>(list_array: &GenericListArray<O>) -> (O, O) {
        debug_assert!(
            !list_array.is_empty(),
            "list_size_range called on an empty list array"
        );
        let len = list_array.len();
        let nulls = list_array.nulls();
        let offsets = list_array.value_offsets();
        let mut min_size = usize::MAX;
        let mut max_size = 0usize;
        for i in 0..len {
            let start = offsets[i];
            let end = offsets[i + 1];
            let is_valid = nulls.map(|nulls| nulls.is_valid(i)).unwrap_or(true);
            let size = if is_valid {
                (end - start).as_usize()
            } else {
                0
            };
            if size < min_size {
                min_size = size;
            }
            if size > max_size {
                max_size = size;
            }
        }
        (
            O::from_usize(min_size).unwrap(),
            O::from_usize(max_size).unwrap(),
        )
    }

    /// Build a column from a list array by taking the i-th element of each list.
    ///
    /// The best (efficient and easy) way to do this is to use a DictionaryArray. This lets us
    /// re-use the same dictionary (the inner values of the list array) for all the flattened columns.
    fn ith_column_from_list<O: OffsetSizeTrait>(
        sub_field: Field,
        list_array: &GenericListArray<O>,
        idx: O,
    ) -> (Field, Arc<dyn Array>) {
        let list_len = list_array.len();
        let list_nulls = list_array.nulls();
        let list_offsets = list_array.value_offsets();

        let mut keys_builder = PrimitiveBuilder::<Int64Type>::with_capacity(list_len);
        let values = list_array.values().clone();
        let values_nulls = values.nulls();

        for list_idx in 0..list_len {
            // if list_array[list_idx] is NULL, eval list_array[list_idx][idx] as NULL
            let list_is_valid = list_nulls
                .map(|nulls| nulls.is_valid(list_idx))
                .unwrap_or(true);
            if !list_is_valid {
                keys_builder.append_null();
                continue;
            }

            // if idx >= list_array[list_idx].len(), eval list_array[list_idx][idx] as NULL
            let start = list_offsets[list_idx];
            let end = list_offsets[list_idx + 1];
            let values_idx = start + idx;
            if values_idx >= end {
                keys_builder.append_null();
                continue;
            }

            // now check if the list_array[list_idx][idx] is NULL itself
            let is_valid = values_nulls
                .map(|nulls| nulls.is_valid(values_idx.as_usize()))
                .unwrap_or(true);
            if !is_valid {
                keys_builder.append_null();
                continue;
            }

            keys_builder.append_value(values_idx.as_usize() as i64);
        }

        let keys = keys_builder.finish();
        let dictionary = unsafe { DictionaryArray::<Int64Type>::new_unchecked(keys, values) };
        let sub_field = sub_field.with_data_type(dictionary.data_type().clone());
        (sub_field, Arc::new(dictionary) as Arc<dyn Array>)
    }
}

/// Create a new dictionary-encoded array with the same indices as the original
/// dictionary-encoded array but with new dictionary values.
///
/// PRE-CONDITION: The new values must have the same length as the original
/// dictionary values so that all the indices in the original array remain valid.
fn dict_encoded_with_same_indices(
    original_dict_encoded: &ArrayData,
    new_values: &Arc<dyn Array + 'static>,
) -> Result<Arc<dyn Array + 'static>, ArrowError> {
    debug_assert!(
        original_dict_encoded.child_data()[0].len() == new_values.len(),
        "new_values must have the same length as the original dictionary values"
    );
    let indices_type = match original_dict_encoded.data_type() {
        DataType::Dictionary(indices_type, _) => indices_type.clone(),
        _ => unreachable!(),
    };
    let values_type = Box::new(new_values.data_type().clone());
    let new_type = DataType::Dictionary(indices_type, values_type);
    let new_data = ArrayData::try_new(
        new_type,
        original_dict_encoded.len(),
        original_dict_encoded
            .nulls()
            .cloned()
            .map(|n| n.into_inner().into_inner()),
        original_dict_encoded.offset(),
        original_dict_encoded.buffers().to_vec(),
        vec![new_values.to_data()], // replace the child data (the dictionary values) with the new values
    )?;

    let new_dict_array = arrow::array::make_array(new_data);
    Ok(new_dict_array)
}

/// Wrapper on an Arrow RecordBatch of flat (non-nested) columns.
///
/// The original batch is kept around for troubleshooting and
/// future needs of data provenance. Buffers are shared between
/// the two instances so this doesn't require much more memory
/// than if we were storing a single batch.
pub(crate) struct FlatRecordBatch {
    /// Flat record batch.
    flat: Arc<RecordBatch>,
    /// Agate [DataType]s for each column in the flat record batch.
    data_types: Vec<crate::DataType>,
    /// The original record batch before the flattening of nested columns.
    original: Option<Arc<RecordBatch>>,
    /// Array converters for each column in the flat record batch.
    ///
    /// These are pre-built converters allow converting any value
    /// in the flat record batch to a minijinja value.
    converters: Vec<Box<dyn ArrayConverter>>,
}

impl FlatRecordBatch {
    pub fn try_new(batch: Arc<RecordBatch>) -> Result<Self, ArrowError> {
        let flat = Arc::new(flatten_record_batch_columns(batch.as_ref()));
        Self::_from_flattened_record_batch(flat, Some(batch))
    }

    pub(crate) fn _from_flattened_record_batch(
        flat: Arc<RecordBatch>,
        original: Option<Arc<RecordBatch>>,
    ) -> Result<Self, ArrowError> {
        let data_types = flat
            .schema()
            .fields()
            .iter()
            .map(|field| {
                let dtype_name = field
                    .metadata()
                    .get(AGATE_DTYPE_METADATA_KEY)
                    .cloned()
                    // TODO(felipecrv): stop relying on this default by ensuring all
                    // _from_flattened_record_batch callers provide proper metadata
                    .unwrap_or_else(|| "Text".to_string());
                crate::DataType::new(dtype_name)
            })
            .collect::<Vec<_>>();
        let converters = flat
            .columns()
            .iter()
            .map(|array| make_array_converter(&**array))
            .collect::<Result<Vec<_>, ArrowError>>()?;
        Ok(Self {
            flat,
            data_types,
            original,
            converters,
        })
    }

    /// The inner [RecordBatch] with only flat columns.
    pub fn inner(&self) -> &Arc<RecordBatch> {
        &self.flat
    }

    /// The original [RecordBatch] before flattening nested columns, if available.
    ///
    /// Some operations (e.g. [FlatRecordBatch::with_single_column] create a [FlatRecordBatch]
    /// without keeping the original batch. The "original" batch only applies to the first
    /// flattening operation performed by [FlatRecordBatch::try_new].
    pub fn original(&self) -> Option<&Arc<RecordBatch>> {
        self.original.as_ref()
    }

    pub fn with_single_column(&self, idx: usize) -> Arc<FlatRecordBatch> {
        let column_batch = single_column_batch(&self.flat, idx);
        // one column pulled from an already flat batch is still flat, so .unwrap() is safe
        let flat = Self::_from_flattened_record_batch(Arc::new(column_batch), None).unwrap();
        Arc::new(flat)
    }

    pub(crate) fn with_renamed_columns(&self, renamed_columns: &[String]) -> Arc<FlatRecordBatch> {
        debug_assert!(renamed_columns.len() == self.flat.num_columns());
        let new_schema = {
            let mut renamed_columns = renamed_columns.iter();
            let mut new_fields = Vec::with_capacity(self.flat.num_columns());
            for field in self.flat.schema().fields().iter() {
                let new_name = renamed_columns.next().unwrap(); // pre-condition ensures this is safe
                let new_field = (**field).clone().with_name(new_name.clone());
                new_fields.push(new_field);
            }
            Arc::new(Schema::new(new_fields))
        };
        // only column names changed, so .unwrap() is safe
        let new_flat = {
            let new_columns = self.flat.columns().to_vec();
            let options = RecordBatchOptions::default().with_row_count(Some(self.flat.num_rows()));
            RecordBatch::try_new_with_options(new_schema, new_columns, &options)
        }
        .unwrap();
        Arc::new(Self::_from_flattened_record_batch(Arc::new(new_flat), None).unwrap())
    }

    pub fn column_types(&self) -> &[crate::DataType] {
        self.data_types.as_slice()
    }

    pub fn column_names(&self) -> impl Iterator<Item = &String> + '_ {
        self.flat
            .schema_ref()
            .fields()
            .iter()
            .map(|field| field.name())
    }

    pub fn top_level_column_names(&self) -> impl Iterator<Item = &String> + '_ {
        self.original
            .as_ref()
            .unwrap_or(&self.flat)
            .schema_ref()
            .fields()
            .iter()
            .map(|field| field.name())
    }

    pub fn column_type(&self, idx: usize) -> &crate::DataType {
        &self.data_types[idx]
    }

    pub fn column_name(&self, idx: usize) -> &String {
        self.flat.schema_ref().field(idx).name()
    }

    /// Create a new [FlatRecordBatch] by selecting a subset of columns from the current one.
    ///
    /// PRE-CONDITION: the indices are valid for the current batch.
    pub(crate) fn select<'a>(
        &'a self,
        indices: impl Iterator<Item = usize> + 'a,
    ) -> Arc<FlatRecordBatch> {
        let (new_schema, new_columns) = {
            let mut columns = Vec::new();
            let mut fields = Vec::new();

            let schema = self.flat.schema_ref();
            for idx in indices {
                fields.push(schema.field(idx).clone());
                columns.push(Arc::clone(self.flat.column(idx)));
            }

            let schema_metadata = schema.metadata().clone();
            (
                Arc::new(Schema::new_with_metadata(fields, schema_metadata)),
                columns,
            )
        };
        // only column selection, so .unwrap() is safe
        let new_flat = {
            let options = RecordBatchOptions::default().with_row_count(Some(self.flat.num_rows()));
            RecordBatch::try_new_with_options(new_schema, new_columns, &options)
        }
        .unwrap();
        Arc::new(Self::_from_flattened_record_batch(Arc::new(new_flat), None).unwrap())
    }

    pub(crate) fn converters(&self) -> &[Box<dyn ArrayConverter>] {
        &self.converters
    }

    pub(crate) fn column_converter(&self, idx: usize) -> &dyn ArrayConverter {
        self.converters[idx].as_ref()
    }
}

impl fmt::Debug for FlatRecordBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.flat.fmt(f)
    }
}

impl Deref for FlatRecordBatch {
    type Target = RecordBatch;

    fn deref(&self) -> &Self::Target {
        &self.flat
    }
}

pub(crate) fn single_column_batch(batch: &RecordBatch, idx: usize) -> RecordBatch {
    let schema_ref = batch.schema_ref();
    let field = schema_ref.field(idx).clone();
    let schema = Schema::new_with_metadata(vec![field], schema_ref.metadata().clone());
    let columns = vec![batch.column(idx).clone()];
    // only one column selected, row-count can be derived, so .unwrap() is safe
    RecordBatch::try_new(Arc::new(schema), columns).unwrap()
}
