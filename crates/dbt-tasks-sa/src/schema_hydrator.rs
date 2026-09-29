use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use dbt_adapter::Adapter;
use dbt_adapter::engine::SidecarClient;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::{EvalArgs, StaticAnalysisKind};
use dbt_common::pretty_string::CYAN;
use dbt_common::static_analysis::is_strict_static_analysis;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{ErrorCode, FsResult};
use dbt_compilation::config::CompilationConfig;
use dbt_compilation::schema_hydration::{
    SchemaHydrationState, SchemaHydrator, SchemaHydratorFactory,
};
use dbt_dag::schedule::Schedule;
use dbt_defer::DeferState;
use dbt_schema_store::CanonicalFqn;
use dbt_schema_store::store::SchemaStore;
use dbt_schemas::schemas::{IntrospectionKind, Nodes};
use dbt_schemas::state::ResolverState;
use dbt_tasks_core::RunTasksArgs;
use dbt_tasks_core::metricflow::MetricflowClient;
use dbt_tasks_core::static_analysis_buckets::StaticAnalysisBuckets;

static EMPTY_DEFERRED: std::sync::LazyLock<HashMap<CanonicalFqn, String>> =
    std::sync::LazyLock::new(HashMap::new);

/// `StaticAnalysisBuckets` implementation that carries the deferred-unique-ids
/// map produced by `defer_common` and returns it for run-cache lenient-dependency
/// matching. All SA classification methods report off/empty — this impl does not
/// rely on Fusion's static analysis infrastructure.
pub struct DefaultStaticAnalysisBuckets {
    deferred: HashMap<CanonicalFqn, String>,
}

impl DefaultStaticAnalysisBuckets {
    pub fn new(deferred: HashMap<CanonicalFqn, String>) -> Self {
        Self { deferred }
    }
}

impl StaticAnalysisBuckets for DefaultStaticAnalysisBuckets {
    fn global_static_analysis(&self) -> Option<StaticAnalysisKind> {
        Some(StaticAnalysisKind::Off)
    }

    fn deferred_unique_ids(&self) -> &HashMap<CanonicalFqn, String> {
        &self.deferred
    }

    fn in_off_closure(&self, _node_id: &str) -> bool {
        true
    }

    fn in_baseline_closure(&self, _node_id: &str) -> bool {
        false
    }

    fn in_dynamic_closure(&self, _node_id: &str) -> bool {
        false
    }

    fn dynamic_node(&self, _node_id: &str) -> Option<IntrospectionKind> {
        None
    }

    fn has_dynamic_closure(&self) -> bool {
        false
    }

    fn will_build_phased_task_graph(&self, _arg: &RunTasksArgs, _task_nodes: &Nodes) {}

    fn did_build_phased_task_graph(
        &self,
        _arg: &RunTasksArgs,
        _nodes_with_no_tasks: &BTreeSet<String>,
    ) {
    }
}

/// A `StaticAnalysisBuckets` that treats all nodes as not part of the static analysis process.
pub struct NoopStaticAnalysisBuckets;

impl StaticAnalysisBuckets for NoopStaticAnalysisBuckets {
    fn global_static_analysis(&self) -> Option<StaticAnalysisKind> {
        Some(StaticAnalysisKind::Off)
    }

    fn deferred_unique_ids(&self) -> &HashMap<CanonicalFqn, String> {
        &EMPTY_DEFERRED
    }

    fn in_off_closure(&self, _node_id: &str) -> bool {
        true
    }

    fn in_baseline_closure(&self, _node_id: &str) -> bool {
        false
    }

    fn in_dynamic_closure(&self, _node_id: &str) -> bool {
        false
    }

    fn dynamic_node(&self, _node_id: &str) -> Option<IntrospectionKind> {
        None
    }

    fn has_dynamic_closure(&self) -> bool {
        false
    }

    fn will_build_phased_task_graph(&self, _arg: &RunTasksArgs, _task_nodes: &Nodes) {}

    fn did_build_phased_task_graph(
        &self,
        _arg: &RunTasksArgs,
        _nodes_with_no_tasks: &BTreeSet<String>,
    ) {
    }
}

/// A `SchemaHydrator` that runs the defer pipeline (synthesize + load state +
/// defer_common + defer_sa_upstreams + fixup) without performing schema
/// hydration or static analysis. This gives the SA binary the same
/// ref-resolution behaviour as Fusion for run-cache auto-deferral and
/// explicit `--defer`/`--state` flags, including SA-dependent upstream
/// deferral for introspective (Execute/Unknown) nodes.
pub struct DefaultSchemaHydrator {
    adapter: Arc<Adapter>,
}

#[async_trait::async_trait]
impl SchemaHydrator for DefaultSchemaHydrator {
    async fn hydrate_schemas(
        self: Box<Self>,
        arg: &EvalArgs,
        schedule: &Schedule<String>,
        resolved_state: &mut ResolverState,
        _schema_hydration_state: &mut SchemaHydrationState,
        defer_state: &mut DeferState,
        token: CancellationToken,
    ) -> FsResult<Box<dyn StaticAnalysisBuckets>> {
        if arg.static_analysis.is_some_and(is_strict_static_analysis) {
            emit_warn_log_message(
                ErrorCode::Generic,
                format!(
                    "static analysis was requested with `--static-analysis strict` but did not run: \
                     this distribution of dbt OSS does not include the static analysis engine. \
                     Install the full dbt distribution to enable it: {}",
                    CYAN.apply_to("dbt system upgrade-distribution")
                ),
            );
        }

        if let Some(defer_nodes) = defer_state.defer_nodes.as_mut() {
            let relation_remap = dbt_defer::defer_sa_upstreams(
                arg,
                resolved_state,
                defer_nodes,
                &mut defer_state.deferred_unique_ids,
                schedule,
                &self.adapter,
            )
            .await?;
            token.check_cancellation()?;
            dbt_defer::rewrite_recorded_relation_calls_with_deferral(
                resolved_state,
                &relation_remap,
            );
        }

        Ok(Box::new(DefaultStaticAnalysisBuckets::new(
            defer_state.deferred_unique_ids.clone(),
        )))
    }
}

/// Factory that produces `DeferSchemaHydrator` instances.
#[derive(Default)]
pub struct DefaultSchemaHydratorFactory;

impl SchemaHydratorFactory for DefaultSchemaHydratorFactory {
    fn create(
        &self,
        adapter: Arc<Adapter>,
        _execute_mode: dbt_schemas::schemas::profiles::Execute,
        _compilation_config: CompilationConfig,
        _schema_store: Arc<SchemaStore>,
        _sidecar_client: Option<Arc<dyn SidecarClient>>,
        _metricflow_server_client: Option<Arc<dyn MetricflowClient>>,
    ) -> Box<dyn SchemaHydrator> {
        Box::new(DefaultSchemaHydrator { adapter })
    }
}
