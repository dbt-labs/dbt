use std::sync::Arc;

use arrow::compute::{CastOptions, cast_with_options, concat_batches};
use arrow::util::display::FormatOptions;
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};

use crate::errors::arrow_error_to_adapter_error;

/// Arrow's `Utf8`/`Binary`/`LargeUtf8`/`LargeBinary` arrays each require one
/// contiguous buffer per column; concatenating enough `run_query` batches to exceed
/// `i32::MAX` bytes for a `Utf8`/`Binary` column panics inside the Arrow concat
/// kernel (`GenericBytesBuilder::next_offset`, "byte array offset overflow"), and
/// even `LargeUtf8`/`LargeBinary` (64-bit offsets) only raises that ceiling rather
/// than removing it. `Utf8View`/`BinaryView` don't require a single contiguous
/// buffer (short values inline, longer ones reference pooled, non-contiguous
/// buffers), so converting to them sidesteps the failure mode structurally.
///
/// Converting is unconditional -- regardless of result size or batch count -- so a
/// given query's result schema is deterministic instead of depending on how much
/// data happened to come back. This is reachable from any fetched `run_query`
/// result -- e.g. Elementary's `on_run_end` hooks over large tables
/// (dbt-labs/dbt-core#15706).
fn to_view_types(
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
) -> Result<(SchemaRef, Vec<RecordBatch>), ArrowError> {
    let view_type: Vec<Option<DataType>> = schema
        .fields()
        .iter()
        .map(|field| match field.data_type() {
            DataType::Utf8 | DataType::LargeUtf8 => Some(DataType::Utf8View),
            DataType::Binary | DataType::LargeBinary => Some(DataType::BinaryView),
            _ => None,
        })
        .collect();

    if view_type.iter().all(Option::is_none) {
        return Ok((schema, batches));
    }

    let fields: Vec<Arc<Field>> = schema
        .fields()
        .iter()
        .zip(&view_type)
        .map(|(field, target)| match target {
            Some(dt) => Arc::new(field.as_ref().clone().with_data_type(dt.clone())),
            None => field.clone(),
        })
        .collect();
    let new_schema = Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()));

    // Widening to a view type is lossless, so surface any unexpected cast error
    // (`safe: false`) rather than silently nulling values.
    let cast_options = CastOptions {
        safe: false,
        format_options: FormatOptions::default(),
    };
    let new_batches = batches
        .into_iter()
        .map(|batch| {
            let columns = batch
                .columns()
                .iter()
                .zip(&view_type)
                .map(|(col, target)| match target {
                    Some(dt) => cast_with_options(col, dt, &cast_options),
                    None => Ok(col.clone()),
                })
                .collect::<Result<Vec<ArrayRef>, ArrowError>>()?;
            RecordBatch::try_new(new_schema.clone(), columns)
        })
        .collect::<Result<Vec<RecordBatch>, ArrowError>>()?;

    Ok((new_schema, new_batches))
}

/// Convert `Utf8`/`Binary` columns to `Utf8View`/`BinaryView` and concatenate the
/// batches into a single [`RecordBatch`]. The concat itself is additionally guarded
/// by `catch_unwind`, so any residual Arrow panic surfaces as a recoverable
/// [`AdapterError`] instead of aborting the process.
pub(crate) fn concat_batches_widened(
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
) -> AdapterResult<RecordBatch> {
    let (schema, batches) = to_view_types(schema, batches).map_err(arrow_error_to_adapter_error)?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        concat_batches(&schema, &batches)
    }))
    .map_err(|_| {
        AdapterError::new(
            AdapterErrorKind::Arrow,
            "failed to materialize query result: the result set is too large to fit in a single Arrow array",
        )
    })?
    .map_err(arrow_error_to_adapter_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::cast::AsArray;
    use arrow_array::{Int64Array, StringArray};

    fn utf8_batch(values: Vec<&str>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new("s", DataType::Utf8, false)])),
            vec![Arc::new(StringArray::from(values)) as ArrayRef],
        )
        .unwrap()
    }

    #[test]
    fn to_view_types_converts_single_small_batch() {
        // Even a single, tiny batch is converted -- the output type must not depend
        // on result size or batch count.
        let batches = vec![utf8_batch(vec!["a"])];
        let schema = batches[0].schema();
        let (out_schema, out_batches) = to_view_types(schema, batches).unwrap();
        assert_eq!(out_schema.field(0).data_type(), &DataType::Utf8View);
        assert_eq!(out_batches.len(), 1);
    }

    #[test]
    fn to_view_types_converts_multi_batch() {
        let batches = vec![utf8_batch(vec!["a", "b"]), utf8_batch(vec!["c"])];
        let schema = batches[0].schema();
        let (out_schema, out_batches) = to_view_types(schema, batches).unwrap();
        assert_eq!(out_schema.field(0).data_type(), &DataType::Utf8View);
        let total = concat_batches(&out_schema, &out_batches).unwrap();
        assert_eq!(total.num_rows(), 3);
        let col = total.column(0).as_string_view();
        assert_eq!((col.value(0), col.value(1), col.value(2)), ("a", "b", "c"));
    }

    #[test]
    fn to_view_types_preserves_schema_metadata() {
        let schema = Arc::new(Schema::new_with_metadata(
            vec![Field::new("s", DataType::Utf8, false)],
            std::collections::HashMap::from([("k".to_string(), "v".to_string())]),
        ));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(StringArray::from(vec!["a"])) as ArrayRef],
        )
        .unwrap();
        let (out_schema, _) = to_view_types(schema, vec![batch]).unwrap();
        assert_eq!(out_schema.field(0).data_type(), &DataType::Utf8View);
        assert_eq!(out_schema.metadata().get("k"), Some(&"v".to_string()));
    }

    #[test]
    fn to_view_types_leaves_non_byte_columns_untouched() {
        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1])) as ArrayRef],
        )
        .unwrap();
        let (out_schema, out_batches) = to_view_types(schema, vec![batch]).unwrap();
        assert_eq!(out_schema.field(0).data_type(), &DataType::Int64);
        assert_eq!(out_batches.len(), 1);
    }

    #[test]
    #[ignore = "allocates >2 GiB; run locally to verify no i32 offset overflow"]
    fn concat_batches_widened_handles_huge_utf8_result() {
        // ~2.6 GiB of Utf8 across 3 batches, exceeding i32::MAX (~2.1 GiB). With
        // Utf8View, concatenation doesn't require one contiguous buffer, so this
        // completes without the "byte array offset overflow" panic.
        let chunk = "x".repeat(64 * 1024 * 1024); // 64 MiB per row
        let rows_per_batch = 14; // ~0.875 GiB per batch
        let make = || utf8_batch(vec![chunk.as_str(); rows_per_batch]);
        let batches = vec![make(), make(), make()];
        let schema = batches[0].schema();
        let total = concat_batches_widened(schema, batches).unwrap();
        assert_eq!(total.schema().field(0).data_type(), &DataType::Utf8View);
        assert_eq!(total.num_rows(), rows_per_batch * 3);
    }
}
