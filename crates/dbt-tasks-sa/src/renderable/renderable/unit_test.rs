extern crate num_cpus;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Debug;
use std::io::Cursor;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use dbt_adapter::errors::into_fs_error;
use dbt_adapter::formatter::SqlLiteralFormatter;
use dbt_adapter::metadata::BIGQUERY_PSEUDOCOLUMNS;
use dbt_adapter::relation::{RelationObject, create_relation_from_node};
use dbt_adapter::sql_types::{DefaultTypeOps, TypeOps, bigquery, make_arrow_field};
use dbt_adapter::{Adapter, Column};
use dbt_adapter_core::{AdapterType, ExecutionPhase, quote_char};
use dbt_common::cancellation::Cancellable;
use dbt_common::collections::DashMap;
use dbt_common::constants::DBT_CTE_PREFIX;
use dbt_common::io_args::{ReplayMode, TimeMachineMode};
use dbt_common::static_analysis::is_static_analysis_off_or_baseline;
use dbt_common::stats::NodeStatus;
use dbt_common::stdfs;
use dbt_common::{ErrorCode, FsResult, MacroSpansOnly, err, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::phases::compile::DependencyValidationConfig;
use dbt_jinja_utils::phases::run::build_run_node_context;
use dbt_jinja_utils::serde::single_expression_body;
use dbt_jinja_utils::utils::add_task_context;
use dbt_jinja_utils::utils::macro_spans_to_macro_span_vec;
use dbt_jinja_utils::utils::render_sql;
use dbt_jinja_utils::{Var, env_var};
use dbt_scheduler::instructions::SqlInstruction;
use dbt_schemas::schemas;
use dbt_schemas::schemas::InternalDbtNode;
use dbt_schemas::schemas::common::{DbtMaterialization, Rows};
use dbt_schemas::schemas::properties::UnitTestOverrides;
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::{DbtUnitTest, InternalDbtNodeAttributes, NodePathKind};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::render_task_hooks::RenderTaskHooks;
use dbt_tasks_core::task::TaskResult;
use dbt_tasks_core::unit_test_schema::{UnitTestExpectedSchemaKey, UnitTestExpectedSchemaKeyInput};
use dbt_telemetry::{ExecutionPhase as TelemetryExecutionPhase, NodeType};

use crate::renderable::unit_test_typing::{DatabricksTyping, SnowflakeTyping};
use crate::task::effective_unit_test_execute;

use super::common::handle_render_result;
use crate::sql::dialect::sqlparser_dialect_for;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use csv::ReaderBuilder;
use itertools::Itertools;
use minijinja::listener::RenderingEventListener;
use minijinja::value::Object;
use minijinja::{CodeLocation, State, Value, Value as MinijinjaValue};
use regex::Regex;
use sqlparser::tokenizer::{Token, Tokenizer, Whitespace};

type YmlValue = dbt_yaml::Value;

const COMPILED_INCREMENTAL_SCHEMA_MARKER: &str = ".compiled_incremental_unit_test_schema_v1";

/// Identifies which unit test relation schema is being resolved for error reporting.
#[derive(Clone, Copy)]
enum UnitTestSchemaTarget {
    GivenUpstream,
    ExpectedModel { incremental_expected_path: bool },
}

impl UnitTestSchemaTarget {
    fn subject(self) -> &'static str {
        match self {
            Self::GivenUpstream => "unit test upstream relation from `given`",
            Self::ExpectedModel {
                incremental_expected_path: false,
            } => "unit test model-under-test relation from `expect`",
            Self::ExpectedModel {
                incremental_expected_path: true,
            } => "incremental unit test model-under-test relation from `expect`",
        }
    }
}

struct UnitTestSchemaProbe {
    schema_sql: String,
    ctes: String,
    query_sql: String,
}

#[derive(Clone, Copy)]
struct ExpectedSchemaInferenceOptions {
    use_query_schema_fallback: bool,
    try_structural_inference: bool,
    cache_result: bool,
}

fn should_use_query_schema_fallback(
    is_tested_model_ephemeral: bool,
    effective_execute: schemas::profiles::Execute,
    replay_active: bool,
) -> bool {
    // Warehouse recordings contain the temporary-table probe, not the local
    // query-schema calls, so replay must preserve the recorded path.
    is_tested_model_ephemeral || (!effective_execute.is_default() && !replay_active)
}

fn has_is_incremental_false_override(unit_test: &DbtUnitTest) -> bool {
    unit_test
        .__unit_test_attr__
        .overrides
        .as_ref()
        .and_then(|overrides| overrides.macros.as_ref().as_ref())
        .and_then(|macros| macros.get("is_incremental"))
        .and_then(YmlValue::as_bool)
        == Some(false)
}

fn recording_uses_compiled_incremental_schema(ctx: &TaskRunnerCtx) -> FsResult<bool> {
    let marker_path = match ctx.inner.arg.replay.as_ref() {
        Some(ReplayMode::FsRecord(path)) => Some((path.clone(), true)),
        Some(ReplayMode::FsReplay(path)) => Some((path.clone(), false)),
        Some(ReplayMode::FsTimeMachine(TimeMachineMode::Record(config))) => Some((
            config.output_path.join(config.invocation_id.to_string()),
            true,
        )),
        Some(ReplayMode::FsTimeMachine(TimeMachineMode::Replay(config))) => {
            Some((config.artifact_path.clone(), false))
        }
        Some(ReplayMode::MantleReplay(_)) => None,
        None => return Ok(true),
    };

    let Some((recording_path, is_recording)) = marker_path else {
        return Ok(false);
    };
    let marker = recording_path.join(COMPILED_INCREMENTAL_SCHEMA_MARKER);
    if is_recording {
        stdfs::create_dir_all(&recording_path)?;
        stdfs::write(marker, b"1\n")?;
        Ok(true)
    } else {
        Ok(marker.is_file())
    }
}

fn use_compiled_schema_for_incremental(
    ctx: &TaskRunnerCtx,
    unit_test: &DbtUnitTest,
) -> FsResult<bool> {
    if !has_is_incremental_false_override(unit_test) {
        return Ok(false);
    }
    recording_uses_compiled_incremental_schema(ctx)
}

fn is_replay_active(ctx: &TaskRunnerCtx) -> bool {
    ctx.inner.arg.replay.as_ref().is_some_and(|mode| {
        matches!(mode, ReplayMode::MantleReplay(_) | ReplayMode::FsReplay(_))
            || mode.is_time_machine_replay()
    }) || ctx
        .env
        .get_base_adapter()
        .is_some_and(|adapter| adapter.as_replay().is_some())
}

/// Small utility for merging objects
#[derive(Debug, Clone)]
struct ObjectOverlay(pub Vec<Value>);

impl Object for ObjectOverlay {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        self.0
            .iter()
            // Err if undefined
            .filter_map(|o| o.get_item(key).ok())
            .next()
    }
}

/// Orchestrate the 3-phase unit test render pipeline.
///
/// ```text
/// Phase 1 — Discover (blocking): discover_given_relations → (given_relations, to_fetch)
/// Phase 2 — Fetch    (async):    fetch_missing_schemas(to_fetch) → schemas in cache
/// Phase 3 — Render   (blocking): render_unit_test(given_relations) → SqlInstruction
/// ```
///
/// Errors from any phase are funneled through `handle_render_result` so a
/// `run_results.json` entry is recorded when the failure happens before render.
pub(crate) async fn run_unit_test_render(
    node: Arc<dyn InternalDbtNodeAttributes>,
    mut ctx: TaskRunnerCtx,
    result_sender: Option<std::sync::mpsc::SyncSender<TaskResult>>,
    task_hooks: Arc<dyn RenderTaskHooks>,
) -> FsResult<NodeStatus> {
    use dbt_tasks_core::task::TaskOp;

    // Phase 1 — Discover
    let ut = node.clone();
    let mut discover_ctx = ctx.clone();
    let discover_outcome = TaskOp::Blocking(Box::new(move || {
        // Downcast to DbtUnitTest for access to unit test fields
        let ut_ref = ut
            .as_any()
            .downcast_ref::<DbtUnitTest>()
            .expect("run_unit_test_render called on non-DbtUnitTest");
        discover_given_relations(ut_ref, &mut discover_ctx)
    }))
    .run()
    .await
    .and_then(|inner| inner);
    let given_relations = match discover_outcome {
        Ok(r) => r,
        Err(e) => {
            return handle_render_result(
                Err(e),
                &node.unique_id(),
                &node.materialized(),
                &mut ctx,
                &result_sender,
            );
        }
    };

    // Phase 2 — Fetch
    if !given_relations.relations_to_fetch.is_empty() {
        let fetch_outcome = TaskOp::r#async(fetch_missing_schemas(
            &given_relations.relations_to_fetch,
            &node.common().unique_id,
            &mut ctx,
            Arc::clone(&task_hooks),
        ))
        .await;
        if let Err(e) = fetch_outcome {
            return handle_render_result(
                Err(e),
                &node.unique_id(),
                &node.materialized(),
                &mut ctx,
                &result_sender,
            );
        }
    }

    // Phase 3 — Render (uses cached schemas + discovered relations)
    TaskOp::Blocking(Box::new(move || {
        let mut ctx = ctx;
        let ut_ref = node
            .as_any()
            .downcast_ref::<DbtUnitTest>()
            .expect("run_unit_test_render called on non-DbtUnitTest");
        let res = render_unit_test(ut_ref, &mut ctx, given_relations, task_hooks.as_ref());
        handle_render_result(
            res,
            &node.unique_id(),
            &node.materialized(),
            &mut ctx,
            &result_sender,
        )
    }))
    .run()
    .await?
}

/// Get schema for upstream unit test inputs, using cache first and remote fallback.
/// Used during the Render phase (after the Fetch phase has populated the cache).
fn get_schema_for_unit_test_relation(
    ctx: &TaskRunnerCtx,
    relation: Arc<dyn BaseRelation>,
    schema_target: UnitTestSchemaTarget,
) -> FsResult<SchemaRef> {
    try_get_schema_from_cache(ctx, relation.as_ref(), schema_target)?.ok_or_else(|| {
        fs_err!(
            ErrorCode::Generic,
            "Schema for {} '{}' was not fetched during pre-render step",
            schema_target.subject(),
            relation.render_self_as_str()
        )
    })
}

/// Try to get schema from cache. Returns `Ok(Some(schema))` if found,
/// `Ok(None)` if a warehouse fetch is allowed, or `Err` if fetch is not allowed.
fn try_get_schema_from_cache(
    ctx: &TaskRunnerCtx,
    relation: &dyn BaseRelation,
    schema_target: UnitTestSchemaTarget,
) -> FsResult<Option<SchemaRef>> {
    let canonical_fqn = relation.get_canonical_fqn()?;
    if let Some(entry) = ctx.schema_cache.get_schema(&canonical_fqn) {
        return Ok(Some(entry.inner().clone()));
    }

    // For upstreams (models, seeds, snapshots, sources), try to get schema from the schema cache by unique_id
    let resolver_state = ctx.resolver_state();
    let mut node_static_analysis_off = false;
    // Iterate over all nodes
    for (node_unique_id, node) in resolver_state.nodes.iter() {
        // If a node is a model, seed, snapshot, or source (testable nodes)
        if node_unique_id.starts_with("model.")
            || node_unique_id.starts_with("seed.")
            || node_unique_id.starts_with("snapshot.")
            || node_unique_id.starts_with("source.")
        {
            let node_relation = create_relation_from_node(node.node_adapter(), node, None)?;
            let node_canonical_fqn = node_relation.get_canonical_fqn().ok();

            // If we have a hit
            if node_canonical_fqn.as_ref() == Some(&canonical_fqn) {
                node_static_analysis_off =
                    is_static_analysis_off_or_baseline(node.static_analysis().into_inner());
                break;
            }
        }
    }

    if ctx.inner.execute.is_default() && !node_static_analysis_off {
        return Err(fs_err!(
            ErrorCode::Generic,
            "Failed to get cached schema for {} '{}'",
            schema_target.subject(),
            relation.render_self_as_str()
        ));
    }

    // Schema not in cache, needs async fetch
    Ok(None)
}

/// Resolve a missing relation schema through the render hook or warehouse fallback.
async fn hydrate_unit_test_relation_schema(
    ctx: &TaskRunnerCtx,
    relation: Arc<dyn BaseRelation>,
    unit_test_unique_id: &str,
    fetched: &mut HashSet<String>,
    schema_target: UnitTestSchemaTarget,
    task_hooks: Arc<dyn RenderTaskHooks>,
    adapter: &Adapter,
) -> FsResult<SchemaRef> {
    let canonical_fqn = relation.get_canonical_fqn()?;
    let semantic_fqn = relation.semantic_fqn();
    let err_subject = || {
        format!(
            "{} '{}'",
            schema_target.subject(),
            relation.render_self_as_str()
        )
    };

    task_hooks
        .will_fetch_schema_for_unit_test_relation(
            ctx,
            unit_test_unique_id,
            fetched,
            &relation,
            adapter.engine().type_ops(),
        )
        .await
        .map_err(|e| {
            fs_err!(
                ErrorCode::Generic,
                "Failed to query schema for {}: {}",
                err_subject(),
                e.message()
            )
        })?;

    // `fetched` tracks fqns whose schemas are already populated in `schema_cache`
    // during this call. On repeat, short-circuit to the cache.
    if fetched.contains(&semantic_fqn) {
        return ctx
            .schema_cache
            .get_schema_async(&canonical_fqn)
            .await
            .map(|entry| entry.inner().clone())
            .ok_or_else(|| {
                fs_err!(
                    ErrorCode::Generic,
                    "Failed to retrieve schema from cache for {}",
                    err_subject()
                )
            });
    }

    // Metadata-adapter path: non-sidecar modes and the sidecar `Ok(None)`
    // fall-through both land here and query the warehouse catalog directly.
    let Some(metadata_adapter) = adapter.metadata_adapter() else {
        return Err(fs_err!(
            ErrorCode::UnsupportedFeature,
            "Adapter '{}' does not support metadata operations required to resolve {}",
            adapter.adapter_type(),
            schema_target.subject()
        ));
    };

    let relations = vec![relation.clone()];
    let schemas = metadata_adapter
        .list_relations_sdf_schemas(
            adapter.engine().as_ref(),
            Some(unit_test_unique_id.to_string()),
            Some(ExecutionPhase::Analyze),
            &relations,
            None,
            adapter.cancellation_token(),
        )
        .await
        .map_err(|e| {
            into_fs_error(e).with_context(format!(
                "Failed to execute metadata adapter call to fetch schema for {}",
                err_subject()
            ))
        })?;

    let schema_res = schemas.get(&semantic_fqn).ok_or_else(|| {
        fs_err!(
            ErrorCode::RemoteError,
            "No schema found for {}",
            err_subject()
        )
    })?;
    match schema_res {
        Ok(schema) => {
            let entry = ctx.schema_cache.register_schema(
                &canonical_fqn,
                schema.original().map(Arc::clone),
                Arc::clone(schema.inner()),
                true,
            )?;
            fetched.insert(semantic_fqn);
            Ok(entry.inner().clone())
        }
        Err(err) => Err(into_fs_error(Cancellable::Error(err.clone()))
            .with_context(format!(
                "Remote database error while fetching schema for {}",
                err_subject()
            ))
            .into()),
    }
}

async fn fetch_schema_for_unit_test_relation(
    ctx: &TaskRunnerCtx,
    relation: Arc<dyn BaseRelation>,
    unit_test_unique_id: &str,
    fetched: &mut HashSet<String>,
    schema_target: UnitTestSchemaTarget,
    task_hooks: Arc<dyn RenderTaskHooks>,
) -> FsResult<SchemaRef> {
    let canonical_fqn = relation.get_canonical_fqn()?;
    let semantic_fqn = relation.semantic_fqn();
    let adapter = ctx.env.get_base_adapter().ok_or_else(|| {
        fs_err!(
            ErrorCode::Generic,
            "Failed to fetch schema for {} '{}': adapter unavailable",
            schema_target.subject(),
            relation.render_self_as_str()
        )
    })?;

    let is_local_unit_test =
        ctx.nodes()
            .unit_tests
            .get(unit_test_unique_id)
            .is_some_and(|unit_test| {
                effective_unit_test_execute(unit_test, ctx.inner.execute)
                    == schemas::profiles::Execute::Sidecar
            });
    let coordinate_fetch = is_local_unit_test
        && matches!(schema_target, UnitTestSchemaTarget::GivenUpstream)
        && adapter.as_replay().is_none()
        && !adapter.engine().is_mock();
    if !coordinate_fetch {
        return hydrate_unit_test_relation_schema(
            ctx,
            relation,
            unit_test_unique_id,
            fetched,
            schema_target,
            task_hooks,
            &adapter,
        )
        .await;
    }

    let fetched_for_fetch = &mut *fetched;
    let schema = ctx
        .inner
        .unit_test_schema
        .get_or_try_fetch_fixture_schema(canonical_fqn, ctx.schema_cache.as_ref(), || async move {
            hydrate_unit_test_relation_schema(
                ctx,
                relation,
                unit_test_unique_id,
                fetched_for_fetch,
                schema_target,
                task_hooks,
                &adapter,
            )
            .await
        })
        .await?;
    fetched.insert(semantic_fqn);
    Ok(schema)
}

fn columns_to_schema(
    type_ops: &dyn TypeOps,
    columns: Vec<Column>,
    unique_id: &str,
) -> FsResult<SchemaRef> {
    let mut fields = Vec::with_capacity(columns.len());

    for column in columns {
        let sql_type_str: Cow<str> = column
            .original_sql_str()
            .map(Cow::from)
            .unwrap_or_else(|| column.data_type().into());

        let field = make_arrow_field(
            type_ops,
            column.name().to_string(),
            sql_type_str.as_ref(),
            None,
            None,
        )
        .map_err(|e| {
            fs_err!(
                ErrorCode::Generic,
                "Failed to parse column type '{}' for unit test {}: {}",
                sql_type_str,
                unique_id,
                e
            )
        })?;
        fields.push(field);
    }

    Ok(Arc::new(Schema::new(fields)))
}

fn build_unit_test_schema_probe(
    adapter_type: AdapterType,
    type_ops: &dyn TypeOps,
    compiled_model_sql: &str,
    rewrite_targets: &[(String, String)],
) -> UnitTestSchemaProbe {
    let schema_sql = replace_subquery_refs_with_cte_names(
        adapter_type,
        type_ops,
        compiled_model_sql.to_string(),
        rewrite_targets,
    );
    let ctes = rewrite_targets
        .iter()
        .filter_map(|(id, query)| {
            let cte_name = create_cte_name_from_fqn(adapter_type, type_ops, id);
            schema_sql
                .contains(&cte_name)
                .then(|| format!("{cte_name} as ({query})"))
        })
        .collect::<Vec<_>>()
        .join(",");
    let query_sql = if ctes.is_empty() {
        schema_sql.clone()
    } else {
        format!("WITH {ctes} {schema_sql}")
    };
    UnitTestSchemaProbe {
        schema_sql,
        ctes,
        query_sql,
    }
}

fn infer_unit_test_expected_schema(
    ctx: &TaskRunnerCtx,
    unit_test: &DbtUnitTest,
    compiled_model_sql: &str,
    subqueries: &[(String, String)],
    fixture_shape_subqueries: &[(String, String)],
    options: ExpectedSchemaInferenceOptions,
    task_hooks: &dyn RenderTaskHooks,
) -> FsResult<SchemaRef> {
    let adapter = ctx.env.get_base_adapter().ok_or_else(|| {
        fs_err!(
            ErrorCode::Generic,
            "Failed to fetch unit test schema: adapter unavailable"
        )
    })?;

    let (mut run_context, _result_store) = build_run_node_context(
        unit_test,
        &unit_test.deprecated_config,
        unit_test.node_adapter(),
        None,
        &ctx.inner.base_context,
        &ctx.inner.arg.io,
        TelemetryExecutionPhase::Render,
        None,
        ctx.runtime_config().dependencies.keys().cloned().collect(),
    );

    // Ensure schema inference macro evaluation is node-scoped for replay and any other
    // context-dependent adapter behavior.
    //
    // Without TARGET_UNIQUE_ID, replay sees an empty node_id ("node ") and disables test-temp
    // alpha conversion, causing mismatches like `__dbt_tmp` vs `__dbt_tmp<digits>`.
    run_context.insert(
        minijinja::constants::TARGET_UNIQUE_ID.to_string(),
        MinijinjaValue::from(unit_test.__common_attr__.unique_id.clone()),
    );
    run_context.insert(
        minijinja::constants::TARGET_PACKAGE_NAME.to_string(),
        MinijinjaValue::from(unit_test.__common_attr__.package_name.clone()),
    );

    if let Some(overrides) = &unit_test.__unit_test_attr__.overrides {
        apply_unit_test_overrides(&mut run_context, overrides, ctx);
    }

    // This is a small part of the dbt-core unit test materialization logic that infers
    // expected schema from the compiled SQL of the model being tested by creating an
    // empty temporary table and inspecting its schema metadata.

    // DuckDB connections are dropped and not returned to the thread-local cache,
    // where temp tables are session-scoped. Sidecar/service adapter calls route
    // DDL through an Execute task, which cannot handle Snowflake-qualified temp
    // tables, so sidecar inference uses the query-schema path as well.
    // The unit test's own adapter throughout: it carries the tested model's
    // (`resolve_unit_tests`), so a test on a non-default model must not be
    // rendered for the default's dialect.
    // ClickHouse: the probe's session-scoped TEMPORARY table is invisible to
    // introspection (never in system.columns), so infer via the query schema.
    let materialization = if unit_test.node_adapter() == AdapterType::DuckDB
        || unit_test.node_adapter() == AdapterType::ClickHouse
        || options.use_query_schema_fallback
    {
        r#"
  {% macro get_expected_columns(sql, select_sql_header) -%}
      {%- if select_sql_header is not none -%}
          {%- set select_sql_header = render(select_sql_header) -%}
      {%- endif -%}
      {%- set columns_in_relation = get_column_schema_from_query(sql, select_sql_header) -%}
      {{ return(columns_in_relation) }}
  {%- endmacro %}"#
    } else {
        r#"
{% macro get_expected_columns(sql, select_sql_header) -%}
    {%- if select_sql_header is not none -%}
        {%- set select_sql_header = render(select_sql_header) -%}
    {%- endif -%}
    {%- set target_relation = this.incorporate(type='table') -%}
    {%- set temp_relation = make_temp_relation(target_relation) -%}
    {% do run_query(get_create_table_as_sql(True, temp_relation, get_empty_subquery_sql(sql, select_sql_header))) %}
    {%- set columns_in_relation = adapter.get_columns_in_relation(temp_relation) -%}
    {% do adapter.drop_relation(temp_relation) %}
    {{ return(columns_in_relation) }}
{%- endmacro %}"#
    };

    // Compile and run a one-off macro without registering it in the shared environment.
    let unique_id = &unit_test.__common_attr__.unique_id;

    // Schema-probe SQL: when the fallback runs outside of default mode,
    // rewrite every mocked subquery into a CTE because we have no warehouse to
    // query. When it runs against the real warehouse, only rewrite ephemerals —
    // non-ephemeral upstreams must remain real relation refs so the warehouse
    // provides their schemas (e.g. for type-checking expect-row literals).
    let fallback_rewrite_targets: Vec<(String, String)> = if options.use_query_schema_fallback {
        subqueries.to_vec()
    } else {
        subqueries
            .iter()
            .filter(|(id, _)| id.starts_with(DBT_CTE_PREFIX))
            .cloned()
            .collect()
    };
    let type_ops = adapter.engine().type_ops();
    let fallback_probe = build_unit_test_schema_probe(
        unit_test.node_adapter(),
        type_ops.as_ref(),
        compiled_model_sql,
        &fallback_rewrite_targets,
    );
    let local_probe_sql = if options.try_structural_inference {
        if options.use_query_schema_fallback {
            Some(fallback_probe.query_sql.clone())
        } else {
            Some(
                build_unit_test_schema_probe(
                    unit_test.node_adapter(),
                    type_ops.as_ref(),
                    compiled_model_sql,
                    subqueries,
                )
                .query_sql,
            )
        }
    } else {
        None
    };

    let cache_key = if options.cache_result {
        let fallback_shape_rewrite_targets: Vec<(String, String)> =
            if options.use_query_schema_fallback {
                fixture_shape_subqueries.to_vec()
            } else {
                fixture_shape_subqueries
                    .iter()
                    .filter(|(id, _)| id.starts_with(DBT_CTE_PREFIX))
                    .cloned()
                    .collect()
            };
        let fallback_probe_shape_sql = build_unit_test_schema_probe(
            unit_test.node_adapter(),
            type_ops.as_ref(),
            compiled_model_sql,
            &fallback_shape_rewrite_targets,
        )
        .query_sql;
        let distinct_local_probe_shape_sql =
            (options.try_structural_inference && !options.use_query_schema_fallback).then(|| {
                build_unit_test_schema_probe(
                    unit_test.node_adapter(),
                    type_ops.as_ref(),
                    compiled_model_sql,
                    fixture_shape_subqueries,
                )
                .query_sql
            });
        let local_probe_shape_sql = distinct_local_probe_shape_sql
            .as_deref()
            .unwrap_or(&fallback_probe_shape_sql);
        let overrides =
            serde_json::to_string(&unit_test.__unit_test_attr__.overrides).map_err(|error| {
                fs_err!(
                    ErrorCode::SerializationError,
                    "Failed to serialize overrides for unit test '{}': {error}",
                    unique_id
                )
            })?;
        Some(UnitTestExpectedSchemaKey::new(
            UnitTestExpectedSchemaKeyInput {
                adapter_type: unit_test.node_adapter(),
                model_unique_id: unit_test
                    .tested_node_unique_id
                    .as_deref()
                    .unwrap_or(unique_id.as_str()),
                local_probe_shape_sql,
                fallback_probe_shape_sql: &fallback_probe_shape_sql,
                fallback_with_query_schema: options.use_query_schema_fallback,
                serialized_overrides: &overrides,
            },
        ))
    } else {
        None
    };

    let UnitTestSchemaProbe {
        schema_sql, ctes, ..
    } = fallback_probe;

    let infer_schema = || {
        if let Some(local_probe_sql) = local_probe_sql.as_deref()
            && let Some(schema) =
                task_hooks.try_infer_unit_test_schema(ctx, unit_test, local_probe_sql)?
        {
            return Ok(schema);
        }

        let template = ctx.env.template_from_str(materialization).map_err(|e| {
            fs_err!(
                ErrorCode::JinjaError,
                "Internal error while compiling macro to infer schema for unit test {}: {}",
                unique_id,
                e
            )
        })?;
        let state = template.eval_to_state(run_context, &[]).map_err(|e| {
            fs_err!(
                ErrorCode::JinjaError,
                "Internal error while preparing macro context to infer schema for unit test {}: {}",
                unique_id,
                e
            )
        })?;
        let func = state.lookup("get_expected_columns", &[]).ok_or_else(|| {
            fs_err!(
                ErrorCode::Unexpected,
                "Internal error: macro lookup failed while inferring schema for unit test {}",
                unique_id
            )
        })?;

        let mut args = vec![Value::from(schema_sql)];
        if ctes.is_empty() {
            args.push(Value::from(()));
        } else {
            args.push(Value::from(format!("WITH {ctes} ")));
        }

        let columns_as_value = func.call(&state, &args, &[]).map_err(|e| {
            fs_err!(
                ErrorCode::JinjaError,
                "Internal error while evaluating macro to infer schema for unit test {}: {}",
                unique_id,
                e
            )
        })?;

        let columns = Column::vec_from_jinja_value(unit_test.node_adapter(), columns_as_value)
            .map_err(|e| {
                fs_err!(
                    ErrorCode::Generic,
                    "Internal error while extracting columns from inferred schema for unit test {}: {}",
                    unit_test.__common_attr__.unique_id,
                    e
                )
            })?;

        columns_to_schema(
            adapter.engine().type_ops().as_ref(),
            columns,
            &unit_test.__common_attr__.unique_id,
        )
    };

    if let Some(key) = cache_key {
        ctx.inner
            .unit_test_schema
            .get_or_try_infer_expected_schema(key, infer_schema)
    } else {
        infer_schema()
    }
}

#[allow(clippy::too_many_arguments)]
fn infer_schema_from_compiled_sql(
    ctx: &TaskRunnerCtx,
    unit_test: &DbtUnitTest,
    raw_model_sql: &str,
    compile_context: &BTreeMap<String, Value>,
    original_file_path: &Path,
    subqueries: &[(String, String)],
    fixture_shape_subqueries: &[(String, String)],
    options: ExpectedSchemaInferenceOptions,
    task_hooks: &dyn RenderTaskHooks,
) -> FsResult<SchemaRef> {
    let compiled_model_sql = render_sql(
        raw_model_sql,
        &ctx.env,
        compile_context,
        ctx.rendering_listener_factory.as_ref(),
        original_file_path,
    )?;

    infer_unit_test_expected_schema(
        ctx,
        unit_test,
        &compiled_model_sql,
        subqueries,
        fixture_shape_subqueries,
        options,
        task_hooks,
    )
}

/// `run_started_at` is a plain datetime global, not a macro invoked with
/// `()` (dbt-core lets it be frozen via `overrides.macros` anyway). Stubbing
/// it out as a callable like other macro overrides breaks
/// `.strftime()`/`.astimezone()`, so eval its value as Jinja and bind the
/// resulting `Value` directly.
fn bind_run_started_at_override(
    macro_value: &YmlValue,
    compile_context: &BTreeMap<String, Value>,
    env: &JinjaEnv,
) -> Value {
    macro_value
        .as_str()
        .and_then(single_expression_body)
        .and_then(|expr| {
            env.compile_expression(expr)
                .ok()?
                .eval(compile_context.clone(), &[])
                .ok()
        })
        .unwrap_or_else(|| MinijinjaValue::from_serialize(macro_value.clone()))
}

fn bind_override_macros(
    macros: &BTreeMap<String, YmlValue>,
    compile_context: &mut BTreeMap<String, Value>,
    env: &JinjaEnv,
) {
    for (macro_name, macro_value) in macros.iter() {
        if macro_name == "run_started_at" {
            let value = bind_run_started_at_override(macro_value, compile_context, env);
            compile_context.insert(macro_name.clone(), value);
            continue;
        }

        let return_value = MinijinjaValue::from_serialize(macro_value.clone());
        let fn_stub =
            MinijinjaValue::from_function(move |_args: &[MinijinjaValue]| Ok(return_value.clone()));

        // If a package already exists, merge the override in
        if let Some((pkg, attr)) = macro_name.split_once(".") {
            let object_stub = Value::from(BTreeMap::from([(attr, fn_stub)]));
            let new_pkg = if let Some(og) = compile_context.remove(pkg) {
                // Check `object_stub` first
                Value::from_object(ObjectOverlay(vec![object_stub, og]))
            } else {
                object_stub
            };

            compile_context.insert(pkg.to_string(), new_pkg);
        } else {
            compile_context.insert(macro_name.to_string(), fn_stub);
        }
    }
}

/// Apply overrides to the compile context for unit tests
pub fn apply_unit_test_overrides(
    compile_context: &mut BTreeMap<String, MinijinjaValue>,
    overrides: &UnitTestOverrides,
    ctx: &TaskRunnerCtx,
) {
    // Override for Macros
    if let Some(macros) = overrides.macros.as_ref() {
        bind_override_macros(macros, compile_context, &ctx.env);
    }

    // Override for Environment Variables
    if let Some(env_vars) = &overrides.env_vars {
        // The overrides are [serde_json::YmlValue]s, but the overrides_fn uses
        // [MinijinjaValue::from_serialize] to convert them into [minijinja::YmlValue]s.
        let env_var_func_func = {
            let the_overrides = env_vars.clone();
            let overrides_fn =
                move |var: &str| the_overrides.get(var).map(MinijinjaValue::from_serialize);
            move |state: &State, args: &[MinijinjaValue]| {
                env_var(
                    true, // placeholder_on_secret_access
                    Some(&overrides_fn),
                    state,
                    args,
                )
            }
        };
        compile_context.insert(
            "env_var".to_string(),
            MinijinjaValue::from_func_func("env_var", env_var_func_func),
        );
    }

    // Override for Variables
    if let Some(vars) = &overrides.vars {
        let base_vars = ctx.inner.arg.vars.clone();
        let overrides_map = Some(vars.clone());
        compile_context.insert(
            "var".to_string(),
            MinijinjaValue::from_object(Var::with_overrides(base_vars, overrides_map)),
        );
    }
}

fn create_cte_name_from_fqn(
    adapter_type: AdapterType,
    type_ops: &dyn TypeOps,
    fqn: &str,
) -> String {
    let cte_name = fqn.replace(quote_char(adapter_type), "").replace(".", "_");
    type_ops.format_ident(&cte_name)
}

fn replace_subquery_refs_with_cte_names(
    adapter_type: AdapterType,
    type_ops: &dyn TypeOps,
    mut sql: String,
    subqueries: &[(String, String)],
) -> String {
    for (fqn, _) in subqueries
        .iter()
        .sorted_by_key(|(fqn, _)| std::cmp::Reverse(fqn.len()))
    {
        let formatted_cte_name = create_cte_name_from_fqn(adapter_type, type_ops, fqn);

        if fqn.starts_with(quote_char(adapter_type)) || fqn.contains(".") {
            sql = sql.replace(fqn, &formatted_cte_name);
            continue;
        }

        let escaped = regex::escape(fqn);
        let re = Regex::new(&format!(r"({escaped})\b")).expect("Must compile regexp");
        sql = re
            .replace_all(&sql, formatted_cte_name.as_str())
            .to_string();
    }
    sql
}

/// Per-given: (rendered FQN string, relation Arc).
type GivenRelations = Vec<(String, Arc<dyn BaseRelation>)>;
/// Relations needing async schema fetch (cache misses).
type RelationsToFetch = Vec<(Arc<dyn BaseRelation>, UnitTestSchemaTarget)>;

#[derive(Default)]
struct DiscoveredGivenRelations {
    given_relations: GivenRelations,
    relations_to_fetch: RelationsToFetch,
    given_relation_ids: Vec<String>,
}

/// A `RenderingEventListener` that collects unique IDs from resolved
/// source and ref calls
#[derive(Debug, Default)]
struct UnitTestGivenCapture {
    pub uids: RefCell<Vec<String>>,
}

impl UnitTestGivenCapture {
    fn new() -> Rc<UnitTestGivenCapture> {
        Rc::new(UnitTestGivenCapture::default())
    }
}

impl RenderingEventListener for UnitTestGivenCapture {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn name(&self) -> &str {
        "UnitTestGivenCapture"
    }
    fn on_macro_start(&self, _file_path: Option<&Path>, _line: &u32, _col: &u32, _offset: &u32) {}
    fn on_macro_stop(&self, _file_path: Option<&Path>, _line: &u32, _col: &u32, _offset: &u32) {}
    fn on_malicious_return(&self, _location: &CodeLocation) {}
    fn on_function_start(&self) {}
    fn on_function_end(&self) {}

    /// Collect all unique IDs encountered for given source/ref eval
    fn on_ref_or_source_resolved(&self, unique_id: &str) {
        self.uids.borrow_mut().push(unique_id.to_string());
    }
}

fn row_column_names_or_default<'a>(
    rows: &[BTreeMap<String, YmlValue>],
    column_names: Vec<&'a str>,
    expect_schema: &'a SchemaRef,
) -> Vec<&'a str> {
    if rows.is_empty() {
        column_names
    } else {
        let names = rows[0]
            .keys()
            .map(|k| k.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        expect_schema
            .fields
            .iter()
            .filter_map(|f| {
                if names.contains(&f.name().to_ascii_lowercase()) {
                    Some(f.name().as_str())
                } else {
                    None
                }
            })
            .collect()
    }
}

fn extract_expect_values<'a>(
    ctx: &TaskRunnerCtx,
    unit_test: &DbtUnitTest,
    expect_schema: &'a SchemaRef,
) -> FsResult<(String, Vec<&'a str>)> {
    let type_ops_arc = ctx
        .env
        .get_base_adapter()
        .map(|a| a.engine().type_ops().clone())
        .unwrap_or_else(|| {
            Arc::new(DefaultTypeOps::new(unit_test.node_adapter())) as Arc<dyn TypeOps>
        });
    let type_ops = type_ops_arc.as_ref();
    let column_names = Vec::from_iter(
        expect_schema
            .fields()
            .into_iter()
            .map(|field| field.name().as_str()),
    );

    let expect_rows = &unit_test.__unit_test_attr__.expect.rows;
    let expect_fixture = &unit_test.__unit_test_attr__.expect.fixture;
    let expect_format = &unit_test.__unit_test_attr__.expect.format;

    match (expect_format, expect_fixture, expect_rows) {
        // dict
        (schemas::common::Formats::Dict, _, Some(Rows::List(rows))) => Ok((
            create_values(
                expect_schema,
                rows,
                unit_test.node_adapter(),
                type_ops,
                None,
                &unit_test.__unit_test_attr__.model,
                false,
            )?,
            row_column_names_or_default(rows, column_names, expect_schema),
        )),
        (schemas::common::Formats::Dict, _, _) => Err(fs_err!(
            ErrorCode::NotYetSupportedOption,
            "Unit test {} is invalid. Format `dict` only supports inline YAML rows.",
            unit_test.__common_attr__.name
        )),
        // csv (inline string)
        (schemas::common::Formats::Csv, _, Some(Rows::String(csv_str))) => {
            let rows = parse_csv_rows(csv_str.as_bytes()).map_err(|e| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Failed to parse expected value in unit test '{}' (model '{}'). {}",
                    unit_test.__common_attr__.name,
                    unit_test.__unit_test_attr__.model,
                    e
                )
            })?;
            Ok((
                create_values(
                    expect_schema,
                    &rows,
                    unit_test.node_adapter(),
                    type_ops,
                    None,
                    &unit_test.__unit_test_attr__.model,
                    false,
                )?,
                row_column_names_or_default(&rows, column_names, expect_schema),
            ))
        }
        (schemas::common::Formats::Csv, _, Some(Rows::List(_))) => Err(fs_err!(
            ErrorCode::InvalidConfig,
            "Unit test {} is invalid. Format `csv` only supports inline string rows or fixture files",
            unit_test.__common_attr__.name
        )),
        (schemas::common::Formats::Sql, _, Some(Rows::List(_))) => Err(fs_err!(
            ErrorCode::NotYetSupportedOption,
            "Unit test {} is invalid. Format `sql` only supports inline string rows or fixture files",
            unit_test.__common_attr__.name
        )),
        // sql (raw fixture, or inline string)
        (schemas::common::Formats::Sql, _, Some(Rows::String(sql))) => {
            Ok((sql.to_string(), column_names))
        }
        (schemas::common::Formats::Sql, Some(fixture), _) => {
            let filename = ctx.inner.arg.io.in_dir.join(fixture.clone());
            Ok((stdfs::read_to_string(filename)?, column_names))
        }
        // all other raw fixture values
        (_, Some(fixture), _) => {
            let rows = get_fixture_rows(fixture, &unit_test.__unit_test_attr__.expect.format, ctx)?;
            Ok((
                create_values(
                    expect_schema,
                    &rows,
                    unit_test.node_adapter(),
                    type_ops,
                    None,
                    &unit_test.__unit_test_attr__.model,
                    false,
                )?,
                row_column_names_or_default(&rows, column_names, expect_schema),
            ))
        }
        (_, _, _) => Err(fs_err!(
            ErrorCode::InvalidConfig,
            "The unit test {} has no fixture",
            unit_test.__common_attr__.name
        )),
    }
}

/// Step 0 (blocking): Eval Jinja for each `given` input to discover relations.
/// Returns the resolved relations and any that need async schema fetching.
fn discover_given_relations(
    ut: &DbtUnitTest,
    ctx: &mut TaskRunnerCtx,
) -> FsResult<DiscoveredGivenRelations> {
    let mut base_context = ctx.inner.base_context.clone();
    add_task_context(&mut base_context, ut.common(), &ctx.thread_id);

    let (compile_context, _config_map) = ctx.build_compile_node_context(
        ut,
        &base_context,
        DependencyValidationConfig::new_unvalidated(),
    )?;

    let mut given_relations = Vec::new();
    let mut relations_to_fetch = Vec::new();
    let mut listeners = ctx.rendering_listener_factory.create_listeners(
        &ut.__common_attr__.path,
        &dbt_frontend_common::error::CodeLocation::start_of_file(),
    );

    let given_capture = UnitTestGivenCapture::new();
    listeners.push(Rc::clone(&given_capture) as Rc<dyn RenderingEventListener>);

    // ClickHouse runs with static analysis off and the persisted schema cache
    // has no TTL, so a cached schema goes stale when an upstream model is
    // rebuilt with different column types between invocations.
    let force_refetch = ut.node_adapter() == AdapterType::ClickHouse;

    for given in &ut.__unit_test_attr__.given {
        let given_relation = {
            ctx.env
                .compile_expression(&given.input)?
                .eval(compile_context.clone(), &listeners)?
        };
        let given_relation = given_relation
            .as_object()
            .expect("Failed to convert relation to object")
            .downcast_ref::<RelationObject>()
            .expect("Failed to downcast relation to object");

        let fqn_string = given_relation.render_self_as_str();
        let relation = given_relation.inner();

        // SQL-format givens use raw SQL directly — no schema needed.
        // Only check/fetch schemas for Dict and Csv formats.
        if given.format != schemas::common::Formats::Sql {
            let canonical_fqn = relation.get_canonical_fqn()?;
            if force_refetch || !ctx.schema_cache.exists(&canonical_fqn) {
                relations_to_fetch
                    .push((Arc::clone(&relation), UnitTestSchemaTarget::GivenUpstream));
            }
        }

        given_relations.push((fqn_string, relation));
    }

    // Static-analysis-off incremental tests normally hydrate the existing model
    // relation. An `is_incremental: false` override instead uses compiled SQL.
    let model_unique_id = get_unique_id(
        &ut.__unit_test_attr__.model,
        &ut.__common_attr__.package_name,
        ut.__unit_test_attr__
            .version
            .as_ref()
            .map(|v| v.to_string()),
        "model",
    );
    let resolver_state = ctx.resolver_state();
    let use_compiled_schema_for_incremental = use_compiled_schema_for_incremental(ctx, ut)?;
    let should_fetch_expected_schema =
        resolver_state
            .nodes
            .get_node(&model_unique_id)
            .is_some_and(|node| {
                node.materialized() == DbtMaterialization::Incremental
                    && is_static_analysis_off_or_baseline(node.static_analysis().into_inner())
                    && !use_compiled_schema_for_incremental
            });

    if should_fetch_expected_schema
        && let Some(expect_relation) = ctx.try_get_relation_from_node(&model_unique_id)
    {
        let schema_relation =
            check_defer_relation(&model_unique_id, ctx).unwrap_or_else(|| expect_relation.clone());
        let canonical_fqn = schema_relation.get_canonical_fqn()?;
        if force_refetch || !ctx.schema_cache.exists(&canonical_fqn) {
            relations_to_fetch.push((
                schema_relation,
                UnitTestSchemaTarget::ExpectedModel {
                    incremental_expected_path: true,
                },
            ));
        }
    }

    listeners.clear();

    let given_relation_ids = Rc::try_unwrap(given_capture)
        .map(|c| c.uids.into_inner())
        .unwrap_or_else(|rc| rc.uids.borrow().clone());

    Ok(DiscoveredGivenRelations {
        given_relations,
        relations_to_fetch,
        given_relation_ids,
    })
}

/// Step 1 (async): Fetch missing schemas into the cache.
async fn fetch_missing_schemas(
    to_fetch: &[(Arc<dyn BaseRelation>, UnitTestSchemaTarget)],
    unit_test_unique_id: &str,
    ctx: &mut TaskRunnerCtx,
    task_hooks: Arc<dyn RenderTaskHooks>,
) -> FsResult<()> {
    let mut fetched = HashSet::new();
    for (relation, schema_target) in to_fetch {
        fetch_schema_for_unit_test_relation(
            ctx,
            Arc::clone(relation),
            unit_test_unique_id,
            &mut fetched,
            *schema_target,
            task_hooks.clone(),
        )
        .await?;
    }
    Ok(())
}

fn check_defer_relation(
    model_unique_id: &str,
    ctx: &TaskRunnerCtx,
) -> Option<Arc<dyn BaseRelation>> {
    ctx.defer_nodes()
        .and_then(|nodes| nodes.get_node(model_unique_id))
        .and_then(|defer_node| {
            create_relation_from_node(defer_node.node_adapter(), defer_node, None).ok()
        })
        .map(|r| Arc::from(r) as Arc<dyn BaseRelation>)
}

fn render_unit_test(
    node: &DbtUnitTest,
    ctx: &mut TaskRunnerCtx,
    given_relations: DiscoveredGivenRelations,
    task_hooks: &dyn RenderTaskHooks,
) -> FsResult<(SqlInstruction, Arc<DashMap<String, MinijinjaValue>>)> {
    let DiscoveredGivenRelations {
        given_relations,
        given_relation_ids,
        ..
    } = given_relations;
    let mut base_context = ctx.inner.base_context.clone();

    add_task_context(&mut base_context, node.common(), &ctx.thread_id);

    // Compiled path lives at target/compiled/<package>/<dir-of-yaml>/<unit_test_name>.sql.
    let absolute_path_unit_test = node.get_node_path_abs(
        NodePathKind::Compiled,
        ctx.inner.arg.io.in_dir.as_path(),
        ctx.inner.arg.io.out_dir.as_path(),
    );
    let new_macro_span_name = format!("{}.macro_spans.json", node.__common_attr__.name);
    let absolute_path_macro_span = absolute_path_unit_test.with_file_name(&new_macro_span_name);
    let relative_path_unit_test = absolute_path_unit_test
        .strip_prefix(&ctx.inner.arg.io.out_dir)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| absolute_path_unit_test.clone());

    let adapter_type = node.node_adapter();
    let base_adapter = ctx.env.get_base_adapter();
    let type_ops_arc = base_adapter
        .as_ref()
        .map(|a| a.engine().type_ops().clone())
        .unwrap_or_else(|| Arc::new(DefaultTypeOps::new(adapter_type)) as Arc<dyn TypeOps>);
    let type_ops = type_ops_arc.as_ref();
    let replay_active = is_replay_active(ctx);
    let is_mock = base_adapter
        .as_ref()
        .is_some_and(|adapter| adapter.engine().is_mock());
    let effective_execute = effective_unit_test_execute(node, ctx.inner.execute);
    let cache_expected_schema =
        effective_execute == schemas::profiles::Execute::Sidecar && !replay_active && !is_mock;

    let model_unique_id = get_unique_id(
        &node.__unit_test_attr__.model,
        &node.__common_attr__.package_name,
        node.__unit_test_attr__
            .version
            .as_ref()
            .map(|v| v.to_string()),
        "model",
    );

    // The tested model's SQL is rendered as part of the unit test, so any dbt
    // UDFs it calls via `{{ function('name') }}` must be allowed by dependency
    // validation. Those functions are dependencies of the *model*, not of the
    // unit test, and (unlike refs/sources) cannot be mocked, so they are never
    // present in the unit test's own `depends_on` or in `given_relation_ids`.
    // Carry them over from the tested model to avoid a spurious dbt1501 error
    // (dbt-core #15246).
    let tested_model_function_deps: Vec<String> = {
        let resolver_state = ctx.resolver_state();
        resolver_state
            .nodes
            .get_node(&model_unique_id)
            .map(|model| {
                model
                    .base()
                    .depends_on
                    .nodes
                    .iter()
                    .filter(|dep| resolver_state.nodes.functions.contains_key(*dep))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    };

    // Build compile context first so we can use it for rendering given_fqn
    let (mut compile_context, config_map) = ctx.build_compile_node_context(
        node,
        &base_context,
        DependencyValidationConfig::new_for_node(node)
            .validate()
            .allow_dependencies(given_relation_ids.iter())
            .allow_dependencies(tested_model_function_deps),
    )?;

    // Apply overrides to the compile context
    if let Some(overrides) = &node.__unit_test_attr__.overrides {
        apply_unit_test_overrides(&mut compile_context, overrides, ctx);
    }

    let resolver_state = ctx.resolver_state();

    // Build subqueries from pre-discovered relations (Discover phase) and their
    // schemas (now cached from Fetch phase).
    let mut subqueries = Vec::new();
    let mut fixture_shape_subqueries = Vec::new();
    for (given_idx, ((fqn_string, relation), given)) in given_relations
        .iter()
        .zip(&node.__unit_test_attr__.given)
        .enumerate()
    {
        let (given_values, fixture_shape_values) = match given.format {
            // SQL uses the string value verbatim
            schemas::common::Formats::Sql => {
                let sql = if let Some(fixture) = &given.fixture {
                    let filename = ctx.inner.arg.io.in_dir.join(fixture.clone());
                    stdfs::read_to_string(filename)?
                } else if let Some(Rows::String(sql)) = &given.rows {
                    sql.clone()
                } else {
                    return Err(fs_err!(
                        ErrorCode::InvalidConfig,
                        "The unit test {} with sql format has no fixture",
                        node.__common_attr__.name
                    ));
                };
                let fixture_shape = cache_expected_schema.then(|| sql.clone());
                (sql, fixture_shape)
            }
            // dict: inline YAML rows only
            schemas::common::Formats::Dict => {
                let given_schema = strip_pseudocolumns(
                    &get_schema_for_unit_test_relation(
                        ctx,
                        Arc::clone(relation),
                        UnitTestSchemaTarget::GivenUpstream,
                    )?,
                    node.node_adapter(),
                );
                if let Some(Rows::List(rows)) = &given.rows {
                    create_values_and_shape_sql(
                        &given_schema,
                        rows,
                        node.node_adapter(),
                        type_ops,
                        given.input.as_str(),
                        cache_expected_schema,
                    )?
                } else {
                    return Err(fs_err!(
                        ErrorCode::InvalidConfig,
                        "Unit test {} is invalid. Format `dict` only supports inline YAML rows.",
                        node.__common_attr__.name
                    ));
                }
            }
            // csv: inline string or fixture file
            schemas::common::Formats::Csv => {
                let given_schema = strip_pseudocolumns(
                    &get_schema_for_unit_test_relation(
                        ctx,
                        Arc::clone(relation),
                        UnitTestSchemaTarget::GivenUpstream,
                    )?,
                    node.node_adapter(),
                );

                if let Some(Rows::String(csv_str)) = &given.rows {
                    let rows = parse_csv_rows(csv_str.as_bytes()).map_err(|e| {
                        fs_err!(
                            ErrorCode::InvalidConfig,
                            "Failed to parse given value in unit test '{}' (given[{}], {}). {}",
                            node.__common_attr__.name,
                            given_idx + 1,
                            given.input.as_str(),
                            e
                        )
                    })?;
                    create_values_and_shape_sql(
                        &given_schema,
                        &rows,
                        node.node_adapter(),
                        type_ops,
                        given.input.as_str(),
                        cache_expected_schema,
                    )?
                } else if let Some(fixture) = &given.fixture {
                    let rows = get_fixture_rows(fixture, &given.format, ctx)?;
                    create_values_and_shape_sql(
                        &given_schema,
                        &rows,
                        node.node_adapter(),
                        type_ops,
                        given.input.as_str(),
                        cache_expected_schema,
                    )?
                } else {
                    return Err(fs_err!(
                        ErrorCode::InvalidConfig,
                        "Unit test {} is invalid. Format `csv` requires inline string rows or a fixture file.",
                        node.__common_attr__.name
                    ));
                }
            }
        };

        subqueries.push((fqn_string.clone(), given_values));
        if let Some(fixture_shape_values) = fixture_shape_values {
            fixture_shape_subqueries.push((fqn_string.clone(), fixture_shape_values));
        }
    }
    // create a subquery for expect...
    // todo: updating the model with a unique id should be done already in parse?
    let expect_relation = ctx
        .try_get_relation_from_node(&model_unique_id)
        .ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "Unit test '{}' references model '{}' which was not found",
                node.__common_attr__.name,
                model_unique_id
            )
        })?;
    let original_file_path = ctx
        .try_get_model_original_file_path(&model_unique_id)
        .ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "Unit test '{}' references model '{}' but its file path was not found",
                node.__common_attr__.name,
                model_unique_id
            )
        })?;
    let absolute_path = ctx
        .inner
        .arg
        .io
        .map_to_workspace_path(original_file_path, NodeType::Model);

    let raw_sql = stdfs::read_to_string(&absolute_path).map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to read file: {} for fqn: {}",
            e,
            expect_relation.render_self_as_str()
        )
    })?;

    let tested_model = resolver_state.nodes.get_node(&model_unique_id);
    let model_static_analysis_off = tested_model.is_some_and(|node| {
        is_static_analysis_off_or_baseline(node.static_analysis().into_inner())
    });
    let model_materialization = tested_model.map(|node| node.materialized());
    let is_incremental = model_materialization == Some(DbtMaterialization::Incremental);
    let use_compiled_schema_for_incremental = use_compiled_schema_for_incremental(ctx, node)?;

    // Ephemeral models have no registered dataset, so the temp-table fallback in
    // `infer_unit_test_expected_schema` fails with "Dataset not found".
    // Force the self-contained query-schema path instead.
    let is_tested_model_ephemeral = model_materialization == Some(DbtMaterialization::Ephemeral);

    let use_query_schema_fallback = should_use_query_schema_fallback(
        is_tested_model_ephemeral,
        effective_execute,
        replay_active,
    );
    let schema_inference_options = ExpectedSchemaInferenceOptions {
        use_query_schema_fallback,
        try_structural_inference: use_query_schema_fallback,
        cache_result: cache_expected_schema,
    };
    let infer_compiled_schema = || {
        infer_schema_from_compiled_sql(
            ctx,
            node,
            &raw_sql,
            &compile_context,
            original_file_path,
            &subqueries,
            &fixture_shape_subqueries,
            schema_inference_options,
            task_hooks,
        )
    };

    // Keep the existing analyzed/hydrated schema paths unless an explicit
    // override changes an incremental model into its full-refresh SQL branch.
    let expect_schema = match (model_static_analysis_off, is_incremental) {
        (_, true) if use_compiled_schema_for_incremental => infer_compiled_schema()?,
        // (static analysis is off) + (incremental model): use prod relation from defer
        // state if available (dev table may not exist yet during build --defer)
        (true, true) => {
            let schema_relation = check_defer_relation(&model_unique_id, ctx)
                .unwrap_or_else(|| expect_relation.clone());
            get_schema_for_unit_test_relation(
                ctx,
                schema_relation,
                UnitTestSchemaTarget::ExpectedModel {
                    incremental_expected_path: true,
                },
            )?
        }
        // (static analysis is on) + (incremental model): require cached schema.
        (false, true) => {
            let expect_cfqn = expect_relation.get_canonical_fqn().map_err(|e| {
                fs_err!(
                    ErrorCode::Unexpected,
                    "Failed to get canonical FQN for unit test '{}': {}",
                    node.__common_attr__.unique_id,
                    e
                )
            })?;
            ctx.schema_cache
                .get_schema(&expect_cfqn)
                .map(|entry| entry.inner().clone())
                .ok_or_else(|| {
                    fs_err!(
                        ErrorCode::Unexpected,
                        "Missing cached schema for unit test '{}'",
                        node.__common_attr__.unique_id
                    )
                })?
        }
        // (static analysis is off) + (not incremental): infer schema via empty relation.
        // (static analysis is on) + (not incremental): try cache, then infer schema.
        (_, false) => {
            let cached_schema = if !model_static_analysis_off {
                let expect_cfqn = expect_relation.get_canonical_fqn().ok();
                if let Some(cfqn) = expect_cfqn {
                    ctx.schema_cache
                        .get_schema(&cfqn)
                        .map(|entry| entry.inner().clone())
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(schema) = cached_schema {
                schema
            } else {
                infer_compiled_schema()?
            }
        }
    };

    let column_names_to_fields: BTreeMap<_, _> = expect_schema
        .fields()
        .iter()
        .map(|field| (field.name().to_ascii_lowercase(), field.as_ref()))
        .collect();

    let (expect_values, expected_column_names_to_compare) =
        extract_expect_values(ctx, node, &expect_schema)?;

    let model_catalog = expect_relation.get_database()?;
    let model_schema = expect_relation.get_schema()?;
    let model_alias = expect_relation.get_identifier()?;

    let expect_table = format!("{model_alias}_expect");
    let fqn_expect = format_fqn(type_ops, &model_catalog, &model_schema, &expect_table);
    subqueries.push((fqn_expect.clone(), expect_values));

    // create a subquery for the model
    let actual_table = format!("{model_alias}_actual");
    let fqn_actual = format_fqn(type_ops, &model_catalog, &model_schema, &actual_table);

    subqueries.push((fqn_actual.clone(), raw_sql));

    // now build the final query
    let mut query_str = String::new();
    // ... iterate over all subqueries
    let mut subqueries_vec = vec![];
    for (fqn, values) in &subqueries {
        let query = format_unit_test_subquery(adapter_type, fqn, values);
        subqueries_vec.push(query);
    }

    // Create ORDER BY clause for all columns except actual_or_expected
    // Filter to only include orderable columns
    let orderable_columns: Vec<String> = expected_column_names_to_compare
        .iter()
        .filter_map(|col| {
            let field = column_names_to_fields.get(&col.to_ascii_lowercase())?;
            is_orderable_type(adapter_type, field).then(|| type_ops.format_ident(field.name()))
        })
        .collect();
    let order_by_columns = if orderable_columns.is_empty() {
        // If no columns are orderable, don't use ORDER BY
        String::new()
    } else {
        orderable_columns.join(", ")
    };

    let order_by_clause = if order_by_columns.is_empty() {
        String::new()
    } else {
        format!("ORDER BY {}", order_by_columns)
    };

    let expected_column_names_formatted = expected_column_names_to_compare
        .iter()
        .map(
            |col| match column_names_to_fields.get(&col.to_ascii_lowercase()) {
                Some(field) => Ok(type_ops.format_ident(field.name())),
                None => Err(fs_err!(
                    ErrorCode::InvalidConfig,
                    "Column {} could not be found",
                    col
                )),
            },
        )
        .collect::<FsResult<Vec<String>>>()?
        .join(", ");

    // Redshift errors (SQLSTATE XX000) on a top-level UNION ALL immediately
    // followed by ORDER BY. dbt-labs/dbt-core#14549
    query_str.push_str(&format!(
        r#"-- Build actual result given inputs
WITH
            {}
        SELECT * FROM (
        (SELECT {}, 'actual' AS actual_or_expected FROM {})
        UNION ALL
        (SELECT {}, 'expected' AS actual_or_expected FROM {})
        ) unit_test_diff
        {}"#,
        subqueries_vec.join(",\n  "),
        expected_column_names_formatted,
        fqn_actual,
        expected_column_names_formatted,
        fqn_expect,
        order_by_clause
    ));

    // create a subquery for the actual value ...

    let rendered_sql = render_sql(
        &query_str,
        &ctx.env,
        &compile_context,
        ctx.rendering_listener_factory.as_ref(),
        original_file_path,
    )
    .map_err(|e| {
        fs_err!(
            ErrorCode::Generic,
            "Error rendering unit test '{}': Please pay attention to fixture files: {}",
            node.__common_attr__.name,
            e
        )
    })
    .map_err(|e| e.with_location(original_file_path.to_path_buf()))?;
    let macro_spans = ctx
        .rendering_listener_factory
        .drain_macro_spans(original_file_path);

    // todo: use Jinja to render all refs, here we just replace the fqns with the actual values
    let rendered_sql =
        replace_subquery_refs_with_cte_names(adapter_type, type_ops, rendered_sql, &subqueries);

    stdfs::create_dir_all(absolute_path_unit_test.parent().unwrap())?;
    stdfs::write(&absolute_path_unit_test, &rendered_sql)?;

    // todo: the macro spans are off: render the whole query, not just the actual sql model
    let macro_spans = macro_spans_to_macro_span_vec(&macro_spans);
    stdfs::write(
        absolute_path_macro_span,
        serde_json::to_string_pretty(&macro_spans).unwrap(),
    )?;

    Ok((
        SqlInstruction {
            fqn: vec![
                node.__base_attr__.database.clone(),
                node.__base_attr__.schema.clone(),
                node.__common_attr__.name.clone(),
            ],
            sql: rendered_sql,
            original_path: relative_path_unit_test,
            spans: Box::new(MacroSpansOnly { macro_spans }),
        },
        config_map,
    ))
}

fn format_unit_test_subquery(adapter_type: AdapterType, fqn: &str, values: &str) -> String {
    let needs_newline = Tokenizer::new(sqlparser_dialect_for(adapter_type), values)
        .tokenize()
        .is_ok_and(|tokens| {
            matches!(
                tokens.last(),
                Some(Token::Whitespace(Whitespace::SingleLineComment { comment, .. }))
                    if !comment.ends_with('\n') && !comment.ends_with('\r')
            )
        });
    let newline = if needs_newline { "\n" } else { "" };

    format!("\t{fqn} as ({values}{newline})")
}

fn format_fqn(type_ops: &dyn TypeOps, catalog: &str, schema: &str, table: &str) -> String {
    format!(
        "{}.{}.{}",
        type_ops.format_ident(catalog),
        type_ops.format_ident(schema),
        type_ops.format_ident(table),
    )
}

// types supported in ORDER BY clauses
fn is_orderable_type(adapter_type: AdapterType, field: &Field) -> bool {
    let data_type = field.data_type();
    match adapter_type {
        AdapterType::Bigquery => {
            !data_type.is_nested()
                && !bigquery::is_type(field, "JSON")
                && !bigquery::is_type(field, "GEOGRAPHY")
        }
        // TODO(jason): Update this as more complex types are supported
        _ => is_supported_type(adapter_type, data_type),
    }
}

// Check if this is a data type we provide support for
fn is_supported_type(adapter_type: AdapterType, ref_type: &DataType) -> bool {
    ref_type.is_primitive()
        || matches!(
            ref_type,
            DataType::Utf8
                | DataType::Utf8View
                | DataType::LargeUtf8
                | DataType::Binary
                | DataType::LargeBinary
                | DataType::Boolean
                | DataType::Null
        )
        || match adapter_type {
            AdapterType::Snowflake => {
                SnowflakeTyping::is_any_timestamp(ref_type).is_yes()
                    || SnowflakeTyping::is_time(ref_type).is_yes()
                    || SnowflakeTyping::is_semi_structured_array(ref_type)
                    || SnowflakeTyping::is_variant(ref_type)
                    || SnowflakeTyping::is_object(ref_type)
                    || SnowflakeTyping::is_geography(ref_type)
                    || SnowflakeTyping::is_geometry(ref_type)
            }
            AdapterType::Bigquery => {
                // Support Arrays and Structs for BigQuery
                matches!(ref_type, DataType::List(_) | DataType::Struct(_))
            }
            AdapterType::Databricks => DatabricksTyping::is_timestamp_ntz(ref_type),
            AdapterType::DuckDB => {
                // DuckDB distinct types are wrapped as FixedSizeList(Field(name, ..), 1)
                matches!(ref_type, DataType::FixedSizeList(_, 1))
            }
            _ => false,
        }
}

/// Renders a `dbt_yaml::value::Sequence` to a SQL literal expression. All dialects
/// other than Snowflake (which supports heterogeneous arrays) have explicit casts
/// for each element.
fn yml_sequence_to_sql_literal(
    adapter_type: AdapterType,
    type_ops: &dyn TypeOps,
    value: dbt_yaml::value::Sequence,
    parent_data_type: &DataType,
) -> FsResult<String> {
    let (DataType::List(element_field)
    | DataType::LargeList(element_field)
    | DataType::FixedSizeList(element_field, _)) = parent_data_type
    else {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "Array value provided for non-array type: {:?}",
            parent_data_type
        ));
    };
    let element_type_literal = format_fixture_type(type_ops, element_field, true)?;
    let cast_elements = match adapter_type {
        AdapterType::Snowflake => false,
        // BigQuery does not cast between arrays with different element types
        // (`Invalid cast from ARRAY<STRUCT<x STRING>> to ARRAY<STRUCT<x TIMESTAMP>>`),
        // so every element must carry its own cast to the reported type.
        AdapterType::Bigquery => true,
        _ => !element_field.data_type().is_nested(),
    };

    let child_literals = value
        .into_iter()
        .map(|v| {
            let literal = yml_value_to_sql_literal(adapter_type, type_ops, v, element_field)?;

            if cast_elements {
                Ok(format!("CAST({literal} AS {element_type_literal})"))
            } else {
                Ok(literal)
            }
        })
        .collect::<FsResult<Vec<_>>>()?;

    match adapter_type {
        AdapterType::Bigquery if child_literals.is_empty() => {
            Ok(format!("ARRAY<{element_type_literal}>[]"))
        }
        _ => Ok(format!("[{}]", child_literals.join(", "))),
    }
}

/// Renders a `dbt_yaml::value::Mapping` to a SQL literal expression. All dialects
/// other than Snowflake use STRUCT() and each field's concrete type; Snowflake
/// flattens the mapping back into JSON and uses JSON_PARSE
fn yml_mapping_to_sql_literal(
    adapter_type: AdapterType,
    type_ops: &dyn TypeOps,
    sql_literal_formatter: &SqlLiteralFormatter,
    mapping: dbt_yaml::mapping::Mapping,
    parent_data_type: &DataType,
) -> FsResult<String> {
    match adapter_type {
        // We represent structs as a list with an arrow bin type object
        AdapterType::Snowflake => {
            let json_str = serde_json::to_string(&mapping).map_err(|_| {
                fs_err!(
                    ErrorCode::InvalidArgument,
                    "Unable to serialize struct field"
                )
            })?;

            Ok(format!(
                "PARSE_JSON({})",
                sql_literal_formatter.format_str(&json_str)
            ))
        }
        // We have type information for individual struct fields
        _ => {
            let DataType::Struct(fields) = parent_data_type else {
                return Err(fs_err!(
                    ErrorCode::InvalidConfig,
                    "Object value provided for non-struct type: {:?}",
                    parent_data_type
                ));
            };

            let struct_fields: Vec<String> = fields
                .iter()
                .map(|field| {
                    let field_name = field.name();
                    let field_value = mapping
                        .get(field_name)
                        .cloned()
                        .unwrap_or_else(YmlValue::null);
                    let value_literal =
                        yml_value_to_sql_literal(adapter_type, type_ops, field_value, field)?;
                    let name_literal = type_ops.format_ident(field_name);
                    Ok(format!("{value_literal} AS {name_literal}"))
                })
                .collect::<FsResult<Vec<_>>>()?;
            Ok(format!("STRUCT({})", struct_fields.join(", ")))
        }
    }
}

/// Converts a yaml value to a String literal for the given adapter type
fn yml_value_to_sql_literal(
    adapter_type: AdapterType,
    type_ops: &dyn TypeOps,
    value: YmlValue,
    field: &Field,
) -> FsResult<String> {
    let literal_formatter = SqlLiteralFormatter::new(adapter_type);
    let data_type = field.data_type();

    match value {
        YmlValue::Null(_) => Ok(literal_formatter.none_value()),
        value if adapter_type == AdapterType::Bigquery && bigquery::is_type(field, "JSON") => {
            let json_str = match value {
                YmlValue::String(s, _) => s,
                _ => serde_json::to_string(&value).map_err(|_| {
                    fs_err!(
                        ErrorCode::InvalidArgument,
                        "Unable to serialize JSON fixture value"
                    )
                })?,
            };
            // Preserve JSON escapes through BigQuery's string literal parser.
            // https://github.com/dbt-labs/dbt-core/issues/15708
            let json_str = json_str.replace('\\', "\\\\");
            Ok(format!(
                "PARSE_JSON({})",
                literal_formatter.format_str(&json_str)
            ))
        }
        YmlValue::Bool(b, _) => Ok(literal_formatter.format_bool(b)),
        YmlValue::Number(n, _) => Ok(n.to_string()),
        // A string fixture for a type that cannot be produced by casting a
        // string literal (e.g. BigQuery STRUCT/GEOGRAPHY) is a SQL expression
        // that must be injected verbatim. See dbt-labs/dbt-core#14625.
        YmlValue::String(s, _)
            if type_ops.cast_from_quoted_string_literal_unsupported_for(field) =>
        {
            Ok(s)
        }
        YmlValue::String(s, _) => Ok(literal_formatter.format_str(&s)),
        // Mappings/sequences have per-dialect customizations
        YmlValue::Mapping(m, _) => {
            yml_mapping_to_sql_literal(adapter_type, type_ops, &literal_formatter, m, data_type)
        }
        YmlValue::Sequence(s, _) => {
            yml_sequence_to_sql_literal(adapter_type, type_ops, s, data_type)
        }
        _ => err!(
            ErrorCode::InvalidConfig,
            "Unsupported JSON value type: {value:?}"
        ),
    }
}

fn format_fixture_type(type_ops: &dyn TypeOps, field: &Field, nullable: bool) -> FsResult<String> {
    match type_ops.adapter_type() {
        AdapterType::Bigquery => type_ops
            .get_original_sql_type_from_field(field)
            .map(Cow::into_owned),
        _ => {
            let mut formatted = String::new();
            type_ops
                .format_arrow_type_as_sql(field.data_type(), nullable, &mut formatted)
                .map(|()| formatted)
        }
    }
    .map_err(|e| {
        fs_err!(
            ErrorCode::InvalidConfig,
            "Failed to format type {:?}: {}",
            field.data_type(),
            e
        )
    })
}

fn columns_to_formatted_types<'a>(
    ref_schema: &'a SchemaRef,
    type_ops: &dyn TypeOps,
) -> FsResult<Vec<(&'a String, &'a DataType, String)>> {
    ref_schema
        .fields()
        .iter()
        .map(|f| {
            let formatted = format_fixture_type(type_ops, f, f.is_nullable())?;
            Ok((f.name(), f.data_type(), formatted))
        })
        .collect::<FsResult<Vec<_>>>()
}

/// Queryable pseudocolumns per adapter that are not reported by the information
/// schema, and therefore never appear in a relation's fetched schema.
///
/// Pseudocolumns are system-generated columns that can be queried but are absent
/// from `INFORMATION_SCHEMA` (e.g. BigQuery's `_FILE_NAME` on external tables).
/// The BigQuery set is sourced from [`dbt_adapter::metadata::BIGQUERY_PSEUDOCOLUMNS`]
/// so there is a single source of truth. Names are compared case-insensitively.
fn known_pseudocolumns(adapter_type: AdapterType) -> &'static [&'static str] {
    match adapter_type {
        AdapterType::Bigquery => &BIGQUERY_PSEUDOCOLUMNS,
        _ => &[],
    }
}

fn strip_pseudocolumns(ref_schema: &SchemaRef, adapter_type: AdapterType) -> SchemaRef {
    let pseudocolumns = known_pseudocolumns(adapter_type);
    if pseudocolumns.is_empty() {
        return ref_schema.clone();
    }

    let fields: Vec<Field> = ref_schema
        .fields()
        .iter()
        .filter(|f| !pseudocolumns.contains(&f.name().as_str()))
        .map(|f| f.as_ref().clone())
        .collect();

    if fields.len() == ref_schema.fields().len() {
        return ref_schema.clone();
    }

    Arc::new(Schema::new(fields))
}

/// Return `ref_schema` extended with any recognized pseudocolumns (see
/// [`known_pseudocolumns`]) that a fixture row references but that are missing
/// from the fetched schema. This lets unit tests provide values for queryable
/// columns like BigQuery's `_FILE_NAME` without raising a spurious "invalid
/// column name" warning. Pseudocolumns are only appended when actually
/// referenced, so fixtures that don't use them produce identical SQL.
///
/// This is intended for `given`/upstream input relations only — see the
/// `allow_pseudocolumns` argument of [`create_values`].
///
/// Note: the dbt Core v1 (Python `dbt-adapters`) implementation only exposes
/// `_FILE_NAME` for tables it has verified are `EXTERNAL`. Here we accept the
/// pseudocolumn purely on the strength of the fixture referencing it, because
/// Fusion's schema cache carries columns but not table type. A fixture that
/// references `_FILE_NAME` against a non-external table is therefore allowed
/// here and only fails later at the warehouse (with a raw SQL error rather
/// than dbt's friendlier column-name message).
fn append_referenced_pseudocolumns(
    ref_schema: &SchemaRef,
    rows: &[BTreeMap<String, YmlValue>],
    adapter_type: AdapterType,
) -> SchemaRef {
    let pseudocolumns = known_pseudocolumns(adapter_type);
    if pseudocolumns.is_empty() {
        return ref_schema.clone();
    }

    let existing: HashSet<String> = ref_schema
        .fields()
        .iter()
        .map(|f| f.name().to_ascii_lowercase())
        .collect();

    let mut extra_fields = Vec::new();
    for pseudocolumn in pseudocolumns {
        let pseudocolumn_lower = pseudocolumn.to_ascii_lowercase();
        if existing.contains(&pseudocolumn_lower) {
            continue;
        }
        let referenced = rows.iter().any(|row| {
            row.keys()
                .any(|k| k.to_ascii_lowercase() == pseudocolumn_lower)
        });
        if referenced {
            // Pseudocolumns are typed as strings here; the fixture supplies a
            // literal that `create_values` casts to this type. This matches the
            // string-valued pseudocolumns (`_FILE_NAME`, `_TABLE_SUFFIX`, …); the
            // few timestamp/date ones would need a richer mapping if used.
            extra_fields.push(Field::new(*pseudocolumn, DataType::Utf8, true));
        }
    }

    if extra_fields.is_empty() {
        return ref_schema.clone();
    }

    let mut fields: Vec<Field> = ref_schema
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    fields.extend(extra_fields);
    Arc::new(Schema::new(fields))
}

fn create_values_and_shape_sql(
    ref_schema: &SchemaRef,
    rows: &[BTreeMap<String, YmlValue>],
    adapter_type: AdapterType,
    type_ops: &dyn TypeOps,
    relation_name: &str,
    include_shape: bool,
) -> FsResult<(String, Option<String>)> {
    let values_sql = create_values(
        ref_schema,
        rows,
        adapter_type,
        type_ops,
        None,
        relation_name,
        true,
    )?;
    let shape_sql = if include_shape {
        let schema = append_referenced_pseudocolumns(ref_schema, rows, adapter_type);
        // Dict and CSV values are explicitly cast to this schema. A typed empty
        // query therefore preserves their output shape without keying on literals.
        Some(create_values(
            &schema,
            &[],
            adapter_type,
            type_ops,
            None,
            relation_name,
            false,
        )?)
    } else {
        None
    };
    Ok((values_sql, shape_sql))
}

/// Reference: https://github.com/dbt-labs/dbt-adapters/blob/60fc903f705f447a7b58d0df6b33d7be7cd00690/dbt-adapters/src/dbt/include/global_project/macros/unit_test_sql/get_fixture_sql.sql#L51
fn create_values(
    ref_schema: &SchemaRef,
    rows: &[BTreeMap<String, YmlValue>],
    adapter_type: AdapterType,
    type_ops: &dyn TypeOps,
    named_column: Option<(String, YmlValue)>,
    relation_name: &str,
    allow_pseudocolumns: bool,
) -> FsResult<String> {
    // Accept queryable pseudocolumns (e.g. BigQuery's `_FILE_NAME`) that a
    // fixture references but the information schema does not report. This only
    // applies to `given`/input relations; `expect` fixtures describe the
    // model's own output and must never gain synthetic columns.
    let augmented_schema = if allow_pseudocolumns {
        append_referenced_pseudocolumns(ref_schema, rows, adapter_type)
    } else {
        ref_schema.clone()
    };
    let ref_schema = &augmented_schema;

    let columns = columns_to_formatted_types(ref_schema, type_ops)?;

    let mut enriched_rows = vec![];
    for (i, row) in rows.iter().enumerate() {
        let mut input_row = row
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v))
            .collect::<BTreeMap<_, _>>();
        let mut enriched_row = vec![];
        for ref_field in ref_schema.fields() {
            let ref_name = ref_field.name();
            let ref_type = ref_field.data_type();

            // Either a value exists, or we push NULL and move on

            let Some(row_value) = input_row.remove(&ref_name.to_ascii_lowercase()) else {
                enriched_row.push(YmlValue::null());
                continue;
            };

            if !is_supported_type(adapter_type, ref_type) {
                return err!(
                    ErrorCode::InvalidConfig,
                    "The column '{}' has a non-primitive type '{}'. Only primitive types like numeric, temporal and varchar are supported for unit_testing{}",
                    ref_name,
                    ref_type.to_string(),
                    match adapter_type {
                        AdapterType::Bigquery => ", plus ARRAY and STRUCT types for BigQuery",
                        _ => "",
                    }
                );
            }
            // todo: this is a hack to handle null values in a more robust way, but maybe we should only allows this if the column is nullable?
            // Emit a null if the value is:
            // - `NULL` (actually null)
            // - "null" (string)
            // - ""     (empty string, but only if *not* string typed)
            let is_string_typed = matches!(
                ref_type,
                DataType::Utf8
                    | DataType::Utf8View
                    | DataType::LargeUtf8
                    | DataType::Binary
                    | DataType::LargeBinary
            );
            if row_value.is_null()
                || row_value.is_string() && (row_value.as_str().unwrap()).to_lowercase() == "null"
                || row_value.is_string()
                    && row_value.as_str().unwrap().is_empty()
                    && !is_string_typed
            {
                enriched_row.push(YmlValue::null());
            } else {
                enriched_row.push(row_value.clone());
            }
        }

        if !input_row.is_empty() {
            let invalid_columns: Vec<_> = input_row.keys().collect();
            let accepted_columns: Vec<_> = ref_schema
                .fields()
                .iter()
                .map(|f| f.name().as_str())
                .collect();
            // `expect` fixtures never allow pseudocolumns; use that to match dbt
            // Core's wording ("expected output" vs the quoted input relation name).
            let fixture_desc = if allow_pseudocolumns {
                format!("'{relation_name}'")
            } else {
                "expected output".to_string()
            };
            return err!(
                ErrorCode::InvalidConfig,
                "Invalid column name(s): {} in row {} of unit test fixture for {fixture_desc}. Accepted columns for {fixture_desc} are: {:?}",
                invalid_columns.iter().map(|c| format!("'{c}'")).join(", "),
                i + 1,
                accepted_columns
            );
        }

        enriched_rows.push(enriched_row);
    }

    let constant_column = if let Some((column_name, column_value)) = named_column {
        format!(", {column_value:?} AS {column_name}")
    } else {
        "".to_string()
    };

    match adapter_type {
        AdapterType::Bigquery => {
            // For BigQuery, just use the UNNEST clause directly
            // https://github.com/dbt-labs/fs/issues/3964
            let values_clause = create_bigquery_relation_to_select_from(
                type_ops,
                enriched_rows,
                ref_schema,
                &columns,
            )?;
            Ok(format!(
                "SELECT * {constant_column} FROM {{% raw %}}{values_clause}{{% endraw %}}"
            ))
        }
        _ => {
            // Use UNION ALL approach - much cleaner and more efficient!
            let query = create_select_with_union_all(
                adapter_type,
                type_ops,
                enriched_rows,
                ref_schema,
                &columns,
                &constant_column,
            )?;
            Ok(format!("{{% raw %}}{query}{{% endraw %}}"))
        }
    }
}

fn create_select_with_union_all(
    adapter_type: AdapterType,
    type_ops: &dyn TypeOps,
    enriched_rows: Vec<Vec<YmlValue>>,
    ref_schema: &SchemaRef,
    columns: &Vec<(&String, &DataType, String)>,
    constant_column: &str,
) -> FsResult<String> {
    if enriched_rows.is_empty() {
        // Handle empty case with NULL values
        let null_casts: Vec<String> = columns
            .iter()
            .map(|(name, df_type, formatted_type)| {
                let formatted_name = type_ops.format_ident(name);
                let ty = match formatted_type.as_str() {
                    "null" => "varchar",
                    _ => formatted_type,
                };

                match adapter_type {
                    AdapterType::Snowflake if SnowflakeTyping::is_geography(df_type) => {
                        Ok(format!("TO_GEOGRAPHY(NULL) AS {formatted_name}"))
                    }
                    AdapterType::Snowflake if SnowflakeTyping::is_geometry(df_type) => {
                        Ok(format!("TO_GEOMETRY(NULL) AS {formatted_name}"))
                    }
                    _ => Ok(format!("CAST(NULL AS {ty}) AS {formatted_name}")),
                }
            })
            .collect::<FsResult<Vec<_>>>()?;
        // Use WHERE FALSE to ensure 0 rows are returned while preserving the schema
        return Ok(format!(
            "SELECT {}{} WHERE FALSE",
            null_casts.join(", "),
            constant_column
        ));
    }

    let select_statements: Vec<String> = enriched_rows
        .into_iter()
        .map(|row| {
            let cast_expressions: Vec<String> = row
                .iter()
                .zip(ref_schema.fields())
                .enumerate()
                .map(|(i, (value, field))| {
                    let sql_literal = yml_value_to_sql_literal(adapter_type, type_ops, value.clone(), field)?;
                    let can_cast = type_ops
                        .can_cast_literal_to_type(&sql_literal, field.data_type())
                        .map_err(|e| fs_err!(
                            ErrorCode::InvalidConfig,
                            "The column '{}' has a literal '{}' that is not compatible with the type '{}' of the reference table: {}",
                            field.name(),
                            sql_literal,
                            field.data_type(),
                            e,
                        ))?;
                    if !can_cast {
                        return err!(
                            ErrorCode::InvalidConfig,
                            "The column '{}' has a literal '{}' that is not compatible with the type '{}' of the reference table",
                            field.name(),
                            sql_literal,
                            field.data_type().to_string()
                        );
                    }
                    let (name, df_type, ty) = &columns[i];
                    let formatted_name = type_ops.format_ident(name);
                    let ty = match ty.as_str() {
                        "null" => "varchar",
                        _ => ty,
                    };
                    // if a cast is to a type of the form NUMBER(x,0), change it to just NUMBER
                    let ty = if ty.starts_with("NUMBER(") && ty.ends_with(",0)") {
                        "NUMBER"
                    } else {
                        ty
                    };

                    let safe_cast_literal = match adapter_type {
                        AdapterType::Snowflake => match value.as_bool() {
                            Some(value) => SqlLiteralFormatter::new(adapter_type)
                                .format_str(if value { "True" } else { "False" }),
                            None if value.is_number() => {
                                SqlLiteralFormatter::new(adapter_type).format_str(&sql_literal)
                            }
                            None => sql_literal.clone(),
                        },
                        _ => sql_literal.clone(),
                    };

                    match adapter_type {
                        AdapterType::Snowflake if SnowflakeTyping::is_geography(df_type) =>
                            Ok(format!("TO_GEOGRAPHY({sql_literal}) AS {formatted_name}")),
                        AdapterType::Snowflake if SnowflakeTyping::is_geometry(df_type) =>
                            Ok(format!("TO_GEOMETRY({sql_literal}) AS {formatted_name}")),
                        AdapterType::Snowflake
                            if !SnowflakeTyping::is_variant(df_type)
                                && !SnowflakeTyping::is_object(df_type)
                                && !SnowflakeTyping::is_semi_structured_array(df_type) =>
                            Ok(format!("TRY_CAST({safe_cast_literal} AS {ty}) AS {formatted_name}")),
                        _ => Ok(format!("CAST({sql_literal} AS {ty}) AS {formatted_name}"))
                    }
                })
                .collect::<FsResult<Vec<_>>>()?;
            Ok(format!(
                "SELECT {}{}",
                cast_expressions.join(", "),
                constant_column
            ))
        })
        .collect::<FsResult<Vec<_>>>()?;

    Ok(select_statements.join("\nUNION ALL\n"))
}

fn create_bigquery_relation_to_select_from(
    type_ops: &dyn TypeOps,
    enriched_rows: Vec<Vec<YmlValue>>,
    schema: &SchemaRef,
    columns: &Vec<(&String, &DataType, String)>,
) -> FsResult<String> {
    let struct_values: Vec<String> = enriched_rows
        .into_iter()
        .map(|row| {
            let struct_fields: Vec<String> = row
                .into_iter()
                .zip(schema.fields())
                .zip(columns)
                .map(|((value, field), (_, _, bigquery_type))| {
                    let formatted_name = type_ops.format_ident(&field.name().to_lowercase());
                    let formatted_value =
                        yml_value_to_sql_literal(AdapterType::Bigquery, type_ops, value, field)?;
                    Ok(format!(
                        "CAST({formatted_value} AS {bigquery_type}) AS {formatted_name}"
                    ))
                })
                .collect::<FsResult<Vec<_>>>()?;
            Ok(format!("STRUCT({})", struct_fields.join(", ")))
        })
        .collect::<FsResult<Vec<_>>>()?;

    if struct_values.is_empty() {
        // Generate a typed empty array that returns 0 rows with correct schema
        // e.g., UNNEST(ARRAY<STRUCT<id INT64, name STRING>>[])
        let struct_fields: Vec<String> = columns
            .iter()
            .map(|(name, _, ty)| {
                let formatted_name = type_ops.format_ident(name);
                format!("{formatted_name} {ty}")
            })
            .collect();
        Ok(format!(
            "UNNEST(ARRAY<STRUCT<{}>>[])",
            struct_fields.join(", ")
        ))
    } else {
        Ok(format!("UNNEST([{}])", struct_values.join(", ")))
    }
}

/// generate the unique id for a model (can be made more extensible for each type of node)
/// Copied from parse.rs
fn get_unique_id(
    model_name: &str,
    package_name: &str,
    version: Option<String>,
    node_type: &str,
) -> String {
    if let Some(version) = version {
        format!("{node_type}.{package_name}.{model_name}.v{version}")
    } else {
        format!("{node_type}.{package_name}.{model_name}")
    }
}

fn parse_csv_rows(data: &[u8]) -> FsResult<Vec<BTreeMap<String, YmlValue>>> {
    let mut reader = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(Cursor::new(data));

    let headers = reader
        .headers()
        .map_err(|e| fs_err!(ErrorCode::InvalidConfig, "Failed to read headers: {}", e))?
        .clone();
    let mut rows = Vec::new();

    for result in reader.records() {
        let record = result
            .map_err(|e| fs_err!(ErrorCode::InvalidConfig, "Failed to read record: {}", e))?;
        if record.len() > headers.len() {
            let error = record.position().map_or_else(
                || {
                    format!(
                        "CSV error: found record with {} fields, but the previous record has {} fields",
                        record.len(),
                        headers.len()
                    )
                },
                |position| {
                    format!(
                        "CSV error: record {} (line: {}, byte: {}): found record with {} fields, but the previous record has {} fields",
                        position.record(),
                        position.line(),
                        position.byte(),
                        record.len(),
                        headers.len()
                    )
                },
            );
            return err!(ErrorCode::InvalidConfig, "Failed to read record: {}", error);
        }
        let mut row = BTreeMap::new();
        for (i, header) in headers.iter().enumerate() {
            let value = match record.get(i) {
                None => YmlValue::null(),
                Some(field) => YmlValue::string(field.to_string()),
            };
            row.insert(header.to_string(), value);
        }
        rows.push(row);
    }
    Ok(rows)
}

fn get_fixture_rows(
    fixture: &str,
    format: &schemas::common::Formats,
    ctx: &TaskRunnerCtx,
) -> FsResult<Vec<BTreeMap<String, YmlValue>>> {
    match format {
        schemas::common::Formats::Dict => Err(fs_err!(
            ErrorCode::NotYetSupportedOption,
            "The unit test {} has a dict format, which is not supported yet",
            fixture
        )),
        schemas::common::Formats::Csv => {
            let fixture_path = ctx.inner.arg.io.in_dir.join(fixture);
            let file = stdfs::read(&fixture_path)?;
            parse_csv_rows(&file).map_err(|e| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Failed to parse fixture '{}'. {}",
                    fixture_path.display(),
                    e
                )
            })
        }
        schemas::common::Formats::Sql => Err(fs_err!(
            ErrorCode::NotYetSupportedOption,
            "The unit test {} has a sql format, which is not supported yet",
            fixture
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderable::unit_test_typing::BigqueryTyping;
    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
    use dbt_common::io_args::ComputeArg;
    use dbt_schemas::schemas::profiles::Execute;
    use dbt_test_primitives::assert_contains;

    type YmlValue = dbt_yaml::Value;

    #[test]
    fn configured_local_unit_test_uses_query_schema_fallback() {
        let mut unit_test = DbtUnitTest::default();
        unit_test.deprecated_config.compute = Some(ComputeArg::Sidecar);
        let effective_execute = effective_unit_test_execute(&unit_test, Execute::Remote);

        assert!(should_use_query_schema_fallback(
            false,
            effective_execute,
            false
        ));
    }

    #[test]
    fn test_parse_csv_rows_fills_missing_trailing_fields_with_null() {
        let rows = parse_csv_rows(b"id,name,note\n1,alpha\n")
            .expect("a short CSV record should match Python DictReader semantics");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("note"), Some(&YmlValue::null()));
        assert_eq!(rows[0].len(), 3);
    }

    #[test]
    fn test_parse_csv_rows_preserves_scalar_text() {
        let rows = parse_csv_rows(b"id,code,enabled,ratio,empty\n1,000001,true,1.5,\n")
            .expect("present CSV cells should preserve DictReader string semantics");

        assert_eq!(rows.len(), 1);
        for (column, expected) in [
            ("id", "1"),
            ("code", "000001"),
            ("enabled", "true"),
            ("ratio", "1.5"),
            ("empty", ""),
        ] {
            assert_eq!(
                rows[0].get(column),
                Some(&YmlValue::string(expected.to_string()))
            );
        }
    }

    #[test]
    fn test_parse_csv_rows_rejects_fields_beyond_the_header() {
        let error = parse_csv_rows(b"id,name\n1,alpha,extra\n")
            .expect_err("an extra CSV field must not be silently discarded");

        assert_contains!(
            error.to_string(),
            "found record with 3 fields, but the previous record has 2 fields"
        );
    }

    /// Binds `value` as the `run_started_at` override and renders `template`,
    /// exercising the same path unit tests use.
    fn render_run_started_at_override(value: YmlValue, template: &str) -> String {
        let mut raw_env = minijinja::Environment::new();
        minijinja_contrib::add_to_environment(&mut raw_env);
        let env = JinjaEnv::new(raw_env);

        let mut macros = BTreeMap::new();
        macros.insert("run_started_at".to_string(), value);

        let mut compile_context: BTreeMap<String, Value> = BTreeMap::new();
        bind_override_macros(&macros, &mut compile_context, &env);

        env.render_str(template, &compile_context, &[])
            .expect("bound run_started_at override should render")
    }

    /// Regression test for https://github.com/dbt-labs/dbt-fusion/issues/12271:
    /// dbt-core lets `overrides.macros` freeze `run_started_at`, but unlike a
    /// real macro override it's accessed without `()`, so wrapping it as a
    /// callable stub broke `.strftime()`/`.astimezone()`.
    #[test]
    fn test_run_started_at_override_binds_a_real_value_not_a_callable_stub() {
        let rendered = render_run_started_at_override(
            YmlValue::string(
                "{{ modules.datetime.datetime.strptime('2025-09-04 20:00:00', '%Y-%m-%d %H:%M:%S') }}"
                    .to_string(),
            ),
            "{{ run_started_at.strftime('%Y-%m-%d') }}",
        );
        assert_eq!(rendered, "2025-09-04");
    }

    // Unhappy paths: a non-datetime override must still bind as a plain value,
    // never a callable stub, so bare interpolation renders it. `.strftime()` on
    // these would error the same as it does in dbt-core.
    #[test]
    fn test_run_started_at_override_plain_string_binds_as_value() {
        let rendered = render_run_started_at_override(
            YmlValue::string("2025-09-04 20:00:00".to_string()),
            "{{ run_started_at }}",
        );
        assert_eq!(rendered, "2025-09-04 20:00:00");
    }

    #[test]
    fn test_run_started_at_override_non_datetime_expression_evaluates() {
        let rendered = render_run_started_at_override(
            YmlValue::string("{{ 'hello' }}".to_string()),
            "{{ run_started_at }}",
        );
        assert_eq!(rendered, "hello");
    }

    #[test]
    fn test_run_started_at_override_non_string_binds_as_value() {
        let rendered =
            render_run_started_at_override(YmlValue::number(12345.into()), "{{ run_started_at }}");
        assert_eq!(rendered, "12345");
    }

    #[test]
    fn test_create_cte_name_from_fqn_with_bigquery_backticks() {
        let adapter_type = AdapterType::Bigquery;
        let fqn = "`project.dataset.table`";
        let result =
            create_cte_name_from_fqn(adapter_type, &DefaultTypeOps::new(adapter_type), fqn);
        assert_eq!(result, "project_dataset_table");
    }

    #[test]
    fn test_create_cte_name_from_fqn_with_bigquery_quoted_fqn() {
        let adapter_type = AdapterType::Bigquery;
        let fqn = "`my-gcp-project`.`my_schema`.`na_unit_test__my_model_expect`";
        let result =
            create_cte_name_from_fqn(adapter_type, &DefaultTypeOps::new(adapter_type), fqn);
        assert_eq!(
            result,
            "`my-gcp-project_my_schema_na_unit_test__my_model_expect`"
        );
    }

    #[test]
    fn test_create_cte_name_from_fqn_with_snowflake_quotes() {
        let adapter_type = AdapterType::Snowflake;
        let fqn = "database.schema.table";
        let result =
            create_cte_name_from_fqn(adapter_type, &DefaultTypeOps::new(adapter_type), fqn);
        assert_eq!(result, "\"database_schema_table\"");
    }

    #[test]
    fn test_unit_test_subquery_closes_after_trailing_line_comment() {
        let result = format_unit_test_subquery(
            AdapterType::Snowflake,
            "\"database_schema_model_actual\"",
            "SELECT 1\n-- trailing comment",
        );

        assert_eq!(
            result,
            "\t\"database_schema_model_actual\" as (SELECT 1\n-- trailing comment\n)"
        );

        let sql = format!("WITH {result} SELECT * FROM \"database_schema_model_actual\"");
        sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::SnowflakeDialect {}, &sql)
            .expect("unit test CTE should remain valid after a trailing line comment");
    }

    #[test]
    fn test_unit_test_subquery_preserves_existing_formatting() {
        for values in [
            "SELECT 1",
            "SELECT '-- not a comment'",
            "SELECT 1\n-- terminated comment\n",
        ] {
            assert_eq!(
                format_unit_test_subquery(
                    AdapterType::Snowflake,
                    "\"database_schema_model_actual\"",
                    values,
                ),
                format!("\t\"database_schema_model_actual\" as ({values})")
            );
        }
    }

    #[test]
    fn test_snowflake_time_is_supported_type() {
        let time_type = DataType::FixedSizeList(
            Arc::new(Field::new(
                "time:9",
                DataType::Time64(TimeUnit::Microsecond),
                true,
            )),
            1,
        );

        assert!(is_supported_type(AdapterType::Snowflake, &time_type));
    }

    #[test]
    fn test_create_bigquery_relation_with_simple_data() {
        let fields = vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("active", DataType::Boolean, false),
        ];
        let schema = Arc::new(Schema::new(fields));
        let id = "id".to_string();
        let name = "name".to_string();
        let active = "active".to_string();
        let columns = vec![
            (&id, &DataType::Int64, "INT64".to_string()),
            (&name, &DataType::Utf8, "STRING".to_string()),
            (&active, &DataType::Boolean, "BOOL".to_string()),
        ];
        let enriched_rows = vec![
            vec![
                YmlValue::number(1.into()),
                YmlValue::string("Alice".to_string()),
                YmlValue::bool(true),
            ],
            vec![
                YmlValue::number(2.into()),
                YmlValue::string("Bob".to_string()),
                YmlValue::bool(false),
            ],
        ];
        let result = create_bigquery_relation_to_select_from(
            &DefaultTypeOps::new(AdapterType::Bigquery),
            enriched_rows,
            &schema,
            &columns,
        )
        .unwrap();
        // Should generate UNNEST with STRUCT values
        assert_contains!(result, "UNNEST");
        assert_contains!(result, "STRUCT");
        assert_contains!(result, "CAST(1 AS INT64) AS id");
        assert_contains!(result, "CAST('Alice' AS STRING) AS name");
        assert_contains!(result, "CAST(true AS BOOL) AS active");
    }

    #[test]
    fn test_create_bigquery_relation_with_nulls() {
        let fields = vec![
            Field::new("id", DataType::Int64, true),
            Field::new("description", DataType::Utf8, true),
        ];
        let schema = Arc::new(Schema::new(fields));
        let id = "id".to_string();
        let description = "description".to_string();
        let columns = vec![
            (&id, &DataType::Int64, "INT64".to_string()),
            (&description, &DataType::Utf8, "STRING".to_string()),
        ];
        let enriched_rows = vec![
            vec![YmlValue::number(1.into()), YmlValue::null()],
            vec![YmlValue::null(), YmlValue::string("Test".to_string())],
        ];
        let result = create_bigquery_relation_to_select_from(
            &DefaultTypeOps::new(AdapterType::Bigquery),
            enriched_rows,
            &schema,
            &columns,
        )
        .unwrap();
        assert_contains!(result, "UNNEST");
        assert_contains!(result, "CAST(NULL AS INT64) AS id");
        assert_contains!(result, "CAST('Test' AS STRING) AS description");
    }

    #[test]
    fn test_create_bigquery_relation_empty_rows() {
        let fields = vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ];
        let schema = Arc::new(Schema::new(fields));
        let id = "id".to_string();
        let name = "name".to_string();
        let columns = vec![
            (&id, &DataType::Int64, "INT64".to_string()),
            (&name, &DataType::Utf8, "STRING".to_string()),
        ];
        let enriched_rows = vec![];
        let result = create_bigquery_relation_to_select_from(
            &DefaultTypeOps::new(AdapterType::Bigquery),
            enriched_rows,
            &schema,
            &columns,
        )
        .unwrap();
        // Should generate typed empty array: UNNEST(ARRAY<STRUCT<id INT64, name STRING>>[])
        assert_contains!(result, "UNNEST(ARRAY<STRUCT<");
        assert_contains!(result, "id INT64");
        assert_contains!(result, "name STRING");
        assert_contains!(result, ">>[]");
    }

    #[test]
    fn test_create_bigquery_relation_with_string_escaping() {
        let fields = vec![Field::new("message", DataType::Utf8, false)];
        let schema = Arc::new(Schema::new(fields));
        let message = "message".to_string();
        let columns = vec![(&message, &DataType::Utf8, "STRING".to_string())];
        let enriched_rows = vec![vec![YmlValue::string("Hello 'World'".to_string())]];
        let result = create_bigquery_relation_to_select_from(
            &DefaultTypeOps::new(AdapterType::Bigquery),
            enriched_rows,
            &schema,
            &columns,
        )
        .unwrap();
        // Should properly escape single quotes for BigQuery
        assert!(result.contains(r"'Hello \'World\''"));
    }

    #[test]
    fn test_create_bigquery_relation_with_different_types() {
        let fields = vec![
            Field::new("date_col", DataType::Date32, false),
            Field::new(
                "timestamp_col",
                DataType::Timestamp(TimeUnit::Nanosecond, None),
                false,
            ),
            Field::new("float_col", DataType::Float64, false),
        ];
        let schema = Arc::new(Schema::new(fields));
        let date_col = "date_col".to_string();
        let timestamp_col = "timestamp_col".to_string();
        let float_col = "float_col".to_string();
        let columns = vec![
            (&date_col, &DataType::Date32, "DATE".to_string()),
            (
                &timestamp_col,
                &DataType::Timestamp(TimeUnit::Nanosecond, None),
                "DATETIME".to_string(),
            ),
            (&float_col, &DataType::Float64, "FLOAT64".to_string()),
        ];
        let enriched_rows = vec![vec![
            YmlValue::string("2023-01-01".to_string()),
            YmlValue::string("2023-01-01 12:00:00".to_string()),
            YmlValue::number(3.15.into()),
        ]];
        let result = create_bigquery_relation_to_select_from(
            &DefaultTypeOps::new(AdapterType::Bigquery),
            enriched_rows,
            &schema,
            &columns,
        )
        .unwrap();
        // Should use appropriate BigQuery types
        assert_contains!(result, "CAST('2023-01-01' AS DATE) AS date_col");
        assert_contains!(
            result,
            "CAST('2023-01-01 12:00:00' AS DATETIME) AS timestamp_col"
        );
        assert_contains!(result, "CAST(3.15 AS FLOAT64) AS float_col");
    }

    #[test]
    fn test_create_select_with_union_all() {
        // Test single row case
        let id = "id".to_string();
        let name = "name".to_string();
        let active = "active".to_string();
        let columns = vec![
            (&id, &DataType::Int64, "BIGINT".to_string()),
            (&name, &DataType::Utf8, "VARCHAR".to_string()),
            (&active, &DataType::Boolean, "BOOLEAN".to_string()),
        ];
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("active", DataType::Boolean, false),
        ]));

        let single_row = vec![vec![
            YmlValue::number(1.into()),
            YmlValue::string("Alice".to_string()),
            YmlValue::bool(true),
        ]];

        let result = create_select_with_union_all(
            AdapterType::Bigquery,
            &DefaultTypeOps::new(AdapterType::Bigquery),
            single_row,
            &schema,
            &columns,
            "",
        )
        .unwrap();

        // Should generate: SELECT CAST(1 AS BIGINT) AS id, CAST('Alice' AS VARCHAR) AS name, CAST(true AS BOOLEAN) AS active
        assert!(result.starts_with("SELECT"));
        assert_contains!(result, "CAST(1 AS BIGINT) AS id");
        assert_contains!(result, "CAST('Alice' AS VARCHAR) AS name");
        assert_contains!(result, "CAST(true AS BOOLEAN) AS active");
        assert!(!result.contains("UNION ALL")); // Single row shouldn't have UNION ALL
        assert!(!result.contains("VALUES")); // No VALUES clause

        // Test multiple rows case
        let multiple_rows = vec![
            vec![
                YmlValue::number(1.into()),
                YmlValue::string("Alice".to_string()),
                YmlValue::bool(true),
            ],
            vec![
                YmlValue::number(2.into()),
                YmlValue::string("Bob".to_string()),
                YmlValue::bool(false),
            ],
            vec![
                YmlValue::number(3.into()),
                YmlValue::string("Charlie".to_string()),
                YmlValue::bool(true),
            ],
        ];

        let result = create_select_with_union_all(
            AdapterType::Bigquery,
            &DefaultTypeOps::new(AdapterType::Bigquery),
            multiple_rows,
            &schema,
            &columns,
            "",
        )
        .unwrap();

        // Should generate UNION ALL structure
        assert_contains!(result, "UNION ALL");
        assert!(!result.contains("VALUES")); // No VALUES clause

        // Should have three SELECT statements
        let select_count = result.matches("SELECT").count();
        assert_eq!(select_count, 3);

        // Should have two UNION ALL clauses (between 3 SELECT statements)
        let union_count = result.matches("UNION ALL").count();
        assert_eq!(union_count, 2);

        // Verify each row's data is present
        assert_contains!(result, "CAST(1 AS BIGINT) AS id");
        assert_contains!(result, "CAST('Alice' AS VARCHAR) AS name");
        assert_contains!(result, "CAST(2 AS BIGINT) AS id");
        assert_contains!(result, "CAST('Bob' AS VARCHAR) AS name");
        assert_contains!(result, "CAST(3 AS BIGINT) AS id");
        assert_contains!(result, "CAST('Charlie' AS VARCHAR) AS name");

        // Verify structure: each SELECT should be on its own line with UNION ALL between
        let lines: Vec<&str> = result.split('\n').collect();
        assert!(lines[0].starts_with("SELECT"));
        assert_eq!(lines[1], "UNION ALL");
        assert!(lines[2].starts_with("SELECT"));
        assert_eq!(lines[3], "UNION ALL");
        assert!(lines[4].starts_with("SELECT"));
    }

    #[test]
    fn test_create_select_with_union_all_empty_rows() {
        // Test empty rows case
        let id = "id".to_string();
        let name = "name".to_string();
        let columns = vec![
            (&id, &DataType::Int64, "BIGINT".to_string()),
            (&name, &DataType::Utf8, "VARCHAR".to_string()),
        ];
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));

        let empty_rows = vec![];

        let result = create_select_with_union_all(
            AdapterType::Bigquery,
            &DefaultTypeOps::new(AdapterType::Bigquery),
            empty_rows,
            &schema,
            &columns,
            "",
        )
        .unwrap();

        // Should generate: SELECT CAST(NULL AS BIGINT) AS id, CAST(NULL AS VARCHAR) AS name WHERE FALSE
        assert!(result.starts_with("SELECT"));
        assert_contains!(result, "CAST(NULL AS BIGINT) AS id");
        assert_contains!(result, "CAST(NULL AS VARCHAR) AS name");
        assert_contains!(result, "WHERE FALSE"); // Empty case returns 0 rows
        assert!(!result.contains("UNION ALL")); // Empty case shouldn't have UNION ALL
        assert!(!result.contains("VALUES")); // No VALUES clause
    }

    #[test]
    fn test_create_select_with_union_all_with_constant_column() {
        // Test with constant column (used for additional metadata)
        let id = "id".to_string();
        let columns = vec![(&id, &DataType::Int64, "BIGINT".to_string())];
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));

        let rows = vec![
            vec![YmlValue::number(1.into())],
            vec![YmlValue::number(2.into())],
        ];

        let result = create_select_with_union_all(
            AdapterType::Bigquery,
            &DefaultTypeOps::new(AdapterType::Bigquery),
            rows,
            &schema,
            &columns,
            ", 'test' AS source",
        )
        .unwrap();

        // Should include the constant column in each SELECT
        assert_contains!(result, "CAST(1 AS BIGINT) AS id, 'test' AS source");
        assert_contains!(result, "CAST(2 AS BIGINT) AS id, 'test' AS source");
        assert_contains!(result, "UNION ALL");
    }

    #[test]
    fn test_create_select_with_union_all_null_handling() {
        // Test proper null handling in the data
        let id = "id".to_string();
        let name = "name".to_string();
        let columns = vec![
            (&id, &DataType::Int64, "BIGINT".to_string()),
            (&name, &DataType::Utf8, "VARCHAR".to_string()),
        ];
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let rows = vec![
            vec![YmlValue::number(1.into()), YmlValue::null()],
            vec![YmlValue::null(), YmlValue::string("Bob".to_string())],
        ];

        let result = create_select_with_union_all(
            AdapterType::Bigquery,
            &DefaultTypeOps::new(AdapterType::Bigquery),
            rows,
            &schema,
            &columns,
            "",
        )
        .unwrap();

        // Should properly handle NULL values in the data
        assert_contains!(
            result,
            "CAST(1 AS BIGINT) AS id, CAST(NULL AS VARCHAR) AS name"
        );
        assert_contains!(
            result,
            "CAST(NULL AS BIGINT) AS id, CAST('Bob' AS VARCHAR) AS name"
        );
        assert_contains!(result, "UNION ALL");
    }

    #[test]
    fn test_create_select_with_union_all_null_type_handling() {
        // Test handling of "null" type (should be converted to varchar)
        let id = "id".to_string();
        let nullable_col = "nullable_col".to_string();
        let columns = vec![
            (&id, &DataType::Int64, "BIGINT".to_string()),
            (&nullable_col, &DataType::Utf8, "null".to_string()), // This should be converted to varchar
        ];
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("nullable_col", DataType::Utf8, true),
        ]));
        let rows = vec![vec![
            YmlValue::number(1.into()),
            YmlValue::string("test".to_string()),
        ]];

        let result = create_select_with_union_all(
            AdapterType::Bigquery,
            &DefaultTypeOps::new(AdapterType::Bigquery),
            rows,
            &schema,
            &columns,
            "",
        )
        .unwrap();

        // "null" type should be converted to "varchar"
        assert_contains!(result, "CAST('test' AS varchar) AS nullable_col");
        assert!(!result.contains("CAST('test' AS null)"));
    }

    #[test]
    fn test_create_select_with_union_all_number_type_normalization() {
        // Test NUMBER(x,0) type normalization (should be converted to just NUMBER)
        let id = "id".to_string();
        let amount = "amount".to_string();
        let columns = vec![
            (&id, &DataType::Int64, "BIGINT".to_string()),
            (&amount, &DataType::Int64, "NUMBER(10,0)".to_string()), // This should be normalized to NUMBER
        ];

        let rows = vec![vec![
            YmlValue::number(1.into()),
            YmlValue::number(100.into()),
        ]];

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("amount", DataType::Int64, false),
        ]));

        let result = create_select_with_union_all(
            AdapterType::Bigquery,
            &DefaultTypeOps::new(AdapterType::Bigquery),
            rows,
            &schema,
            &columns,
            "",
        )
        .unwrap();

        // NUMBER(10,0) should be normalized to NUMBER
        assert_contains!(result, "CAST(100 AS NUMBER) AS amount");
        assert!(!result.contains("CAST(100 AS NUMBER(10,0)) AS amount"));

        // Other types should remain unchanged
        assert_contains!(result, "CAST(1 AS BIGINT) AS id");
    }

    #[test]
    fn test_create_select_with_union_all_number_type_variants() {
        // Test various NUMBER type variants to ensure only NUMBER(x,0) is normalized
        let id = "id".to_string();
        let amount1 = "amount1".to_string();
        let amount2 = "amount2".to_string();
        let amount3 = "amount3".to_string();
        let columns = vec![
            (&id, &DataType::Int64, "BIGINT".to_string()),
            (&amount1, &DataType::Int64, "NUMBER(10,0)".to_string()), // Should normalize to NUMBER
            (&amount2, &DataType::Int64, "NUMBER(10,2)".to_string()), // Should remain NUMBER(10,2)
            (&amount3, &DataType::Int64, "NUMBER".to_string()),       // Should remain NUMBER
        ];

        let rows = vec![vec![
            YmlValue::number(1.into()),
            YmlValue::number(100.into()),
            YmlValue::number(99.99.into()),
            YmlValue::number(50.into()),
        ]];

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("amount1", DataType::Int64, false),
            Field::new("amount2", DataType::Int64, false),
            Field::new("amount3", DataType::Int64, false),
        ]));

        let result = create_select_with_union_all(
            AdapterType::Bigquery,
            &DefaultTypeOps::new(AdapterType::Bigquery),
            rows,
            &schema,
            &columns,
            "",
        )
        .unwrap();

        // Verify normalization behavior
        assert_contains!(result, "CAST(100 AS NUMBER) AS amount1"); // Normalized
        assert_contains!(result, "CAST(99.99 AS NUMBER(10,2)) AS amount2"); // Not normalized
        assert_contains!(result, "CAST(50 AS NUMBER) AS amount3"); // Already NUMBER
        assert!(!result.contains("CAST(100 AS NUMBER(10,0)) AS amount1")); // Original form should not appear
    }

    #[test]
    fn test_get_unique_id_without_version() {
        let result = get_unique_id("my_model", "my_pkg", None, "model");
        assert_eq!(result, "model.my_pkg.my_model");
    }

    #[test]
    fn test_get_unique_id_with_version() {
        let result = get_unique_id("my_model", "my_pkg", Some("2".to_string()), "model");
        assert_eq!(result, "model.my_pkg.my_model.v2");
    }

    #[test]
    fn test_create_select_with_union_all_semistructured() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("variant_col", SnowflakeTyping::variant(), true),
            Field::new("object_col", SnowflakeTyping::object(), true),
            Field::new(
                "array_col",
                DataType::List(Arc::new(Field::new(
                    "item",
                    SnowflakeTyping::variant(),
                    true,
                ))),
                true,
            ),
        ]));

        let columns =
            columns_to_formatted_types(&schema, &DefaultTypeOps::new(AdapterType::Snowflake))
                .expect("Must format column types");

        let mut object_map = dbt_yaml::mapping::Mapping::new();
        object_map.insert(
            YmlValue::string("foo".to_string()),
            YmlValue::string("bar".to_string()),
        );

        let yml_rows = vec![vec![
            YmlValue::null(),
            YmlValue::Mapping(object_map.clone(), Default::default()),
            // Snowflake supports hlists, so try serializing number, string, mapping
            YmlValue::Sequence(
                vec![
                    YmlValue::number(4.into()),
                    YmlValue::string("abcdef".to_string()),
                    YmlValue::Mapping(object_map, Default::default()),
                ],
                Default::default(),
            ),
        ]];

        let result = create_select_with_union_all(
            AdapterType::Snowflake,
            &DefaultTypeOps::new(AdapterType::Snowflake),
            yml_rows,
            &schema,
            &columns,
            "",
        )
        .unwrap();

        assert_contains!(
            result,
            "CAST(NULL AS VARIANT) AS \"variant_col\"",
            "variant_col should be NULL"
        );
        assert_contains!(
            result,
            "CAST(PARSE_JSON('{\"foo\":\"bar\"}') AS OBJECT) AS \"object_col\"",
            "object_col should use PARSE_JSON"
        );
        assert_contains!(
            result,
            "CAST([4, 'abcdef', PARSE_JSON('{\"foo\":\"bar\"}')] AS ARRAY) AS \"array_col\"",
            "array_col should be a heterogeneous array literal"
        );
    }

    #[test]
    fn test_create_select_with_union_all_snowflake_safe_scalar_casts() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("invalid_number", DataType::Decimal128(19, 0), true),
            Field::new("valid_number", DataType::Decimal128(19, 0), true),
            Field::new("numeric_yaml", DataType::Decimal128(19, 0), true),
            Field::new("boolean_yaml", DataType::Boolean, true),
            Field::new("boolean_string", DataType::Utf8, true),
            Field::new("invalid_binary", DataType::Binary, true),
            Field::new("valid_binary", DataType::Binary, true),
            Field::new("time_value", DataType::Time64(TimeUnit::Microsecond), true),
            Field::new(
                "timestamp_value",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                true,
            ),
            Field::new("null_value", DataType::Utf8, true),
            Field::new("string_value", DataType::Utf8, true),
        ]));
        let formatted_types = [
            "NUMBER(19,0)",
            "NUMBER(19,0)",
            "NUMBER(19,0)",
            "BOOLEAN",
            "VARCHAR",
            "BINARY",
            "BINARY",
            "TIME(6)",
            "TIMESTAMP_NTZ(6)",
            "VARCHAR",
            "VARCHAR",
        ];
        let columns = schema
            .fields()
            .iter()
            .zip(formatted_types)
            .map(|(field, formatted_type)| {
                (field.name(), field.data_type(), formatted_type.to_string())
            })
            .collect();
        let rows = vec![vec![
            YmlValue::string("invalid-number".to_string()),
            YmlValue::string("42".to_string()),
            YmlValue::number(7.into()),
            YmlValue::bool(true),
            YmlValue::bool(false),
            YmlValue::string("not-hex".to_string()),
            YmlValue::string("4142".to_string()),
            YmlValue::string("12:34:56".to_string()),
            YmlValue::string("2024-01-02 03:04:05".to_string()),
            YmlValue::null(),
            YmlValue::string("ordinary".to_string()),
        ]];

        let result = create_select_with_union_all(
            AdapterType::Snowflake,
            &DefaultTypeOps::new(AdapterType::Snowflake),
            rows,
            &schema,
            &columns,
            "",
        )
        .unwrap();

        for expected in [
            "TRY_CAST('invalid-number' AS NUMBER) AS \"invalid_number\"",
            "TRY_CAST('42' AS NUMBER) AS \"valid_number\"",
            "TRY_CAST('7' AS NUMBER) AS \"numeric_yaml\"",
            "TRY_CAST('True' AS BOOLEAN) AS \"boolean_yaml\"",
            "TRY_CAST('False' AS VARCHAR) AS \"boolean_string\"",
            "TRY_CAST('not-hex' AS BINARY) AS \"invalid_binary\"",
            "TRY_CAST('4142' AS BINARY) AS \"valid_binary\"",
            "TRY_CAST('12:34:56' AS TIME(6)) AS \"time_value\"",
            "TRY_CAST('2024-01-02 03:04:05' AS TIMESTAMP_NTZ(6)) AS \"timestamp_value\"",
            "TRY_CAST(NULL AS VARCHAR) AS \"null_value\"",
            "TRY_CAST('ordinary' AS VARCHAR) AS \"string_value\"",
        ] {
            assert_contains!(result, expected);
        }
    }

    #[test]
    fn test_create_bigquery_relation_semistructured() {
        // BigQuery has 'full' type information for the object and array columns
        let schema = Arc::new(Schema::new(vec![
            Field::new("numeric_col", BigqueryTyping::numeric(), true),
            Field::new(
                "object_col",
                DataType::Struct(vec![Arc::new(Field::new("foo", DataType::Utf8, true))].into()),
                true,
            ),
            Field::new(
                "array_col",
                DataType::List(Arc::new(Field::new(
                    "item",
                    BigqueryTyping::numeric(),
                    true,
                ))),
                true,
            ),
        ]));

        let columns =
            columns_to_formatted_types(&schema, &DefaultTypeOps::new(AdapterType::Bigquery))
                .expect("Must format column types");

        let mut object_map = dbt_yaml::mapping::Mapping::new();
        object_map.insert(
            YmlValue::string("foo".to_string()),
            YmlValue::string("bar".to_string()),
        );

        let yml_rows = vec![vec![
            YmlValue::null(),
            YmlValue::Mapping(object_map, Default::default()),
            YmlValue::Sequence(
                vec![YmlValue::number(4.into()), YmlValue::number(2.into())],
                Default::default(),
            ),
        ]];

        let result = create_bigquery_relation_to_select_from(
            &DefaultTypeOps::new(AdapterType::Bigquery),
            yml_rows,
            &schema,
            &columns,
        )
        .unwrap();

        // DefaultTypeOpsImpl maps Decimal128(38,9) to float64 and Utf8 to string (lowercase);
        // the proprietary TypeOpsImpl would produce NUMERIC and STRING for BigQuery.
        assert_contains!(
            result,
            "STRUCT(CAST(NULL AS float64) AS numeric_col",
            "numeric_col should be NULL cast"
        );
        assert_contains!(
            result,
            "CAST(STRUCT('bar' AS foo) AS STRUCT<foo string>) AS object_col",
            "object_col should use typed STRUCT with AS aliases"
        );
        assert_contains!(
            result,
            "CAST([CAST(4 AS float64), CAST(2 AS float64)] AS ARRAY<float64>) AS array_col",
            "array_col should have typed elements and ARRAY<T> type parameter"
        );
    }

    /// A STRUCT fixture supplied as a YAML mapping (not a string SQL
    /// expression) must still render as a `STRUCT(...)` built from typed
    /// fields — the non-string path flagged in review for dbt-core#14625.
    #[test]
    fn test_create_bigquery_relation_struct_from_mapping() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "struct_field",
            DataType::Struct(
                vec![
                    Arc::new(Field::new("field1", DataType::Utf8, true)),
                    Arc::new(Field::new("field2", DataType::Int64, true)),
                ]
                .into(),
            ),
            true,
        )]));

        let columns =
            columns_to_formatted_types(&schema, &DefaultTypeOps::new(AdapterType::Bigquery))
                .expect("Must format column types");

        let mut object_map = dbt_yaml::mapping::Mapping::new();
        object_map.insert(
            YmlValue::string("field1".to_string()),
            YmlValue::string("A".to_string()),
        );
        object_map.insert(
            YmlValue::string("field2".to_string()),
            YmlValue::number(1.into()),
        );
        let yml_rows = vec![vec![YmlValue::Mapping(object_map, Default::default())]];

        let result = create_bigquery_relation_to_select_from(
            &DefaultTypeOps::new(AdapterType::Bigquery),
            yml_rows,
            &schema,
            &columns,
        )
        .unwrap();

        assert_contains!(
            result,
            "STRUCT('A' AS field1, 1 AS field2)",
            "a mapping fixture must render as STRUCT(...) with typed fields"
        );
    }

    /// Regression for dbt-labs/dbt-core#14625: a string fixture value for a
    /// STRUCT / GEOGRAPHY column is a SQL expression and must be injected
    /// verbatim, not single-quoted as a string literal (which BigQuery cannot
    /// CAST to the target type). Plain string columns must still be quoted.
    #[test]
    fn test_create_bigquery_relation_string_expr_for_complex_type() {
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "struct_field",
                DataType::Struct(
                    vec![
                        Arc::new(Field::new("field1", DataType::Utf8, true)),
                        Arc::new(Field::new("field2", DataType::Utf8, true)),
                    ]
                    .into(),
                ),
                true,
            ),
            Field::new("name", DataType::Utf8, true),
        ]));

        let columns =
            columns_to_formatted_types(&schema, &DefaultTypeOps::new(AdapterType::Bigquery))
                .expect("Must format column types");

        let yml_rows = vec![vec![
            YmlValue::string("STRUCT(\"A\" AS field1, \"B\" AS field2)".to_string()),
            YmlValue::string("hello".to_string()),
        ]];

        let result = create_bigquery_relation_to_select_from(
            &DefaultTypeOps::new(AdapterType::Bigquery),
            yml_rows,
            &schema,
            &columns,
        )
        .unwrap();

        assert_contains!(
            result,
            "CAST(STRUCT(\"A\" AS field1, \"B\" AS field2) AS STRUCT<field1 string, field2 string>) AS struct_field",
            "struct expression must be injected raw, not quoted"
        );
        assert_contains!(
            result,
            "CAST('hello' AS string) AS name",
            "plain string values must remain quoted"
        );
    }

    fn row(pairs: &[(&str, i64)]) -> BTreeMap<String, YmlValue> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), YmlValue::number((*v).into())))
            .collect()
    }

    /// A fixture row with a numeric `id` and a string-valued `_FILE_NAME`
    /// pseudocolumn (`file_name_key` lets tests vary the casing), mirroring how
    /// a real BigQuery external-table fixture supplies `_FILE_NAME`.
    fn row_with_file_name(
        id: i64,
        file_name_key: &str,
        file_name: &str,
    ) -> BTreeMap<String, YmlValue> {
        BTreeMap::from([
            ("id".to_string(), YmlValue::number(id.into())),
            (
                file_name_key.to_string(),
                YmlValue::string(file_name.to_string()),
            ),
        ])
    }

    fn schema_id_name() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]))
    }

    #[test]
    fn different_fixture_rows_share_expected_schema_inference() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use dbt_tasks_core::unit_test_schema::UnitTestSchemaState;

        let schema = schema_id_name();
        let type_ops = DefaultTypeOps::new(AdapterType::Snowflake);
        let (first_values, first_shape) = create_values_and_shape_sql(
            &schema,
            &[row(&[("id", 1)])],
            AdapterType::Snowflake,
            &type_ops,
            "orders",
            true,
        )
        .unwrap();
        let (second_values, second_shape) = create_values_and_shape_sql(
            &schema,
            &[row(&[("id", 2)])],
            AdapterType::Snowflake,
            &type_ops,
            "orders",
            true,
        )
        .unwrap();

        assert_ne!(first_values, second_values);
        assert_eq!(first_shape, second_shape);

        let (_, omitted_shape) = create_values_and_shape_sql(
            &schema,
            &[row(&[("id", 1)])],
            AdapterType::Snowflake,
            &type_ops,
            "orders",
            false,
        )
        .unwrap();
        assert!(omitted_shape.is_none());

        let state = UnitTestSchemaState::default();
        let inference_count = AtomicUsize::new(0);
        for fixture_shape in [first_shape.unwrap(), second_shape.unwrap()] {
            let probe_shape = format!("WITH orders AS ({fixture_shape}) SELECT * FROM orders");
            let key = UnitTestExpectedSchemaKey::new(UnitTestExpectedSchemaKeyInput {
                adapter_type: AdapterType::Snowflake,
                model_unique_id: "model.pkg.orders",
                local_probe_shape_sql: &probe_shape,
                fallback_probe_shape_sql: &probe_shape,
                fallback_with_query_schema: true,
                serialized_overrides: "null",
            });
            state
                .get_or_try_infer_expected_schema(key, || {
                    inference_count.fetch_add(1, Ordering::Relaxed);
                    Ok(Arc::clone(&schema))
                })
                .unwrap();
        }

        let stats = state.stats_snapshot();
        assert_eq!(inference_count.load(Ordering::Relaxed), 1);
        assert_eq!(stats.expected_misses, 1);
        assert_eq!(stats.expected_hits, 1);
    }

    #[test]
    fn test_pseudocolumn_appended_when_referenced_bigquery() {
        let schema = schema_id_name();
        let rows = vec![row_with_file_name(1, "_FILE_NAME", "gs://bucket/file1.csv")];
        let result = append_referenced_pseudocolumns(&schema, &rows, AdapterType::Bigquery);
        let names: Vec<&str> = result.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["id", "name", "_FILE_NAME"]);
        let file_name = result.field_with_name("_FILE_NAME").unwrap();
        assert_eq!(file_name.data_type(), &DataType::Utf8);
    }

    #[test]
    fn test_pseudocolumn_matching_is_case_insensitive() {
        let schema = schema_id_name();
        let rows = vec![row_with_file_name(1, "_file_name", "gs://bucket/file1.csv")];
        let result = append_referenced_pseudocolumns(&schema, &rows, AdapterType::Bigquery);
        // The canonical pseudocolumn name is appended regardless of the fixture casing.
        assert!(result.field_with_name("_FILE_NAME").is_ok());
    }

    #[test]
    fn test_pseudocolumn_not_appended_when_unreferenced() {
        let schema = schema_id_name();
        let rows = vec![row(&[("id", 1), ("name", 2)])];
        let result = append_referenced_pseudocolumns(&schema, &rows, AdapterType::Bigquery);
        let names: Vec<&str> = result.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["id", "name"]);
    }

    #[test]
    fn test_pseudocolumn_not_appended_for_other_adapters() {
        let schema = schema_id_name();
        let rows = vec![row_with_file_name(1, "_FILE_NAME", "gs://bucket/file1.csv")];
        // Only BigQuery exposes pseudocolumns today; other adapters leave the fixture
        // to raise the usual "invalid column" warning.
        let result = append_referenced_pseudocolumns(&schema, &rows, AdapterType::Postgres);
        let names: Vec<&str> = result.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(names, vec!["id", "name"]);
    }

    #[test]
    fn test_pseudocolumn_not_duplicated_when_already_present() {
        // A relation that already reports `_FILE_NAME` as a native column must not gain a duplicate.
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("_FILE_NAME", DataType::Utf8, true),
        ]));
        let rows = vec![row_with_file_name(1, "_FILE_NAME", "gs://bucket/file1.csv")];
        let result = append_referenced_pseudocolumns(&schema, &rows, AdapterType::Bigquery);
        let count = result
            .fields()
            .iter()
            .filter(|f| f.name().eq_ignore_ascii_case("_FILE_NAME"))
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn format_fixture_type_falls_back_without_metadata() {
        let type_ops = DefaultTypeOps::new(AdapterType::Bigquery);
        let field = Field::new(
            "c",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        );
        assert_eq!(
            format_fixture_type(&type_ops, &field, true).unwrap(),
            "datetime"
        );

        let type_ops = DefaultTypeOps::new(AdapterType::Snowflake);
        let field =
            make_arrow_field(&type_ops, "c".to_string(), "TIMESTAMP_NTZ(9)", None, None).unwrap();
        assert_eq!(
            format_fixture_type(&type_ops, &field, true).unwrap(),
            "timestamp without time zone"
        );
    }

    mod bigquery_fixtures {
        use super::*;
        use dbt_adapter::column::BigqueryColumnMode;

        fn render_fixture(schema: &SchemaRef, yaml: &str) -> String {
            let rows: Vec<BTreeMap<String, YmlValue>> = dbt_yaml::from_str(yaml).unwrap();
            let render = |allow_pseudocolumns| {
                create_values(
                    schema,
                    &rows,
                    AdapterType::Bigquery,
                    &DefaultTypeOps::new(AdapterType::Bigquery),
                    None,
                    "t",
                    allow_pseudocolumns,
                )
                .unwrap()
            };
            let expected = render(false);
            assert_eq!(
                expected,
                render(true),
                "given and expect fixtures must render identically"
            );
            expected
        }

        fn schema_from_columns(columns: &[(&str, &str)]) -> SchemaRef {
            let type_ops = DefaultTypeOps::new(AdapterType::Bigquery);
            let columns = columns
                .iter()
                .map(|(name, sql_type)| {
                    Column::new_bigquery(
                        name.to_string(),
                        sql_type.to_string(),
                        Vec::new(),
                        BigqueryColumnMode::Nullable,
                    )
                })
                .collect();
            columns_to_schema(&type_ops, columns, "unit_test.regression").unwrap()
        }

        #[test]
        fn geography_and_json_are_not_orderable() {
            let schema = schema_from_columns(&[
                ("some_point", "GEOGRAPHY"),
                ("payload", "JSON"),
                ("name", "STRING"),
            ]);
            let orderable: Vec<&str> = schema
                .fields()
                .iter()
                .filter(|field| is_orderable_type(AdapterType::Bigquery, field))
                .map(|field| field.name().as_str())
                .collect();
            assert_eq!(orderable, ["name"]);
        }

        #[test]
        fn fixture_casts_preserve_reported_types() {
            for (sql_type, value) in [
                ("TIMESTAMP", "2026-01-01 00:00:00+00"),
                ("DATETIME", "2026-01-01 00:00:00"),
                ("NUMERIC", "123456789.123456789"),
                ("BIGNUMERIC", "123456789.123456789123456789"),
            ] {
                let schema = schema_from_columns(&[
                    ("scalar", sql_type),
                    ("items", &format!("ARRAY<{sql_type}>")),
                    ("records", &format!("ARRAY<STRUCT<item_value {sql_type}>>")),
                ]);
                let sql = render_fixture(
                    &schema,
                    &format!(
                        "- scalar: '{value}'\n  items: ['{value}']\n  records: [{{item_value: '{value}'}}]"
                    ),
                );
                assert_contains!(sql, &format!("CAST('{value}' AS {sql_type}) AS scalar"));
                assert_contains!(
                    sql,
                    &format!("CAST([CAST('{value}' AS {sql_type})] AS ARRAY<{sql_type}>) AS items")
                );
                assert_contains!(
                    sql,
                    &format!(
                        "CAST([CAST(STRUCT('{value}' AS item_value) AS STRUCT<item_value {sql_type}>)] AS ARRAY<STRUCT<item_value {sql_type}>>) AS records"
                    )
                );
                assert_contains!(
                    render_fixture(&schema, "[]"),
                    &format!(
                        "ARRAY<STRUCT<scalar {sql_type}, items ARRAY<{sql_type}>, records ARRAY<STRUCT<item_value {sql_type}>>>>[]"
                    )
                );
            }
        }

        #[test]
        fn nested_empty_arrays_keep_their_element_type() {
            for sql_type in [
                "TIMESTAMP",
                "DATETIME",
                "NUMERIC",
                "BIGNUMERIC",
                "GEOGRAPHY",
                "JSON",
            ] {
                let schema =
                    schema_from_columns(&[("nested", &format!("STRUCT<items ARRAY<{sql_type}>>"))]);
                let sql = render_fixture(&schema, "- nested: {items: []}");
                assert_contains!(sql, &format!("STRUCT(ARRAY<{sql_type}>[] AS items)"));
            }
        }

        #[test]
        fn test_create_bigquery_relation_string_expr_for_geography() {
            let schema = schema_from_columns(&[
                ("some_point", "GEOGRAPHY"),
                ("nested_struct", "STRUCT<nested_point GEOGRAPHY>"),
                ("geography_array", "ARRAY<GEOGRAPHY>"),
                ("missing", "GEOGRAPHY"),
            ]);
            let result = render_fixture(
                &schema,
                r#"
- some_point: ST_GEOGPOINT(100, -37)
  nested_struct:
    nested_point: ST_GEOGPOINT(101, -38)
  geography_array: ["ST_GEOGPOINT(102, -39)"]
"#,
            );

            for expected in [
                "CAST(ST_GEOGPOINT(100, -37) AS GEOGRAPHY) AS some_point",
                "CAST(STRUCT(ST_GEOGPOINT(101, -38) AS nested_point) AS STRUCT<nested_point GEOGRAPHY>) AS nested_struct",
                "CAST([CAST(ST_GEOGPOINT(102, -39) AS GEOGRAPHY)] AS ARRAY<GEOGRAPHY>) AS geography_array",
                "CAST(NULL AS GEOGRAPHY) AS missing",
            ] {
                assert_contains!(result, expected);
            }
        }

        /// Regression for dbt-labs/dbt-core#15708: a BigQuery `JSON` column is
        /// mockable from either an object or a JSON string, both rendered with
        /// `PARSE_JSON` before casting to the reported type.
        #[test]
        fn test_create_values_bigquery_json_column() {
            let schema = schema_from_columns(&[
                ("from_object", "JSON"),
                ("from_string", "JSON"),
                ("with_escapes", "JSON"),
                ("missing", "JSON"),
                ("from_array", "ARRAY<JSON>"),
                ("nested", "STRUCT<payload JSON>"),
            ]);
            let result = render_fixture(
                &schema,
                r#"
- from_object: {segmentCode: HORECA}
  from_string: '{"segmentCode":"HORECA"}'
  with_escapes: {note: "a\nb"}
  missing: null
  from_array: [{segmentCode: HORECA}]
  nested: {payload: {segmentCode: HORECA}}
"#,
            );

            for expected in [
                r#"CAST(PARSE_JSON('{"segmentCode":"HORECA"}') AS JSON) AS from_object"#,
                r#"CAST(PARSE_JSON('{"segmentCode":"HORECA"}') AS JSON) AS from_string"#,
                r#"CAST(PARSE_JSON('{"note":"a\\nb"}') AS JSON) AS with_escapes"#,
                "CAST(NULL AS JSON) AS missing",
                r#"CAST([CAST(PARSE_JSON('{"segmentCode":"HORECA"}') AS JSON)] AS ARRAY<JSON>) AS from_array"#,
                r#"CAST(STRUCT(PARSE_JSON('{"segmentCode":"HORECA"}') AS payload) AS STRUCT<payload JSON>) AS nested"#,
            ] {
                assert_contains!(result, expected);
            }
        }
    }
}
