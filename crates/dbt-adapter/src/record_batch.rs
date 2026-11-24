use crate::AdapterType;
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use arrow::array::{
    Array, ArrayRef, AsArray, FixedSizeListArray, GenericListArray, MapArray, StringArray,
    StringBuilder, StructArray,
};
use arrow::datatypes::{DataType, Field, FieldRef, Fields, Schema};
use arrow_json::writer::{EncoderOptions, make_encoder};
use std::sync::Arc;

pub(crate) const SNOWFLAKE_DML_COLUMNS: &[&str] = &[
    "number of rows inserted",
    "number of rows updated",
    "number of rows deleted",
];

pub(crate) const ROWS_AFFECTED_META: &str = "rows_affected";

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

/// Turn one nested column into the JSON text of each of its values.
pub fn jsonify_column(field: &FieldRef, array: &ArrayRef) -> StringArray {
    // FIXME: build `EncoderOptions` once instead of once per nested column.
    let options = EncoderOptions::default();
    let (field, array) = jsonify_map_keys(field, array, &options);
    encode_array_to_strings(&field, &array, &options)
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{
        Float64Builder, Int32Array, Int32Builder, Int64Array, ListArray, MapBuilder,
    };
    use arrow::buffer::OffsetBuffer;
    use arrow::datatypes::Int32Type;
    use arrow::record_batch::RecordBatch;
    use dbt_adapter_engine::record_batch::RecordBatchExt;
    use std::collections::HashMap;

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

        let result = batch.jsonify_nested_columns(jsonify_column);
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

        let result = batch.jsonify_nested_columns(jsonify_column);
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

        let result = batch.jsonify_nested_columns(jsonify_column);
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

        let result = batch.jsonify_nested_columns(jsonify_column);
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

        let result = batch.jsonify_nested_columns(jsonify_column);
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

        let result = batch.jsonify_nested_columns(jsonify_column);
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

        let result = batch.jsonify_nested_columns(jsonify_column);
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

        let result = batch.jsonify_nested_columns(jsonify_column);
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

        let result = batch.jsonify_nested_columns(jsonify_column);
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

        let result = batch.jsonify_nested_columns(jsonify_column);
        assert_eq!(result.schema().field(0).data_type(), &DataType::Utf8);

        let col = column_as_string(&result, "m");
        let row0: serde_json::Value = serde_json::from_str(col.value(0)).unwrap();
        assert_eq!(row0, serde_json::json!({"[1,2]": 10, "[3]": 20}));
    }

    fn string_struct() -> StructArray {
        StructArray::from(vec![(
            Arc::new(Field::new("col", DataType::Utf8, false)),
            Arc::new(StringArray::from(vec!["a", "b"])) as ArrayRef,
        )])
    }

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
}
