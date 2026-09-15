use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use dbt_common::collections::DashMap;
use dbt_common::constants::DBT_EPHEMERAL_DIR_NAME;
use dbt_common::constants::RENDERING;
use dbt_common::serde_utils::convert_yml_to_dash_map;
use dbt_common::stats::NodeStatus;
use dbt_common::tracing::emit::emit_debug_event;
use dbt_common::{FsResult, MacroSpansOnly, stdfs};
use dbt_jinja_utils::phases::compile::DependencyValidationConfig;
use dbt_jinja_utils::utils::{
    add_task_context, inject_and_persist_ephemeral_models, macro_spans_to_macro_span_vec,
    render_sql,
};
use dbt_scheduler::instructions::SqlInstruction;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::properties::UnitTestOverrides;
use dbt_schemas::schemas::{InternalDbtNodeAttributes, NodePathKind};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::TaskOp;
use dbt_telemetry::{CompiledCode, NodeType};
use minijinja::Value as MinijinjaValue;

use dbt_tasks_core::task::TaskResult;

use super::common::handle_render_result;
use super::unit_test;

pub async fn run_default_render(
    node: Arc<dyn InternalDbtNodeAttributes>,
    ctx: TaskRunnerCtx,
    result_sender: Option<std::sync::mpsc::SyncSender<TaskResult>>,
    local_exec_unit_test_overrides: Option<UnitTestOverrides>,
) -> FsResult<NodeStatus> {
    let render_step = Box::new(move || {
        let mut ctx = ctx;
        let res = render_default(&node, &mut ctx, &local_exec_unit_test_overrides);
        handle_render_result(
            res,
            &node.unique_id(),
            &node.materialized(),
            &mut ctx,
            &result_sender,
        )
    });
    TaskOp::Blocking(render_step).run().await?
}

fn render_default(
    node: &Arc<dyn InternalDbtNodeAttributes>,
    ctx: &mut TaskRunnerCtx,
    local_exec_unit_test_overrides: &Option<UnitTestOverrides>,
) -> FsResult<(SqlInstruction, Arc<DashMap<String, MinijinjaValue>>)> {
    report_rendering_progress(node, ctx);

    if let Some((rendered_sql_maybe_with_cte, macro_spans, reclassify_spans)) = ctx
        .inner
        .compiled_sql_cache
        .try_get_compiled_sql(&ctx.inner.arg.io, node.common())
    {
        let config_map = Arc::new(convert_yml_to_dash_map(node.serialized_config()));
        emit_compiled_code(node, ctx, &rendered_sql_maybe_with_cte);
        // The cache returns the raw span components; the rendering listener
        // factory rebuilds the `CompiledSpans`.
        let spans = ctx
            .rendering_listener_factory
            .create_spans(macro_spans, reclassify_spans);
        return Ok((
            SqlInstruction {
                fqn: vec![
                    node.base().database.clone(),
                    node.base().schema.clone(),
                    node.base().alias.clone(),
                ],
                sql: rendered_sql_maybe_with_cte,
                original_path: node.common().original_file_path.to_path_buf(),
                spans,
            },
            config_map,
        ));
    }

    let mut base_context = ctx.inner.base_context.clone();

    add_task_context(&mut base_context, node.common(), &ctx.thread_id);

    // For snapshots, `path` is the generated SQL artifact used as render input.
    // Display/error locations still use the definition path via `get_node_path`.
    let file_path = if node.resource_type() == NodeType::Snapshot {
        &node.common().path
    } else {
        &node.common().original_file_path
    };
    let absolute_path = ctx
        .inner
        .arg
        .io
        .map_to_workspace_path(file_path, node.resource_type());
    let raw_sql = stdfs::read_to_string(&absolute_path)?;

    // Python models skip Jinja rendering and use raw Python code + py_script_postfix
    if node.common().language.as_deref() == Some("python") {
        return render_python_model(node, ctx, &raw_sql, &base_context);
    }

    let (mut compile_context, config_map) = ctx.build_compile_node_context(
        node.as_ref(),
        &base_context,
        DependencyValidationConfig::new_validated(),
    )?;

    if let Some(overrides) = local_exec_unit_test_overrides {
        unit_test::apply_unit_test_overrides(&mut compile_context, overrides, ctx);
    }

    let render_file_path = node
        .get_node_path(
            NodePathKind::Definition,
            ctx.inner.arg.io.in_dir.as_path(),
            ctx.inner.arg.io.out_dir.as_path(),
        )
        .into_owned();

    let rendered_sql = render_sql(
        &raw_sql,
        &ctx.env,
        &compile_context,
        ctx.rendering_listener_factory.as_ref(),
        &render_file_path,
    )
    .map_err(|e| e.with_location(render_file_path.clone()))?;

    let mut macro_spans = ctx
        .rendering_listener_factory
        .drain_macro_spans(&render_file_path);
    let rendered_sql_maybe_with_cte = inject_and_persist_ephemeral_models(
        rendered_sql,
        &mut macro_spans,
        &node.base().alias,
        node.materialized() == DbtMaterialization::Ephemeral,
        &ctx.inner.arg.io.out_dir.join(DBT_EPHEMERAL_DIR_NAME),
    )
    .map_err(|e| e.with_location(render_file_path.clone()))?;

    let macro_spans = macro_spans_to_macro_span_vec(&macro_spans);

    let spans = ctx
        .rendering_listener_factory
        .compiled_spans(macro_spans, &render_file_path);

    ctx.inner.compiled_sql_cache.set_compiled_sql(
        &ctx.inner.arg.io,
        node.common(),
        &rendered_sql_maybe_with_cte,
        spans.as_ref(),
    )?;

    emit_compiled_code(node, ctx, &rendered_sql_maybe_with_cte);

    Ok((
        SqlInstruction {
            fqn: vec![node.database(), node.schema(), node.alias()],
            sql: rendered_sql_maybe_with_cte,
            original_path: node.common().original_file_path.to_path_buf(),
            spans,
        },
        config_map,
    ))
}

fn report_rendering_progress(node: &Arc<dyn InternalDbtNodeAttributes>, ctx: &TaskRunnerCtx) {
    let io = &ctx.inner.arg.io;

    if let Some(reporter) = io.status_reporter.as_ref() {
        // Status lines always cite the definition path so users see where they wrote the
        // code, not a phase-specific artifact. (Errors, in contrast, use phase-accurate
        // paths so the user can open the actual file that failed.)
        let display_path = node
            .get_node_path(
                NodePathKind::Definition,
                io.in_dir.as_path(),
                io.out_dir.as_path(),
            )
            .display()
            .to_string();

        reporter.show_progress(RENDERING, display_path.as_ref(), None);
    }
}

fn emit_compiled_code(
    node: &Arc<dyn InternalDbtNodeAttributes>,
    ctx: &TaskRunnerCtx,
    rendered_sql_maybe_with_cte: &str,
) {
    let io = &ctx.inner.arg.io;

    // Emit compiled SQL events for all node types. Downstream layers decide filtering.
    let compiled_absolute_path = ctx
        .inner
        .compiled_sql_cache
        .get_compiled_sql_path(io, node.common());
    let compiled_relative_path =
        stdfs::diff_paths(&compiled_absolute_path, &io.in_dir).unwrap_or(compiled_absolute_path);

    emit_debug_event(
        CompiledCode {
            relative_path: compiled_relative_path.to_string_lossy().to_string(),
            sql: rendered_sql_maybe_with_cte.to_string(),
            unique_id: node.common().unique_id.clone(),
            node_name: node.common().name.clone(),
        },
        None,
    );
}

/// Render a Python model without Jinja processing
fn render_python_model(
    node: &Arc<dyn InternalDbtNodeAttributes>,
    ctx: &mut TaskRunnerCtx,
    raw_python: &str,
    base_context: &BTreeMap<String, MinijinjaValue>,
) -> FsResult<(SqlInstruction, Arc<DashMap<String, MinijinjaValue>>)> {
    let (compile_context, config_map) = ctx.build_compile_node_context(
        node.as_ref(),
        base_context,
        DependencyValidationConfig::new_validated(),
    )?;

    let postfix_template = "{{ py_script_postfix(model) }}";
    let rendered_postfix = render_sql(
        postfix_template,
        &ctx.env,
        &compile_context,
        ctx.rendering_listener_factory.as_ref(),
        &PathBuf::from("py_script_postfix"),
    )
    .map_err(|e| *e)?;

    let compiled_python = format!("{}\n\n{}", raw_python.trim_end(), rendered_postfix);

    emit_compiled_code(node, ctx, &compiled_python);

    ctx.inner.compiled_sql_cache.set_compiled_sql(
        &ctx.inner.arg.io,
        node.common(),
        &compiled_python,
        &MacroSpansOnly::default(),
    )?;

    Ok((
        SqlInstruction {
            fqn: vec![
                node.base().database.clone(),
                node.base().schema.clone(),
                node.base().alias.clone(),
            ],
            sql: compiled_python,
            original_path: node.common().original_file_path.to_path_buf(),
            spans: Box::<MacroSpansOnly>::default(),
        },
        config_map,
    ))
}
