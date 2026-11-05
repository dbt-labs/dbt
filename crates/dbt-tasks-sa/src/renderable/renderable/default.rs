use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dbt_common::collections::SccHashMap;
use dbt_common::constants::DBT_EPHEMERAL_DIR_NAME;
use dbt_common::constants::RENDERING;
use dbt_common::serde_utils::convert_yml_to_scc_map;
use dbt_common::stats::NodeStatus;
use dbt_common::tracing::emit::emit_debug_event;
use dbt_common::{ErrorCode, FsResult, MacroSpansOnly, fs_err, stdfs};
use dbt_jinja_utils::phases::compile::DependencyValidationConfig;
use dbt_jinja_utils::render_trace::RenderTrace;
use dbt_jinja_utils::utils::{
    add_task_context, inject_and_persist_ephemeral_models, macro_spans_to_macro_span_vec,
    render_sql, render_sql_with_trace,
};
use dbt_scheduler::instructions::SqlInstruction;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::properties::UnitTestOverrides;
use dbt_schemas::schemas::{InternalDbtNodeAttributes, NodePathKind};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::run_blocking_task_operation_with_wait_parents;
use dbt_telemetry::{CompiledCode, NodeType};
use minijinja::Value as MinijinjaValue;
use serde::Serialize;

use dbt_tasks_core::task::TaskResult;

use super::common::handle_render_result;
use super::unit_test;

pub async fn run_default_render(
    node: Arc<dyn InternalDbtNodeAttributes>,
    ctx: TaskRunnerCtx,
    result_sender: Option<std::sync::mpsc::SyncSender<TaskResult>>,
    local_exec_unit_test_overrides: Option<UnitTestOverrides>,
    wait_parents: Option<Vec<tracing::span::Id>>,
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
    run_blocking_task_operation_with_wait_parents(render_step, wait_parents).await?
}

fn render_default(
    node: &Arc<dyn InternalDbtNodeAttributes>,
    ctx: &mut TaskRunnerCtx,
    local_exec_unit_test_overrides: &Option<UnitTestOverrides>,
) -> FsResult<(SqlInstruction, Arc<SccHashMap<String, MinijinjaValue>>)> {
    report_rendering_progress(node, ctx);

    if let Some(cached) = cached_render(node, ctx) {
        return Ok(cached);
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
        node.clone(),
        &base_context,
        DependencyValidationConfig::new_validated(),
    )?;

    if let Some(overrides) = local_exec_unit_test_overrides {
        unit_test::apply_unit_test_overrides(&mut compile_context, overrides, ctx)?;
    }

    let render_file_path = node
        .get_node_path(
            NodePathKind::Definition,
            ctx.inner.arg.io.in_dir.as_path(),
            ctx.inner.arg.io.out_dir.as_path(),
        )
        .into_owned();

    let rendered_sql = render_model_sql(
        node,
        ctx,
        &compile_context,
        &ModelRenderInput {
            source: &raw_sql,
            filename: &render_file_path,
            source_path: &absolute_path,
        },
    )?;

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

/// Restore the complete rendered result only when execution events are not needed.
fn cached_render(
    node: &Arc<dyn InternalDbtNodeAttributes>,
    ctx: &TaskRunnerCtx,
) -> Option<(SqlInstruction, Arc<SccHashMap<String, MinijinjaValue>>)> {
    // The compiled-SQL cache does not store render events. Trace requests must
    // observe a fresh execution rather than reuse SQL alone.
    if dbt_flags::WRITE_RENDER_TRACE.enabled() {
        return None;
    }
    let (rendered_sql, macro_spans, reclassify_spans) = ctx
        .inner
        .compiled_sql_cache
        .try_get_compiled_sql(&ctx.inner.arg.io, node.common())?;
    let config_map = Arc::new(convert_yml_to_scc_map(node.serialized_config()));
    emit_compiled_code(node, ctx, &rendered_sql);
    // The cache returns raw components; the factory rebuilds CompiledSpans.
    let spans = ctx
        .rendering_listener_factory
        .create_spans(macro_spans, reclassify_spans);
    Some((
        SqlInstruction {
            fqn: vec![
                node.base().database.clone(),
                node.base().schema.clone(),
                node.base().alias.clone(),
            ],
            sql: rendered_sql,
            original_path: node.common().original_file_path.to_path_buf(),
            spans,
        },
        config_map,
    ))
}

/// The exact Jinja input and its display and on-disk source identities.
struct ModelRenderInput<'a> {
    source: &'a str,
    filename: &'a Path,
    source_path: &'a Path,
}

fn render_model_sql(
    node: &Arc<dyn InternalDbtNodeAttributes>,
    ctx: &TaskRunnerCtx,
    compile_context: &BTreeMap<String, MinijinjaValue>,
    input: &ModelRenderInput<'_>,
) -> FsResult<String> {
    if dbt_flags::WRITE_RENDER_TRACE.enabled() {
        let trace = render_sql_with_trace(
            input.source,
            &ctx.env,
            compile_context,
            ctx.rendering_listener_factory.as_ref(),
            input.filename,
        )
        .map_err(|error| error.with_location(input.filename.to_path_buf()))?;
        write_model_render_trace(node, ctx, input, &trace)?;
        Ok(trace.rendered)
    } else {
        Ok(render_sql(
            input.source,
            &ctx.env,
            compile_context,
            ctx.rendering_listener_factory.as_ref(),
            input.filename,
        )
        .map_err(|error| error.with_location(input.filename.to_path_buf()))?)
    }
}

#[derive(Serialize)]
struct ModelRenderTrace<'a> {
    protocol_version: u32,
    dbt_version: &'static str,
    invocation_id: String,
    unique_id: String,
    filename: &'a Path,
    source_path: &'a Path,
    render_stage: &'static str,
    #[serde(flatten)]
    trace: &'a RenderTrace,
}

fn write_model_render_trace(
    node: &Arc<dyn InternalDbtNodeAttributes>,
    ctx: &TaskRunnerCtx,
    input: &ModelRenderInput<'_>,
    trace: &RenderTrace,
) -> FsResult<()> {
    let io = &ctx.inner.arg.io;
    let path = ctx
        .inner
        .compiled_sql_cache
        .get_compiled_sql_path(io, node.common())
        .with_extension("render_trace.json");
    let artifact = ModelRenderTrace {
        protocol_version: 1,
        dbt_version: env!("CARGO_PKG_VERSION"),
        invocation_id: io.invocation_id.to_string(),
        unique_id: node.unique_id(),
        filename: input.filename,
        source_path: input.source_path,
        render_stage: "jinja_before_ephemeral_injection",
        trace,
    };
    let bytes = serde_json::to_vec_pretty(&artifact).map_err(|error| {
        fs_err!(
            ErrorCode::SerializationError,
            "Failed to serialize render trace for '{}': {error}",
            node.unique_id()
        )
    })?;
    if let Some(parent) = path.parent() {
        stdfs::create_dir_all(parent)?;
    }
    stdfs::write(path, bytes)
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
) -> FsResult<(SqlInstruction, Arc<SccHashMap<String, MinijinjaValue>>)> {
    let (compile_context, config_map) = ctx.build_compile_node_context(
        node.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_jinja_utils::jinja_environment::JinjaEnv;
    use dbt_jinja_utils::listener::DefaultRenderingEventListenerFactory;
    use minijinja::Environment;

    #[test]
    fn render_trace_example_matches_fixture() {
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/render_trace/example.sql"
        ));
        let mut environment = Environment::new();
        environment.set_keep_trailing_newline(true);
        let filename = Path::new("models/trace.sql");
        let trace = render_sql_with_trace(
            source,
            &JinjaEnv::new(environment),
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            filename,
        )
        .unwrap();
        // Fixed producer metadata keeps the example independent of releases,
        // machines and invocation IDs. The artifact uses the production type.
        let artifact = ModelRenderTrace {
            protocol_version: 1,
            dbt_version: "0.0.0-example",
            invocation_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            unique_id: "model.example.trace".to_owned(),
            filename,
            source_path: filename,
            render_stage: "jinja_before_ephemeral_injection",
            trace: &trace,
        };
        let expected: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/render_trace/example.render_trace.json"
        )))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&artifact).unwrap(),
            expected,
            "Rendered trace fixture:\n{}",
            serde_json::to_string_pretty(&artifact).unwrap()
        );
        assert!(trace.rendered.contains("select 2 as n union all"));
        assert!(trace.rendered.contains("select 4 as n"));
        assert!(!trace.rendered.contains("skipped"));
        for kind in [
            "literal",
            "templated",
            "comment",
            "loop_iteration",
            "loop_skipped",
        ] {
            assert!(
                trace.events.iter().any(|event| event.kind == kind),
                "missing {kind}"
            );
        }
    }
}
