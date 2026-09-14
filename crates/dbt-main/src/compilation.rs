use crate::freshness::run_freshness_command;
use crate::{dbt_lib::write_catalog_json, version_check};
use arrow::datatypes::SchemaRef;
use dbt_adapter::{
    Adapter, adapter::AdapterFactory, relation::create_relation, sql_types::TypeOpsFactory,
};
use dbt_adapter_core::AdapterType;
use dbt_clap_core::{Cli, Command, CompileArgs, CoreCommand, ShowArgs};
use dbt_common::{
    create_info_span,
    static_analysis::{StaticAnalysisDeprecationOrigin, check_deprecated_static_analysis_kind},
    stats::NodeStatus,
    tracing::{
        dbt_metrics::{FusionMetricKey, return_exit_code_from_error_counter},
        metrics::increment_metric,
    },
};
use dbt_compilation::{
    core::{AdapterConnectionMode, DbtLoadedProject},
    schema_hydration::SchemaHydrationState,
};
use dbt_dag::{deps_mgmt::reverse, schedule::Schedule};
use dbt_defer::DeferState;
use dbt_features::feature_stack::FeatureStack;
use dbt_features::index::write_metadata_parquet;
use dbt_index_core::ingest::ingest_state::IngestState;
use dbt_index_core::ingest::metadata_to_parquet::{
    has_persisted_state, ingest_from_metadata_direct,
};
use dbt_index_core::{WriteSource, save_artifact_meta, write_info_schema};
use dbt_jinja_utils::{
    JinjaFactory,
    invocation_args::InvocationArgs,
    jinja_environment::JinjaEnv,
    listener::JinjaTypeCheckingEventListenerFactory,
    node_resolver::{NodeResolver, PackageSearchOrder},
    phases::{build_operation_context_btreemap, configure_compile_and_run_jinja_environment},
};
use dbt_loader::args::*;
use dbt_parser::args::ResolveArgs;
use dbt_scheduler::args::SchedulerArgs;
use dbt_schema_store::{
    SchemaStoreTrait,
    store::{DataStore, SchemaStore},
};
use dbt_schemas::schemas::selection_override::resolve_selection_override;
use dbt_tasks_core::{
    CompiledSqlCache, RunTaskResults,
    local_schema_builder::{init_data_store, init_schema_store},
    metricflow::MetricflowClient,
    run_cache::run_cache_service::run_cache_service_after_run_failed,
    run_cache_lifecycle::RunCacheLifecycle,
    static_analysis_buckets::{StaticAnalysisBuckets, build_refresh_intervals},
    utils::{build_run_results_artifact, write_run_results_json, write_run_results_json_or_warn},
};

use dbt_common::{
    DiscreteEventEmitter, ErrorCode, FsResult,
    artifact_io::write_artifact_to_file,
    cancellation::CancellationToken,
    constants::{DBT_MANIFEST_JSON, DBT_SEMANTIC_MANIFEST_JSON},
    fail_fast::FailFast,
    fs_err,
    io_args::{
        EvalArgs, EvalArgsBuilder, ListOutputFormat, Phases, ShowOptions,
        optimize_test_defaults_from_project_flags, resolve_effective_optimize_tests,
    },
    io_utils::{checkpoint_error_count_maybe_exit, checkpoint_maybe_exit},
    path::DbtPath,
    tracing::dbt_emit::{emit_info_log_message, emit_warn_log_message},
    tracing::emit::emit_info_event,
};
use dbt_common::{
    io_args::FsCommand, node_selector::selectors_require_manifest, stats::Stat, stdfs,
};
use dbt_metadata::file_registry::CompleteStateWithKind;
use dbt_schemas::{
    filter::RunFilter,
    schemas::{
        DbtCommandExecutionArtifacts, Nodes, ResolvedCloudConfig, common::ResolvedQuoting,
        manifest::DbtManifestV12, profiles::Execute, project::DbtProject,
        semantic_layer::semantic_manifest::SemanticManifest,
    },
    state::{CacheState, DbtPackage, DbtState, Macros, ModelStatus, ResolverState},
    stats::Stats,
};
use dbt_tasks_core::task_runner_hooks::TaskRunnerHooksFactory;
use dbt_tasks_sa::utils::{
    update_columns_from_schemas, update_resolved_states_manifest_with_schemas_and_compiled_sql,
};
use dbt_tasks_sa::{
    base_context::build_base_context,
    compilation_pipeline::{
        self, schedule as build_schedule_from_resolved, schedule_with_cache_state,
        schedule_with_select, schedule_with_unique_ids,
    },
};
use dbt_tasks_sa::{compiled_sql_cache::CompiledSqlCacheImpl, task_runner::TaskRunner};
use dbt_tasks_sa::{
    constraints::render_all_model_constraint_refs_in_place,
    run_operation::{INLINE_SQL_NAME, run_operation, run_operation_inline_sql},
};
use dbt_tasks_sa::{graph::GraphBuilder, utils::typecheck_macros};
use dbt_telemetry::{ExecutionPhase, NodeType, PhaseExecuted, ShowResult};

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    num::NonZeroUsize,
    path::PathBuf,
    sync::OnceLock,
};
use tracing::Instrument;
use vortex_events::{adapter_info_event, resource_counts_event};

use dbt_schemas::schemas::{
    InternalDbtNode, InternalDbtNodeAttributes, OnManifestLoadFailure, StateArtifacts,
    legacy_catalog::DbtCatalog, manifest::build_manifest,
};

use dbt_compilation::config::CompilationConfig;
use dbt_tasks_core::RunTasksArgs;
use dbt_tasks_sa::debug::DebugArgs;

use dbt_telemetry::{ArtifactType, ListItemOutput};

use std::{sync::Arc, time::SystemTime};

use crate::utils::update_manifest_with_macro_depends_on;
use dbt_common::io_args::LocalExecutionBackendKind;
use dbt_schemas::state::NodeResolverTracker;
use serde_json::to_string_pretty;

fn should_skip_tasks_when_no_selected_nodes(
    command: &FsCommand,
    schedule: &Schedule<String>,
) -> bool {
    if !schedule.selected_nodes.is_empty() {
        return false;
    }

    // Mantle short-circuits when there's nothing to process; match that by skipping work that would
    // otherwise eagerly run on-run-start hooks (which can trigger adapter calls).
    matches!(
        command,
        FsCommand::Run | FsCommand::Test | FsCommand::Build | FsCommand::Seed | FsCommand::Snapshot
    )
}

struct CompilationPhasesExecutor<'a> {
    arg: Cow<'a, EvalArgs>,
    cli: Cow<'a, Cli>,
    lazy_dbt_manifest: OnceLock<DbtManifestV12>,
    catalog_artifact: Option<DbtCatalog>,
    token: CancellationToken,
}

/// Whether this `dbt show` reads `target/info_schema/` instead of the warehouse.
///
/// Thin wrapper so the three call sites read as one decision: the predicate itself
/// lives in `dbt-tasks-sa` because it needs the Jinja parser, and that crate cannot
/// take a `ShowArgs` without depending on `dbt-clap-core`.
pub(crate) fn show_queries_info_schema(show_args: &ShowArgs) -> bool {
    dbt_tasks_sa::show_info::queries_info_schema(
        show_args.info.as_deref(),
        show_args.inline.as_deref(),
    )
}

impl<'a> CompilationPhasesExecutor<'a> {
    pub fn new(arg: Cow<'a, EvalArgs>, cli: Cow<'a, Cli>, token: CancellationToken) -> Self {
        Self {
            arg,
            cli,
            lazy_dbt_manifest: OnceLock::new(),
            catalog_artifact: None,
            token,
        }
    }
}

impl<'a> CompilationPhasesExecutor<'a> {
    /// Load the project, validate configuration, and override [EvalArgs].
    ///
    /// `self` if `&mut` because the `self.arg` is re-written to implement special
    /// invocation behaviors.
    async fn load(
        &mut self,
        feature_stack: &Arc<FeatureStack>,
        config: CompilationConfig,
        maybe_prev_compilation: &Option<Arc<DbtProjectCompilation>>,
        maybe_event_emitter: Option<&dyn DiscreteEventEmitter>,
        version_check_handle: &mut Option<tokio::task::JoinHandle<Option<String>>>,
    ) -> FsResult<(DbtLoadedProject, Option<CacheState>)> {
        use CoreCommand::*;

        let inline_sql = match &self.cli.command {
            Command::Core(Compile(CompileArgs {
                inline: Some(sql), ..
            })) => Some(sql.clone()),
            // Info-schema show queries DuckDB over `target/info_schema/`; injecting
            // the SQL as a warehouse model would bind `dbt.<view>` on the project adapter.
            Command::Core(Show(show_args))
                if show_args.inline.is_some() && !show_queries_info_schema(show_args) =>
            {
                show_args.inline.clone()
            }
            // `show --query-id <id>`: no SQL to compile at all -- this is a
            // direct result-fetch, not a query. A placeholder gets the
            // inline-node machinery (loader/Showable) to build the same
            // ephemeral node `--inline` uses, so `run_adhoc.rs` has a node to
            // execute against; it's never actually rendered/run as SQL,
            // since `eval_args.query_id` being set makes `run_adhoc.rs` set
            // `RESULT_QUERY_ID` and fetch instead of compiling this string.
            Command::Core(Show(ShowArgs {
                query_id: Some(_), ..
            })) => Some("select 1".to_string()),
            _ => None,
        };
        let has_inline = inline_sql.is_some();
        let common_args = self.cli.common_args();
        let load_args = LoadArgs::from_eval_args(self.arg.as_ref())
            .with_inline_sql(inline_sql)
            .with_cli_warn_error(common_args.get_warn_error())
            .with_cli_warn_error_options(common_args.warn_error_options);
        // These are the initial invocation args before project flags are read.
        let invocation_args = InvocationArgs::from_eval_args(self.arg.as_ref());

        let maybe_prev_loaded_project = maybe_prev_compilation
            .as_ref()
            .map(|x| x.loaded_project.as_ref());

        let loaded_project = DbtLoadedProject::load(
            config,
            load_args,
            Cow::Borrowed(&invocation_args),
            Arc::clone(&feature_stack.adapter.type_ops_factory),
            Arc::clone(&feature_stack.adapter.adapter_factory),
            maybe_prev_loaded_project,
            Some(feature_stack.tracing.config_provider.as_ref()),
            &self.token,
            feature_stack.loader.hooks.clone(),
            feature_stack.loader.private_package_resolver.clone(),
            feature_stack.jinja.factory.clone(),
        )
        .await?;
        self.token.check_cancellation()?;

        let config = loaded_project.config();
        let dbt_state = loaded_project.dbt_state();
        let arg = EvalArgsBuilder::from_eval_args(self.arg.as_ref())
            .with_warn_error_options(dbt_state.warn_error, dbt_state.warn_error_options.clone())
            .build();
        let arg = set_eval_args_threads_and_target(&arg, &dbt_state);
        set_runtime_max_parallelism(&arg, &dbt_state);
        self.arg = Cow::Owned(arg);
        // --inline warm path: run load_cache normally to detect changed files.
        // On NoFilesChanged, build a synthetic all-unimpacted CacheState so the resolver
        // only processes the inline SQL file (avoids 7+ second full re-resolve).
        // On actual file changes, the real CacheState flows through unchanged — changed nodes
        // are re-resolved and the inline file is processed on top.
        let cache_prev = maybe_prev_compilation
            .as_ref()
            .map(|x| (x.loaded_project.as_ref(), &x.resolved_state));
        let build_cache_changes = if has_inline {
            match loaded_project
                .load_cache(self.arg.command, &self.arg.io, cache_prev, &self.token)
                .await
            {
                Ok(cache) => cache,
                Err(e) if e.code == ErrorCode::NoFilesChanged => {
                    if let Some(prev) = maybe_prev_compilation {
                        let prev_state =
                            dbt_metadata::parse_cache::PreviousResolvedState::from_resolved_state(
                                &prev.resolved_state,
                            );
                        // Empty changed_files_list: all prev nodes are unimpacted.
                        // The inline file is in model_sql_files but NOT in all_paths, so it
                        // is not in unimpacted_files and the resolver picks it up as new.
                        dbt_metadata::parse_cache::determine_cache_state_from_previous_previous_resolved_nodes(
                            &self.arg.io,
                            prev_state,
                            &[],
                            &loaded_project.dbt_state(),
                        )?
                    } else {
                        None
                    }
                }
                Err(e) => return Err(e),
            }
        } else {
            loaded_project
                .load_cache(self.arg.command, &self.arg.io, cache_prev, &self.token)
                .await?
        };
        self.token.check_cancellation()?;

        feature_stack.cli.hooks.will_validate_compilation_cli_args(
            self.cli.as_ref(),
            &mut self.arg,
            &dbt_state,
            config,
        )?;
        if let Some(static_analysis) = self.arg.static_analysis {
            check_deprecated_static_analysis_kind(
                static_analysis,
                StaticAnalysisDeprecationOrigin::CliArg,
                None,
            );
        }
        self.token.check_cancellation()?;

        *version_check_handle = spawn_version_check_if_possible(
            config,
            self.arg.local_execution_backend,
            !feature_stack.cli.version_check_enabled,
            feature_stack.cli.command_name,
        );
        self.token.check_cancellation()?;

        if self.arg.io.should_show(ShowOptions::InputFiles) {
            emit_info_event(
                ShowResult::new_text(dbt_state.to_string(), "input_files", "Input files"),
                None,
            );
        }
        self.token.check_cancellation()?;

        // Handle 'debug' or 'init' commands to run debug.
        if let FsCommand::Debug | FsCommand::Init = self.arg.command {
            let mut debug_args = DebugArgs::from_eval_args(self.arg.as_ref());
            debug_args.lake_compute_propagation_checker =
                feature_stack.lake_compute.propagation_checker.clone();
            debug_args.lake_compute_catalog_attach_checker =
                feature_stack.lake_compute.catalog_attach_checker.clone();
            debug_args.mdls_checker = feature_stack.lake_compute.mdls_checker.clone();
            compilation_pipeline::loaded_project::debug(&loaded_project, debug_args, &self.token)
                .await?;
            self.token.check_cancellation()?;
        }

        send_vortex_telemetry_if_possible(self.arg.as_ref(), &dbt_state, maybe_event_emitter);
        self.token.check_cancellation()?;

        // This also exits the init command b/c init `to_eval_args` sets the phase to debug
        // 'debug' < 'deps'
        checkpoint_maybe_exit(self.arg.as_ref(), Phases::Deps)?;
        self.token.check_cancellation()?;

        Ok((loaded_project, build_cache_changes))
    }

    /// Emit Vortex resource_counts if instrumentation is enabled
    fn resource_counts_event(&self, resolved_state: &ResolverState, catalog_count: i32) {
        if self.arg.send_anonymous_usage_stats {
            let invocation_args = InvocationArgs::from_eval_args(self.arg.as_ref());
            resource_counts_event(invocation_args, resolved_state, catalog_count);
        }
    }

    fn memoized_manifest<'b>(
        &'b self,
        invocation_id: &'b str,
        resolved_state: &'b ResolverState,
    ) -> &'b DbtManifestV12 {
        self.lazy_dbt_manifest
            .get_or_init(|| build_manifest(invocation_id, resolved_state))
    }

    fn run_verify_partial_parse(
        &self,
        type_ops_factory: Arc<dyn TypeOpsFactory>,
        adapter_factory: Arc<dyn AdapterFactory>,
        jinja_factory: Arc<dyn JinjaFactory>,
    ) {
        let config = CompilationConfig {
            use_build_cache_for_scheduling: true,
            cacheable_commands: vec![
                FsCommand::Parse,
                FsCommand::Compile,
                FsCommand::Run,
                FsCommand::Test,
                FsCommand::Extension("lineage"),
                FsCommand::Seed,
            ],
            disable_local_compute_checks: false,
            use_resolver_state_deps: false,
            no_version_check: self.cli.common_args.no_version_check,
            use_full_schema_store: false,
        };
        match try_load_prev_compilation(
            &self.arg,
            &config,
            self.cli.as_ref(),
            type_ops_factory,
            adapter_factory,
            jinja_factory,
        ) {
            (PrevCompilationResult::Incremental(_), _) => {
                emit_info_log_message(
                    "verify-partial-parse: round-trip PASSED — state deserialized successfully",
                );
            }
            (PrevCompilationResult::FullParse | PrevCompilationResult::None, _) => {
                emit_warn_log_message(
                    ErrorCode::Generic,
                    "verify-partial-parse: round-trip FAILED — deserialization did not reconstruct valid state",
                );
            }
        }
    }

    /// Whether the parse-time check gate will run, which decides whether the parse metadata
    /// epochs have to be written even with the index off.
    ///
    /// Must agree with the gate's own condition in `dbt_lib::execute_all_phases`. It is stated
    /// twice because the two sites answer it at different times — here, before the epochs are
    /// written; there, once the schedule exists — and the epochs must not be skipped for a gate
    /// that then runs and finds nothing to read.
    ///
    /// `nodes.checks` is enabled-only (`resolve_checks` inserts a node only when enabled), so an
    /// empty map means the gate has nothing to run and this write can be skipped.
    fn parse_time_checks_will_run(&self, resolved_state: &ResolverState) -> bool {
        matches!(self.arg.command, FsCommand::Build | FsCommand::Check)
            && !self.arg.skip_checks
            && !resolved_state.nodes.checks.is_empty()
    }

    #[allow(clippy::too_many_arguments)]
    async fn maybe_write_json_and_exit(
        &mut self,
        feature_stack: &Arc<FeatureStack>,
        loaded_project: &DbtLoadedProject,
        build_cache_changes: Option<&CacheState>,
        resolved_state: &ResolverState,
        _file_kind_registry: &CompleteStateWithKind,
        semantic_manifest: &SemanticManifest,
        invocation_id: &str,
        jinja_env: &Arc<JinjaEnv>,
    ) -> FsResult<()> {
        if self.arg.write_json {
            // Write semantic manifest
            if !resolved_state.semantic_layer_spec_is_legacy {
                write_artifact_to_file(
                    &semantic_manifest,
                    ArtifactType::SemanticManifest,
                    &self.arg.io.out_dir,
                    DBT_SEMANTIC_MANIFEST_JSON,
                    &self.arg.io.in_dir,
                )?;
            }

            // For parse, load/resolve is the final phase and this is the only manifest write.
            // Other commands write an updated manifest later in execute_all_phases.
            if self.arg.command == FsCommand::Parse {
                let dbt_manifest = self.memoized_manifest(invocation_id, resolved_state);
                write_artifact_to_file(
                    &dbt_manifest,
                    ArtifactType::Manifest,
                    &self.arg.io.out_dir,
                    DBT_MANIFEST_JSON,
                    &self.arg.io.in_dir,
                )?;
            }
        }

        // Save incremental state when --partial-parse is enabled.
        // When --partial-load is active, changed_nodes is Some(set) so save() writes only a
        // delta epoch for the loaded+changed nodes; missing changed nodes get picked up on the
        // next full-load run. When nothing changed, changed_nodes is Some(empty) and save() is
        // a no-op (early return inside parquet_incremental::save).
        //
        // Also when the index is being written, even if partial parse is off: these epochs are
        // the input the index ingest converts into the `dbt.*` layers, so without them the
        // ingest finds nothing to convert and leaves a half-built index (run_results only).
        // Reading `EvalArgs` here rather than the raw `CommonArgs` is what lets `build` default
        // the index on without also turning incremental reuse on — the two read sites below stay
        // on `effective_partial_parse()`. Writing this state and *consuming* it to skip work are
        // separable, and only the consuming side carries the staleness risk.
        //
        // And when checks will run, whatever the index is doing: these same epochs are what the
        // parse-time gate queries. Stating it as its own condition rather than leaning on the
        // implied index is the point — `--no-write-index` must not decide whether a check runs,
        // and it did for as long as this was the index's write site alone.
        if self.cli.common_args.effective_partial_parse()
            || self.arg.write_index
            || self.parse_time_checks_will_run(resolved_state)
        {
            let dbt_state = loaded_project.dbt_state();
            let env_vars = dbt_jinja_utils::utils::ENV_VARS
                .lock()
                .map(|m| m.clone())
                .unwrap_or_default();
            let changed_nodes = build_cache_changes.map(|c| c.changed_nodes.as_ref());
            if let Err(e) = dbt_metadata::partial_parse::save_parse_state(
                &self.arg.io,
                &dbt_state,
                resolved_state,
                &self.cli.common_args.vars,
                env_vars,
                changed_nodes,
            ) {
                tracing::warn!("Failed to save incremental parse state: {e}");
            } else if self.cli.common_args.verify_partial_parse {
                self.run_verify_partial_parse(
                    feature_stack.adapter.type_ops_factory.clone(),
                    feature_stack.adapter.adapter_factory.clone(),
                    feature_stack.jinja.factory.clone(),
                );
            }
        }

        // check if command is parse and write catalog.json
        self.catalog_artifact = if (self.arg.command == FsCommand::Parse
            || self.arg.command == FsCommand::Compile)
            && self.arg.write_catalog
        {
            Some(
                write_catalog(
                    self.arg.as_ref(),
                    loaded_project,
                    resolved_state,
                    jinja_env,
                    &self.token,
                )
                .await?,
            )
        } else {
            None
        };

        // Produce parquet metadata epoch files for `parse` (no lineage or schemas available).
        if self.arg.write_metadata && self.arg.command == FsCommand::Parse {
            let manifest = self.memoized_manifest(invocation_id, resolved_state);

            let empty_targets = HashSet::new();
            let empty_grain_infos = HashMap::new();
            let empty_node_classifiers = HashMap::new();
            let empty_column_classifiers = HashMap::new();
            write_metadata_parquet(
                &self.arg,
                manifest,
                None,
                None,
                None,
                &empty_targets,
                &empty_grain_infos,
                &empty_node_classifiers,
                &empty_column_classifiers,
            );

            write_parse_artifacts(&self.arg);
        }

        if self.arg.io.should_show(ShowOptions::Manifest) {
            emit_info_event(
                ShowResult::new_text(
                    to_string_pretty(self.memoized_manifest(invocation_id, resolved_state))?,
                    "manifest",
                    "Manifest",
                ),
                None,
            );
        }
        if let Err(err) = checkpoint_maybe_exit(self.arg.as_ref(), Phases::Parse) {
            if err.exit_status().is_some() {
                // Preserve manifest artifact semantics for commands that short-circuit at parse
                // after load/resolve but before execute_all_phases reaches the late write path.
                if (self.arg.write_json || self.arg.write_metadata)
                    && self.arg.command != FsCommand::Parse
                {
                    // Write run_results with error status for nodes that had
                    // resolution errors so that `dbt retry` and `dbt agent` can pick them up.
                    let now = SystemTime::now();
                    let error_stats = Stats {
                        stats: resolved_state
                            .nodes_with_resolution_errors
                            .iter()
                            .map(|uid| Stat {
                                unique_id: uid.clone(),
                                num_rows: None,
                                rows_affected: None,
                                start_time: now,
                                end_time: now,
                                status: NodeStatus::Errored,
                                thread_id: "main".to_string(),
                                message: Some("Compilation Error".to_string()),
                            })
                            .collect(),
                        nodes: Some(resolved_state.nodes.clone()),
                        batch_results: Default::default(),
                        compiled_code: Default::default(),
                    };
                    if self.arg.write_json {
                        write_run_results_json_or_warn(
                            // TODO: should also be captured by the caller?
                            &build_run_results_artifact(
                                &error_stats,
                                // Adapter responses are not available during parse phase.
                                &HashMap::new(),
                                self.arg.as_ref(),
                            ),
                            self.arg.as_ref(),
                        );
                    }
                    if self.arg.write_metadata {
                        crate::utils::write_runtime_results_parquet(
                            &error_stats,
                            &HashMap::new(),
                            self.arg.as_ref(),
                        );
                    }

                    let dbt_manifest = self.memoized_manifest(invocation_id, resolved_state);
                    write_artifact_to_file(
                        dbt_manifest,
                        ArtifactType::Manifest,
                        &self.arg.io.out_dir,
                        DBT_MANIFEST_JSON,
                        &self.arg.io.in_dir,
                    )?;
                }
            }
            return Err(err); // exit with a status as before
        }
        Ok(())
    }

    /// Load previous state (priority: --state > --defer-state > auto-downloaded manifest)
    async fn try_load_previous_state(
        &self,
        root_project_quoting: ResolvedQuoting,
        cloud_defer_path: Option<PathBuf>,
        maybe_prev_compilation: &Option<Arc<DbtProjectCompilation>>,
    ) -> FsResult<Option<Arc<StateArtifacts>>> {
        let io = &self.arg.io;
        let path_to_state = &self.arg.state;
        let defer_state = &self.arg.defer_state;

        // Only in LSP Mode
        if let Some(previous_state) = maybe_prev_compilation
            .as_ref()
            .and_then(|x| x.previous_state.clone())
        {
            Ok(Some(previous_state))
        } else if let Some(state) = path_to_state {
            // --state was explicitly provided. If manifest.json exists but is broken,
            // try_new_with_target_path always errors regardless of on_failure (issue #1319).
            // For a missing manifest.json, error only when the selector actually needs it
            // (state:modified / state:new). Selectors like source_status:fresher and
            // result: only need sources.json / run_results.json respectively, so a missing
            // manifest.json should be silently ignored for them.
            let manifest_on_failure = if selectors_require_manifest(
                self.arg.select.as_ref(),
                self.arg.exclude.as_ref(),
            ) {
                OnManifestLoadFailure::Error
            } else {
                OnManifestLoadFailure::Ignore
            };
            Ok(Some(Arc::new(StateArtifacts::try_new_with_target_path(
                state,
                root_project_quoting,
                Some(io.out_dir.clone()),
                manifest_on_failure,
            )?)))
        } else {
            let hydrated_state_manifest = defer_state.clone().or(cloud_defer_path);

            match hydrated_state_manifest.as_ref() {
                // A manifest path was resolved (via --defer-state or cloud download):
                // a broken manifest is always an error regardless of the selector.
                Some(state_path) => Ok(Some(Arc::new(StateArtifacts::try_new_with_target_path(
                    state_path,
                    root_project_quoting,
                    Some(io.out_dir.clone()),
                    OnManifestLoadFailure::Error,
                )?))),
                None => Ok(None),
            }
        }
    }

    fn refresh_node_resolver(
        &self,
        loaded_project: &DbtLoadedProject,
        build_cache_changes: Option<&CacheState>,
        resolved_state: &mut ResolverState,
    ) -> FsResult<()> {
        if build_cache_changes.is_some() {
            resolved_state.node_resolver = {
                let root_project_name = loaded_project.root_project_name().to_string();
                let compile_or_test =
                    self.arg.command == FsCommand::Compile || self.arg.command == FsCommand::Test;
                let node_resolver = NodeResolver::from_dbt_nodes(
                    &resolved_state.nodes,
                    resolved_state.adapter_type,
                    root_project_name,
                    None,
                    RunFilter::try_from(self.arg.empty, self.arg.sample.clone())?,
                    BTreeMap::new(), // renaming
                    compile_or_test,
                    PackageSearchOrder::resolve(
                        loaded_project.dbt_state().root_project().flags.as_ref(),
                    ),
                )?;
                Arc::new(node_resolver)
            };
            Ok(())
        } else {
            Ok(())
        }
    }
}

pub use dbt_compilation::schedule::{
    DbtCustomScheduleDescription, DbtProjectCompilationCacheChanges, DbtScheduleDescription,
};

#[allow(dead_code)]
/// This is an immutable data type
/// that represents a resolved project
/// for compilation.
pub struct DbtProjectCompilation {
    pub(crate) loaded_project: Arc<DbtLoadedProject>,
    pub(crate) resolved_state: ResolverState,
    pub(crate) lazy_dbt_manifest: OnceLock<DbtManifestV12>,
    pub(crate) file_kind_registry: CompleteStateWithKind,
    pub(crate) metricflow_server_client: Option<Arc<dyn MetricflowClient>>,
    pub(crate) catalog_artifact: Option<DbtCatalog>,
    /// --state
    pub(crate) previous_state: Option<Arc<StateArtifacts>>,
    /// Request context for service-backed state selection when no local state is available.
    pub(crate) run_cache_state_selector_args: Option<RunCacheStateSelectorArgs>,
    pub(crate) invocation_id: String,
    /// True when --partial-load actually applied a unique_id filter to the cache load
    /// (either via the fast path or filtered incremental load). False on a full parse
    /// or when the filter fell back to None. Used to gate --verify-partial-load checks.
    pub(crate) partial_load_filter_applied: bool,
}

pub struct DbtProjectCompilationCacheState {
    pub schema_store: Arc<SchemaStore>,
    pub(crate) data_store: Arc<DataStore>,
    schema_hydration_state: SchemaHydrationState,
    compiled_sql_cache: Arc<dyn CompiledSqlCache>,
}

impl DbtProjectCompilationCacheState {
    /// The compiled-SQL cache, so post-graph work can read what render tasks produced rather than
    /// re-rendering or reading rendered SQL off a node.
    pub fn compiled_sql_cache(&self) -> Arc<dyn CompiledSqlCache> {
        self.compiled_sql_cache.clone()
    }

    pub fn schema_exists_by_unique_id(&self, unique_id: &str) -> bool {
        self.schema_store.exists_by_unique_id(unique_id)
    }

    pub fn get_schema_by_unique_id(&self, unique_id: &str) -> Option<SchemaRef> {
        self.schema_store
            .get_schema_by_unique_id(unique_id)
            .map(|x| x.into_inner())
    }

    pub fn data_store(&self) -> Arc<DataStore> {
        self.data_store.clone()
    }
}

impl CompilationCache for DbtProjectCompilationCacheState {
    fn schema_exists_by_unique_id(&self, unique_id: &str) -> bool {
        self.schema_store.exists_by_unique_id(unique_id)
    }

    fn get_schema_by_unique_id(&self, unique_id: &str) -> Option<SchemaRef> {
        self.schema_store
            .get_schema_by_unique_id(unique_id)
            .map(|x| x.into_inner())
    }

    fn schema_store(&self) -> Arc<SchemaStore> {
        self.schema_store.clone()
    }

    fn data_store(&self) -> Arc<DataStore> {
        self.data_store.clone()
    }

    fn into_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync> {
        self
    }
}

#[allow(clippy::type_complexity)]
pub type DbtRunTasksResult = (
    Arc<RunTasksArgs>,
    RunTaskResults,
    Arc<JinjaEnv>,
    Arc<Adapter>,
    Arc<DbtProjectCompilationCacheState>,
);

use crate::partial_parse::{
    PrevCompilationResult, try_lazy_load_fast_path, try_load_prev_compilation,
};
use dbt_compilation::traits::{CompilationCache, CompiledProject};
use dbt_state::selector::RunCacheStateSelectorArgs;

impl DbtProjectCompilation {
    fn dbt_state(&self) -> Arc<DbtState> {
        self.loaded_project.dbt_state()
    }

    pub(crate) fn dbt_cloud_config(&self) -> Option<&ResolvedCloudConfig> {
        self.loaded_project.dbt_cloud_config()
    }

    pub fn resolved_state(&self) -> &ResolverState {
        &self.resolved_state
    }

    pub fn loaded_project(&self) -> &DbtLoadedProject {
        &self.loaded_project
    }

    pub fn metricflow_server_client(&self) -> Option<Arc<dyn MetricflowClient>> {
        self.metricflow_server_client.clone()
    }

    pub fn root_package(&self) -> &DbtPackage {
        self.loaded_project.root_package()
    }

    pub fn root_project(&self) -> &DbtProject {
        self.loaded_project.root_project()
    }

    pub fn root_project_name(&self) -> &str {
        self.loaded_project.root_project_name()
    }

    pub fn root_project_id(&self) -> String {
        self.loaded_project.root_project_id()
    }

    pub fn root_project_directory(&self) -> DbtPath {
        let path = self
            .loaded_project
            .root_package()
            .package_root_path
            .as_path();
        DbtPath::from(path)
    }

    pub fn adapter_type(&self) -> AdapterType {
        self.resolved_state.adapter_type
    }

    pub fn create_jinja_env(&self, arg: &EvalArgs, token: CancellationToken) -> FsResult<JinjaEnv> {
        let invocation_args = InvocationArgs::from_eval_args(arg);
        self.loaded_project.create_jinja_env(
            &self.resolved_state,
            &arg.io,
            &invocation_args,
            &token,
        )
    }

    pub fn take_dbt_manifest(&mut self) -> DbtManifestV12 {
        self.lazy_dbt_manifest
            .take()
            .unwrap_or_else(|| build_manifest(&self.invocation_id, self.resolved_state()))
    }

    pub fn has_file_changed(&self, relative_file_path: &DbtPath) -> bool {
        let root_package = self.root_package();
        let file_path = root_package
            .package_root_path
            .join(relative_file_path.as_path());

        let Ok(last_write_time) = stdfs::last_modified(&file_path) else {
            return false;
        };
        for kv in &self.dbt_state().root_package().all_paths {
            let values = kv.1;
            let Some((_, value)) = values.iter().find(|x| relative_file_path == &x.0) else {
                continue;
            };
            return value != &last_write_time;
        }
        false
    }

    pub fn macros(&self) -> &Macros {
        &self.resolved_state().macros
    }

    pub fn node_resolver(&self) -> Option<&NodeResolver> {
        self.resolved_state().node_resolver.as_any().downcast_ref()
    }

    pub fn nodes(&self) -> &Nodes {
        &self.resolved_state().nodes
    }

    pub fn models_count(&self) -> u32 {
        self.resolved_state().nodes.models.len() as u32
    }

    pub fn lookup_ref(
        &self,
        maybe_package_name: &Option<String>,
        model_name: &str,
        name: &Option<String>,
        maybe_node_package_name: &Option<String>,
    ) -> Option<(String, ModelStatus)> {
        let (unique_id, _, status, _) = match self.node_resolver()?.lookup_ref(
            maybe_package_name,
            model_name,
            name,
            maybe_node_package_name,
        ) {
            Ok(result) => result,
            Err(_) => {
                return None;
            }
        };
        Some((unique_id, status))
    }

    /// Initializes a new compilation.
    /// The resulting compilation is based on the state of the file system.
    #[allow(clippy::too_many_arguments)]
    pub async fn initialize_cli(
        feature_stack: &Arc<FeatureStack>,
        arg: &EvalArgs,
        cli: &Cli,
        event_emitter: Option<&dyn DiscreteEventEmitter>,
        jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
        token: &CancellationToken,
        version_check_handle: &mut Option<tokio::task::JoinHandle<Option<String>>>,
        artifacts_sink: &mut DbtCommandExecutionArtifacts,
    ) -> FsResult<(
        DbtProjectCompilation,
        JinjaEnv,
        Option<DbtProjectCompilationCacheChanges>,
    )> {
        // Route through the incremental path when partial_parse is active.
        // initialize_cli_incremental handles cache load, fast-reuse, and fallback.
        if cli.common_args.effective_partial_parse() {
            return Self::initialize_cli_incremental(
                feature_stack,
                arg,
                cli,
                event_emitter,
                jinja_type_checking_event_listener_factory,
                token,
                version_check_handle,
                artifacts_sink,
            )
            .await;
        }

        token.check_cancellation()?;

        let config = CompilationConfig {
            use_build_cache_for_scheduling: true,
            cacheable_commands: vec![
                FsCommand::Parse,
                FsCommand::Compile,
                FsCommand::Run,
                FsCommand::Test,
                FsCommand::Extension("lineage"),
                FsCommand::Seed,
            ],
            disable_local_compute_checks: false,
            use_resolver_state_deps: false,
            no_version_check: cli.common_args.no_version_check,
            use_full_schema_store: false,
        };
        DbtProjectCompilation::initialize(
            feature_stack,
            arg,
            cli,
            config,
            event_emitter,
            jinja_type_checking_event_listener_factory,
            None,
            token,
            version_check_handle,
            artifacts_sink,
        )
        .await
    }

    /// Initializes a new CLI compilation with incremental parse support.
    /// Attempts to reconstruct a previous compilation from parse_state.json
    /// on disk. If reconstruction fails for any reason, falls back to a full parse.
    #[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
    pub async fn initialize_cli_incremental(
        feature_stack: &Arc<FeatureStack>,
        arg: &EvalArgs,
        cli: &Cli,
        event_emitter: Option<&dyn DiscreteEventEmitter>,
        jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
        token: &CancellationToken,
        version_check_handle: &mut Option<tokio::task::JoinHandle<Option<String>>>,
        artifacts_sink: &mut DbtCommandExecutionArtifacts,
    ) -> FsResult<(
        DbtProjectCompilation,
        JinjaEnv,
        Option<DbtProjectCompilationCacheChanges>,
    )> {
        token.check_cancellation()?;

        let config = CompilationConfig {
            use_build_cache_for_scheduling: true,
            cacheable_commands: vec![
                FsCommand::Parse,
                FsCommand::Compile,
                FsCommand::Run,
                FsCommand::Test,
                FsCommand::Extension("lineage"),
                FsCommand::Seed,
            ],
            disable_local_compute_checks: false,
            use_resolver_state_deps: false,
            no_version_check: cli.common_args.no_version_check,
            use_full_schema_store: false,
        };

        let (mut maybe_prev, use_lazy_filter) = if cli.common_args.effective_partial_parse() {
            match try_load_prev_compilation(
                arg,
                &config,
                cli,
                Arc::clone(&feature_stack.adapter.type_ops_factory),
                Arc::clone(&feature_stack.adapter.adapter_factory),
                Arc::clone(&feature_stack.jinja.factory),
            ) {
                (PrevCompilationResult::Incremental(prev), lazy) => (Some(prev), lazy),
                (PrevCompilationResult::FullParse, _) => {
                    // Non-incremental change (e.g. yaml modified) — full parse without prev
                    return DbtProjectCompilation::initialize(
                        feature_stack,
                        arg,
                        cli,
                        config,
                        event_emitter,
                        jinja_type_checking_event_listener_factory,
                        None,
                        token,
                        version_check_handle,
                        artifacts_sink,
                    )
                    .await;
                }
                (PrevCompilationResult::None, _) => {
                    tracing::debug!(
                        "Partial parse: no previous state found, performing full parse"
                    );
                    (None, false)
                }
            }
        } else {
            (None, false)
        };

        // partial-load fast path (also fires for state:dirty): if no file mtimes changed,
        // skip WalkDir entirely and return prev as-is. Skip when --inline is set since the
        // inline SQL node must be injected during load().
        //
        // Also skip it when a `parse` was asked to write artifacts. `parse`'s metadata epochs
        // and information schema are written from `maybe_write_json_and_exit`, which returning
        // early here never reaches — so `dbt parse --generate-info-schema` on a project with a
        // warm parse cache exited 0 having written nothing at all, no artifact and no warning.
        // Every other command escapes this because it writes from the post-run site in
        // `dbt_lib`, which the fast path does not bypass.
        //
        // Scoped to `parse` deliberately: `write_metadata` is defaulted on for build/run/check,
        // so testing it alone would disable the fast path for them too.
        let has_inline = match &cli.command {
            Command::Core(CoreCommand::Compile(CompileArgs {
                inline: Some(_), ..
            })) => true,
            Command::Core(CoreCommand::Show(show_args)) => {
                show_args.inline.is_some() && !show_queries_info_schema(show_args)
            }
            _ => false,
        };
        // For `parse`, `write_metadata` is only true when an artifact was actually requested
        // (`--generate-info-schema` and `--write-index` both imply it); a plain `dbt parse`
        // leaves it false and keeps the fast path.
        let parse_artifacts_requested =
            arg.command == FsCommand::Parse && (arg.write_metadata || arg.generate_info_schema);
        if use_lazy_filter && !has_inline && !parse_artifacts_requested {
            // Skip the fast path when the --static-analysis level has changed since the
            // previous compilation. The fast path reuses nodes whose `base().static_analysis`
            // was stamped by the prior run; if the CLI arg changed (e.g. baseline→strict),
            // those stale values would prevent schema hydration from correctly upgrading SA,
            // causing the analyze phase (and lineage computation) to be skipped.
            let sa_changed = arg.static_analysis.is_some_and(|requested| {
                maybe_prev
                    .as_ref()
                    .and_then(|prev| {
                        prev.resolved_state
                            .nodes
                            .models
                            .values()
                            .next()
                            .map(|m| *m.base().static_analysis.as_ref() != requested)
                    })
                    .unwrap_or(false)
            });
            if !sa_changed {
                if let Some(mut compilation) = try_lazy_load_fast_path(&mut maybe_prev) {
                    tracing::debug!("Partial parse: partial-load fast path — no file changes");
                    compilation.partial_load_filter_applied = true;
                    let jinja_env = compilation.create_jinja_env(arg, token.clone())?;
                    return Ok((compilation, jinja_env, None));
                }
            } else {
                tracing::debug!(
                    "Partial parse: skipping fast path — static_analysis level changed"
                );
            }
        }

        // When partial_load is active and we have a prev state, the load was filtered.
        // Capture this before moving maybe_prev into initialize().
        let partial_load_filter_applied = use_lazy_filter && maybe_prev.is_some();

        let result = DbtProjectCompilation::initialize(
            feature_stack,
            arg,
            cli,
            config.clone(),
            event_emitter,
            jinja_type_checking_event_listener_factory.clone(),
            maybe_prev.clone(),
            token,
            version_check_handle,
            artifacts_sink,
        )
        .await;

        match result {
            Err(e) if e.code == ErrorCode::NoFilesChanged => {
                // When the static_analysis level has changed, we cannot reuse the previous
                // compilation because nodes carry stale SA assignments that would prevent
                // the analyze phase from running at the correct level.
                let sa_changed_on_prev = arg.static_analysis.is_some_and(|requested| {
                    maybe_prev
                        .as_ref()
                        .and_then(|prev| {
                            prev.resolved_state
                                .nodes
                                .models
                                .values()
                                .next()
                                .map(|m| *m.base().static_analysis.as_ref() != requested)
                        })
                        .unwrap_or(false)
                });
                if let Some(prev) = maybe_prev {
                    if sa_changed_on_prev {
                        tracing::debug!(
                            "Partial parse: no files changed but static_analysis level changed, performing full parse"
                        );
                        return DbtProjectCompilation::initialize(
                            feature_stack,
                            arg,
                            cli,
                            config,
                            event_emitter,
                            jinja_type_checking_event_listener_factory,
                            None,
                            token,
                            version_check_handle,
                            artifacts_sink,
                        )
                        .await;
                    }
                    // Same reasoning as the partial-load fast path above: reusing the
                    // previous compilation returns before `maybe_write_json_and_exit`, which
                    // is where `parse` writes its metadata epochs and information schema. A
                    // `parse` that was asked for them has to take the full path, exactly as
                    // it does when the static-analysis level changed.
                    if arg.command == FsCommand::Parse
                        && (arg.write_metadata || arg.generate_info_schema)
                    {
                        tracing::debug!(
                            "Partial parse: no files changed, but parse artifacts were requested"
                        );
                        return DbtProjectCompilation::initialize(
                            feature_stack,
                            arg,
                            cli,
                            config,
                            event_emitter,
                            jinja_type_checking_event_listener_factory,
                            None,
                            token,
                            version_check_handle,
                            artifacts_sink,
                        )
                        .await;
                    }
                    tracing::debug!(
                        "Partial parse: no files changed, reusing previous compilation"
                    );
                    let compilation = match Arc::try_unwrap(prev) {
                        Ok(c) => c,
                        Err(_) => {
                            return DbtProjectCompilation::initialize(
                                feature_stack,
                                arg,
                                cli,
                                config,
                                event_emitter,
                                jinja_type_checking_event_listener_factory,
                                None,
                                token,
                                version_check_handle,
                                artifacts_sink,
                            )
                            .await;
                        }
                    };
                    let jinja_env = compilation.create_jinja_env(arg, token.clone())?;
                    Ok((compilation, jinja_env, None))
                } else {
                    DbtProjectCompilation::initialize(
                        feature_stack,
                        arg,
                        cli,
                        config,
                        event_emitter,
                        jinja_type_checking_event_listener_factory,
                        None,
                        token,
                        version_check_handle,
                        artifacts_sink,
                    )
                    .await
                }
            }
            // Cache is structurally inconsistent (package count changed, resource kinds
            // changed, etc.). This is not a user error — discard the cache and do a clean
            // full parse so the command still succeeds.
            Err(e) if e.code == ErrorCode::CacheError => {
                tracing::debug!(
                    "Partial parse: cache inconsistent ({}), falling back to full parse",
                    e.context
                );
                DbtProjectCompilation::initialize(
                    feature_stack,
                    arg,
                    cli,
                    config,
                    event_emitter,
                    jinja_type_checking_event_listener_factory,
                    None,
                    token,
                    version_check_handle,
                    artifacts_sink,
                )
                .await
            }
            Ok((mut compilation, jinja_env, changes)) => {
                compilation.partial_load_filter_applied = partial_load_filter_applied;
                Ok((compilation, jinja_env, changes))
            }
            other => other,
        }
    }
}

impl DbtProjectCompilation {
    /// Initializes a new compilation for use as a stateful server.
    /// The resulting compilation is based on the state of the file system.
    #[allow(clippy::too_many_arguments)]
    pub async fn initialize_server(
        feature_stack: &Arc<FeatureStack>,
        arg: &EvalArgs,
        cli: &Cli,
        jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
        prev_compilation: Option<Arc<DbtProjectCompilation>>,
        token: &CancellationToken,
    ) -> FsResult<(
        DbtProjectCompilation,
        JinjaEnv,
        Option<DbtProjectCompilationCacheChanges>,
    )> {
        token.check_cancellation()?;

        let config = CompilationConfig {
            use_build_cache_for_scheduling: false,
            cacheable_commands: vec![], // Server doesn't use cache commands
            disable_local_compute_checks: true,
            use_resolver_state_deps: true,
            no_version_check: true,
            use_full_schema_store: true,
        };

        DbtProjectCompilation::initialize(
            feature_stack,
            arg,
            cli,
            config,
            None,
            jinja_type_checking_event_listener_factory,
            prev_compilation,
            token,
            &mut None,
            &mut Default::default(),
        )
        .await
    }

    /// Initializes a new compilation.
    /// The resulting compilation is based on the state of the file system.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::cognitive_complexity)]
    async fn initialize(
        feature_stack: &Arc<FeatureStack>,
        arg_use_once: &EvalArgs,
        cli: &Cli,
        config: CompilationConfig,
        maybe_event_emitter: Option<&dyn DiscreteEventEmitter>,
        jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
        maybe_prev_compilation: Option<Arc<DbtProjectCompilation>>,
        token: &CancellationToken,
        version_check_handle: &mut Option<tokio::task::JoinHandle<Option<String>>>,
        artifacts_sink: &mut DbtCommandExecutionArtifacts,
    ) -> FsResult<(
        DbtProjectCompilation,
        JinjaEnv,
        Option<DbtProjectCompilationCacheChanges>,
    )> {
        token.check_cancellation()?;

        let mut executor = CompilationPhasesExecutor::new(
            Cow::Borrowed(arg_use_once),
            Cow::Borrowed(cli),
            token.clone(),
        );

        let invocation_id = executor.arg.io.invocation_id.to_string();

        // ========================================================================
        // PHASE 1: Use Pipeline for Load
        // ========================================================================
        let (loaded_project, mut build_cache_changes) = executor
            .load(
                feature_stack,
                config,
                &maybe_prev_compilation,
                maybe_event_emitter,
                version_check_handle,
            )
            .await?;

        // PHASE 2: Use Pipeline for Resolve
        // ========================================================================

        // Create FileKindRegistry before dropping unchanged nodes
        let file_kind_registry =
            CompleteStateWithKind::from_dbt_state(&executor.arg.io, &loaded_project.dbt_state())?;
        token.check_cancellation()?;

        let (mut resolved_state, jinja_env) = {
            let resolve_args = ResolveArgs::try_from_eval_args(executor.arg.as_ref())?;
            let invocation_args = InvocationArgs::from_eval_args(executor.arg.as_ref());

            loaded_project
                .resolve(
                    resolve_args,
                    &invocation_args,
                    build_cache_changes.as_ref(),
                    token,
                    jinja_type_checking_event_listener_factory.clone(),
                    feature_stack.resolver.hooks.clone(),
                )
                .await?
        };
        token.check_cancellation()?;

        feature_stack
            .cli
            .hooks
            .did_resolve_project(
                executor.cli.as_ref(),
                executor.arg.as_ref(),
                &resolved_state,
                &jinja_env,
            )
            .await?;

        // todo: if you execute local then no sources can be used!
        // currently system times out, go add a error

        let semantic_manifest = SemanticManifest::from(&resolved_state.nodes);
        token.check_cancellation()?;

        // Catalogs are parsed into DbtState (catalogs.yml), not ResolverState, so
        // count them here from the loaded project where DbtState is in scope, and
        // pass the count through to the telemetry event. Both v1 and v2
        // catalogs.yml store their entries under the top-level `catalogs` key.
        let catalog_count = {
            let dbt_state = loaded_project.dbt_state();
            dbt_state
                .catalogs
                .as_ref()
                .and_then(|catalogs| catalogs.catalogs_seq().ok())
                .map(|seq| seq.len() as i32)
                .unwrap_or(0)
        };
        executor.resource_counts_event(&resolved_state, catalog_count);
        token.check_cancellation()?;

        let metricflow_server_client =
            if let Some(factory) = feature_stack.metricflow.factory.as_ref() {
                factory
                    .create_client(
                        executor.arg.as_ref(),
                        &semantic_manifest,
                        loaded_project.dbt_cloud_config(),
                        resolved_state.semantic_layer_spec_is_legacy,
                    )
                    .await?
            } else {
                None
            };
        token.check_cancellation()?;

        match executor
            .maybe_write_json_and_exit(
                feature_stack,
                &loaded_project,
                build_cache_changes.as_ref(),
                &resolved_state,
                &file_kind_registry,
                &semantic_manifest,
                &invocation_id,
                &jinja_env,
            )
            .await
        {
            // `compile --write-catalog` returns Ok here, so the catalog has to be
            // taken on this path too or it is never surfaced.
            Ok(()) => artifacts_sink.catalog = executor.catalog_artifact.take(),
            Err(e) => {
                // Capture artifacts if possible before early exit
                artifacts_sink.manifest = executor.lazy_dbt_manifest.take();
                artifacts_sink.catalog = executor.catalog_artifact.take();

                return Err(e);
            }
        };
        token.check_cancellation()?;

        let mut cloud_defer_path: Option<PathBuf> = None;
        if executor.arg.defer && executor.arg.state.is_none() && executor.arg.defer_state.is_none()
        {
            feature_stack
                .cli
                .hooks
                .will_load_deferred_state(
                    &executor.arg.io,
                    loaded_project.dbt_cloud_config(),
                    &mut cloud_defer_path,
                )
                .await?;
        }

        let maybe_previous_state = executor
            .try_load_previous_state(
                resolved_state.root_project_quoting,
                cloud_defer_path,
                &maybe_prev_compilation,
            )
            .await?;
        token.check_cancellation()?;

        // Seed the truncated-name → state-uid index so that state:modified+ correctly
        // recognises tests from Mantle-produced state manifests. Mantle uses the full
        // untruncated test name in unique_ids; Fusion truncates long names. Without this,
        // all tests with names longer than 63 chars appear as state:new.
        if let Some(prev_state) = &maybe_previous_state {
            prev_state.set_test_name_truncations(&resolved_state.test_name_truncations);
        }

        // Only initialize run-cache state selector when no local previous state is available.
        // The lifecycle handles all enablement checks and fail-open logic internally.
        let run_cache_state_selector_args = if maybe_previous_state.is_none() {
            create_run_cache_state_selector_args(
                arg_use_once,
                &resolved_state,
                loaded_project.dbt_cloud_config(),
                loaded_project.root_package().package_root_path.as_path(),
            )
            .await?
        } else {
            None
        };

        // Refresh node_resolver (no renaming needed - sample plan is resolved after scheduling)
        executor.refresh_node_resolver(
            &loaded_project,
            build_cache_changes.as_ref(),
            &mut resolved_state,
        )?;
        token.check_cancellation()?;

        let compilation_cache_changes = build_cache_changes
            .take()
            .map(DbtProjectCompilationCacheChanges::new);

        if let Some(prev_state) = &maybe_previous_state {
            if let Some(nodes) = &prev_state.nodes {
                resolved_state.defer_nodes = Some(nodes.clone());
            }
        }

        Ok((
            DbtProjectCompilation {
                loaded_project: Arc::new(loaded_project),
                resolved_state,
                lazy_dbt_manifest: executor.lazy_dbt_manifest,
                file_kind_registry,
                metricflow_server_client,
                catalog_artifact: executor.catalog_artifact.take(),
                previous_state: maybe_previous_state,
                run_cache_state_selector_args,
                invocation_id,
                partial_load_filter_applied: false,
            },
            Arc::into_inner(jinja_env).expect("should have one reference"),
            compilation_cache_changes,
        ))
    }

    pub async fn create_schedule<'a>(
        &self,
        cli: &Cli,
        arg: &EvalArgs,
        schedule_desc: DbtScheduleDescription<'a>,
        exclude_unique_ids: HashSet<String>,
        token: &CancellationToken,
    ) -> FsResult<Schedule<String>> {
        let maybe_previous_state = self.previous_state.clone();
        let run_cache_state_selector_args = self.run_cache_state_selector_args.as_ref();

        // ========================================================================
        // PHASE 3: Use Pipeline for Schedule
        // ========================================================================

        // For Pull command, use its select args for scheduling
        let pull_select = cli.sample_select();
        let selection_override = resolve_selection_override(arg)?;
        let scheduler_args =
            SchedulerArgs::from_eval_args_with_exclude_unique_ids_and_selection_override(
                arg,
                exclude_unique_ids,
                selection_override,
            );

        let schedule = if let Some(ref pull_select_args) = pull_select {
            let pull_exclude = cli.sample_exclude();
            // Pull command with --select: use schedule_with_select
            use dbt_common::node_selector::parse_model_specifiers;
            let select_expr = parse_model_specifiers(pull_select_args)?;
            let exclude_expr = if let Some(ref args) = pull_exclude {
                Some(parse_model_specifiers(args)?)
            } else {
                None
            };
            schedule_with_select(
                &self.resolved_state,
                scheduler_args,
                maybe_previous_state.as_ref().map(|x| x.as_ref()),
                run_cache_state_selector_args,
                select_expr,
                exclude_expr,
                arg.local_execution_backend,
                token,
            )
            .await?
        } else {
            match schedule_desc {
                DbtScheduleDescription::Default => {
                    build_schedule_from_resolved(
                        &self.resolved_state,
                        scheduler_args,
                        maybe_previous_state.as_ref().map(|x| x.as_ref()),
                        run_cache_state_selector_args,
                        arg.local_execution_backend,
                        token,
                    )
                    .await?
                }
                DbtScheduleDescription::Custom(custom_schedule_desc) => {
                    schedule_with_unique_ids(
                        &self.resolved_state,
                        scheduler_args,
                        maybe_previous_state.as_ref().map(|x| x.as_ref()),
                        run_cache_state_selector_args,
                        &custom_schedule_desc.unique_ids,
                        custom_schedule_desc.include_parents,
                        custom_schedule_desc.include_children,
                        custom_schedule_desc.indirect_selection,
                        arg.local_execution_backend,
                        token,
                    )
                    .await?
                }
                DbtScheduleDescription::CacheChanges(compilation_cache_changes) => {
                    schedule_with_cache_state(
                        &self.resolved_state,
                        scheduler_args,
                        maybe_previous_state.as_ref().map(|x| x.as_ref()),
                        run_cache_state_selector_args,
                        compilation_cache_changes.as_cache_state(),
                        arg.local_execution_backend,
                        token,
                    )
                    .await?
                }
            }
        };
        Ok(schedule)
    }

    /// Run tasks based on the arguments.
    /// This can be called multiple times on the same compilation.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::cognitive_complexity)]
    pub async fn run_tasks(
        &self,
        arg: &EvalArgs,
        cli: &Cli,
        start: SystemTime,
        mut jinja_env: JinjaEnv,
        feature_stack: Arc<FeatureStack>,
        schedule: Schedule<String>,
        compilation_cache_changes: Option<&DbtProjectCompilationCacheChanges>,
        previous_cache_state: Option<Arc<DbtProjectCompilationCacheState>>,
        jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
        task_runner_hooks_factory: &dyn TaskRunnerHooksFactory,
        token: &CancellationToken,
        previous_batch_results: HashMap<String, dbt_schemas::schemas::BatchResults>,
        artifacts_sink: &mut DbtCommandExecutionArtifacts,
    ) -> FsResult<DbtRunTasksResult> {
        token.check_cancellation()?;

        let dbt_cloud_config = self.dbt_cloud_config().cloned();
        let metricflow_server_client = self.metricflow_server_client.clone();
        let maybe_previous_state = self.previous_state.clone();
        let run_cache_state_selector_args = self.run_cache_state_selector_args.as_ref();
        let root_project_quoting = self.resolved_state.root_project_quoting;
        let project_optimize_tests =
            optimize_test_defaults_from_project_flags(self.root_project().flags.as_ref());
        let optimize_tests = resolve_effective_optimize_tests(
            arg.command,
            &arg.optimize_tests,
            &project_optimize_tests,
        );

        // ========================================================================
        // PHASE 3: Use Pipeline for Schedule
        // ========================================================================

        token.check_cancellation()?;

        // FEATURES: schedule cmd_show
        // Warn if no nodes were selected after applying selection criteria.
        if schedule.selected_nodes.is_empty() {
            // Only warn for commands where selection matters (not for parse, list, pull, etc.)
            let command = arg.command;
            // `Check` too: its schedule is deliberately emptied after the parse-time gate passes
            // (nothing else needs rendering/analyzing/running to answer "did the checks pass"),
            // and that emptiness is not a "nothing to do" mistake worth surfacing here -- a
            // check's own pass/fail/skip line is check's actual "did anything happen" signal.
            if !matches!(
                command,
                FsCommand::Parse
                    | FsCommand::List
                    | FsCommand::RunOperation
                    | FsCommand::Source
                    | FsCommand::Check
                    | FsCommand::Freshness
                    | FsCommand::Extension("pull")
            ) {
                // For the Show command, check if the selector matches a macro file.
                // Macros cannot be previewed — give a clear error instead of a
                // confusing empty result set (see dbt-fusion#525).
                if command == FsCommand::Show {
                    if let Some(select_expr) = &schedule.select {
                        if let Some(matched) =
                            select_matches_macro(select_expr, &self.resolved_state.macros)
                        {
                            return Err(fs_err!(
                                ErrorCode::InvalidArgument,
                                "Macros cannot be previewed. The selected path '{}' resolves to a macro. Only models, seeds, snapshots, analyses, and tests can be previewed with 'dbt show'",
                                matched
                            ));
                        }
                    }
                }

                // Format the selection expression for the warning
                if let Some(select_expr) = &schedule.select {
                    emit_warn_log_message(
                        ErrorCode::NoNodesForSelectionCriteria,
                        format!(
                            "The selection criterion '{}' does not match any enabled nodes",
                            select_expr
                        ),
                    );
                }

                emit_warn_log_message(
                    ErrorCode::NoNodesSelected,
                    "Nothing to do. Try checking your model configs and model specification args",
                );
            }
        }

        // We may have elevated errors from selection
        checkpoint_error_count_maybe_exit(arg)?;

        feature_stack
            .cli
            .hooks
            .will_run_tasks(cli, arg, &self.resolved_state, token)?;

        // Give extensions access to the built schedule (and its resolved
        // selection) before any tasks run. May terminate execution.
        feature_stack.cli.hooks.did_build_schedule(
            cli,
            arg,
            &self.resolved_state,
            &schedule,
            &jinja_env,
            token,
        )?;

        // FEATURES: schedule
        if arg.io.should_show(ShowOptions::Schedule) {
            emit_info_event(
                ShowResult::new_text(schedule.to_string(), "schedule", "Compile Time Schedule"),
                None,
            );
        }
        checkpoint_maybe_exit(arg, Phases::Schedule)?;

        if cli.common_args.verify_partial_load {
            if self.partial_load_filter_applied {
                run_verify_partial_load(arg, &schedule);
            } else {
                tracing::debug!(
                    "verify-partial-load: skipping — partial-load filter was not applied (full parse or no selector filter)"
                );
            }
        }

        // FEATURES: cmd_list
        if arg.io.should_show(ShowOptions::Nodes) {
            // Convert DisplayFormat to ListOutputFormat, warning if unsupported.
            // Normally should not happen as EvalArgs are built from Cli which only allows supported formats.
            let list_format = match ListOutputFormat::try_from(arg.format) {
                Ok(format) => format,
                Err(_) => {
                    if cli.as_command() == FsCommand::List {
                        emit_warn_log_message(
                            ErrorCode::UnsupportedFeature,
                            format!(
                                "Output format '{}' is not supported for list command. Using default 'selector' format. Supported formats: {}",
                                arg.format,
                                ListOutputFormat::supported_formats_display()
                            ),
                        );
                    }
                    ListOutputFormat::Selector
                }
            };

            // Emit each list item as a structured event (header will be emitted once by layers)
            let list_items =
                schedule.show_dbt_nodes(&self.resolved_state.nodes, list_format, &arg.output_keys);

            for item in list_items {
                let event =
                    ListItemOutput::new(list_format.into(), item.content, Some(item.unique_id));
                emit_info_event(event, None);
            }
        }
        checkpoint_maybe_exit(arg, Phases::List)?;

        token.check_cancellation()?;

        // ========================================================================
        // PHASE 4: Initialize Schema + Data Store. Initialize Adapter. Handle Defer
        // ========================================================================

        let operation_dep_ids: BTreeSet<String> = self
            .resolved_state
            .operations
            .on_run_start
            .iter()
            .chain(&self.resolved_state.operations.on_run_end)
            .flat_map(|op| op.__base_attr__.depends_on.nodes.iter().cloned())
            .collect();
        let extra_frontier_unique_ids: Option<&BTreeSet<String>> = if operation_dep_ids.is_empty() {
            None
        } else {
            Some(&operation_dep_ids)
        };

        let execute_mode = Execute::from_compute_flag(arg.local_execution_backend);

        let clear_schema_cache_dir = |cache_dir: PathBuf| {
            if cache_dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&cache_dir) {
                    tracing::warn!(
                        "Failed to clear stale schema cache at {}: {}",
                        cache_dir.display(),
                        e
                    );
                }
            }
        };

        let is_duckdb_adapter = self.adapter_type() == AdapterType::DuckDB;

        // Sidecar/service schemas and all remote entries in the epoch cache share
        // this directory, so clear it before loading schemas from the current run.
        if matches!(execute_mode, Execute::Sidecar | Execute::Service) || is_duckdb_adapter {
            clear_schema_cache_dir(arg.metadata_dir().join("warehouse").join("schemas"));
        }

        // DuckDB `run` does not mirror updated schemas to the legacy frontier cache.
        // Clear only internal entries so deferred and external caches remain intact.
        if is_duckdb_adapter {
            clear_schema_cache_dir(
                arg.io
                    .out_dir
                    .join("schemas")
                    .join("sourced_remote")
                    .join("internal"),
            );
        }

        // FEATURES: schema_hydration build_cache
        let schema_store = if let Some(previous_cache_state) = &previous_cache_state {
            let store = previous_cache_state.schema_store.clone();

            // Evict stale entries based on refresh intervals
            let mut all_frontier_ids = schedule.frontier_nodes.clone();
            if let Some(extra_unique_ids) = extra_frontier_unique_ids {
                for unique_id in extra_unique_ids {
                    // Skip if already selected -- no need for a Frontier entry
                    if schedule.selected_nodes.contains(unique_id) {
                        continue;
                    }
                    all_frontier_ids.insert(unique_id.clone());
                }
            }
            let refresh_intervals =
                build_refresh_intervals(&all_frontier_ids, &self.resolved_state.nodes);
            let evicted = store.evict_stale_entries(&refresh_intervals);

            if evicted > 0 {
                dbt_common::tracing::dbt_emit::emit_debug_log_message(format!(
                    "Evicted {} stale schema cache entries",
                    evicted
                ));
            }

            store
        } else {
            let schedule = if self.loaded_project.config().use_full_schema_store {
                Cow::Owned(
                    build_schedule_from_resolved(
                        &self.resolved_state,
                        SchedulerArgs::from_eval_args(arg),
                        maybe_previous_state.as_ref().map(|x| x.as_ref()),
                        run_cache_state_selector_args,
                        arg.local_execution_backend,
                        token,
                    )
                    .await?,
                )
            } else {
                Cow::Borrowed(&schedule)
            };
            Arc::new(init_schema_store(
                schedule.as_ref(),
                &self.resolved_state.nodes,
                &arg.io.out_dir,
                self.resolved_state.adapter_type,
                extra_frontier_unique_ids,
            )?)
        };

        // FEATURES: data_store
        let data_store = if let Some(previous_cache_state) = &previous_cache_state {
            Arc::new(previous_cache_state.data_store.as_ref().clone())
        } else {
            Arc::new(init_data_store(&arg.io.out_dir))
        };

        // FEATURES: sidecar service
        let sidecar_client = if matches!(execute_mode, Execute::Sidecar | Execute::Service) {
            let factory = feature_stack.sidecar.factory.as_ref().ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidArgument,
                    "--compute sidecar/service is not available in this build"
                )
            })?;
            Some(
                factory
                    .create_client(
                        &arg.io,
                        &self.resolved_state,
                        arg.long_living,
                        execute_mode == Execute::Service,
                        arg.io.debug,
                    )
                    .await?,
            )
        } else {
            None
        };

        // FEATURES: auth record_replay
        // Initialize adapters. The store is kept, not just the adapter selected
        // off it: a task that knows its node's adapter type asks it for the one
        // that executes that node.
        let adapter_store = self.loaded_project().init_adapter_store(
            &self.resolved_state,
            arg.replay.clone(),
            &jinja_env,
            Some(schema_store.clone()),
            token,
            sidecar_client.clone(),
            execute_mode,
            if arg.infer_schemas_and_typeless {
                AdapterConnectionMode::Offline
            } else {
                AdapterConnectionMode::AllowRemote
            },
        )?;
        let adapter = if let Some(adapter_override) = arg
            .adapter_override
            .as_deref()
            .filter(|_| matches!(arg.command, FsCommand::RunOperation | FsCommand::Show))
        {
            let adapter_type: AdapterType = adapter_override.parse().map_err(|_| {
                fs_err!(
                    ErrorCode::InvalidArgument,
                    "--adapter '{adapter_override}' is not a recognized adapter type"
                )
            })?;
            adapter_store.get(adapter_type)?
        } else {
            adapter_store.default_adapter()?
        };
        token.check_cancellation()?;

        if adapter.engine().has_query_cache() {
            let reverse_deps =
                // This is only true for the LSP currently.
                // The reason why is because the LSP
                // needs to have context of the entire project
                // even when it schedules a select few nodes.
                // This effectively gives context of the entire project.
                if self.loaded_project().config().use_resolver_state_deps {
                    self.resolved_state().create_reverse_deps()
                } else {
                    reverse(&schedule.deps)
                };
            adapter
                .engine()
                .set_query_cache_reverse_deps(reverse_deps)?;
        }
        token.check_cancellation()?;

        // FEATURES: render
        // Configure jinja env early (no node_resolver clone yet).
        // build_compiler_env is called AFTER Phase 2 defer to avoid Arc refcount issues.
        configure_compile_and_run_jinja_environment(&mut jinja_env, adapter.clone());
        // Shared from here on: the pre-run hooks and the freshness command hand the
        // environment to the blocking pool, which needs an owned handle.
        let jinja_env = Arc::new(jinja_env);

        let mut schema_hydration_state = previous_cache_state
            .as_ref()
            .map(|x| x.schema_hydration_state.clone())
            .unwrap_or_default();

        let run_task_args: Arc<RunTasksArgs> = {
            let mut run_task_args =
                RunTasksArgs::from_eval_args(arg, feature_stack.cli.fail_fast.clone())
                    .with_resolved_profile(&self.resolved_state.dbt_profile);
            run_task_args.optimize_tests = optimize_tests;
            run_task_args.sample_renaming = BTreeMap::new();
            run_task_args.previous_batch_results = previous_batch_results;
            run_task_args.into()
        };

        // FEATURES: build_cache render
        let compiled_sql_cache = previous_cache_state
            .map(|x| x.compiled_sql_cache.clone())
            .unwrap_or_else(|| Arc::new(CompiledSqlCacheImpl::default()));
        if let Some(prev_changed_nodes) = compilation_cache_changes.map(|x| {
            // If any yml files were changed, invalidate
            // the impacted nodes as they need to be re-rendered because
            // the source names could have changed.
            if x.did_any_yml_files_change() {
                x.impacted_nodes()
            } else {
                x.changed_nodes()
            }
        }) {
            for node in prev_changed_nodes.iter() {
                compiled_sql_cache.clear(node);
            }
        }

        // XXX: costly clone, but we need to enrich the resolved state now
        let mut resolved_state = self.resolved_state.deep_clone();

        // Initialize Deferral State
        let mut defer_state = if arg.defer {
            // we need to clone these to prevent the create_run_cache() closure below
            // from capturing them and preventing their use elsewhere
            let run_task_args_copy = run_task_args.clone();
            let adapter_type = resolved_state.adapter_type;
            let cloud_config = resolved_state.cloud_config.clone();

            DeferState::load(
                arg,
                adapter.clone(),
                &schedule,
                &mut resolved_state,
                &jinja_env,
                maybe_previous_state.clone(),
                root_project_quoting,
                // note: async move is required to prevent a lifetime issue with #[async_trait] in driver.rs
                async move || {
                    RunCacheLifecycle::get_or_initialize(
                        run_task_args_copy.as_ref(),
                        execute_mode,
                        adapter_type,
                        cloud_config.as_ref(),
                    )
                    .await
                },
            )
            .await?
        } else {
            DeferState::from_previous_state(maybe_previous_state.clone())
        };

        token.check_cancellation()?;

        // FEATURES: static_analysis build_cache test_runner csv_loader snapshot_strategy on_run_hooks freshness semantic_layer
        // Renders, Analyzes, and Runs Tasks (lineage is handled in background for lineage command)
        let schema_hydrator = feature_stack.task_runner.schema_hydrator_factory.create(
            adapter.clone(),
            execute_mode,
            self.loaded_project.config().clone(),
            schema_store.clone(),
            sidecar_client.clone(),
            metricflow_server_client.clone(),
        );

        let static_analysis_buckets = schema_hydrator
            .hydrate_schemas(
                arg,
                &schedule,
                &mut resolved_state,
                &mut schema_hydration_state,
                &mut defer_state,
                token.clone(),
            )
            .await?;

        // Step 5: Fixup resolved state to store the defer nodes. Node resolver also gets fixed-up too.
        if let Some(nodes) = defer_state.defer_nodes {
            dbt_defer::set_defer_context_on_resolver(
                &mut resolved_state,
                &schedule.sorted_nodes,
                &schedule.frontier_nodes,
            );

            feature_stack
                .cli
                .hooks
                .did_defer_nodes(
                    arg,
                    &defer_state.deferred_unique_ids,
                    &mut resolved_state,
                    &nodes,
                    adapter.clone(),
                )
                .await?;

            resolved_state.defer_nodes = Some(nodes);
        }

        if run_task_args.command == FsCommand::Clone && resolved_state.defer_nodes.is_none() {
            return Err(fs_err!(
                ErrorCode::InvalidArgument,
                "Use --state or --defer-state to provide a state manifest for cloning"
            ));
        }
        if run_task_args.command == FsCommand::Extension("compare") && self.previous_state.is_none()
        {
            return Err(fs_err!(
                ErrorCode::InvalidArgument,
                "Compare command requires a state manifest (via --state, --defer-state, or dbt Cloud)"
            ));
        }

        if let Some(replay_adapter) = adapter.as_replay() {
            for (truncated, full_name) in resolved_state.test_name_truncations.iter() {
                replay_adapter.record_test_name_truncation(truncated, full_name);
            }
            // A versioned model's unique_id ends in `.v<N>`, which is not its alias, so an
            // ephemeral+versioned model's own node_id can't be traced back to the CTE name
            // dbt-core compiled it under without this. See fs#11684.
            for model in resolved_state.nodes.models.values() {
                replay_adapter.record_node_alias(&model.unique_id(), &model.alias());
            }
        }

        let base_context = build_base_context(&resolved_state, &jinja_env);
        token.check_cancellation()?;

        render_all_model_constraint_refs_in_place(&mut resolved_state, &jinja_env, &base_context)?;
        token.check_cancellation()?;

        let freshness_results = feature_stack
            .cli
            .hooks
            .did_pre_run(
                arg,
                cli,
                Arc::clone(&jinja_env),
                &resolved_state,
                &schedule,
                Arc::clone(&adapter),
                &base_context,
                token,
            )
            .await?;
        token.check_cancellation()?;

        // FEATURES: cmd_source_freshness on_run_hooks artifact_output
        let sources_only = arg.command.is_sources_only_freshness();
        let freshness_results = if arg.command.is_freshness_command() {
            run_freshness_command(
                arg,
                &jinja_env,
                &resolved_state,
                &schedule,
                Arc::clone(&adapter),
                &base_context,
                artifacts_sink,
                sources_only,
            )
            .await?
        } else {
            freshness_results
        };

        feature_stack
            .cli
            .hooks
            .did_handle_defer(
                arg,
                cli,
                Cow::Borrowed(&jinja_env),
                &resolved_state,
                &schedule,
                self.metricflow_server_client.clone(),
                token,
            )
            .await?;
        token.check_cancellation()?;

        if let Command::Core(CoreCommand::RunOperation(..)) = &cli.command {
            // Shared for this branch only, which ends in a `return`: the inline-SQL
            // path hands the resolved state to the blocking pool, so it needs an
            // owned handle. `Nodes` is maps of `Arc`s, so the clone below is shallow.
            let resolved_state = Arc::new(resolved_state);

            // Macro path returns the macro author's `return` value (any `Value`,
            // stringified for display); inline-SQL path returns the Phase 1 rendered
            // SQL string.
            let (display_text, show_label, stat_name) =
                if let Some(inline_sql) = arg.macro_sql.as_deref() {
                    let rendered = run_operation_inline_sql(
                        inline_sql,
                        &resolved_state,
                        &jinja_env,
                        &base_context,
                        &arg.io,
                    )
                    .await?;
                    let unique_id = format!(
                        "sql_operation.{}.{INLINE_SQL_NAME}",
                        resolved_state.root_project_name
                    );
                    (rendered, "Inline SQL rendered", unique_id)
                } else {
                    // Clap guarantees exactly one of <MACRO> or --sql is set.
                    let macro_name = arg.macro_name.as_deref().expect(
                        "run-operation requires <MACRO> when --sql is absent (enforced by clap)",
                    );
                    let result = run_operation(
                        macro_name,
                        &arg.macro_args,
                        &resolved_state,
                        &jinja_env,
                        &base_context,
                    )
                    .await?;
                    (result.to_string(), "Macro result", macro_name.to_string())
                };

            let stat = Stat::new(stat_name, start, None, NodeStatus::Succeeded, None, 1);

            if arg.io.should_show(ShowOptions::Data) {
                emit_info_event(ShowResult::new_text(display_text, "data", show_label), None);
            }

            let run_stats = Stats {
                stats: vec![stat],
                nodes: Some(resolved_state.nodes.clone()),
                batch_results: Default::default(),
                compiled_code: Default::default(),
            };
            if arg.write_json {
                // TODO: should also be captured by the caller?
                write_run_results_json(
                    &build_run_results_artifact(
                        &run_stats,
                        // NOTE: dbt Core v1 does not populate AdapterResponse on `dbt run-operation`.
                        // However we technically could pull the adapter responses from the macro
                        // execution and add them here. We just have to wire it up.
                        &HashMap::new(),
                        arg,
                    ),
                    arg,
                )?;
            }

            return Err(return_exit_code_from_error_counter());
        };
        token.check_cancellation()?;

        typecheck_macros(
            &resolved_state,
            Arc::clone(&jinja_env),
            jinja_type_checking_event_listener_factory.clone(),
            run_task_args.as_ref(),
        )?;
        token.check_cancellation()?;

        let resolved_state = Arc::new(resolved_state);
        let static_analysis_buckets: Arc<dyn StaticAnalysisBuckets> =
            static_analysis_buckets.into();

        let tasks_for_node_factory = Arc::clone(&feature_stack.task_runner.tasks_for_node_factory);
        let compare_task_graph_builder =
            feature_stack.task_runner.compare_task_graph_builder.clone();
        let graph_builder = GraphBuilder::new(
            Arc::clone(&run_task_args),
            Arc::clone(&static_analysis_buckets),
            tasks_for_node_factory,
            compare_task_graph_builder,
        );

        // Increment counters used in final run reporting.
        // Checks are not graph work. Counting every selected check here reports them as
        // "Processed" even when the parse-time gate never ran — because the user asked to skip
        // it, or because the metadata it reads is not there. Asked the same way the gate asks,
        // and it is the same answer: both run after parse, so the epochs either exist for both
        // or for neither.
        let count_checks = !arg.skip_checks
            && dbt_tasks_sa::check::metadata_unavailable_reason(&arg.metadata_dir()).is_none();
        schedule.selected_nodes.iter().for_each(|unique_id| {
            let node_type = self
                .resolved_state
                .nodes
                .get_node(unique_id)
                .expect("Node must exist")
                .resource_type();
            if node_type == NodeType::Check && !count_checks {
                return;
            }
            increment_metric(FusionMetricKey::NodeCounts(node_type), 1);
        });

        let hooks = task_runner_hooks_factory.create(
            dbt_cloud_config,
            maybe_previous_state.clone(),
            adapter.clone(),
            Arc::clone(&resolved_state),
            Arc::clone(&jinja_env),
            schema_store.clone(),
            data_store.clone(),
            metricflow_server_client,
        );

        let run_cache = RunCacheLifecycle::get_or_initialize(
            run_task_args.as_ref(),
            execute_mode,
            resolved_state.adapter_type,
            resolved_state.cloud_config.as_ref(),
        )
        .await?;

        let task_runner = TaskRunner::new(
            hooks,
            adapter.clone(),
            adapter_store,
            resolved_state,
            jinja_env,
            schema_store.clone(),
            data_store.clone(),
            compiled_sql_cache.clone(),
            Arc::clone(&feature_stack.task_runner.task_runner_ctx_factory),
            static_analysis_buckets,
            run_cache,
        );
        let run_task_results = {
            if run_task_args.command == FsCommand::Extension("jinja-check")
                || should_skip_tasks_when_no_selected_nodes(&run_task_args.command, &schedule)
            {
                task_runner.into_empty_results()?
            } else {
                // Whether any selected node is in the dynamic closure so we only register hook UDFs
                // when strict/unsafe analysis will actually bind plans.
                let has_dynamic_closure = graph_builder.has_dynamic_closure();
                let (graph, generic_test_relationships) = {
                    let _sp = create_info_span(PhaseExecuted::start_general(
                        ExecutionPhase::TaskGraphBuild,
                    ))
                    .entered();
                    graph_builder.build(&schedule, &task_runner.resolved_state)
                }?;

                if run_task_args.io.should_show(ShowOptions::TaskGraph) {
                    task_runner.show_taskgraph(&graph);
                }

                task_runner
                    .register_seeds_for_selected_ids(run_task_args.as_ref(), &schedule)
                    .await?;

                let ctx = task_runner
                    .create_context(
                        Arc::clone(&run_task_args),
                        generic_test_relationships,
                        &graph,
                        base_context.clone(),
                        schedule.clone(),
                        freshness_results,
                    )
                    .await?;

                // Cheap clone (all fields are `Arc`s): keeps a handle to the
                // run's telemetry state even if `run` drops `ctx` on an early
                // error, so any failure still reports `SessionEnd` instead of
                // leaving the session open. No-ops if a session never started.
                let run_cache_ctx_on_error = ctx.clone();
                let run_result = task_runner
                    .run(
                        Arc::clone(&run_task_args),
                        schedule,
                        base_context,
                        ctx,
                        graph,
                        has_dynamic_closure,
                        token.clone(),
                    )
                    .await;
                if run_result.is_err() {
                    run_cache_service_after_run_failed(
                        &run_cache_ctx_on_error,
                        token.is_cancelled(),
                    )
                    .await;
                }
                run_result?
            }
        };

        // Shut down the adapter-level sidecar client so its dbt-db-runner
        // subprocess releases the DuckDB advisory lock before the next
        // invocation starts. In the in-process test model the tokio runtime is
        // shared across sequential TaskSeq steps, so without an explicit
        // shutdown the background reader tasks keep Arc<RunnerManager> alive
        // (preventing Drop from firing) and the runner holds the DB lock
        // indefinitely. The task-runner sidecar is shut down separately in
        // did_collect_all_run_task_results via ext.local_engine.shutdown().
        if let Some(client) = &sidecar_client {
            let _ = client.shutdown();
        }

        token.check_cancellation()?;

        // Flush the parquet schema cache to disk (no-op for non-ParquetCache stores).
        if let Err(e) = schema_store.save(&arg.io.out_dir) {
            dbt_common::tracing::dbt_emit::emit_debug_log_message(format!(
                "Failed to save schema cache: {e}"
            ));
        }

        // FEATURES: build_cache schema_hydration data_store
        let cache_state = DbtProjectCompilationCacheState {
            schema_store,
            data_store,
            schema_hydration_state,
            compiled_sql_cache,
        };

        let jinja_env = Arc::clone(&run_task_results.jinja_env);

        Ok((
            run_task_args,
            run_task_results,
            jinja_env,
            adapter,
            Arc::new(cache_state),
        ))
    }
}

#[async_trait::async_trait]
impl CompiledProject for DbtProjectCompilation {
    fn resolved_state(&self) -> &ResolverState {
        DbtProjectCompilation::resolved_state(self)
    }

    fn nodes(&self) -> &Nodes {
        DbtProjectCompilation::nodes(self)
    }

    fn loaded_project(&self) -> &DbtLoadedProject {
        DbtProjectCompilation::loaded_project(self)
    }

    fn root_project(&self) -> &DbtProject {
        DbtProjectCompilation::root_project(self)
    }

    fn adapter_type(&self) -> AdapterType {
        DbtProjectCompilation::adapter_type(self)
    }

    fn has_file_changed(&self, relative_path: &DbtPath) -> bool {
        DbtProjectCompilation::has_file_changed(self, relative_path)
    }

    fn create_jinja_env(&self, arg: &EvalArgs, token: CancellationToken) -> FsResult<JinjaEnv> {
        DbtProjectCompilation::create_jinja_env(self, arg, token)
    }

    fn lookup_ref(
        &self,
        maybe_package_name: &Option<String>,
        model_name: &str,
        name: &Option<String>,
        maybe_node_package_name: &Option<String>,
    ) -> Option<(String, ModelStatus)> {
        DbtProjectCompilation::lookup_ref(
            self,
            maybe_package_name,
            model_name,
            name,
            maybe_node_package_name,
        )
    }

    async fn create_schedule<'a>(
        &self,
        cli: &Cli,
        arg: &EvalArgs,
        schedule_desc: DbtScheduleDescription<'a>,
        exclude_unique_ids: HashSet<String>,
        token: &CancellationToken,
    ) -> FsResult<Schedule<String>> {
        DbtProjectCompilation::create_schedule(
            self,
            cli,
            arg,
            schedule_desc,
            exclude_unique_ids,
            token,
        )
        .await
    }

    fn macros(&self) -> &Macros {
        DbtProjectCompilation::macros(self)
    }

    fn root_project_name(&self) -> &str {
        DbtProjectCompilation::root_project_name(self)
    }

    fn root_project_id(&self) -> String {
        DbtProjectCompilation::root_project_id(self)
    }

    fn models_count(&self) -> u32 {
        DbtProjectCompilation::models_count(self)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn into_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync> {
        self
    }
}

/// Updates/fills the column information for each node in the [ResolverState] and [DbtManifestV12].
/// Updates the compiled source text for each node in [DbtManifestV12].
/// Updates macro depends on information in [DbtManifestV12].
/// Mutates [DbtManifestV12].
/// Returns a new [ResolverState].
pub(crate) fn update_manifest(
    run_task_args: &RunTasksArgs,
    type_ops_factory: &Arc<dyn TypeOpsFactory>,
    schema_store: &Arc<dyn SchemaStoreTrait>,
    arc_resolved_state: Arc<ResolverState>,
    macro_depends_on: &BTreeMap<String, BTreeSet<String>>,
    dbt_manifest: &mut DbtManifestV12,
) -> FsResult<ResolverState> {
    update_manifest_with_macro_depends_on(dbt_manifest, macro_depends_on);

    // TODO: Cloning resolved state here is inefficient, but we have some
    // non-deterministic behavior with regards to Arc references that means
    // we cannot use `get_mut` or `into_inner` to get a mutable reference to
    // the resolved state.
    let mut resolved_state = (*arc_resolved_state).clone();

    update_resolved_states_manifest_with_schemas_and_compiled_sql(
        run_task_args,
        type_ops_factory.as_ref(),
        &mut resolved_state,
        dbt_manifest,
        schema_store,
    )?;

    Ok(resolved_state)
}

/// Updates/fills the column information for each node in the [ResolverState].
/// Returns a new [ResolverState].
pub fn update_resolved_state_node_columns(
    run_task_args: &RunTasksArgs,
    type_ops_factory: &Arc<dyn TypeOpsFactory>,
    schema_store: &Arc<dyn SchemaStoreTrait>,
    arc_resolved_state: Arc<ResolverState>,
) -> FsResult<ResolverState> {
    // TODO: Cloning resolved state here is inefficient, but we have some
    // non-deterministic behavior with regards to Arc references that means
    // we cannot use `get_mut` or `into_inner` to get a mutable reference to
    // the resolved state.
    let mut resolved_state = (*arc_resolved_state).clone();

    update_columns_from_schemas(
        run_task_args,
        type_ops_factory.as_ref(),
        &mut resolved_state,
        schema_store,
    )?;

    Ok(resolved_state)
}

/// Spawn version check now that profile is loaded, only for remote execute mode.
/// Skipped for local execution (`--compute inline`).
fn spawn_version_check_if_possible(
    config: &CompilationConfig,
    compute_flag: LocalExecutionBackendKind,
    version_check_disabled: bool,
    command_name: &'static str,
) -> Option<tokio::task::JoinHandle<Option<String>>> {
    if version_check_disabled {
        return None;
    }
    let is_local = matches!(compute_flag, LocalExecutionBackendKind::Inline);
    if !is_local {
        let disable_version_check =
            { config.no_version_check || std::env::var("DBT_DISABLE_VERSION_CHECK").is_ok() };
        let current_version = env!("CARGO_PKG_VERSION");
        if !disable_version_check {
            return Some(tokio::spawn(
                version_check::check_version_and_build_hint(current_version, None, command_name)
                    .in_current_span(),
            ));
        }
    }
    None
}

fn set_eval_args_threads_and_target(arg: &EvalArgs, dbt_state: &DbtState) -> EvalArgs {
    EvalArgsBuilder::from_eval_args(arg)
        .with_additional(
            dbt_state.dbt_profile.target.to_string(),
            dbt_state.dbt_profile.threads,
            dbt_state.dbt_profile.default_db_config().adapter_type(),
        )
        .build()
}

/// Reconcile the `dbt_runtime` pool with the `threads` of this invocation.
///
/// The pool is built in `main_impl` before any profile is loaded, so it starts
/// with a default cap. This is called as soon as `threads` is resolved, and
/// before any node execution or metadata query, so that all jinja and database
/// work of the invocation runs with the parallelism the user asked for.
///
/// `--no-parallel` already pins the pool to a single thread, and `--threads 0`
/// means "as many as possible"; in both cases the cap set by `main_impl` stands.
fn set_runtime_max_parallelism(arg: &EvalArgs, dbt_state: &DbtState) {
    let threads = if arg.no_parallel {
        1
    } else {
        match arg.num_threads {
            // `--threads 0` asks for as many as possible.
            Some(0) => dbt_runtime::builder::DEFAULT_MAX_BLOCKING_THREADS,
            Some(threads) => threads,
            // select a default based on the adapter type (ensure non-zero).
            None => {
                let adapter_type = dbt_state.dbt_profile.default_db_config().adapter_type();
                adapter_type.default_max_threads().max(1)
            }
        }
    };
    dbt_runtime::Handle::current().set_max_parallelism(NonZeroUsize::new(threads).unwrap());
}

fn send_vortex_telemetry_if_possible(
    arg: &EvalArgs,
    dbt_state: &DbtState,
    maybe_event_emitter: Option<&dyn DiscreteEventEmitter>,
) {
    // Vortex telemetry
    let root_project_name = if !dbt_state.packages.is_empty() {
        dbt_state.root_project_name()
    } else {
        ""
    };
    if arg.send_anonymous_usage_stats {
        if let Some(event_emitter) = maybe_event_emitter {
            event_emitter.invocation_start_event(
                &arg.io.invocation_id,
                root_project_name,
                Some(&dbt_state.dbt_profile.relative_profile_path),
                arg.command.as_str().to_string(),
            );
        }
        adapter_info_event(
            arg.io.invocation_id.to_string(),
            dbt_state
                .dbt_profile
                .default_db_config()
                .adapter_type()
                .to_string(),
            dbt_state
                .dbt_profile
                .default_db_config()
                .get_adapter_unique_id()
                .unwrap(),
        );
    }
}

async fn write_catalog(
    arg: &EvalArgs,
    loaded_project: &DbtLoadedProject,
    resolved_state: &ResolverState,
    jinja_env: &Arc<JinjaEnv>,
    token: &CancellationToken,
) -> FsResult<DbtCatalog> {
    let project_name = loaded_project.root_project_name();

    let execute = Execute::from_compute_flag(arg.local_execution_backend);
    let adapter = loaded_project.init_adapter(
        resolved_state,
        arg.replay.clone(),
        jinja_env,
        None,
        token,
        None,
        execute,
        if arg.infer_schemas_and_typeless {
            AdapterConnectionMode::Offline
        } else {
            AdapterConnectionMode::AllowRemote
        },
    )?;
    let mut jinja_env = Arc::unwrap_or_clone(jinja_env.clone());
    configure_compile_and_run_jinja_environment(&mut jinja_env, adapter.clone());
    let namespace_keys: Vec<String> = jinja_env
        .env
        .get_macro_namespace_registry()
        .map(|r| r.keys().map(|k| k.to_string()).collect())
        .unwrap_or_default();
    let base_context = build_operation_context_btreemap(
        resolved_state.node_resolver.clone(),
        &resolved_state.root_project_name,
        &resolved_state.nodes,
        resolved_state.defer_nodes.as_ref(),
        resolved_state.runtime_config.clone(),
        namespace_keys,
        None,
    );

    let relations = resolved_state
        .nodes
        .models
        .values()
        .map(|n| &n.__base_attr__)
        .chain(
            resolved_state
                .nodes
                .snapshots
                .values()
                .map(|n| &n.__base_attr__),
        )
        .chain(
            resolved_state
                .nodes
                .seeds
                .values()
                .map(|n| &n.__base_attr__),
        )
        .chain(
            resolved_state
                .nodes
                .sources
                .values()
                .map(|n| &n.__base_attr__),
        )
        .map(|base| {
            Arc::from(
                create_relation(
                    resolved_state.adapter_type,
                    base.database.clone(),
                    base.schema.clone(),
                    Some(base.alias.clone()),
                    None,
                    base.quoting,
                )
                .expect("Failed to create relations from nodes"),
            )
        })
        .collect::<Vec<_>>();
    write_catalog_json(
        &adapter,
        resolved_state,
        relations,
        jinja_env.as_ref(),
        project_name,
        &base_context,
        arg,
        20,
    )
    .await
}

/// Check if a select expression matches any macro's file path.
/// Returns the matched selector value if a macro was matched.
fn select_matches_macro(
    select: &dbt_common::node_selector::SelectExpression,
    macros: &Macros,
) -> Option<String> {
    use dbt_common::node_selector::{MethodName, SelectExpression};
    use std::path::Path;

    fn collect_path_file_values(expr: &SelectExpression, values: &mut Vec<(MethodName, String)>) {
        match expr {
            SelectExpression::Atom(criteria) => {
                if matches!(criteria.method, MethodName::Path | MethodName::File) {
                    if let Some(value) = criteria.value.as_str() {
                        values.push((criteria.method, value.to_string()));
                    }
                }
            }
            SelectExpression::And(exprs) | SelectExpression::Or(exprs) => {
                for e in exprs {
                    collect_path_file_values(e, values);
                }
            }
            SelectExpression::Exclude(e) => {
                collect_path_file_values(e, values);
            }
        }
    }

    let mut values = Vec::new();
    collect_path_file_values(select, &mut values);

    for (method, value) in &values {
        for macro_node in macros.macros.values() {
            let macro_path = &macro_node.original_file_path;
            match method {
                MethodName::Path => {
                    let selector_path = Path::new(value);
                    if macro_path.as_path() == selector_path
                        || macro_path.starts_with(selector_path)
                    {
                        return Some(value.clone());
                    }
                }
                MethodName::File => {
                    if let Some(fname) = macro_path.file_name().and_then(|s| s.to_str()) {
                        if fname == value.as_str() {
                            return Some(value.clone());
                        }
                        if let Some(stem) = Path::new(fname).file_stem().and_then(|s| s.to_str()) {
                            if stem == value.as_str() {
                                return Some(value.clone());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn run_verify_partial_load(arg: &EvalArgs, schedule: &Schedule<String>) {
    use dbt_metadata::partial_parse::unique_id_filter_for_selector;
    use std::io::Write as _;

    let line = if let Some(filter) = unique_id_filter_for_selector(arg, &arg.io) {
        // Nodes the scheduler actually needs (selected + frontier).
        let scheduled: HashSet<&str> = schedule
            .selected_nodes
            .iter()
            .chain(schedule.frontier_nodes.iter())
            .map(|s| s.as_str())
            .collect();

        // Nodes in filter but not needed by scheduler (over-approximation, OK).
        let mut extra: Vec<&str> = filter
            .iter()
            .map(|s| s.as_str())
            .filter(|id| !scheduled.contains(id))
            .collect();
        extra.sort_unstable();

        // Nodes the scheduler needs but filter missed (correctness violations).
        let mut missing: Vec<&str> = scheduled
            .iter()
            .copied()
            .filter(|id| !filter.contains(*id))
            .collect();
        missing.sort_unstable();

        let status = if missing.is_empty() {
            "PASSED"
        } else {
            "FAILED"
        };

        let mut detail = format!(
            "{status} scheduled={} (selected={} frontier={}) filter={} extra=+{} missing=-{}",
            scheduled.len(),
            schedule.selected_nodes.len(),
            schedule.frontier_nodes.len(),
            filter.len(),
            extra.len(),
            missing.len(),
        );

        // Append per-node annotations when there are discrepancies.
        if !extra.is_empty() || !missing.is_empty() {
            detail.push_str(" |");
            for id in &extra {
                detail.push_str(&format!(" +{id}"));
            }
            for id in &missing {
                detail.push_str(&format!(" -{id}"));
            }
        }
        detail.push('\n');
        detail
    } else {
        format!(
            "SKIP scheduled={} (selected={} frontier={}) selector not eligible for partial-load filter\n",
            schedule.selected_nodes.len() + schedule.frontier_nodes.len(),
            schedule.selected_nodes.len(),
            schedule.frontier_nodes.len(),
        )
    };

    // Write to DBT_VERIFY_PARTIAL_LOAD_LOG if set; otherwise fall back to tracing so
    // the result is still visible in the normal log file.
    if let Ok(path) = std::env::var("DBT_VERIFY_PARTIAL_LOAD_LOG") {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    } else if line.starts_with("FAILED") {
        tracing::warn!("verify-partial-load: {}", line.trim());
    } else {
        tracing::info!("verify-partial-load: {}", line.trim());
    }
}

async fn create_run_cache_state_selector_args(
    arg: &EvalArgs,
    resolved_state: &ResolverState,
    cloud_config: Option<&ResolvedCloudConfig>,
    project_root: &std::path::Path,
) -> FsResult<Option<RunCacheStateSelectorArgs>> {
    let execute_mode = Execute::from_compute_flag(arg.local_execution_backend);
    let run_task_args = RunTasksArgs::from_eval_args(arg, FailFast::new());
    let lifecycle = RunCacheLifecycle::get_or_initialize(
        run_task_args.as_ref(),
        execute_mode,
        resolved_state.adapter_type,
        cloud_config,
    )
    .await?;

    let Some(client) = lifecycle.service_client() else {
        return Ok(None);
    };

    let defer_to = lifecycle
        .defer_to_target(&resolved_state.dbt_profile)
        .unwrap_or_default();
    let project_id = cloud_config.and_then(|c| c.project_id.clone());

    Ok(Some(RunCacheStateSelectorArgs {
        client,
        defer_to,
        project_id,
        macros: resolved_state.macros.macros.clone(),
        project_root: DbtPath::from(project_root),
    }))
}

/// Convert the `parse` metadata epochs into whichever opt-in artifacts were
/// requested.
///
/// Both artifact sets are independent: either, neither, or both may be asked
/// for. Failures are advisory — an artifact that could not be written must not
/// fail the command.
///
/// What `parse` can produce is limited: nodes and node-level lineage, but no
/// column types and no column-level lineage, both of which need a compile.
fn write_parse_artifacts(arg: &EvalArgs) {
    let metadata_dir = arg.metadata_dir();

    if arg.write_index {
        let index_dir = arg.index_dir();
        let mut state = IngestState::default();
        // `ingest_from_metadata_direct` creates the directory, so the
        // fingerprint below has somewhere to land.
        match ingest_from_metadata_direct(&metadata_dir, &index_dir, &mut state) {
            Ok(_) => {
                if let Err(e) =
                    save_artifact_meta(&index_dir, &arg.io.out_dir, WriteSource::DirectWrite, None)
                {
                    emit_warn_log_message(
                        ErrorCode::Generic,
                        format!("dbt-index: save_artifact_meta: {e}"),
                    );
                }
                emit_warn_log_message(
                    ErrorCode::Generic,
                    "--write-index: the index produced by `parse` is incomplete; column schemas and column-level lineage are only written by `compile`, `run`, or `build`.",
                );
            }
            // `Generic`, like the advisory above: this arm is only reachable through
            // `parse --write-index`, and that flag is undocumented. The index failures a
            // user can actually meet -- `build`/`run`/`check`, where the index is implied
            // on -- are reported from `dbt_lib` under `IndexWriteFailed`.
            Err(e) => {
                emit_warn_log_message(ErrorCode::Generic, format!("dbt-index: write-index: {e}"))
            }
        }
    }

    if arg.generate_info_schema {
        let info_schema_dir = arg.info_schema_dir();
        // Reuse the flat index at `target/private/index` as the intermediate when one is
        // already there (the same ingest builds both, so an index written earlier
        // in this run is picked up via the delta path rather than re-ingested).
        // With no index to reuse — e.g. `--no-write-index` — stage privately
        // instead, so we never materialise an index the caller opted out of.
        let index_dir = arg.index_dir();
        let staging_dir = if has_persisted_state(&index_dir) {
            index_dir
        } else {
            arg.info_schema_staging_dir()
        };
        match write_info_schema(&metadata_dir, &info_schema_dir, &staging_dir) {
            Ok(_) => emit_warn_log_message(
                ErrorCode::InfoSchemaIncomplete,
                "--generate-info-schema: the information schema produced by `parse` is incomplete; column types, column-level lineage, and runtime results are only written by `compile`, `run`, or `build`.",
            ),
            Err(e) => emit_warn_log_message(
                ErrorCode::InfoSchemaWriteFailed,
                format!("dbt: generate-info-schema: {e}"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_tasks_for_empty_selection_on_runnable_commands() {
        let schedule = Schedule::<String>::default();

        assert!(should_skip_tasks_when_no_selected_nodes(
            &FsCommand::Run,
            &schedule
        ));
        assert!(should_skip_tasks_when_no_selected_nodes(
            &FsCommand::Test,
            &schedule
        ));
        assert!(should_skip_tasks_when_no_selected_nodes(
            &FsCommand::Build,
            &schedule
        ));
        assert!(should_skip_tasks_when_no_selected_nodes(
            &FsCommand::Seed,
            &schedule
        ));
        assert!(should_skip_tasks_when_no_selected_nodes(
            &FsCommand::Snapshot,
            &schedule
        ));

        // Non-runnable commands should not be short-circuited here.
        assert!(!should_skip_tasks_when_no_selected_nodes(
            &FsCommand::Compile,
            &schedule
        ));
        assert!(!should_skip_tasks_when_no_selected_nodes(
            &FsCommand::Parse,
            &schedule
        ));
        assert!(!should_skip_tasks_when_no_selected_nodes(
            &FsCommand::List,
            &schedule
        ));
        assert!(!should_skip_tasks_when_no_selected_nodes(
            &FsCommand::Source,
            &schedule
        ));
        assert!(!should_skip_tasks_when_no_selected_nodes(
            &FsCommand::Freshness,
            &schedule
        ));
    }
}
