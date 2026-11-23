//! The kinds of node a package defines, and what they resolve with.

use crate::args::ResolveArgs;
use crate::dbt_project_config::RootProjectConfigs;
pub use crate::resolve::properties_index::{IndexCtx, IndexedName};
use crate::resolve::properties_index::{PackageProperties, PropertiesEntry, section_of};
use crate::resolve::resolve_analyses::Analyses;
use crate::resolve::resolve_checks::Checks;
use crate::resolve::resolve_exposures::Exposures;
use crate::resolve::resolve_functions::Functions;
use crate::resolve::resolve_groups::Groups;
use crate::resolve::resolve_macros::MacroProperties;
use crate::resolve::resolve_metrics::Metrics;
use crate::resolve::resolve_models::Models;
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
use dbt_common::io_args::project_flag_bool;
use dbt_flags::FlagDef;
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

/// One kind of node a package defines: the properties it owns, and how its nodes of one package
/// resolve.
#[async_trait]
pub trait ResolvableNode: Send + Sync {
    /// The feature flag that turns the kind on. A kind without one is always on.
    ///
    /// The items under the keys of a kind that is off are ignored, with a warning that names the
    /// flag.
    fn flag(&self) -> Option<&'static FlagDef> {
        None
    }

    /// The top-level keys of properties files that hold the kind's items.
    ///
    /// A key may be a section's: `section.key` is the list under `key` in the top-level mapping
    /// `section`. Kinds can own keys of the same section.
    fn properties_keys(&self) -> &'static [&'static str] {
        &[]
    }

    /// The names that `item`, under the kind's key `key`, defines.
    ///
    /// The default is one name, found under `key`: the item's `name`, with Jinja rendered.
    /// Nothing else of the item is read. An error fails the package's resolution.
    async fn index_item(
        &self,
        key: &'static str,
        item: &dbt_yaml::Value,
        ctx: &IndexCtx<'_>,
    ) -> FsResult<Vec<IndexedName>> {
        ctx.named(key, item)
    }

    /// Resolves the kind's nodes of `ctx.package`, and adds them to `ctx.nodes` and
    /// `ctx.disabled_nodes`.
    async fn resolve(&self, ctx: &mut ResolveCtx<'_>) -> FsResult<()>;
}

/// The kinds of node a project resolves with, in the order they resolve. The items of a
/// properties file are indexed kind by kind in the same order.
///
/// A kind reads the nodes of the kinds before it. Data tests read the sources, seeds, snapshots
/// and models that declare them. Semantic models and unit tests read models.
#[derive(Clone)]
pub struct ResolvableNodes {
    nodes: Vec<Arc<dyn ResolvableNode>>,
}

impl Default for ResolvableNodes {
    fn default() -> Self {
        Self::dbt()
    }
}

impl ResolvableNodes {
    /// dbt's own kinds of node.
    pub fn dbt() -> Self {
        ResolvableNodes {
            nodes: vec![
                Arc::new(Sources),
                Arc::new(Seeds),
                Arc::new(Snapshots),
                Arc::new(Groups),
                Arc::new(Models),
                Arc::new(Analyses),
                Arc::new(Checks),
                Arc::new(Functions),
                Arc::new(Exposures),
                Arc::new(SemanticModels),
                Arc::new(Metrics),
                Arc::new(SavedQueries),
                Arc::new(DataTests),
                Arc::new(UnitTests),
                Arc::new(MacroProperties),
            ],
        }
    }

    /// Adds `node` after the kinds added so far. It resolves after them.
    ///
    /// # Panics
    ///
    /// If a key of `node` is a section's and a kind owns the section's name as a top-level key,
    /// or the reverse: `parts.bolts` and `parts`.
    pub fn with(mut self, node: Arc<dyn ResolvableNode>) -> Self {
        self.nodes.push(node);
        let keys: Vec<&str> = self.properties_keys().collect();
        for key in &keys {
            if let Some(section) = section_of(key) {
                assert!(
                    !keys.contains(&section),
                    "`{section}` is a top-level key and the section of `{key}`"
                );
            }
        }
        self
    }

    /// The top-level keys of properties files that the kinds own.
    pub fn properties_keys(&self) -> impl Iterator<Item = &'static str> {
        self.nodes
            .iter()
            .flat_map(|node| node.properties_keys())
            .copied()
    }

    /// Sorts the kinds by whether their flag is on, reading the root project's `flags:`.
    pub(crate) fn enabled(self, project_flags: Option<&dbt_yaml::Value>) -> EnabledNodes {
        let mut enabled = EnabledNodes {
            on: Vec::new(),
            off_keys: BTreeMap::new(),
        };
        for node in self.nodes {
            match node.flag() {
                Some(flag)
                    if !flag
                        .resolve_with_project(&project_flag_bool(project_flags))
                        .value =>
                {
                    for key in node.properties_keys() {
                        enabled.off_keys.insert(*key, flag);
                    }
                }
                _ => enabled.on.push(node),
            }
        }
        enabled
    }
}

/// The kinds of node of one invocation: the ones that are on, and the keys of the others.
pub struct EnabledNodes {
    on: Vec<Arc<dyn ResolvableNode>>,
    off_keys: BTreeMap<&'static str, &'static FlagDef>,
}

impl EnabledNodes {
    /// The kinds that are on, in the order they resolve.
    pub(crate) fn on(&self) -> Vec<&dyn ResolvableNode> {
        self.on.iter().map(|node| node.as_ref()).collect()
    }

    /// The top-level keys of the kinds that are off, with the flag that turns each kind on.
    pub(crate) fn off_keys(&self) -> &BTreeMap<&'static str, &'static FlagDef> {
        &self.off_keys
    }
}

/// What resolving one package's nodes reads, and what it has resolved so far.
///
/// The kinds of node resolve one after another. Each reads what it needs from here and adds its
/// nodes to `nodes` and `disabled_nodes`, where the kinds after it find them.
pub struct ResolveCtx<'a> {
    pub(crate) arg: &'a ResolveArgs,
    /// The package whose nodes resolve.
    pub(crate) package: &'a DbtPackage,
    pub(crate) dbt_state: Arc<DbtState>,
    pub(crate) root_package_name: &'a str,
    pub(crate) root_project_configs: &'a RootProjectConfigs,
    /// The target's default adapter.
    pub(crate) adapter_type: AdapterType,
    pub(crate) parse_adapter_scopes: Arc<AdapterScopes>,
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

    /// The package's properties files, and the index of what they define. A kind takes its own
    /// entries.
    pub(crate) properties: PackageProperties,
    /// The entries under `models:`, by the name each is found under. Several kinds read them.
    pub(crate) model_properties: BTreeMap<String, PropertiesEntry>,
    /// The properties of the package's macros. They are applied once every package has resolved.
    pub(crate) macro_properties: BTreeMap<String, PropertiesEntry>,
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
