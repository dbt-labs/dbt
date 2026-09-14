use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use dbt_adapter::relation::create_relation_from_node;
use dbt_adapter::{Adapter, AdapterStore};
use dbt_adapter_core::AdapterType;
use dbt_common::FsError;
use dbt_common::collections::DashMap;
use dbt_common::node_selector::SelectExpression;
use dbt_dag::schedule::Schedule;
use dbt_frontend_common::sources_extractor::SourcesExtractor;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::RenderingEventListenerFactory;
use dbt_schema_store::{DataStoreTrait, SchemaStoreTrait};
use dbt_schemas::schemas::profiles::Execute;
use dbt_schemas::state::ResolverState;
use dbt_state::explain::{
    StateExplainLogRecord, StateExplainRunConfig, StateExplainRunStart,
    append_state_explain_log_record, new_state_explain_log_path, prune_state_explain_logs,
};
use dbt_state::service_config::RunCacheServiceConfig;
use dbt_state::view_traversal::ViewDefinitionTraverser;
use petgraph::graph::DiGraph;

use crate::CompiledSqlCache;
use crate::PreTaskRunData;
use crate::RunTasksArgs;
use crate::context::{ExtendedCtx, RunCacheCtx, TaskRunnerCtx, TaskRunnerCtxInner};
use crate::run_cache_lifecycle::{RunCacheLifecycle, RunCacheServiceLifecycle};
use crate::span_manager::SpanManager;
use crate::static_analysis_buckets::StaticAnalysisBuckets;
use crate::task::Task;
use crate::task_spans::populate_span_manager;
use crate::test_aggregation::GenericTestRelationships;

/// Abstract [TaskRunnerCtx] factory.
pub trait TaskRunnerCtxFactory: Send + Sync + 'static {
    fn rendering_listener_factory(&self) -> Arc<dyn RenderingEventListenerFactory>;
    fn sources_extractor(&self) -> Arc<dyn SourcesExtractor>;

    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn build(
        self: Arc<Self>,
        run_task_args: Arc<RunTasksArgs>,
        worker_id: String,
        resolver_state: Arc<ResolverState>,
        extended_ctx_factory: Box<dyn ExtendedTaskRunnerCtxFactory>,
        generic_test_relationships: GenericTestRelationships,
        graph: &DiGraph<Arc<dyn Task>, ()>,
        schema_store: Arc<dyn SchemaStoreTrait>,
        data_store: Arc<dyn DataStoreTrait>,
        compiled_sql_cache: Arc<dyn CompiledSqlCache>,
        mut base_context: BTreeMap<String, minijinja::Value>,
        schedule: Schedule<String>,
        jinja_env: Arc<JinjaEnv>,
        freshness_results: Option<Box<dyn PreTaskRunData>>,
        static_analysis_buckets: Arc<dyn StaticAnalysisBuckets>,
        adapter: Arc<Adapter>,
        adapter_store: Arc<AdapterStore>,
        run_cache_lifecycle: Arc<RunCacheLifecycle>,
    ) -> Pin<Box<dyn Future<Output = Result<TaskRunnerCtx, Box<FsError>>> + Send>> {
        let rendering_listener_factory = self.rendering_listener_factory();
        let span_manager = Arc::new({
            let sm = SpanManager::new_empty();
            let _ = populate_span_manager(
                &sm,
                graph,
                run_task_args.io.in_dir.as_ref(),
                run_task_args.io.out_dir.as_ref(),
                &schedule.selected_nodes,
            );
            sm
        });
        let adhoc_runner = extended_ctx_factory.adhoc_runner(
            Arc::clone(&jinja_env),
            resolver_state.adapter_type,
            Arc::clone(&run_task_args),
            resolver_state.root_project_name.clone(),
        );
        Box::pin(async move {
            let execute = Execute::from_compute_flag(run_task_args.local_execution_backend);

            let extended_ctx = extended_ctx_factory
                .build(run_cache_lifecycle.is_requested())
                .await?;

            let node_hashes = self
                .build_node_hashes(
                    run_task_args.as_ref(),
                    &schedule,
                    &worker_id,
                    resolver_state.as_ref(),
                    &jinja_env,
                    freshness_results.as_deref(),
                    extended_ctx.as_ref(),
                )
                .await?;

            base_context.insert(
                "selected_resources".to_string(),
                minijinja::Value::from_iter(schedule.selected_nodes.iter().cloned()),
            );

            let schema_cache = schema_store as Arc<dyn SchemaStoreTrait>;

            let run_cache_deferred_fqns = static_analysis_buckets
                .deferred_unique_ids()
                .values()
                .filter_map(|unique_id| {
                    let node = resolver_state.get_defer_node_by_id(unique_id)?;
                    let relation =
                        create_relation_from_node(resolver_state.adapter_type, node, None).ok()?;
                    Some(relation.semantic_fqn())
                })
                .collect();

            let run_cache_service = &run_cache_lifecycle.service;
            let run_cache_metadata = run_cache_lifecycle.metadata.clone();

            let state_explain_log_path =
                state_explain_log_path_for_run(run_cache_service, run_task_args.as_ref());
            if let (Some(path), Some(config)) = (
                state_explain_log_path.as_ref(),
                run_cache_service.config.as_ref(),
            ) {
                write_state_explain_run_start(path, run_task_args.as_ref(), config);
                prune_state_explain_logs(path, config);
            }

            let sources_extractor = self.sources_extractor();
            let run_cache_ctx = RunCacheCtx {
                run_cache_metadata,
                run_cache_dev_cloned_nodes: DashMap::default(),
                run_cache_deferred_fqns,
                run_cache_service_requested: run_cache_service.requested,
                run_cache_service_config: run_cache_service.config.clone(),
                run_cache_service_client: run_cache_service.client.clone(),
                state_explain_log_path,
                view_traverser: adapter.metadata_adapter().map(|metadata_adapter| {
                    Arc::new(ViewDefinitionTraverser::new(
                        Arc::from(metadata_adapter),
                        Arc::clone(&sources_extractor),
                    ))
                }),
                heuristic_clock: std::sync::OnceLock::new(),
                prefetch: Default::default(),
                telemetry_event_order: std::sync::atomic::AtomicI64::new(0),
                telemetry_session_start: std::sync::OnceLock::new(),
                telemetry_session_ended: std::sync::atomic::AtomicBool::new(false),
                telemetry_dispatcher: std::sync::OnceLock::new(),
            };

            Ok(TaskRunnerCtx {
                inner: Arc::new(TaskRunnerCtxInner::new(
                    run_task_args,
                    worker_id,
                    schedule,
                    base_context,
                    node_hashes,
                    extended_ctx,
                    compiled_sql_cache,
                    adhoc_runner,
                    &resolver_state,
                    generic_test_relationships,
                    span_manager,
                    execute,
                    adapter_store,
                    sources_extractor,
                    run_cache_ctx,
                )),
                schema_cache,
                data_store,
                resolver_state,
                rendering_listener_factory,
                env: jinja_env,
                thread_id: 0,
            })
        })
    }

    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn build_node_hashes<'a>(
        &'a self,
        arg: &'a RunTasksArgs,
        schedule: &'a Schedule<String>,
        worker_id: &'a str,
        resolver_state: &'a ResolverState,
        env: &'a Arc<JinjaEnv>,
        freshness_results: Option<&'a dyn PreTaskRunData>,
        extended_ctx: &'a dyn ExtendedCtx,
    ) -> Pin<Box<dyn Future<Output = Result<DashMap<String, String>, Box<FsError>>> + Send + 'a>>;
}

/// Only the dbt State dispatch path appends node records, so a log created without it
/// would stay empty and mask the newest log that has records.
fn state_explain_log_path_for_run(
    service: &RunCacheServiceLifecycle,
    run_task_args: &RunTasksArgs,
) -> Option<PathBuf> {
    if !service.requested {
        return None;
    }
    let config = service.config.as_ref()?;
    if !config.enable_response_logging {
        return None;
    }
    Some(new_state_explain_log_path(
        run_task_args.io.in_dir.as_path(),
        run_task_args.io.log_path.as_deref(),
        config,
    ))
}

fn write_state_explain_run_start(
    path: &Path,
    run_task_args: &RunTasksArgs,
    config: &RunCacheServiceConfig,
) {
    let record = StateExplainLogRecord::RunStart(StateExplainRunStart {
        start_timestamp_utc: chrono::Utc::now().to_rfc3339(),
        run_config: StateExplainRunConfig {
            org_id: config.org_id.clone(),
            defer_to_target: config.defer_to.clone(),
            freshness_tolerance_seconds: config.freshness_tolerance_seconds,
            tolerate_nondeterminism: config.tolerate_nondeterminism,
            clone_incremental_in_dev: config.clone_incremental_in_dev.as_str().to_string(),
            metadata_cache_ttl_seconds: config.metadata_cache_ttl_seconds,
            snowflake_get_view_ddl_override: config.snowflake_get_view_ddl_override.clone(),
            profile_name: run_task_args.resolved_profile.clone(),
            target_name: run_task_args.resolved_target.clone(),
            select: selector_arg(&run_task_args.select),
            exclude: selector_arg(&run_task_args.exclude),
        },
    });
    if let Err(err) = append_state_explain_log_record(path, &record) {
        tracing::warn!("Failed to write dbt State explain run_start record: {err}");
    }
}

fn selector_arg(selector: &Option<SelectExpression>) -> Vec<String> {
    match selector.as_ref() {
        Some(SelectExpression::Or(expressions)) => {
            expressions.iter().map(ToString::to_string).collect()
        }
        Some(selector) => vec![selector.to_string()],
        None => Vec::new(),
    }
}

/// Abstract factory for building the extended context, which is a component of [TaskRunnerCtx].
pub trait ExtendedTaskRunnerCtxFactory: Send + Sync {
    fn adhoc_runner(
        &self,
        env: Arc<JinjaEnv>,
        adapter_type: AdapterType,
        args: Arc<RunTasksArgs>,
        root_project_name: String,
    ) -> Arc<dyn crate::AdhocRunner>;

    #[allow(clippy::type_complexity)]
    fn build(
        self: Box<Self>,
        run_cache_enabled: bool,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn ExtendedCtx>, Box<FsError>>> + Send>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::node_selector::{MethodName, SelectExpression, SelectionCriteria};

    #[test]
    fn write_state_explain_run_start_captures_run_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("state-explain.jsonl");
        let mut args = RunTasksArgs {
            resolved_profile: "jaffle_shop".to_string(),
            resolved_target: "dev".to_string(),
            select: Some(SelectExpression::Or(vec![
                selector("orders"),
                selector("customers"),
            ])),
            exclude: Some(selector("customers")),
            ..Default::default()
        };
        args.io.in_dir = temp_dir.path().to_path_buf();
        let mut config = RunCacheServiceConfig::disabled();
        config.org_id = Some("org-1".to_string());
        config.defer_to = "prod".to_string();
        config.clone_incremental_in_dev = dbt_state::service_config::CloneIncrementalInDev::Always;

        write_state_explain_run_start(&path, &args, &config);

        let log = dbt_state::explain::read_state_explain_log(&path).unwrap();
        let run_config = log.run_start.unwrap().run_config;
        assert_eq!(run_config.org_id.as_deref(), Some("org-1"));
        assert_eq!(run_config.defer_to_target, "prod");
        assert_eq!(run_config.clone_incremental_in_dev, "ALWAYS");
        assert_eq!(run_config.profile_name, "jaffle_shop");
        assert_eq!(run_config.target_name, "dev");
        assert_eq!(run_config.select, ["fqn:orders", "fqn:customers"]);
        assert_eq!(run_config.exclude, ["fqn:customers"]);
    }

    #[test]
    fn state_explain_log_path_requires_dispatched_state_service() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut args = RunTasksArgs::default();
        args.io.in_dir = temp_dir.path().to_path_buf();

        assert!(
            state_explain_log_path_for_run(&service_lifecycle(false, true), &args).is_none(),
            "an unrequested service would only create an empty log"
        );
        assert!(
            state_explain_log_path_for_run(&service_lifecycle(true, false), &args).is_none(),
            "response logging is opt-out"
        );
        assert!(state_explain_log_path_for_run(&service_lifecycle(true, true), &args).is_some());
    }

    fn service_lifecycle(
        requested: bool,
        enable_response_logging: bool,
    ) -> RunCacheServiceLifecycle {
        let mut config = RunCacheServiceConfig::disabled();
        config.enable_response_logging = enable_response_logging;
        RunCacheServiceLifecycle {
            requested,
            config: Some(config),
            client: None,
        }
    }

    fn selector(value: &str) -> SelectExpression {
        SelectExpression::Atom(SelectionCriteria::new(
            MethodName::Fqn,
            Vec::new(),
            value.to_string(),
            false,
            None,
            None,
            None,
            None,
        ))
    }
}
