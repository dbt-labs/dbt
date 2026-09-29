use std::collections::HashMap;
use std::sync::Arc;

use crate::AdapterResult;
use crate::AdapterType;
use crate::errors::{AdapterError, AdapterErrorKind};

use arrow::array::{
    Array, ArrayRef, AsArray, FixedSizeListArray, GenericListArray, Int64Array, MapArray,
    StringArray, StringBuilder, StructArray,
};
use arrow::compute::{CastOptions, cast_with_options};
use arrow::datatypes::{DataType, Field, FieldRef, Fields, Schema};
use arrow::record_batch::RecordBatch;
use arrow_json::writer::{EncoderOptions, make_encoder};

pub(crate) const SNOWFLAKE_DML_COLUMNS: &[&str] = &[
    "number of rows inserted",
    "number of rows updated",
    "number of rows deleted",
];

pub(crate) const ROWS_AFFECTED_META: &str = "rows_affected";

/// Information about a column that was renamed during disambiguation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenamedColumn<'a> {
    /// The original column name (duplicate).
    pub original: &'a str,
    /// The new unique column name (e.g., "col_2", "col_3").
    pub renamed: &'a str,
}

pub trait RecordBatchExt {
    fn meta_string(&self, key: &str) -> Option<String>;
    fn meta_i64(&self, key: &str) -> Option<i64>;
    fn first_value_as_i64(&self) -> Option<i64>;
    fn named_value_as_i64(&self, column_name: &str) -> Option<i64>;
    fn column_typed<'a>(&'a self, name: &str) -> AdapterResult<&'a Arc<dyn Array>>;
    fn column_values<T>(&self, column_name: &str) -> AdapterResult<T>
    where
        T: std::any::Any + Clone;
    fn disambiguate_column_names(
        self,
        on_disambiguate: Option<impl FnOnce(&[RenamedColumn<'_>])>,
    ) -> RecordBatch;
    fn jsonify_nested_columns(self) -> RecordBatch;
    /// Lowercase all column names in the RecordBatch schema.
    ///
    /// This is used for database commands whose result column names are defined
    /// as lowercase even when a driver reports them in uppercase.
    fn lowercase_column_names(self) -> RecordBatch;
}

impl RecordBatchExt for RecordBatch {
    fn meta_string(&self, key: &str) -> Option<String> {
        self.schema().metadata().get(key).cloned()
    }

    fn meta_i64(&self, key: &str) -> Option<i64> {
        self.schema()
            .metadata()
            .get(key)
            .and_then(|v| v.parse::<i64>().ok())
    }

    fn first_value_as_i64(&self) -> Option<i64> {
        cast_column_to_i64(self.columns().first()?.as_ref())
    }

    fn named_value_as_i64(&self, column_name: &str) -> Option<i64> {
        let idx = self.schema().index_of(column_name).ok()?;
        cast_column_to_i64(self.column(idx).as_ref())
    }

    fn column_typed<'a>(&'a self, name: &str) -> AdapterResult<&'a Arc<dyn Array>> {
        self.column_by_name(name).ok_or_else(|| {
            let schema = self.schema();
            let columns = schema.fields().iter().map(|f| f.name()).collect::<Vec<_>>();
            AdapterError::new(
                AdapterErrorKind::Internal,
                format!("expected column {name} not found, available are: {columns:?}"),
            )
        })
    }

    fn column_values<T>(&self, column_name: &str) -> AdapterResult<T>
    where
        T: std::any::Any + Clone,
    {
        let column = self.column_typed(column_name)?;

        // Metadata/catalog queries are parsed by downcasting straight to a
        // concrete `StringArray`, but drivers/engines may now return
        // `Utf8View`/`LargeUtf8` for what used to always be plain `Utf8`
        // (e.g. the query-execution path unconditionally widens to view
        // types). Normalize down to `Utf8` first so callers expecting
        // `StringArray` keep working regardless of which string
        // representation produced the result.
        let normalized: ArrayRef = if std::any::TypeId::of::<T>()
            == std::any::TypeId::of::<StringArray>()
            && matches!(column.data_type(), DataType::Utf8View | DataType::LargeUtf8)
        {
            cast_with_options(column.as_ref(), &DataType::Utf8, &CastOptions::default())
                .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))?
        } else {
            column.clone()
        };

        Ok(normalized
            .as_any()
            .downcast_ref::<T>()
            .ok_or_else(|| {
                let schema = self.schema();
                let field = schema.fields().iter().find(|f| f.name() == column_name);
                AdapterError::new(
                    AdapterErrorKind::Internal,
                    format!(
                        "expected column of type: {} not found, available are: {field:?}",
                        std::any::type_name::<T>()
                    ),
                )
            })?
            .to_owned())
    }

    fn disambiguate_column_names(
        self,
        on_disambiguate: Option<impl FnOnce(&[RenamedColumn<'_>])>,
    ) -> RecordBatch {
        let schema = self.schema();
        let fields = schema.fields();

        let mut name_counts: HashMap<&str, usize> = HashMap::new();
        let mut new_names: Vec<String> = Vec::with_capacity(fields.len());

        for field in fields.iter() {
            let name = field.name().as_str();
            let count = name_counts.entry(name).or_insert(0);
            *count += 1;
            if *count > 1 {
                new_names.push(format!("{}_{}", name, count));
            } else {
                new_names.push(name.to_string());
            }
        }

        let renamed_columns: Vec<_> = fields
            .iter()
            .zip(new_names.iter())
            .filter(|(field, new_name)| field.name() != *new_name)
            .map(|(field, new_name)| RenamedColumn {
                original: field.name().as_str(),
                renamed: new_name.as_str(),
            })
            .collect();

        if renamed_columns.is_empty() {
            return self;
        }

        if let Some(callback) = on_disambiguate {
            callback(&renamed_columns);
        }

        let new_fields: Vec<_> = fields
            .iter()
            .zip(new_names.iter())
            .map(|(field, new_name)| Arc::new(field.as_ref().clone().with_name(new_name.clone())))
            .collect();

        let new_schema = Arc::new(Schema::new_with_metadata(
            new_fields,
            schema.metadata().clone(),
        ));

        RecordBatch::try_new(new_schema, self.columns().to_vec())
            .expect("disambiguate_column_names: schema and columns should be compatible")
    }

    /// Stringify nested columns, ignoring struct fields
    ///
    /// dbt Core flattens struct fields as JSON strings for certain adapters, only keeping the
    /// root-level columns.
    ///
    /// Reference: https://github.com/dbt-labs/dbt-common/blob/f21aa0d3093e98c7c35ed93163832c84f46eb3d8/dbt_common/clients/agate_helper.py#L110
    fn jsonify_nested_columns(self) -> RecordBatch {
        let schema = self.schema();
        let fields = schema.fields();

        if !fields.iter().any(|f| f.data_type().is_nested()) {
            return self;
        }

        let options = EncoderOptions::default();
        let mut new_fields: Vec<Arc<Field>> = Vec::with_capacity(fields.len());
        let mut new_columns: Vec<Arc<dyn Array>> = Vec::with_capacity(self.num_columns());

        for (field, column) in fields.iter().zip(self.columns().iter()) {
            if field.data_type().is_nested() {
                let (encode_field, encode_column) = jsonify_map_keys(field, column, &options);
                let string_col = encode_array_to_strings(&encode_field, &encode_column, &options);
                new_columns.push(Arc::new(string_col));
                new_fields.push(Arc::new(
                    Field::new(field.name(), DataType::Utf8, field.is_nullable())
                        .with_metadata(field.metadata().clone()),
                ));
            } else {
                new_columns.push(column.clone());
                new_fields.push(field.clone());
            }
        }

        let new_schema = Arc::new(Schema::new_with_metadata(
            new_fields,
            schema.metadata().clone(),
        ));
        RecordBatch::try_new(new_schema, new_columns)
            .expect("jsonify_nested_columns: rewritten schema and columns are consistent")
    }

    fn lowercase_column_names(self) -> RecordBatch {
        let schema = self.schema();
        let fields = schema.fields();

        if fields.iter().all(|f| {
            f.name()
                .chars()
                .all(|c| c.is_lowercase() || !c.is_alphabetic())
        }) {
            return self;
        }

        let new_fields: Vec<_> = fields
            .iter()
            .map(|f| Arc::new(f.as_ref().clone().with_name(f.name().to_lowercase())))
            .collect();

        let new_schema = Arc::new(Schema::new_with_metadata(
            new_fields,
            schema.metadata().clone(),
        ));

        RecordBatch::try_new(new_schema, self.columns().to_vec())
            .expect("lowercase_column_names: schema and columns should be compatible")
    }
}

pub trait StructArrayExt {
    /// Looks up a named field in the struct and returns it as a typed array `T`.
    /// Errors if the field is absent or is not of type `T`.
    fn column_as<T: 'static>(&self, name: &str) -> AdapterResult<&T>;
}

impl StructArrayExt for StructArray {
    fn column_as<T: 'static>(&self, name: &str) -> AdapterResult<&T> {
        self.column_by_name(name)
            .and_then(|c| c.as_any().downcast_ref::<T>())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::UnexpectedResult,
                    format!("Missing or invalid '{name}' column"),
                )
            })
    }
}

pub trait SchemaExt {
    fn has_dml_columns(&self, adapter_type: AdapterType) -> bool;
}

impl SchemaExt for Schema {
    fn has_dml_columns(&self, adapter_type: AdapterType) -> bool {
        match adapter_type {
            AdapterType::Snowflake => self
                .fields()
                .iter()
                .any(|f| SNOWFLAKE_DML_COLUMNS.contains(&f.name().as_str())),
            _ => false,
        }
    }
}

fn encode_array_to_strings(
    field: &FieldRef,
    array: &ArrayRef,
    options: &EncoderOptions,
) -> StringArray {
    let mut encoder = make_encoder(field, array.as_ref(), options)
        .expect("make_encoder for nested column should not fail");
    let mut builder = StringBuilder::with_capacity(array.len(), array.len() * 32);
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    for row in 0..array.len() {
        if encoder.is_null(row) {
            builder.append_null();
        } else {
            buf.clear();
            encoder.encode(row, &mut buf);
            let s = std::str::from_utf8(&buf).expect("arrow_json::Encoder emits UTF-8");
            builder.append_value(s);
        }
    }
    builder.finish()
}

/// Recursively rewrite every nested `Map` so its keys become `Utf8`.
///
/// arrow_json's map encoder only supports UTF-8 keys, while dbt Core stringifies any key via
/// `json.dumps`. Non-string keys (integers, floats, structs, lists, ...) are JSON-encoded into
/// their string form so the resulting map serializes to valid JSON.
fn jsonify_map_keys(
    field: &FieldRef,
    array: &ArrayRef,
    options: &EncoderOptions,
) -> (FieldRef, ArrayRef) {
    match field.data_type() {
        DataType::Map(entries, ordered) => {
            let map = array.as_map();
            let DataType::Struct(entry_fields) = entries.data_type() else {
                return (field.clone(), array.clone());
            };
            let (Some(key_field), Some(value_field)) = (entry_fields.first(), entry_fields.get(1))
            else {
                return (field.clone(), array.clone());
            };

            let (new_value_field, new_value_arr) =
                jsonify_map_keys(value_field, map.values(), options);

            let (new_key_field, new_key_arr): (FieldRef, ArrayRef) = if matches!(
                key_field.data_type(),
                DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
            ) {
                (key_field.clone(), map.keys().clone())
            } else {
                let (norm_key_field, norm_key_arr) =
                    jsonify_map_keys(key_field, map.keys(), options);
                let key_strs = encode_array_to_strings(&norm_key_field, &norm_key_arr, options);
                (
                    Arc::new(
                        Field::new(key_field.name(), DataType::Utf8, key_field.is_nullable())
                            .with_metadata(key_field.metadata().clone()),
                    ),
                    Arc::new(key_strs),
                )
            };

            let new_entry_fields = Fields::from(vec![new_key_field, new_value_field]);
            let new_entries = StructArray::new(
                new_entry_fields.clone(),
                vec![new_key_arr, new_value_arr],
                map.entries().nulls().cloned(),
            );
            let new_entries_field = Arc::new(
                Field::new(
                    entries.name(),
                    DataType::Struct(new_entry_fields),
                    entries.is_nullable(),
                )
                .with_metadata(entries.metadata().clone()),
            );
            let new_map = MapArray::new(
                new_entries_field.clone(),
                map.offsets().clone(),
                new_entries,
                map.nulls().cloned(),
                *ordered,
            );
            let new_field = Arc::new(
                Field::new(
                    field.name(),
                    DataType::Map(new_entries_field, *ordered),
                    field.is_nullable(),
                )
                .with_metadata(field.metadata().clone()),
            );
            (new_field, Arc::new(new_map))
        }
        DataType::Struct(child_fields) => {
            let struct_arr = array.as_struct();
            let (new_fields, new_columns): (Vec<FieldRef>, Vec<ArrayRef>) = child_fields
                .iter()
                .zip(struct_arr.columns().iter())
                .map(|(child_field, child_arr)| jsonify_map_keys(child_field, child_arr, options))
                .unzip();
            let new_fields = Fields::from(new_fields);
            let new_struct =
                StructArray::new(new_fields.clone(), new_columns, struct_arr.nulls().cloned());
            let new_field = Arc::new(
                Field::new(
                    field.name(),
                    DataType::Struct(new_fields),
                    field.is_nullable(),
                )
                .with_metadata(field.metadata().clone()),
            );
            (new_field, Arc::new(new_struct))
        }
        DataType::List(child_field) => {
            let list = array.as_list::<i32>();
            let (new_child_field, new_values) =
                jsonify_map_keys(child_field, list.values(), options);
            let new_list = GenericListArray::<i32>::new(
                new_child_field.clone(),
                list.offsets().clone(),
                new_values,
                list.nulls().cloned(),
            );
            let new_field = Arc::new(
                Field::new(
                    field.name(),
                    DataType::List(new_child_field),
                    field.is_nullable(),
                )
                .with_metadata(field.metadata().clone()),
            );
            (new_field, Arc::new(new_list))
        }
        DataType::LargeList(child_field) => {
            let list = array.as_list::<i64>();
            let (new_child_field, new_values) =
                jsonify_map_keys(child_field, list.values(), options);
            let new_list = GenericListArray::<i64>::new(
                new_child_field.clone(),
                list.offsets().clone(),
                new_values,
                list.nulls().cloned(),
            );
            let new_field = Arc::new(
                Field::new(
                    field.name(),
                    DataType::LargeList(new_child_field),
                    field.is_nullable(),
                )
                .with_metadata(field.metadata().clone()),
            );
            (new_field, Arc::new(new_list))
        }
        DataType::FixedSizeList(child_field, size) => {
            let list = array.as_fixed_size_list();
            let (new_child_field, new_values) =
                jsonify_map_keys(child_field, list.values(), options);
            let new_list = FixedSizeListArray::new(
                new_child_field.clone(),
                *size,
                new_values,
                list.nulls().cloned(),
            );
            let new_field = Arc::new(
                Field::new(
                    field.name(),
                    DataType::FixedSizeList(new_child_field, *size),
                    field.is_nullable(),
                )
                .with_metadata(field.metadata().clone()),
            );
            (new_field, Arc::new(new_list))
        }
        _ => (field.clone(), array.clone()),
    }
}

fn cast_column_to_i64(column: &dyn Array) -> Option<i64> {
    if column.is_empty() {
        return None;
    }
    let casted = cast_with_options(column, &DataType::Int64, &CastOptions::default())
        .inspect_err(|_| {
            debug_assert!(
                false,
                "cast_column_to_i64: unsupported data type {:?}",
                column.data_type()
            );
        })
        .ok()?;
    casted
        .as_any()
        .downcast_ref::<Int64Array>()?
        .iter()
        .next()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{
        ArrayRef, Float64Array, Float64Builder, Int32Array, Int32Builder, Int64Array, ListArray,
        MapBuilder, StringArray, StringBuilder, StructArray,
    };
    use arrow::buffer::OffsetBuffer;
    use arrow::datatypes::{DataType, Field, Int32Type};
    use dbt_test_primitives::assert_contains;

    #[test]
    fn has_dml_columns() {
        let dml_schema = Schema::new(vec![Field::new(
            "number of rows inserted",
            DataType::Int64,
            false,
        )]);
        assert!(dml_schema.has_dml_columns(AdapterType::Snowflake));
        assert!(!dml_schema.has_dml_columns(AdapterType::Bigquery));
        let select_schema = Schema::new(vec![Field::new("id", DataType::Int64, false)]);
        assert!(!select_schema.has_dml_columns(AdapterType::Snowflake));
    }

    #[test]
    fn named_value_as_i64_missing_column_returns_none() {
        let schema = Schema::new(vec![Field::new("id", DataType::Int64, false)]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(Int64Array::from(vec![99]))])
                .unwrap();
        assert!(batch.named_value_as_i64("nonexistent").is_none());
    }

    use std::sync::LazyLock;

    static TEST_DATA: LazyLock<RecordBatch> = LazyLock::new(|| {
        let schema = Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("score", DataType::Float64, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(vec!["FOO"])),
                Arc::new(Float64Array::from(vec![42.0])),
            ],
        )
        .unwrap()
    });

    #[test]
    fn column_values_success() {
        let result: AdapterResult<StringArray> = TEST_DATA.column_values("name");
        assert!(result.is_ok());
    }

    #[test]
    fn column_values_column_not_found() {
        let result: AdapterResult<Int32Array> = TEST_DATA.column_values("nonexistent");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), AdapterErrorKind::Internal);
        assert_contains!(error.message(), "expected column nonexistent not found");
        assert_contains!(error.message(), "available are");
        assert_contains!(error.message(), "name");
        assert_contains!(error.message(), "score");
    }

    #[test]
    fn column_values_normalizes_utf8_view_to_string_array() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "name",
                DataType::Utf8View,
                false,
            )])),
            vec![Arc::new(arrow_array::StringViewArray::from(vec!["FOO"]))],
        )
        .unwrap();
        let result: AdapterResult<StringArray> = batch.column_values("name");
        assert_eq!(result.unwrap(), StringArray::from(vec!["FOO"]));
    }

    #[test]
    fn column_values_normalizes_large_utf8_to_string_array() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "name",
                DataType::LargeUtf8,
                false,
            )])),
            vec![Arc::new(arrow_array::LargeStringArray::from(vec!["FOO"]))],
        )
        .unwrap();
        let result: AdapterResult<StringArray> = batch.column_values("name");
        assert_eq!(result.unwrap(), StringArray::from(vec!["FOO"]));
    }

    #[test]
    fn column_values_wrong_type() {
        let result: AdapterResult<Int32Array> = TEST_DATA.column_values("name");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), AdapterErrorKind::Internal);
        assert_contains!(error.message(), "expected column of type");
        assert!(error.message().contains(
            "arrow_array::array::primitive_array::PrimitiveArray<arrow_array::types::Int32Type>"
        ));
    }

    fn string_struct() -> StructArray {
        StructArray::from(vec![(
            Arc::new(Field::new("col", DataType::Utf8, false)),
            Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef,
        )])
    }

    #[test]
    fn column_as_success() {
        let s = string_struct();
        let col = s.column_as::<StringArray>("col").unwrap();
        assert_eq!(col.value(0), "a");
        assert_eq!(col.value(1), "b");
    }

    #[test]
    fn column_as_missing_column() {
        let s = string_struct();
        let error = s.column_as::<StringArray>("missing").unwrap_err();
        assert_eq!(error.kind(), AdapterErrorKind::UnexpectedResult);
        assert_eq!(error.message(), "Missing or invalid 'missing' column");
    }

    #[test]
    fn column_as_wrong_type() {
        let s = string_struct();
        let error = s.column_as::<Int32Array>("col").unwrap_err();
        assert_eq!(error.kind(), AdapterErrorKind::UnexpectedResult);
        assert_eq!(error.message(), "Missing or invalid 'col' column");
    }

    #[test]
    fn column_as_nested_struct() {
        let inner = string_struct();
        let outer = StructArray::from(vec![(
            Arc::new(Field::new("inner", inner.data_type().clone(), false)),
            Arc::new(inner) as ArrayRef,
        )]);
        let got_inner = outer.column_as::<StructArray>("inner").unwrap();
        let col = got_inner.column_as::<StringArray>("col").unwrap();
        assert_eq!(col.value(0), "a");
        assert_eq!(col.value(1), "b");
    }

    #[test]
    fn column_as_i32() {
        let s = StructArray::from(vec![(
            Arc::new(Field::new("col", DataType::Int32, false)),
            Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
        )]);
        let col = s.column_as::<Int32Array>("col").unwrap();
        assert_eq!(col.value(0), 1);
        assert_eq!(col.value(1), 2);
    }

    #[test]
    fn disambiguate_no_duplicates() {
        let schema = Schema::new(vec![
            Field::new("a", DataType::Int32, false),
            Field::new("b", DataType::Int32, false),
            Field::new("c", DataType::Int32, false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![4, 5, 6])),
                Arc::new(Int32Array::from(vec![7, 8, 9])),
            ],
        )
        .unwrap();

        let callback_invoked = std::cell::Cell::new(false);
        let result = batch.disambiguate_column_names(Some(|_: &[RenamedColumn]| {
            callback_invoked.set(true);
        }));
        assert!(!callback_invoked.get());
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn disambiguate_with_duplicates() {
        let schema = Schema::new(vec![
            Field::new("A", DataType::Int32, false),
            Field::new("B", DataType::Int32, false),
            Field::new("A", DataType::Int32, false),
            Field::new("A", DataType::Int32, false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(Int32Array::from(vec![4, 5, 6])),
                Arc::new(Int32Array::from(vec![7, 8, 9])),
                Arc::new(Int32Array::from(vec![10, 11, 12])),
            ],
        )
        .unwrap();

        let captured = std::cell::RefCell::new(Vec::new());
        let result = batch.disambiguate_column_names(Some(|renamed: &[RenamedColumn]| {
            captured.borrow_mut().extend(
                renamed
                    .iter()
                    .map(|r| (r.original.to_string(), r.renamed.to_string())),
            );
        }));

        let renamed = captured.into_inner();
        assert_eq!(renamed.len(), 2);
        assert_eq!(renamed[0], ("A".to_string(), "A_2".to_string()));
        assert_eq!(renamed[1], ("A".to_string(), "A_3".to_string()));
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["A", "B", "A_2", "A_3"]);
    }

    #[test]
    fn disambiguate_multiple_duplicates() {
        let schema = Schema::new(vec![
            Field::new("x", DataType::Int32, false),
            Field::new("y", DataType::Int32, false),
            Field::new("x", DataType::Int32, false),
            Field::new("y", DataType::Int32, false),
            Field::new("x", DataType::Int32, false),
        ]);
        let cols: Vec<_> = (0..5)
            .map(|_| Arc::new(Int32Array::from(vec![1])) as _)
            .collect();
        let batch = RecordBatch::try_new(Arc::new(schema), cols).unwrap();
        let result = batch.disambiguate_column_names(None::<fn(&[RenamedColumn])>);
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["x", "y", "x_2", "y_2", "x_3"]);
    }

    fn column_as_string<'a>(batch: &'a RecordBatch, name: &str) -> &'a StringArray {
        batch
            .column_by_name(name)
            .unwrap_or_else(|| panic!("column {name} missing"))
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap_or_else(|| panic!("column {name} is not Utf8"))
    }

    #[test]
    fn jsonify_no_nested_columns_passthrough() {
        let schema = Schema::new(vec![
            Field::new("a", DataType::Int64, false),
            Field::new("b", DataType::Utf8, true),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema.clone()),
            vec![
                Arc::new(Int64Array::from(vec![1, 2])),
                Arc::new(StringArray::from(vec![Some("x"), None])),
            ],
        )
        .unwrap();

        let result = batch.jsonify_nested_columns();
        assert_eq!(result.schema().as_ref(), &schema);
        assert_eq!(result.num_rows(), 2);
    }

    #[test]
    fn jsonify_struct_column() {
        let struct_fields = vec![
            Arc::new(Field::new("a", DataType::Int32, false)),
            Arc::new(Field::new("b", DataType::Utf8, false)),
        ];
        let struct_array = StructArray::from(vec![
            (
                struct_fields[0].clone(),
                Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
            ),
            (
                struct_fields[1].clone(),
                Arc::new(StringArray::from(vec!["x", "y"])) as ArrayRef,
            ),
        ]);

        let schema = Schema::new(vec![Field::new(
            "s",
            DataType::Struct(struct_fields.into()),
            false,
        )]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(struct_array) as ArrayRef])
                .unwrap();

        let result = batch.jsonify_nested_columns();
        assert_eq!(result.schema().field(0).data_type(), &DataType::Utf8);

        let col = column_as_string(&result, "s");
        let row0: serde_json::Value = serde_json::from_str(col.value(0)).unwrap();
        let row1: serde_json::Value = serde_json::from_str(col.value(1)).unwrap();
        assert_eq!(row0, serde_json::json!({"a": 1, "b": "x"}));
        assert_eq!(row1, serde_json::json!({"a": 2, "b": "y"}));
    }

    #[test]
    fn jsonify_list_column() {
        let list_array = ListArray::from_iter_primitive::<Int32Type, _, _>(vec![
            Some(vec![Some(1), Some(2), Some(3)]),
            Some(vec![]),
            None,
        ]);
        let list_field = list_array.data_type().clone();
        let schema = Schema::new(vec![Field::new("l", list_field, true)]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(list_array) as ArrayRef]).unwrap();

        let result = batch.jsonify_nested_columns();
        assert_eq!(result.schema().field(0).data_type(), &DataType::Utf8);

        let col = column_as_string(&result, "l");
        let row0: serde_json::Value = serde_json::from_str(col.value(0)).unwrap();
        let row1: serde_json::Value = serde_json::from_str(col.value(1)).unwrap();
        assert_eq!(row0, serde_json::json!([1, 2, 3]));
        assert_eq!(row1, serde_json::json!([]));
        assert!(col.is_null(2));
    }

    #[test]
    fn jsonify_map_column() {
        let mut builder = MapBuilder::new(None, StringBuilder::new(), Int32Array::builder(0));
        builder.keys().append_value("k1");
        builder.values().append_value(10);
        builder.keys().append_value("k2");
        builder.values().append_value(20);
        builder.append(true).unwrap();
        builder.append(true).unwrap();
        let map_array = builder.finish();

        let map_field = map_array.data_type().clone();
        let schema = Schema::new(vec![Field::new("m", map_field, false)]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(map_array) as ArrayRef]).unwrap();

        let result = batch.jsonify_nested_columns();
        assert_eq!(result.schema().field(0).data_type(), &DataType::Utf8);

        let col = column_as_string(&result, "m");
        let row0: serde_json::Value = serde_json::from_str(col.value(0)).unwrap();
        assert_eq!(row0, serde_json::json!({"k1": 10, "k2": 20}));
        let row1: serde_json::Value = serde_json::from_str(col.value(1)).unwrap();
        assert_eq!(row1, serde_json::json!({}));
    }

    #[test]
    fn jsonify_mixed_columns() {
        let struct_fields = vec![Arc::new(Field::new("n", DataType::Int32, false))];
        let struct_array = StructArray::from(vec![(
            struct_fields[0].clone(),
            Arc::new(Int32Array::from(vec![7, 8])) as ArrayRef,
        )]);

        let schema = Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("s", DataType::Struct(struct_fields.into()), false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(Int64Array::from(vec![100, 200])),
                Arc::new(struct_array) as ArrayRef,
            ],
        )
        .unwrap();

        let result = batch.jsonify_nested_columns();
        assert_eq!(result.schema().field(0).data_type(), &DataType::Int64);
        assert_eq!(result.schema().field(1).data_type(), &DataType::Utf8);

        let id_col = result
            .column_by_name("id")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(id_col.value(0), 100);
        assert_eq!(id_col.value(1), 200);

        let s_col = column_as_string(&result, "s");
        let row0: serde_json::Value = serde_json::from_str(s_col.value(0)).unwrap();
        assert_eq!(row0, serde_json::json!({"n": 7}));
    }

    #[test]
    fn jsonify_preserves_schema_metadata() {
        let struct_fields = vec![Arc::new(Field::new("a", DataType::Int32, false))];
        let struct_array = StructArray::from(vec![(
            struct_fields[0].clone(),
            Arc::new(Int32Array::from(vec![1])) as ArrayRef,
        )]);

        let metadata = HashMap::from([("DATABRICKS_QUERY_ID".to_string(), "abc-123".to_string())]);
        let schema = Schema::new_with_metadata(
            vec![Field::new(
                "s",
                DataType::Struct(struct_fields.into()),
                false,
            )],
            metadata.clone(),
        );
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(struct_array) as ArrayRef])
                .unwrap();

        let result = batch.jsonify_nested_columns();
        assert_eq!(result.schema().metadata(), &metadata);
    }

    #[test]
    fn jsonify_map_int_keys() {
        let mut builder = MapBuilder::new(None, Int32Builder::new(), Int32Builder::new());
        builder.keys().append_value(1);
        builder.values().append_value(10);
        builder.keys().append_value(2);
        builder.values().append_value(20);
        builder.append(true).unwrap();
        let map_array = builder.finish();

        let schema = Schema::new(vec![Field::new("m", map_array.data_type().clone(), false)]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(map_array) as ArrayRef]).unwrap();

        let result = batch.jsonify_nested_columns();
        assert_eq!(result.schema().field(0).data_type(), &DataType::Utf8);

        let col = column_as_string(&result, "m");
        let row0: serde_json::Value = serde_json::from_str(col.value(0)).unwrap();
        assert_eq!(row0, serde_json::json!({"1": 10, "2": 20}));
    }

    #[test]
    fn jsonify_map_float_keys() {
        let mut builder = MapBuilder::new(None, Float64Builder::new(), Int32Builder::new());
        builder.keys().append_value(1.5);
        builder.values().append_value(10);
        builder.append(true).unwrap();
        let map_array = builder.finish();

        let schema = Schema::new(vec![Field::new("m", map_array.data_type().clone(), false)]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(map_array) as ArrayRef]).unwrap();

        let result = batch.jsonify_nested_columns();
        assert_eq!(result.schema().field(0).data_type(), &DataType::Utf8);

        let col = column_as_string(&result, "m");
        let row0: serde_json::Value = serde_json::from_str(col.value(0)).unwrap();
        assert_eq!(row0, serde_json::json!({"1.5": 10}));
    }

    #[test]
    fn jsonify_map_struct_keys() {
        // map<struct<a: int, b: int>, int> with one row holding two entries.
        let key_struct_fields = Fields::from(vec![
            Field::new("a", DataType::Int32, false),
            Field::new("b", DataType::Int32, false),
        ]);
        let keys = StructArray::new(
            key_struct_fields.clone(),
            vec![
                Arc::new(Int32Array::from(vec![1, 3])) as ArrayRef,
                Arc::new(Int32Array::from(vec![2, 4])) as ArrayRef,
            ],
            None,
        );
        let values = Int32Array::from(vec![10, 20]);

        let entries_fields = Fields::from(vec![
            Field::new("key", DataType::Struct(key_struct_fields), false),
            Field::new("value", DataType::Int32, true),
        ]);
        let entries = StructArray::new(
            entries_fields.clone(),
            vec![Arc::new(keys) as ArrayRef, Arc::new(values) as ArrayRef],
            None,
        );
        let entries_field = Arc::new(Field::new(
            "entries",
            DataType::Struct(entries_fields),
            false,
        ));
        let map_array = MapArray::new(
            entries_field,
            OffsetBuffer::from_lengths([2]),
            entries,
            None,
            false,
        );

        let schema = Schema::new(vec![Field::new("m", map_array.data_type().clone(), false)]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(map_array) as ArrayRef]).unwrap();

        let result = batch.jsonify_nested_columns();
        assert_eq!(result.schema().field(0).data_type(), &DataType::Utf8);

        let col = column_as_string(&result, "m");
        let row0: serde_json::Value = serde_json::from_str(col.value(0)).unwrap();
        assert_eq!(
            row0,
            serde_json::json!({"{\"a\":1,\"b\":2}": 10, "{\"a\":3,\"b\":4}": 20}),
        );
    }

    #[test]
    fn jsonify_map_list_keys() {
        // map<list<int>, int> with one row holding two entries.
        let keys = ListArray::from_iter_primitive::<Int32Type, _, _>(vec![
            Some(vec![Some(1), Some(2)]),
            Some(vec![Some(3)]),
        ]);
        let values = Int32Array::from(vec![10, 20]);

        let key_field = Arc::new(Field::new("key", keys.data_type().clone(), false));
        let entries_fields = Fields::from(vec![
            key_field.as_ref().clone(),
            Field::new("value", DataType::Int32, true),
        ]);
        let entries = StructArray::new(
            entries_fields.clone(),
            vec![Arc::new(keys) as ArrayRef, Arc::new(values) as ArrayRef],
            None,
        );
        let entries_field = Arc::new(Field::new(
            "entries",
            DataType::Struct(entries_fields),
            false,
        ));
        let map_array = MapArray::new(
            entries_field,
            OffsetBuffer::from_lengths([2]),
            entries,
            None,
            false,
        );

        let schema = Schema::new(vec![Field::new("m", map_array.data_type().clone(), false)]);
        let batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(map_array) as ArrayRef]).unwrap();

        let result = batch.jsonify_nested_columns();
        assert_eq!(result.schema().field(0).data_type(), &DataType::Utf8);

        let col = column_as_string(&result, "m");
        let row0: serde_json::Value = serde_json::from_str(col.value(0)).unwrap();
        assert_eq!(row0, serde_json::json!({"[1,2]": 10, "[3]": 20}));
    }

    #[test]
    fn lowercase_column_names_already_lowercase_is_noop() {
        let schema = Schema::new(vec![
            Field::new("column_name", DataType::Utf8, false),
            Field::new("data_type", DataType::Utf8, false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(vec!["id"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["integer"])) as ArrayRef,
            ],
        )
        .unwrap();

        let original_ptr = Arc::as_ptr(batch.schema_ref());
        let result = batch.lowercase_column_names();
        // Schema should be the same object (no rebuild needed)
        assert_eq!(Arc::as_ptr(result.schema_ref()), original_ptr);
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["column_name", "data_type"]);
    }

    #[test]
    fn lowercase_column_names_snowflake_uppercase() {
        // Simulates what the Snowflake ADBC driver returns for SHOW / system queries.
        // dbt_constraints accesses columns["column_name"].values() (lowercase).
        // Without lowercasing, columns["column_name"] returns undefined.
        let schema = Schema::new(vec![
            Field::new("COLUMN_NAME", DataType::Utf8, false),
            Field::new("DATA_TYPE", DataType::Utf8, false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(vec!["id", "name"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["integer", "varchar"])) as ArrayRef,
            ],
        )
        .unwrap();

        let result = batch.lowercase_column_names();
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["column_name", "data_type"]);

        // Data should be preserved unchanged.
        let col = result
            .column_by_name("column_name")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(col.value(0), "id");
        assert_eq!(col.value(1), "name");
    }

    #[test]
    fn lowercase_column_names_mixed_case() {
        let schema = Schema::new(vec![
            Field::new("Column_Name", DataType::Utf8, false),
            Field::new("id_42", DataType::Int32, false),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(vec!["x"])) as ArrayRef,
                Arc::new(Int32Array::from(vec![1])) as ArrayRef,
            ],
        )
        .unwrap();

        let result = batch.lowercase_column_names();
        let schema = result.schema();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["column_name", "id_42"]);
    }
}
