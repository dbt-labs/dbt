//! The kinds of node a package defines, and what they resolve with.

use crate::args::ResolveArgs;
use crate::dbt_project_config::RootProjectConfigs;
use crate::experimental::ActiveExperimentals;
use crate::resolve::resolve_analyses::Analyses;
use crate::resolve::resolve_checks::Checks;
use crate::resolve::resolve_experimentals::Experimentals;
use crate::resolve::resolve_exposures::Exposures;
use crate::resolve::resolve_functions::Functions;
use crate::resolve::resolve_groups::Groups;
use crate::resolve::resolve_metrics::Metrics;
use crate::resolve::resolve_models::Models;
use crate::resolve::resolve_properties::MinimalProperties;
use crate::resolve::resolve_saved_queries::SavedQueries;
use crate::resolve::resolve_seeds::Seeds;
use crate::resolve::resolve_semantic_models::SemanticModels;
use crate::resolve::resolve_snapshots::Snapshots;
use crate::resolve::resolve_sources::Sources;
use crate::resolve::resolve_tests::resolve_data_tests::DataTests;
use crate::resolve::resolve_tests::resolve_unit_tests::UnitTests;
use async_trait::async_trait;
use dbt_adapter_core::AdapterType;
use dbt_common::FsResult;
use dbt_common::cancellation::CancellationToken;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::JinjaTypeCheckingEventListenerFactory;
use dbt_jinja_utils::node_resolver::NodeResolver;
use dbt_jinja_utils::phases::AdapterScopes;
use dbt_schemas::schemas::Nodes;
use dbt_schemas::schemas::common::DbtQuoting;
use dbt_schemas::schemas::properties::ModelProperties;
use dbt_schemas::state::{DbtPackage, DbtRuntimeConfig, DbtState, GenericTestAsset, Macros};
use minijinja::Value as MinijinjaValue;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

/// One kind of node a package defines: how its nodes of one package resolve.
#[async_trait]
pub(crate) trait ResolvableNode: Send + Sync {
    /// Resolves the kind's nodes of `cx.package`, and adds them to `cx.nodes` and
    /// `cx.disabled_nodes`.
    async fn resolve(&self, cx: &mut ResolveCx<'_>) -> FsResult<()>;
}

/// dbt's kinds of node, in the order they resolve.
///
/// A kind reads the nodes of the kinds before it. Data tests read the sources, seeds, snapshots
/// and models that declare them. Semantic models and unit tests read models.
pub(crate) static RESOLVABLE_NODES: [&dyn ResolvableNode; 15] = [
    &Sources,
    &Seeds,
    &Snapshots,
    &Groups,
    &Models,
    &Analyses,
    &Checks,
    &Functions,
    &Exposures,
    &Experimentals,
    &SemanticModels,
    &Metrics,
    &SavedQueries,
    &DataTests,
    &UnitTests,
];

/// What resolving one package's nodes reads, and what it has resolved so far.
///
/// The kinds of node resolve one after another. Each reads what it needs from here and adds its
/// nodes to `nodes` and `disabled_nodes`, where the kinds after it find them.
pub(crate) struct ResolveCx<'a> {
    pub(crate) arg: &'a ResolveArgs,
    /// The package whose nodes resolve.
    pub(crate) package: &'a DbtPackage,
    pub(crate) dbt_state: &'a DbtState,
    pub(crate) root_package_name: &'a str,
    pub(crate) root_project_configs: &'a RootProjectConfigs,
    /// The target's default adapter.
    pub(crate) adapter_type: AdapterType,
    pub(crate) parse_adapter_scopes: &'a Arc<AdapterScopes>,
    pub(crate) macros: &'a Macros,
    pub(crate) jinja_env: Arc<JinjaEnv>,
    /// The Jinja context of the package's properties files.
    pub(crate) base_ctx: Arc<BTreeMap<String, MinijinjaValue>>,
    /// The package's runtime config.
    pub(crate) runtime_config: Arc<DbtRuntimeConfig>,
    pub(crate) root_runtime_config: Arc<DbtRuntimeConfig>,
    pub(crate) token: &'a CancellationToken,
    pub(crate) jinja_type_checking_event_listener_factory:
        Arc<dyn JinjaTypeCheckingEventListenerFactory>,
    pub(crate) experimentals: &'a ActiveExperimentals,
    /// The profile's database.
    pub(crate) database: &'a str,
    /// The profile's schema.
    pub(crate) schema: &'a str,
    /// The name of `package`.
    pub(crate) package_name: &'a str,
    pub(crate) package_quoting: DbtQuoting,
    /// Whether the package defines its semantic layer with the legacy YAML.
    pub(crate) semantic_layer_spec_is_legacy: bool,
    /// The typed properties of the models that have a `semantic_model` or `metrics`.
    pub(crate) typed_models_properties: BTreeMap<String, ModelProperties>,

    /// The entries of the package's properties files. A kind consumes its own.
    pub(crate) min_properties: MinimalProperties,
    /// The package's nodes resolved so far.
    pub(crate) nodes: Nodes,
    /// The package's disabled nodes resolved so far.
    pub(crate) disabled_nodes: Nodes,
    /// The bodies of the package's functions, by unique id.
    pub(crate) function_bodies: HashMap<String, String>,
    pub(crate) node_resolver: NodeResolver,
    /// The generic tests that the kinds resolved so far declare, for data tests to resolve.
    pub(crate) collected_generic_tests: Vec<GenericTestAsset>,
    pub(crate) test_name_truncations: &'a mut HashMap<String, String>,
    pub(crate) seen_generic_test_paths: &'a mut HashMap<PathBuf, String>,
}
