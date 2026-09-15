use arrow::compute::cast_with_options;
use arrow::{
    self,
    datatypes::{DataType, TimeUnit},
};
use chrono::{DateTime, Utc};
use datafusion_common::{cast::as_timestamp_nanosecond_array, format::DEFAULT_CAST_OPTIONS};
use dbt_adapter::{
    Adapter,
    errors::into_fs_error,
    relation::{RelationObject, create_relation_from_node, do_create_relation},
};
use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_common::constants::{DBT_FRESHNESS_JSON, DBT_SOURCES_JSON, default_metadata_dir};
use dbt_common::io_args::{ClapResourceType, IoArgs, ShowOptions};
use dbt_common::tracing::dbt_emit::emit_info_log_message;
use dbt_common::tracing::event_info::store_event_attributes;
use dbt_common::tracing::span_info::{record_span_status_with_attrs, update_span_attrs};
use dbt_common::tracing::{
    dbt_emit::{
        emit_error_log_from_fs_error, emit_info_progress_message, emit_warn_log_from_fs_error,
        emit_warn_log_message,
    },
    emit::{create_info_span, emit_info_event},
};
use dbt_common::{
    CodeLocationWithFile, ErrorCode, FsError, FsResult, err, fs_err, stdfs::File, unexpected_fs_err,
};
use dbt_dag::schedule::Schedule;
use dbt_jinja_ctx::{CompileBaseCtx, CustomSqlRenderCtx, DummyConfig, JinjaObject, OperationCtx};
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv,
    phases::{build_compile_base_ctx, run::build_run_node_ctx},
};
use dbt_schemas::schemas::telemetry::{ExecutionPhase, PhaseExecuted};
use dbt_schemas::schemas::{
    DbtModel, DbtSource, FreshnessNodeRef, FreshnessResultsArtifact, FreshnessResultsMetadata,
    FreshnessResultsNode, InternalDbtNodeAttributes, NodePathKind,
    common::{FreshnessDefinition, FreshnessPeriod, FreshnessRules, FreshnessStatus},
    is_freshness_node,
    relations::base::BaseRelation,
};
use dbt_schemas::state::ResolverState;
use dbt_tasks_core::PreTaskRunData;
use dbt_tasks_core::task::TaskOp;
use dbt_telemetry::{
    ArtifactType, ArtifactWritten, NodeOutcome, NodeProcessed, NodeType, ProgressMessage,
    ShowResult, SourceFreshnessDetail, SourceFreshnessOutcome, node_processed,
    update_dbt_core_event_code_for_node_processed_end,
};
use itertools::Itertools;
use std::collections::HashMap;
use std::path::Path;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct FreshnessResult {
    pub max_loaded_at: DateTime<Utc>,
    pub snapshotted_at: DateTime<Utc>,
    pub age: i64,
    pub status: FreshnessStatus,
}

impl FreshnessResult {
    pub fn from(
        max_loaded_at: DateTime<Utc>,
        snapshotted_at: DateTime<Utc>,
        age: i64,
    ) -> FreshnessResult {
        FreshnessResult {
            max_loaded_at,
            snapshotted_at,
            age,
            status: FreshnessStatus::Pass,
        }
    }
}

pub struct FreshnessTimestamps(pub BTreeMap<String, FreshnessResult>);

impl PreTaskRunData for FreshnessTimestamps {
    fn get(&self, node_id: &str) -> Option<String> {
        self.0.get(node_id).map(|r| r.max_loaded_at.to_string())
    }
}

fn period_to_seconds(period: &FreshnessPeriod) -> i64 {
    match period {
        FreshnessPeriod::second => 1,
        FreshnessPeriod::minute => 60,
        FreshnessPeriod::hour => 60 * 60,
        FreshnessPeriod::day => 60 * 60 * 24,
    }
}

pub fn calculate_seconds(freshness: &FreshnessRules) -> FsResult<i64> {
    // F2 safety net: parse-time validation now demotes a partially-populated
    // freshness rule to a warning so `parse` / `run` / `build` are not
    // aborted. When the rule is actually consumed (e.g. by
    // `dbt source freshness`), revalidate and surface the same `dbt1007`
    // error rather than panicking on the missing field.
    FreshnessRules::validate(Some(freshness))?;
    let count = freshness.count.ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidArgument,
            "count and period are required when freshness is provided, count: {:?}, period: {:?}",
            freshness.count,
            freshness.period
        )
    })?;
    let period = freshness.period.as_ref().ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidArgument,
            "count and period are required when freshness is provided, count: {:?}, period: {:?}",
            freshness.count,
            freshness.period
        )
    })?;
    Ok(count * period_to_seconds(period))
}

const MAX_LOADED_AT_COLUMN: usize = 0;
const SNAPSHOTTED_AT_COLUMN: usize = 1;

/// `dbt source freshness` keeps its own wording verbatim: the message is
/// user-visible and predates the unified command, so only the new spelling
/// mentions models.
fn nothing_to_do_message(sources_only: bool) -> &'static str {
    if sources_only {
        "Nothing to do. No sources with freshness config."
    } else {
        "Nothing to do. No sources or models with a freshness config."
    }
}

/// Parse time only warns on a partial rule, so the `dbt1007` error has to surface
/// here. Up front, because failing inside the results loop would discard every
/// other node's already-measured result before any artifact is written.
fn validate_freshness_rules_early(node: &dyn FreshnessNodeRef) -> FsResult<()> {
    let criteria = node.freshness_criteria();
    let loc = node.common().name_span.start.clone();
    FreshnessRules::validate(criteria.error_after.as_ref())
        .map_err(|e| e.with_location(loc.clone()))?;
    FreshnessRules::validate(criteria.warn_after.as_ref()).map_err(|e| e.with_location(loc))?;
    Ok(())
}

/// Revalidates on the way through: parse time only warns on a partial rule.
fn evaluate_freshness_thresholds(
    criteria: &FreshnessDefinition,
    age: i64,
) -> FsResult<(FreshnessStatus, SourceFreshnessOutcome)> {
    let error_after = criteria
        .error_after
        .as_ref()
        .map(calculate_seconds)
        .transpose()?;
    let warn_after = criteria
        .warn_after
        .as_ref()
        .map(calculate_seconds)
        .transpose()?;

    if error_after.is_some_and(|threshold| age > threshold) {
        Ok((
            FreshnessStatus::Error,
            SourceFreshnessOutcome::OutcomeFailed,
        ))
    } else if warn_after.is_some_and(|threshold| age > threshold) {
        Ok((FreshnessStatus::Warn, SourceFreshnessOutcome::OutcomeWarned))
    } else {
        Ok((FreshnessStatus::Pass, SourceFreshnessOutcome::OutcomePassed))
    }
}

/// `None` when the node declares neither key, and so belongs to the batch
/// metadata pre-pass instead.
#[allow(clippy::too_many_arguments)]
async fn measure_query_based_freshness(
    node: &dyn FreshnessNodeRef,
    adapter_type: AdapterType,
    jinja_env: &Arc<JinjaEnv>,
    compile_base: &CompileBaseCtx,
    io_args: &IoArgs,
    dependencies: BTreeSet<String>,
) -> FsResult<Option<FreshnessResult>> {
    let mut result = None;

    let loaded_at_field = node.get_loaded_at_field();
    if !loaded_at_field.is_empty() {
        let filter = node.get_freshness_filter().unwrap_or("");
        result = Some(
            calculate_freshness(
                &loaded_at_field.escape_default().to_string(),
                &filter.escape_default().to_string(),
                node,
                adapter_type,
                jinja_env,
                compile_base,
                io_args,
                dependencies.clone(),
            )
            .await?,
        );
    }

    // `loaded_at_query` wins when both are set, as before.
    let loaded_at_query = node.get_loaded_at_query();
    if !loaded_at_query.is_empty() {
        result = Some(
            calculate_freshness_custom_sql(
                loaded_at_query,
                node,
                adapter_type,
                jinja_env,
                compile_base,
                io_args,
                dependencies,
            )
            .await?,
        );
    }

    Ok(result)
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, level = "trace")]
pub async fn calculate_freshness(
    loaded_at_field: &str,
    filter: &str,
    node: &dyn FreshnessNodeRef,
    adapter_type: AdapterType,
    jinja_env: &Arc<JinjaEnv>,
    compile_base: &CompileBaseCtx,
    io_args: &IoArgs,
    dependencies: BTreeSet<String>,
) -> FsResult<FreshnessResult> {
    let relation = node
        .base()
        .relation_name
        .as_ref()
        .ok_or_else(|| unexpected_fs_err!("{} needs a relation name", node.kind_label()))?
        .as_str();
    let macro_expr = if filter.is_empty() {
        format!("collect_freshness('{relation}', '{loaded_at_field}').table")
    } else {
        format!("collect_freshness('{relation}', '{loaded_at_field}', '{filter}').table")
    };

    calculate_freshness_common(
        &macro_expr,
        node,
        adapter_type,
        jinja_env,
        compile_base,
        io_args,
        dependencies,
        "loaded_at_field",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, level = "trace")]
pub async fn calculate_freshness_custom_sql(
    loaded_at_query: &str,
    node: &dyn FreshnessNodeRef,
    adapter_type: AdapterType,
    jinja_env: &Arc<JinjaEnv>,
    compile_base: &CompileBaseCtx,
    io_args: &IoArgs,
    dependencies: BTreeSet<String>,
) -> FsResult<FreshnessResult> {
    let relation = node
        .base()
        .relation_name
        .as_ref()
        .ok_or_else(|| unexpected_fs_err!("{} needs a relation name", node.kind_label()))?
        .as_str();

    // Create a context with `this` relation for rendering the loaded_at_query
    // This allows users to use {{this}} in their loaded_at_query to reference the source table
    let this_relation = RelationObject::new(Arc::from(
        do_create_relation(
            adapter_type,
            node.base().database.clone(),
            node.base().schema.clone(),
            Some(node.base().alias.clone()),
            None,
            node.base().quoting,
        )
        .map_err(|e| {
            fs_err!(
                code => ErrorCode::Unexpected,
                loc => node.get_node_path(NodePathKind::Definition, io_args.in_dir.as_path(), io_args.out_dir.as_path()).into_owned(),
                "Failed to create 'this' relation for {} '{}': {}",
                node.kind_label().to_lowercase(),
                node.common().unique_id,
                e
            )
        })?,
    ))
    .into_value();

    // Pre-render context: operation scope (base + no-op `config`) plus
    // `this`/`database`/`schema`/`identifier` so loaded_at_query can reference
    // `{{ this }}`. Rendered typed, no BTreeMap.
    let render_context = CustomSqlRenderCtx {
        operation: OperationCtx {
            base: compile_base.clone(),
            config: JinjaObject::new(DummyConfig {}),
        },
        this: this_relation,
        database: node.base().database.clone(),
        schema: node.base().schema.clone(),
        identifier: node.base().alias.clone(),
    };

    // Pre-render the loaded_at_query to resolve any Jinja expressions like {{this}}
    let source_path = node
        .get_node_path(
            NodePathKind::Definition,
            io_args.in_dir.as_path(),
            io_args.out_dir.as_path(),
        )
        .into_owned();
    // A user's `loaded_at_query` can call a macro that reaches the adapter, so
    // this render goes to the blocking pool like the `collect_freshness`
    // expression below it: every database connection must be created by a
    // `dbt-runtime` worker thread.
    let rendered_query = {
        let jinja_env = Arc::clone(jinja_env);
        let loaded_at_query = loaded_at_query.to_owned();
        TaskOp::Blocking(Box::new(move || -> FsResult<String> {
            jinja_env
                .render_named_str(
                    &source_path.to_string_lossy(),
                    &loaded_at_query,
                    &render_context,
                    &[],
                )
                .map_err(|e| {
                    let loc = e.significant_span().map(|span| {
                        CodeLocationWithFile::new(
                            span.start_line,
                            span.start_col,
                            span.start_offset,
                            source_path.clone(),
                        )
                    });
                    let fs_err = FsError::from_jinja_err(e, "Failed to render the Jinja str");
                    match loc {
                        Some(loc) => fs_err.with_location(loc),
                        None => fs_err.with_location(source_path.clone()),
                    }
                    .into()
                })
        }))
        .run()
        .await??
    };
    // Insert loaded_at_query as an escaped string
    let macro_expr = format!(
        "collect_freshness_custom_sql('{relation}', '{}').table",
        rendered_query.escape_default()
    );

    calculate_freshness_common(
        &macro_expr,
        node,
        adapter_type,
        jinja_env,
        compile_base,
        io_args,
        dependencies,
        "loaded_at_query",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, level = "trace")]
async fn calculate_freshness_common(
    macro_expr: &str,
    node: &dyn FreshnessNodeRef,
    adapter_type: AdapterType,
    jinja_env: &Arc<JinjaEnv>,
    compile_base: &CompileBaseCtx,
    io_args: &IoArgs,
    dependencies: BTreeSet<String>,
    error_message: &str,
) -> FsResult<FreshnessResult> {
    let context = build_run_node_ctx(
        node,
        &node.serialized_config(),
        adapter_type,
        None,
        compile_base,
        io_args,
        ExecutionPhase::Run,
        None,
        dependencies,
    );

    // The `collect_freshness` macros query the warehouse, so the expression has
    // to be evaluated on the blocking pool: every database connection must be
    // created by a `dbt-runtime` worker thread.
    let table = {
        let jinja_env = Arc::clone(jinja_env);
        let macro_expr = macro_expr.to_owned();
        TaskOp::Blocking(Box::new(move || {
            let expr = jinja_env.compile_expression(&macro_expr)?;
            expr.eval(&context, &[])?
                .downcast_object::<AgateTable>()
                .ok_or_else(|| unexpected_fs_err!("Agate table expected"))
        }))
        .run()
        .await??
    };

    let batch = table.original_record_batch();

    // check if table has one row and 2 columns, if not raise an error
    if batch.num_rows() != 1 || batch.num_columns() != 2 {
        return err!(
            code => ErrorCode::Unexpected,
            loc => node.get_node_path(NodePathKind::Definition, io_args.in_dir.as_path(), io_args.out_dir.as_path()).into_owned(),
            "{} '{}' freshness result table should have 1 row and 2 columns, but got {} rows and {} columns",
            node.kind_label(),
            node.common().unique_id,
            batch.num_rows(),
            batch.num_columns()
        );
    }

    // A user provided query/field has to have a timestamp type of any precision
    // and we convert it to nanoseconds later.
    let max_loaded_at_column = validate_and_extract_timestamp_column(
        &batch,
        MAX_LOADED_AT_COLUMN,
        node,
        error_message,
        io_args,
    )?;

    let snapshotted_at = validate_and_extract_timestamp_column(
        &batch,
        SNAPSHOTTED_AT_COLUMN,
        node,
        error_message,
        io_args,
    )?;

    let max_loaded_at = as_timestamp_nanosecond_array(&cast_with_options(
        max_loaded_at_column.as_ref(),
        &DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into())),
        &DEFAULT_CAST_OPTIONS,
    )?)?
    .value(0);

    let snapshotted_at = as_timestamp_nanosecond_array(&cast_with_options(
        snapshotted_at.as_ref(),
        &DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into())),
        &DEFAULT_CAST_OPTIONS,
    )?)?
    .value(0);

    let age = (snapshotted_at - max_loaded_at) / 1_000_000_000;
    let result = FreshnessResult::from(
        DateTime::from_timestamp_nanos(max_loaded_at),
        DateTime::from_timestamp_nanos(snapshotted_at),
        age,
    );

    Ok(result)
}

/// Whether a source takes part in the freshness pass that state-aware orchestration
/// (`dbt build` / `dbt run`) runs before the task runner, to decide whether downstream
/// models must rebuild.
///
/// A source opts out of freshness with the documented escape hatch
/// `config: freshness: null`, which is the *only* way `__source_attr__.freshness` ends up
/// `None`: a source that configures nothing still resolves to an empty `FreshnessDefinition`
/// (see `merge_freshness_unwrapped` in `dbt-parser`'s `resolve_sources`, META-5461), so those
/// sources keep their metadata tracked here. An opted-out source has no rule to evaluate, so
/// querying `INFORMATION_SCHEMA.TABLES` for it buys nothing and warns for objects that don't
/// live there at all — e.g. Snowflake SEQUENCE objects declared as sources, which is what
/// dbt-labs/dbt-core#14534 reported.
///
/// A `loaded_at_field` / `loaded_at_query` is still honored on an opted-out source: the user
/// named a column/query to measure freshness from, so it is measured.
///
/// Skipped sources have no entry in the returned results, which the run-cache treats like any
/// other source without freshness data: the source counts as just-updated and its dependents
/// rebuild.
fn collects_freshness_during_build(node: &DbtSource) -> bool {
    node.__source_attr__.freshness.is_some()
        || !node.get_loaded_at_field().is_empty()
        || !node.get_loaded_at_query().is_empty()
}

/// This function is used to calculate the freshness of the sources and extended models.
/// Whether `ty` survives `--resource-type`/`--exclude-resource-type` filtering.
///
/// Shared between [`run_freshness`] (gating which nodes are actually measured) and
/// [`selection_contains_source`] (gating the `sources.json` write guard), so the two
/// can never disagree about whether sources are in scope for a given invocation.
fn resource_type_allowed(
    ty: ClapResourceType,
    resource_types: &[ClapResourceType],
    exclude_resource_types: &[ClapResourceType],
) -> bool {
    (resource_types.is_empty() || resource_types.contains(&ty))
        && !exclude_resource_types.contains(&ty)
}

#[allow(clippy::cognitive_complexity)]
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip_all,
    fields(
        _e = ?store_event_attributes(PhaseExecuted::start_general(ExecutionPhase::FreshnessAnalysis)),
    )
)]
pub async fn run_freshness(
    io_args: &IoArgs,
    schedule: &Schedule<String>,
    resolver_state: &ResolverState,
    adapter: Arc<Adapter>,
    env: &Arc<JinjaEnv>,
    is_freshness_command: bool,
    check_all: bool,
    sources_only: bool,
    resource_types: &[ClapResourceType],
    exclude_resource_types: &[ClapResourceType],
) -> FsResult<BTreeMap<String, FreshnessResult>> {
    // Frontier nodes (upstream dependencies of a selected node that are not
    // themselves selected) are chained in below regardless of selection, so a
    // source read by an included model would otherwise always be measured —
    // bypassing `--exclude-resource-type source`/`--resource-type model`. Gate
    // each resource type explicitly here rather than relying solely on the
    // generic scheduler-level filter over `selected_nodes`.
    let sources_allowed = resource_type_allowed(
        ClapResourceType::Source,
        resource_types,
        exclude_resource_types,
    );
    let models_allowed = resource_type_allowed(
        ClapResourceType::Model,
        resource_types,
        exclude_resource_types,
    );

    // First, collect all sources and extended models
    let set_of_nodes = schedule
        .all_selected_nodes
        .clone()
        .into_iter()
        .chain(schedule.frontier_nodes.clone().into_iter())
        .collect::<BTreeSet<_>>();

    let mut sources = if sources_allowed {
        set_of_nodes
            .iter()
            .map(|unique_id| {
                resolver_state
                    .nodes
                    .get_node(unique_id)
                    .ok_or_else(|| unexpected_fs_err!("Node must be resolved"))
            })
            .filter_map_ok(|node| node.as_any().downcast_ref::<DbtSource>())
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![]
    };

    if is_freshness_command {
        for node in sources.iter() {
            validate_freshness_rules_early(*node)?;
        }
    }

    // Build-time freshness handles models through the extended-model path below.
    let sla_models = if is_freshness_command && !sources_only && models_allowed {
        set_of_nodes
            .iter()
            .map(|unique_id| {
                resolver_state
                    .nodes
                    .get_node(unique_id)
                    .ok_or_else(|| unexpected_fs_err!("Node must be resolved"))
            })
            .filter_map_ok(|node| {
                if is_freshness_node(node) {
                    node.as_any().downcast_ref::<DbtModel>()
                } else {
                    None
                }
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![]
    };

    // `sla_models` is only populated for the freshness command, so no extra gate.
    for node in sla_models.iter() {
        validate_freshness_rules_early(*node)?;
    }

    // If we are not checking all
    let extended_models = if is_freshness_command {
        if !check_all {
            sources = sources
                .into_iter()
                .filter(|node| {
                    node.__source_attr__
                        .freshness
                        .as_ref()
                        .is_some_and(|f| f.error_after.is_some() || f.warn_after.is_some())
                })
                .collect::<Vec<_>>();
        }
        // If this is a source freshness command, then we don't need to check extended models
        vec![]
    } else {
        sources.retain(|node| collects_freshness_during_build(node));

        // If this is not a source freshness command, then we need to check extended models
        set_of_nodes
            .iter()
            .map(|unique_id| {
                resolver_state
                    .nodes
                    .get_node(unique_id)
                    .ok_or_else(|| unexpected_fs_err!("Node must be resolved"))
            })
            .filter_map_ok(|node| {
                if node.is_extended_model() {
                    node.as_any().downcast_ref::<DbtModel>()
                } else {
                    None
                }
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    // Early out if we have nothing to check freshness for
    if sources.is_empty() && sla_models.is_empty() && extended_models.is_empty() {
        // If this is a freshness command, issue "Nothing to do" message.
        if is_freshness_command {
            emit_warn_log_message(
                ErrorCode::FreshnessConfigInvalid,
                nothing_to_do_message(sources_only),
            );
        }
        return Ok(BTreeMap::new());
    }

    // Create spans for all sources BEFORE issuing any queries
    // These spans will be updated with outcomes after freshness is calculated.
    // When freshness is collected as part of a build rather than by
    // `dbt source freshness`, no spans are created so users are not confused by
    // extra counts of processed nodes. We instead issue a progress log line.
    let node_spans: HashMap<&str, tracing::Span> = if is_freshness_command {
        let sources = sources.iter().map(|node| *node as &dyn FreshnessNodeRef);
        let models = sla_models.iter().map(|node| *node as &dyn FreshnessNodeRef);
        sources
            .chain(models)
            .map(|node| {
                let node_processed_event = node.get_node_processed_event(
                    Some(ExecutionPhase::FreshnessAnalysis),
                    io_args.in_dir.as_path(),
                    io_args.out_dir.as_path(),
                    true,
                );
                let span = create_info_span(node_processed_event);
                (node.common().unique_id.as_str(), span)
            })
            .collect()
    } else {
        HashMap::new()
    };

    // Build the typed compile base once for the whole freshness pass and
    // thread `&CompileBaseCtx` into the node contexts below.
    let namespace_keys: Vec<String> = env
        .env
        .get_macro_namespace_registry()
        .map(|r| r.keys().map(|k| k.to_string()).collect())
        .unwrap_or_default();
    let compile_base = build_compile_base_ctx(
        resolver_state.node_resolver.clone(),
        &resolver_state.root_project_name,
        &resolver_state.nodes,
        resolver_state.defer_nodes.as_ref(),
        resolver_state.runtime_config.clone(),
        namespace_keys,
    );

    let results = run_freshness_with_spans(
        &sources,
        &sla_models,
        &extended_models,
        resolver_state,
        adapter,
        env,
        &compile_base,
        io_args,
        is_freshness_command,
        &node_spans,
    )
    .await;

    if is_freshness_command && let Err(ref err) = results {
        let error_message = err.to_string();
        for span in node_spans.values() {
            record_span_status_with_attrs(
                span,
                |attrs| {
                    if let Some(node_processed) = attrs.downcast_mut::<NodeProcessed>() {
                        node_processed.node_outcome = NodeOutcome::Error as i32;
                        update_dbt_core_event_code_for_node_processed_end(node_processed);
                    }
                },
                Some(error_message.as_str()),
            );
        }
    }

    // node_spans are dropped here, which closes the spans and emits the end events
    results
}

#[allow(clippy::too_many_arguments)]
async fn run_freshness_with_spans(
    sources: &Vec<&DbtSource>,
    sla_models: &Vec<&DbtModel>,
    extended_models: &Vec<&DbtModel>,
    resolver_state: &ResolverState,
    adapter: Arc<Adapter>,
    env: &Arc<JinjaEnv>,
    compile_base: &CompileBaseCtx,
    io_args: &IoArgs,
    is_freshness_command: bool,
    node_spans: &HashMap<&str, tracing::Span>,
) -> FsResult<BTreeMap<String, FreshnessResult>> {
    let mut results: BTreeMap<String, FreshnessResult> = BTreeMap::new();
    // Collect all relations that do not specify a 'loaded_at' field or a custom sql query.
    let (relations, name_map) = collect_relations(
        sources,
        sla_models,
        extended_models,
        resolver_state.adapter_type,
    )?;

    // Get the freshness information for the collected source relations.
    if !relations.is_empty()
        && let Some(metadata_adapter) = adapter.metadata_adapter()
    {
        let freshness_result = metadata_adapter
            .freshness(&relations, adapter.cancellation_token())
            .await
            .map_err(into_fs_error);

        // When freshness is collected as part of a build, metadata query failures
        // (e.g. authorization errors for databases the role cannot access) should not
        // fail the build. Sources without freshness data will be handled by the
        // downstream fallback (treated as updated).
        let freshness = match freshness_result {
            Ok(f) => f,
            Err(e) if !is_freshness_command => {
                emit_warn_log_message(
                    ErrorCode::FreshnessMetadataWarning,
                    format!(
                        "Failed to extract freshness metadata: {e}. Affected sources will be treated as updated."
                    ),
                );
                BTreeMap::new()
            }
            Err(e) => return Err(e),
        };

        let snapshotted_at = Utc::now();
        for (relation, metadata) in freshness {
            let unique_ids = name_map
                .get(&relation)
                .ok_or_else(|| unexpected_fs_err!("key must exist"))?;
            let mut max_loaded_at = metadata.last_altered;
            if metadata.is_view {
                // Views are considered to have fresh data by default.
                max_loaded_at = snapshotted_at;
                // Only show warning for sources, not models
                for unique_id in unique_ids {
                    if unique_id.starts_with("source.") {
                        emit_warn_log_message(
                            ErrorCode::FreshnessConfigInvalid,
                            format!(
                                "{} is a view with no 'loaded_at_field' or 'loaded_at_query' so will always be considered fresh",
                                unique_id
                            ),
                        );
                    }
                }
            }
            let freshness_result = FreshnessResult::from(
                max_loaded_at,
                snapshotted_at,
                snapshotted_at.timestamp() - max_loaded_at.timestamp(),
            );
            for unique_id in unique_ids {
                results.insert(unique_id.clone(), freshness_result.clone());
            }
        }
    }

    // Run 'loaded_at' and custom queries.
    let dependencies: BTreeSet<String> = resolver_state
        .runtime_config
        .dependencies
        .keys()
        .cloned()
        .collect();
    let mut node_errors: HashMap<String, Box<FsError>> =
        HashMap::with_capacity(sources.len() + sla_models.len());
    let sources_as_query_nodes = sources.iter().map(|node| *node as &dyn FreshnessNodeRef);
    let sla_models_as_query_nodes = sla_models.iter().map(|node| *node as &dyn FreshnessNodeRef);
    let query_based_nodes = sources_as_query_nodes.chain(sla_models_as_query_nodes);
    for node in query_based_nodes {
        match measure_query_based_freshness(
            node,
            resolver_state.adapter_type,
            env,
            compile_base,
            io_args,
            dependencies.clone(),
        )
        .await
        {
            Ok(Some(result)) => {
                results.insert(node.common().unique_id.clone(), result);
            }
            Ok(None) => {}
            Err(e) if !is_freshness_command => {
                emit_warn_log_message(
                    ErrorCode::FreshnessMetadataWarning,
                    format!(
                        "Failed to calculate freshness for {} '{}': {e}. It will be treated as updated, and dependent models will rebuild.",
                        node.kind_label().to_lowercase(),
                        node.common().unique_id
                    ),
                );
            }
            Err(e) => {
                node_errors.insert(node.common().unique_id.clone(), e);
            }
        }
    }

    let reported_nodes = sources
        .iter()
        .map(|node| *node as &dyn FreshnessNodeRef)
        .chain(sla_models.iter().map(|node| *node as &dyn FreshnessNodeRef));
    for node in reported_nodes {
        let kind = node.kind_label().to_lowercase();
        if let Some(result) = results.get_mut(&node.common().unique_id) {
            if !is_freshness_command {
                // Emit a freshness info log when freshness is collected as part of a
                // build. The source freshness command generates `NodeProcessed`, so it
                // does not need this.
                emit_info_progress_message(ProgressMessage::new_from_action_and_target(
                    "Freshness".to_string(),
                    format!(
                        "{} last updated {} ago",
                        node.common().unique_id,
                        humantime::format_duration(std::time::Duration::from_secs(
                            result.age as u64
                        ))
                    ),
                ));
            }
            let (status, freshness_outcome) =
                evaluate_freshness_thresholds(&node.freshness_criteria(), result.age)?;
            result.status = status;

            // Update the span status with freshness outcome
            // The span was created before the queries, now we update it with the result
            if is_freshness_command {
                if let Some(span) = node_spans.get(node.common().unique_id.as_str()) {
                    update_span_attrs(span, |ev: &mut NodeProcessed| {
                        ev.node_outcome = NodeOutcome::Success as i32;
                        ev.node_outcome_detail =
                            Some(node_processed::NodeOutcomeDetail::NodeFreshnessOutcome(
                                SourceFreshnessDetail {
                                    node_freshness_outcome: freshness_outcome as i32,
                                    age_seconds: Some(result.age),
                                },
                            ));
                        update_dbt_core_event_code_for_node_processed_end(ev);
                    });
                }

                // Emit error/warn logs for source freshness command (for backward compatibility)
                if freshness_outcome == SourceFreshnessOutcome::OutcomeFailed {
                    let err = fs_err!(
                        code => ErrorCode::StaleSource,
                        loc => node.get_node_path(NodePathKind::Definition, io_args.in_dir.as_path(), io_args.out_dir.as_path()).into_owned(),
                        "Stale {} {}",
                        kind,
                        node.common().unique_id
                    );
                    emit_error_log_from_fs_error(*err);
                } else if freshness_outcome == SourceFreshnessOutcome::OutcomeWarned {
                    let err = fs_err!(
                        code => ErrorCode::StaleSource,
                        loc => node.get_node_path(NodePathKind::Definition, io_args.in_dir.as_path(), io_args.out_dir.as_path()).into_owned(),
                        "Stale {} {}",
                        kind,
                        node.common().unique_id
                    );
                    emit_warn_log_from_fs_error(*err);
                }
            }
        } else if is_freshness_command {
            if let Some(span) = node_spans.get(node.common().unique_id.as_str()) {
                update_span_attrs(span, |ev: &mut NodeProcessed| {
                    ev.node_outcome = NodeOutcome::Error as i32;
                    update_dbt_core_event_code_for_node_processed_end(ev);
                });
            }
            let err = node_errors
                .remove(&node.common().unique_id)
                .unwrap_or_else(|| {
                    fs_err!(
                        code => ErrorCode::Unexpected,
                        loc => node.common().name_span.start.clone(),
                        "Could not find freshness information for {kind} '{}'. Please verify that you have access to view metadata for this {kind} in the warehouse.",
                        node.common().unique_id
                    )
                });
            emit_error_log_from_fs_error(*err);
        } else {
            // When freshness is collected as part of a build, missing freshness info is
            // non-fatal. The downstream task runner falls back to Utc::now() for sources
            // without results, conservatively forcing dependent models to rebuild.
            emit_warn_log_message(
                ErrorCode::FreshnessMetadataWarning,
                format!(
                    "Could not find freshness information for {kind} '{}'. The {kind} will be treated as updated, and dependent models will rebuild.",
                    node.common().unique_id
                ),
            );
        }
    }

    Ok(results)
}

// Helper function to create relation and update name map
fn create_relation_and_map(
    node: &impl InternalDbtNodeAttributes,
    adapter_type: AdapterType,
    name_map: &mut BTreeMap<String, Vec<String>>,
) -> FsResult<Arc<dyn BaseRelation>> {
    let relation = create_relation_from_node(adapter_type, node, None)?;
    name_map
        .entry(relation.semantic_fqn())
        .or_default()
        .push(node.unique_id());
    Ok(relation.into())
}

/// Return collection of relations and a map from relation FQN to node unique_ids.
///
/// Multiple source nodes from different packages may point to the same physical
/// table (same FQN). The map therefore uses `Vec<String>` so every unique_id
/// that shares a FQN receives freshness data.  The relations list is deduplicated
/// so the warehouse is queried only once per physical table.
#[allow(clippy::type_complexity)]
fn collect_relations(
    sources: &[&DbtSource],
    sla_models: &Vec<&DbtModel>,
    extended_models: &Vec<&DbtModel>,
    adapter_type: AdapterType,
) -> FsResult<(Vec<Arc<dyn BaseRelation>>, BTreeMap<String, Vec<String>>)> {
    let mut name_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut all_relations = Vec::new();

    // Collect relations from sources
    let source_relations = sources
        .iter()
        .filter(|node| {
            node.get_loaded_at_field().is_empty() && node.get_loaded_at_query().is_empty()
        })
        .map(|node| create_relation_and_map(*node, adapter_type, &mut name_map))
        .collect::<FsResult<Vec<_>>>()?;

    all_relations.extend(source_relations);

    // Share the sources' single batch metadata call. Query-measured models are
    // excluded, same as sources.
    let sla_model_relations = sla_models
        .iter()
        .filter(|node| {
            node.get_loaded_at_field().is_empty() && node.get_loaded_at_query().is_empty()
        })
        .map(|node| create_relation_and_map(*node, adapter_type, &mut name_map))
        .collect::<FsResult<Vec<_>>>()?;

    all_relations.extend(sla_model_relations);

    // Collect relations from extended models
    let model_relations = extended_models
        .iter()
        .map(|node| create_relation_and_map(*node, adapter_type, &mut name_map))
        .collect::<FsResult<Vec<_>>>()?;

    all_relations.extend(model_relations);

    // Deduplicate relations so each physical table is queried only once.
    // name_map already holds all unique_ids per FQN, so dropping duplicates
    // from the relations list is safe.
    let mut seen = BTreeSet::new();
    all_relations.retain(|r| seen.insert(r.semantic_fqn()));

    Ok((all_relations, name_map))
}

// Helper function to validate and extract timestamp column
fn validate_and_extract_timestamp_column(
    batch: &arrow::record_batch::RecordBatch,
    column_index: usize,
    node: &dyn FreshnessNodeRef,
    error_message: &str,
    io_args: &IoArgs,
) -> FsResult<arrow::array::ArrayRef> {
    match batch.column(column_index).data_type() {
        DataType::Timestamp(_, None) => {
            // No timezone info is attached so we issue a warning as we are about to make some timezone up.
            let err = fs_err!(
                code => ErrorCode::Unexpected,
                loc => node.get_node_path(NodePathKind::Definition, io_args.in_dir.as_path(), io_args.out_dir.as_path()).into_owned(),
                "{} '{}' {} has a timestamp type without a timezone, we will assume a UTC timezone",
                node.kind_label(),
                node.common().unique_id,
                error_message
            );
            emit_warn_log_from_fs_error(*err);

            Ok(batch.column(column_index).clone())
        }
        DataType::Timestamp(_, Some(_)) => Ok(batch.column(column_index).clone()),
        _ => {
            err!(
                code => ErrorCode::Unexpected,
                loc => node.get_node_path(NodePathKind::Definition, io_args.in_dir.as_path(), io_args.out_dir.as_path()).into_owned(),
                "{} '{}' {} should have a timestamp type, but got {} type",
                node.kind_label(),
                node.common().unique_id,
                error_message,
                batch.column(column_index).data_type()
            )
        }
    }
}

/// Whether the selection contains any source that would actually be measured.
///
/// Gates the `sources.json` write: a model-only `dbt freshness --select some_model`
/// would otherwise overwrite a good artifact with an empty one, even though the run
/// measured no sources and the user never asked about them.
///
/// Keyed on what was *selected*, not on what was measured. A source that was selected
/// but produced no result is a different situation, already fatal in
/// `run_freshness_with_spans`, and must still write the artifact rather than silently
/// skip it.
///
/// Also folds in `resource_types`/`exclude_resource_types`: a source that is only
/// present as a frontier node (an upstream dependency of a selected model) is still
/// "selected" here, but `run_freshness` never measures it when `--exclude-resource-type
/// source`/`--resource-type model` rules sources out. Without this check the guard
/// would think a source is in scope, write `sources.json`, and clobber it with empty
/// results anyway.
pub fn selection_contains_source(
    schedule: &Schedule<String>,
    resolver_state: &ResolverState,
    resource_types: &[ClapResourceType],
    exclude_resource_types: &[ClapResourceType],
) -> bool {
    if !resource_type_allowed(
        ClapResourceType::Source,
        resource_types,
        exclude_resource_types,
    ) {
        return false;
    }
    schedule
        .all_selected_nodes
        .iter()
        .chain(schedule.frontier_nodes.iter())
        .any(|unique_id| {
            resolver_state
                .nodes
                .get_node(unique_id)
                .is_some_and(|node| node.resource_type() == NodeType::Source)
        })
}

/// Whether `sources.json` should be (re)written for this invocation.
///
/// `dbt source freshness` (`sources_only`) always writes it, empty results
/// included, matching dbt-core. The unified `dbt freshness` spelling only writes
/// it when [`selection_contains_source`] holds, so a model-only selection — or one
/// where `--exclude-resource-type source` rules sources out entirely — doesn't
/// clobber a good artifact with an empty one.
pub fn should_write_sources_json(
    sources_only: bool,
    schedule: &Schedule<String>,
    resolver_state: &ResolverState,
    resource_types: &[ClapResourceType],
    exclude_resource_types: &[ClapResourceType],
) -> bool {
    sources_only
        || selection_contains_source(
            schedule,
            resolver_state,
            resource_types,
            exclude_resource_types,
        )
}

/// Sources only, `resource_type` unset: `sources.json` keeps the exact shape
/// `dbt source freshness` has always written.
pub fn freshness_results_to_nodes(
    resolver_state: &ResolverState,
    results: &BTreeMap<String, FreshnessResult>,
) -> Vec<FreshnessResultsNode> {
    build_result_nodes(resolver_state, results, true)
}

/// Sources and models, each tagged with its resource type.
pub fn freshness_results_to_freshness_nodes(
    resolver_state: &ResolverState,
    results: &BTreeMap<String, FreshnessResult>,
) -> Vec<FreshnessResultsNode> {
    build_result_nodes(resolver_state, results, false)
}

fn build_result_nodes(
    resolver_state: &ResolverState,
    results: &BTreeMap<String, FreshnessResult>,
    sources_only: bool,
) -> Vec<FreshnessResultsNode> {
    results
        .iter()
        .filter_map(|(unique_id, result)| {
            let node = resolver_state
                .nodes
                .get_node(unique_id)
                .expect("node must exist");
            if sources_only && node.resource_type() != NodeType::Source {
                return None;
            }
            Some(FreshnessResultsNode {
                unique_id: unique_id.clone(),
                resource_type: (!sources_only)
                    .then(|| node.resource_type().as_static_ref().to_string()),
                max_loaded_at: result.max_loaded_at,
                snapshotted_at: result.snapshotted_at,
                max_loaded_at_time_ago_in_s: result.age as f64,
                status: result.status.clone(),
                criteria: node.freshness_criteria(),
                adapter_response: BTreeMap::new(),
                timing: vec![],
                thread_id: format!(
                    "Thread-{}",
                    format!("{:?}", std::thread::current().id())
                        .trim_start_matches("ThreadId(")
                        .trim_end_matches(")")
                ),
                execution_time: 0.0,
                node: None,
            })
        })
        .collect()
}

/// Builds freshness results for the Jinja `on_run_end` context, including the node.
///
/// Unlike `freshness_results_to_nodes` (used for the sources.json artifact), this
/// variant attaches the resolved node so that macros like elementary's
/// `upload_source_freshness` can access `result.node.unique_id` and other node
/// fields. Do not drop `node: Some(..)`.
pub fn freshness_results_to_context(
    resolver_state: &ResolverState,
    results: &BTreeMap<String, FreshnessResult>,
) -> Vec<FreshnessResultsNode> {
    results
        .iter()
        .map(|(unique_id, result)| {
            let node = resolver_state
                .nodes
                .get_node_owned(unique_id)
                .expect("node must exist");
            let criteria = node.freshness_criteria();
            FreshnessResultsNode {
                unique_id: unique_id.clone(),
                resource_type: None,
                max_loaded_at: result.max_loaded_at,
                snapshotted_at: result.snapshotted_at,
                max_loaded_at_time_ago_in_s: result.age as f64,
                status: result.status.clone(),
                criteria,
                adapter_response: BTreeMap::new(),
                timing: vec![],
                thread_id: format!(
                    "Thread-{}",
                    format!("{:?}", std::thread::current().id())
                        .trim_start_matches("ThreadId(")
                        .trim_end_matches(")")
                ),
                execution_time: 0.0,
                node: Some(node),
            }
        })
        .collect()
}

/// Builds the `sources.json` artifact. Kept separate from the write so callers can
/// hand it back in memory even when the write fails.
pub fn build_sources_artifact(
    invocation_id: &uuid::Uuid,
    resolver_state: &ResolverState,
    results: &BTreeMap<String, FreshnessResult>,
) -> FreshnessResultsArtifact {
    build_artifact(
        invocation_id,
        freshness_results_to_nodes(resolver_state, results),
        "https://schemas.getdbt.com/dbt/sources/v3.json",
    )
}

/// Builds the `freshness.json` artifact: the `sources.json` shape plus
/// `resource_type`, covering models as well as sources.
pub fn build_freshness_artifact(
    invocation_id: &uuid::Uuid,
    resolver_state: &ResolverState,
    results: &BTreeMap<String, FreshnessResult>,
) -> FreshnessResultsArtifact {
    build_artifact(
        invocation_id,
        freshness_results_to_freshness_nodes(resolver_state, results),
        "https://schemas.getdbt.com/dbt/freshness/v0.json",
    )
}

fn build_artifact(
    invocation_id: &uuid::Uuid,
    results: Vec<FreshnessResultsNode>,
    dbt_schema_version: &str,
) -> FreshnessResultsArtifact {
    let generated_at: DateTime<Utc> = Utc::now();
    let metadata = FreshnessResultsMetadata {
        dbt_schema_version: dbt_schema_version.to_string(),
        dbt_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at,
        invocation_id: invocation_id.to_string(),
        invocation_started_at: None,
        env: dbt_common::constants::collect_dbt_custom_envs(),
    };

    FreshnessResultsArtifact {
        metadata,
        results,
        elapsed_time: 0.0,
    }
}

pub fn write_sources_json(
    out_dir: &Path,
    in_dir: &Path,
    sources_artifact: &FreshnessResultsArtifact,
) -> FsResult<()> {
    write_freshness_artifact(out_dir, in_dir, sources_artifact, true)
}

/// `sources.json` is still written alongside `freshness.json` for back-compat.
pub fn write_freshness_json(
    out_dir: &Path,
    in_dir: &Path,
    freshness_artifact: &FreshnessResultsArtifact,
) -> FsResult<()> {
    write_freshness_artifact(out_dir, in_dir, freshness_artifact, false)
}

fn write_freshness_artifact(
    out_dir: &Path,
    in_dir: &Path,
    artifact: &FreshnessResultsArtifact,
    sources_only: bool,
) -> FsResult<()> {
    let (file_name, artifact_type, log_message) = if sources_only {
        (
            DBT_SOURCES_JSON,
            ArtifactType::Sources,
            "Successfully wrote sources.json",
        )
    } else {
        (
            DBT_FRESHNESS_JSON,
            ArtifactType::Freshness,
            "Successfully wrote freshness.json",
        )
    };

    let results_path = out_dir.join(file_name);

    let rel_path = pathdiff::diff_paths(&results_path, in_dir)
        .unwrap_or_else(|| results_path.clone())
        .to_string_lossy()
        .into_owned();

    let _sp = create_info_span(ArtifactWritten {
        artifact_type: artifact_type as i32,
        relative_path: rel_path,
    })
    .entered();

    let results_file = File::create(results_path)?;

    serde_json::to_writer(results_file, artifact)?;
    emit_info_log_message(log_message);
    Ok(())
}

pub fn write_freshness_results_parquet(
    io: &IoArgs,
    resolver_state: &ResolverState,
    results: &BTreeMap<String, FreshnessResult>,
) {
    use dbt_metadata_parquet::runtime_freshness::{FreshnessResultRow, write_freshness_results};

    let freshness_dir = default_metadata_dir(&io.out_dir)
        .join("run")
        .join("freshness");

    // Microseconds, matching the column's Arrow type and every sibling producer.
    let ingested_at: i64 = Utc::now().timestamp_micros();

    let rows: Vec<FreshnessResultRow> = results
        .iter()
        .filter_map(|(unique_id, result)| {
            let node = resolver_state.nodes.get_node(unique_id)?;
            let criteria = node.freshness_criteria();
            Some(FreshnessResultRow {
                invocation_id: io.invocation_id.to_string(),
                unique_id: unique_id.clone(),
                resource_type: Some(node.resource_type().as_static_ref().to_string()),
                status: result.status.to_string(),
                max_loaded_at: Some(result.max_loaded_at.to_rfc3339()),
                snapshotted_at: Some(result.snapshotted_at.to_rfc3339()),
                max_loaded_at_time_ago: Some(result.age as f64),
                execution_time: None,
                warn_after_count: criteria
                    .warn_after
                    .as_ref()
                    .and_then(|w| w.count.map(|c| c as i32)),
                warn_after_period: criteria
                    .warn_after
                    .as_ref()
                    .and_then(|w| w.period.as_ref().map(|p| p.to_string())),
                error_after_count: criteria
                    .error_after
                    .as_ref()
                    .and_then(|e| e.count.map(|c| c as i32)),
                error_after_period: criteria
                    .error_after
                    .as_ref()
                    .and_then(|e| e.period.as_ref().map(|p| p.to_string())),
                ingested_at,
            })
        })
        .collect();

    if let Err(e) = write_freshness_results(&freshness_dir, &rows) {
        emit_warn_log_message(
            ErrorCode::IoError,
            format!("Failed to write freshness results parquet: {e}"),
        );
    }
}

pub fn emit_freshness_stats(io_args: &IoArgs, results: &BTreeMap<String, FreshnessResult>) {
    if io_args.should_show(ShowOptions::Stats) {
        let info = results
            .iter()
            .map(|(unique_id, result)| format!("{unique_id} {result:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        emit_info_event(
            ShowResult::new_text(info, "stats", "Source freshness stats"),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::IndexMap;
    use dbt_schemas::schemas::InternalDbtNode;
    use dbt_schemas::schemas::Nodes;
    use dbt_schemas::schemas::common::ModelFreshnessRules;
    use dbt_schemas::schemas::profiles::{DbConfig, SnowflakeDbConfig};
    use dbt_schemas::schemas::properties::ModelFreshness;
    use dbt_schemas::state::{
        DbtProfile, DbtRuntimeConfig, DummyNodeResolverTracker, Macros, Operations, ProfileAdapter,
        RenderResults,
    };

    fn rules(count: i64, period: FreshnessPeriod) -> FreshnessRules {
        FreshnessRules {
            count: Some(count),
            period: Some(period),
        }
    }

    /// Minimal `ResolverState` for tests that only need `nodes` populated.
    fn test_resolver_state_with_nodes(nodes: Nodes) -> ResolverState {
        let user_defined_schema_registry = dbt_schemas::state::hydrate_user_defined_schema_registry(
            &nodes,
            AdapterType::Snowflake,
        );
        ResolverState {
            root_project_name: "test".to_string(),
            adapter_type: AdapterType::Snowflake,
            nodes,
            disabled_nodes: Nodes::default(),
            macros: Macros::default(),
            operations: Operations::default(),
            dbt_profile: {
                let db_config = DbConfig::Snowflake(Box::<SnowflakeDbConfig>::default());
                let default_adapter = db_config.adapter_type();
                let adapters =
                    IndexMap::from([(default_adapter, ProfileAdapter::single(db_config))]);
                DbtProfile {
                    profile: "default".to_string(),
                    target: "dev".to_string(),
                    defer_to_target: None,
                    allow_clones: true,
                    adapters,
                    default_adapter,
                    schema: "dbt_test".to_string(),
                    database: "db".to_string(),
                    relative_profile_path: std::path::PathBuf::new(),
                    threads: None,
                }
            },
            cloud_config: None,
            render_results: RenderResults::default(),
            node_resolver: Arc::new(DummyNodeResolverTracker),
            get_relation_calls: Default::default(),
            get_columns_in_relation_calls: Default::default(),
            patterned_dangling_sources: Default::default(),
            run_started_at: Utc::now().with_timezone(&chrono_tz::UTC),
            runtime_config: Arc::new(DbtRuntimeConfig::default()),
            manifest_path_configs: BTreeMap::new(),
            manifest_selectors: BTreeMap::new(),
            resolved_selectors: Default::default(),
            root_project_quoting: Default::default(),
            defer_nodes: None,
            nodes_with_resolution_errors: Default::default(),
            nodes_with_access_errors: Default::default(),
            semantic_layer_spec_is_legacy: false,
            test_name_truncations: Default::default(),
            user_defined_schema_registry,
        }
    }

    #[test]
    fn selection_contains_source_is_false_with_no_sources_selected() {
        let resolver_state = test_resolver_state_with_nodes(Nodes::default());
        let schedule = Schedule::<String>::default();
        assert!(!selection_contains_source(
            &schedule,
            &resolver_state,
            &[],
            &[]
        ));
    }

    #[test]
    fn selection_contains_source_is_true_when_a_frontier_source_is_present() {
        let mut nodes = Nodes::default();
        let source = source_with_freshness(None);
        let unique_id = source.__common_attr__.unique_id.clone();
        nodes.sources.insert(unique_id.clone(), Arc::new(source));
        let resolver_state = test_resolver_state_with_nodes(nodes);

        let mut schedule = Schedule::<String>::default();
        schedule.frontier_nodes.insert(unique_id);

        assert!(selection_contains_source(
            &schedule,
            &resolver_state,
            &[],
            &[]
        ));
    }

    #[test]
    fn selection_contains_source_is_false_when_frontier_source_is_resource_type_excluded() {
        // A source that is only present as a frontier node (an upstream dependency of
        // a selected model) must not count once `--exclude-resource-type source` (or
        // an equivalent `--resource-type model`) rules it out: `run_freshness` will
        // never measure it, so the guard must not think it's in scope either.
        let mut nodes = Nodes::default();
        let source = source_with_freshness(None);
        let unique_id = source.__common_attr__.unique_id.clone();
        nodes.sources.insert(unique_id.clone(), Arc::new(source));
        let resolver_state = test_resolver_state_with_nodes(nodes);

        let mut schedule = Schedule::<String>::default();
        schedule.frontier_nodes.insert(unique_id);

        assert!(!selection_contains_source(
            &schedule,
            &resolver_state,
            &[],
            &[ClapResourceType::Source],
        ));
        assert!(!selection_contains_source(
            &schedule,
            &resolver_state,
            &[ClapResourceType::Model],
            &[],
        ));
    }

    #[test]
    fn should_write_sources_json_is_always_true_for_source_freshness() {
        // `dbt source freshness` always writes `sources.json`, matching dbt-core,
        // even when the resolved selection contains zero source nodes.
        let resolver_state = test_resolver_state_with_nodes(Nodes::default());
        let schedule = Schedule::<String>::default();

        assert!(!selection_contains_source(
            &schedule,
            &resolver_state,
            &[],
            &[]
        ));
        assert!(should_write_sources_json(
            true,
            &schedule,
            &resolver_state,
            &[],
            &[]
        ));
    }

    #[test]
    fn should_write_sources_json_is_false_for_freshness_with_no_source_selected() {
        // The unified `dbt freshness` spelling keeps the guard: a model-only
        // selection must not clobber a good `sources.json` with an empty one.
        let resolver_state = test_resolver_state_with_nodes(Nodes::default());
        let schedule = Schedule::<String>::default();

        assert!(!should_write_sources_json(
            false,
            &schedule,
            &resolver_state,
            &[],
            &[]
        ));
    }

    #[test]
    fn should_write_sources_json_is_false_when_frontier_source_is_resource_type_excluded() {
        // Regression test for the gap left by `run_freshness`'s resource-type gate:
        // a model that depends on a source still pulls that source in as a frontier
        // node, but `--exclude-resource-type source` means it is never measured. The
        // write guard must follow suit and skip the write, not clobber an existing
        // good `sources.json` with `{"results": []}`.
        let mut nodes = Nodes::default();
        let source = source_with_freshness(None);
        let unique_id = source.__common_attr__.unique_id.clone();
        nodes.sources.insert(unique_id.clone(), Arc::new(source));
        let resolver_state = test_resolver_state_with_nodes(nodes);

        let mut schedule = Schedule::<String>::default();
        schedule.frontier_nodes.insert(unique_id);

        assert!(!should_write_sources_json(
            false,
            &schedule,
            &resolver_state,
            &[],
            &[ClapResourceType::Source],
        ));
        assert!(!should_write_sources_json(
            false,
            &schedule,
            &resolver_state,
            &[ClapResourceType::Model],
            &[],
        ));
    }

    #[test]
    fn source_freshness_writes_sources_json_with_empty_results_when_no_source_selected() {
        // Regression test: `dbt source freshness` must still write an (empty)
        // `sources.json` rather than skip the write, even though the resolved
        // selection contains no source nodes.
        let resolver_state = test_resolver_state_with_nodes(Nodes::default());
        let schedule = Schedule::<String>::default();
        let sources_only = true;

        assert!(should_write_sources_json(
            sources_only,
            &schedule,
            &resolver_state,
            &[],
            &[]
        ));

        let empty_results: BTreeMap<String, FreshnessResult> = BTreeMap::new();
        let artifact =
            build_sources_artifact(&uuid::Uuid::new_v4(), &resolver_state, &empty_results);
        assert!(artifact.results.is_empty());

        let tmp_dir = tempfile::tempdir().unwrap();
        write_sources_json(tmp_dir.path(), tmp_dir.path(), &artifact).unwrap();

        let written = std::fs::read_to_string(tmp_dir.path().join(DBT_SOURCES_JSON)).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            parsed["results"].as_array().unwrap().len(),
            0,
            "sources.json should contain an empty results list, not be skipped"
        );
    }

    fn source_with_freshness(freshness: Option<FreshnessDefinition>) -> DbtSource {
        let mut source = DbtSource::default();
        source.__common_attr__.unique_id = "source.pkg.raw.orders".to_string();
        source.__source_attr__.freshness = freshness;
        source
    }

    fn model_with_freshness(freshness: Option<ModelFreshness>) -> DbtModel {
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.pkg.stg_orders".to_string();
        model.__model_attr__.freshness = freshness;
        model
    }

    #[test]
    fn source_accessors_read_source_attr() {
        let mut source = source_with_freshness(Some(FreshnessDefinition {
            warn_after: Some(rules(12, FreshnessPeriod::hour)),
            filter: Some("id > 0".to_string()),
            ..Default::default()
        }));
        source.__source_attr__.loaded_at_field = Some("updated_at".to_string());

        let node: &dyn FreshnessNodeRef = &source;
        assert_eq!(node.get_loaded_at_field(), "updated_at");
        assert_eq!(node.get_loaded_at_query(), "");
        assert_eq!(node.get_freshness_filter(), Some("id > 0"));
    }

    #[test]
    fn model_accessors_read_model_attr_freshness() {
        let model = model_with_freshness(Some(ModelFreshness {
            warn_after: Some(rules(12, FreshnessPeriod::hour)),
            filter: Some("id > 0".to_string()),
            loaded_at_field: Some("updated_at".to_string()),
            ..Default::default()
        }));

        let node: &dyn FreshnessNodeRef = &model;
        assert_eq!(node.get_loaded_at_field(), "updated_at");
        assert_eq!(node.get_loaded_at_query(), "");
        assert_eq!(node.get_freshness_filter(), Some("id > 0"));
    }

    #[test]
    fn model_accessors_are_empty_when_no_freshness() {
        let model = model_with_freshness(None);
        let node: &dyn FreshnessNodeRef = &model;
        assert_eq!(node.get_loaded_at_field(), "");
        assert_eq!(node.get_loaded_at_query(), "");
        assert_eq!(node.get_freshness_filter(), None);
    }

    #[test]
    fn criteria_for_source_reads_source_attr_freshness() {
        let definition = FreshnessDefinition {
            error_after: Some(rules(2, FreshnessPeriod::day)),
            warn_after: Some(rules(12, FreshnessPeriod::hour)),
            ..Default::default()
        };
        let source = source_with_freshness(Some(definition.clone()));

        assert_eq!(source.freshness_criteria(), definition);
    }

    #[test]
    fn criteria_for_source_without_freshness_is_default() {
        let source = source_with_freshness(None);
        assert_eq!(source.freshness_criteria(), FreshnessDefinition::default());
    }

    #[test]
    fn criteria_for_model_uses_sla_fields_and_ignores_build_after() {
        let model = model_with_freshness(Some(ModelFreshness {
            build_after: Some(ModelFreshnessRules {
                count: Some(1),
                period: Some(FreshnessPeriod::day),
                updates_on: None,
            }),
            warn_after: Some(rules(12, FreshnessPeriod::hour)),
            error_after: Some(rules(2, FreshnessPeriod::day)),
            filter: Some("id > 0".to_string()),
            loaded_at_field: Some("updated_at".to_string()),
            loaded_at_query: None,
        }));

        assert_eq!(
            model.freshness_criteria(),
            FreshnessDefinition {
                error_after: Some(rules(2, FreshnessPeriod::day)),
                warn_after: Some(rules(12, FreshnessPeriod::hour)),
                filter: Some("id > 0".to_string()),
                loaded_at_field: Some("updated_at".to_string()),
                loaded_at_query: None,
            }
        );
    }

    /// These paths used to hard-downcast to `DbtSource` and panic on a model.
    #[test]
    fn criteria_for_model_does_not_panic() {
        let model = model_with_freshness(Some(ModelFreshness {
            warn_after: Some(rules(1, FreshnessPeriod::hour)),
            ..Default::default()
        }));
        let node: &dyn InternalDbtNode = &model;

        assert_eq!(
            node.freshness_criteria(),
            FreshnessDefinition {
                warn_after: Some(rules(1, FreshnessPeriod::hour)),
                ..Default::default()
            }
        );
    }

    #[test]
    fn thresholds_pass_when_age_within_warn() {
        let criteria = FreshnessDefinition {
            warn_after: Some(rules(1, FreshnessPeriod::hour)),
            error_after: Some(rules(2, FreshnessPeriod::hour)),
            ..Default::default()
        };
        let (status, outcome) = evaluate_freshness_thresholds(&criteria, 60).unwrap();
        assert_eq!(status, FreshnessStatus::Pass);
        assert_eq!(outcome, SourceFreshnessOutcome::OutcomePassed);
    }

    #[test]
    fn thresholds_warn_when_age_exceeds_warn_after() {
        let criteria = FreshnessDefinition {
            warn_after: Some(rules(1, FreshnessPeriod::hour)),
            error_after: Some(rules(2, FreshnessPeriod::hour)),
            ..Default::default()
        };
        let (status, outcome) = evaluate_freshness_thresholds(&criteria, 3601).unwrap();
        assert_eq!(status, FreshnessStatus::Warn);
        assert_eq!(outcome, SourceFreshnessOutcome::OutcomeWarned);
    }

    #[test]
    fn thresholds_error_takes_precedence_over_warn() {
        let criteria = FreshnessDefinition {
            warn_after: Some(rules(1, FreshnessPeriod::hour)),
            error_after: Some(rules(2, FreshnessPeriod::hour)),
            ..Default::default()
        };
        let (status, outcome) = evaluate_freshness_thresholds(&criteria, 7201).unwrap();
        assert_eq!(status, FreshnessStatus::Error);
        assert_eq!(outcome, SourceFreshnessOutcome::OutcomeFailed);
    }

    #[test]
    fn thresholds_pass_when_no_rules_set() {
        let (status, outcome) =
            evaluate_freshness_thresholds(&FreshnessDefinition::default(), 999_999).unwrap();
        assert_eq!(status, FreshnessStatus::Pass);
        assert_eq!(outcome, SourceFreshnessOutcome::OutcomePassed);
    }

    fn metadata_source(name: &str) -> DbtSource {
        let mut source = source_with_freshness(Some(FreshnessDefinition {
            warn_after: Some(rules(12, FreshnessPeriod::hour)),
            ..Default::default()
        }));
        source.__common_attr__.unique_id = format!("source.pkg.raw.{name}");
        source.__common_attr__.name = name.to_string();
        source.__base_attr__.database = "db".to_string();
        source.__base_attr__.schema = "sch".to_string();
        source.__base_attr__.alias = name.to_string();
        source
    }

    fn sla_model(name: &str, freshness: ModelFreshness) -> DbtModel {
        let mut model = model_with_freshness(Some(freshness));
        model.__common_attr__.unique_id = format!("model.pkg.{name}");
        model.__common_attr__.name = name.to_string();
        model.__base_attr__.database = "db".to_string();
        model.__base_attr__.schema = "sch".to_string();
        model.__base_attr__.alias = name.to_string();
        model
    }

    fn metadata_sla_model(name: &str) -> DbtModel {
        sla_model(
            name,
            ModelFreshness {
                warn_after: Some(rules(12, FreshnessPeriod::hour)),
                ..Default::default()
            },
        )
    }

    #[test]
    fn collect_relations_batches_sources_and_sla_models_together() {
        let s1 = metadata_source("orders");
        let s2 = metadata_source("customers");
        let m1 = metadata_sla_model("stg_orders");
        let m2 = metadata_sla_model("stg_customers");

        let (relations, name_map) =
            collect_relations(&[&s1, &s2], &vec![&m1, &m2], &vec![], AdapterType::DuckDB).unwrap();

        assert_eq!(relations.len(), 4);
        let mapped: BTreeSet<String> = name_map.values().flatten().cloned().collect();
        assert_eq!(
            mapped,
            BTreeSet::from([
                "source.pkg.raw.orders".to_string(),
                "source.pkg.raw.customers".to_string(),
                "model.pkg.stg_orders".to_string(),
                "model.pkg.stg_customers".to_string(),
            ])
        );
    }

    #[test]
    fn collect_relations_excludes_sla_models_measured_by_query() {
        let field_model = sla_model(
            "by_field",
            ModelFreshness {
                warn_after: Some(rules(12, FreshnessPeriod::hour)),
                loaded_at_field: Some("updated_at".to_string()),
                ..Default::default()
            },
        );
        let query_model = sla_model(
            "by_query",
            ModelFreshness {
                warn_after: Some(rules(12, FreshnessPeriod::hour)),
                loaded_at_query: Some("select max(updated_at) from {{ this }}".to_string()),
                ..Default::default()
            },
        );
        let metadata_model = metadata_sla_model("by_metadata");

        let (relations, name_map) = collect_relations(
            &[],
            &vec![&field_model, &query_model, &metadata_model],
            &vec![],
            AdapterType::DuckDB,
        )
        .unwrap();

        assert_eq!(relations.len(), 1);
        let mapped: BTreeSet<String> = name_map.values().flatten().cloned().collect();
        assert_eq!(
            mapped,
            BTreeSet::from(["model.pkg.by_metadata".to_string()])
        );
    }

    #[test]
    fn collect_relations_with_no_sla_models_is_sources_only() {
        let s1 = metadata_source("orders");

        let (relations, name_map) =
            collect_relations(&[&s1], &vec![], &vec![], AdapterType::DuckDB).unwrap();

        assert_eq!(relations.len(), 1);
        let mapped: BTreeSet<String> = name_map.values().flatten().cloned().collect();
        assert_eq!(
            mapped,
            BTreeSet::from(["source.pkg.raw.orders".to_string()])
        );
    }

    #[test]
    fn collect_relations_keeps_extended_models_separate() {
        let mut extended = DbtModel::default();
        extended.__common_attr__.unique_id = "model.pkg.extended".to_string();
        extended.__common_attr__.name = "extended".to_string();
        extended.__base_attr__.database = "db".to_string();
        extended.__base_attr__.schema = "sch".to_string();
        extended.__base_attr__.alias = "extended".to_string();
        extended.__base_attr__.extended_model = true;

        let (relations, name_map) =
            collect_relations(&[], &vec![], &vec![&extended], AdapterType::DuckDB).unwrap();

        assert_eq!(relations.len(), 1);
        let mapped: BTreeSet<String> = name_map.values().flatten().cloned().collect();
        assert_eq!(mapped, BTreeSet::from(["model.pkg.extended".to_string()]));
    }

    #[test]
    fn early_validation_rejects_partial_rule_on_model() {
        let partial = sla_model(
            "bad_rule",
            ModelFreshness {
                warn_after: Some(FreshnessRules {
                    count: Some(24),
                    period: None,
                }),
                ..Default::default()
            },
        );
        assert!(validate_freshness_rules_early(&partial).is_err());
    }

    #[test]
    fn early_validation_rejects_partial_rule_on_source() {
        let partial = source_with_freshness(Some(FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: None,
                period: Some(FreshnessPeriod::hour),
            }),
            ..Default::default()
        }));
        assert!(validate_freshness_rules_early(&partial).is_err());
    }

    #[test]
    fn early_validation_accepts_well_formed_and_empty_rules() {
        assert!(validate_freshness_rules_early(&metadata_source("ok")).is_ok());
        assert!(validate_freshness_rules_early(&metadata_sla_model("ok_model")).is_ok());

        // Empty rule object == omitted (F1), so it must not be rejected.
        let empty = source_with_freshness(Some(FreshnessDefinition {
            warn_after: Some(FreshnessRules::default()),
            ..Default::default()
        }));
        assert!(validate_freshness_rules_early(&empty).is_ok());
    }

    /// `dbt source freshness` must keep the exact string it shipped with.
    #[test]
    fn nothing_to_do_message_is_unchanged_for_source_freshness() {
        assert_eq!(
            nothing_to_do_message(true),
            "Nothing to do. No sources with freshness config."
        );
    }

    #[test]
    fn nothing_to_do_message_mentions_models_for_unified_freshness() {
        assert_eq!(
            nothing_to_do_message(false),
            "Nothing to do. No sources or models with a freshness config."
        );
    }

    #[test]
    fn thresholds_error_on_partial_rule() {
        let criteria = FreshnessDefinition {
            warn_after: Some(FreshnessRules {
                count: Some(1),
                period: None,
            }),
            ..Default::default()
        };
        assert!(evaluate_freshness_thresholds(&criteria, 10).is_err());
    }

    #[test]
    fn source_without_any_freshness_config_is_collected() {
        // No `freshness` anywhere in the project resolves to an empty rule, not to `None`
        // (`merge_freshness_unwrapped`, META-5461). These sources are the ones state-aware
        // orchestration tracks off `INFORMATION_SCHEMA.TABLES.LAST_ALTERED`, so they must keep
        // being collected — skipping them would make every dependent model rebuild every run.
        let node = source_with_freshness(Some(FreshnessDefinition::default()));
        assert!(collects_freshness_during_build(&node));
    }

    #[test]
    fn source_with_freshness_rules_is_collected() {
        let node = source_with_freshness(Some(FreshnessDefinition {
            warn_after: Some(FreshnessRules {
                count: Some(12),
                period: Some(FreshnessPeriod::hour),
            }),
            ..Default::default()
        }));
        assert!(collects_freshness_during_build(&node));
    }

    #[test]
    fn source_opted_out_with_freshness_null_is_skipped() {
        // `config: freshness: null` — dbt-labs/dbt-core#14534. Nothing to evaluate, so the
        // source is dropped before any metadata query is issued.
        let node = source_with_freshness(None);
        assert!(!collects_freshness_during_build(&node));
    }

    #[test]
    fn source_opted_out_but_with_loaded_at_field_is_collected() {
        let mut node = source_with_freshness(None);
        node.__source_attr__.loaded_at_field = Some("updated_at".to_string());
        assert!(collects_freshness_during_build(&node));
    }

    #[test]
    fn source_opted_out_but_with_loaded_at_query_is_collected() {
        let mut node = source_with_freshness(None);
        node.__source_attr__.loaded_at_query = Some("select max(updated_at) from t".to_string());
        assert!(collects_freshness_during_build(&node));
    }

    #[test]
    fn empty_loaded_at_strings_do_not_count_as_configured() {
        // `apply_freshness_loaded_at_override` writes `Some("")` for the peer field when only
        // one of `loaded_at_field` / `loaded_at_query` is set, so an empty string must read as
        // "unset" here.
        let mut node = source_with_freshness(None);
        node.__source_attr__.loaded_at_field = Some(String::new());
        node.__source_attr__.loaded_at_query = Some(String::new());
        assert!(!collects_freshness_during_build(&node));
    }
}
