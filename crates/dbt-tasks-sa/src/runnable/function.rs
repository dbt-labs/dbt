use crate::materialize::materialize_function;
use dbt_common::constants::DBT_COMPILED_DIR_NAME;
use dbt_common::path::get_target_write_path;
use dbt_common::{FsResult, io_args::FsCommand, stats::NodeStatus, stdfs};
use dbt_jinja_utils::utils::add_task_context;
use dbt_schemas::schemas::{BatchResults, DbtFunction, InternalDbtNode, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::TaskResult;

pub fn execute_function_remote(
    function: &DbtFunction,
    ctx: &TaskRunnerCtx,
    task_result: &TaskResult,
) -> FsResult<NodeStatus> {
    if ctx.inner.arg.command == FsCommand::Run {
        return Ok(NodeStatus::NoOp);
    }

    let mut base_context = ctx.inner.base_context.clone();
    add_task_context(&mut base_context, function.common(), &ctx.thread_id);

    let sql_instruction = &task_result.sql_instruction;

    // Execute the root function materialization
    materialize_function(
        &sql_instruction.sql,
        function,
        function.node_adapter(),
        ctx.runtime_config(),
        &ctx.inner.materialization_resolver,
        ctx.env.clone(),
        &base_context,
        &ctx.inner.arg.io,
    )?;

    if function.__function_attr__.overloads.is_empty() {
        return Ok(NodeStatus::Succeeded);
    }

    let unique_id = &function.__common_attr__.unique_id;

    // Load already-successful overloads from previous run (pre-populated during retry)
    let already_successful: std::collections::HashSet<String> = ctx
        .inner
        .batch_results_map
        .get(unique_id)
        .map(|br| br.successful.iter().map(|(name, _)| name.clone()).collect())
        .unwrap_or_default();

    // LazyModelWrapper reads model.compiled_code from disk. All overloads share
    // the root's compiled file path, so we pre-write each overload's SQL before
    // its materialization and restore the root's SQL after the loop.
    let compiled_path = get_target_write_path(
        &ctx.inner.arg.io.in_dir,
        &ctx.inner.arg.io.out_dir.join(DBT_COMPILED_DIR_NAME),
        &function.__common_attr__.package_name,
        &function.__common_attr__.path,
        &function.__common_attr__.original_file_path,
    );
    if let Some(parent) = compiled_path.parent() {
        stdfs::create_dir_all(parent)?;
    }

    let mut successful: Vec<(String, String)> = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();

    for overload in &function.__function_attr__.overloads {
        let overload_name = &overload.defined_in;

        if already_successful.contains(overload_name) {
            successful.push((overload_name.clone(), "success".to_string()));
            continue;
        }

        let overload_sql = overload
            .compiled_body
            .as_deref()
            .or(overload.raw_body.as_deref())
            .unwrap_or("");

        let mut overload_function = function.clone();
        overload_function.__function_attr__.arguments = overload.arguments.clone();
        if let Some(returns) = &overload.returns {
            overload_function.__function_attr__.returns = Some(returns.clone());
        }
        overload_function.__common_attr__.raw_code = Some(overload_sql.to_string());

        // LazyModelWrapper reads the compiled SQL from disk, so this pre-write is
        // load-bearing for the materialization below. A failure here means the
        // overload can't be created correctly — record it as a failed overload
        // and continue with the rest (continue-on-failure semantics).
        if let Err(e) = stdfs::write(&compiled_path, overload_sql) {
            failed.push((overload_name.clone(), e.to_string()));
            continue;
        }

        match materialize_function(
            overload_sql,
            &overload_function,
            overload_function.node_adapter(),
            ctx.runtime_config(),
            &ctx.inner.materialization_resolver,
            ctx.env.clone(),
            &base_context,
            &ctx.inner.arg.io,
        ) {
            Ok(_) => {
                successful.push((overload_name.clone(), "success".to_string()));
            }
            Err(e) => {
                failed.push((overload_name.clone(), e.to_string()));
            }
        }
    }

    // Restore the root function's compiled SQL to the compiled file so only the
    // root's compiled SQL persists on disk (matching dbt-core's _write_node
    // behavior). Use sql_instruction.sql (the rendered SQL used to materialize
    // the root above), not raw_code — raw_code is un-rendered and would leave
    // Jinja in the compiled file for roots whose body references other nodes.
    stdfs::write(&compiled_path, &sql_instruction.sql)?;

    let has_failures = !failed.is_empty();

    ctx.inner
        .batch_results_map
        .insert(unique_id.clone(), BatchResults { successful, failed });

    if has_failures {
        Ok(NodeStatus::Errored)
    } else {
        Ok(NodeStatus::Succeeded)
    }
}
