use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc;

use dbt_common::pretty_string::make_title;
use dbt_common::stats::NodeStatus;
use dbt_common::tracing::{
    dbt_emit::{emit_error_log_from_fs_error, emit_warn_log_message},
    emit::emit_info_event,
};
use dbt_common::{ErrorCode, FsResult, MacroSpansOnly, err, io_args};
use dbt_jinja_utils::utils::{macro_spans_to_macro_span_vec, render_sql};
use dbt_pretty_table::{make_column_names, pretty_data_table};
use dbt_scheduler::instructions::{Instruction, SqlInstruction};
use dbt_schemas::schemas::profiles::Execute;
use dbt_schemas::schemas::telemetry::NodeType;
use dbt_schemas::schemas::{InternalDbtNodeAttributes, Nodes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::pretty_table::from_pretty_table_error;
use dbt_tasks_core::show_task_hooks::ShowTaskHooks;
use dbt_tasks_core::task::TaskOp;
use dbt_tasks_core::task::TaskResult;
use dbt_tasks_core::task::{TP, Task};
use dbt_telemetry::{ShowDataOutput, ShowDataOutputFormat};

use minijinja::Value as MinijinjaValue;

mod analysis;
mod model;
mod seed;
mod snapshot;
mod test;

pub trait Showable: InternalDbtNodeAttributes {
    fn visit_show<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
        result_receiver: &'a mut Option<mpsc::Receiver<TaskResult>>,
        show_task_hooks: &'a Arc<dyn ShowTaskHooks>,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>>;
}

pub(super) async fn run_show<F>(
    ctx: &mut TaskRunnerCtx,
    unique_id: &str,
    resource_label: &str,
    is_inline: bool,
    node_name: &str,
    show_task_hooks: &Arc<dyn ShowTaskHooks>,
    base_query_provider: F,
) -> FsResult<NodeStatus>
where
    F: FnOnce(&TaskRunnerCtx) -> FsResult<String>,
{
    let query = base_query_provider(ctx)?;
    let limit = ctx.inner.arg.limit.filter(|limit| *limit > 0);

    // `--query-id` always goes through the adhoc_runner path (RemoteAdhocRunner
    // is the only one that knows how to fetch an existing query's result
    // directly) -- never the local sidecar worker, which would just execute
    // the placeholder SQL and silently ignore the requested query id.
    let use_worker_backend = ctx.inner.arg.query_id.is_none()
        && matches!(ctx.inner.execute, Execute::Sidecar | Execute::Service)
        && ctx.is_sidecar();

    let mut compile_ctx = make_show_compile_context(ctx)?;
    compile_ctx.insert(
        "compiled_code".to_string(),
        MinijinjaValue::from(query.as_str()),
    );
    compile_ctx.insert(
        "show_limit".to_string(),
        match limit {
            Some(l) => MinijinjaValue::from(l as i64),
            None => MinijinjaValue::from(()),
        },
    );

    let filename = PathBuf::from(format!("show-{unique_id}"));

    // `get_show_sql` dispatches by adapter and a project can override it with a
    // macro that reaches the adapter, so this render goes to the blocking pool:
    // every database connection must be created by a `dbt-runtime` worker thread.
    let final_rendered_sql = {
        let template = if use_worker_backend {
            // Worker backend applies limit separately via run_query_batches; render the query as-is.
            query.clone()
        } else {
            // Non-worker path: delegate limit handling to the adapter-aware get_show_sql macro,
            // which dispatches to adapter-specific limit syntax (e.g. FETCH FIRST n ROWS ONLY for Fabric).
            "{{ get_show_sql(compiled_code, none, show_limit) }}".to_owned()
        };
        let env = Arc::clone(&ctx.env);
        let listener_factory = Arc::clone(&ctx.rendering_listener_factory);
        let filename = filename.clone();
        TaskOp::Blocking(Box::new(move || {
            render_sql(&template, &env, &compile_ctx, &*listener_factory, &filename)
        }))
        .run()
        .await??
    };
    let macro_spans = ctx.rendering_listener_factory.drain_macro_spans(&filename);

    let object_name = format!("show_{}", unique_id.replace('.', "_"));
    let fqn = vec![
        ctx.dbt_profile().database.to_string(),
        ctx.dbt_profile().schema.to_string(),
        object_name.clone(),
    ];

    let sql_instruction = SqlInstruction {
        fqn,
        sql: final_rendered_sql.clone(),
        original_path: filename,
        spans: Box::new(MacroSpansOnly {
            macro_spans: macro_spans_to_macro_span_vec(&macro_spans),
        }),
    };

    let batches_result = if use_worker_backend {
        match show_task_hooks
            .run_show_query_batches(
                ctx,
                unique_id,
                final_rendered_sql.clone(),
                limit.map(|v| v as u64),
            )
            .await
        {
            Some(result) => result,
            None => {
                return Err(dbt_common::fs_err!(
                    ErrorCode::Unexpected,
                    "show hook returned None on sidecar path; worker backend unavailable"
                ));
            }
        }
    } else {
        Arc::clone(&ctx.inner.adhoc_runner)
            .run_adhoc(
                &Instruction::Sql(sql_instruction),
                &final_rendered_sql,
                Some(unique_id),
                &mut None,
            )
            .await
    };

    let (batches, schema) = match batches_result {
        Ok(batches) => batches,
        Err(e) => {
            let error_message = e.to_string();
            emit_error_log_from_fs_error(*e);
            *ctx.inner.preview_error.lock() = Some(error_message);
            return Ok(NodeStatus::Errored);
        }
    };

    // Capture for LSP preview path. No-op if no caller is reading preview_result.
    {
        let mut guard = ctx.inner.preview_results.lock();
        *guard = Some((batches.clone(), schema.clone()));
    }

    if batches.is_empty() && schema.fields().is_empty() {
        emit_warn_log_message(
            ErrorCode::NoResultsToShow,
            format!("No data to show for {}: {}", resource_label, unique_id),
        );

        return Ok(NodeStatus::Succeeded);
    }

    let display_format = match ctx.inner.arg.format {
        io_args::DisplayFormat::Table => dbt_pretty_table::DisplayFormat::Table,
        io_args::DisplayFormat::Csv => dbt_pretty_table::DisplayFormat::Csv,
        io_args::DisplayFormat::Tsv => dbt_pretty_table::DisplayFormat::Tsv,
        io_args::DisplayFormat::Json => dbt_pretty_table::DisplayFormat::Json,
        io_args::DisplayFormat::NdJson => dbt_pretty_table::DisplayFormat::NdJson,
        io_args::DisplayFormat::Yml => dbt_pretty_table::DisplayFormat::Yml,
        io_args::DisplayFormat::Selector => dbt_pretty_table::DisplayFormat::Selector,
        io_args::DisplayFormat::Name => dbt_pretty_table::DisplayFormat::Name,
        io_args::DisplayFormat::Path => dbt_pretty_table::DisplayFormat::Path,
    };
    let column_names = make_column_names(schema.as_ref());
    let mut title = make_title("Query", object_name.as_str());
    // Surface the dbt-compute query id, if this ran via lake compute: the only
    // way to later recover this result via dbt-compute's
    // `/download_credentials` endpoint, and otherwise never shown to the user.
    if let Some(query_id) = schema
        .metadata()
        .get(dbt_adbc::lake_compute::schema_metadata::QUERY_ID)
        .filter(|id| !id.is_empty())
    {
        title = format!("{title} (query_id={query_id})");
    }
    let table = pretty_data_table(
        &title,
        "",
        &column_names,
        batches.as_slice(),
        display_format,
        ctx.inner.arg.limit,
        true,
        None,
    )
    .map_err(from_pretty_table_error)?;

    let output_format = match ctx.inner.arg.format {
        io_args::DisplayFormat::Table => ShowDataOutputFormat::Text,
        io_args::DisplayFormat::Csv => ShowDataOutputFormat::Csv,
        io_args::DisplayFormat::Tsv => ShowDataOutputFormat::Tsv,
        io_args::DisplayFormat::Json => ShowDataOutputFormat::Json,
        io_args::DisplayFormat::NdJson => ShowDataOutputFormat::Ndjson,
        io_args::DisplayFormat::Yml => ShowDataOutputFormat::Yml,
        _ => {
            // TODO(mike.perlov): we should limit the argument that is passed
            // to command itself, rather than checking so late, but this is
            // currently blocked by EvalArgs using a single unified DisplayFormat
            // for multiple commands.
            return err!(
                ErrorCode::UnsupportedFeature,
                "DisplayFormat::{:?} is not supported for show command",
                ctx.inner.arg.format
            );
        }
    };
    let event = ShowDataOutput::new_with_default_code(
        output_format,
        table,
        node_name.to_string(),
        is_inline,
        // TODO: call-sites should probably not pass a unique_id for inline nodes
        if is_inline {
            None
        } else {
            Some(unique_id.to_string())
        },
        column_names,
    );
    emit_info_event(event, None);

    Ok(NodeStatus::Succeeded)
}

pub(super) fn rendered_sql_for(
    ctx: &TaskRunnerCtx,
    unique_id: &str,
    resource_label: &str,
) -> FsResult<String> {
    let rendered_node_info = ctx.inner.rendered_sql.get(unique_id).ok_or_else(|| {
        dbt_common::fs_err!(
            ErrorCode::Unexpected,
            "No rendered SQL found for {resource_label}: {}",
            unique_id
        )
    })?;

    Ok(rendered_node_info.sql.clone())
}

fn make_show_compile_context(ctx: &TaskRunnerCtx) -> FsResult<BTreeMap<String, MinijinjaValue>> {
    Ok(ctx.inner.base_context.clone())
}

/// A task that shows the compiled SQL results for a node
pub struct ShowableTask {
    inner: Arc<dyn Showable>,
    // Channel receiver for getting results from a previous task (e.g. analyze)
    result_receiver: parking_lot::Mutex<Option<mpsc::Receiver<TaskResult>>>,
    show_task_hooks: Arc<dyn ShowTaskHooks>,
}

impl ShowableTask {
    pub fn new(
        inner: Arc<dyn Showable>,
        result_receiver: Option<mpsc::Receiver<TaskResult>>,
        show_task_hooks: Arc<dyn ShowTaskHooks>,
    ) -> Self {
        Self {
            inner,
            result_receiver: parking_lot::Mutex::new(result_receiver),
            show_task_hooks,
        }
    }
}

impl Task for ShowableTask {
    fn run_task<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let mut result_receiver = { self.result_receiver.lock().take() };
            self.inner
                .visit_show(ctx, &mut result_receiver, &self.show_task_hooks)
                .await
        })
    }

    fn task_type(&self) -> &str {
        "show"
    }

    fn resource_type(&self) -> NodeType {
        self.inner.resource_type()
    }

    fn work_node_id(&self) -> &str {
        self.inner.common().unique_id.as_str()
    }

    fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
        vec![self.inner.clone()]
    }

    fn task_phase(&self) -> Option<TP> {
        Some(TP::Show)
    }
}

pub fn showable_task(nodes: &Nodes, unique_id: &str) -> Option<Arc<dyn Showable>> {
    if let Some(model) = nodes.models.get(unique_id) {
        Some(model.clone() as Arc<dyn Showable>)
    } else if let Some(seed) = nodes.seeds.get(unique_id) {
        Some(seed.clone() as Arc<dyn Showable>)
    } else if let Some(snapshot) = nodes.snapshots.get(unique_id) {
        Some(snapshot.clone() as Arc<dyn Showable>)
    } else if let Some(analysis) = nodes.analyses.get(unique_id) {
        Some(analysis.clone() as Arc<dyn Showable>)
    } else if let Some(test) = nodes.tests.get(unique_id) {
        Some(test.clone() as Arc<dyn Showable>)
    } else {
        None
    }
}
