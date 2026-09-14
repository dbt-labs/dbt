use chrono_tz::Tz;
use dbt_adapter_core::AdapterType;
use dbt_yaml::Spanned;
use indexmap::IndexMap;
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
    time::SystemTime,
};

use crate::schemas::{
    DbtSource, InternalDbtNode, InternalDbtNodeAttributes, Nodes, ResolvedCloudConfig,
    common::{DbtQuoting, ResolvedQuoting},
    dbt_catalogs::DbtCatalogs,
    macros::{DbtDocsMacro, DbtMacro},
    manifest::{DbtOperation, DbtSelector},
    profiles::DbConfig,
    project::{
        DbtProject, ProjectDataTestConfig, ProjectModelConfig, ProjectSeedConfig,
        ProjectSnapshotConfig, ProjectSourceConfig, QueryComment,
    },
    relations::base::{BaseRelation, RelationPattern},
    selectors::ResolvedSelector,
    serde::{FloatOrString, SpannedStringOrArrayOfStrings, StringOrArrayOfStrings},
};
use blake3::Hasher;
use chrono::{DateTime, Local, Utc};
use dbt_common::{
    ErrorCode, FsResult, fs_err,
    io_args::{FsCommand, resolve_latest_version_pointer_enabled_by_default},
    path::DbtPath,
    warn_error_options::WarnErrorOptions,
};
use minijinja::{MacroSpans, Value as MinijinjaValue, value::Object};
use serde::Deserialize;
use serde::Serialize;
use std::fmt;

#[derive(Debug, Hash, Eq, PartialEq, Clone, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePathKind {
    ProfilePaths,
    ModelPaths,
    AnalysisPaths,
    AssetPaths,
    DocsPaths,
    MacroPaths,
    SeedPaths,
    SnapshotPaths,
    TestPaths,
    FixturePaths,
    SessionPaths,
    FunctionPaths,
    CheckPaths,
}

impl fmt::Display for ResourcePathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind_str = match self {
            ResourcePathKind::ModelPaths => "model paths",
            ResourcePathKind::AnalysisPaths => "analysis paths",
            ResourcePathKind::AssetPaths => "asset paths",
            ResourcePathKind::DocsPaths => "docs paths",
            ResourcePathKind::MacroPaths => "macro paths",
            ResourcePathKind::SeedPaths => "seed paths",
            ResourcePathKind::SnapshotPaths => "snapshot paths",
            ResourcePathKind::TestPaths => "test paths",
            ResourcePathKind::ProfilePaths => "profile paths",
            ResourcePathKind::FixturePaths => "fixture paths",
            ResourcePathKind::SessionPaths => "session paths",
            ResourcePathKind::FunctionPaths => "function paths",
            ResourcePathKind::CheckPaths => "check paths",
        };
        write!(f, "{kind_str}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DbtAsset {
    // in_dir (or project_dir), if the asset is input,
    // out_dir (or target_dir), if asset is an output
    pub base_path: PathBuf,
    // original location that generated this asset; used to generate
    // the fqn based on original location to preserve config hierarchy
    pub original_path: PathBuf,
    // relative path to project root
    pub path: PathBuf,
    // package name
    pub package_name: String,
}

impl DbtAsset {
    pub fn is_python(&self) -> bool {
        self.path.extension().and_then(|ext| ext.to_str()) == Some("py")
    }

    pub fn is_javascript(&self) -> bool {
        self.path.extension().and_then(|ext| ext.to_str()) == Some("js")
    }

    /// Assumes all paths used are canonicalized
    pub fn to_display_path(&self, project_root: &Path) -> PathBuf {
        let absolute_path = self.base_path.join(&self.path);
        if project_root == self.base_path {
            self.path.clone()
        } else {
            absolute_path
                .strip_prefix(project_root)
                .unwrap_or(&absolute_path)
                .to_owned()
        }
    }
}

impl fmt::Display for DbtAsset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DbtAsset {{ base_path: {}, path: {}, package_name: {} }}",
            self.base_path.display(),
            self.path.display(),
            self.package_name
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GenericTestAsset {
    pub dbt_asset: DbtAsset,
    pub resource_name: String,
    pub resource_type: String,
    /// Set only when `resource_type == "source"`. Carries the source-collection name
    /// (e.g. `salesforce` for `source('salesforce', 'accounts')`) so downstream code
    /// can build the `sources.<source_name>` file_key_name that dbt-core emits.
    pub source_name: Option<String>,
    pub test_name: String,
    pub defined_at: dbt_common::CodeLocationWithFile,
    /// The column this test is attached to, taken from its position in the schema file
    /// (`columns[*].data_tests`). `None` for a model-level test, regardless of any
    /// `column_name` kwarg the test passes to its macro.
    pub column_name: Option<String>,
    // Structured metadata for generic tests (optional; not used for singular tests)
    pub test_metadata_name: Option<String>,
    pub test_metadata_namespace: Option<String>,
    /// The `column_name` macro kwarg, which a generic test may set to an arbitrary
    /// sequence. Distinct from [`GenericTestAsset::column_name`].
    pub test_metadata_column_name: Option<StringOrArrayOfStrings>,
    pub test_metadata_combination_of_columns: Option<Vec<String>>,
    /// The model kwarg for generic tests, e.g. "{{ get_where_subquery(ref('foo')) }}"
    pub test_metadata_model: Option<String>,
    /// Full kwargs map for test_metadata, including all user-provided macro arguments.
    /// Excludes dbt config keys ("config", "_config_raw"). Empty for singular tests.
    pub test_metadata_kwargs: BTreeMap<String, dbt_yaml::Value>,
    /// YAML-supplied config keys to append to generic test raw_code.
    pub raw_code_config: BTreeMap<String, dbt_yaml::Value>,
    /// The original (untruncated) test name, if truncation occurred.
    /// When test names exceed 63 characters, dbt truncates to `<first 30 chars>_<md5 hash>`.
    /// This field stores the original name for selector matching purposes.
    pub original_name: Option<String>,
    /// Pre-computed unique_id hash suffix (last 10 hex chars of md5(fqn_name + metadata_repr)).
    /// Computed in persist_generic_data_tests using full kwargs, matching dbt-core/Mantle's algorithm.
    pub unique_id_hash: Option<String>,
    /// Version of the attached model for tests on versioned models (e.g. "1"). None for
    /// unversioned models and non-model resources. Used to build `attached_node` with the
    /// correct `.v<version>` suffix, matching dbt-core's `RefableLookup.get_unique_id`.
    pub version: Option<String>,
    /// Tags from `columns[*].config.tags` in schema YAML, carried for generic column tests.
    /// Empty for model-level and singular tests.
    pub column_tags: Vec<String>,
    /// Raw (unrendered) schema.yml `config:` block authored on this generic test, captured
    /// from the raw YAML before Jinja render. Fed as the `schema` arg to
    /// `build_unrendered_config` so generic tests carry their schema.yml config (`where`,
    /// `severity`, `limit`, schema.yml `tags`/`meta`, …) in `unrendered_config`, matching
    /// models/seeds/snapshots. Empty when the test has no schema.yml config.
    pub unrendered_schema_config: BTreeMap<String, dbt_yaml::Value>,
    /// User-authored `description:` on the generic test in schema YAML. `None` for the
    /// bare `- unique` form and for singular tests. Carried onto the test node's
    /// `description` so it surfaces in the manifest, matching dbt-core.
    pub description: Option<String>,
}

impl fmt::Display for GenericTestAsset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GenericTestAsset {{ dbt_asset: {}, resource_name: {}, resource_type: {}, source_name: {:?}, test_name: {}, test_metadata_name: {:?} }}",
            self.dbt_asset,
            self.resource_name,
            self.resource_type,
            self.source_name,
            self.test_name,
            self.test_metadata_name
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbtProfile {
    pub profile: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_to_target: Option<String>,
    /// The active target's `allow_clones` setting, read directly from its
    /// `outputs.<target>` block in profiles.yml. Note that if omitted defaults to `true`
    pub allow_clones: bool,
    /// Every adapter the active target declares, keyed by adapter type, in
    /// declaration order. A target using the legacy single-adapter
    /// `profiles.yml` shape has exactly one entry.
    pub adapters: IndexMap<AdapterType, ProfileAdapter>,
    /// The adapter used by nodes that do not select one. Always a key of
    /// [`Self::adapters`].
    pub default_adapter: AdapterType,
    pub schema: String,
    pub database: String,
    pub relative_profile_path: PathBuf,
    #[serde(skip)]
    pub threads: Option<usize>, // from flags in dbt
}

/// One credential set under an adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConnection {
    /// As written in `profiles.yml`, or `default` when omitted. Diagnostics only
    /// — nothing routes by connection name yet.
    pub name: String,
    pub config: DbConfig,
}

/// One adapter a target declares, and its connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileAdapter {
    /// Declaration order preserved. Never empty.
    pub connections: Vec<ProfileConnection>,
    /// Index into [`Self::connections`] of the one that is actually used.
    ///
    /// Several connections may be declared but only this one is reachable; profile
    /// load warns about the rest.
    pub default_connection: usize,
}

impl ProfileAdapter {
    /// A single unnamed connection — the legacy shape, and what synthetic
    /// profiles and tests want.
    pub fn single(config: DbConfig) -> Self {
        ProfileAdapter {
            connections: vec![ProfileConnection {
                name: "default".to_string(),
                config,
            }],
            default_connection: 0,
        }
    }

    /// The connection that is actually used.
    pub fn config(&self) -> &DbConfig {
        &self
            .connections
            .get(self.default_connection)
            .expect("ProfileAdapter::default_connection is always a valid index")
            .config
    }

    /// Mutable access to the connection that is actually used.
    pub fn config_mut(&mut self) -> &mut DbConfig {
        &mut self
            .connections
            .get_mut(self.default_connection)
            .expect("ProfileAdapter::default_connection is always a valid index")
            .config
    }

    /// Whether this adapter declares connections that cannot be reached yet.
    pub fn has_unreachable_connections(&self) -> bool {
        self.connections.len() > 1
    }

    fn is_valid(&self) -> bool {
        self.default_connection < self.connections.len()
    }
}

impl DbtProfile {
    /// The connection of the target's default adapter — the one used by nodes
    /// that do not select an adapter.
    ///
    /// Most callers want exactly this. Only code that must honour a node's
    /// adapter selection should reach for [`Self::adapter`].
    pub fn default_db_config(&self) -> &DbConfig {
        self.adapters
            .get(&self.default_adapter)
            .expect("DbtProfile::default_adapter is always a key of `adapters`")
            .config()
    }

    /// Mutable access to every connection of every adapter, for settings that
    /// apply target-wide (thread count, for instance).
    pub fn adapters_mut(&mut self) -> impl Iterator<Item = &mut DbConfig> {
        self.adapters
            .values_mut()
            .flat_map(|adapter| adapter.connections.iter_mut().map(|c| &mut c.config))
    }

    /// The adapter types the target declares, in declaration order.
    ///
    /// This is the set whose internal macro packages a run needs.
    pub fn adapter_types(&self) -> Vec<AdapterType> {
        self.adapters.keys().copied().collect()
    }

    /// The adapter a node's `+adapter` config selects, if the target declares it.
    pub fn adapter(&self, adapter_type: AdapterType) -> Option<&DbConfig> {
        self.adapters.get(&adapter_type).map(ProfileAdapter::config)
    }

    /// Whether the adapter map upholds the invariants [`Self::default_db_config`] relies
    /// on. Checked after deserializing a persisted profile, where a stale or
    /// hand-edited artifact could otherwise violate them.
    pub fn has_valid_adapters(&self) -> bool {
        self.adapters.contains_key(&self.default_adapter)
            && self.adapters.values().all(ProfileAdapter::is_valid)
    }

    pub fn blake3_hash(&self) -> String {
        let mut hasher = Hasher::new();
        // Serialize self, skipping threads due to #[serde(skip)]
        let bytes = serde_json::to_vec(self).expect("Serialization failed");
        hasher.update(&bytes);
        let hash = hasher.finalize();
        // Truncate to 16 bytes and encode as hex
        hex::encode(&hash.as_bytes()[..16])
    }
}
impl fmt::Display for DbtProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DbtProfile {{ profile: {}, target: {}, adapters: {:?}, default_adapter: {}, schema: {}, database: {} , path: {}, threads: {:?}}}",
            self.profile,
            self.target,
            self.adapters,
            self.default_adapter,
            self.schema,
            self.database,
            self.relative_profile_path.display(),
            self.threads,
        )
    }
}

#[derive(Debug, Clone)]
pub struct DbtPackage {
    pub dbt_project: DbtProject,
    pub package_root_path: PathBuf,
    pub dbt_properties: Vec<DbtAsset>,
    pub analysis_files: Vec<DbtAsset>,
    pub check_files: Vec<DbtAsset>,
    pub model_sql_files: Vec<DbtAsset>,
    pub function_sql_files: Vec<DbtAsset>,
    pub macro_files: Vec<DbtAsset>,
    pub test_files: Vec<DbtAsset>,
    pub fixture_files: Vec<DbtAsset>,
    pub seed_files: Vec<DbtAsset>,
    pub docs_files: Vec<DbtAsset>,
    pub snapshot_files: Vec<DbtAsset>,
    pub dependencies: BTreeSet<String>,
    pub all_paths: HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>>,
    /// Pre-read file contents for embedded (internal) packages.
    /// `None` for disk-based packages, `Some(map)` for embedded packages.
    /// Keyed by relative path (same as DbtAsset.path).
    pub embedded_file_contents: Option<HashMap<DbtPath, String>>,
    /// Raw dbt_project.yml.
    pub raw_project_yml: dbt_yaml::Value,
}

impl Default for DbtPackage {
    fn default() -> Self {
        DbtPackage {
            dbt_project: DbtProject::default(),
            package_root_path: PathBuf::new(),
            dbt_properties: vec![],
            analysis_files: vec![],
            check_files: vec![],
            model_sql_files: vec![],
            function_sql_files: vec![],
            macro_files: vec![],
            test_files: vec![],
            fixture_files: vec![],
            seed_files: vec![],
            docs_files: vec![],
            snapshot_files: vec![],
            dependencies: BTreeSet::new(),
            all_paths: HashMap::new(),
            embedded_file_contents: None,
            raw_project_yml: dbt_yaml::Value::default(),
        }
    }
}

pub use dbt_jinja_vars::DbtVars;

#[derive(Debug, Clone)]
pub struct DbtState {
    pub dbt_profile: DbtProfile,
    pub run_started_at: DateTime<Tz>,
    pub packages: Vec<DbtPackage>,
    /// Key is the package name, value are all package scoped vars
    pub vars: BTreeMap<String, IndexMap<String, DbtVars>>,
    pub cli_vars: BTreeMap<String, dbt_yaml::Value>,
    pub catalogs: Option<Arc<DbtCatalogs>>,
    pub cloud_config: Option<ResolvedCloudConfig>,
    pub warn_error: bool,
    pub warn_error_options: WarnErrorOptions,
}

impl DbtState {
    /// Assumes the root project is at the first entry
    /// see `fn all_package_paths` impl and its caller
    pub fn root_project_name(&self) -> &str {
        self.root_project().name.as_str()
    }

    pub fn root_package(&self) -> &DbtPackage {
        &self.packages[0]
    }

    pub fn root_project(&self) -> &DbtProject {
        &self.root_package().dbt_project
    }

    pub fn root_project_flags(&self) -> BTreeMap<String, minijinja::Value> {
        let flags = self.root_project().flags.clone();
        if let Some(flags) = flags {
            // Convert YmlValue directly to minijinja map
            crate::schemas::serde::yml_value_to_minijinja_map(flags)
        } else {
            BTreeMap::new()
        }
    }
}

impl fmt::Display for DbtState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for package in self.packages.iter() {
            writeln!(f, "Package: {}", package.dbt_project.name)?;
            let mut sorted_paths: Vec<_> = package.all_paths.iter().collect();
            sorted_paths.sort_by(|a, b| a.0.cmp(b.0));

            for (path_kind, paths) in sorted_paths {
                if !paths.is_empty() {
                    writeln!(f, "  {path_kind}:")?;
                    for (path, system_time) in paths {
                        let datetime: DateTime<Local> = DateTime::from(*system_time);
                        writeln!(
                            f,
                            "    {}, {}",
                            path.display(),
                            datetime.format("%Y-%m-%d %H:%M:%S")
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
}

pub trait NodeResolverTracker: fmt::Debug + Send + Sync {
    fn deep_clone(&self) -> Box<dyn NodeResolverTracker>;
    fn as_any(&self) -> &dyn Any;
    fn insert_ref(
        &mut self,
        node: &dyn InternalDbtNodeAttributes,
        adapter_type: AdapterType,
        model_status: ModelStatus,
        overwrite: bool,
    ) -> FsResult<()>;
    fn insert_function(
        &mut self,
        node: &dyn InternalDbtNodeAttributes,
        adapter_type: AdapterType,
        model_status: ModelStatus,
    ) -> FsResult<()>;
    fn insert_source(
        &mut self,
        package_name: &str,
        source: &DbtSource,
        adapter_type: AdapterType,
        model_status: ModelStatus,
    ) -> FsResult<()>;
    fn lookup_ref(
        &self,
        package_name: &Option<String>,
        name: &str,
        version: &Option<String>,
        node_package_name: &Option<String>,
    ) -> FsResult<(String, MinijinjaValue, ModelStatus, Option<MinijinjaValue>)>;
    /// Resolve a node's own relation by identity, for binding `this`. Unlike
    /// `lookup_ref`, a name shared with another node is not ambiguous here
    /// because `unique_id` selects the record.
    fn lookup_self_relation(
        &self,
        unique_id: &str,
        package_name: &str,
        name: &str,
        version: &Option<String>,
    ) -> FsResult<(MinijinjaValue, Option<MinijinjaValue>)>;
    fn lookup_source(
        &self,
        node_package_name: &str,
        source_name: &str,
        table_name: &str,
    ) -> FsResult<(String, MinijinjaValue, ModelStatus)>;
    fn lookup_function(
        &self,
        target_package: &Option<String>,
        function_name: &str,
        node_package: &Option<String>,
    ) -> FsResult<(String, MinijinjaValue, ModelStatus)>;
    fn compile_or_test(&self) -> bool;
    fn update_ref_with_deferral(
        &mut self,
        node: &dyn InternalDbtNodeAttributes,
        adapter_type: AdapterType,
        is_frontier: bool,
    ) -> FsResult<()>;

    /// Store pre-computed defer context for O(1) per-ref evaluation.
    /// Called once after defer phases + SA classification, before compilation.
    fn set_defer_context(
        &mut self,
        _node_introspections: HashMap<String, crate::schemas::IntrospectionKind>,
        _has_analyzed_schema: HashSet<String>,
        _nodes_materialized: HashSet<String>,
    ) {
        // No-op default for resolvers that don't need defer support
    }

    /// Returns whether the deferred (production) relation should be used for
    /// the given upstream node when referenced from `current_node_id`.
    fn prefers_deferred(&self, _current_node_id: &str, _upstream_id: &str) -> bool {
        false
    }
}

// test only
#[derive(Debug, Clone)]
pub struct DummyNodeResolverTracker;

impl NodeResolverTracker for DummyNodeResolverTracker {
    fn deep_clone(&self) -> Box<dyn NodeResolverTracker> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn insert_ref(
        &mut self,
        _node: &dyn InternalDbtNodeAttributes,
        _adapter_type: AdapterType,
        _model_status: ModelStatus,
        _overwrite: bool,
    ) -> FsResult<()> {
        // No-op for dummy
        Ok(())
    }

    fn insert_function(
        &mut self,
        _node: &dyn InternalDbtNodeAttributes,
        _adapter_type: AdapterType,
        _model_status: ModelStatus,
    ) -> FsResult<()> {
        // No-op for dummy
        Ok(())
    }

    fn insert_source(
        &mut self,
        _package_name: &str,
        _source: &DbtSource,
        _adapter_type: AdapterType,
        _model_status: ModelStatus,
    ) -> FsResult<()> {
        // No-op for dummy
        Ok(())
    }

    fn lookup_ref(
        &self,
        _package_name: &Option<String>,
        name: &str,
        _version: &Option<String>,
        _node_package_name: &Option<String>,
    ) -> FsResult<(String, MinijinjaValue, ModelStatus, Option<MinijinjaValue>)> {
        Err(fs_err!(
            ErrorCode::NotImplemented,
            "DummyNodeResolverTracker: lookup_ref not implemented for '{}'",
            name
        ))
    }

    fn lookup_self_relation(
        &self,
        _unique_id: &str,
        _package_name: &str,
        name: &str,
        _version: &Option<String>,
    ) -> FsResult<(MinijinjaValue, Option<MinijinjaValue>)> {
        Err(fs_err!(
            ErrorCode::NotImplemented,
            "DummyNodeResolverTracker: lookup_self_relation not implemented for '{}'",
            name
        ))
    }

    fn lookup_source(
        &self,
        _node_package_name: &str,
        source_name: &str,
        table_name: &str,
    ) -> FsResult<(String, MinijinjaValue, ModelStatus)> {
        Err(fs_err!(
            ErrorCode::NotImplemented,
            "DummyNodeResolverTracker: lookup_source not implemented for '{}.{}'",
            source_name,
            table_name
        ))
    }

    fn lookup_function(
        &self,
        _target_package: &Option<String>,
        function_name: &str,
        _node_package: &Option<String>,
    ) -> FsResult<(String, MinijinjaValue, ModelStatus)> {
        Err(fs_err!(
            ErrorCode::NotImplemented,
            "DummyNodeResolverTracker: lookup_function not implemented for '{}'",
            function_name
        ))
    }

    fn update_ref_with_deferral(
        &mut self,
        _node: &dyn InternalDbtNodeAttributes,
        _adapter_type: AdapterType,
        _is_frontier: bool,
    ) -> FsResult<()> {
        Err(fs_err!(
            ErrorCode::NotImplemented,
            "DummyNodeResolverTracker: update_ref_with_deferral not implemented"
        ))
    }

    fn compile_or_test(&self) -> bool {
        false
    }
}

impl Default for DummyNodeResolverTracker {
    fn default() -> Self {
        DummyNodeResolverTracker
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Macros {
    pub macros: BTreeMap<String, DbtMacro>,
    pub docs_macros: BTreeMap<String, DbtDocsMacro>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Operations {
    pub on_run_start: Vec<Spanned<DbtOperation>>,
    pub on_run_end: Vec<Spanned<DbtOperation>>,
}

pub type GetRelationCalls = BTreeMap<String, Vec<Arc<dyn BaseRelation>>>;
pub type GetColumnsInRelationCalls = BTreeMap<String, Vec<Arc<dyn BaseRelation>>>;
pub type PatternedDanglingSources = BTreeMap<String, Vec<RelationPattern>>;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestPathConfig {
    /// Package root relative to the root project directory. Empty for the root project.
    pub package_root_prefix: PathBuf,
    pub model_paths: Vec<String>,
    pub seed_paths: Vec<String>,
    pub snapshot_paths: Vec<String>,
    pub test_paths: Vec<String>,
    pub check_paths: Vec<String>,
    pub analysis_paths: Vec<String>,
    pub function_paths: Vec<String>,
    pub macro_paths: Vec<String>,
    pub docs_paths: Vec<String>,
}

impl ManifestPathConfig {
    pub fn from_dbt_project(dbt_project: &DbtProject) -> Self {
        Self {
            package_root_prefix: PathBuf::new(),
            model_paths: dbt_project.model_paths.clone().unwrap_or_default(),
            seed_paths: dbt_project.seed_paths.clone().unwrap_or_default(),
            snapshot_paths: dbt_project.snapshot_paths.clone().unwrap_or_default(),
            test_paths: dbt_project.test_paths.clone().unwrap_or_default(),
            check_paths: dbt_project.check_paths.clone().unwrap_or_default(),
            analysis_paths: dbt_project.analysis_paths.clone().unwrap_or_default(),
            function_paths: dbt_project.function_paths.clone().unwrap_or_default(),
            macro_paths: dbt_project.macro_paths.clone().unwrap_or_default(),
            docs_paths: dbt_project.docs_paths.clone().unwrap_or_default(),
        }
    }

    pub fn from_package(package: &DbtPackage, root_package_path: &Path) -> Self {
        let mut config = Self::from_dbt_project(&package.dbt_project);
        config.package_root_prefix = package
            .package_root_path
            .strip_prefix(root_package_path)
            .ok()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        config
    }

    pub fn for_packages(packages: &[DbtPackage]) -> BTreeMap<String, Self> {
        let root_package_path = packages
            .first()
            .map(|package| package.package_root_path.as_path())
            .unwrap_or_else(|| Path::new(""));
        packages
            .iter()
            .map(|package| {
                (
                    package.dbt_project.name.clone(),
                    Self::from_package(package, root_package_path),
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct ResolverState {
    pub root_project_name: String,
    pub adapter_type: AdapterType,
    pub nodes: Nodes,
    pub disabled_nodes: Nodes,
    pub macros: Macros,
    pub operations: Operations,
    pub dbt_profile: DbtProfile,
    pub cloud_config: Option<ResolvedCloudConfig>,
    pub render_results: RenderResults,
    pub node_resolver: Arc<dyn NodeResolverTracker>,
    pub get_relation_calls: GetRelationCalls,
    pub get_columns_in_relation_calls: GetColumnsInRelationCalls,
    pub patterned_dangling_sources: PatternedDanglingSources,
    pub run_started_at: DateTime<Tz>,
    pub runtime_config: Arc<DbtRuntimeConfig>,
    /// Minimal package-name lookup for manifest path serialization.
    ///
    /// This keeps artifact path normalization scoped to serialization instead
    /// of depending on full package runtime configs.
    pub manifest_path_configs: BTreeMap<String, ManifestPathConfig>,
    pub manifest_selectors: BTreeMap<String, DbtSelector>,
    pub resolved_selectors: ResolvedSelector,
    pub root_project_quoting: ResolvedQuoting,
    pub defer_nodes: Option<Nodes>,
    /// Nodes that had resolution errors (e.g., unresolved refs/sources)
    pub nodes_with_resolution_errors: HashSet<String>,
    /// Nodes whose SQL references models they are not permitted to access (group/access violations)
    pub nodes_with_access_errors: HashSet<String>,
    pub semantic_layer_spec_is_legacy: bool,
    /// Mapping from truncated/hashed generic test names to their original pre-hash full names.
    ///
    /// For now this is populated as empty; later we can use it (e.g. in replay mode) to reconcile
    /// naming differences between Fusion and external recordings.
    pub test_name_truncations: HashMap<String, String>,
    /// Declared (user-authored YAML `columns:`) schema, hydrated once from
    /// `nodes` when this `ResolverState` is built. Ground truth, not
    /// inferred — consulted during binding via a `CatalogProviderList`
    /// wrapper (see `dbt-tasks::declared_schema_catalog`).
    pub user_defined_schema_registry:
        Arc<dbt_common::user_defined_schema_registry::UserDefinedSchemaRegistry>,
}

pub fn hydrate_user_defined_schema_registry(
    nodes: &Nodes,
    adapter_type: AdapterType,
) -> Arc<dbt_common::user_defined_schema_registry::UserDefinedSchemaRegistry> {
    let registry =
        Arc::new(dbt_common::user_defined_schema_registry::UserDefinedSchemaRegistry::new());

    fn record(
        node: &dyn InternalDbtNode,
        registry: &dbt_common::user_defined_schema_registry::UserDefinedSchemaRegistry,
        adapter_type: AdapterType,
    ) {
        let base = node.base();
        if base.columns.is_empty() {
            return;
        }
        let (database, schema, alias) = match adapter_type {
            AdapterType::Snowflake => (
                base.database.to_uppercase(),
                base.schema.to_uppercase(),
                base.alias.to_uppercase(),
            ),
            _ => (
                base.database.clone(),
                base.schema.clone(),
                base.alias.clone(),
            ),
        };
        let relation =
            datafusion_common::TableReference::full(database, schema, alias).to_quoted_string();
        for column in &base.columns {
            let name = match adapter_type {
                AdapterType::Snowflake => column.name.to_uppercase(),
                _ => column.name.clone(),
            };
            registry.record_column(&relation, &name);
        }
    }

    for model in nodes.models.values() {
        record(model.as_ref(), &registry, adapter_type);
    }
    for source in nodes.sources.values() {
        record(source.as_ref(), &registry, adapter_type);
    }
    for seed in nodes.seeds.values() {
        record(seed.as_ref(), &registry, adapter_type);
    }
    for snapshot in nodes.snapshots.values() {
        record(snapshot.as_ref(), &registry, adapter_type);
    }

    registry
}

impl ResolverState {
    // Build reverse dependency map (who depends on whom)
    pub fn create_reverse_deps(&self) -> BTreeMap<String, BTreeSet<String>> {
        let mut reverse_deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (unique_id, node) in self.nodes.iter() {
            for (dep, _) in node.base().depends_on.nodes_with_ref_location.iter() {
                reverse_deps
                    .entry(dep.clone())
                    .or_default()
                    .insert(unique_id.clone());
            }
        }
        reverse_deps
    }

    pub fn deep_clone(&self) -> Self {
        // XXX: for some reason, Clone is not a deep clone yet
        let mut resolved_state = self.clone();
        resolved_state.node_resolver = resolved_state.node_resolver.deep_clone().into();
        resolved_state
    }

    pub fn get_defer_node_by_id(&self, node_id: &str) -> Option<&dyn InternalDbtNodeAttributes> {
        if let Some(defer_nodes) = &self.defer_nodes {
            defer_nodes.get_node(node_id)
        } else {
            None
        }
    }
}

impl fmt::Display for ResolverState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ResolverState {{ nodes: {:?}, dbt_profile: {}, macro_collector: {:?} }}",
            self.nodes, self.dbt_profile, self.render_results
        )
    }
}

// A subset of resolver state
#[derive(Debug, Clone, Default)]
pub struct ResolvedNodes {
    pub nodes: Nodes,
    pub disabled_nodes: Nodes,
    pub macros: Macros,
    pub operations: Operations,
}
// A changeset describes the difference between two sets of files
// - one on the file system
// - one in content addressable store CAS) in a dbt project.
// The changeset contains:
// - files that are the same in both sets
// - files that are different in both sets
// - files that are missing in the filesystem
// - files that are missing in the CAS
// - whether the deps are the same e.g. (i.e dependencies.yml, package.lock and all dbt_packages)
// files are represented by their relative path to the project root
#[derive(Debug, Clone, Default)]
pub struct FileChanges {
    // changed files
    pub changed_files: HashSet<DbtPath>,
    // unimpacted files
    pub unimpacted_files: HashSet<DbtPath>,
    // impacted files
    pub impacted_files: HashSet<DbtPath>,
    // deleted files
    pub deleted_files: HashSet<DbtPath>,
    // new files
    pub new_files: HashSet<DbtPath>,
}
impl FileChanges {
    pub fn no_change(&self) -> bool {
        self.impacted_files.is_empty()
            && self.deleted_files.is_empty()
            && self.new_files.is_empty()
            && !self.unimpacted_files.is_empty()
    }
    pub fn has_changes(&self) -> bool {
        !self.impacted_files.is_empty() || !self.new_files.is_empty()
    }
}
/// Represents the execution state of a node in the dbt project.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NodeExecutionState {
    #[default]
    NotProcessed,
    Parsed,
    Compiled,
    Run,
}
impl NodeExecutionState {
    /// Converts a command to a NodeExecutionState
    pub fn from_cmd(cmd: FsCommand) -> Self {
        match cmd {
            FsCommand::Parse => NodeExecutionState::Parsed,
            FsCommand::Compile => NodeExecutionState::Compiled,
            FsCommand::Run
            | FsCommand::Build
            | FsCommand::Test
            | FsCommand::Snapshot
            | FsCommand::Seed => NodeExecutionState::Run,
            _ => NodeExecutionState::NotProcessed,
        }
    }
}
impl fmt::Display for NodeExecutionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
/// Represents the status of a phase in the execution of a node.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NodeExecutionStatus {
    #[default]
    Success,
    Error,
    Skipped,
    Aborted, // e.g. interrupted by user.
    Reused,
    Passed, // For test nodes.
    Failed, // For test nodes.
}
impl fmt::Display for NodeExecutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NodeStatus {
    pub latest_state: Option<NodeExecutionState>,
    pub latest_status: Option<NodeExecutionStatus>,
    pub latest_time: Option<String>,
    pub latest_message: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CacheState {
    pub file_changes: FileChanges,
    /// Only the unimpacted resolved nodes from file changes.
    pub unimpacted_resolved_nodes: ResolvedNodes,
    /// Only the unimpacted node statuses from file changes.
    pub unimpacted_node_statuses: HashMap<String, NodeStatus>,
    /// Only the unimpacted relation calls from file changes.
    /// Only needed for incremental compile.
    /// This can be empty for the build cache.
    pub unimpacted_get_relation_calls: GetRelationCalls,
    /// Only the unimpacted columns in relation calls from file changes.
    /// Only needed for incremental compile.
    /// This can be empty for the build cache.
    pub unimpacted_get_columns_in_relation_calls: GetColumnsInRelationCalls,
    /// Only the unimpacted dangling sources from file changes.
    /// Only needed for incremental compile.
    /// This can be empty for the build cache.
    pub unimpacted_patterned_dangling_sources: PatternedDanglingSources,
    /// The changed nodes, by unique id, based on file changes.
    /// Does not include any deleted or added nodes.
    pub changed_nodes: Arc<HashSet<String>>,
    /// The impacted nodes, by unique id, from files changes.
    pub impacted_nodes: Arc<HashSet<String>>,
}
impl CacheState {
    pub fn has_changes(&self) -> bool {
        self.file_changes.has_changes()
    }
}
#[derive(Debug, Clone, Default)]
pub struct RenderResults {
    pub rendering_results: BTreeMap<String, (String, MacroSpans)>,
}

#[derive(Debug, Clone, Default)]
pub struct DbtRuntimeConfig {
    pub runtime_config: BTreeMap<String, minijinja::Value>,
    pub dependencies: BTreeMap<String, Arc<DbtRuntimeConfig>>,
    pub vars: minijinja::Value,
    pub inner: DbtRuntimeConfigInner,
}

type RuntimeConfigMap = BTreeMap<String, Arc<DbtRuntimeConfig>>;
type SharedRuntimeConfigMap = Arc<RwLock<RuntimeConfigMap>>;

/// Global registry of runtime configs, used to populate `config.dependencies` in a way that matches
/// dbt-core’s effective behavior ("all loaded packages"), even when direct deps are skipped
/// (e.g. `--skip-private-deps` but packages exist on disk in a repro bundle).
static GLOBAL_RUNTIME_CONFIGS: OnceLock<SharedRuntimeConfigMap> = OnceLock::new();

fn global_runtime_configs() -> &'static SharedRuntimeConfigMap {
    GLOBAL_RUNTIME_CONFIGS.get_or_init(|| Arc::new(RwLock::new(RuntimeConfigMap::new())))
}

/// Register a package runtime config into the global registry.
pub fn register_global_runtime_config(package_name: String, cfg: Arc<DbtRuntimeConfig>) {
    let mut map = global_runtime_configs().write().expect("poisoned lock");
    map.insert(package_name, cfg);
}

fn snapshot_global_runtime_configs() -> Option<RuntimeConfigMap> {
    let map = global_runtime_configs().read().ok()?;
    if map.is_empty() {
        None
    } else {
        Some(map.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InvocationArgs {
    pub require_explicit_package_overrides_for_builtin_materializations: bool,
    pub require_resource_names_without_spaces: bool,
    pub source_freshness_run_project_hooks: bool,
    pub skip_nodes_if_on_run_start_fails: bool,
    pub state_modified_compare_more_unrendered_values: bool,
    pub require_yaml_configuration_for_mf_time_spines: bool,
    pub require_batched_execution_for_custom_microbatch_strategy: bool,
}

impl Default for InvocationArgs {
    fn default() -> Self {
        Self {
            require_explicit_package_overrides_for_builtin_materializations: true,
            require_resource_names_without_spaces: true,
            source_freshness_run_project_hooks: true,
            skip_nodes_if_on_run_start_fails: true,
            state_modified_compare_more_unrendered_values: true,
            require_yaml_configuration_for_mf_time_spines: true,
            require_batched_execution_for_custom_microbatch_strategy: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbtRuntimeConfigInner {
    // Profile configuration
    pub profile_name: String,
    pub target_name: String,
    pub threads: Option<usize>,
    pub credentials: Option<DbConfig>,
    pub profile_env_vars: HashMap<String, String>,
    pub args: InvocationArgs,

    // Project configuration
    pub project_name: String,
    pub version: Option<String>,
    pub project_root: PathBuf,

    // Path configurations
    pub model_paths: Vec<String>,
    pub macro_paths: Vec<String>,
    pub seed_paths: Vec<String>,
    pub test_paths: Vec<String>,
    pub analysis_paths: Vec<String>,
    pub docs_paths: Vec<String>,
    pub asset_paths: Vec<String>,
    pub target_path: String,
    pub snapshot_paths: Vec<String>,
    pub clean_targets: Vec<String>,
    pub log_path: String,

    // Package configurations
    pub packages_install_path: String,

    // Project configurations
    pub quoting: Option<DbtQuoting>,
    pub models: Option<ProjectModelConfig>,
    pub seeds: Option<ProjectSeedConfig>,
    pub snapshots: Option<ProjectSnapshotConfig>,
    pub sources: Option<ProjectSourceConfig>,
    pub tests: Option<ProjectDataTestConfig>,
    pub query_comment: Option<QueryComment>,

    // Variables and hooks
    pub vars: IndexMap<String, DbtVars>,
    pub cli_vars: BTreeMap<String, dbt_yaml::Value>,
    pub on_run_start: Vec<Spanned<String>>,
    pub on_run_end: Vec<Spanned<String>>,

    // Version info
    pub config_version: Option<i32>,
    pub require_dbt_version: Option<StringOrArrayOfStrings>,
    pub restrict_access: Option<bool>,

    // Runtime info
    pub invoked_at: DateTime<Utc>,

    // Project flags
    /// When true, `latest_version_pointer` is enabled by default for all versioned models
    /// (can be overridden per-model with `latest_version_pointer: {enabled: false}`)
    #[serde(default = "default_true")]
    pub latest_version_pointer_enabled_by_default: bool,
}

impl DbtRuntimeConfig {
    /// Adds a self reference to the dependencies
    pub fn add_self_to_dependencies(&mut self, package_name: &str) {
        // TODO: This is a hack to get around the fact that DbtRuntimeConfig is a circular reference
        // it fixes this issue one level deep, but not more (hence, config.dependencies[self].dependencies will
        // not contain itself).
        let self_clone = self.clone();
        self.dependencies
            .insert(package_name.to_string(), Arc::new(self_clone));
    }

    pub fn new(
        in_dir: &Path,
        package: &DbtPackage,
        profile: &DbtProfile,
        dependency_lookup: &BTreeMap<String, Arc<DbtRuntimeConfig>>,
        vars: &IndexMap<String, DbtVars>,
        cli_vars: &BTreeMap<String, dbt_yaml::Value>,
    ) -> Self {
        let runtime_config_inner = DbtRuntimeConfigInner {
            profile_name: profile.profile.clone(),
            target_name: profile.target.clone(),
            threads: profile.threads,
            credentials: Some(profile.default_db_config().clone()),
            profile_env_vars: HashMap::new(),

            project_name: package.dbt_project.name.clone(),
            version: package.dbt_project.version.clone().map(|v| match v {
                FloatOrString::Number(n) => n.to_string(),
                FloatOrString::String(s) => s,
            }),
            project_root: in_dir.to_path_buf(),
            model_paths: package.dbt_project.model_paths.clone().unwrap_or_default(),
            macro_paths: package.dbt_project.macro_paths.clone().unwrap_or_default(),
            seed_paths: package.dbt_project.seed_paths.clone().unwrap_or_default(),
            test_paths: package.dbt_project.test_paths.clone().unwrap_or_default(),
            analysis_paths: package
                .dbt_project
                .analysis_paths
                .clone()
                .unwrap_or_default(),
            docs_paths: package.dbt_project.docs_paths.clone().unwrap_or_default(),
            asset_paths: package.dbt_project.asset_paths.clone().unwrap_or_default(),
            target_path: package
                .dbt_project
                .target_path
                .clone()
                .unwrap_or_default()
                .to_string(),
            snapshot_paths: package
                .dbt_project
                .snapshot_paths
                .clone()
                .unwrap_or_default(),
            clean_targets: package
                .dbt_project
                .clean_targets
                .clone()
                .unwrap_or_default(),
            log_path: package
                .dbt_project
                .log_path
                .clone()
                .unwrap_or_default()
                .to_string(),
            packages_install_path: package
                .dbt_project
                .packages_install_path
                .clone()
                .unwrap_or_default(),
            quoting: package.dbt_project.quoting.clone().into_inner(),
            models: package.dbt_project.models.clone(),
            seeds: package.dbt_project.seeds.clone(),
            snapshots: package.dbt_project.snapshots.clone(),
            sources: package.dbt_project.sources.clone(),
            tests: package.dbt_project.tests.clone(),
            query_comment: (*package.dbt_project.query_comment).clone(),
            vars: vars.clone(),
            cli_vars: cli_vars.clone(),
            on_run_start: match &*package.dbt_project.on_run_start {
                Some(SpannedStringOrArrayOfStrings::String(s)) => vec![s.clone()],
                Some(SpannedStringOrArrayOfStrings::ArrayOfStrings(v)) => v.clone(),
                _ => vec![],
            },
            on_run_end: match &*package.dbt_project.on_run_end {
                Some(SpannedStringOrArrayOfStrings::String(s)) => vec![s.clone()],
                Some(SpannedStringOrArrayOfStrings::ArrayOfStrings(v)) => v.clone(),
                _ => vec![],
            },
            config_version: package.dbt_project.config_version,
            require_dbt_version: package.dbt_project.require_dbt_version.clone(),
            restrict_access: package.dbt_project.restrict_access,
            invoked_at: Utc::now(),
            args: InvocationArgs::default(),
            latest_version_pointer_enabled_by_default:
                resolve_latest_version_pointer_enabled_by_default(
                    package.dbt_project.flags.as_ref(),
                ),
        };

        // TODO(anna): Look into whether this should also be Index map
        let mut runtime_config = Self {
            runtime_config: Deserialize::deserialize(
                dbt_yaml::to_value(&runtime_config_inner).unwrap(),
            )
            .unwrap(),
            dependencies: BTreeMap::new(),
            vars: minijinja::Value::from_object(VarProvider::new(
                Deserialize::deserialize(dbt_yaml::to_value(&runtime_config_inner.vars).unwrap())
                    .unwrap(),
            )),
            inner: runtime_config_inner,
        };

        runtime_config.add_self_to_dependencies(&package.dbt_project.name);
        for package_name in package.dependencies.iter() {
            runtime_config.dependencies.insert(
                package_name.clone(),
                dependency_lookup
                    .get(package_name)
                    .expect("Dependency not resolved in correct order")
                    .clone(),
            );
        }
        runtime_config
    }

    /// Converts this runtime config to a pure map structure for MiniJinja
    pub fn to_minijinja_map(&self) -> BTreeMap<String, minijinja::Value> {
        let mut result = self.runtime_config.clone();

        // Convert dependencies to maps recursively
        let mut deps_map = BTreeMap::new();
        for (key, dep_config) in &self.dependencies {
            deps_map.insert(
                key.clone(),
                minijinja::Value::from_object(dep_config.to_minijinja_map()),
            );
        }

        // Add dependencies to the result
        result.insert(
            "dependencies".to_string(),
            minijinja::Value::from_object(deps_map),
        );

        result
    }
}

impl Object for DbtRuntimeConfig {
    fn get_value(self: &Arc<Self>, key: &minijinja::Value) -> Option<minijinja::value::Value> {
        match key.as_str()? {
            // This is a special case for the dependencies
            // We use the to_minijinja_map helper to convert dependencies recursively
            "dependencies" => {
                let mut deps = BTreeMap::new();
                // Prefer the global registry (all loaded packages) if it has been populated, but
                // always merge in this config’s local dependency map.
                //
                // This preserves the invariant that `config.dependencies[<this_project>]` exists
                // (via `add_self_to_dependencies`) even when the global registry is only partially
                // populated during early compile phases.
                if let Some(global) = snapshot_global_runtime_configs() {
                    for (key, value) in global.iter() {
                        deps.insert(
                            key.clone(),
                            minijinja::Value::from_object(value.to_minijinja_map()),
                        );
                    }
                }
                for (key, value) in self.dependencies.iter() {
                    deps.insert(
                        key.clone(),
                        minijinja::Value::from_object(value.to_minijinja_map()),
                    );
                }
                Some(minijinja::Value::from_object(deps))
            }
            "vars" => Some(self.vars.clone()),
            // Otherwise, we just return the value from the runtime config
            other => self.runtime_config.get(other).cloned(),
        }
    }
}

/// Analogue of python VarProvider
#[derive(Debug, Default, Clone)]
pub struct VarProvider(BTreeMap<String, minijinja::Value>);

impl VarProvider {
    pub fn new(map: BTreeMap<String, minijinja::Value>) -> VarProvider {
        VarProvider(map)
    }
}
impl Object for VarProvider {
    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[minijinja::Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<minijinja::Value, minijinja::Error> {
        match method {
            "to_dict" => {
                if !args.is_empty() {
                    return Err(minijinja::Error::new(
                        minijinja::ErrorKind::TooManyArguments,
                        "to_dict() takes no arguments",
                    ));
                }
                Ok(minijinja::Value::from(self.0.clone()))
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("Unknown method on VarProvider: '{method}'"),
            )),
        }
    }
}
/// Represents the status of a model
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum ModelStatus {
    /// Model is enabled and successfully parsed
    Enabled,
    /// Model is disabled by configuration
    Disabled,
    /// Model failed to parse
    ParsingFailed,
}

#[cfg(test)]
mod dbt_profile_adapter_tests {
    use super::*;
    use crate::schemas::profiles::{DuckDbConfig, SnowflakeDbConfig};

    fn profile(
        adapters: IndexMap<AdapterType, ProfileAdapter>,
        default_adapter: AdapterType,
    ) -> DbtProfile {
        DbtProfile {
            profile: "p".into(),
            target: "prod".into(),
            defer_to_target: None,
            allow_clones: true,
            adapters,
            default_adapter,
            schema: "s".into(),
            database: "d".into(),
            relative_profile_path: PathBuf::from("profiles.yml"),
            threads: None,
        }
    }

    fn duckdb() -> DbConfig {
        DbConfig::DuckDB(Box::<DuckDbConfig>::default())
    }

    fn snowflake() -> DbConfig {
        DbConfig::Snowflake(Box::<SnowflakeDbConfig>::default())
    }

    fn one(config: DbConfig) -> (AdapterType, ProfileAdapter) {
        (config.adapter_type(), ProfileAdapter::single(config))
    }

    /// `default_db_config()` must follow `default_adapter`, not insertion order — the
    /// default is frequently not the first entry.
    #[test]
    fn db_config_follows_default_adapter_not_declaration_order() {
        let adapters = IndexMap::from([one(snowflake()), one(duckdb())]);
        let p = profile(adapters, AdapterType::DuckDB);

        assert_eq!(p.default_db_config().adapter_type(), AdapterType::DuckDB);
        assert_eq!(
            p.adapter(AdapterType::Snowflake).unwrap().adapter_type(),
            AdapterType::Snowflake
        );
        assert!(p.adapter(AdapterType::Bigquery).is_none());
    }

    #[test]
    fn has_valid_adapters_rejects_a_default_that_is_not_a_key() {
        let adapters = IndexMap::from([one(snowflake())]);
        let p = profile(adapters, AdapterType::DuckDB);

        assert!(
            !p.has_valid_adapters(),
            "a default_adapter absent from `adapters` must be reported invalid"
        );
    }

    /// A persisted profile could carry a default_connection past the end of the
    /// list; `db_config()` would panic on it, so it must be reported invalid first.
    #[test]
    fn has_valid_adapters_rejects_an_out_of_range_default_connection() {
        let mut adapter = ProfileAdapter::single(snowflake());
        adapter.default_connection = 3;
        let adapters = IndexMap::from([(AdapterType::Snowflake, adapter)]);
        let p = profile(adapters, AdapterType::Snowflake);

        assert!(!p.has_valid_adapters());
    }

    /// Thread count is applied target-wide, so every *connection* of every adapter
    /// sees it -- not just the reachable one.
    #[test]
    fn adapters_mut_reaches_every_connection() {
        let mut snowflake_adapter = ProfileAdapter::single(snowflake());
        snowflake_adapter.connections.push(ProfileConnection {
            name: "secondary".to_string(),
            config: snowflake(),
        });
        let adapters = IndexMap::from([(AdapterType::Snowflake, snowflake_adapter), one(duckdb())]);
        let mut p = profile(adapters, AdapterType::Snowflake);

        for adapter in p.adapters_mut() {
            adapter.set_threads(Some(crate::schemas::serde::StringOrInteger::Integer(7)));
        }

        let reached: Vec<_> = p
            .adapters
            .values()
            .flat_map(|a| a.connections.iter())
            .map(|c| c.config.get_threads().cloned())
            .collect();
        assert_eq!(reached.len(), 3, "two snowflake connections plus duckdb");
        for threads in reached {
            assert!(
                matches!(
                    threads,
                    Some(crate::schemas::serde::StringOrInteger::Integer(7))
                ),
                "every connection should have threads set, got {threads:?}"
            );
        }
    }
}

#[cfg(test)]
mod user_defined_schema_registry_tests {
    use super::*;
    use crate::schemas::dbt_column::DbtColumn;
    use crate::schemas::{
        CommonAttributes, DbtModel, DbtModelAttr, DbtSeed, DbtSnapshot, NodeBaseAttributes,
    };
    use std::collections::BTreeMap;

    fn base_attr(
        database: &str,
        schema: &str,
        alias: &str,
        columns: &[&str],
    ) -> NodeBaseAttributes {
        NodeBaseAttributes {
            database: database.to_string(),
            schema: schema.to_string(),
            alias: alias.to_string(),
            columns: columns
                .iter()
                .map(|name| {
                    Arc::new(DbtColumn {
                        name: name.to_string(),
                        ..Default::default()
                    })
                })
                .collect(),
            ..Default::default()
        }
    }

    fn model_with_columns(database: &str, schema: &str, alias: &str, columns: &[&str]) -> DbtModel {
        DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: "model.m".to_string(),
                ..Default::default()
            },
            __base_attr__: base_attr(database, schema, alias, columns),
            __model_attr__: DbtModelAttr::default(),
            ..Default::default()
        }
    }

    #[test]
    fn hydrate_from_declared_columns_uppercases_for_snowflake() {
        let mut models = BTreeMap::new();
        models.insert(
            "model.m".to_string(),
            Arc::new(model_with_columns("db", "sch", "tbl", &["col"])),
        );
        let nodes = Nodes {
            models,
            ..Default::default()
        };
        let registry = hydrate_user_defined_schema_registry(&nodes, AdapterType::Snowflake);
        let relation =
            datafusion_common::TableReference::full("DB", "SCH", "TBL").to_quoted_string();
        assert!(registry.columns_for(&relation).unwrap().contains("COL"));
    }

    #[test]
    fn hydrate_from_declared_columns_keeps_case_for_non_snowflake_adapters() {
        let mut models = BTreeMap::new();
        models.insert(
            "model.m".to_string(),
            Arc::new(model_with_columns("db", "sch", "tbl", &["col"])),
        );
        let nodes = Nodes {
            models,
            ..Default::default()
        };
        let registry = hydrate_user_defined_schema_registry(&nodes, AdapterType::DuckDB);
        let relation =
            datafusion_common::TableReference::full("db", "sch", "tbl").to_quoted_string();
        assert!(registry.columns_for(&relation).unwrap().contains("col"));
    }

    #[test]
    fn hydrate_from_declared_columns_skips_models_without_declared_columns() {
        let mut models = BTreeMap::new();
        models.insert(
            "model.m".to_string(),
            Arc::new(model_with_columns("db", "sch", "tbl", &[])),
        );
        let nodes = Nodes {
            models,
            ..Default::default()
        };
        let registry = hydrate_user_defined_schema_registry(&nodes, AdapterType::Snowflake);
        let relation =
            datafusion_common::TableReference::full("DB", "SCH", "TBL").to_quoted_string();
        assert!(registry.columns_for(&relation).is_none());
    }

    #[test]
    fn hydrate_from_declared_columns_covers_sources_seeds_and_snapshots_too() {
        let mut sources = BTreeMap::new();
        sources.insert(
            "source.s".to_string(),
            Arc::new(DbtSource {
                __common_attr__: CommonAttributes {
                    unique_id: "source.s".to_string(),
                    ..Default::default()
                },
                __base_attr__: base_attr("db", "sch", "src_tbl", &["scol"]),
                ..Default::default()
            }),
        );

        let mut seeds = BTreeMap::new();
        seeds.insert(
            "seed.sd".to_string(),
            Arc::new(DbtSeed {
                __common_attr__: CommonAttributes {
                    unique_id: "seed.sd".to_string(),
                    ..Default::default()
                },
                __base_attr__: base_attr("db", "sch", "seed_tbl", &["seedcol"]),
                ..Default::default()
            }),
        );

        let mut snapshots = BTreeMap::new();
        snapshots.insert(
            "snapshot.sn".to_string(),
            Arc::new(DbtSnapshot {
                __common_attr__: CommonAttributes {
                    unique_id: "snapshot.sn".to_string(),
                    ..Default::default()
                },
                __base_attr__: base_attr("db", "sch", "snap_tbl", &["snapcol"]),
                ..Default::default()
            }),
        );

        let nodes = Nodes {
            sources,
            seeds,
            snapshots,
            ..Default::default()
        };
        let registry = hydrate_user_defined_schema_registry(&nodes, AdapterType::DuckDB);

        let source_relation =
            datafusion_common::TableReference::full("db", "sch", "src_tbl").to_quoted_string();
        assert!(
            registry
                .columns_for(&source_relation)
                .unwrap()
                .contains("scol")
        );

        let seed_relation =
            datafusion_common::TableReference::full("db", "sch", "seed_tbl").to_quoted_string();
        assert!(
            registry
                .columns_for(&seed_relation)
                .unwrap()
                .contains("seedcol")
        );

        let snapshot_relation =
            datafusion_common::TableReference::full("db", "sch", "snap_tbl").to_quoted_string();
        assert!(
            registry
                .columns_for(&snapshot_relation)
                .unwrap()
                .contains("snapcol")
        );
    }
}
