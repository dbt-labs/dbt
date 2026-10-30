use dbt_schemas::schemas::InternalDbtNodeAttributes;
use itertools::{EitherOrBoth, Itertools};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use crate::{
    microbatch::BatchContext,
    runnable::microbatch::{extend_microbatch_node_context, render_batch_sql},
};
use arrow::array::{Date32Array, Date64Array, Float32Array};
use arrow::{
    self,
    array::{
        Array, ArrayRef, BinaryArray, BinaryViewArray, BooleanArray, Decimal128Array,
        Decimal256Array, DictionaryArray, FixedSizeBinaryArray, Float64Array, Int8Array,
        Int16Array, Int32Array, Int64Array, IntervalDayTimeArray, IntervalMonthDayNanoArray,
        IntervalYearMonthArray, LargeBinaryArray, MapArray, RecordBatch, StringArray,
        TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
        TimestampSecondArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array,
    },
    buffer::NullBuffer,
    compute::{CastOptions, cast_with_options},
    datatypes::{ArrowDictionaryKeyType, DataType, Field, IntervalUnit, Schema, TimeUnit},
    util::display::{
        ArrayFormatter, ArrayFormatterFactory, DisplayIndex, FormatOptions, FormatResult,
    },
    util::pretty::{pretty_format_batches, print_batches},
};
use chrono::DateTime;
use dbt_adapter::Adapter;
use dbt_adapter::LATEST_VERSION_POINTER_SUFFIX;
use dbt_adapter::adapter::NodeOverride;
use dbt_adapter::connection::drop_thread_local_connection;
use dbt_adapter::relation::{RelationObject, do_create_relation};
use dbt_adapter::response::AdapterResponse;
use dbt_adapter::stmt_splitter::StmtSplitter;
use dbt_adapter_core::AdapterType;
use dbt_agate::{AgateTable, MappedSequence, Tuple};
use dbt_common::{
    ErrorCode, FsResult, constants::DBT_COMPILED_DIR_NAME, fs_err, io_args::IoArgs,
    path::get_target_write_path, unexpected_fs_err,
};
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv, listener::JinjaTraceListener, phases::run::build_run_node_context,
};
use dbt_schemas::{
    materialization_resolver::MaterializationResolver,
    schemas::{
        DbtFunction, DbtModel, DbtSeed, DbtSnapshot, DbtTest, DbtUnitTest, InternalDbtNode,
        NodePathKind, Nodes, common::DbtMaterialization,
    },
    state::{DbtRuntimeConfig, NodeResolverTracker, ResolverState},
};
use dbt_tasks_core::test_aggregation::GenericTestRelationships;
use dbt_telemetry::ExecutionPhase;
use dbt_yaml::Verbatim;
use minijinja::Value;
use minijinja::constants::TARGET_UNIQUE_ID;
use minijinja::listener::RenderingEventListener;
use serde_json::value::RawValue;
use tracing::debug;

/// Macro to handle NULL values in Arrow arrays
macro_rules! null_or {
    ($arr:expr, $index:expr, $value_expr:expr) => {
        if $arr.is_null($index) {
            "NULL".to_string()
        } else {
            $value_expr
        }
    };
}

#[derive(Debug, Clone)]
pub struct CompareRecordBatchResult {
    pub actual_rows: usize,
    pub expected_rows: usize,
    pub diff_batch: RecordBatch,
    pub has_differences: bool,
}

#[allow(clippy::too_many_arguments)]
fn execute_materialization_macro(
    jinja_env: Arc<JinjaEnv>,
    macro_name: &str,
    context: &mut BTreeMap<String, Value>,
    resource_type: &str,
    unique_id: &str,
    node_alias: &str,
    run_path: PathBuf,
) -> FsResult<Value> {
    let macro_string = format!("{macro_name}()");
    let expr = jinja_env.compile_expression(&macro_string)?;
    with_jinja_trace(&run_path, unique_id, |listeners| {
        expr.eval_with_file_only_stack_frame(context, listeners, run_path.as_path()).map_err(|e| {
            if e.code.is_database_error() {
                // Format like dbt-core: show model name and path first, then the raw
                // database error message indented.
                let indented_body = e
                    .context
                    .lines()
                    .map(|line| format!("  {line}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let message = format!(
                    "Database Error in {resource_type} {node_alias} ({})\n{indented_body}",
                    run_path.display(),
                );
                Box::new(dbt_common::FsError::new(e.code, message))
            } else if e.code == ErrorCode::JinjaWarnUpgradedToError {
                // warn() upgraded to error via warn-error-options: format like dbt-core.
                // emit_warn_log_from_fs_error already reported this to stderr.
                let indented_body = e
                    .context
                    .lines()
                    .map(|line| format!("  {line}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let message = format!(
                    "Compilation Error in {resource_type} {node_alias} ({})\n{indented_body}",
                    run_path.display(),
                );
                Box::new(dbt_common::FsError::new(e.code, message))
            } else {
                // For non-database errors (macro syntax errors, config errors, etc.)
                // keep the verbose format which helps debug the macro call chain.
                let message = format!(
                    "Error executing materialization macro '{macro_name}' for {resource_type} {unique_id}: {}",
                    e.context
                );
                Box::new(e.with_context(message))
            }
        })
    })
}

fn apply_node_overrides(
    adapter: &Adapter,
    custom_warehouse: Option<String>,
    target_database: &str,
    unique_id: &str,
) -> FsResult<Vec<NodeOverride>> {
    let mut node_overrides = Vec::new();
    let adapter_type = adapter.adapter_type();
    if adapter_type == AdapterType::Snowflake
        && let Some(warehouse) = custom_warehouse
        && let Some(node_override) = adapter.use_warehouse(Some(warehouse), unique_id)?
    {
        node_overrides.push(node_override);
    }
    if adapter_type == AdapterType::Redshift
        && let Some(node_override) = adapter.use_database(target_database, unique_id)?
    {
        node_overrides.push(node_override);
    }
    Ok(node_overrides)
}

/// Reset the per-node connection overrides (`USE` database / `use warehouse`) after a
/// materialization. If a reset fails the connection is stuck in the wrong scope, so drop it
/// (do NOT leave it in the thread-local slot) so no other node inherits it, then fail this
/// node loudly with a clear
/// error; the run continues on other nodes.
///
/// The fingerprint check in `borrow_tlocal_connection_impl` does not help here:
/// the connection's *configuration* is unchanged, only its session scope is
/// wrong.
fn reset_node_overrides(
    adapter: &Adapter,
    unique_id: &str,
    targets: &[NodeOverride],
) -> FsResult<()> {
    for target in targets.iter().copied() {
        let result = match target {
            NodeOverride::Database => adapter.reset_database(unique_id).map_err(|e| {
                fs_err!(
                    ErrorCode::ExecutionError,
                    "Failed to reset database context (RESET USE) for node '{unique_id}': {e}"
                )
            }),
            NodeOverride::Warehouse => adapter.restore_warehouse(unique_id).map_err(|e| {
                fs_err!(
                    ErrorCode::ExecutionError,
                    "Failed to restore warehouse for node '{unique_id}': {e}"
                )
            }),
        };

        if let Err(err) = result {
            // A failed reset leaves the connection in the wrong scope: drop it
            // so no other node inherits it.
            drop_thread_local_connection();
            return Err(err);
        }
    }
    Ok(())
}

fn with_jinja_trace<F, T>(compiled_path: &Path, unique_id: &str, f: F) -> FsResult<T>
where
    F: FnOnce(&[Rc<dyn RenderingEventListener>]) -> FsResult<T>,
{
    let trace_listener =
        if dbt_adapter::time_machine::is_replaying() || dbt_adapter::time_machine::is_recording() {
            Some(Rc::new(JinjaTraceListener::new()))
        } else {
            None
        };
    let listeners: Vec<Rc<dyn RenderingEventListener>> = trace_listener
        .iter()
        .map(|l| l.clone() as Rc<dyn RenderingEventListener>)
        .collect();

    f(&listeners).inspect_err(|_| {
        if let Some(ref listener) = trace_listener {
            dump_jinja_trace(listener, compiled_path, unique_id);
        }
    })
}

fn dump_jinja_trace(listener: &JinjaTraceListener, compiled_path: &Path, unique_id: &str) {
    if listener.is_empty() {
        return;
    }
    let trace = listener.format_trace();
    let sanitized = unique_id.replace(['/', '\\', '.'], "_");
    let filename = format!("jinja_trace_{sanitized}.txt");
    if let Some(dir) = compiled_path.parent() {
        let path = dir.join(&filename);
        if std::fs::write(&path, &trace).is_ok() {
            eprintln!("Jinja trace dump written to: {}", path.display());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeHookPhase {
    Pre,
    Post,
}

/// Describes how a node's materialization macro invokes pre/post hooks.
///
/// Reuse paths run hooks outside the normal materialization macro, so they must
/// mirror the adapter macro's hook shape to avoid reuse-only hook side effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeHookStyle {
    SplitTransaction,
    Plain,
}

/// Returns the hook invocation style used by a model's adapter materialization.
pub fn model_hook_style(
    adapter_type: AdapterType,
    materialization: &DbtMaterialization,
) -> NodeHookStyle {
    use AdapterType::*;
    use DbtMaterialization::*;

    match (adapter_type, materialization) {
        (Bigquery | Snowflake | Spark | Databricks, View | Table | Incremental) => {
            NodeHookStyle::Plain
        }
        (Snowflake, DynamicTable | InteractiveTable) => NodeHookStyle::Plain,
        _ => NodeHookStyle::SplitTransaction,
    }
}

fn node_hook_expression(
    context: &BTreeMap<String, Value>,
    style: NodeHookStyle,
    phase: NodeHookPhase,
) -> Option<&'static str> {
    match style {
        NodeHookStyle::Plain => match phase {
            NodeHookPhase::Pre => context
                .contains_key("pre_hooks")
                .then_some("run_hooks(pre_hooks)"),
            NodeHookPhase::Post => context
                .contains_key("post_hooks")
                .then_some("run_hooks(post_hooks)"),
        },
        NodeHookStyle::SplitTransaction => match phase {
            NodeHookPhase::Pre => context.contains_key("pre_hooks").then_some(
                "run_hooks(pre_hooks, inside_transaction=False) ~ run_hooks(pre_hooks, inside_transaction=True)",
            ),
            NodeHookPhase::Post => {
                if context.contains_key("post_hooks") {
                    Some(
                        "run_hooks(post_hooks, inside_transaction=True) ~ adapter.commit() ~ run_hooks(post_hooks, inside_transaction=False)",
                    )
                } else {
                    Some("adapter.commit()")
                }
            }
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub fn execute_node_hooks<S: serde::Serialize>(
    node: &dyn InternalDbtNode,
    deprecated_config: &S,
    adapter_type: AdapterType,
    runtime_config: &DbtRuntimeConfig,
    jinja_env: Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
    sql: Option<&str>,
    style: NodeHookStyle,
    error_path_kind: NodePathKind,
    phase: NodeHookPhase,
) -> FsResult<()> {
    let (mut context, _result_store) = build_run_node_context(
        node,
        deprecated_config,
        adapter_type,
        None,
        base_context,
        io_args,
        ExecutionPhase::Run,
        None,
        runtime_config.dependencies.keys().cloned().collect(),
    );

    if let Some(sql) = sql {
        context.insert("sql".to_string(), Value::from(sql));
        context.insert("compiled_code".to_string(), Value::from(sql));
    }

    let hook_name = match phase {
        NodeHookPhase::Pre => "pre_hooks",
        NodeHookPhase::Post => "post_hooks",
    };
    let Some(hook_expression) = node_hook_expression(&context, style, phase) else {
        return Ok(());
    };
    let expr = jinja_env.compile_expression(hook_expression)?;
    expr.eval(&context, &[]).map(|_| ()).map_err(|e| {
        let resource_type = node.resource_type().as_static_ref();
        let message = format!(
            "Error executing {hook_name} for {resource_type} {}: {}",
            node.common().unique_id,
            e.context
        );
        Box::new(
            e.with_location(node.get_node_path_abs(
                error_path_kind,
                &io_args.in_dir,
                &io_args.out_dir,
            ))
            .with_context(message),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        NodeHookPhase, NodeHookStyle, body_for_materialization, cell_as_bool, model_hook_style,
        node_hook_expression,
    };
    use dbt_adapter::stmt_splitter::DefaultStmtSplitter;
    use dbt_adapter_core::AdapterType;
    use dbt_schemas::schemas::common::DbtMaterialization;
    use minijinja::Value;
    use std::collections::BTreeMap;

    /// Pins no `use warehouse` connection borrow on a non-Snowflake adapter. Without the gate the
    /// mock trips `use_warehouse`'s debug assert, so the test only fails in debug builds.
    #[dbt_runtime::worker_test]
    fn node_overrides_skip_use_warehouse_off_snowflake() {
        let store = dbt_adapter::AdapterStore::new_mock(
            vec![AdapterType::Snowflake, AdapterType::Bigquery],
            AdapterType::Snowflake,
        )
        .expect("valid store");
        let adapter = store.get(AdapterType::Bigquery).expect("bigquery adapter");
        let overrides =
            super::apply_node_overrides(&adapter, Some("NODE_WH".to_string()), "db", "model.pkg.m")
                .expect("no override");
        assert!(overrides.is_empty());
    }

    fn body(sql: &str, language: Option<&str>) -> String {
        body_for_materialization(sql, language, &DefaultStmtSplitter, AdapterType::DuckDB)
            .to_string()
    }

    /// Materializations splice the body into a wrapping query, so a terminal `;`
    /// has to go before the macros see it.
    #[test]
    fn sql_bodies_lose_their_trailing_terminator() {
        assert_eq!(body("select 1 as id;", None), "select 1 as id");
        assert_eq!(body("select 1 as id;", Some("sql")), "select 1 as id");
        // Shapes a plain suffix trim would miss.
        assert_eq!(body("select 1 as id;;", None), "select 1 as id");
        assert_eq!(body("select 1 as id; -- c", None), "select 1 as id");
        assert_eq!(body("select 1 as id;\n-- c\n", None), "select 1 as id");
    }

    /// Python bodies are not SQL; running a SQL splitter over them would be
    /// meaningless, so they must pass through byte for byte.
    #[test]
    fn python_bodies_are_handed_over_verbatim() {
        let py = "def model(dbt, session):\n    return session.sql('select 1;')\n";
        assert_eq!(body(py, Some("python")), py);
    }

    #[test]
    fn bodies_without_a_terminator_are_untouched() {
        assert_eq!(body("\n  select 1 as id\n", None), "\n  select 1 as id\n");
        // A `;` inside a literal is not a terminator.
        assert_eq!(body("select 'a;b' as id\n", None), "select 'a;b' as id\n");
    }

    /// A genuine multi-statement body keeps its terminators, so the warehouse
    /// still rejects it rather than us silently rewriting the user's SQL.
    #[test]
    fn multi_statement_bodies_keep_their_terminators() {
        assert_eq!(
            body("select 1 as id; select 2 as id;", None),
            "select 1 as id; select 2 as id;"
        );
    }

    #[test]
    fn split_transaction_post_hooks_commit_even_when_post_hooks_are_absent() {
        let mut context = BTreeMap::new();
        context.insert("pre_hooks".to_string(), Value::from(Vec::<Value>::new()));

        assert_eq!(
            node_hook_expression(
                &context,
                NodeHookStyle::SplitTransaction,
                NodeHookPhase::Post
            ),
            Some("adapter.commit()")
        );
    }

    #[test]
    fn split_transaction_post_hooks_preserve_post_hook_execution_when_configured() {
        let mut context = BTreeMap::new();
        context.insert("post_hooks".to_string(), Value::from(Vec::<Value>::new()));

        assert_eq!(
            node_hook_expression(
                &context,
                NodeHookStyle::SplitTransaction,
                NodeHookPhase::Post
            ),
            Some(
                "run_hooks(post_hooks, inside_transaction=True) ~ adapter.commit() ~ run_hooks(post_hooks, inside_transaction=False)"
            )
        );
    }

    #[test]
    fn split_transaction_pre_hooks_skip_when_pre_hooks_are_absent() {
        assert_eq!(
            node_hook_expression(
                &BTreeMap::new(),
                NodeHookStyle::SplitTransaction,
                NodeHookPhase::Pre
            ),
            None
        );
    }

    #[test]
    fn plain_hooks_match_non_transactional_adapter_materializations() {
        let mut context = BTreeMap::new();
        context.insert("pre_hooks".to_string(), Value::from(Vec::<Value>::new()));
        context.insert("post_hooks".to_string(), Value::from(Vec::<Value>::new()));

        assert_eq!(
            node_hook_expression(&context, NodeHookStyle::Plain, NodeHookPhase::Pre),
            Some("run_hooks(pre_hooks)")
        );
        assert_eq!(
            node_hook_expression(&context, NodeHookStyle::Plain, NodeHookPhase::Post),
            Some("run_hooks(post_hooks)")
        );
    }

    #[test]
    fn plain_post_hooks_skip_when_post_hooks_are_absent() {
        assert_eq!(
            node_hook_expression(&BTreeMap::new(), NodeHookStyle::Plain, NodeHookPhase::Post),
            None
        );
    }

    #[test]
    fn model_hook_style_matches_adapter_materialization_macros() {
        assert_eq!(
            model_hook_style(AdapterType::Bigquery, &DbtMaterialization::Table),
            NodeHookStyle::Plain
        );
        assert_eq!(
            model_hook_style(AdapterType::Snowflake, &DbtMaterialization::Incremental),
            NodeHookStyle::Plain
        );
        assert_eq!(
            model_hook_style(AdapterType::Spark, &DbtMaterialization::View),
            NodeHookStyle::Plain
        );
        assert_eq!(
            model_hook_style(AdapterType::Databricks, &DbtMaterialization::Table),
            NodeHookStyle::Plain
        );
        assert_eq!(
            model_hook_style(AdapterType::Redshift, &DbtMaterialization::Table),
            NodeHookStyle::SplitTransaction
        );
        assert_eq!(
            model_hook_style(
                AdapterType::Snowflake,
                &DbtMaterialization::InteractiveTable
            ),
            NodeHookStyle::Plain
        );
        assert_eq!(
            model_hook_style(AdapterType::Bigquery, &DbtMaterialization::MaterializedView),
            NodeHookStyle::SplitTransaction
        );
    }

    #[test]
    fn cell_as_bool_reads_adapter_text_booleans() {
        assert!(cell_as_bool(&Value::from("true")));
        assert!(cell_as_bool(&Value::from("True")));
        assert!(cell_as_bool(&Value::from("TRUE")));
        assert!(!cell_as_bool(&Value::from("false")));
        assert!(!cell_as_bool(&Value::from("False")));
        assert!(!cell_as_bool(&Value::from("FALSE")));
    }

    #[test]
    fn cell_as_bool_falls_back_to_value_truthiness() {
        assert!(cell_as_bool(&Value::from(true)));
        assert!(!cell_as_bool(&Value::from(false)));
        assert!(cell_as_bool(&Value::from(1i64)));
        assert!(!cell_as_bool(&Value::from(0i64)));
        assert!(!cell_as_bool(&Value::from(())));
    }
}

#[allow(clippy::too_many_arguments)]
pub fn materialize_clone<S: serde::Serialize>(
    node: &dyn InternalDbtNode,
    deprecated_config: &S,
    adapter: &Adapter,
    runtime_config: &DbtRuntimeConfig,
    defer_nodes: Option<&Nodes>,
    materialization_resolver: &Arc<MaterializationResolver>,
    jinja_env: Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
    custom_warehouse: Option<String>,
) -> FsResult<Value> {
    let adapter_type = adapter.adapter_type();
    let (mut context, _result_store) = build_run_node_context(
        node,
        &deprecated_config,
        adapter_type,
        None,
        base_context,
        io_args,
        ExecutionPhase::Run,
        None,
        runtime_config.dependencies.keys().cloned().collect(),
    );

    let macro_name =
        materialization_resolver.find_materialization_macro_by_name("clone", adapter_type)?;

    let unique_id = node.common().unique_id.clone();
    let defer_option = defer_nodes
        .as_ref()
        .and_then(|nodes| nodes.get_node(&unique_id));

    if let Some(defer) = defer_option {
        let relation: Arc<dyn dbt_schemas::schemas::relations::base::BaseRelation> = Arc::from(
            do_create_relation(
                adapter_type,
                defer.base().database.clone(),
                defer.base().schema.clone(),
                Some(defer.base().alias.clone()),
                None,
                defer.quoting(),
            )
            .unwrap(),
        );

        context.insert(
            "defer_relation".to_string(),
            RelationObject::new(Arc::clone(&relation)).into_value(),
        );

        let sql = format!("SELECT * FROM {}", relation.render_self_as_str());

        context.insert("sql".to_string(), Value::from(&sql));
        context.insert("compiled_code".to_string(), Value::from(&sql));
    }

    let node_alias = node.base().alias.clone();
    // Runtime-phase errors report the run/Executable path per the path-requirements matrix.
    let run_path = node
        .get_node_path(NodePathKind::Executable, &io_args.in_dir, &io_args.out_dir)
        .into_owned();

    let node_overrides =
        apply_node_overrides(adapter, custom_warehouse, &node.base().database, &unique_id)?;

    let res = execute_materialization_macro(
        jinja_env,
        &macro_name,
        &mut context,
        "clone",
        &unique_id,
        &node_alias,
        run_path,
    );

    reset_node_overrides(adapter, &unique_id, &node_overrides)?;
    res
}
#[allow(clippy::too_many_arguments)]
pub fn materialize_seed(
    seed: &DbtSeed,
    adapter: &Adapter,
    runtime_config: &DbtRuntimeConfig,
    materialization_resolver: &Arc<MaterializationResolver>,
    jinja_env: Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    agate_table: AgateTable,
    io_args: &IoArgs,
) -> FsResult<(Value, Option<AdapterResponse>)> {
    let adapter_type = adapter.adapter_type();
    let macro_name =
        materialization_resolver.find_materialization_macro_by_name("seed", adapter_type)?;

    let (mut context, result_store) = build_run_node_context(
        seed,
        &seed.deprecated_config,
        adapter_type,
        Some(agate_table),
        base_context,
        io_args,
        ExecutionPhase::Run,
        None,
        runtime_config.dependencies.keys().cloned().collect(),
    );

    // Runtime-phase errors report the run/Executable path per the path-requirements matrix.
    let run_path = seed
        .get_node_path(NodePathKind::Executable, &io_args.in_dir, &io_args.out_dir)
        .into_owned();

    let unique_id = seed.__common_attr__.unique_id.clone();
    let node_alias = seed.__base_attr__.alias.clone();
    let node_overrides = apply_node_overrides(
        adapter,
        seed.deprecated_config
            .__warehouse_specific_config__
            .snowflake_warehouse
            .clone(),
        &seed.__base_attr__.database,
        &unique_id,
    )?;

    let result = execute_materialization_macro(
        jinja_env,
        &macro_name,
        &mut context,
        "seed",
        &unique_id,
        &node_alias,
        run_path,
    );

    // Surface a failed reset only when the seed itself succeeded: `reset_node_overrides`
    // already drops the poisoned connection, and a seed error is far more actionable than the
    // restore error it would otherwise mask. It also keeps the CSV-column hint that
    // `chain_materialize_seed_error_with_pending_hint` appends attached to a real seed error.
    let reset = reset_node_overrides(adapter, &unique_id, &node_overrides);
    let value = result?;
    reset?;
    Ok((value, result_store.main_adapter_response()))
}

/// Normalize a node body before handing it to a materialization macro.
fn body_for_materialization<'a>(
    sql: &'a str,
    language: Option<&str>,
    splitter: &dyn StmtSplitter,
    adapter_type: AdapterType,
) -> &'a str {
    match language {
        // Python bodies are not SQL and must be handed over verbatim.
        Some("python") => sql,
        _ => splitter.strip_trailing_statement_terminator(sql, adapter_type),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn materialize_model(
    sql: &str,
    node: Arc<dyn InternalDbtNode>,
    adapter: &Adapter,
    runtime_config: &DbtRuntimeConfig,
    materialization_resolver: &Arc<MaterializationResolver>,
    jinja_env: Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
    sql_header: Option<Value>,
) -> FsResult<(Value, Option<AdapterResponse>)> {
    let model = node
        .as_any()
        .downcast_ref::<DbtModel>()
        .expect("model materialization requires a model node");
    let adapter_type = adapter.adapter_type();
    // get materialization
    let (mut context, result_store) = dbt_jinja_utils::phases::run::build_run_node_context_from_arc(
        node.clone(),
        &model.deprecated_config,
        adapter_type,
        None,
        base_context,
        io_args,
        ExecutionPhase::Run,
        sql_header,
        runtime_config.dependencies.keys().cloned().collect(),
    );
    let materialization = model.__base_attr__.materialized.clone();

    let macro_name = materialization_resolver
        .find_materialization_macro_by_name(&materialization.to_string(), adapter_type)?;

    let sql = body_for_materialization(
        sql,
        model.__common_attr__.language.as_deref(),
        adapter.engine().splitter(),
        adapter_type,
    );
    context.insert("sql".to_string(), Value::from(sql));
    context.insert("compiled_code".to_string(), Value::from(sql));

    let unique_id = model.__common_attr__.unique_id.clone();
    let node_alias = model.__base_attr__.alias.clone();
    // Python models execute from their source file. SQL models execute from target/run.
    let run_path = if model.__common_attr__.language.as_deref() == Some("python") {
        io_args
            .in_dir
            .join(&model.__common_attr__.original_file_path)
    } else {
        model
            .get_node_path(NodePathKind::Executable, &io_args.in_dir, &io_args.out_dir)
            .into_owned()
    };

    let custom_warehouse = if let Some(snowflake_attr) = &model.__adapter_attr__.snowflake_attr {
        snowflake_attr.snowflake_warehouse.clone()
    } else {
        None
    };

    let node_overrides = apply_node_overrides(
        adapter,
        custom_warehouse,
        &model.__base_attr__.database,
        &unique_id,
    )?;

    let result = execute_materialization_macro(
        jinja_env,
        &macro_name,
        &mut context,
        "model",
        &unique_id,
        &node_alias,
        run_path,
    );

    reset_node_overrides(adapter, &unique_id, &node_overrides)?;
    result.map(|value| (value, result_store.main_adapter_response()))
}

/// Checks whether the latest version pointer should be created for this model.
///
/// Returns `true` when:
/// - The model is versioned (`version` is set)
/// - It is the latest version (`version == latest_version`)
/// - `latest_version_pointer.enabled` is explicitly `true`, OR `enabled` is `None` and
///   the project flag `latest_version_pointer_enabled_by_default` is `true`
pub fn should_create_latest_version_pointer(
    model: &DbtModel,
    runtime_config: &DbtRuntimeConfig,
) -> bool {
    let version = match &model.__model_attr__.version {
        Some(v) => v,
        None => return false,
    };
    let latest_version = match &model.__model_attr__.latest_version {
        Some(v) => v,
        None => return false,
    };
    if version.to_string() != latest_version.to_string() {
        return false;
    }
    model
        .deprecated_config
        .latest_version_pointer
        .as_ref()
        .and_then(|p| p.enabled)
        .unwrap_or(
            runtime_config
                .inner
                .latest_version_pointer_enabled_by_default,
        )
}

/// Determines the pointer view identifier by calling
/// `generate_latest_version_pointer_alias(custom_alias_name, model)`.
///
/// The default macro (shipped in dbt-adapters) returns `custom_alias_name`
/// if set, otherwise the unsuffixed model name. Users can override this macro
/// in their project to customize pointer naming globally.
fn latest_version_pointer_identifier(
    model: &DbtModel,
    jinja_env: &JinjaEnv,
    context: &mut BTreeMap<String, Value>,
) -> FsResult<String> {
    let custom_alias = model
        .deprecated_config
        .latest_version_pointer
        .as_ref()
        .and_then(|p| p.alias.as_ref())
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty());

    let alias_arg = custom_alias
        .as_deref()
        .map(Value::from)
        .unwrap_or_else(|| Value::from(()));
    context.insert("__lvp_custom_alias__".to_string(), alias_arg);

    let macro_template = "{{ generate_latest_version_pointer_alias(__lvp_custom_alias__, model) }}";
    let render_result = jinja_env.render_str(macro_template, &mut *context, &[]);
    context.remove("__lvp_custom_alias__");

    let rendered = render_result.map_err(|e| {
        fs_err!(
            ErrorCode::Generic,
            "Failed to render `generate_latest_version_pointer_alias` macro for model '{}': {}. \
             Check your project's macro override for syntax errors.",
            model.__common_attr__.unique_id,
            e
        )
    })?;

    let trimmed = rendered.trim();
    if trimmed.is_empty() {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "`generate_latest_version_pointer_alias` returned an empty string for model '{}'. \
             The macro must return a non-empty identifier.",
            model.__common_attr__.unique_id,
        ));
    }
    Ok(trimmed.to_string())
}

/// After the latest version of a versioned model materializes, create a view
/// at the unsuffixed name (or custom alias) pointing to the latest version.
///
/// This mirrors dbt-core's `_materialize_latest_version_view` behavior: it
/// creates a synthetic context for the view materialization macro, pointing
/// the `this` relation at the pointer identifier with SQL = `SELECT * FROM <latest>`.
///
/// The pointer identifier is resolved via `generate_latest_version_pointer_alias`
/// macro if defined, otherwise falls back to `config.alias` or the unsuffixed name.
#[allow(clippy::too_many_arguments)]
pub fn materialize_latest_version_pointer(
    model: &DbtModel,
    adapter_type: AdapterType,
    runtime_config: &DbtRuntimeConfig,
    materialization_resolver: &Arc<MaterializationResolver>,
    jinja_env: Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
) -> FsResult<Value> {
    // Build a context from the original model first so macros can access `model`
    let (mut resolve_context, _result_store) = build_run_node_context(
        model,
        &model.deprecated_config,
        adapter_type,
        None,
        base_context,
        io_args,
        ExecutionPhase::Run,
        None,
        runtime_config.dependencies.keys().cloned().collect(),
    );

    let pointer_identifier =
        latest_version_pointer_identifier(model, &jinja_env, &mut resolve_context)?;
    let model_alias = &model.__base_attr__.alias;

    // Build the source relation (the versioned model's actual relation in the warehouse).
    // This is pure in-memory construction (no warehouse round-trip).
    let source_relation = do_create_relation(
        adapter_type,
        model.__base_attr__.database.clone(),
        model.__base_attr__.schema.clone(),
        Some(model_alias.clone()),
        None,
        model.__base_attr__.quoting,
    )
    .map_err(|e| {
        fs_err!(
            ErrorCode::Generic,
            "Failed to create source relation for latest version pointer: {}",
            e
        )
    })?;

    // Collision check: compare source (latest version) and pointer as the warehouse
    // would resolve them, not as raw strings. `identifier_as_resolved_str` applies the
    // adapter's quoting/casing policy, so an unquoted alias like `DIM_CUSTOMERS` and a
    // pointer name `dim_customers` are correctly detected as the same relation on
    // case-insensitive adapters (e.g. Snowflake).
    let pointer_relation = do_create_relation(
        adapter_type,
        model.__base_attr__.database.clone(),
        model.__base_attr__.schema.clone(),
        Some(pointer_identifier.clone()),
        None,
        model.__base_attr__.quoting,
    )
    .map_err(|e| {
        fs_err!(
            ErrorCode::Generic,
            "Failed to create pointer relation for latest version pointer: {}",
            e
        )
    })?;
    let source_identifier = source_relation.identifier_as_resolved_str().map_err(|e| {
        fs_err!(
            ErrorCode::Generic,
            "Failed to resolve source identifier: {}",
            e
        )
    })?;
    let pointer_resolved_identifier =
        pointer_relation.identifier_as_resolved_str().map_err(|e| {
            fs_err!(
                ErrorCode::Generic,
                "Failed to resolve pointer identifier: {}",
                e
            )
        })?;
    if source_identifier == pointer_resolved_identifier {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "Cannot create latest version pointer: the latest version of '{}' \
             is already aliased to '{}'. Set `latest_version_pointer: {{enabled: false}}` \
             or remove the conflicting alias.",
            model.__common_attr__.name,
            pointer_identifier,
        ));
    }

    let source_relation_str = source_relation.render_self_as_str();

    // Uppercase, matching dbt-core's own generated SQL for this synthetic pointer (confirmed
    // against a production recording) and this file's other synthetic pass-through SQL
    // (`materialize_clone`, above). Time Machine's SQL comparison is a sanitized string-equality
    // check with no keyword-case folding, so drifting from dbt-core's casing here is a literal
    // dbt1308 SqlMismatch on every account with a `latest_version_pointer`, not just a style nit.
    let pointer_sql = format!("SELECT * FROM {source_relation_str}");

    // The id the pointer view's own adapter calls target. dbt-core records the pointer view's
    // create-view/grant statements on the base model's thread, interleaved before the base
    // model's own trailing grants/persist_docs/column calls, but Fusion runs the base model's
    // materialization macro (including those trailing calls) to completion before this function
    // ever runs. Sequential replay would let the trailing calls permanently consume (and discard)
    // the earlier pointer-view records while skipping past them. A distinct target id lets the
    // replay adapter route these calls through unordered, content-based matching instead
    // (`find_key_for_node_id`'s `LATEST_VERSION_POINTER_SUFFIX` handling) rather than the shared
    // sequential cursor. See fs#13705.
    //
    // This is deliberately NOT the pointer model's `unique_id`: the pointer view is not a node,
    // and overwriting the node's identity leaks the synthetic id into everything keyed on it —
    // connection pooling, telemetry, and the recording keys of cross-version record/replay,
    // which then cannot find events any earlier version recorded under the real node id.
    let target_unique_id = format!(
        "{}{LATEST_VERSION_POINTER_SUFFIX}",
        model.__common_attr__.unique_id
    );

    // Build a synthetic model with the pointer's alias and view materialization.
    // Clone the model and override:
    //   - alias → pointer identifier
    //   - materialization → "view"
    //   - hooks cleared (pre/post hooks should not run for the pointer)
    //   - persist_docs cleared (avoid duplicating doc persistence)
    //   - contract cleared: dbt-core never contract-checks the synthetic pointer view (it's a
    //     plain `create or replace view as select *`, not an independent contracted node), but
    //     Fusion's `snowflake__create_or_replace_view` reads `config.get('contract')` off this
    //     same cloned model. Left enforced, it calls `get_assert_columns_equivalent` ->
    //     `get_column_schema_from_query` for the pointer, a call dbt-core's recording never
    //     made, producing a genuine ReplayDataMissing. See fs#13705.
    let mut pointer_model = model.clone();
    pointer_model.__base_attr__.alias = pointer_identifier.clone();
    pointer_model.__base_attr__.materialized = DbtMaterialization::View;
    pointer_model.deprecated_config.materialized = Some(DbtMaterialization::View);
    pointer_model.deprecated_config.pre_hook = Verbatim::from(None);
    pointer_model.deprecated_config.post_hook = Verbatim::from(None);
    pointer_model.deprecated_config.persist_docs = None;
    pointer_model.deprecated_config.contract = None;
    pointer_model.__model_attr__.contract = None;

    debug!(
        "Creating latest version pointer view '{}' -> '{}' for model '{}'",
        pointer_identifier, model_alias, model.__common_attr__.unique_id
    );

    let (mut context, _result_store) = build_run_node_context(
        &pointer_model,
        &pointer_model.deprecated_config,
        adapter_type,
        None,
        base_context,
        io_args,
        ExecutionPhase::Run,
        None,
        runtime_config.dependencies.keys().cloned().collect(),
    );

    let macro_name = materialization_resolver
        .find_materialization_macro_by_name("view", model.node_adapter())?;
    context.insert("sql".to_string(), Value::from(pointer_sql.as_str()));
    context.insert(
        "compiled_code".to_string(),
        Value::from(pointer_sql.as_str()),
    );
    // `TARGET_UNIQUE_ID` is the single channel carrying the pointer view's distinct identity.
    // Adapter methods either read it straight out of Jinja state or pick it up from
    // `QueryCtx::target_unique_id` (which `query_ctx_from_state` fills from this same key), so
    // every lookup path lands on the same id without the node's own `unique_id` being disturbed.
    // See fs#13705.
    context.insert(
        TARGET_UNIQUE_ID.to_string(),
        Value::from(target_unique_id.as_str()),
    );

    let run_path = model
        .get_node_path(NodePathKind::Executable, &io_args.in_dir, &io_args.out_dir)
        .into_owned();

    execute_materialization_macro(
        jinja_env,
        &macro_name,
        &mut context,
        "model",
        &target_unique_id,
        &pointer_identifier,
        run_path,
    )
}

/// Executes a single batch of a microbatch model.
#[allow(clippy::too_many_arguments)]
pub fn materialize_microbatch_model(
    sql_template: &str,
    model: &DbtModel,
    node_resolver: Arc<dyn NodeResolverTracker>,
    runtime_config: &DbtRuntimeConfig,
    materialization_resolver: &Arc<MaterializationResolver>,
    jinja_env: Arc<JinjaEnv>,
    batch_ctx: &BatchContext,
    adapter: &Adapter,
    mut run_node_context: BTreeMap<String, Value>,
    event_time_mapping: Arc<BTreeMap<String, String>>,
    io_args: &IoArgs,
) -> FsResult<Value> {
    let adapter_type = adapter.adapter_type();
    // Microbatch base context is shared
    extend_microbatch_node_context(
        batch_ctx,
        adapter_type,
        model,
        node_resolver,
        runtime_config,
        &mut run_node_context,
        event_time_mapping,
    );

    // Re-render the SQL template to get batch-filtered SQL
    // The template should use {{ ref(...) }} which will now be filtered
    let batch_sql = render_batch_sql(
        sql_template,
        jinja_env.clone(),
        &run_node_context,
        &io_args.out_dir,
    )?;

    let batch_sql = Value::from(body_for_materialization(
        &batch_sql,
        model.__common_attr__.language.as_deref(),
        adapter.engine().splitter(),
        adapter_type,
    ));

    // Databricks incremental materialization reads `model['compiled_code']`.
    let batch_model = run_node_context
        .get("model")
        .and_then(|model| model.downcast_object_ref::<dbt_schemas::jinja_node::LazyNodeContext>())
        .ok_or_else(|| {
            unexpected_fs_err!(
                "No batch-local model context for microbatch model {}",
                batch_ctx.id
            )
        })?;
    batch_model.insert(Value::from("compiled_code"), batch_sql.clone());
    run_node_context.insert("sql".to_string(), batch_sql.clone());
    run_node_context.insert("compiled_code".to_string(), batch_sql);

    // Get the incremental materialization macro
    let macro_name = materialization_resolver.find_materialization_macro_by_name(
        &DbtMaterialization::Incremental.to_string(),
        adapter_type,
    )?;

    let custom_warehouse = if let Some(snowflake_attr) = &model.__adapter_attr__.snowflake_attr {
        snowflake_attr.snowflake_warehouse.clone()
    } else {
        None
    };

    let node_alias = model.__base_attr__.alias.clone();
    // Runtime-phase errors report the run/Executable path per the path-requirements matrix.
    let run_path = model
        .get_node_path(NodePathKind::Executable, &io_args.in_dir, &io_args.out_dir)
        .into_owned();
    let unique_id = model.__common_attr__.unique_id.clone();

    let node_overrides = apply_node_overrides(
        adapter,
        custom_warehouse,
        &model.__base_attr__.database,
        &unique_id,
    )?;

    let result = execute_materialization_macro(
        jinja_env,
        &macro_name,
        &mut run_node_context,
        "model",
        &unique_id,
        &node_alias,
        run_path,
    );

    reset_node_overrides(adapter, &unique_id, &node_overrides)?;
    result
}

#[allow(clippy::too_many_arguments)]
pub fn materialize_snapshot(
    sql: &str,
    snapshot: &DbtSnapshot,
    adapter: &Adapter,
    runtime_config: &DbtRuntimeConfig,
    materialization_resolver: &Arc<MaterializationResolver>,
    jinja_env: Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
) -> FsResult<(Value, Option<AdapterResponse>)> {
    let adapter_type = adapter.adapter_type();
    // get materialization
    let mut snapshot = snapshot.clone();
    snapshot.compiled = Some(true);
    snapshot.compiled_code = Some(sql.to_string());

    let (mut context, result_store) = build_run_node_context(
        &snapshot,
        &snapshot.serialized_config(),
        adapter_type,
        None,
        base_context,
        io_args,
        ExecutionPhase::Run,
        None,
        runtime_config.dependencies.keys().cloned().collect(),
    );

    context.insert("sql".to_string(), Value::from(sql));
    context.insert("compiled_code".to_string(), Value::from(sql));

    let macro_name =
        materialization_resolver.find_materialization_macro_by_name("snapshot", adapter_type)?;

    let unique_id = snapshot.__common_attr__.unique_id.clone();
    let node_alias = snapshot.__base_attr__.alias.clone();
    // Runtime-phase errors report the run/Executable path per the path-requirements matrix.
    let run_path = snapshot
        .get_node_path(NodePathKind::Executable, &io_args.in_dir, &io_args.out_dir)
        .into_owned();

    let custom_warehouse = if let Some(snowflake_attr) = &snapshot.__adapter_attr__.snowflake_attr {
        snowflake_attr.snowflake_warehouse.clone()
    } else {
        None
    };

    let node_overrides = apply_node_overrides(
        adapter,
        custom_warehouse,
        &snapshot.__base_attr__.database,
        &unique_id,
    )?;

    let result = execute_materialization_macro(
        jinja_env,
        &macro_name,
        &mut context,
        "snapshot",
        &unique_id,
        &node_alias,
        run_path,
    );

    reset_node_overrides(adapter, &unique_id, &node_overrides)?;
    result.map(|value| (value, result_store.main_adapter_response()))
}

pub fn materialize_unit_test(
    sql: &str,
    unit_test: &DbtUnitTest,
    resolver_state: Arc<ResolverState>,
    materialization_resolver: Arc<MaterializationResolver>,
    jinja_env: Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
) -> FsResult<bool> {
    // The unit test's own adapter, not the target default: `resolve_unit_tests`
    // gives it the tested model's (`adapter: model_adapter.unwrap_or(default)`),
    // already excluding lake compute, since a unit test never takes the
    // compute-platform path.
    let adapter_type = unit_test.node_adapter();
    let (mut context, _result_store) = build_run_node_context(
        unit_test,
        &unit_test.deprecated_config,
        adapter_type,
        None,
        base_context,
        io_args,
        ExecutionPhase::Run,
        None,
        resolver_state
            .runtime_config
            .dependencies
            .keys()
            .cloned()
            .collect(),
    );
    let materialization = DbtMaterialization::Unit;
    let macro_name = materialization_resolver.find_materialization_macro_by_name(
        &materialization.to_string(),
        unit_test.node_adapter(),
    )?;

    context.insert("sql".to_string(), Value::from(sql));
    context.insert("compiled_code".to_string(), Value::from(sql));

    // Compiled path of the unit test itself (the .sql we wrote when compiling). The
    // helper yields target/compiled/<package>/<dir-of-yaml>/<yaml-filename>; we then
    // swap in <unit_test_name>.sql to match the actual on-disk artifact.
    let compiled_path = get_target_write_path(
        &io_args.in_dir,
        &io_args.out_dir.join(DBT_COMPILED_DIR_NAME),
        &unit_test.__common_attr__.package_name,
        &unit_test.__common_attr__.path,
        &unit_test.__common_attr__.original_file_path,
    )
    .with_file_name(format!("{}.sql", unit_test.__common_attr__.name));

    let _ = with_jinja_trace(
        &compiled_path,
        &unit_test.__common_attr__.unique_id,
        |listeners| {
            jinja_env
                .render_str(
                    &format!("{{{{ {macro_name}() }}}}"),
                    &mut context,
                    listeners,
                )
                .map_err(|e| {
                    Box::new(
                        fs_err!(
                            ErrorCode::JinjaError,
                            "Error materializing unit test {}: {}",
                            unit_test.__common_attr__.unique_id,
                            e
                        )
                        .with_location(compiled_path.clone()),
                    )
                })
        },
    )?;

    let expr = jinja_env.compile_expression("load_result('main').table")?;
    let table = expr
        .eval(&context, &[])
        .unwrap()
        .downcast_object::<AgateTable>()
        .unwrap();
    // print_batches(&[table.to_record_batch().as_ref().clone()])?;
    let CompareRecordBatchResult {
        has_differences,
        diff_batch,
        ..
    } = compare_record_batches(table.original_record_batch().as_ref())?;
    if has_differences {
        print_batches(&[diff_batch])?;
    }

    Ok(!has_differences)
}

pub fn materialize_unit_test_fast_pass(
    sql: &str,
    unit_test: &DbtUnitTest,
    adapter_type: AdapterType,
    runtime_config: &DbtRuntimeConfig,
    jinja_env: Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
) -> FsResult<(bool, usize, String)> {
    let (mut context, _result_store) = build_run_node_context(
        unit_test,
        &unit_test.deprecated_config,
        adapter_type,
        None,
        base_context,
        io_args,
        ExecutionPhase::Run,
        None,
        runtime_config.dependencies.keys().cloned().collect(),
    );

    context.insert("sql".to_string(), Value::from(sql));

    let materialization = r#"
    {% set res, table = adapter.execute(sql, fetch=True) %}
    {% do store_result('main', response=res, agate_table=table) %}
"#;

    // Compiled path of the unit test itself; same construction as materialize_unit_test.
    let compiled_path = get_target_write_path(
        &io_args.in_dir,
        &io_args.out_dir.join(DBT_COMPILED_DIR_NAME),
        &unit_test.__common_attr__.package_name,
        &unit_test.__common_attr__.path,
        &unit_test.__common_attr__.original_file_path,
    )
    .with_file_name(format!("{}.sql", unit_test.__common_attr__.name));

    let _render_str = with_jinja_trace(
        &compiled_path,
        &unit_test.__common_attr__.unique_id,
        |listeners| {
            jinja_env
                .render_str(materialization, &mut context, listeners)
                .map_err(|e| {
                    Box::new(
                        fs_err!(
                            ErrorCode::JinjaError,
                            "Error materializing unit test {}: {}",
                            unit_test.__common_attr__.unique_id,
                            e
                        )
                        .with_location(compiled_path.clone()),
                    )
                })
        },
    )?;

    let expr = jinja_env.compile_expression("load_result('main').table")?;
    let table = expr
        .eval(&context, &[])
        .unwrap()
        .downcast_object::<AgateTable>()
        .unwrap();
    let CompareRecordBatchResult {
        has_differences,
        diff_batch,
        actual_rows,
        expected_rows,
    } = compare_record_batches(table.original_record_batch().as_ref())?;
    let diff_num_rows = diff_batch.num_rows();

    Ok((!has_differences, diff_num_rows, {
        let mut s = pretty_format_batches(&[diff_batch])?.to_string();
        s.push('\n');
        s.push_str(&format!("{diff_num_rows} row(s) differ."));

        if actual_rows != expected_rows {
            s.push_str(&format!(
                "\nExpected {expected_rows} row(s), got {actual_rows} row(s)."
            ))
        }

        s
    }))
}

#[derive(Debug)]
pub struct TestResult {
    pub column_name: Option<String>,
    pub failures: i64,
    pub should_warn: bool,
    pub should_error: bool,
}

impl TestResult {
    pub fn new(
        column_name: Option<String>,
        failures: i64,
        should_warn: bool,
        should_error: bool,
    ) -> Self {
        TestResult {
            column_name,
            failures,
            should_warn,
            should_error,
        }
    }
}

/// Coerce a test-result cell to bool. Some adapters (SQL Server) have no
/// boolean literal and return the text "true"/"false".
fn cell_as_bool(v: &Value) -> bool {
    match v.as_str() {
        Some(s) if s.eq_ignore_ascii_case("true") => true,
        Some(s) if s.eq_ignore_ascii_case("false") => false,
        _ => v.is_true(),
    }
}

fn get_test_results(table: &AgateTable) -> FsResult<Vec<TestResult>> {
    let column_names = table.column_names();
    let column_name_idx = column_names
        .iter()
        .position(|n| n.eq_ignore_ascii_case("column_name"));
    let failures_idx = column_names
        .iter()
        .position(|n| n.eq_ignore_ascii_case("failures"));
    let should_warn_idx = column_names
        .iter()
        .position(|n| n.eq_ignore_ascii_case("should_warn"));
    let should_error_idx = column_names
        .iter()
        .position(|n| n.eq_ignore_ascii_case("should_error"));

    if column_name_idx.is_some() {
        let mut results = Vec::new();
        for row in 0..table.num_rows() {
            results.push(get_column_test_result(
                &table.columns().values(),
                row,
                column_name_idx,
                failures_idx,
                should_warn_idx,
                should_error_idx,
            ));
        }
        Ok(results)
    } else {
        if table.num_rows() != 1 || table.num_columns() != 3 {
            return Err(fs_err!(
                ErrorCode::Unexpected,
                "Test result table should have 1 row and 3 columns, but got {} rows and {} columns",
                table.num_rows(),
                table.num_columns()
            ));
        }

        let columns: Tuple = table.columns().values();
        let failures = columns.get(0).unwrap().get_item_by_index(0).ok();
        let should_warn = columns.get(1).unwrap().get_item_by_index(0).ok();
        let should_error = columns.get(2).unwrap().get_item_by_index(0).ok();

        let failures_val = failures.and_then(|v| v.as_i64()).unwrap_or(-1);
        let should_warn_val = should_warn.map(|v| cell_as_bool(&v)).unwrap_or(false);
        let should_error_val = should_error.map(|v| cell_as_bool(&v)).unwrap_or(false);

        Ok(vec![TestResult {
            column_name: None,
            failures: failures_val,
            should_warn: should_warn_val,
            should_error: should_error_val,
        }])
    }
}

fn get_column_test_result(
    values: &Tuple,
    row: usize,
    column_name_idx: Option<usize>,
    failures_idx: Option<usize>,
    should_warn_idx: Option<usize>,
    should_error_idx: Option<usize>,
) -> TestResult {
    let column_name = get_cell_value(values, row, column_name_idx)
        .and_then(|v| v.as_str().map(|s| s.to_string()));

    let failures = get_cell_value(values, row, failures_idx)
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    let should_warn = get_cell_value(values, row, should_warn_idx)
        .map(|v| cell_as_bool(&v))
        .unwrap_or(false);

    let should_error = get_cell_value(values, row, should_error_idx)
        .map(|v| cell_as_bool(&v))
        .unwrap_or(false);

    TestResult {
        column_name,
        failures,
        should_warn,
        should_error,
    }
}

fn get_cell_value(values: &Tuple, row: usize, column: Option<usize>) -> Option<Value> {
    column
        .and_then(|idx| values.get(idx as isize))
        .and_then(|col| col.get_item_by_index(row).ok())
}

#[allow(clippy::too_many_arguments)]
pub fn materialize_test(
    sql: &str,
    test: &DbtTest,
    relationships: &GenericTestRelationships,
    adapter: &Adapter,
    runtime_config: &DbtRuntimeConfig,
    materialization_resolver: &Arc<MaterializationResolver>,
    jinja_env: Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
) -> FsResult<(
    Vec<TestResult>,
    Option<RecordBatch>,
    Option<AdapterResponse>,
)> {
    let adapter_type = adapter.adapter_type();
    let packages = runtime_config.dependencies.keys().cloned().collect();
    let (mut context, result_store) = build_run_node_context(
        test,
        &test.deprecated_config,
        adapter_type,
        None,
        base_context,
        io_args,
        ExecutionPhase::Run,
        None,
        packages,
    );

    let is_aggregated = relationships.unique_ids.contains_key(&test.common().name);
    let materialization_name = if is_aggregated {
        "aggregated_test"
    } else {
        "test"
    };
    let macro_name = materialization_resolver
        .find_materialization_macro_by_name(materialization_name, adapter_type)?;

    context.insert("sql".to_string(), Value::from(sql));

    let compiled_path = get_target_write_path(
        &io_args.in_dir,
        &io_args.out_dir.join(DBT_COMPILED_DIR_NAME),
        &test.__common_attr__.package_name,
        &test.__common_attr__.path,
        &test.__common_attr__.original_file_path,
    );

    let custom_warehouse = if let Some(snowflake_attr) = &test.__adapter_attr__.snowflake_attr {
        snowflake_attr.snowflake_warehouse.clone()
    } else {
        None
    };

    let unique_id = test.__common_attr__.unique_id.clone();
    // Run path: where target/run/.../<alias>.sql lives; surfaced in runtime (database)
    // errors so the user can open the actual file the database executed.
    let run_display_path = test
        .get_node_path(NodePathKind::Executable, &io_args.in_dir, &io_args.out_dir)
        .display()
        .to_string();

    let node_overrides = apply_node_overrides(
        adapter,
        custom_warehouse,
        &test.__base_attr__.database,
        &unique_id,
    )?;

    let render_result = with_jinja_trace(&compiled_path, &unique_id, |listeners| {
        jinja_env
            .render_str(
                &format!("{{{{ {macro_name}() }}}}"),
                &mut context,
                listeners,
            )
            .map_err(|e| {
                if e.code.is_database_error() {
                    let indented_body = e
                        .context
                        .lines()
                        .map(|line| format!("  {line}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let message = format!(
                        "Error materializing test {unique_id} ({run_display_path})\n{indented_body}",
                    );
                    Box::new(dbt_common::FsError::new(e.code, message))
                } else {
                    Box::new(
                        dbt_common::FsError::new(
                            ErrorCode::JinjaError,
                            format!("Error running test {unique_id}: {e}"),
                        )
                        .with_location(compiled_path.clone()),
                    )
                }
            })
    });

    reset_node_overrides(adapter, &unique_id, &node_overrides)?;

    // Any render/execution error (including database errors) is a hard error,
    // independent of severity. Severity only governs successfully-returned test
    // results, so it must not downgrade an execution failure to a warning.
    let _ = render_result?;

    let expr = jinja_env.compile_expression("load_result('main').table")?;
    let table = expr
        .eval(&context, &[])
        .unwrap()
        .downcast_object::<AgateTable>()
        .unwrap();

    let test_results = get_test_results(&table)?;
    Ok((test_results, None, result_store.main_adapter_response()))
}

/// `AgateTable`'s underlying batches may carry `Utf8View`/`LargeUtf8` columns
/// (query results are normalized to view types at the adapter boundary; see
/// `dbt-adapter`'s `concat_batches::to_view_types`). The comparison logic below
/// assumes plain `Utf8`/`StringArray`, so downgrade back to that before comparing.
///
/// TODO(felipecrv): implement semantic comparison functionality in dbt-agate.
fn normalize_to_utf8(batch: &RecordBatch) -> arrow::error::Result<RecordBatch> {
    let schema = batch.schema();
    if !schema
        .fields()
        .iter()
        .any(|f| matches!(f.data_type(), DataType::Utf8View | DataType::LargeUtf8))
    {
        return Ok(batch.clone());
    }

    let fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|f| match f.data_type() {
            DataType::Utf8View | DataType::LargeUtf8 => {
                f.as_ref().clone().with_data_type(DataType::Utf8)
            }
            _ => f.as_ref().clone(),
        })
        .collect();
    let new_schema = Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()));

    let cast_options = CastOptions {
        safe: false,
        format_options: Default::default(),
    };
    let columns: Vec<ArrayRef> = batch
        .columns()
        .iter()
        .zip(new_schema.fields())
        .map(|(col, field)| match field.data_type() {
            DataType::Utf8 if col.data_type() != &DataType::Utf8 => {
                cast_with_options(col, &DataType::Utf8, &cast_options)
            }
            _ => Ok(col.clone()),
        })
        .collect::<Result<_, _>>()?;

    RecordBatch::try_new(new_schema, columns)
}

pub fn compare_record_batches(
    batch: &RecordBatch,
) -> arrow::error::Result<CompareRecordBatchResult> {
    compare_record_batches_with_json_columns(batch, &[])
}

// Preserve scalar encodings so sorting keys cannot round high-precision numbers.
fn canonicalize_json_object_keys(value: &RawValue) -> serde_json::Result<Box<RawValue>> {
    match value.get().as_bytes().first() {
        Some(b'{') => {
            let object: BTreeMap<String, &RawValue> = serde_json::from_str(value.get())?;
            let object = object
                .into_iter()
                .map(|(key, value)| Ok((key, canonicalize_json_object_keys(value)?)))
                .collect::<serde_json::Result<BTreeMap<_, _>>>()?;
            serde_json::value::to_raw_value(&object)
        }
        Some(b'[') => {
            let array: Vec<&RawValue> = serde_json::from_str(value.get())?;
            let array = array
                .into_iter()
                .map(canonicalize_json_object_keys)
                .collect::<serde_json::Result<Vec<_>>>()?;
            serde_json::value::to_raw_value(&array)
        }
        _ => Ok(value.to_owned()),
    }
}

/// Compare selected JSON columns independently of object key order.
pub fn compare_record_batches_with_json_columns(
    batch: &RecordBatch,
    json_columns: &[usize],
) -> arrow::error::Result<CompareRecordBatchResult> {
    let batch = normalize_to_utf8(batch)?;
    let batch = &batch;
    let schema = batch.schema();

    let label_col_index = schema
        .fields()
        .iter()
        .position(|f| f.name().to_lowercase() == "actual_or_expected")
        .ok_or_else(|| {
            arrow::error::ArrowError::SchemaError(
                "Missing 'actual_or_expected' column in unit test result. \
                 This may indicate an issue with unit test execution in the current adapter mode."
                    .to_string(),
            )
        })?;

    let mut actual_rows = vec![];
    let mut expected_rows = vec![];

    // An empty result has no labels to inspect, regardless of the physical type
    // inferred for the label column.
    if batch.num_rows() == 0 {
        return Ok(CompareRecordBatchResult {
            actual_rows: 0,
            expected_rows: 0,
            diff_batch: batch.clone(),
            has_differences: false,
        });
    }

    let label_column = batch.column(label_col_index);
    let label_column = match label_column.data_type() {
        DataType::Utf8 => label_column.clone(),
        DataType::Binary | DataType::BinaryView | DataType::LargeBinary => {
            let cast_options = CastOptions {
                safe: false,
                format_options: Default::default(),
            };
            cast_with_options(label_column, &DataType::Utf8, &cast_options)?
        }
        data_type => {
            return Err(arrow::error::ArrowError::SchemaError(format!(
                "'actual_or_expected' column must be a string or binary, found {data_type}"
            )));
        }
    };
    let label_array = label_column
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| {
            arrow::error::ArrowError::SchemaError(
                "Failed to normalize 'actual_or_expected' column to a string".to_string(),
            )
        })?;

    for i in 0..batch.num_rows() {
        match label_array.value(i) {
            "actual" => actual_rows.push(i),
            "expected" => expected_rows.push(i),
            _ => {
                return Err(arrow::error::ArrowError::ComputeError(format!(
                    "Invalid value in 'actual_or_expected' column: '{}'. Expected 'actual' or 'expected'",
                    label_array.value(i)
                )));
            }
        }
    }

    let format_options = FormatOptions::new()
        .with_quoted_strings(true)
        .with_null("NULL")
        .with_display_error(false)
        .with_formatter_factory(Some(&UnitTestFormatters));
    let formatters = batch
        .columns()
        .iter()
        .map(|column| unit_test_formatter(column.as_ref(), &format_options))
        .collect::<arrow::error::Result<Vec<_>>>()?;
    // Encoded arrays can contain a valid key whose referenced value is null.
    // Decode validity once per column instead of once per cell comparison.
    let logical_nulls = batch
        .columns()
        .iter()
        .map(|column| column.logical_nulls())
        .collect::<Vec<_>>();

    // Adapters can encode nested columns as strings before comparison. Align
    // every result by complete row keys, including duplicate occurrences.
    let comparison_rows = (0..batch.num_rows())
        .map(|row| {
            batch
                .columns()
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    if index == label_col_index
                        || logical_nulls[index]
                            .as_ref()
                            .is_some_and(|nulls| nulls.is_null(row))
                    {
                        return Ok(None);
                    }
                    let value = if json_columns.contains(&index) {
                        value_as_string(
                            column,
                            row,
                            schema.field(index).data_type(),
                            &formatters[index],
                        )?
                    } else {
                        format_unit_test_value(&formatters[index], row)?
                    };
                    Ok(Some(if json_columns.contains(&index) {
                        serde_json::from_str::<&RawValue>(&value)
                            .and_then(canonicalize_json_object_keys)
                            .map(|value| value.get().to_owned())
                            .unwrap_or(value)
                    } else {
                        value
                    }))
                })
                .collect::<arrow::error::Result<Vec<_>>>()
        })
        .collect::<arrow::error::Result<Vec<_>>>()?;
    actual_rows.sort_by(|a, b| comparison_rows[*a].cmp(&comparison_rows[*b]));
    expected_rows.sort_by(|a, b| comparison_rows[*a].cmp(&comparison_rows[*b]));

    // Prepare new columns - include all data columns in output
    let mut new_columns: Vec<ArrayRef> = vec![];
    let mut new_fields: Vec<Field> = vec![];
    let mut has_differences = actual_rows.len() != expected_rows.len();

    for (col_index, field) in schema.fields().iter().enumerate() {
        if col_index == label_col_index {
            continue; // skip the label column
        }

        let col = batch.column(col_index);
        let data_type = field.data_type();
        let compare_json = json_columns.contains(&col_index);
        if compare_json && *data_type != DataType::Utf8 {
            return Err(arrow::error::ArrowError::SchemaError(format!(
                "JSON comparison column '{}' must be Utf8, found {data_type}",
                field.name()
            )));
        }

        let diffs: Vec<String> = actual_rows
            .iter()
            .zip_longest(expected_rows.iter())
            .map(|pair| match pair {
                EitherOrBoth::Both(a, e) => {
                    // JSON object key order is not meaningful. Other columns
                    // use native equality or the canonical fallback, independent
                    // of the display text in the diff table.
                    let equal = if compare_json {
                        comparison_rows[*a][col_index] == comparison_rows[*e][col_index]
                    } else {
                        values_equal(
                            col,
                            *a,
                            *e,
                            data_type,
                            &formatters[col_index],
                            logical_nulls[col_index].as_ref(),
                        )?
                    };
                    let expected_val = value_as_string(col, *e, data_type, &formatters[col_index])?;
                    if equal {
                        Ok(expected_val)
                    } else {
                        has_differences = true;
                        let actual_val =
                            value_as_string(col, *a, data_type, &formatters[col_index])?;
                        Ok(format!("{expected_val} -> {actual_val}"))
                    }
                }
                EitherOrBoth::Left(a) => {
                    let actual_val = value_as_string(col, *a, data_type, &formatters[col_index])?;
                    has_differences = true;
                    Ok(format!("∅ -> {actual_val}"))
                }
                EitherOrBoth::Right(e) => {
                    let expected_val = value_as_string(col, *e, data_type, &formatters[col_index])?;
                    has_differences = true;
                    Ok(format!("{expected_val} -> ∅"))
                }
            })
            .collect::<arrow::error::Result<_>>()?;

        let new_array = Arc::new(StringArray::from(diffs)) as ArrayRef;
        new_columns.push(new_array);
        new_fields.push(Field::new(field.name(), DataType::Utf8, false));
    }

    // Handle the case where no columns have differences
    let diff_batch = if !has_differences {
        // Create a summary batch showing that all columns matched
        let summary_schema = Arc::new(Schema::new(vec![Field::new(
            "summary",
            DataType::Utf8,
            false,
        )]));
        let summary_data = Arc::new(StringArray::from(vec![
            format!(
                "✅ All {} columns matched perfectly",
                schema.fields().len() - 1
            ), // -1 for label column
        ])) as ArrayRef;
        RecordBatch::try_new(summary_schema, vec![summary_data])?
    } else {
        let new_schema = Arc::new(Schema::new(new_fields));
        RecordBatch::try_new(new_schema, new_columns)?
    };

    Ok(CompareRecordBatchResult {
        actual_rows: actual_rows.len(),
        expected_rows: expected_rows.len(),
        diff_batch,
        has_differences,
    })
}

/// Compare two cells of the same column by their typed Arrow value.
///
/// Scalar types retain their precision through native comparison. The fallback
/// quotes nested strings and bytes, marks nulls, and normalizes SQL intervals.
fn values_equal(
    array: &ArrayRef,
    a: usize,
    e: usize,
    data_type: &DataType,
    formatter: &ArrayFormatter<'_>,
    nulls: Option<&NullBuffer>,
) -> arrow::error::Result<bool> {
    let is_null = |index| nulls.is_some_and(|nulls| nulls.is_null(index));
    if is_null(a) || is_null(e) {
        return Ok(is_null(a) && is_null(e));
    }

    macro_rules! typed_eq {
        ($ty:ty) => {{
            let arr = array.as_any().downcast_ref::<$ty>().ok_or_else(|| {
                arrow::error::ArrowError::SchemaError(format!(
                    "Unit test comparison expected a {} for column type {data_type}",
                    stringify!($ty)
                ))
            })?;
            arr.value(a) == arr.value(e)
        }};
    }

    // `-0.0 == 0.0` holds, and two NaNs are treated as the same value so a NaN
    // expectation can be written down at all.
    macro_rules! float_eq {
        ($ty:ty) => {{
            let arr = array.as_any().downcast_ref::<$ty>().ok_or_else(|| {
                arrow::error::ArrowError::SchemaError(format!(
                    "Unit test comparison expected a {} for column type {data_type}",
                    stringify!($ty)
                ))
            })?;
            let (x, y) = (arr.value(a), arr.value(e));
            x == y || (x.is_nan() && y.is_nan())
        }};
    }

    let equal = match data_type {
        DataType::Int8 => typed_eq!(Int8Array),
        DataType::Int16 => typed_eq!(Int16Array),
        DataType::Int32 => typed_eq!(Int32Array),
        DataType::Int64 => typed_eq!(Int64Array),
        DataType::UInt8 => typed_eq!(UInt8Array),
        DataType::UInt16 => typed_eq!(UInt16Array),
        DataType::UInt32 => typed_eq!(UInt32Array),
        DataType::UInt64 => typed_eq!(UInt64Array),
        DataType::Float32 => float_eq!(Float32Array),
        DataType::Float64 => float_eq!(Float64Array),
        DataType::Boolean => typed_eq!(BooleanArray),
        DataType::Utf8 => typed_eq!(StringArray),
        // The raw i128/i256 is compared at a fixed scale, so sign and every
        // fractional digit are intact.
        DataType::Decimal128(_, _) => typed_eq!(Decimal128Array),
        DataType::Decimal256(_, _) => typed_eq!(Decimal256Array),
        DataType::Date32 => typed_eq!(Date32Array),
        DataType::Date64 => typed_eq!(Date64Array),
        DataType::Timestamp(TimeUnit::Second, _) => typed_eq!(TimestampSecondArray),
        DataType::Timestamp(TimeUnit::Millisecond, _) => typed_eq!(TimestampMillisecondArray),
        DataType::Timestamp(TimeUnit::Microsecond, _) => typed_eq!(TimestampMicrosecondArray),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => typed_eq!(TimestampNanosecondArray),
        _ => format_unit_test_value(formatter, a)? == format_unit_test_value(formatter, e)?,
    };
    Ok(equal)
}

#[derive(Debug)]
struct UnitTestFormatters;

fn unit_test_formatter<'a>(
    array: &'a dyn Array,
    options: &FormatOptions<'a>,
) -> arrow::error::Result<ArrayFormatter<'a>> {
    match UnitTestFormatters.create_array_formatter(array, options, None)? {
        Some(formatter) => Ok(formatter),
        None => ArrayFormatter::try_new(array, options),
    }
}

impl ArrayFormatterFactory for UnitTestFormatters {
    fn create_array_formatter<'a>(
        &self,
        array: &'a dyn Array,
        options: &FormatOptions<'a>,
        _field: Option<&'a Field>,
    ) -> arrow::error::Result<Option<ArrayFormatter<'a>>> {
        match array.data_type() {
            DataType::Map(_, _) => {
                let array = array.as_any().downcast_ref::<MapArray>().unwrap();
                let keys = unit_test_formatter(array.keys().as_ref(), options)?;
                let values = unit_test_formatter(array.values().as_ref(), options)?;
                Ok(Some(ArrayFormatter::new(
                    Box::new(UnitTestMapDisplay {
                        array,
                        keys,
                        values,
                        null: options.null(),
                    }),
                    options.safe(),
                )))
            }
            DataType::Interval(_)
            | DataType::Float32
            | DataType::Float64
            | DataType::Binary
            | DataType::LargeBinary
            | DataType::BinaryView
            | DataType::FixedSizeBinary(_) => Ok(Some(ArrayFormatter::new(
                Box::new(UnitTestScalarDisplay {
                    array,
                    null: options.null(),
                }),
                options.safe(),
            ))),
            // Arrow's dictionary formatter bypasses the factory for its values.
            // Keep the binary and interval rules when those values are encoded.
            DataType::Dictionary(_, _) => arrow::array::downcast_dictionary_array!(
                array => {
                    let values = unit_test_formatter(array.values().as_ref(), options)?;
                    Ok(Some(ArrayFormatter::new(
                        Box::new(UnitTestDictionaryDisplay { array, values, null: options.null() }),
                        options.safe(),
                    )))
                }
                _ => unreachable!("dictionary formatter requires a dictionary array")
            ),
            _ => Ok(None),
        }
    }
}

struct UnitTestMapDisplay<'a> {
    array: &'a MapArray,
    keys: ArrayFormatter<'a>,
    values: ArrayFormatter<'a>,
    null: &'a str,
}

impl DisplayIndex for UnitTestMapDisplay<'_> {
    fn write(&self, index: usize, out: &mut dyn std::fmt::Write) -> FormatResult {
        if self.array.is_null(index) {
            write!(out, "{}", self.null)?;
            return Ok(());
        }
        let offsets = self.array.value_offsets();
        let mut entries = (offsets[index] as usize..offsets[index + 1] as usize)
            .map(|index| {
                Ok((
                    format_unit_test_value(&self.keys, index)?,
                    format_unit_test_value(&self.values, index)?,
                ))
            })
            .collect::<arrow::error::Result<Vec<_>>>()?;
        entries.sort_unstable();
        write!(out, "{{")?;
        for (index, (key, value)) in entries.iter().enumerate() {
            if index > 0 {
                write!(out, ", ")?;
            }
            write!(out, "{key}: {value}")?;
        }
        write!(out, "}}")?;
        Ok(())
    }
}

struct UnitTestDictionaryDisplay<'a, K: ArrowDictionaryKeyType> {
    array: &'a DictionaryArray<K>,
    values: ArrayFormatter<'a>,
    null: &'a str,
}

impl<K: ArrowDictionaryKeyType> DisplayIndex for UnitTestDictionaryDisplay<'_, K> {
    fn write(&self, index: usize, out: &mut dyn std::fmt::Write) -> FormatResult {
        match self.array.key(index) {
            Some(key) => write!(out, "{}", self.values.value(key))?,
            None => write!(out, "{}", self.null)?,
        }
        Ok(())
    }
}

struct UnitTestScalarDisplay<'a> {
    array: &'a dyn Array,
    null: &'a str,
}

impl DisplayIndex for UnitTestScalarDisplay<'_> {
    fn write(&self, index: usize, out: &mut dyn std::fmt::Write) -> FormatResult {
        if self.array.is_null(index) {
            write!(out, "{}", self.null)?;
            return Ok(());
        }

        // SQL equality ignores the sign of zero. Nested values and row keys
        // must follow the same rule as native scalar comparison.
        match self.array.data_type() {
            DataType::Float32 => {
                let value = self
                    .array
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .unwrap()
                    .value(index);
                write!(out, "{:?}", if value == 0.0 { 0.0 } else { value })?;
                return Ok(());
            }
            DataType::Float64 => {
                let value = self
                    .array
                    .as_any()
                    .downcast_ref::<Float64Array>()
                    .unwrap()
                    .value(index);
                write!(out, "{:?}", if value == 0.0 { 0.0 } else { value })?;
                return Ok(());
            }
            _ => {}
        }

        let bytes = match self.array.data_type() {
            DataType::Binary => Some(
                self.array
                    .as_any()
                    .downcast_ref::<BinaryArray>()
                    .unwrap()
                    .value(index),
            ),
            DataType::LargeBinary => Some(
                self.array
                    .as_any()
                    .downcast_ref::<LargeBinaryArray>()
                    .unwrap()
                    .value(index),
            ),
            DataType::BinaryView => Some(
                self.array
                    .as_any()
                    .downcast_ref::<BinaryViewArray>()
                    .unwrap()
                    .value(index),
            ),
            DataType::FixedSizeBinary(_) => Some(
                self.array
                    .as_any()
                    .downcast_ref::<FixedSizeBinaryArray>()
                    .unwrap()
                    .value(index),
            ),
            _ => None,
        };
        if let Some(bytes) = bytes {
            write!(out, "b\"")?;
            for byte in bytes {
                write!(out, "{byte:02x}")?;
            }
            write!(out, "\"")?;
            return Ok(());
        }

        const NANOS_PER_DAY: i128 = 86_400_000_000_000;
        // SQL interval comparison treats a month as 30 days. Use i128 so even
        // the largest Arrow month/day components retain nanosecond precision.
        // https://duckdb.org/docs/stable/sql/data_types/interval#equality-and-comparison
        let total = match self.array.data_type() {
            DataType::Interval(IntervalUnit::YearMonth) => {
                let array = self
                    .array
                    .as_any()
                    .downcast_ref::<IntervalYearMonthArray>()
                    .unwrap();
                i128::from(array.value(index)) * 30 * NANOS_PER_DAY
            }
            DataType::Interval(IntervalUnit::DayTime) => {
                let array = self
                    .array
                    .as_any()
                    .downcast_ref::<IntervalDayTimeArray>()
                    .unwrap();
                let value = array.value(index);
                i128::from(value.days) * NANOS_PER_DAY + i128::from(value.milliseconds) * 1_000_000
            }
            DataType::Interval(IntervalUnit::MonthDayNano) => {
                let array = self
                    .array
                    .as_any()
                    .downcast_ref::<IntervalMonthDayNanoArray>()
                    .unwrap();
                let value = array.value(index);
                (i128::from(value.months) * 30 + i128::from(value.days)) * NANOS_PER_DAY
                    + i128::from(value.nanoseconds)
            }
            _ => unreachable!("interval formatter requires an interval array"),
        };
        let (days, nanos) = (total / NANOS_PER_DAY, total % NANOS_PER_DAY);
        match (days, nanos) {
            (_, 0) => write!(out, "{days} days")?,
            (0, _) => write!(out, "{nanos} ns")?,
            _ => write!(out, "{days} days {nanos} ns")?,
        }
        Ok(())
    }
}

fn format_unit_test_value(
    formatter: &ArrayFormatter<'_>,
    index: usize,
) -> arrow::error::Result<String> {
    formatter.value(index).try_to_string().map_err(|error| {
        arrow::error::ArrowError::ComputeError(format!("Cannot compare unit test values: {error}"))
    })
}

/// Split a timestamp at `unit` resolution into whole seconds plus a nanosecond
/// remainder. Euclidean division keeps the remainder non-negative, so a
/// pre-epoch sub-second instant does not round towards the epoch.
fn split_seconds_nanos(value: i64, per_second: i64) -> (i64, u32) {
    let seconds = value.div_euclid(per_second);
    let sub = value.rem_euclid(per_second);
    let nanos = sub * (1_000_000_000 / per_second);
    (seconds, nanos as u32)
}

fn timestamp_as_string(
    array: &ArrayRef,
    index: usize,
    unit: &TimeUnit,
    data_type: &DataType,
) -> String {
    macro_rules! at {
        ($ty:ty, $per_second:expr) => {{
            match array.as_any().downcast_ref::<$ty>() {
                Some(arr) => split_seconds_nanos(arr.value(index), $per_second),
                None => return format!("[unrenderable {data_type}]"),
            }
        }};
    }

    let (seconds, nanos) = match unit {
        TimeUnit::Second => at!(TimestampSecondArray, 1),
        TimeUnit::Millisecond => at!(TimestampMillisecondArray, 1_000),
        TimeUnit::Microsecond => at!(TimestampMicrosecondArray, 1_000_000),
        TimeUnit::Nanosecond => at!(TimestampNanosecondArray, 1_000_000_000),
    };

    match DateTime::from_timestamp(seconds, nanos) {
        Some(dt) => dt.to_string(),
        None => format!("[unrenderable {data_type}]"),
    }
}

/// Render a cell for the diff table the user reads.
///
/// Equality uses `values_equal`. The fallback formatter is also used for row
/// alignment when a result includes JSON columns.
fn value_as_string(
    array: &ArrayRef,
    index: usize,
    data_type: &DataType,
    formatter: &ArrayFormatter<'_>,
) -> arrow::error::Result<String> {
    let value = match data_type {
        DataType::Int32 => {
            let arr = array.as_any().downcast_ref::<Int32Array>().unwrap();
            null_or!(arr, index, arr.value(index).to_string())
        }
        DataType::Int64 => {
            let arr = array.as_any().downcast_ref::<Int64Array>().unwrap();
            null_or!(arr, index, arr.value(index).to_string())
        }
        // `{:?}` is the shortest representation that round-trips, and keeps a
        // trailing `.0` on whole numbers. `{:.1}` used to drop every digit past
        // the first decimal place.
        DataType::Float32 => {
            let arr = array.as_any().downcast_ref::<Float32Array>().unwrap();
            null_or!(arr, index, format!("{:?}", arr.value(index)))
        }
        DataType::Float64 => {
            let arr = array.as_any().downcast_ref::<Float64Array>().unwrap();
            null_or!(arr, index, format!("{:?}", arr.value(index)))
        }
        DataType::Utf8 => {
            let arr = array.as_any().downcast_ref::<StringArray>().unwrap();
            null_or!(arr, index, arr.value(index).to_string())
        }
        DataType::Boolean => {
            let arr = array.as_any().downcast_ref::<BooleanArray>().unwrap();
            null_or!(arr, index, arr.value(index).to_string())
        }
        DataType::Timestamp(unit, _) => {
            if array.is_null(index) {
                "NULL".to_string()
            } else {
                timestamp_as_string(array, index, unit, data_type)
            }
        }
        DataType::Date32 => {
            let arr = array.as_any().downcast_ref::<Date32Array>().unwrap();
            null_or!(arr, index, {
                let days = arr.value(index) as i64;
                let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
                (epoch + chrono::Duration::days(days)).to_string()
            })
        }
        // Use the same nested boundaries and interval rules as comparison.
        _ => {
            if array.is_null(index) {
                "NULL".to_string()
            } else {
                format_unit_test_value(formatter, index)?
            }
        }
    };
    Ok(value)
}

#[cfg(test)]
mod compare_record_batches_tests {
    use super::{
        CompareRecordBatchResult, compare_record_batches, compare_record_batches_with_json_columns,
    };
    use arrow::array::{
        Array, ArrayRef, BinaryArray, BinaryViewArray, Decimal128Array, DictionaryArray,
        Float64Array, Int8Array, Int32Array, Int32Builder, Int64Array, IntervalDayTimeArray,
        IntervalMonthDayNanoArray, LargeBinaryArray, ListArray, ListBuilder, MapBuilder,
        StringArray, StringBuilder, StringViewArray, StructArray, TimestampMicrosecondArray,
        TimestampMicrosecondBuilder, TimestampMillisecondArray, TimestampNanosecondArray,
    };
    use arrow::datatypes::{
        DataType, Field, Int8Type, Int32Type, IntervalDayTimeType, IntervalMonthDayNanoType,
        Schema, TimeUnit,
    };
    use arrow::util::pretty::pretty_format_batches;
    use dbt_adapter::record_batch::RecordBatchExt;
    use dbt_agate::AgateTable;
    use std::sync::Arc;

    fn compare_column(column: ArrayRef) -> arrow::error::Result<CompareRecordBatchResult> {
        compare_record_batches(&typed_batch(column.data_type().clone(), column))
    }

    fn databricks_table(batch: arrow::array::RecordBatch) -> AgateTable {
        let display = batch.clone().jsonify_nested_columns();
        AgateTable::from_record_batch_with_original(Arc::new(display), Arc::new(batch))
    }

    #[test]
    fn databricks_nonfinite_map_values_remain_distinct() {
        for (expected, actual, differs) in [
            (Some(f64::NAN), Some(f64::INFINITY), true),
            (Some(f64::INFINITY), Some(f64::NEG_INFINITY), true),
            (Some(f64::NAN), Some(-f64::NAN), false),
            (Some(-0.0), Some(0.0), false),
            (None, Some(f64::NAN), true),
            (None, None, false),
        ] {
            let mut doubles = MapBuilder::new(
                None,
                StringBuilder::new(),
                arrow::array::Float64Builder::new(),
            );
            let mut floats = MapBuilder::new(
                None,
                StringBuilder::new(),
                arrow::array::Float32Builder::new(),
            );
            for value in [expected, actual] {
                doubles.keys().append_value("k");
                doubles.values().append_option(value);
                doubles.append(true).unwrap();
                floats.keys().append_value("k");
                floats
                    .values()
                    .append_option(value.map(|value| value as f32));
                floats.append(true).unwrap();
            }
            for column in [
                Arc::new(doubles.finish()) as ArrayRef,
                Arc::new(floats.finish()),
            ] {
                let table = databricks_table(typed_batch(column.data_type().clone(), column));
                let result =
                    compare_record_batches(table.original_record_batch().as_ref()).unwrap();
                assert_eq!(
                    result.has_differences, differs,
                    "{expected:?} vs {actual:?}"
                );
            }
        }
    }

    #[test]
    fn databricks_maps_keep_null_empty_and_null_entries_distinct() {
        for (expected, actual, differs) in [
            (0, 1, true),
            (1, 1, false),
            (1, 2, true),
            (0, 2, true),
            (2, 2, false),
        ] {
            let mut maps = MapBuilder::new(None, StringBuilder::new(), Int32Builder::new());
            for kind in [expected, actual] {
                if kind == 2 {
                    maps.keys().append_value("k");
                    maps.values().append_null();
                }
                maps.append(kind != 0).unwrap();
            }
            let maps = Arc::new(maps.finish()) as ArrayRef;
            let table = databricks_table(typed_batch(maps.data_type().clone(), maps));
            let result = compare_record_batches(table.original_record_batch().as_ref()).unwrap();
            assert_eq!(result.has_differences, differs);
        }
    }

    #[test]
    fn databricks_duplicate_struct_fields_preserve_every_value() {
        for (first_field, differs) in [([1, 3], true), ([1, 1], false)] {
            let mut maps = MapBuilder::new(None, StringBuilder::new(), Int32Builder::new());
            for entries in [[("x", 1), ("y", 2)], [("y", 2), ("x", 1)]] {
                for (key, value) in entries {
                    maps.keys().append_value(key);
                    maps.values().append_value(value);
                }
                maps.append(true).unwrap();
            }
            let maps = Arc::new(maps.finish()) as ArrayRef;
            let structs = Arc::new(StructArray::from(vec![
                (
                    Arc::new(Field::new("a", DataType::Int32, false)),
                    Arc::new(Int32Array::from(first_field.to_vec())) as ArrayRef,
                ),
                (
                    Arc::new(Field::new("a", DataType::Int32, false)),
                    Arc::new(Int32Array::from(vec![2, 2])) as ArrayRef,
                ),
                (
                    Arc::new(Field::new("m", maps.data_type().clone(), false)),
                    maps,
                ),
            ])) as ArrayRef;
            let lists = Arc::new(ListArray::new(
                Arc::new(Field::new("item", structs.data_type().clone(), false)),
                arrow::buffer::OffsetBuffer::new(vec![0, 1, 2].into()),
                structs.clone(),
                None,
            )) as ArrayRef;
            for column in [structs, lists] {
                let table = databricks_table(typed_batch(column.data_type().clone(), column));
                let result =
                    compare_record_batches(table.original_record_batch().as_ref()).unwrap();
                assert_eq!(result.has_differences, differs);
            }
        }
    }

    #[test]
    fn databricks_map_rows_align_after_json_encoding() {
        let expected = [Some(1), Some(2), Some(1), None];
        for (actual, differs) in [
            (vec![None, Some(1), Some(1), Some(2)], false),
            (vec![None, Some(1), Some(2), Some(2)], true),
            (vec![None, Some(1), Some(1), Some(3)], true),
            (vec![None, Some(1), Some(2)], true),
            (vec![Some(1), Some(1), Some(2), Some(2)], true),
        ] {
            let mut maps = MapBuilder::new(None, StringBuilder::new(), Int32Builder::new());
            for (index, value) in expected.iter().chain(&actual).enumerate() {
                if let Some(value) = value {
                    let entries = if index < expected.len() {
                        [("a", 0), ("k", *value)]
                    } else {
                        [("k", *value), ("a", 0)]
                    };
                    for (key, value) in entries {
                        maps.keys().append_value(key);
                        maps.values().append_value(value);
                    }
                }
                maps.append(value.is_some()).unwrap();
            }
            let maps = Arc::new(maps.finish()) as ArrayRef;
            let structs = Arc::new(StructArray::from(vec![(
                Arc::new(Field::new("m", maps.data_type().clone(), true)),
                maps.clone(),
            )])) as ArrayRef;
            let lists = Arc::new(ListArray::new(
                Arc::new(Field::new("item", maps.data_type().clone(), true)),
                arrow::buffer::OffsetBuffer::new(
                    (0..=maps.len() as i32).collect::<Vec<_>>().into(),
                ),
                maps.clone(),
                None,
            )) as ArrayRef;
            for column in [maps, structs, lists] {
                let batch = arrow::array::RecordBatch::try_from_iter(vec![
                    (
                        "actual_or_expected",
                        Arc::new(StringArray::from(
                            std::iter::repeat_n("expected", expected.len())
                                .chain(std::iter::repeat_n("actual", actual.len()))
                                .collect::<Vec<_>>(),
                        )) as ArrayRef,
                    ),
                    ("value", column),
                    (
                        "scalar_sort_key",
                        Arc::new(Int32Array::from(vec![1; expected.len() + actual.len()])),
                    ),
                ])
                .unwrap();
                let table = databricks_table(batch);
                assert_eq!(
                    table.to_record_batch().schema().field(1).data_type(),
                    &DataType::Utf8
                );
                let result =
                    compare_record_batches(table.original_record_batch().as_ref()).unwrap();
                assert_eq!(result.has_differences, differs, "actual: {actual:?}");
                assert_eq!(
                    (result.expected_rows, result.actual_rows),
                    (expected.len(), actual.len())
                );
            }
        }
    }

    #[test]
    fn nested_rows_align_without_warehouse_sort_keys() {
        for (rows, differs) in [
            ([1, 2, 2, 1], false),
            ([1, 2, 1, 3], true),
            ([1, 1, 1, 2], true),
        ] {
            let lists = ListArray::from_iter_primitive::<Int32Type, _, _>(
                rows.map(|value| Some(vec![Some(value)])),
            );
            let structs = StructArray::from(vec![(
                Arc::new(Field::new("n", DataType::Int32, false)),
                Arc::new(Int32Array::from(rows.to_vec())) as ArrayRef,
            )]);
            for column in [Arc::new(lists) as ArrayRef, Arc::new(structs)] {
                let batch = arrow::array::RecordBatch::try_from_iter(vec![
                    (
                        "actual_or_expected",
                        Arc::new(StringArray::from(vec![
                            "expected", "expected", "actual", "actual",
                        ])) as ArrayRef,
                    ),
                    ("value", column),
                    ("scalar_sort_key", Arc::new(Int32Array::from(vec![1; 4]))),
                ])
                .unwrap();
                assert_eq!(
                    compare_record_batches(&batch).unwrap().has_differences,
                    differs
                );
            }
        }
    }

    #[test]
    fn nested_and_dictionary_floats_use_scalar_equality() {
        for (expected, actual, differs) in [
            (-0.0, 0.0, false),
            (f64::NAN, -f64::NAN, false),
            (0.01, 0.0, true),
            (f64::INFINITY, f64::NEG_INFINITY, true),
        ] {
            let lists = ListArray::from_iter_primitive::<arrow::datatypes::Float64Type, _, _>([
                Some(vec![Some(expected)]),
                Some(vec![Some(actual)]),
            ]);
            let structs = StructArray::from(vec![(
                Arc::new(Field::new("n", DataType::Float32, false)),
                Arc::new(arrow::array::Float32Array::from(vec![
                    expected as f32,
                    actual as f32,
                ])) as ArrayRef,
            )]);
            let dictionary = DictionaryArray::<Int8Type>::try_new(
                Int8Array::from(vec![0, 1]),
                Arc::new(Float64Array::from(vec![expected, actual])),
            )
            .unwrap();
            for column in [
                Arc::new(lists) as ArrayRef,
                Arc::new(structs),
                Arc::new(dictionary),
            ] {
                assert_eq!(compare_column(column).unwrap().has_differences, differs);
            }
        }
    }

    #[test]
    fn json_row_alignment_uses_scalar_zero_equality() {
        let batch = arrow::array::RecordBatch::try_from_iter(vec![
            (
                "actual_or_expected",
                Arc::new(StringArray::from(vec![
                    "expected", "expected", "actual", "actual",
                ])) as ArrayRef,
            ),
            (
                "n",
                Arc::new(Float64Array::from(vec![-0.0, 0.0, 0.0, -0.0])),
            ),
            (
                "json",
                Arc::new(StringArray::from(vec![
                    r#"{"v":1}"#,
                    r#"{"v":2}"#,
                    r#"{"v":1}"#,
                    r#"{"v":2}"#,
                ])),
            ),
        ])
        .unwrap();
        assert!(
            !compare_record_batches_with_json_columns(&batch, &[2])
                .unwrap()
                .has_differences
        );
    }

    #[test]
    fn dictionary_null_keys_and_null_values_compare_equally() {
        for (keys, values, differs) in [
            (vec![None, Some(0)], vec![None], false),
            (vec![Some(0), Some(1)], vec![None, Some("NULL")], true),
        ] {
            let column: ArrayRef = Arc::new(
                DictionaryArray::<Int8Type>::try_new(
                    Int8Array::from(keys),
                    Arc::new(StringArray::from(values)),
                )
                .unwrap(),
            );
            assert_eq!(
                compare_column(column.clone()).unwrap().has_differences,
                differs
            );

            let batch = arrow::array::RecordBatch::try_from_iter(vec![
                (
                    "actual_or_expected",
                    Arc::new(StringArray::from(vec!["expected", "actual"])) as ArrayRef,
                ),
                ("value", column),
                ("json", Arc::new(StringArray::from(vec!["{}", "{}"]))),
            ])
            .unwrap();
            assert_eq!(
                compare_record_batches_with_json_columns(&batch, &[2])
                    .unwrap()
                    .has_differences,
                differs
            );
        }
    }

    #[test]
    fn nested_string_comparison_distinguishes_boundaries_nulls_and_escaping() {
        for (expected, actual) in [
            (vec![], vec![Some("")]),
            (vec![], vec![None]),
            (vec![None], vec![Some("")]),
            (vec![None], vec![Some("NULL")]),
            (vec![Some("a, b")], vec![Some("a"), Some("b")]),
            (vec![Some("a\", \"b")], vec![Some("a"), Some("b")]),
            (vec![Some("\\n")], vec![Some("\n")]),
        ] {
            let mut builder = ListBuilder::new(StringBuilder::new());
            for row in [expected, actual] {
                for value in row {
                    builder.values().append_option(value);
                }
                builder.append(true);
            }
            let values = builder.finish();
            let result = compare_column(Arc::new(values)).unwrap();
            assert!(result.has_differences);
            let diff = result
                .diff_batch
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            let (expected, actual) = diff.value(0).split_once(" -> ").unwrap();
            assert_ne!(expected, actual);
        }
    }

    #[test]
    fn struct_string_comparison_preserves_field_boundaries() {
        let fields = vec![
            (
                Arc::new(Field::new("a", DataType::Utf8, true)),
                Arc::new(StringArray::from(vec!["x, b: y", "x"])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("b", DataType::Utf8, true)),
                Arc::new(StringArray::from(vec!["z", "y, b: z"])) as ArrayRef,
            ),
        ];
        assert!(
            compare_column(Arc::new(StructArray::from(fields)))
                .unwrap()
                .has_differences
        );
    }

    #[test]
    fn nested_binary_comparison_preserves_empty_element_boundaries() {
        let binary: ArrayRef = Arc::new(BinaryArray::from(vec![b"".as_slice()]));
        let dictionary: ArrayRef = Arc::new(
            DictionaryArray::<Int8Type>::try_new(Int8Array::from(vec![0]), binary.clone()).unwrap(),
        );
        let variants: Vec<ArrayRef> = vec![
            binary,
            Arc::new(BinaryViewArray::from_iter_values([b"".as_slice()])),
            Arc::new(LargeBinaryArray::from(vec![b"".as_slice()])),
            dictionary,
        ];
        for values in variants {
            let field = Arc::new(Field::new("item", values.data_type().clone(), true));
            let offsets = arrow::buffer::OffsetBuffer::new(vec![0i32, 0, 1].into());
            let lists = ListArray::new(field, offsets, values, None);
            assert!(compare_column(Arc::new(lists)).unwrap().has_differences);
        }
    }

    #[test]
    fn dictionary_values_use_the_same_interval_and_null_rules() {
        let intervals: ArrayRef = Arc::new(IntervalMonthDayNanoArray::from(vec![
            IntervalMonthDayNanoType::make_value(1, 0, 0),
            IntervalMonthDayNanoType::make_value(0, 30, 0),
        ]));
        let dictionary =
            DictionaryArray::<Int8Type>::try_new(Int8Array::from(vec![0, 1]), intervals).unwrap();
        assert!(
            !compare_column(Arc::new(dictionary))
                .unwrap()
                .has_differences
        );

        let bytes: ArrayRef = Arc::new(BinaryArray::from(vec![Some(b"".as_slice()), None]));
        let dictionary =
            DictionaryArray::<Int8Type>::try_new(Int8Array::from(vec![0, 1]), bytes).unwrap();
        assert!(
            compare_column(Arc::new(dictionary))
                .unwrap()
                .has_differences
        );
    }

    #[test]
    fn interval_comparison_uses_sql_month_day_and_time_equivalence() {
        let value = IntervalMonthDayNanoType::make_value;
        const DAY: i64 = 86_400_000_000_000;
        for (expected, actual, differs) in [
            (value(1, 0, 0), value(0, 30, 0), false),
            (value(0, 1, 0), value(0, 0, DAY), false),
            (value(-1, 30, 0), value(0, 0, 0), false),
            (value(0, -1, 1), value(0, 0, -DAY + 1), false),
            (value(i32::MAX, 0, 0), value(i32::MAX, 1, -DAY), false),
            (value(1, 0, 0), value(0, 31, 0), true),
            (value(i32::MAX, 0, 0), value(i32::MAX, 0, 1), true),
        ] {
            let values = IntervalMonthDayNanoArray::from(vec![expected, actual]);
            assert_eq!(
                compare_column(Arc::new(values)).unwrap().has_differences,
                differs
            );
        }
        let values = IntervalDayTimeArray::from(vec![
            IntervalDayTimeType::make_value(1, 0),
            IntervalDayTimeType::make_value(0, 86_400_000),
        ]);
        assert!(!compare_column(Arc::new(values)).unwrap().has_differences);
    }

    #[test]
    fn nested_intervals_use_the_same_comparison_rules() {
        let values = Arc::new(IntervalMonthDayNanoArray::from(vec![
            IntervalMonthDayNanoType::make_value(1, 0, 0),
            IntervalMonthDayNanoType::make_value(0, 30, 0),
        ]));
        let field = Arc::new(Field::new("duration", values.data_type().clone(), true));
        let values = StructArray::from(vec![(field, values as ArrayRef)]);
        assert!(!compare_column(Arc::new(values)).unwrap().has_differences);
    }

    #[test]
    fn nested_formatting_errors_are_not_compared_as_text() {
        let mut values = ListBuilder::new(TimestampMicrosecondBuilder::new());
        for _ in 0..2 {
            values.values().append_value(i64::MAX);
            values.append(true);
        }
        let error = compare_column(Arc::new(values.finish())).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Cannot compare unit test values")
        );
    }

    #[test]
    fn fallback_rules_also_apply_when_json_columns_align_rows() {
        let mut strings = ListBuilder::new(StringBuilder::new());
        strings.append(true);
        strings.values().append_value("");
        strings.append(true);

        let mut invalid_timestamps = ListBuilder::new(TimestampMicrosecondBuilder::new());
        for _ in 0..2 {
            invalid_timestamps.values().append_value(i64::MAX);
            invalid_timestamps.append(true);
        }

        let cases: Vec<(ArrayRef, Option<bool>)> = vec![
            (Arc::new(strings.finish()), Some(true)),
            (
                Arc::new(IntervalMonthDayNanoArray::from(vec![
                    IntervalMonthDayNanoType::make_value(1, 0, 0),
                    IntervalMonthDayNanoType::make_value(0, 30, 0),
                ])),
                Some(false),
            ),
            (Arc::new(invalid_timestamps.finish()), None),
        ];
        for (column, differs) in cases {
            let batch = typed_batch(column.data_type().clone(), column);
            let mut fields = batch.schema().fields().to_vec();
            fields.push(Arc::new(Field::new("json", DataType::Utf8, false)));
            let mut columns = batch.columns().to_vec();
            columns.push(Arc::new(StringArray::from(vec![
                r#"{"a":1,"b":2}"#,
                r#"{"b":2,"a":1}"#,
            ])));
            let batch =
                arrow::array::RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap();
            let result = compare_record_batches_with_json_columns(&batch, &[2]);
            match differs {
                Some(differs) => assert_eq!(result.unwrap().has_differences, differs),
                None => assert!(result.is_err()),
            }
        }
    }

    #[test]
    fn named_timezone_timestamps_keep_typed_comparison() {
        for timezone in ["UTC", "America/New_York"] {
            for (actual, differs) in [(1, false), (2, true)] {
                let values =
                    TimestampMicrosecondArray::from(vec![1, actual]).with_timezone(timezone);
                assert_eq!(
                    compare_column(Arc::new(values)).unwrap().has_differences,
                    differs
                );
            }
        }
    }

    fn json_batch(expected: Option<&str>, actual: Option<&str>) -> arrow::array::RecordBatch {
        json_rows_batch(&[expected], &[actual])
    }

    fn json_rows_batch(
        expected: &[Option<&str>],
        actual: &[Option<&str>],
    ) -> arrow::array::RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("actual_or_expected", DataType::Utf8View, false),
            Field::new("value", DataType::Utf8View, true),
        ]));
        arrow::array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringViewArray::from(
                    std::iter::repeat_n("expected", expected.len())
                        .chain(std::iter::repeat_n("actual", actual.len()))
                        .collect::<Vec<_>>(),
                )),
                Arc::new(StringViewArray::from(
                    expected.iter().chain(actual).copied().collect::<Vec<_>>(),
                )),
            ],
        )
        .unwrap()
    }

    #[test]
    fn json_comparison_ignores_nested_object_key_order() {
        let batch = json_batch(
            Some(r#"{"z":0.1,"nested":[{"b":2,"a":1},3],"a":null}"#),
            Some(r#"{ "a": null, "nested": [{"a":1,"b":2},3], "z": 0.1 }"#),
        );
        let result = compare_record_batches_with_json_columns(&batch, &[1]).unwrap();
        assert!(!result.has_differences);
        assert_eq!((result.actual_rows, result.expected_rows), (1, 1));
    }

    #[test]
    fn json_comparison_preserves_values_arrays_and_nulls() {
        for (expected, actual) in [
            (Some(r#"{"a":1}"#), Some(r#"{"a":2}"#)),
            (Some(r#"{"a":[1,2]}"#), Some(r#"{"a":[2,1]}"#)),
            (Some(r#"{"a":true}"#), Some(r#"{"a":"true"}"#)),
            (Some("null"), Some(r#""null""#)),
            (None, Some("null")),
        ] {
            let batch = json_batch(expected, actual);
            let result = compare_record_batches_with_json_columns(&batch, &[1]).unwrap();
            assert!(
                result.has_differences,
                "{expected:?} must differ from {actual:?}"
            );
        }
        let result =
            compare_record_batches_with_json_columns(&json_batch(None, None), &[1]).unwrap();
        assert!(!result.has_differences);
    }

    #[test]
    fn json_comparison_preserves_high_precision_numbers() {
        for (expected, actual) in [
            (
                r#"{"n":18446744073709551616}"#,
                r#"{"n":18446744073709551617}"#,
            ),
            (
                r#"{"n":12345678901234567890.1234567890}"#,
                r#"{"n":12345678901234567890.1234567891}"#,
            ),
        ] {
            let batch = json_batch(Some(expected), Some(actual));
            let result = compare_record_batches_with_json_columns(&batch, &[1]).unwrap();
            assert!(
                result.has_differences,
                "numeric precision must be preserved"
            );
        }
    }

    #[test]
    fn json_comparison_keeps_plain_strings_byte_exact() {
        let batch = json_batch(Some(r#"{"b":2,"a":1}"#), Some(r#"{"a":1,"b":2}"#));
        let result = compare_record_batches(&batch).unwrap();
        assert!(result.has_differences);
    }

    #[test]
    fn json_comparison_aligns_rows_and_preserves_duplicate_counts() {
        let expected = [
            Some(r#"{"a":2,"z":0}"#),
            Some(r#"{"z":0,"a":1}"#),
            Some(r#"{"a":1,"z":0}"#),
        ];
        for (actual, has_differences) in [
            (
                [
                    Some(r#"{"a":1,"z":0}"#),
                    Some(r#"{"z":0,"a":2}"#),
                    Some(r#"{"z":0,"a":1}"#),
                ],
                false,
            ),
            (
                [
                    Some(r#"{"a":1,"z":0}"#),
                    Some(r#"{"z":0,"a":2}"#),
                    Some(r#"{"z":0,"a":2}"#),
                ],
                true,
            ),
        ] {
            let batch = json_rows_batch(&expected, &actual);
            let result = compare_record_batches_with_json_columns(&batch, &[1]).unwrap();
            assert_eq!(result.has_differences, has_differences);
            assert_eq!((result.actual_rows, result.expected_rows), (3, 3));
        }
    }

    // Query results are normalized to Utf8View/LargeUtf8 at the adapter boundary
    // (dbt-adapter's concat_batches::to_view_types), so AgateTable's batches carry
    // Utf8View columns rather than plain Utf8. Regression test for a real mismatch
    // being masked as a match once that switch happened.
    #[test]
    fn detects_mismatch_in_utf8_view_column() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("actual_or_expected", DataType::Utf8View, false),
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8View, false),
        ]));
        let batch = arrow::array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringViewArray::from(vec!["expected", "actual"])),
                Arc::new(Int32Array::from(vec![1, 1])),
                Arc::new(StringViewArray::from(vec!["alice", "bob"])),
            ],
        )
        .unwrap();

        let result = compare_record_batches(&batch).unwrap();
        assert!(
            result.has_differences,
            "expected 'alice' vs 'bob' to be flagged as a mismatch, but Utf8View columns were silently treated as unsupported"
        );
    }

    #[test]
    fn no_differences_when_utf8_view_values_match() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("actual_or_expected", DataType::Utf8View, false),
            Field::new("name", DataType::Utf8View, false),
        ]));
        let batch = arrow::array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringViewArray::from(vec!["expected", "actual"])),
                Arc::new(StringViewArray::from(vec!["alice", "alice"])),
            ],
        )
        .unwrap();

        let result = compare_record_batches(&batch).unwrap();
        assert!(!result.has_differences);
    }

    #[test]
    fn empty_non_string_label_column_has_no_differences() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("actual_or_expected", DataType::Int64, false),
            Field::new("id", DataType::Int64, false),
        ]));
        let batch = arrow::array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(Int64Array::from(Vec::<i64>::new())),
            ],
        )
        .unwrap();

        let result = compare_record_batches(&batch).unwrap();
        assert!(!result.has_differences);
        assert_eq!(result.actual_rows, 0);
        assert_eq!(result.expected_rows, 0);
        assert_eq!(result.diff_batch.schema(), batch.schema());
    }

    #[test]
    fn non_empty_invalid_label_type_returns_schema_error() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("actual_or_expected", DataType::Int64, false),
            Field::new("id", DataType::Int64, false),
        ]));
        let batch = arrow::array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![1])),
                Arc::new(Int64Array::from(vec![1])),
            ],
        )
        .unwrap();

        let error = compare_record_batches(&batch).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Schema error: 'actual_or_expected' column must be a string or binary, found Int64"
        );
    }

    #[test]
    fn accepts_binary_label_column() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("actual_or_expected", DataType::Binary, false),
            Field::new("id", DataType::Int32, false),
        ]));
        let batch = arrow::array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(BinaryArray::from(vec![
                    b"expected".as_slice(),
                    b"actual".as_slice(),
                ])),
                Arc::new(Int32Array::from(vec![1, 1])),
            ],
        )
        .unwrap();

        let result = compare_record_batches(&batch).unwrap();
        assert!(!result.has_differences);
    }

    /// One expected cell and one actual cell of the same column, built from the
    /// typed values the warehouse would hand back.
    fn typed_batch(data_type: DataType, column: ArrayRef) -> arrow::array::RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("actual_or_expected", DataType::Utf8, false),
            Field::new("value", data_type, true),
        ]));
        arrow::array::RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["expected", "actual"])),
                column,
            ],
        )
        .unwrap()
    }

    /// dbt-labs/dbt#16651: `format!("{:.1}")` made every float difference below
    /// 0.1 render identically, so the string comparison reported a match.
    #[test]
    fn float_differences_below_one_decimal_place_are_differences() {
        for (expected, actual) in [(0.01f64, 0.0), (300.01, 300.0), (0.76, 0.75)] {
            let batch = typed_batch(
                DataType::Float64,
                Arc::new(Float64Array::from(vec![expected, actual])),
            );
            let result = compare_record_batches(&batch).unwrap();
            assert!(
                result.has_differences,
                "{expected} must differ from {actual}"
            );
        }
    }

    #[test]
    fn equal_floats_still_match() {
        for value in [0.0f64, 300.01, f64::NAN, f64::INFINITY] {
            let batch = typed_batch(
                DataType::Float64,
                Arc::new(Float64Array::from(vec![value, value])),
            );
            let result = compare_record_batches(&batch).unwrap();
            assert!(!result.has_differences, "{value} must match itself");
        }
        // `-0.0 == 0.0` in IEEE 754, and a signed zero is not a meaningful
        // difference for a fixture to assert on.
        let batch = typed_batch(
            DataType::Float64,
            Arc::new(Float64Array::from(vec![0.0f64, -0.0])),
        );
        assert!(!compare_record_batches(&batch).unwrap().has_differences);
    }

    /// dbt-labs/dbt#16649: microsecond timestamps fell through to the
    /// `[unsupported]` catch-all, which compares equal to itself, so any wrong
    /// expected timestamp passed.
    #[test]
    fn microsecond_timestamp_differences_are_differences() {
        let batch = typed_batch(
            DataType::Timestamp(TimeUnit::Microsecond, None),
            Arc::new(TimestampMicrosecondArray::from(vec![
                1_893_456_000_000_000, // 2030-01-01
                1_767_225_600_000_000, // 2026-01-01
            ])),
        );
        let result = compare_record_batches(&batch).unwrap();
        assert!(result.has_differences);
    }

    #[test]
    fn millisecond_timestamp_differences_are_differences() {
        let batch = typed_batch(
            DataType::Timestamp(TimeUnit::Millisecond, None),
            Arc::new(TimestampMillisecondArray::from(vec![1_000, 1_500])),
        );
        assert!(compare_record_batches(&batch).unwrap().has_differences);
    }

    /// Sub-second differences used to be divided away before the comparison.
    #[test]
    fn nanosecond_timestamp_sub_second_differences_are_differences() {
        let batch = typed_batch(
            DataType::Timestamp(TimeUnit::Nanosecond, None),
            Arc::new(TimestampNanosecondArray::from(vec![500_000_000, 0])),
        );
        assert!(compare_record_batches(&batch).unwrap().has_differences);
    }

    /// Truncating division towards zero used to render a pre-epoch sub-second
    /// instant as if it were after the epoch, so the two sides collided.
    #[test]
    fn pre_epoch_sub_second_timestamps_do_not_collide() {
        let batch = typed_batch(
            DataType::Timestamp(TimeUnit::Nanosecond, None),
            Arc::new(TimestampNanosecondArray::from(vec![
                -500_000_000,
                500_000_000,
            ])),
        );
        let result = compare_record_batches(&batch).unwrap();
        assert!(result.has_differences);
        let rendered = pretty_format_batches(&[result.diff_batch])
            .unwrap()
            .to_string();
        assert!(
            rendered.contains("1969-12-31 23:59:59.500 UTC"),
            "pre-epoch instant must render before the epoch, got:\n{rendered}"
        );
    }

    /// The old decimal formatting dropped the sign for |v| < 1 and dropped
    /// leading fractional zeros, so these pairs rendered the same string.
    #[test]
    fn decimal_sign_and_leading_zeros_are_differences() {
        for (expected, actual) in [(-50i128, 50), (5, 5000)] {
            let column = Decimal128Array::from(vec![expected, actual])
                .with_precision_and_scale(10, 2)
                .unwrap();
            let batch = typed_batch(DataType::Decimal128(10, 2), Arc::new(column));
            assert!(
                compare_record_batches(&batch).unwrap().has_differences,
                "raw {expected} must differ from raw {actual} at scale 2"
            );
        }
    }

    /// BigQuery admits `ARRAY` columns into unit tests, so a list reaches the
    /// comparator. It used to render `[unsupported]` on both sides; element
    /// order is now compared.
    #[test]
    fn list_element_order_is_a_difference() {
        let expected = ListArray::from_iter_primitive::<Int32Type, _, _>(vec![
            Some(vec![Some(1), Some(2), Some(3)]),
            Some(vec![Some(3), Some(2), Some(1)]),
        ]);
        let batch = typed_batch(
            DataType::List(Arc::new(Field::new("item", DataType::Int32, true))),
            Arc::new(expected),
        );
        assert!(compare_record_batches(&batch).unwrap().has_differences);
    }

    #[test]
    fn equal_lists_still_match() {
        let column = ListArray::from_iter_primitive::<Int32Type, _, _>(vec![
            Some(vec![Some(1), Some(2)]),
            Some(vec![Some(1), Some(2)]),
        ]);
        let batch = typed_batch(
            DataType::List(Arc::new(Field::new("item", DataType::Int32, true))),
            Arc::new(column),
        );
        assert!(!compare_record_batches(&batch).unwrap().has_differences);
    }

    /// The diff table is what the user reads to find out what went wrong, so a
    /// mismatch must show both values rather than a placeholder.
    #[test]
    fn diff_table_shows_real_values_not_a_placeholder() {
        let batch = typed_batch(
            DataType::Timestamp(TimeUnit::Microsecond, None),
            Arc::new(TimestampMicrosecondArray::from(vec![
                1_893_456_000_000_000,
                1_767_225_600_000_000,
            ])),
        );
        let result = compare_record_batches(&batch).unwrap();
        let rendered = pretty_format_batches(&[result.diff_batch])
            .unwrap()
            .to_string();
        assert!(
            rendered.contains("2030-01-01 00:00:00 UTC -> 2026-01-01 00:00:00 UTC"),
            "diff must name both instants, got:\n{rendered}"
        );
        assert!(!rendered.contains("[unsupported]"));
    }

    /// A null is only equal to another null, whatever the column type.
    #[test]
    fn nulls_compare_against_nulls_only() {
        let batch = typed_batch(
            DataType::Timestamp(TimeUnit::Microsecond, None),
            Arc::new(TimestampMicrosecondArray::from(vec![None, Some(0)])),
        );
        assert!(compare_record_batches(&batch).unwrap().has_differences);

        let batch = typed_batch(
            DataType::Timestamp(TimeUnit::Microsecond, None),
            Arc::new(TimestampMicrosecondArray::from(vec![
                None::<i64>,
                None::<i64>,
            ])),
        );
        assert!(!compare_record_batches(&batch).unwrap().has_differences);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn materialize_function(
    sql: &str,
    function: &DbtFunction,
    adapter_type: AdapterType,
    runtime_config: &DbtRuntimeConfig,
    materialization_resolver: &Arc<MaterializationResolver>,
    jinja_env: Arc<JinjaEnv>,
    base_context: &BTreeMap<String, Value>,
    io_args: &IoArgs,
) -> FsResult<Value> {
    let (mut context, _result_store) = build_run_node_context(
        function,
        &function.deprecated_config,
        adapter_type,
        None,
        base_context,
        io_args,
        ExecutionPhase::Run,
        None,
        runtime_config.dependencies.keys().cloned().collect(),
    );

    // Find the function materialization macro
    let macro_name = materialization_resolver
        .find_materialization_macro_by_name("function", function.node_adapter())?;

    context.insert("sql".to_string(), Value::from(sql));
    context.insert("compiled_code".to_string(), Value::from(sql));

    let compiled_path =
        function.get_node_path_abs(NodePathKind::Compiled, &io_args.in_dir, &io_args.out_dir);

    let unique_id = function.__common_attr__.unique_id.clone();
    let node_alias = function.__base_attr__.alias.clone();
    let run_path = function
        .get_node_path(NodePathKind::Executable, &io_args.in_dir, &io_args.out_dir)
        .into_owned();

    let result = execute_materialization_macro(
        jinja_env,
        &macro_name,
        &mut context,
        "function",
        &unique_id,
        &node_alias,
        run_path,
    );

    // Write compiled SQL to file
    if let Some(parent) = compiled_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&compiled_path, sql);

    result
}
