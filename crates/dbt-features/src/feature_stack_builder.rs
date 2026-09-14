use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dbt_adapter::adapter::DefaultAdapterFactory;
use dbt_adapter::sql_types::DefaultTypeOpsFactory;
use dbt_common::FsError;
use dbt_common::collections::DashMap;
use dbt_dag::schedule::Schedule;
use dbt_frontend_common::sources_extractor::SourcesExtractor;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::{
    DefaultRenderingEventListenerFactory, RenderingEventListenerFactory,
};
use dbt_login::DefaultLoginHooks;
use dbt_parser::resolver_hooks::NoOpResolverHooks;
use dbt_schemas::state::ResolverState;
use dbt_tasks_core::context::ExtendedCtx;
use dbt_tasks_core::context_factory::TaskRunnerCtxFactory;
use dbt_tasks_core::{PreTaskRunData, RunTasksArgs};
use dbt_tasks_sa::schema_hydrator::DefaultSchemaHydratorFactory;
use dbt_tasks_sa::sources_extractor::DefaultSourcesExtractor;
use dbt_tasks_sa::task::DefaultTasksForNodeFactory;
use dbt_tasks_sa::task_runner_hooks::DefaultTaskRunnerHooksFactory;

use crate::adapter::AdapterFeature;
use crate::antlr_parser::AntlrParserFeature;
use crate::cli::CliFeatureBuilder;
use crate::feature_stack::{FeatureStack, InstrumentationFeature};
use crate::index::{IndexFeature, NoOpIndexHooks};
use crate::loader::LoaderFeature;
use crate::login::LoginFeature;
use crate::metricflow::MetricflowFeature;
use crate::resolver::ResolverFeature;
use crate::sidecar::SidecarFeature;
use crate::task_runner::TaskRunnerFeature;
use crate::tracing::TracingFeature;

struct DefaultTaskRunnerCtxFactory {
    rendering_listener_factory: Arc<dyn RenderingEventListenerFactory>,
    sources_extractor: Arc<dyn SourcesExtractor>,
}
impl DefaultTaskRunnerCtxFactory {
    fn new(
        rendering_listener_factory: Arc<dyn RenderingEventListenerFactory>,
        sources_extractor: Arc<dyn SourcesExtractor>,
    ) -> Self {
        Self {
            rendering_listener_factory,
            sources_extractor,
        }
    }
}

impl TaskRunnerCtxFactory for DefaultTaskRunnerCtxFactory {
    fn rendering_listener_factory(&self) -> Arc<dyn RenderingEventListenerFactory> {
        Arc::clone(&self.rendering_listener_factory)
    }

    fn sources_extractor(&self) -> Arc<dyn SourcesExtractor> {
        Arc::clone(&self.sources_extractor)
    }

    fn build_node_hashes<'a>(
        &'a self,
        _arg: &'a RunTasksArgs,
        _schedule: &'a Schedule<String>,
        _worker_id: &'a str,
        _resolver_state: &'a ResolverState,
        _env: &'a Arc<JinjaEnv>,
        _freshness_results: Option<&'a dyn PreTaskRunData>,
        _extended_ctx: &'a dyn ExtendedCtx,
    ) -> Pin<Box<dyn Future<Output = Result<DashMap<String, String>, Box<FsError>>> + Send + 'a>>
    {
        Box::pin(async move { Ok(DashMap::default()) })
    }
}

pub struct FeatureStackBuilder {
    name: &'static str,
    distribution: &'static str,
    tracing: TracingFeature,
}

impl FeatureStackBuilder {
    pub fn new(tracing: TracingFeature) -> Self {
        Self {
            name: "dbt-oss",
            distribution: "dbt-oss",
            tracing,
        }
    }

    pub fn build(self) -> Box<FeatureStack> {
        let instrumentation = InstrumentationFeature {
            event_emitter: vortex_events::default_event_emitter(None, self.distribution),
        };

        let cli = CliFeatureBuilder::new(self.name)
            .version_check_enabled(false)
            .build();

        let index = IndexFeature {
            hooks: Box::new(NoOpIndexHooks),
            providers_factory: crate::index::default_providers_factory,
        };

        let adapter = {
            let type_ops_factory = Arc::new(DefaultTypeOpsFactory);
            let adapter_factory = Arc::new(DefaultAdapterFactory);

            AdapterFeature {
                type_ops_factory,
                adapter_factory,
            }
        };

        let antlr_parser = AntlrParserFeature::default();
        let sidecar = SidecarFeature { factory: None };
        let metricflow = MetricflowFeature::default();

        let task_runner = {
            let rendering_listener_factory: Arc<dyn RenderingEventListenerFactory> =
                Arc::new(DefaultRenderingEventListenerFactory::default());

            let sources_extractor = Arc::new(DefaultSourcesExtractor) as Arc<dyn SourcesExtractor>;

            let task_runner_ctx_factory = Arc::new(DefaultTaskRunnerCtxFactory::new(
                Arc::clone(&rendering_listener_factory),
                Arc::clone(&sources_extractor),
            )) as Arc<dyn TaskRunnerCtxFactory>;

            TaskRunnerFeature {
                schema_hydrator_factory: Arc::new(DefaultSchemaHydratorFactory),
                tasks_for_node_factory: Arc::new(DefaultTasksForNodeFactory),
                compare_task_graph_builder: None,
                rendering_listener_factory,
                sources_extractor,
                task_runner_ctx_factory,
                hooks_factory: Arc::new(DefaultTaskRunnerHooksFactory),
            }
        };

        let resolver = {
            let hooks = Arc::new(NoOpResolverHooks);
            ResolverFeature { hooks }
        };

        let loader = LoaderFeature::default();

        let login = LoginFeature {
            hooks: Arc::new(DefaultLoginHooks),
        };

        let jinja = crate::jinja::JinjaFeature {
            factory: Arc::new(dbt_jinja_utils::DefaultJinjaFactory),
        };

        let lake_compute = crate::lake_compute::LakeComputeFeature {
            propagation_checker: None,
            catalog_attach_checker: None,
            mdls_checker: None,
        };

        let stack = FeatureStack {
            instrumentation,
            cli,
            index,
            tracing: self.tracing,
            adapter,
            antlr_parser,
            sidecar,
            metricflow,
            task_runner,
            resolver,
            loader,
            jinja,
            lake_compute,
            login,
        };
        Box::new(stack)
    }
}
