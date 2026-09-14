use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dbt_common::{
    ErrorCode, FsError, FsResult,
    constants::{DBT_CTE_PREFIX, DBT_EPHEMERAL_DIR_NAME},
    fs_err,
    io_args::IoArgs,
    stdfs, tokiofs, unexpected_fs_err,
};
use dbt_dag::deps_mgmt::topological_sort;
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv,
    listener::{DefaultRenderingEventListenerFactory, RenderingEventListenerFactory},
    phases::{
        MacroLookupContext,
        compile::{DependencyValidationConfig, build_compile_node_context},
        run::{WriteConfig, extend_base_context_stateful_fn},
    },
    utils::{inject_and_persist_ephemeral_models, render_sql},
};
use dbt_schemas::schemas::{
    ContextRunResult, InternalDbtNode, InternalDbtNodeAttributes, NodePathKind, TimingInfo,
    common::DbtMaterialization, manifest::DbtOperation,
};
use dbt_schemas::state::ResolverState;
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_telemetry::NodeType;
use dbt_yaml::Spanned;
use minijinja::{
    MacroSpans, Value, Value as MinijinjaValue,
    constants::{CURRENT_PATH, CURRENT_SPAN, TARGET_PACKAGE_NAME},
    value::Kwargs,
};
use minijinja_contrib::modules::py_datetime::datetime::PyDateTime;

/// Synthetic node name for ad-hoc `dbt run-operation --sql ...` invocations.
pub const INLINE_SQL_NAME: &str = "inline_query";
const INLINE_SQL_PAYLOAD_VAR: &str = "__dbt_inline_sql_payload__";

/// Renders an ad-hoc SQL/Jinja string (`ref`/`source`/`var`/`target` available)
/// and executes it via `{% call statement(...) %}`; returns the rendered SQL.
pub async fn run_operation_inline_sql(
    sql: &str,
    resolver_state: &Arc<ResolverState>,
    jinja_env: &Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
) -> FsResult<String> {
    if sql.trim().is_empty() {
        return Err(fs_err!(
            ErrorCode::Generic,
            "--sql must be a non-empty SQL/Jinja string"
        ));
    }

    let mut ctx = base_context.clone();
    ctx.insert(
        TARGET_PACKAGE_NAME.to_string(),
        Value::from(resolver_state.root_project_name.clone()),
    );

    let mut packages: BTreeSet<String> = resolver_state
        .runtime_config
        .dependencies
        .keys()
        .cloned()
        .collect();
    packages.insert(resolver_state.root_project_name.clone());
    ctx.insert(
        "context".to_owned(),
        MinijinjaValue::from_object(MacroLookupContext::new(
            resolver_state.root_project_name.clone(),
            None,
            packages,
        )),
    );

    ctx.insert(CURRENT_PATH.to_string(), Value::from("<inline_sql>"));
    ctx.insert(
        CURRENT_SPAN.to_string(),
        Value::from_serialize(minijinja::machinery::Span::default()),
    );

    let listener_factory = Arc::new(DefaultRenderingEventListenerFactory::default());
    let filename = io_args.in_dir.join("inline_query.sql");
    // Rendering may reach the warehouse (and the `statement` wrapper below always
    // does), so it happens on the blocking pool: every database connection must be
    // created by a `dbt-runtime` worker thread.
    let rendered_user_sql = {
        let jinja_env = Arc::clone(jinja_env);
        let sql = sql.to_owned();
        let ctx = ctx.clone();
        let listener_factory = Arc::clone(&listener_factory);
        let filename = filename.clone();
        dbt_runtime::spawn_blocking(move || {
            render_sql(&sql, &jinja_env, &ctx, listener_factory.as_ref(), &filename)
        })
        .await
        .map_err(|e| fs_err!(ErrorCode::Generic, "spawn_blocking join error: {}", e))??
    };

    let final_user_sql = if rendered_user_sql.contains(DBT_CTE_PREFIX) {
        // Compiling the ephemeral models renders their bodies, which may call the
        // adapter, so it belongs on the blocking pool like the renders above.
        {
            let resolver_state = Arc::clone(resolver_state);
            let jinja_env = Arc::clone(jinja_env);
            let base_context = base_context.clone();
            let io_args = io_args.clone();
            dbt_runtime::spawn_blocking(move || {
                precompile_ephemeral_models(&resolver_state, &jinja_env, &base_context, &io_args)
            })
            .await
            .map_err(|e| fs_err!(ErrorCode::Generic, "spawn_blocking join error: {}", e))??;
        }
        let mut macro_spans = MacroSpans::default();
        inject_and_persist_ephemeral_models(
            rendered_user_sql,
            &mut macro_spans,
            INLINE_SQL_NAME,
            /* is_current_model_ephemeral = */ false,
            &io_args.out_dir.join(DBT_EPHEMERAL_DIR_NAME),
        )?
    } else {
        rendered_user_sql
    };

    let mut wrap_ctx = ctx.clone();
    wrap_ctx.insert(
        INLINE_SQL_PAYLOAD_VAR.to_owned(),
        Value::from(final_user_sql.clone()),
    );
    let wrapper = format!(
        "{{% call statement('{name}', auto_begin=false, fetch_result=false) %}}\n{{{{ {var} }}}}\n{{% endcall %}}",
        name = INLINE_SQL_NAME,
        var = INLINE_SQL_PAYLOAD_VAR,
    );
    {
        let jinja_env = Arc::clone(jinja_env);
        let _ = dbt_runtime::spawn_blocking(move || {
            render_sql(
                &wrapper,
                &jinja_env,
                &wrap_ctx,
                listener_factory.as_ref(),
                &filename,
            )
        })
        .await
        .map_err(|e| fs_err!(ErrorCode::Generic, "spawn_blocking join error: {}", e))??;
    }

    Ok(final_user_sql)
}

/// Compile every ephemeral model in dependency order and persist its CTE body
/// to `target/<ephemeral_dir>/<name>.sql` so that
/// [`inject_and_persist_ephemeral_models`] can find each upstream when it
/// inlines CTEs into a downstream query.
fn precompile_ephemeral_models(
    resolver_state: &ResolverState,
    jinja_env: &JinjaEnv,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
) -> FsResult<()> {
    // Collect ephemeral models keyed by unique_id.
    let ephemerals: BTreeMap<String, Arc<dbt_schemas::schemas::DbtModel>> = resolver_state
        .nodes
        .models
        .iter()
        .filter(|(_, m)| matches!(m.as_ref().materialized(), DbtMaterialization::Ephemeral))
        .map(|(uid, m)| (uid.clone(), m.clone()))
        .collect();

    if ephemerals.is_empty() {
        return Ok(());
    }

    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (uid, model) in &ephemerals {
        let parent_set: BTreeSet<String> = model
            .__base_attr__
            .depends_on
            .nodes
            .iter()
            .filter(|d| ephemerals.contains_key(*d))
            .cloned()
            .collect();
        deps.insert(uid.clone(), parent_set);
    }
    let sorted = topological_sort(&deps);

    let listener_factory = DefaultRenderingEventListenerFactory::default();
    let ephemeral_dir = io_args.out_dir.join(DBT_EPHEMERAL_DIR_NAME);

    for uid in sorted {
        let Some(model) = ephemerals.get(&uid) else {
            continue;
        };

        // Build the per-node compile context — this binds `this`, the ref-graph,
        // adapter dispatch, etc. exactly as the normal pipeline does.
        let (compile_ctx, _) = build_compile_node_context(
            model.as_ref(),
            resolver_state,
            base_context,
            DependencyValidationConfig::new_unvalidated(),
        )?;

        let original_file_path = model.__common_attr__.original_file_path.clone();
        let absolute_path =
            io_args.map_to_workspace_path(&original_file_path, model.as_ref().resource_type());
        let raw_sql = stdfs::read_to_string(&absolute_path)?;

        let rendered = render_sql(
            &raw_sql,
            jinja_env,
            &compile_ctx,
            &listener_factory,
            &original_file_path,
        )?;

        let mut macro_spans = MacroSpans::default();
        inject_and_persist_ephemeral_models(
            rendered,
            &mut macro_spans,
            &model.__base_attr__.alias,
            /* is_current_model_ephemeral = */ true,
            &ephemeral_dir,
        )?;
    }

    Ok(())
}

/// Runs an operation (on_run_start/on_run_end) and returns the rendered SQL.
///
/// The `results` parameter accepts a pre-serialized `Value` to allow passing different
/// result types (e.g., `ContextRunResult` for run/test, `FreshnessResultsNode` for source freshness).
#[allow(clippy::too_many_arguments)]
pub async fn run_operation_on_run(
    operation: &Spanned<DbtOperation>,
    io_args: &IoArgs,
    schemas: &Option<Value>,
    database_schemas: &Option<Value>,
    listener_factory: Option<Arc<dyn RenderingEventListenerFactory>>,
    jinja_env: &Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    results: Value,
) -> FsResult<String> {
    if operation
        .__common_attr__
        .raw_code
        .as_ref()
        .is_some_and(|code| code.trim().is_empty())
    {
        return Ok(String::new());
    }

    // Get the base context
    let mut operation_ctx = base_context.clone();
    operation_ctx.insert(
        TARGET_PACKAGE_NAME.to_string(),
        Value::from(operation.__common_attr__.package_name.clone()),
    );
    if let Some(schemas) = schemas {
        operation_ctx.insert("schemas".to_string(), schemas.clone());
    }
    if let Some(database_schemas) = database_schemas {
        operation_ctx.insert("database_schemas".to_string(), database_schemas.clone());
    }

    operation_ctx.insert("results".to_string(), results);

    operation_ctx.insert(CURRENT_PATH.to_string(), Value::from("<RUN_OPERATION>"));
    operation_ctx.insert(
        CURRENT_SPAN.to_string(),
        Value::from_serialize(minijinja::machinery::Span::default()),
    );
    let listener_factory: Arc<dyn RenderingEventListenerFactory> = listener_factory
        .unwrap_or_else(|| Arc::new(DefaultRenderingEventListenerFactory::default()));

    // First, wrap the raw_code with reset_span and render it.
    // `name_span` is a plain field on `CommonAttributes` (unlike the outer
    // `Spanned<DbtOperation>`'s own span, which is lost when the operation is
    // round-tripped through the `--partial-parse` JSON cache), so it stays
    // valid across cold and warm runs alike. Skip reset_span only when it was
    // never populated (e.g. an empty hook body).
    let raw_code = operation
        .__common_attr__
        .raw_code
        .as_ref()
        .expect("raw_code is required in operation");
    let name_span = &operation.__common_attr__.name_span;
    let raw_code_with_reset_span = if name_span.start.line > 0 {
        format!(
            "{{% do reset_span('{}', {}, {}, {}, {}, {}, {}) %}}\n{}",
            operation
                .__common_attr__
                .original_file_path
                .to_string_lossy(),
            name_span.start.line,
            name_span.start.col,
            name_span.start.index,
            name_span.stop.line,
            name_span.stop.col,
            name_span.stop.index,
            raw_code,
        )
    } else {
        raw_code.to_string()
    };

    operation_ctx.insert(
        "write".to_owned(),
        MinijinjaValue::from_object(WriteConfig {
            resource_type: NodeType::Operation.as_str_name().to_string(),
            run_file_path: operation.get_node_path_abs(
                NodePathKind::Executable,
                &io_args.in_dir,
                &io_args.out_dir,
            ),
        }),
    );

    // The hook body is rendered and then executed through `statement`, so it
    // talks to the warehouse: both renders have to happen on the blocking pool,
    // because every database connection must be created by a `dbt-runtime`
    // worker thread.
    let rendered_sql = {
        let jinja_env = Arc::clone(jinja_env);
        let file_path = operation.__common_attr__.original_file_path.to_path_buf();
        dbt_runtime::spawn_blocking(move || {
            let rendered_sql = render_sql(
                &raw_code_with_reset_span,
                &jinja_env,
                &operation_ctx,
                listener_factory.as_ref(),
                &file_path,
            )
            .map_err(|e| e.with_location(file_path.clone()))?;
            // Then wrap the rendered SQL with call statement and render again
            let instruction_plus_wrapper = format!(
                "{{% call statement(None, auto_begin=false, fetch_result=false) %}}\n{}\n{{% endcall %}}",
                &rendered_sql,
            );

            // actually execute the query
            let _ = render_sql(
                &instruction_plus_wrapper,
                &jinja_env,
                &operation_ctx,
                listener_factory.as_ref(),
                &file_path,
            )
            .map_err(|e| e.with_location(file_path.clone()))?;
            Ok::<_, Box<FsError>>(rendered_sql)
        })
        .await
        .map_err(|e| fs_err!(ErrorCode::Generic, "spawn_blocking join error: {}", e))??
    };

    // Save the rendered sql to `target/compiled/{project}/dbt_project.yml/hooks/{name}.sql`,
    // mirroring the run path obtained above and dbt-core's nested operation layout.
    let compiled_path =
        operation.get_node_path_abs(NodePathKind::Compiled, &io_args.in_dir, &io_args.out_dir);
    if let Some(parent) = compiled_path.parent() {
        tokiofs::create_dir_all(parent).await?;
    }
    tokiofs::write(compiled_path, rendered_sql.as_bytes()).await?;
    Ok(rendered_sql)
}

fn context_run_results_to_value(results: &[ContextRunResult]) -> Value {
    Value::from(
        results
            .iter()
            .map(|result| {
                let base = Value::from_serialize(result);
                let timing = result.timing.iter().map(timing_info_to_value);

                // Serialized maps are immutable, so copy the fields to replace timing.
                let mut map = BTreeMap::new();
                if let Ok(fields) = base.try_iter() {
                    for key in fields {
                        if let Some(key_str) = key.as_str() {
                            if key_str != "timing" {
                                if let Some(value) =
                                    base.get_item(&key).ok().filter(|v| !v.is_undefined())
                                {
                                    map.insert(key_str.to_string(), value);
                                }
                            }
                        }
                    }
                }
                map.insert("timing".to_string(), Value::from_iter(timing));
                Value::from_iter(map)
            })
            .collect::<Vec<_>>(),
    )
}

fn timing_info_to_value(timing: &TimingInfo) -> Value {
    let mut map = BTreeMap::new();
    map.insert("name".to_string(), Value::from(timing.name.clone()));
    map.insert(
        "started_at".to_string(),
        timing
            .started_at
            .map(|dt| Value::from_object(PyDateTime::new_naive(dt.naive_utc())))
            .unwrap_or_else(|| Value::from(())),
    );
    map.insert(
        "completed_at".to_string(),
        timing
            .completed_at
            .map(|dt| Value::from_object(PyDateTime::new_naive(dt.naive_utc())))
            .unwrap_or_else(|| Value::from(())),
    );
    Value::from_iter(map)
}

/// Runs an operation (on_run_start/on_run_end) with context and returns the rendered SQL.
pub async fn run_operation_on_run_with_ctx(
    operation: &Spanned<DbtOperation>,
    ctx: &TaskRunnerCtx,
    schemas: &Option<Vec<String>>,
    database_schemas: &Option<Vec<(String, String)>>,
    results: &Option<Vec<ContextRunResult>>,
) -> FsResult<String> {
    // Build operation-specific context with static_analysis=unsafe
    // This ensures refs/sources in operations use deferred relations
    let (mut operation_ctx, _) = ctx.build_compile_node_context(
        &**operation,
        &ctx.inner.base_context,
        DependencyValidationConfig::new_unvalidated(),
    )?;

    // Extend with stateful functions
    extend_base_context_stateful_fn(
        &mut operation_ctx,
        &operation.__common_attr__.package_name,
        ctx.runtime_config().dependencies.keys().cloned().collect(),
    );

    // Add selected_resources list from schedule
    let selected_resources: Vec<String> =
        ctx.inner.schedule.selected_nodes.iter().cloned().collect();
    operation_ctx.insert(
        "selected_resources".to_string(),
        Value::from_iter(selected_resources),
    );

    let schemas = schemas.as_ref().map(|s| Value::from_iter(s.clone()));
    let database_schemas = database_schemas
        .as_ref()
        .map(|s| Value::from_serialize(s.clone()));
    let results_value = results
        .as_deref()
        .map(context_run_results_to_value)
        .unwrap_or_default();
    run_operation_on_run(
        operation,
        &ctx.inner.arg.io,
        &schemas,
        &database_schemas,
        Some(Arc::clone(&ctx.rendering_listener_factory)),
        &ctx.env,
        &operation_ctx,
        results_value,
    )
    .await
}

/// Runs a named macro (the `dbt run-operation <MACRO>` path).
pub async fn run_operation(
    input_macro_name: &str,
    input_macro_args: &BTreeMap<String, dbt_yaml::Value>,
    resolver_state: &ResolverState,
    jinja_env: &Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
) -> FsResult<Value> {
    // Convert macro arguments from yaml to jinja values
    let macro_args = input_macro_args
        .iter()
        .map(|(k, v)| (k.clone(), Value::from_serialize(v)))
        .collect::<BTreeMap<_, _>>();

    // Extract template and macro names
    // 1. if the input name contains a dot '.' then it is interpreted as package.macro
    // 2. otherwise the package name is searched in the manifest (root, dbt, the rest)
    let (template_name, macro_name) = match input_macro_name.split_once('.') {
        Some((package_name, macro_name)) => (
            format!("{package_name}.{macro_name}"),
            macro_name.to_string(),
        ),
        None => {
            if let Some(m) = search_resolved_macro(
                resolver_state,
                &resolver_state.root_project_name,
                input_macro_name,
            ) {
                m
            } else if let Some(m) = search_resolved_macro(resolver_state, "dbt", input_macro_name) {
                m
            } else if let Some(m) = search_resolved_macro(
                resolver_state,
                &format!("dbt_{}", &resolver_state.adapter_type),
                input_macro_name,
            ) {
                m
            } else {
                let macros = resolver_state
                    .macros
                    .macros
                    .values()
                    .filter(|m| m.name == input_macro_name)
                    .collect::<Vec<_>>();
                if macros.is_empty() {
                    return Err(fs_err!(
                        ErrorCode::JinjaError,
                        "Template not found for macro '{}'",
                        input_macro_name
                    ));
                }
                if macros.len() > 1 {
                    return Err(fs_err!(
                        ErrorCode::JinjaError,
                        "More than one template found for macro '{}'",
                        input_macro_name
                    ));
                }
                (
                    format!("{}.{input_macro_name}", macros[0].package_name),
                    input_macro_name.to_string(),
                )
            }
        }
    };
    let mut run_operation_context = base_context.clone();
    run_operation_context.insert(
        TARGET_PACKAGE_NAME.to_string(),
        Value::from(
            template_name
                .split('.')
                .next()
                .expect("target is required in operation"),
        ),
    );
    // Build packages from runtime config dependencies
    let mut packages: BTreeSet<String> = resolver_state
        .runtime_config
        .dependencies
        .keys()
        .cloned()
        .collect();
    packages.insert(resolver_state.root_project_name.clone());

    run_operation_context.insert(
        "context".to_owned(),
        Value::from_object(MacroLookupContext::new(
            resolver_state.root_project_name.clone(),
            None,
            packages,
        )),
    );

    // The macro is free to query the warehouse, so it runs on the blocking pool:
    // every database connection must be created by a `dbt-runtime` worker thread.
    let jinja_env = Arc::clone(jinja_env);
    dbt_runtime::spawn_blocking(move || {
        let template = jinja_env.get_template(&template_name)?;
        let state = template.eval_to_state(run_operation_context, &[])?;
        let func = state
            .lookup(&macro_name, &[])
            .ok_or_else(|| unexpected_fs_err!("macro lookup failed"))?;

        func.call(&state, &[Value::from(Kwargs::from_iter(macro_args))], &[])
            .map_err(|err| Box::new(FsError::from_jinja_err(err, "Failed to run operation")))
    })
    .await
    .map_err(|e| fs_err!(ErrorCode::Generic, "spawn_blocking join error: {}", e))?
}

fn search_resolved_macro(
    resolver_state: &ResolverState,
    package: &str,
    macro_name: &str,
) -> Option<(String, String)> {
    let template_name = format!("{package}.{macro_name}");
    if resolver_state
        .macros
        .macros
        .contains_key(&format!("macro.{template_name}"))
    {
        Some((template_name, macro_name.to_string()))
    } else {
        None
    }
}
