//! Invocation-wide nested Parquet artifact for Jinja render provenance.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{
    Array, ArrayRef, ListArray, StringArray, StructArray, UInt32Array, UInt64Array,
};
use arrow::buffer::{OffsetBuffer, ScalarBuffer};
use arrow::datatypes::{DataType, Field, Fields, Schema};
use arrow::record_batch::RecordBatch;
use dbt_common::{ErrorCode, FsResult, fs_err, stdfs};
use dbt_tasks_core::context::{ModelRenderTrace, TaskRunnerCtx};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

const RENDER_TRACE_FILE_NAME: &str = "render_traces.parquet";

pub(crate) fn prepare_render_trace_output(ctx: &TaskRunnerCtx) -> FsResult<()> {
    if !dbt_flags::WRITE_RENDER_TRACE.enabled() {
        return Ok(());
    }
    let path = ctx.inner.arg.io.out_dir.join(RENDER_TRACE_FILE_NAME);
    let tmp_path = path.with_extension("parquet.tmp");
    for stale in [&path, &tmp_path] {
        if stale.exists() {
            stdfs::remove_file(stale)?;
        }
    }
    Ok(())
}

pub(crate) fn write_collected_render_traces(ctx: &TaskRunnerCtx) -> FsResult<()> {
    if !dbt_flags::WRITE_RENDER_TRACE.enabled() {
        return Ok(());
    }

    let mut traces = std::mem::take(&mut *ctx.inner.render_traces.lock());
    if traces.is_empty() {
        return Ok(());
    }
    traces.sort_unstable_by(|left, right| left.unique_id.cmp(&right.unique_id));

    let path = ctx.inner.arg.io.out_dir.join(RENDER_TRACE_FILE_NAME);
    write_render_trace_file(&path, &traces)
}

fn write_render_trace_file(path: &Path, traces: &[ModelRenderTrace]) -> FsResult<()> {
    let tmp_path = path.with_extension("parquet.tmp");
    let batch = render_trace_batch(traces)?;
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap()))
        .build();
    let file = File::create(&tmp_path)?;
    let mut writer =
        ArrowWriter::try_new(file, batch.schema(), Some(properties)).map_err(|error| {
            fs_err!(
                ErrorCode::SerializationError,
                "Failed to create render trace Parquet writer: {error}"
            )
        })?;
    writer.write(&batch).map_err(|error| {
        fs_err!(
            ErrorCode::SerializationError,
            "Failed to write render trace Parquet batch: {error}"
        )
    })?;
    writer.close().map_err(|error| {
        fs_err!(
            ErrorCode::SerializationError,
            "Failed to close render trace Parquet writer: {error}"
        )
    })?;
    stdfs::rename(tmp_path, path)?;
    Ok(())
}

fn render_trace_batch(traces: &[ModelRenderTrace]) -> FsResult<RecordBatch> {
    let event_fields = Fields::from(vec![
        Field::new("kind", DataType::Utf8, false),
        Field::new("source_start", DataType::UInt64, false),
        Field::new("source_end", DataType::UInt64, false),
        Field::new("rendered_start", DataType::UInt64, false),
        Field::new("rendered_end", DataType::UInt64, false),
    ]);
    let event_type = DataType::Struct(event_fields.clone());
    let schema = Arc::new(Schema::new(vec![
        Field::new("protocol_version", DataType::UInt32, false),
        Field::new("dbt_version", DataType::Utf8, false),
        Field::new("invocation_id", DataType::Utf8, false),
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("filename", DataType::Utf8, false),
        Field::new("source_path", DataType::Utf8, false),
        Field::new("render_stage", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("rendered", DataType::Utf8, false),
        Field::new(
            "events",
            DataType::List(Arc::new(Field::new("item", event_type, false))),
            false,
        ),
    ]));

    let mut offsets = Vec::with_capacity(traces.len() + 1);
    let mut kinds = Vec::new();
    let mut source_starts = Vec::new();
    let mut source_ends = Vec::new();
    let mut rendered_starts = Vec::new();
    let mut rendered_ends = Vec::new();
    offsets.push(0_i32);
    for trace in traces {
        for event in trace.trace.events.iter() {
            kinds.push(event.kind.as_str());
            source_starts.push(event.source_start as u64);
            source_ends.push(event.source_end as u64);
            rendered_starts.push(event.rendered_start as u64);
            rendered_ends.push(event.rendered_end as u64);
        }
        offsets.push(i32::try_from(kinds.len()).map_err(|_| {
            fs_err!(
                ErrorCode::SerializationError,
                "Render trace contains too many events for an Arrow List"
            )
        })?);
    }

    let event_values = StructArray::new(
        event_fields,
        vec![
            Arc::new(StringArray::from(kinds)) as ArrayRef,
            Arc::new(UInt64Array::from(source_starts)),
            Arc::new(UInt64Array::from(source_ends)),
            Arc::new(UInt64Array::from(rendered_starts)),
            Arc::new(UInt64Array::from(rendered_ends)),
        ],
        None,
    );
    let events = ListArray::new(
        Arc::new(Field::new("item", event_values.data_type().clone(), false)),
        OffsetBuffer::new(ScalarBuffer::from(offsets)),
        Arc::new(event_values),
        None,
    );

    let columns: Vec<ArrayRef> = vec![
        Arc::new(UInt32Array::from_iter_values(
            traces.iter().map(|trace| trace.protocol_version),
        )),
        Arc::new(StringArray::from_iter_values(
            traces.iter().map(|trace| trace.dbt_version),
        )),
        Arc::new(StringArray::from_iter_values(
            traces.iter().map(|trace| trace.invocation_id.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            traces.iter().map(|trace| trace.unique_id.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            traces.iter().map(|trace| trace.filename.to_string_lossy()),
        )),
        Arc::new(StringArray::from_iter_values(
            traces
                .iter()
                .map(|trace| trace.source_path.to_string_lossy()),
        )),
        Arc::new(StringArray::from_iter_values(
            traces.iter().map(|trace| trace.render_stage),
        )),
        Arc::new(StringArray::from_iter_values(
            traces.iter().map(|trace| trace.trace.source.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            traces.iter().map(|trace| trace.trace.rendered.as_str()),
        )),
        Arc::new(events),
    ];
    RecordBatch::try_new(schema, columns).map_err(|error| {
        fs_err!(
            ErrorCode::SerializationError,
            "Failed to construct render trace Arrow batch: {error}"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::StringArray;
    use dbt_jinja_utils::render_trace::{RenderTrace, RenderTraceEvent, RenderTraceEvents};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use std::path::PathBuf;

    fn artifact(unique_id: &str, kind: &str) -> ModelRenderTrace {
        ModelRenderTrace {
            protocol_version: 2,
            dbt_version: "0.0.0-test",
            invocation_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            unique_id: unique_id.to_owned(),
            filename: PathBuf::from("models/example.sql"),
            source_path: PathBuf::from("models/example.sql"),
            render_stage: "jinja_before_ephemeral_injection",
            trace: RenderTrace {
                source: "select {{ value }}".to_owned(),
                rendered: "select 1".to_owned(),
                events: RenderTraceEvents::from(vec![RenderTraceEvent {
                    kind: kind.to_owned(),
                    source_start: 0,
                    source_end: 18,
                    rendered_start: 0,
                    rendered_end: 8,
                }]),
            },
        }
    }

    #[test]
    fn writes_one_ordered_nested_parquet_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RENDER_TRACE_FILE_NAME);
        let traces = vec![
            artifact("model.example.second", "templated"),
            artifact("model.example.first", "literal"),
        ];
        write_render_trace_file(&path, &traces).unwrap();

        let file = File::open(path).unwrap();
        let mut reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(
            batch
                .schema()
                .field_with_name("events")
                .unwrap()
                .data_type(),
            &DataType::List(Arc::new(Field::new(
                "item",
                DataType::Struct(Fields::from(vec![
                    Field::new("kind", DataType::Utf8, false),
                    Field::new("source_start", DataType::UInt64, false),
                    Field::new("source_end", DataType::UInt64, false),
                    Field::new("rendered_start", DataType::UInt64, false),
                    Field::new("rendered_end", DataType::UInt64, false),
                ])),
                false,
            )))
        );
        let unique_ids = batch
            .column_by_name("unique_id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(unique_ids.value(0), "model.example.second");
        assert_eq!(unique_ids.value(1), "model.example.first");
    }
}
