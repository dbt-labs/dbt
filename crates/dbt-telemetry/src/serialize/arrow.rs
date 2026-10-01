//! Arrow serialization support for dbt telemetry attributes using serde_arrow.

use crate::{
    ArtifactType, ExecutionPhase, NodeCancelReason, NodeErrorType, NodeMaterialization,
    NodeOutcome, NodeSkipReason, NodeType, QueryOutcome,
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// A special type used to derive the schema for telemetry event attributes in arrow
/// serialization, as well as a intermediate representation for serialization and deserialization.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArrowAttributes<'a> {
    // This field is used to serialize all non-well known and commonly used attributes,
    // as a JSON blob. This is especially useful for events which are not frequent per
    // -invocation, as it avoids creating many sparse columns in the arrow table.
    pub json_payload: Option<String>,
    // Well-known fields common across many event types
    pub name: Option<Cow<'a, str>>,
    pub database: Option<Cow<'a, str>>,
    pub schema: Option<Cow<'a, str>>,
    pub identifier: Option<Cow<'a, str>>,
    pub dbt_core_event_code: Option<Cow<'a, str>>,
    // Well-known phase fields
    pub phase: Option<ExecutionPhase>,
    // Well-known node fields
    pub unique_id: Option<Cow<'a, str>>,
    pub materialization: Option<NodeMaterialization>,
    pub custom_materialization: Option<Cow<'a, str>>,
    pub node_type: Option<NodeType>,
    pub node_outcome: Option<NodeOutcome>,
    pub node_error_type: Option<NodeErrorType>,
    pub node_cancel_reason: Option<NodeCancelReason>,
    pub node_skip_reason: Option<NodeSkipReason>,
    pub sao_enabled: Option<bool>,
    // CallTrace/Unknown fields
    pub dev_name: Option<Cow<'a, str>>,
    // Fusion source code location fields (debug only)
    pub file: Option<Cow<'a, str>>,
    pub line: Option<u32>,
    // Log fields
    pub code: Option<u32>,
    pub code_name: Option<Cow<'a, str>>,
    pub original_severity_number: Option<i32>,
    pub original_severity_text: Option<Cow<'a, str>>,
    pub package_name: Option<Cow<'a, str>>,
    // Artifact or node paths & location
    pub relative_path: Option<Cow<'a, str>>,
    pub code_line: Option<u32>,
    pub code_column: Option<u32>,
    pub artifact_type: Option<ArtifactType>,
    // Query fields
    pub query_id: Option<Cow<'a, str>>,
    pub query_outcome: Option<QueryOutcome>,
    pub adapter_type: Option<Cow<'a, str>>,
    pub query_error_vendor_code: Option<i32>,
    /// Associated content hash (e.g. can be CAS hash for artifacts stored in CAS).
    /// or node checksum.
    pub content_hash: Option<Cow<'a, str>>,
    // Formatted output fields (e.g. `list` command)
    pub output_format: Option<Cow<'a, str>>,
    pub content: Option<Cow<'a, str>>,
    // Node processing duration
    pub duration_ms: Option<u64>,
    // Number of rows affected by this event (e.g. node operation)
    pub rows_affected: Option<u64>,
    // Group identifier for model notifications
    pub group: Option<Cow<'a, str>>,
}

#[cfg(test)]
mod tests {
    use crate::TelemetryEventTypeRegistry;
    use arrow::{
        array::{
            Array, ArrayRef, DictionaryArray, Int32Builder, LargeStringArray, StructArray,
            UInt64Array,
        },
        compute::{CastOptions, cast_with_options},
        datatypes::{DataType, Field, FieldRef, Fields, Int32Type, Schema, TimeUnit},
        record_batch::RecordBatch,
        util::display::FormatOptions,
    };
    use dbt_tracing::{
        TelemetryEventRecType, TelemetryOutputFlags,
        serialize::arrow::{TelemetryArrowSchemas, deserialize_from_arrow, serialize_to_arrow},
    };
    use parquet::{
        arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder},
        basic::Compression,
        file::properties::WriterProperties,
    };

    use super::*;
    use std::sync::Arc;
    use std::{
        collections::{HashMap, HashSet},
        rc::Rc,
    };

    use crate::test_utils::{
        create_all_fake_attributes, create_all_fake_records, create_test_log_record,
    };

    fn cast_array(
        array: &ArrayRef,
        data_type: &DataType,
    ) -> Result<ArrayRef, Box<dyn std::error::Error>> {
        Ok(cast_with_options(
            array.as_ref(),
            data_type,
            &CastOptions {
                safe: false,
                format_options: FormatOptions::new().with_display_error(false),
            },
        )?)
    }

    fn batch_with_dictionary_large_utf8_column(
        batch: &RecordBatch,
        column_name: &str,
    ) -> RecordBatch {
        let column_index = batch
            .schema()
            .index_of(column_name)
            .expect("column should exist");
        let column = batch.column(column_index);
        let large_utf8 =
            cast_array(column, &DataType::LargeUtf8).expect("failed to cast column to LargeUtf8");
        let string_array = large_utf8
            .as_any()
            .downcast_ref::<LargeStringArray>()
            .expect("expected LargeUtf8 column");

        let mut builder = Int32Builder::with_capacity(string_array.len());
        let mut dictionary = Vec::new();
        let mut indices: HashMap<String, i32> = HashMap::new();

        for value in string_array.iter() {
            match value {
                Some(value) => {
                    let entry = indices.entry(value.to_string()).or_insert_with(|| {
                        let idx = dictionary.len() as i32;
                        dictionary.push(value.to_string());
                        idx
                    });
                    builder.append_value(*entry);
                }
                None => builder.append_null(),
            }
        }

        let keys = builder.finish();
        let values = Arc::new(LargeStringArray::from(dictionary)) as ArrayRef;
        let dictionary_array = Arc::new(
            DictionaryArray::<Int32Type>::try_new(keys, values)
                .expect("failed to build dictionary"),
        ) as ArrayRef;

        replace_column(batch, column_index, dictionary_array)
    }

    fn batch_with_large_utf8_column(batch: &RecordBatch, column_name: &str) -> RecordBatch {
        let column_index = batch
            .schema()
            .index_of(column_name)
            .expect("column should exist");
        let column = batch.column(column_index);
        let large_utf8 =
            cast_array(column, &DataType::LargeUtf8).expect("failed to cast to LargeUtf8");

        replace_column(batch, column_index, large_utf8)
    }

    fn batch_with_extra_column(batch: &RecordBatch) -> RecordBatch {
        let mut fields: Vec<FieldRef> = batch
            .schema()
            .fields()
            .iter()
            .map(|field| Arc::new(field.as_ref().clone()))
            .collect();
        fields.push(Arc::new(Field::new(
            "__test_extra_column",
            DataType::UInt64,
            true,
        )));

        let extra_values = UInt64Array::from(vec![Some(42u64); batch.num_rows()]);
        let mut columns = batch.columns().to_vec();
        columns.push(Arc::new(extra_values) as ArrayRef);

        let schema = Arc::new(Schema::new(fields));
        RecordBatch::try_new(schema, columns).expect("failed to build batch with extra column")
    }

    fn batch_with_non_nullable_body(batch: &RecordBatch) -> RecordBatch {
        let column_index = batch
            .schema()
            .index_of("body")
            .expect("body column should exist");
        assert!(
            batch
                .schema()
                .field_with_name("body")
                .expect("body column should exist")
                .is_nullable(),
            "Body field expected to be nullablefor this test"
        );
        let column = batch.column(column_index);
        assert_eq!(
            column.null_count(),
            0,
            "body column cannot be made non-nullable when it contains nulls"
        );

        let mut fields: Vec<FieldRef> = batch
            .schema()
            .fields()
            .iter()
            .map(|field| Arc::new(field.as_ref().clone()))
            .collect();
        fields[column_index] = Arc::new(
            batch
                .schema()
                .field(column_index)
                .clone()
                .with_nullable(false),
        );

        let columns = batch.columns().to_vec();
        let schema = Arc::new(Schema::new(fields));
        RecordBatch::try_new(schema, columns).expect("failed to make body non-nullable")
    }

    fn batch_with_non_nullable_json_payload(batch: &RecordBatch) -> RecordBatch {
        let attributes_index = batch
            .schema()
            .index_of("attributes")
            .expect("attributes column should exist");

        let attributes_column = batch.column(attributes_index).clone();
        let struct_array = attributes_column
            .as_ref()
            .as_any()
            .downcast_ref::<StructArray>()
            .expect("attributes column should be a StructArray");

        let DataType::Struct(child_fields) = attributes_column.data_type() else {
            panic!("attributes column should have struct data type");
        };

        let mut updated_fields: Vec<FieldRef> = child_fields
            .iter()
            .map(|field| Arc::new(field.as_ref().clone()))
            .collect();

        let json_index = updated_fields
            .iter()
            .position(|field| field.name() == "json_payload")
            .expect("json_payload field should exist");

        let json_column = struct_array.column(json_index);
        let json_values = json_column
            .as_ref()
            .as_any()
            .downcast_ref::<LargeStringArray>()
            .expect("json_payload should be LargeUtf8");

        let cleaned_json_column: ArrayRef = Arc::new(LargeStringArray::from_iter_values(
            json_values
                .iter()
                .map(|value| value.unwrap_or_default().to_owned()),
        ));

        let updated_json_field = updated_fields[json_index]
            .as_ref()
            .clone()
            .with_nullable(false);
        updated_fields[json_index] = Arc::new(updated_json_field);

        let mut child_columns: Vec<ArrayRef> = (0..struct_array.num_columns())
            .map(|idx| struct_array.column(idx).clone())
            .collect();
        child_columns[json_index] = cleaned_json_column;

        let new_struct = Arc::new(StructArray::new(
            Fields::from(updated_fields),
            child_columns,
            struct_array.logical_nulls(),
        )) as ArrayRef;

        replace_column(batch, attributes_index, new_struct)
    }

    fn batch_missing_column(batch: &RecordBatch, column_name: &str) -> RecordBatch {
        let column_index = batch
            .schema()
            .index_of(column_name)
            .expect("column should exist");

        let fields: Vec<FieldRef> = batch
            .schema()
            .fields()
            .iter()
            .enumerate()
            .filter(|&(idx, _)| idx != column_index)
            .map(|(_, field)| Arc::new(field.as_ref().clone()))
            .collect();

        let columns: Vec<ArrayRef> = batch
            .columns()
            .iter()
            .enumerate()
            .filter(|&(idx, _)| idx != column_index)
            .map(|(_, column)| column.clone())
            .collect();

        let schema = Arc::new(Schema::new(fields));
        RecordBatch::try_new(schema, columns).expect("failed to remove column")
    }

    fn replace_column(
        batch: &RecordBatch,
        column_index: usize,
        new_column: ArrayRef,
    ) -> RecordBatch {
        let mut fields: Vec<FieldRef> = batch
            .schema()
            .fields()
            .iter()
            .map(|field| Arc::new(field.as_ref().clone()))
            .collect();

        fields[column_index] = Arc::new(
            batch
                .schema()
                .field(column_index)
                .clone()
                .with_data_type(new_column.data_type().clone()),
        );

        let mut columns = batch.columns().to_vec();
        columns[column_index] = new_column;

        let schema = Arc::new(Schema::new(fields));
        RecordBatch::try_new(schema, columns).expect("failed to replace column")
    }

    #[test]
    fn test_deserialize_from_arrow_schema_normalization() {
        let attributes = create_all_fake_attributes(
            "schema_norm_seed",
            TelemetryEventTypeRegistry::public(),
            TelemetryOutputFlags::EXPORT_PARQUET,
        )
        .into_iter()
        .find(|attrs| matches!(attrs.record_category(), TelemetryEventRecType::Log))
        .expect("expected at least one log attribute");

        let log_record = create_test_log_record("schema_norm_seed", attributes);
        let records = vec![log_record];
        let schemas = TelemetryArrowSchemas::new::<TelemetryEventTypeRegistry>();
        let base_batch =
            serialize_to_arrow(&records, &schemas).expect("failed to serialize base batch");
        let registry = TelemetryEventTypeRegistry::public();

        let variations = vec![
            (
                "event_type_large_utf8_instead_of_dict_utf8",
                batch_with_large_utf8_column(&base_batch, "event_type"),
            ),
            (
                "event_type_dict_large_utf8_instead_of_dict_utf8",
                batch_with_dictionary_large_utf8_column(&base_batch, "event_type"),
            ),
            (
                "severity_text_large_utf8_instead_of_dict_utf8",
                batch_with_large_utf8_column(&base_batch, "severity_text"),
            ),
            ("extra_column", batch_with_extra_column(&base_batch)),
            (
                "body_non_nullable",
                batch_with_non_nullable_body(&base_batch),
            ),
            (
                "attributes_json_payload_non_nullable",
                batch_with_non_nullable_json_payload(&base_batch),
            ),
            (
                "missing_nullable_column",
                batch_missing_column(&base_batch, "links"),
            ),
        ];

        for (name, variant) in variations {
            let (deserialized, errors) = deserialize_from_arrow(&variant, registry)
                .unwrap_or_else(|e| panic!("expected success for {name}: {e}"));
            assert!(
                errors.is_empty(),
                "unexpected deserialize errors for {name}: {errors:?}"
            );
            assert_eq!(deserialized, records, "variation {name} mismatch");
        }

        let missing = batch_missing_column(&base_batch, "event_type");
        assert!(
            deserialize_from_arrow(&missing, registry).is_err(),
            "missing required column should fail"
        );
    }

    #[test]
    fn test_arrow_roundtrip_all_record_types() {
        // Create records of each record & event (aka attribute) type with a pseudo-random seed
        let original_records = create_all_fake_records(
            "test_seed",
            TelemetryEventTypeRegistry::public(),
            TelemetryOutputFlags::EXPORT_PARQUET,
        );

        let schemas = TelemetryArrowSchemas::new::<TelemetryEventTypeRegistry>();
        let batch = serialize_to_arrow(&original_records, &schemas).unwrap();
        let (mut deserialized, errors) =
            deserialize_from_arrow(&batch, TelemetryEventTypeRegistry::public()).unwrap();
        assert!(
            errors.is_empty(),
            "unexpected deserialize errors: {errors:?}"
        );

        // Use PartialEq to compare entire records
        for (original, deserialized) in original_records.iter().zip(deserialized.iter()) {
            assert_eq!(
                original, deserialized,
                "Record roundtrip failed for: {original:?}"
            );
        }

        // Now through parquet
        let mut buffer = Rc::new(Vec::new());
        {
            let cursor = std::io::Cursor::new(Rc::get_mut(&mut buffer).unwrap());

            let mut parquet_writer = ArrowWriter::try_new(
                cursor,
                Schema::new(schemas.schema_with_timestamps().to_vec()).into(),
                Some(
                    WriterProperties::builder()
                        .set_compression(Compression::SNAPPY)
                        .build(),
                ),
            )
            .expect("Failed to create Parquet writer");
            parquet_writer.write(&batch).expect("Failed to write batch");
            parquet_writer.close().expect("Failed to close writer");
        }

        let parquet_reader = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from_owner(
            Rc::into_inner(buffer).unwrap(),
        ))
        .unwrap()
        .build()
        .unwrap();

        deserialized.clear();
        for batch_result in parquet_reader {
            let (records, errors) = deserialize_from_arrow(
                &batch_result.unwrap(),
                TelemetryEventTypeRegistry::public(),
            )
            .unwrap();
            assert!(
                errors.is_empty(),
                "unexpected deserialize errors: {errors:?}"
            );
            deserialized.extend(records);
        }

        // Use PartialEq to compare entire records
        for (original, deserialized) in original_records.iter().zip(deserialized.iter()) {
            assert_eq!(
                original, deserialized,
                "Record roundtrip via parquet failed for: {original:?}"
            );
        }
    }

    #[test]
    fn test_schema_creation() {
        let schemas = TelemetryArrowSchemas::new::<TelemetryEventTypeRegistry>();
        let serialisable_schema = schemas.serialisable_schema();
        let schema_with_timestamps = schemas.schema_with_timestamps();
        assert!(!serialisable_schema.is_empty());
        assert!(!schema_with_timestamps.is_empty());

        // Assert all expected top-level keys present (they are stable)
        [
            "record_type",
            "trace_id",
            "span_id",
            "event_id",
            "span_name",
            "parent_span_id",
            "links",
            "start_time_unix_nano",
            "end_time_unix_nano",
            "time_unix_nano",
            "severity_number",
            "severity_text",
            "body",
            "status_code",
            "status_message",
            "event_type",
            "attributes",
        ]
        .iter()
        .for_each(|&field| {
            let serializable_schema_field = serialisable_schema
                .iter()
                .find(|f| f.name() == field)
                .expect("Missing field in `serialisable_schema`");
            let schema_with_timestamps_field = schema_with_timestamps
                .iter()
                .find(|f| f.name() == field)
                .expect("Missing field in `schema_with_timestamps`");

            if field == "start_time_unix_nano"
                || field == "end_time_unix_nano"
                || field == "time_unix_nano"
            {
                assert_eq!(
                    *schema_with_timestamps_field.data_type(),
                    DataType::Timestamp(TimeUnit::Nanosecond, None),
                    "Field {field} should be Timestamp(NANOSECOND)"
                );
                assert_eq!(
                    *serializable_schema_field.data_type(),
                    DataType::UInt64,
                    "Field {field} should be UInt64 in `serialisable_schema`"
                );
            } else {
                assert_eq!(
                    serializable_schema_field.data_type(),
                    schema_with_timestamps_field.data_type(),
                    "Field {field} should have the same type in both schemas"
                );
            }
        });

        // Test attributes struct schema has all keys from ArrowAttributes
        let attributes_field = serialisable_schema
            .iter()
            .find(|f| f.name() == "attributes")
            .expect("Missing attributes field");
        let DataType::Struct(attribute_fields) = attributes_field.data_type() else {
            panic!("Attributes field should be a Struct");
        };
        let attribute_field_names: HashSet<&str> =
            attribute_fields.iter().map(|f| f.name().as_str()).collect();

        let fake_attrs = serde_json::to_value(ArrowAttributes::default())
            .expect("Failed to serialize ArrowAttributes");
        let expected_field_names = fake_attrs
            .as_object()
            .expect("ArrowAttributes should serialize to a JSON object")
            .keys()
            .map(|s| s.as_str())
            .collect::<HashSet<&str>>();

        let missing_fields: Vec<&str> = expected_field_names
            .difference(&attribute_field_names)
            .copied()
            .collect();

        let extra_fields: Vec<&str> = attribute_field_names
            .difference(&expected_field_names)
            .copied()
            .collect();

        let mut err_msg = String::new();
        if !missing_fields.is_empty() {
            err_msg.push_str(&format!("Missing fields: {}.\n", missing_fields.join(", ")));
        }
        if !extra_fields.is_empty() {
            err_msg.push_str(&format!("Extra fields: {}.", extra_fields.join(", ")));
        }

        assert!(
            err_msg.is_empty(),
            "Attribute schema vs. struct fields mismatch: {err_msg}"
        );
    }
}
