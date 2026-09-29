use std::fmt;

use dbt_common::DiscreteEventEmitter;

use crate::adapter::AdapterFeature;
use crate::antlr_parser::AntlrParserFeature;
use crate::cli::CliFeature;
use crate::index::IndexFeature;
use crate::jinja::JinjaFeature;
use crate::lake_compute::LakeComputeFeature;
use crate::loader::LoaderFeature;
use crate::login::LoginFeature;
use crate::metricflow::MetricflowFeature;
use crate::resolver::ResolverFeature;
use crate::sidecar::SidecarFeature;
use crate::task_runner::TaskRunnerFeature;
use crate::tracing::TracingFeature;

/// The instrumentation feature. Exposed as a set of instrumentation services.
pub struct InstrumentationFeature {
    pub event_emitter: Box<dyn DiscreteEventEmitter>,
    // TODO: add more instrumentation services here
}

impl InstrumentationFeature {
    fn configure(&mut self, config: &FeatureStackConfig) {
        self.event_emitter
            .configure(config.send_anonymous_usage_stats);
    }
}

/// A feature stack is an object that can be initialized with containers of
/// type-erased objects that implement feature-specific services.
///
/// It serves as the root of the dependency graph for all features [1] and a
/// very simple abstract state-machine for the setup and teardown of features.
///
/// The crates implementing features should not depend on the [FeatureStack]
/// struct directly or even the `*Feature` structs, but the more granular
/// services that the features expose (e.g. `SchemaHydratorFactory` in the
/// `TaskRunnerFeature`). If you violate this principle you will end up with
/// a circular dependency and won't be able to build the project.
///
/// Not everything should necessarily go in the feature stack. In fact, the
/// requirements for what goes in the feature stack are very restrictive. These
/// objects should be buildable before everything including the setup of the CLI
/// parser and should only depend on other services in the feature stack, hence
/// the name "stack". Features are stacked on top of each other and can only
/// depend on features below them in the stack. Do not cheat by introducing
/// global singletons or static variables to get around having to pass
/// dependencies through the feature stack.
///
/// [1] https://martinfowler.com/articles/injection.html
pub struct FeatureStack {
    pub instrumentation: InstrumentationFeature,
    pub tracing: TracingFeature,
    pub cli: CliFeature,
    pub index: IndexFeature,
    pub adapter: AdapterFeature,
    pub antlr_parser: AntlrParserFeature,
    pub sidecar: SidecarFeature,
    pub metricflow: MetricflowFeature,
    pub task_runner: TaskRunnerFeature,
    pub resolver: ResolverFeature,
    pub loader: LoaderFeature,
    pub jinja: JinjaFeature,
    pub lake_compute: LakeComputeFeature,
    pub login: LoginFeature,
}

/// Configuration that can only be known after [FeatureStack] is built.
///
/// For instance, we need the [CliFeature] to parse the CLI arguments to know if
/// we should send anonymous usage stats or not. The [FeatureStack] should be
/// constructred before everything else, so we need the ability to defer a
/// small set of configuration knobs.
#[derive(Default)]
pub struct FeatureStackConfig {
    pub send_anonymous_usage_stats: bool,
}

impl FeatureStack {
    pub fn configure(mut self, config: &FeatureStackConfig) -> Self {
        self.instrumentation.configure(config);
        self
    }
}

impl fmt::Debug for FeatureStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeatureStack").finish()
    }
}
