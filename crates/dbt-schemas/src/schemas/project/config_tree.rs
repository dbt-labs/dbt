//! Module defines the global project configuration, which is used to
//! load and propagate configuration properties from the root `dbt_project.yml`
//! to the individual model directories.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use dbt_common::warn_error_options::project_flags_get_value;
use dbt_common::{ErrorCode, FsError};
use indexmap::IndexMap;

use crate::schemas::{common::DbtQuoting, project::DbtProject};
use crate::schemas::{
    project::{
        AdapterProjectConfig, AnalysesConfig, CheckConfig, DataTestConfig, ExposureConfig,
        FunctionConfig, MetricConfig, ModelConfig, ProjectDataTestConfig, ProjectFunctionConfig,
        ProjectModelConfig, ProjectSeedConfig, ProjectSnapshotConfig, ResolvableConfig,
        SavedQueryConfig, SeedConfig, SemanticModelConfig, SkillConfig, SnapshotConfig,
        SourceConfig, TypedRecursiveConfig, UnitTestConfig,
    },
    serde::yaml_to_fs_error,
};
use crate::state::ProfileAdapter;
use dbt_adapter_core::AdapterType;
use dbt_common::{
    FsResult,
    tracing::dbt_emit::{emit_strict_parse_error, emit_warn_log_message},
};
use dbt_yaml::ShouldBe;

/// Used to deserialize the top-level `dbt_project.yml` configuration
/// for `models`, `data_tests`, `seeds` etc..
///
/// ```yaml
/// models:
///   dbt_jinja(project_name):
///     adapter(folder_name in project):
///       +schema: 'dbt_jinja'
///       get_relation_cache:
///       +alias: 'dbt_jinja'
/// ```
///
/// This configuration is path based, meaning each key that is not a
/// property of it's configuration <T> is the name of a directory, which may have
/// source files or apply additional configuration. Configuration precedence
/// is given to the most specific path configuration. All unspecified
/// configuration is inherited from the parent.
///
#[derive(Debug, Clone)]
pub struct DbtProjectConfig<T: ResolvableConfig<T>> {
    /// The root configuration (i.e. at the `dbt_project.yml` level or inherited from `profiles.yml`)
    pub config: T,
    /// Child configuration applied by path part (preserves insertion order like Python dicts)
    pub children: IndexMap<String, DbtProjectConfig<T>>,
}

impl<T: ResolvableConfig<T>> DbtProjectConfig<T> {
    /// Create a new [GlobalProjectConfig] from a default configuration and the root dbt_project.yml [DbtProjectConfigs]
    pub fn try_new<S: Into<T> + TypedRecursiveConfig>(
        configs: Option<&S>,
        package_defaults: T::PackageDefaults,
        dependency_package_name: Option<&str>,
        disallow_plus_prefix: bool,
        default_adapter: AdapterType,
    ) -> FsResult<Self>
    where
        T: PartialEq,
    {
        let on_error = |variant: &ShouldBe<S>, key: &str, key_path: &str| {
            let key_path = if key_path.is_empty() {
                key.to_string()
            } else {
                format!("{key_path}.{key}")
            };
            match variant {
                ShouldBe::AndIs(_) => {
                    // If we're calling `on_error` on a valid key, it must start with `+`.
                    let detail = format!(
                        "Unrecognized key `{key_path}`. Custom keys must go under `+meta`."
                    );
                    let fs_err = FsError::new(
                        ErrorCode::SerializationError,
                        format!(
                            "Invalid {} definition `{}`: {}",
                            S::type_name(),
                            key_path,
                            detail
                        ),
                    );
                    emit_strict_parse_error(fs_err, dependency_package_name);
                }
                ShouldBe::ButIsnt(_) => {
                    let filename = if let Some(raw) = variant.as_ref_raw()
                        && let Some(filename) = raw.span().get_filename()
                    {
                        Some(filename)
                    } else {
                        None
                    };

                    // An unknown key produces the error message `expected struct <SelfType>` due to
                    // the recursive type. Catch the error here to inject a more descriptive error.
                    let err_msg = variant
                        .as_err_msg()
                        .expect("Error message always present on ShouldBe::ButIsnt variant");
                    let self_type = std::any::type_name::<S>()
                        .rsplit("::")
                        .next()
                        .unwrap_or_default();
                    // YAML reads an unquoted `on`/`off`/`yes`/`no` key as a boolean, which
                    // fails as a field identifier. The spelling is lost, so name the rule.
                    let bool_key = variant
                        .as_ref_raw()
                        .filter(|_| err_msg.contains("expected field identifier"))
                        .and_then(|raw| raw.as_mapping())
                        .and_then(|mapping| mapping.keys().find_map(|k| k.as_bool()));
                    let detail = if let Some(bool_key) = bool_key {
                        format!(
                            "A key under `{key_path}` reads as the boolean `{bool_key}`, because \
                             YAML treats unquoted `on`, `off`, `yes`, `no`, `true` and `false` as \
                             booleans. If it names a folder, quote it, e.g. `'off':`."
                        )
                    } else if err_msg.contains(&format!("expected struct {self_type}")) {
                        format!("Unrecognized key `{key_path}`. Custom keys must go under `+meta`.")
                    } else {
                        err_msg.to_string()
                    };

                    let err = variant
                        .take_err()
                        .expect("Error always present on ShouldBe::ButIsnt variant");
                    let fs_err = yaml_to_fs_error(err, filename).with_context(format!(
                        "Invalid {} definition `{}`: {}",
                        S::type_name(),
                        key_path,
                        detail
                    ));
                    emit_strict_parse_error(fs_err, dependency_package_name);
                }
            }
        };
        if !disallow_plus_prefix && let Some(configs) = configs {
            warn_plus_prefixed_resource_paths::<S>("", configs, "", false);
        }
        Ok(project_config_tree(
            configs,
            package_defaults,
            disallow_plus_prefix,
            default_adapter,
            &on_error,
        ))
    }

    /// Get the configuration for a fully qualified name (fqn)
    ///
    /// This method is recommended for nodes that don't derive from SQL files or where
    /// the node name doesn't match the filename. Examples include:
    /// - Exposures (defined in YAML files)
    /// - Unit tests (where the test name is separate from the model filename)
    /// - Sources (where the source and table names don't match file paths)
    /// - Any node where the fqn provides a more accurate representation than the file path
    ///
    /// The fqn should contain [package_name, path_component1, path_component2, ..., node_name]
    ///
    /// # Example
    /// ```rust
    /// use dbt_schemas::schemas::project::DbtProjectConfig;
    /// use dbt_schemas::schemas::project::ModelConfig;
    /// use indexmap::IndexMap;
    ///
    /// // Minimal config tree; in practice this is built from dbt_project.yml
    /// let config = DbtProjectConfig::<ModelConfig> {
    ///     config: ModelConfig::default(),
    ///     children: IndexMap::new(),
    /// };
    /// let fqn = vec!["analytics".to_string(), "weekly_revenue_report".to_string()];
    /// let _cfg = config.get_config_for_fqn(&fqn);
    /// ```
    pub fn get_config_for_fqn(&self, fqn: &[String]) -> &T {
        let mut current_config = self;

        // Traverse through all components in the fqn
        for component in fqn {
            if let Some(child) = current_config.children.get(component) {
                current_config = child;
            } else {
                break;
            }
        }

        &current_config.config
    }

    /// Set the configuration for the root [GlobalProjectConfig]
    pub fn with_config(&mut self, config: T) {
        self.config = config;
    }
}

/// Resolves the final config for a node by merging three layers in increasing order of precedence:
///
/// 1. **Local project config** — `dbt_project.yml` for this package, path-matched by FQN
/// 2. **Properties / inline config** — `schema.yml` or inline `{{ config(...) }}` values
/// 3. **Root overlay** — root project's `dbt_project.yml`, applied only for dependency packages
///
/// Merging uses `ResolvableConfig`: each higher-precedence layer fills in unset fields from the
/// layers below it. `enabled` intentionally has no default until `finalize()` so that the root
/// overlay can disable a dependency node regardless of what lower layers set.
///
/// For the root package, `root` is `None` and no overlay is applied.
///
/// Each node is merged over the trees, and with the alias table, of the adapter it picks; see
/// [`NodeLayers`]. Adapters the target does not declare share the default's.
#[derive(Clone)]
pub struct ProjectConfigResolver<T: ResolvableConfig<T>> {
    local: DbtProjectConfig<T>,
    root: Option<DbtProjectConfig<T>>,
    resolve_defaults: T::ResolveDefaults,
    default_adapter: AdapterType,
    adapter_override: Option<AdapterType>,
    /// `None` when no other adapter has trees of its own.
    other_adapters: Option<Arc<OtherAdapterTrees<T>>>,
}

/// A node's config layers, lowest precedence first, as the resolver merges them.
pub struct NodeLayers<'a, T> {
    /// The fqn the local tree is read at; see [`ProjectConfigResolver::resolve_with_configs`].
    pub original_fqn: &'a [String],
    /// The fqn the root overlay is read at.
    pub fqn: &'a [String],
    pub configs: &'a [Option<&'a T>],
    /// The adapter the node runs on when no layer sets one (a data test's attached node's).
    pub inherited_adapter: Option<AdapterType>,
}

impl<'a, T> NodeLayers<'a, T> {
    /// Layers read at one fqn, inheriting no adapter.
    pub fn new(fqn: &'a [String], configs: &'a [Option<&'a T>]) -> Self {
        NodeLayers {
            original_fqn: fqn,
            fqn,
            configs,
            inherited_adapter: None,
        }
    }
}

/// A node's config with the adapter it picked and the alias table it was read with.
#[derive(Debug, Clone)]
pub struct ResolvedNode<C> {
    pub config: C,
    /// `--adapter`, else the layers' `+adapter`, else the inherited one; `None` when nothing sets one.
    pub adapter: Option<AdapterType>,
    /// The adapter whose alias table read the config: `adapter` when it has trees of its own,
    /// else the default.
    pub alias_adapter: AdapterType,
}

/// Whose alias table an inline `config()` call renames its raw keys with.
#[derive(Debug, Clone)]
pub struct InlineConfigAliasing {
    /// The alias adapter of a pick the inline layer cannot change: `--adapter`, root overlay.
    fixed: Option<AdapterType>,
    /// The alias adapter of the pick before rendering.
    pre_render: AdapterType,
    /// The adapters with trees of their own; empty without per-adapter trees.
    selectable: Vec<AdapterType>,
    default: AdapterType,
}

impl InlineConfigAliasing {
    /// Every call renames with `adapter`'s alias table.
    pub fn fixed(adapter: AdapterType) -> Self {
        InlineConfigAliasing {
            fixed: Some(adapter),
            pre_render: adapter,
            selectable: Vec::new(),
            default: adapter,
        }
    }

    /// No fixed pick: a call's `adapter=` among `selectable` picks its table, else `default`'s.
    pub fn selectable(default: AdapterType, selectable: Vec<AdapterType>) -> Self {
        InlineConfigAliasing {
            fixed: None,
            pre_render: default,
            selectable,
            default,
        }
    }

    /// The aliases a raw rename leaves to `T`'s typed hook: only while the table can still change.
    pub fn deferred_alias_keys<T: ResolvableConfig<T>>(&self) -> &'static [&'static str] {
        if self.fixed.is_none() && !self.selectable.is_empty() {
            T::typed_alias_keys()
        } else {
            &[]
        }
    }

    /// The alias adapter for a call whose own `adapter=` kwarg is `call_adapter`.
    pub fn alias_adapter(&self, call_adapter: Option<AdapterType>) -> AdapterType {
        self.fixed.unwrap_or_else(|| match call_adapter {
            Some(adapter) if self.selectable.contains(&adapter) => adapter,
            Some(_) => self.default,
            None => self.pre_render,
        })
    }
}

/// The trees and alias table one node merges with.
struct NodeTrees<'a, T: ResolvableConfig<T>> {
    local: &'a DbtProjectConfig<T>,
    root: Option<&'a DbtProjectConfig<T>>,
    alias_adapter: AdapterType,
}

impl<T: ResolvableConfig<T>> NodeTrees<'_, T> {
    /// Merges the local tree at `fqn` with `configs`, without the root overlay.
    fn with_configs(&self, fqn: &[String], configs: &[Option<&T>]) -> T {
        let mut config = self.local.get_config_for_fqn(fqn).clone();
        for c in configs.iter().flatten() {
            let mut c = (*c).clone();
            c.canonicalize_adapter_aliases(self.alias_adapter);
            c.default_to(&config);
            config = c;
        }
        config
    }

    /// Applies the root project config overlay for dependency packages.
    fn apply_root_overlay(&self, config: &mut T, fqn: &[String]) {
        if let Some(root) = self.root {
            let mut root_config = root.get_config_for_fqn(fqn).clone();
            root_config.canonicalize_adapter_aliases(self.alias_adapter);
            root_config.default_to(config);
            *config = root_config;
        }
    }
}

impl<T: ResolvableConfig<T>> ProjectConfigResolver<T> {
    /// Use when the current package is the root project (no root overlay needed).
    pub fn for_root(config: DbtProjectConfig<T>, default_adapter: AdapterType) -> Self {
        ProjectConfigResolver {
            local: config,
            root: None,
            resolve_defaults: T::ResolveDefaults::default(),
            default_adapter,
            adapter_override: None,
            other_adapters: None,
        }
    }

    /// Use when the current package is a dependency.
    pub fn for_dependency(
        local: DbtProjectConfig<T>,
        root: DbtProjectConfig<T>,
        default_adapter: AdapterType,
    ) -> Self {
        ProjectConfigResolver {
            local,
            root: Some(root),
            resolve_defaults: T::ResolveDefaults::default(),
            default_adapter,
            adapter_override: None,
            other_adapters: None,
        }
    }

    /// Sets the resolve defaults, overriding the `Default` value.
    pub fn with_resolve_defaults(mut self, defaults: T::ResolveDefaults) -> Self {
        self.resolve_defaults = defaults;
        self
    }

    /// Sets the `--adapter` override, which every node picks over its layers' `+adapter`.
    pub fn with_adapter_override(mut self, adapter_override: Option<AdapterType>) -> Self {
        self.adapter_override = adapter_override;
        self
    }

    /// Builds a resolver from a root config. When `is_dependency` is true, `build_local` is
    /// called to construct the local package config; the closure is never called for root packages
    /// because the `root` argument itself serves as the local config (root packages have no
    /// separate overlay to apply).
    ///
    /// `default_adapter` is required (not an optional setter) so no resolver can be built without
    /// wiring the per-layer alias-canonicalization hook below.
    pub fn build<F>(
        root: DbtProjectConfig<T>,
        is_dependency: bool,
        build_local: F,
        default_adapter: AdapterType,
    ) -> FsResult<Self>
    where
        F: FnOnce() -> FsResult<DbtProjectConfig<T>>,
    {
        if is_dependency {
            Ok(Self::for_dependency(build_local()?, root, default_adapter))
        } else {
            Ok(Self::for_root(root, default_adapter))
        }
    }

    /// Like [`Self::build`], plus lazy trees per other declared adapter: `adapter_root` picks
    /// this type's root tree from them, and `local` is read only for a dependency package.
    #[allow(clippy::too_many_arguments)]
    pub fn per_adapter<S>(
        root: DbtProjectConfig<T>,
        adapter_root: fn(&AdapterRootConfigs) -> &DbtProjectConfig<T>,
        root_project_configs: &RootProjectConfigs,
        local: &Option<S>,
        dependency_package_name: Option<&str>,
        resolve_defaults: T::ResolveDefaults,
        default_adapter: AdapterType,
        adapter_override: Option<AdapterType>,
    ) -> FsResult<Self>
    where
        T: PartialEq + 'static,
        T::PackageDefaults: Default,
        S: TypedRecursiveConfig + Into<T> + Send + Sync + 'static,
    {
        let disallow_plus_prefix = root_project_configs.disallow_plus_prefix;
        let is_dependency = dependency_package_name.is_some();
        let mut resolver = Self::build(
            root,
            is_dependency,
            || {
                init_project_config(
                    local,
                    T::PackageDefaults::default(),
                    dependency_package_name,
                    disallow_plus_prefix,
                    default_adapter,
                )
            },
            default_adapter,
        )?
        .with_resolve_defaults(resolve_defaults)
        .with_adapter_override(adapter_override);
        resolver.other_adapters = root_project_configs
            .by_adapter
            .as_ref()
            .map(|root_configs| {
                Arc::new(OtherAdapterTrees {
                    trees: root_configs
                        .adapters()
                        .map(|adapter| (adapter, OnceLock::new()))
                        .collect(),
                    root_configs: root_configs.clone(),
                    root_tree: adapter_root,
                    local_tree: is_dependency.then(|| {
                        let local = local.clone();
                        Box::new(move |adapter| {
                            project_config_tree(
                                local.as_ref(),
                                T::PackageDefaults::default(),
                                disallow_plus_prefix,
                                adapter,
                                &|_, _, _| {},
                            )
                        }) as LocalTreeBuilder<T>
                    }),
                })
            });
        Ok(resolver)
    }

    /// The `+adapter` the layers merge to, read in the merge's precedence off the layers as they
    /// are: no alias table renames `adapter`. Clones and emits nothing.
    fn peek_adapter(&self, layers: &NodeLayers<'_, T>) -> Option<AdapterType> {
        self.root
            .as_ref()
            .and_then(|root| root.get_config_for_fqn(layers.fqn).get_adapter())
            .or_else(|| {
                layers
                    .configs
                    .iter()
                    .rev()
                    .flatten()
                    .find_map(|c| c.get_adapter())
            })
            .or_else(|| {
                self.local
                    .get_config_for_fqn(layers.original_fqn)
                    .get_adapter()
            })
    }

    /// The adapter a node runs on, known before its layers merge; `None` means the default.
    fn pick_adapter(&self, layers: &NodeLayers<'_, T>) -> Option<AdapterType> {
        self.adapter_override
            .or_else(|| self.peek_adapter(layers))
            .or(layers.inherited_adapter)
    }

    /// `adapter` when it has trees of its own, else the default; builds no tree.
    fn alias_adapter_of(&self, adapter: Option<AdapterType>) -> AdapterType {
        match (&self.other_adapters, adapter) {
            (Some(others), Some(adapter)) if others.trees.contains_key(&adapter) => adapter,
            _ => self.default_adapter,
        }
    }

    /// The trees `adapter` merges with: its own when it has them, else the default's.
    fn trees_for(&self, adapter: Option<AdapterType>) -> NodeTrees<'_, T> {
        if let Some(others) = &self.other_adapters
            && let Some((adapter, trees)) =
                adapter.and_then(|adapter| others.trees.get_key_value(&adapter))
        {
            let trees = trees.get_or_init(|| others.build(*adapter));
            return NodeTrees {
                local: &trees.local,
                root: trees.root.as_ref(),
                alias_adapter: *adapter,
            };
        }
        NodeTrees {
            local: &self.local,
            root: self.root.as_ref(),
            alias_adapter: self.default_adapter,
        }
    }

    /// Whether other declared adapters have trees of their own, so a node's alias table can
    /// differ from the default's.
    pub fn has_per_adapter_trees(&self) -> bool {
        self.other_adapters
            .as_ref()
            .is_some_and(|others| !others.trees.is_empty())
    }

    /// How the node's inline `config()` calls pick their alias table, given its other layers.
    pub fn inline_config_aliasing(&self, layers: &NodeLayers<'_, T>) -> InlineConfigAliasing {
        let root_adapter = self
            .root
            .as_ref()
            .and_then(|root| root.get_config_for_fqn(layers.fqn).get_adapter());
        InlineConfigAliasing {
            fixed: self
                .adapter_override
                .or(root_adapter)
                .map(|adapter| self.alias_adapter_of(Some(adapter))),
            pre_render: self.alias_adapter_of(self.pick_adapter(layers)),
            selectable: self
                .other_adapters
                .as_ref()
                .map(|others| others.trees.keys().copied().collect())
                .unwrap_or_default(),
            default: self.default_adapter,
        }
    }

    /// The node's inline `config()` calls as one layer: each read with the alias table the node
    /// picks with them, then folded in source order. `None` without calls.
    pub fn fold_inline_calls(&self, layers: &NodeLayers<'_, T>, calls: &[&T]) -> Option<T> {
        let configs: Vec<Option<&T>> = layers
            .configs
            .iter()
            .copied()
            .chain(calls.iter().copied().map(Some))
            .collect();
        let alias_adapter = self.alias_adapter_of(self.pick_adapter(&NodeLayers {
            configs: &configs,
            ..*layers
        }));
        let mut folded: Option<T> = None;
        for call in calls {
            let mut call = (*call).clone();
            call.canonicalize_adapter_aliases(alias_adapter);
            if let Some(prev) = &folded {
                call.default_to(prev);
            }
            folded = Some(call);
        }
        folded
    }

    /// Merges every layer, the root overlay and the resolve defaults, before `finalize`.
    fn merge_node(&self, layers: &NodeLayers<'_, T>) -> ResolvedNode<T> {
        let adapter = self.pick_adapter(layers);
        let trees = self.trees_for(adapter);
        let mut config = trees.with_configs(layers.original_fqn, layers.configs);
        trees.apply_root_overlay(&mut config, layers.fqn);
        debug_assert_eq!(config.get_adapter(), self.peek_adapter(layers));
        config.apply_resolve_defaults(self.resolve_defaults.clone());
        ResolvedNode {
            config,
            adapter,
            alias_adapter: trees.alias_adapter,
        }
    }

    /// Merges the local project config with `layers.configs`, without the root overlay or
    /// `finalize`: the Jinja render context before inline `{{ config(...) }}` calls are processed.
    pub fn with_node_configs(&self, layers: &NodeLayers<'_, T>) -> ResolvedNode<T> {
        let adapter = self.pick_adapter(layers);
        let trees = self.trees_for(adapter);
        ResolvedNode {
            config: trees.with_configs(layers.original_fqn, layers.configs),
            adapter,
            alias_adapter: trees.alias_adapter,
        }
    }

    /// Fully resolves a node by applying all layers and calling `finalize`.
    pub fn resolve_node(&self, layers: &NodeLayers<'_, T>) -> ResolvedNode<T::Resolved> {
        self.resolve_node_with_overrides(layers, |_| {})
    }

    /// Like [`Self::resolve_node`], applying `override_fn` just before `finalize`.
    pub fn resolve_node_with_overrides(
        &self,
        layers: &NodeLayers<'_, T>,
        override_fn: impl FnOnce(&mut T),
    ) -> ResolvedNode<T::Resolved> {
        let ResolvedNode {
            mut config,
            adapter,
            alias_adapter,
        } = self.merge_node(layers);
        override_fn(&mut config);
        ResolvedNode {
            config: config.finalize(),
            adapter,
            alias_adapter,
        }
    }

    /// Merges the local project config with additional `configs` layers without applying the root
    /// overlay or calling `finalize`. Use this when the intermediate result is needed as the
    /// Jinja render context before inline `{{ config(...) }}` calls are processed.
    pub fn with_configs(&self, fqn: &[String], configs: &[Option<&T>]) -> T {
        self.with_node_configs(&NodeLayers::new(fqn, configs))
            .config
    }

    /// Like `with_configs` but also applies the root project overlay. Use this when you need to
    /// validate explicitly-configured values (including root overlay) before
    /// `apply_resolve_defaults` fills in CLI-flag defaults.
    pub fn with_configs_and_root_overlay(&self, fqn: &[String], configs: &[Option<&T>]) -> T {
        let trees = self.trees_for(self.pick_adapter(&NodeLayers::new(fqn, configs)));
        let mut config = trees.with_configs(fqn, configs);
        trees.apply_root_overlay(&mut config, fqn);
        config
    }

    /// Fully resolves config by applying all layers and calling `finalize`.
    ///
    /// `original_fqn` is used for local project config lookup so that nodes whose paths are
    /// transformed by fusion (snapshots, generated tests) still resolve against their original
    /// directory hierarchy. `fqn` is used for root overlay lookup. Pass the same value for both
    /// when no path transformation occurs (models, seeds, etc.).
    pub fn resolve_with_configs(
        &self,
        original_fqn: &[String],
        fqn: &[String],
        configs: &[Option<&T>],
    ) -> T::Resolved {
        self.resolve_with_overrides(original_fqn, fqn, configs, |_| {})
    }

    /// Like `resolve_with_configs` but applies `override_fn` to the merged config after all layers
    /// (including the root overlay and resolve defaults) are applied, just before `finalize`.
    /// Use this when a caller needs to unconditionally force a field value regardless of what the
    /// user configured (e.g. forcing `enabled = false` on a render-error path).
    pub fn resolve_with_overrides(
        &self,
        original_fqn: &[String],
        fqn: &[String],
        configs: &[Option<&T>],
        override_fn: impl FnOnce(&mut T),
    ) -> T::Resolved {
        let layers = NodeLayers {
            original_fqn,
            fqn,
            configs,
            inherited_adapter: None,
        };
        self.resolve_node_with_overrides(&layers, override_fn)
            .config
    }

    /// Like `resolve_with_overrides` but `override_fn` may fail. Use this when the override
    /// logic itself can produce an error that must propagate to the caller.
    pub fn try_resolve_with_overrides<E>(
        &self,
        original_fqn: &[String],
        fqn: &[String],
        configs: &[Option<&T>],
        override_fn: impl FnOnce(&mut T) -> Result<(), E>,
    ) -> Result<T::Resolved, E> {
        let layers = NodeLayers {
            original_fqn,
            fqn,
            configs,
            inherited_adapter: None,
        };
        let mut config = self.merge_node(&layers).config;
        override_fn(&mut config)?;
        Ok(config.finalize())
    }

    /// Convenience wrapper: equivalent to `resolve_with_configs(fqn, fqn, &[properties_config])`.
    pub fn resolve_with_properties(
        &self,
        fqn: &[String],
        properties_config: Option<&T>,
    ) -> T::Resolved {
        self.resolve_with_configs(fqn, fqn, &[properties_config])
    }

    /// Returns true if the root overlay explicitly sets `enabled = false` for this FQN.
    /// When true, SQL rendering can be skipped entirely: the root overlay has the highest
    /// precedence for dependency packages, so no inline `{{ config(...) }}` call can re-enable
    /// the node.
    pub fn is_disabled_by_root_overlay(&self, fqn: &[String]) -> bool {
        self.root
            .as_ref()
            .map(|root| !root.get_config_for_fqn(fqn).get_enabled_with_default())
            .unwrap_or(false)
    }

    /// Returns true if the root overlay explicitly sets `enabled = true` for this FQN.
    /// The root overlay has the highest precedence for dependency packages, so an inline
    /// `{{ config(enabled=false) }}` must not short-circuit rendering in that case.
    pub fn is_enabled_by_root_overlay(&self, fqn: &[String]) -> bool {
        self.root
            .as_ref()
            .map(|root| root.get_config_for_fqn(fqn).get_enabled() == Some(true))
            .unwrap_or(false)
    }
}

type LocalTreeBuilder<T> = Box<dyn Fn(AdapterType) -> DbtProjectConfig<T> + Send + Sync>;

/// One other declared adapter's trees for a package.
struct AdapterTrees<T: ResolvableConfig<T>> {
    local: DbtProjectConfig<T>,
    root: Option<DbtProjectConfig<T>>,
}

/// The trees of each declared adapter other than the default, built when a node first picks it.
struct OtherAdapterTrees<T: ResolvableConfig<T>> {
    root_configs: Arc<RootConfigsByAdapter>,
    root_tree: fn(&AdapterRootConfigs) -> &DbtProjectConfig<T>,
    /// `None` for the root package, whose local tree is its root tree.
    local_tree: Option<LocalTreeBuilder<T>>,
    trees: HashMap<AdapterType, OnceLock<AdapterTrees<T>>>,
}

impl<T: ResolvableConfig<T>> OtherAdapterTrees<T> {
    fn build(&self, adapter: AdapterType) -> AdapterTrees<T> {
        let root_configs = self
            .root_configs
            .get(adapter)
            .expect("trees are keyed by the root configs' adapters");
        let root = (self.root_tree)(root_configs).clone();
        match &self.local_tree {
            Some(local_tree) => AdapterTrees {
                local: local_tree(adapter),
                root: Some(root),
            },
            None => AdapterTrees {
                local: root,
                root: None,
            },
        }
    }
}

/// Recursively build the [DbtProjectConfig] from a parent and child configuration.
///
/// The `on_error` closure is called for each `ShouldBe::ButIsnt` variant encountered
/// during traversal. Use this to emit parse errors or silently skip invalid children.
pub fn recur_build_dbt_project_config<T, S, F>(
    parent_config: &T,
    child: &S,
    key_path: &str,
    on_error: &F,
    disallow_plus_prefix: bool,
    default_adapter: AdapterType,
) -> DbtProjectConfig<T>
where
    T: ResolvableConfig<T> + PartialEq,
    S: Into<T> + TypedRecursiveConfig,
    F: Fn(&ShouldBe<S>, &str, &str),
{
    let mut child_config: T = child.clone().into();
    // Canonicalize this level's own config source before merging with its (already-canonical)
    // parent, per-layer. [dbt-core `credentials.translate_aliases`]
    child_config.canonicalize_adapter_aliases(default_adapter);
    child_config.default_to(parent_config);
    let mut children = IndexMap::new();

    // Handle additional properties generically - each child inherits from current config
    for (key, maybe_child_config_variant) in child.iter_children() {
        let child_key_path = if key_path.is_empty() {
            key.clone()
        } else {
            format!("{key_path}.{key}")
        };

        if key.starts_with("+") {
            if disallow_plus_prefix {
                on_error(maybe_child_config_variant, key, key_path);
                continue;
            }
        }

        let child_config_variant = match maybe_child_config_variant {
            ShouldBe::AndIs(config) => config,
            ShouldBe::ButIsnt(..) => {
                on_error(maybe_child_config_variant, key, key_path);
                continue;
            }
        };

        children.insert(
            key.clone(),
            recur_build_dbt_project_config(
                &child_config,
                child_config_variant,
                &child_key_path,
                on_error,
                disallow_plus_prefix,
                default_adapter,
            ),
        );
    }

    DbtProjectConfig {
        config: child_config,
        children,
    }
}

/// Emit one warning for each `+`-prefixed resource path in the config tree.
///
/// A `+`-prefixed key is a resource path whose name starts with `+` when it carries a
/// non-default config or has a `+`-prefixed child.
///
/// Returns whether `key` itself starts with `+`.
fn warn_plus_prefixed_resource_paths<S>(key: &str, value: &S, path: &str, inside_plus: bool) -> bool
where
    S: TypedRecursiveConfig,
{
    let key_is_plus = key.starts_with('+');

    let key_path = if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    };

    let warn = || {
        emit_warn_log_message(
            ErrorCode::InvalidConfig,
            format!(
                "Resource path `{key_path}` in dbt_project.yml starts with `+`. This will be deprecated in future versions of dbt."
            ),
        );
    };

    // Case 1: `value` is non-default, meaning there is a valid config inside it somewhere.
    // We definitely know a config value was set, because it must have this shape:
    //
    // ```
    // +key:
    //   +some_config: some_value
    // ```
    //
    // This is just an approximation and will miss cases where the config is explicitly set to
    // default.
    let valid_config_exists = value.has_set_fields();

    // We want to throw warnings at the location of the topmost +-prefixed key, so we throw exactly
    // one warning per offending resource path.
    let is_parent_plus = key_is_plus && !inside_plus;
    if is_parent_plus && valid_config_exists {
        warn();
        return key_is_plus;
    }

    // Case 2: Any non-config children start with +, or any descendants fall under case 1.
    let mut any_child_plus = false;
    for (child_key, child_variant) in value.iter_children() {
        let child_is_plus = match child_variant {
            ShouldBe::AndIs(child) => warn_plus_prefixed_resource_paths::<S>(
                child_key,
                child,
                &key_path,
                inside_plus || key_is_plus,
            ),
            ShouldBe::ButIsnt(..) => child_key.starts_with('+'),
        };
        any_child_plus |= child_is_plus;
    }
    if is_parent_plus && any_child_plus {
        warn();
    }

    key_is_plus || valid_config_exists || any_child_plus
}

/// Config wrapping propagated configs for the root project
#[derive(Debug)]
pub struct RootProjectConfigs {
    /// Model configs
    pub models: DbtProjectConfig<ModelConfig>,
    /// Source configs
    pub sources: DbtProjectConfig<SourceConfig>,
    /// Snapshot configs
    pub snapshots: DbtProjectConfig<SnapshotConfig>,
    /// Seed configs
    pub seeds: DbtProjectConfig<SeedConfig>,
    /// Test configs
    pub tests: DbtProjectConfig<DataTestConfig>,
    /// Unit test configs
    pub unit_tests: DbtProjectConfig<UnitTestConfig>,
    /// Exposure configs
    pub exposures: DbtProjectConfig<ExposureConfig>,
    /// Semantic model configs
    pub semantic_models: DbtProjectConfig<SemanticModelConfig>,
    /// Metric configs
    pub metrics: DbtProjectConfig<MetricConfig>,
    /// Saved query configs
    pub saved_queries: DbtProjectConfig<SavedQueryConfig>,
    /// Analysis configs
    pub analyses: DbtProjectConfig<AnalysesConfig>,
    /// Function configs
    pub functions: DbtProjectConfig<FunctionConfig>,
    /// Check configs
    pub checks: DbtProjectConfig<CheckConfig>,
    /// Skill configs
    pub skills: DbtProjectConfig<SkillConfig>,
    /// Authored identifier quoting per declared adapter name — the adapter's
    /// `adapters:` entry, plus the top-level `quoting:` block for the target's
    /// default adapter only. Left unresolved so node config still wins over it.
    ///
    /// Root-project config, hence its home here: `adapters:` is a root-only key,
    /// and the top-level `quoting:` block that layers under it is the root's for
    /// every package — a dependency's own `quoting:` block was already overridden
    /// by the root's via [`ProjectConfigResolver::apply_root_overlay`], and stays
    /// overridden.
    pub adapter_quoting: IndexMap<AdapterType, DbtQuoting>,
    /// The target's other declared adapters' trees; `None` for a single-adapter target or
    /// without multi-adapter support.
    by_adapter: Option<Arc<RootConfigsByAdapter>>,
    /// The root project's `require_resource_names_without_plus_prefix` flag.
    disallow_plus_prefix: bool,
}

/// The root project's trees for the resource types that can select an adapter, with aliases
/// translated for one non-default adapter.
#[derive(Debug)]
pub struct AdapterRootConfigs {
    /// Model configs
    pub models: DbtProjectConfig<ModelConfig>,
    /// Seed configs
    pub seeds: DbtProjectConfig<SeedConfig>,
    /// Snapshot configs
    pub snapshots: DbtProjectConfig<SnapshotConfig>,
    /// Test configs
    pub tests: DbtProjectConfig<DataTestConfig>,
    /// Function configs
    pub functions: DbtProjectConfig<FunctionConfig>,
}

/// [`AdapterRootConfigs`] per declared non-default adapter, built when a node first selects it,
/// which is when a Databricks `catalog`/`database` conflict in them is reported, once.
#[derive(Debug)]
struct RootConfigsByAdapter {
    models: Option<ProjectModelConfig>,
    seeds: Option<ProjectSeedConfig>,
    snapshots: Option<ProjectSnapshotConfig>,
    tests: Option<ProjectDataTestConfig>,
    functions: Option<ProjectFunctionConfig>,
    disallow_plus_prefix: bool,
    built: HashMap<AdapterType, OnceLock<AdapterRootConfigs>>,
}

impl RootConfigsByAdapter {
    fn new(
        root_project: &DbtProject,
        target_adapters: &IndexMap<AdapterType, ProfileAdapter>,
        default_adapter: AdapterType,
        disallow_plus_prefix: bool,
    ) -> Option<Self> {
        let built: HashMap<_, _> = target_adapters
            .keys()
            .copied()
            .filter(|adapter| *adapter != default_adapter)
            .map(|adapter| (adapter, OnceLock::new()))
            .collect();
        (!built.is_empty()).then(|| RootConfigsByAdapter {
            models: root_project.models.clone(),
            seeds: root_project.seeds.clone(),
            snapshots: root_project.snapshots.clone(),
            tests: root_project.data_tests_config().cloned(),
            functions: root_project.functions.clone(),
            disallow_plus_prefix,
            built,
        })
    }

    /// The declared non-default adapters.
    fn adapters(&self) -> impl Iterator<Item = AdapterType> + '_ {
        self.built.keys().copied()
    }

    /// `adapter`'s trees, or `None` for the default adapter or one the target does not declare.
    fn get(&self, adapter: AdapterType) -> Option<&AdapterRootConfigs> {
        let trees = self.built.get(&adapter)?;
        Some(trees.get_or_init(|| {
            let disallow_plus_prefix = self.disallow_plus_prefix;
            AdapterRootConfigs {
                models: project_config_tree(
                    self.models.as_ref(),
                    DbtQuoting::default(),
                    disallow_plus_prefix,
                    adapter,
                    &|_, _, _| {},
                ),
                seeds: project_config_tree(
                    self.seeds.as_ref(),
                    DbtQuoting::default(),
                    disallow_plus_prefix,
                    adapter,
                    &|_, _, _| {},
                ),
                snapshots: project_config_tree(
                    self.snapshots.as_ref(),
                    DbtQuoting::default(),
                    disallow_plus_prefix,
                    adapter,
                    &|_, _, _| {},
                ),
                tests: project_config_tree(
                    self.tests.as_ref(),
                    DbtQuoting::default(),
                    disallow_plus_prefix,
                    adapter,
                    &|_, _, _| {},
                ),
                functions: project_config_tree(
                    self.functions.as_ref(),
                    DbtQuoting::default(),
                    disallow_plus_prefix,
                    adapter,
                    &|_, _, _| {},
                ),
            }
        }))
    }
}

/// Read the `require_resource_names_without_plus_prefix` behavior flag from a
/// project's `flags` block. Absent or non-boolean values resolve to `false`.
///
/// `pub` rather than `pub(crate)` because this module moved down from
/// dbt-parser, whose modules still need it.
pub fn disallow_plus_prefix_from_flags(flags: Option<&dbt_yaml::Value>) -> bool {
    flags
        .and_then(|flags| {
            project_flags_get_value(flags, "require_resource_names_without_plus_prefix")
        })
        .and_then(dbt_yaml::Value::as_bool)
        .unwrap_or(false)
}

/// Build the [RootProjectConfigs] from a [DbtProject]
///
/// Every resource type that carries quoting — models, seeds, snapshots, data tests
/// and functions — is also one that can select an adapter with `+adapter`, so all
/// of them seed **nothing** for quoting, not even the authored top-level block. The
/// top-level block applies only to the target's *default* adapter, so it cannot be
/// folded in before the node's adapter is known; the matching `resolve_*` layers it
/// back on per node, out of `adapter_quoting`. Seeding all-`None` is also what keeps
/// the subtree and node-level `+quoting:` values distinguishable from the top-level
/// block after the field-wise merge.
///
/// Every other resource type has no quoting to seed at all.
/// The authored quoting each declared adapter contributes, keyed by adapter type.
///
/// Two layers, left **unresolved** (`None`s preserved) so that a node's own
/// `+quoting:` still wins over both:
///
/// 1. the adapter's entry in the root `dbt_project.yml` `adapters:` block;
/// 2. the top-level `quoting:` block — but **only for the target's default
///    adapter**. A node on a non-default adapter does not inherit the top-level
///    block; it takes its own entry and then falls through to its adapter type's
///    default. Configuring the default adapter is what the top-level block is for,
///    and letting it leak across adapters is what would otherwise force every
///    adapter in a target to agree on one policy.
///
/// Both inputs come from the **root** project, so this is computed once per run and
/// lives on `RootProjectConfigs`. A dependency package's own top-level `quoting:`
/// block does not enter the chain: it was already overridden by the root's via the
/// root-config overlay, and stays overridden.
///
/// `pub` and living here rather than in dbt-parser because its only caller,
/// `build_root_project_configs`, moved down into this crate; dbt-parser's tests
/// still exercise it through this path.
pub fn authored_quoting_per_adapter(
    adapters: Option<&IndexMap<AdapterType, AdapterProjectConfig>>,
    target_adapters: &IndexMap<AdapterType, ProfileAdapter>,
    default_adapter: AdapterType,
    top_level_quoting: Option<DbtQuoting>,
) -> IndexMap<AdapterType, DbtQuoting> {
    let top_level = top_level_quoting.unwrap_or_default();

    target_adapters
        .keys()
        .map(|adapter_type| {
            let own = adapters
                .and_then(|configured| configured.get(adapter_type))
                .and_then(|entry| entry.quoting)
                .unwrap_or_default();

            let layered = if *adapter_type == default_adapter {
                own.filled_from(&top_level)
            } else {
                own
            };
            (*adapter_type, layered)
        })
        .collect()
}

/// `multi_adapter` builds the other declared adapters' trees; without it every node reads the
/// default's.
pub fn build_root_project_configs(
    root_project: &DbtProject,
    target_adapters: &IndexMap<AdapterType, ProfileAdapter>,
    default_adapter: AdapterType,
    multi_adapter: bool,
) -> FsResult<RootProjectConfigs> {
    let maybe_root_project_config = root_project.data_tests_config().cloned();
    let disallow_plus_prefix = disallow_plus_prefix_from_flags(root_project.flags.as_ref());

    Ok(RootProjectConfigs {
        models: init_project_config(
            &root_project.models,
            DbtQuoting::default(),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        sources: init_project_config(
            &root_project.sources,
            (),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        snapshots: init_project_config(
            &root_project.snapshots,
            DbtQuoting::default(),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        seeds: init_project_config(
            &root_project.seeds,
            DbtQuoting::default(),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        tests: init_project_config(
            &maybe_root_project_config,
            DbtQuoting::default(),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        unit_tests: init_project_config(
            &root_project.unit_tests,
            (),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        exposures: init_project_config(
            &root_project.exposures,
            (),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        semantic_models: init_project_config(
            &root_project.semantic_models,
            (),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        metrics: init_project_config(
            &root_project.metrics,
            (),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        saved_queries: init_project_config(
            &root_project.saved_queries,
            (),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        analyses: init_project_config(
            &root_project.analyses,
            (),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        functions: init_project_config(
            &root_project.functions,
            DbtQuoting::default(),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        checks: init_project_config(
            &root_project.checks,
            (),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        skills: init_project_config(
            &root_project.skills,
            (),
            None,
            disallow_plus_prefix,
            default_adapter,
        )?,
        adapter_quoting: authored_quoting_per_adapter(
            root_project.adapters.as_ref(),
            target_adapters,
            default_adapter,
            *root_project.quoting,
        ),
        by_adapter: multi_adapter
            .then(|| {
                RootConfigsByAdapter::new(
                    root_project,
                    target_adapters,
                    default_adapter,
                    disallow_plus_prefix,
                )
            })
            .flatten()
            .map(Arc::new),
        disallow_plus_prefix,
    })
}

/// generate the project config that will be inherited throughout the project
pub fn init_project_config<
    T: ResolvableConfig<T> + PartialEq,
    S: TypedRecursiveConfig + Into<T>,
>(
    dbt_project_configs: &Option<S>,
    package_defaults: T::PackageDefaults,
    dependency_package_name: Option<&str>,
    disallow_plus_prefix: bool,
    default_adapter: AdapterType,
) -> FsResult<DbtProjectConfig<T>> {
    DbtProjectConfig::try_new(
        dbt_project_configs.as_ref(),
        package_defaults,
        dependency_package_name,
        disallow_plus_prefix,
        default_adapter,
    )
}

/// Builds a project tree for `adapter`, passing each invalid child to `on_error`.
fn project_config_tree<T, S, F>(
    configs: Option<&S>,
    package_defaults: T::PackageDefaults,
    disallow_plus_prefix: bool,
    adapter: AdapterType,
    on_error: &F,
) -> DbtProjectConfig<T>
where
    T: ResolvableConfig<T> + PartialEq,
    S: TypedRecursiveConfig + Into<T>,
    F: Fn(&ShouldBe<S>, &str, &str),
{
    let mut default_config = T::default();
    default_config.apply_package_defaults(package_defaults);
    match configs {
        Some(configs) => recur_build_dbt_project_config(
            &default_config,
            configs,
            "",
            on_error,
            disallow_plus_prefix,
            adapter,
        ),
        None => DbtProjectConfig {
            config: default_config,
            children: IndexMap::new(),
        },
    }
}

/// Strip resource paths from the beginning of a reference path
/// This function tries to find which resource path is a prefix of the ref_path
/// and returns the path with that prefix stripped
pub fn strip_resource_paths_from_ref_path(ref_path: &Path, resource_paths: &[String]) -> PathBuf {
    // Try to find a resource path that is a prefix of the ref_path
    for resource_path in resource_paths {
        let resource_pathbuf = PathBuf::from(resource_path);

        // Use Path::starts_with which properly handles path components
        if ref_path.starts_with(&resource_pathbuf) {
            // Use Path::strip_prefix which is designed for this exact purpose
            if let Ok(stripped) = ref_path.strip_prefix(&resource_pathbuf) {
                // Only return the stripped path if it's not empty
                // (i.e., ref_path was not exactly equal to resource_path)
                if stripped.as_os_str().is_empty() {
                    return ref_path.to_path_buf();
                } else {
                    return stripped.to_path_buf();
                }
            }
        }
    }

    // If no resource path matches, return the original path
    ref_path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_resource_paths_single_level() {
        let ref_path = Path::new("models/my_model.sql");
        let resource_paths = vec!["models".to_string()];
        let result = strip_resource_paths_from_ref_path(ref_path, &resource_paths);
        assert_eq!(result, PathBuf::from("my_model.sql"));
    }

    #[test]
    fn test_strip_resource_paths_nested_structure() {
        let ref_path = Path::new("dbt/models/example/my_first_model.sql");
        let resource_paths = vec!["dbt/models".to_string()];
        let result = strip_resource_paths_from_ref_path(ref_path, &resource_paths);
        assert_eq!(result, PathBuf::from("example/my_first_model.sql"));
    }

    #[test]
    fn test_strip_resource_paths_deep_nesting() {
        let ref_path = Path::new("warehouse/staging/models/marts/finance/revenue.sql");
        let resource_paths = vec!["warehouse/staging/models".to_string()];
        let result = strip_resource_paths_from_ref_path(ref_path, &resource_paths);
        assert_eq!(result, PathBuf::from("marts/finance/revenue.sql"));
    }

    #[test]
    fn test_strip_resource_paths_multiple_paths() {
        let ref_path = Path::new("src/models/staging/customers.sql");
        let resource_paths = vec![
            "models".to_string(),
            "src/models".to_string(),
            "dbt/models".to_string(),
        ];
        let result = strip_resource_paths_from_ref_path(ref_path, &resource_paths);
        assert_eq!(result, PathBuf::from("staging/customers.sql"));
    }

    #[test]
    fn test_strip_resource_paths_no_match() {
        let ref_path = Path::new("analysis/my_analysis.sql");
        let resource_paths = vec!["models".to_string(), "seeds".to_string()];
        let result = strip_resource_paths_from_ref_path(ref_path, &resource_paths);
        assert_eq!(result, PathBuf::from("analysis/my_analysis.sql"));
    }

    #[test]
    fn test_strip_resource_paths_empty_resource_paths() {
        let ref_path = Path::new("models/example/my_model.sql");
        let resource_paths: Vec<String> = vec![];
        let result = strip_resource_paths_from_ref_path(ref_path, &resource_paths);
        assert_eq!(result, PathBuf::from("models/example/my_model.sql"));
    }

    #[test]
    fn test_strip_resource_paths_exact_match() {
        let ref_path = Path::new("models");
        let resource_paths = vec!["models".to_string()];
        let result = strip_resource_paths_from_ref_path(ref_path, &resource_paths);
        // Should return original path since stripping would result in empty string
        assert_eq!(result, PathBuf::from("models"));
    }

    #[test]
    fn test_strip_resource_paths_first_match_wins() {
        // Test that the function uses the first matching path in the array
        let ref_path = Path::new("models/staging/customers.sql");
        let resource_paths = vec![
            "models".to_string(),         // This should match first
            "models/staging".to_string(), // This is more specific but comes later
        ];
        let result = strip_resource_paths_from_ref_path(ref_path, &resource_paths);
        // Should strip "models" (first match), not "models/staging"
        assert_eq!(result, PathBuf::from("staging/customers.sql"));
    }

    #[test]
    fn test_resource_path_edge_cases() {
        // Test various edge cases that could occur in real projects

        // Case 1: Resource path with trailing slash
        let result1 = strip_resource_paths_from_ref_path(
            Path::new("models/my_model.sql"),
            &["models/".to_string()],
        );
        assert_eq!(result1, PathBuf::from("my_model.sql"));

        // Case 2: Very deep nesting
        let result2 = strip_resource_paths_from_ref_path(
            Path::new("data/warehouse/dbt/models/marts/finance/reporting/revenue_monthly.sql"),
            &["data/warehouse/dbt/models".to_string()],
        );
        assert_eq!(
            result2,
            PathBuf::from("marts/finance/reporting/revenue_monthly.sql")
        );

        // Case 3: Path that has similar prefix but different directory
        // This should NOT be stripped because "models_backup" is not the "models" directory
        let result3 = strip_resource_paths_from_ref_path(
            Path::new("models_backup/my_model.sql"),
            &["models".to_string()],
        );
        // Fixed behavior: no stripping since "models_backup" != "models" directory
        assert_eq!(result3, PathBuf::from("models_backup/my_model.sql"));
    }

    #[test]
    fn test_path_component_boundary_matching() {
        // Test that we correctly distinguish between path components vs string prefixes

        // Should strip: exact directory match
        let result1 = strip_resource_paths_from_ref_path(
            Path::new("models/staging/customers.sql"),
            &["models".to_string()],
        );
        assert_eq!(result1, PathBuf::from("staging/customers.sql"));

        // Should NOT strip: different directory with similar name
        let result2 = strip_resource_paths_from_ref_path(
            Path::new("models_v2/customers.sql"),
            &["models".to_string()],
        );
        assert_eq!(result2, PathBuf::from("models_v2/customers.sql"));

        // Should NOT strip: file that starts with resource path name
        let result3 =
            strip_resource_paths_from_ref_path(Path::new("models.sql"), &["models".to_string()]);
        assert_eq!(result3, PathBuf::from("models.sql"));

        // Should strip: nested path with exact component match
        let result4 = strip_resource_paths_from_ref_path(
            Path::new("src/models/staging/customers.sql"),
            &["src/models".to_string()],
        );
        assert_eq!(result4, PathBuf::from("staging/customers.sql"));

        // Should NOT strip: similar but different nested path
        let result5 = strip_resource_paths_from_ref_path(
            Path::new("src/models_new/customers.sql"),
            &["src/models".to_string()],
        );
        assert_eq!(result5, PathBuf::from("src/models_new/customers.sql"));
    }

    #[test]
    fn test_get_config_for_fqn_basic() {
        let mut config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        config.config.enabled = Some(true);

        // Add a child config for project "test_project"
        let mut project_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        project_config.config.enabled = Some(false);
        config
            .children
            .insert("test_project".to_string(), project_config);

        let fqn = vec!["test_project".to_string()];
        let result = config.get_config_for_fqn(&fqn);

        assert_eq!(result.enabled, Some(false));
    }

    #[test]
    fn test_get_config_for_fqn_nested() {
        let mut config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        config.config.enabled = Some(true);

        // Add project config
        let mut project_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        project_config.config.enabled = Some(false);

        // Add staging subdirectory config
        let mut staging_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        staging_config.config.enabled = Some(true);
        staging_config.config.materialized =
            Some(crate::schemas::common::DbtMaterialization::Table);

        project_config
            .children
            .insert("staging".to_string(), staging_config);
        config
            .children
            .insert("test_project".to_string(), project_config);

        let fqn = vec!["test_project".to_string(), "staging".to_string()];
        let result = config.get_config_for_fqn(&fqn);

        assert_eq!(result.enabled, Some(true));
        assert_eq!(
            result.materialized,
            Some(crate::schemas::common::DbtMaterialization::Table)
        );
    }

    #[test]
    fn test_get_config_for_fqn_node_specific() {
        let mut config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        config.config.enabled = Some(true);

        // Add project config
        let mut project_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        project_config.config.enabled = Some(false);

        // Add staging subdirectory config
        let mut staging_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        staging_config.config.enabled = Some(true);

        // Add node-specific config
        let mut node_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        node_config.config.enabled = Some(false);
        node_config.config.materialized =
            Some(crate::schemas::common::DbtMaterialization::Incremental);

        staging_config
            .children
            .insert("stg_customers".to_string(), node_config);
        project_config
            .children
            .insert("staging".to_string(), staging_config);
        config
            .children
            .insert("test_project".to_string(), project_config);

        let fqn = vec![
            "test_project".to_string(),
            "staging".to_string(),
            "stg_customers".to_string(),
        ];
        let result = config.get_config_for_fqn(&fqn);

        assert_eq!(result.enabled, Some(false));
        assert_eq!(
            result.materialized,
            Some(crate::schemas::common::DbtMaterialization::Incremental)
        );
    }

    #[test]
    fn test_get_config_for_fqn_partial_match() {
        let mut config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        config.config.enabled = Some(true);

        // Add project config
        let mut project_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        project_config.config.enabled = Some(false);

        // Add staging subdirectory config - only staging exists, not finance
        let mut staging_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        staging_config.config.enabled = Some(false);
        staging_config.config.materialized = Some(crate::schemas::common::DbtMaterialization::View);

        project_config
            .children
            .insert("staging".to_string(), staging_config);
        config
            .children
            .insert("test_project".to_string(), project_config);

        // FQN has staging/finance but only staging config exists
        let fqn = vec![
            "test_project".to_string(),
            "staging".to_string(),
            "finance".to_string(),
            "customers".to_string(),
        ];
        let result = config.get_config_for_fqn(&fqn);

        // Should get staging config since finance doesn't exist
        assert_eq!(result.enabled, Some(false));
        assert_eq!(
            result.materialized,
            Some(crate::schemas::common::DbtMaterialization::View)
        );
    }

    #[test]
    fn test_get_config_for_fqn_nonexistent_project() {
        let mut config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        config.config.enabled = Some(true);
        config.config.materialized = Some(crate::schemas::common::DbtMaterialization::Table);

        let fqn = vec![
            "nonexistent_project".to_string(),
            "staging".to_string(),
            "customers".to_string(),
        ];
        let result = config.get_config_for_fqn(&fqn);

        // Should return root config
        assert_eq!(result.enabled, Some(true));
        assert_eq!(
            result.materialized,
            Some(crate::schemas::common::DbtMaterialization::Table)
        );
    }

    #[test]
    fn test_get_config_for_fqn_empty() {
        let mut config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        config.config.enabled = Some(true);
        config.config.materialized = Some(crate::schemas::common::DbtMaterialization::View);

        let fqn: Vec<String> = vec![];
        let result = config.get_config_for_fqn(&fqn);

        // Should return root config
        assert_eq!(result.enabled, Some(true));
        assert_eq!(
            result.materialized,
            Some(crate::schemas::common::DbtMaterialization::View)
        );
    }

    #[test]
    fn test_get_config_for_fqn_complex_hierarchy() {
        // Test a complex hierarchy that might occur with non-file-based nodes
        let mut config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        config.config.enabled = Some(true);

        // Set up: my_project -> marts -> finance -> revenue_reports -> monthly_revenue
        let mut project_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        project_config.config.enabled = Some(true);

        let mut marts_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        marts_config.config.materialized = Some(crate::schemas::common::DbtMaterialization::Table);

        let mut finance_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        finance_config.config.enabled = Some(false);

        let mut revenue_reports_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        revenue_reports_config.config.materialized =
            Some(crate::schemas::common::DbtMaterialization::View);

        let mut monthly_revenue_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        monthly_revenue_config.config.enabled = Some(true);
        monthly_revenue_config.config.materialized =
            Some(crate::schemas::common::DbtMaterialization::Incremental);

        revenue_reports_config
            .children
            .insert("monthly_revenue".to_string(), monthly_revenue_config);
        finance_config
            .children
            .insert("revenue_reports".to_string(), revenue_reports_config);
        marts_config
            .children
            .insert("finance".to_string(), finance_config);
        project_config
            .children
            .insert("marts".to_string(), marts_config);
        config
            .children
            .insert("my_project".to_string(), project_config);

        let fqn = vec![
            "my_project".to_string(),
            "marts".to_string(),
            "finance".to_string(),
            "revenue_reports".to_string(),
            "monthly_revenue".to_string(),
        ];
        let result = config.get_config_for_fqn(&fqn);

        // Should get the most specific config (node-level)
        assert_eq!(result.enabled, Some(true));
        assert_eq!(
            result.materialized,
            Some(crate::schemas::common::DbtMaterialization::Incremental)
        );
    }

    #[test]
    fn test_get_config_for_fqn_deep_nested_path() {
        // Test equivalent to get_config_for_path_empty_resource_paths
        // This tests traversing a full deep path hierarchy
        let mut config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        config.config.enabled = Some(true);

        // Add project config with nested subdirectory structure
        let mut project_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        let mut models_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        let mut example_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        example_config.config.materialized =
            Some(crate::schemas::common::DbtMaterialization::Table);

        models_config
            .children
            .insert("example".to_string(), example_config);
        project_config
            .children
            .insert("models".to_string(), models_config);
        config
            .children
            .insert("test_project".to_string(), project_config);

        // FQN represents the full hierarchy: test_project -> models -> example -> my_model
        let fqn = vec![
            "test_project".to_string(),
            "models".to_string(),
            "example".to_string(),
            "my_model".to_string(),
        ];
        let result = config.get_config_for_fqn(&fqn);

        // Should traverse the full path and get the example config
        // (since my_model doesn't exist, it stops at example)
        assert_eq!(
            result.materialized,
            Some(crate::schemas::common::DbtMaterialization::Table)
        );
    }

    #[test]
    fn test_get_config_for_fqn_integration_realistic_dbt_structure() {
        // Integration test equivalent to test_integration_real_dbt_project_structure
        // Test a realistic DBT project scenario end-to-end with FQN
        let mut config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        config.config.enabled = Some(true);
        config.config.materialized = Some(crate::schemas::common::DbtMaterialization::View);

        // Set up project structure like: my_project -> staging -> +materialized: table
        let mut project_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        project_config.config.enabled = Some(true);

        let mut staging_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        staging_config.config.materialized =
            Some(crate::schemas::common::DbtMaterialization::Table);
        staging_config.config.enabled = Some(true);

        // Add specific model config
        let mut customers_config = DbtProjectConfig {
            config: ModelConfig::default(),
            children: IndexMap::new(),
        };
        customers_config.config.materialized =
            Some(crate::schemas::common::DbtMaterialization::Incremental);
        customers_config.config.enabled = Some(false);

        staging_config
            .children
            .insert("stg_customers".to_string(), customers_config);
        project_config
            .children
            .insert("staging".to_string(), staging_config);
        config
            .children
            .insert("my_project".to_string(), project_config);

        // FQN: my_project -> staging -> stg_customers
        // This represents the logical hierarchy similar to path:
        // warehouse/dbt/models/staging/stg_customers.sql with resource_paths stripped
        let fqn = vec![
            "my_project".to_string(),
            "staging".to_string(),
            "stg_customers".to_string(),
        ];
        let result = config.get_config_for_fqn(&fqn);

        // Should get the most specific config (file-level)
        assert_eq!(result.enabled, Some(false));
        assert_eq!(
            result.materialized,
            Some(crate::schemas::common::DbtMaterialization::Incremental)
        );
    }
}
