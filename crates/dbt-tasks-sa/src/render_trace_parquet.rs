//! Adapter from task render traces to the metadata Parquet schema.

use dbt_common::FsResult;
use dbt_metadata_parquet::parse_alive::read_alive_ids;
use dbt_metadata_parquet::render_traces::{
    RenderTraceEventRow, RenderTraceRow, write_render_traces,
};
use dbt_tasks_core::context::TaskRunnerCtx;

pub(crate) fn write_collected_render_traces(ctx: &TaskRunnerCtx) -> FsResult<()> {
    if !dbt_flags::WRITE_RENDER_TRACE.enabled() {
        return Ok(());
    }

    // A partially loaded resolver state omits live, unselected nodes. Only the
    // project-wide parse snapshot can authorize pruning their retained traces.
    let valid_ids = if ctx.inner.arg.write_metadata {
        read_alive_ids(&ctx.inner.arg.metadata_dir.join("parse/alive.parquet"))?
    } else {
        None
    };
    let ingested_at = chrono::Utc::now().timestamp_micros();
    let rows = std::mem::take(&mut *ctx.inner.render_traces.lock())
        .into_iter()
        .map(|trace| RenderTraceRow {
            protocol_version: i64::from(trace.protocol_version),
            ingested_at,
            dbt_version: trace.dbt_version.to_owned(),
            invocation_id: trace.invocation_id,
            unique_id: trace.unique_id,
            filename: trace.filename.to_string_lossy().into_owned(),
            source_path: trace.source_path.to_string_lossy().into_owned(),
            render_stage: trace.render_stage.to_owned(),
            source: trace.trace.source,
            rendered: trace.trace.rendered,
            events: trace
                .trace
                .events
                .into_iter()
                .map(|event| RenderTraceEventRow {
                    kind: event.kind,
                    source_start: event.source_start as i64,
                    source_end: event.source_end as i64,
                    rendered_start: event.rendered_start as i64,
                    rendered_end: event.rendered_end as i64,
                })
                .collect(),
        })
        .collect();
    let dir = ctx
        .inner
        .arg
        .metadata_dir
        .join("compile")
        .join("render_traces");
    let recomputed_ids = std::mem::take(&mut *ctx.inner.render_trace_attempts.lock());
    write_render_traces(&dir, rows, &recomputed_ids, valid_ids.as_ref())
}
