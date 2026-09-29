use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::SystemTime,
};

use crate::config::CompilationConfig;
use dbt_adapter::{
    Adapter, AdapterBuilder, AdapterEngine, AdapterImpl, AdapterStore, AdapterType,
    adapter::AdapterFactory,
    cache::RelationCache,
    config::AdapterConfig,
    engine::{SidecarClient, SidecarEngine, query_comment::QueryCommentConfig},
    sql_types::TypeOpsFactory,
};
use dbt_adbc::Backend;
use dbt_common::{
    cancellation::CancellationToken,
    io_args::{FsCommand, ReplayMode},
    path::DbtPath,
    tracing::TracingConfigProvider,
};
use dbt_error::{ErrorCode, FsResult, fs_err};
use dbt_jinja_utils::{
    JinjaFactory, flags::Flags, jinja_environment::JinjaEnv,
    listener::JinjaTypeCheckingEventListenerFactory,
};
use dbt_loader::{
    args::{IoArgs, LoadArgs},
    load,
    loader_hooks::LoaderHooks,
};
use dbt_metadata::parse_cache::{
    PreviousResolvedState, add_all_unchanged_nodes,
    determine_cache_state_from_previous_previous_resolved_nodes, drop_all_unchanged_nodes,
};
use dbt_parser::args::ResolveArgs;
use dbt_parser::resolver_hooks::ResolverHooks;
use dbt_schema_store::SchemaStoreTrait;
use dbt_schemas::{
    dbt_utils::resolve_package_quoting,
    schemas::{
        ResolvedCloudConfig,
        common::{DbtQuoting, ResolvedQuoting},
        macros::build_macro_units,
        profiles::{DbConfig, Execute},
        project::{DbtProject, QueryComment},
        relations::DEFAULT_RESOLVED_QUOTING,
    },
    state::{
        CacheState, DbtPackage, DbtState, GetColumnsInRelationCalls, GetRelationCalls,
        PatternedDanglingSources, ResolverState, ResourcePathKind,
    },
};
use fs_deps::private_package::PrivatePackageResolver;

/// `Clone` so the adapter-store builder closure can own a handle: every field is
/// either `Arc` or cheaply cloneable, so this is a handle copy rather than a deep
/// one.
#[derive(Clone)]
pub struct DbtLoadedProject {
    config: CompilationConfig,
    type_ops_factory: Arc<dyn TypeOpsFactory>,
    /// DO NOT EXPOSE. Callers should use [DbtLoadedProject::init_adapter].
    adapter_factory: Arc<dyn AdapterFactory>,
    dbt_state: Arc<DbtState>,
    jinja_factory: Arc<dyn JinjaFactory>,
}

/// Controls whether adapter initialization may construct an engine backed by a remote system.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterConnectionMode {
    AllowRemote,
    Offline,
}

/// Phase 1: Load and hydrate cache with optional previous state for incremental compilation
async fn load_phase(
    config: CompilationConfig,
    mut load_args: LoadArgs,
    invocation_args: Cow<'_, dbt_jinja_utils::invocation_args::InvocationArgs>,
    type_ops_factory: Arc<dyn TypeOpsFactory>,
    adapter_factory: Arc<dyn AdapterFactory>,
    maybe_prev_loaded_project: Option<&DbtLoadedProject>,
    tracing_config: Option<&dyn TracingConfigProvider>,
    token: &CancellationToken,
    loader_hooks: Arc<dyn LoaderHooks>,
    private_package_resolver: Arc<dyn PrivatePackageResolver>,
    jinja_factory: Arc<dyn JinjaFactory>,
) -> FsResult<DbtLoadedProject> {
    // Set previous state for incremental compilation if provided
    if let Some(prev_dbt_state) = maybe_prev_loaded_project
        .as_ref()
        .map(|x| x.dbt_state.clone())
    {
        load_args.prev_dbt_state = Some(prev_dbt_state);
    }

    // Load dbt project
    let dbt_state = load(
        &load_args,
        invocation_args,
        tracing_config,
        token,
        loader_hooks,
        private_package_resolver,
    )
    .await?;

    Ok(DbtLoadedProject {
        config,
        type_ops_factory,
        adapter_factory,
        dbt_state: Arc::new(dbt_state),
        jinja_factory,
    })
}

/// Phase 2: Resolve and parse with optional listener factory for LSP
async fn resolve_phase(
    loaded_project: &DbtLoadedProject,
    resolve_args: ResolveArgs,
    invocation_args: &dbt_jinja_utils::invocation_args::InvocationArgs,
    cache_state: Option<&CacheState>,
    token: &CancellationToken,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
    resolver_hooks: Arc<dyn ResolverHooks>,
) -> FsResult<(ResolverState, Arc<JinjaEnv>)> {
    use dbt_parser::resolver::resolve;
    use dbt_schemas::state::ResolvedNodes;

    let io = &resolve_args.io;
    let dbt_state = loaded_project.dbt_state.clone();

    // Drop unchanged nodes for build cache and incremental
    let dbt_state = if let Some(cache) = cache_state {
        // only clone if we have a cache state
        let mut dbt_state = dbt_state.as_ref().clone();
        drop_all_unchanged_nodes(io, &mut dbt_state, &cache.file_changes.unimpacted_files);
        Arc::new(dbt_state)
    } else {
        dbt_state
    };

    // Get cached macros and nodes if available. `operations` is intentionally not seeded
    // here; `add_all_unchanged_nodes` below owns restoring it after resolution.
    let ResolvedNodes {
        nodes,
        disabled_nodes,
        macros,
        operations: _,
    } = cache_state
        .map(|c| c.unimpacted_resolved_nodes.clone())
        .unwrap_or_default();

    let get_relation_calls = if let Some(cache) = cache_state {
        cache.unimpacted_get_relation_calls.clone()
    } else {
        GetRelationCalls::default()
    };

    let get_columns_in_relation_calls = if let Some(cache) = cache_state {
        cache.unimpacted_get_columns_in_relation_calls.clone()
    } else {
        GetColumnsInRelationCalls::default()
    };

    let patterned_dangling_sources = if let Some(cache) = cache_state {
        cache.unimpacted_patterned_dangling_sources.clone()
    } else {
        PatternedDanglingSources::default()
    };

    // Call the actual resolver
    let (mut resolved_state, jinja_env) = resolve(
        &resolve_args,
        invocation_args,
        dbt_state.clone(),
        macros,
        nodes,
        disabled_nodes,
        get_relation_calls,
        get_columns_in_relation_calls,
        patterned_dangling_sources,
        token,
        jinja_type_checking_event_listener_factory,
        resolver_hooks,
        loaded_project.jinja_factory.clone(),
    )
    .await?;
    // Add unchanged nodes back if we have cache
    if let Some(cache) = cache_state {
        add_all_unchanged_nodes(&mut resolved_state, &cache.unimpacted_resolved_nodes);
    }

    Ok((resolved_state, jinja_env))
}

/// Contains a list of file paths that have been modified.
#[derive(Debug)]
struct FileChangeset {
    /// Files that have been modified.
    pub changed: Vec<String>,
}

fn is_resource_changeable(kind: &ResourcePathKind) -> bool {
    if let ResourcePathKind::ModelPaths
    | ResourcePathKind::SeedPaths
    | ResourcePathKind::AnalysisPaths = kind
    {
        return true;
    }
    false
}

// TODO: Chenyu this will be moved to use CAS as previous state on next PR.
fn is_internal_package(pkg: &DbtPackage) -> bool {
    pkg.package_root_path
        .components()
        .any(|c| c.as_os_str() == "dbt_internal_packages")
}

async fn compute_file_changeset(
    prev_dbt_state: &DbtState,
    current_dbt_state: &DbtState,
    token: &CancellationToken,
) -> FsResult<FileChangeset> {
    let prev_packages: Vec<_> = prev_dbt_state
        .packages
        .iter()
        .filter(|p| !is_internal_package(p))
        .collect();
    let current_packages: Vec<_> = current_dbt_state
        .packages
        .iter()
        .filter(|p| !is_internal_package(p))
        .collect();

    if prev_packages.len() != current_packages.len() {
        return Err(fs_err!(
            ErrorCode::CacheError,
            "Number of packages changed: prev={} current={}",
            prev_packages.len(),
            current_packages.len()
        ));
    }

    let mut changed = Vec::new();

    let mut i = 0;
    while i < prev_packages.len() {
        let prev_package = &prev_packages[i];
        let current_package = &current_packages[i];

        // Packages must appear in the same order across runs (insertion order from loader).
        // Verify both identity and position so a reordering produces a clear message.
        if prev_package.package_root_path != current_package.package_root_path {
            return Err(fs_err!(
                ErrorCode::CacheError,
                "Package at index {i} changed: prev='{}' current='{}'",
                prev_package.dbt_project.name,
                current_package.dbt_project.name
            ));
        }

        let prev_fs_timestamps = prev_package.all_paths.clone();
        let mut new_fs_timestamps = current_package.all_paths.clone();

        let prev_fs_keys: HashSet<&ResourcePathKind> =
            HashSet::from_iter(prev_fs_timestamps.keys());
        let new_fs_keys = HashSet::from_iter(new_fs_timestamps.keys());
        if prev_fs_keys != new_fs_keys {
            return Err(fs_err!(
                ErrorCode::CacheError,
                "Resource kinds changed in package '{}': prev={:?} current={:?}",
                prev_package.dbt_project.name,
                prev_fs_keys,
                new_fs_keys
            ));
        }

        for (key, prev_value) in prev_fs_timestamps {
            token.check_cancellation()?;

            let is_doc = key == ResourcePathKind::DocsPaths;

            let current_value = new_fs_timestamps.get_mut(&key).unwrap();

            // When we check to see if any input files have changed,
            // we take into account case-sensitivity as the casing
            // should be reflected to the user.
            let prev_fs: HashMap<&Path, SystemTime> =
                prev_value.iter().map(|x| (x.0.as_path(), x.1)).collect();
            let current_fs: HashMap<&DbtPath, SystemTime> =
                current_value.iter().map(|x| (&x.0, x.1)).collect();

            if prev_fs.len() != current_fs.len() {
                return Err(fs_err!(
                    ErrorCode::CacheError,
                    "File count changed in package '{}' kind '{:?}': prev={} current={}",
                    prev_package.dbt_project.name,
                    key,
                    prev_fs.len(),
                    current_fs.len()
                ));
            }

            for current in current_fs.iter() {
                token.check_cancellation()?;
                // Using [std::path::Path] for the lookup here means
                // it will always be case-sensitive.
                let Some(prev_timestamp) = prev_fs.get(current.0.as_path()) else {
                    return Err(fs_err!(
                        ErrorCode::CacheError,
                        "File '{}' appeared in package '{}' (not in previous cache)",
                        current.0.display(),
                        current_package.dbt_project.name
                    ));
                };
                if current.1 != prev_timestamp {
                    if is_doc && !current.0.has_extension("md") {
                        // Ignore changes to non-md files if our path-kind is a doc.
                        continue;
                    }
                    if !is_resource_changeable(&key) {
                        return Err(fs_err!(
                            ErrorCode::CacheError,
                            "Non-incremental file changed: '{}' (kind {:?}) — full parse required",
                            current.0.display(),
                            key
                        ));
                    }
                    // Only root-package (i=0) model/analysis changes are safe for incremental.
                    // Changes in dependency packages require a full parse because their nodes
                    // may be referenced by the root and cannot be partially re-resolved.
                    if i > 0 {
                        return Err(fs_err!(
                            ErrorCode::CacheError,
                            "File '{}' changed in dependency package '{}' — full parse required",
                            current.0.display(),
                            current_package.dbt_project.name
                        ));
                    }
                    // The paths in all_paths are already absolute paths
                    // We need to make them relative to the package root path
                    // For the root package, this is the same as io.in_dir
                    let package_root = &current_dbt_state.packages[0].package_root_path;
                    let relative_path = current
                        .0
                        .get_relative_path(package_root)
                        .map(|p| p.to_str().unwrap_or_default().to_string())
                        .unwrap_or_else(|| current.0.to_str().unwrap_or_default().to_string());
                    changed.push(relative_path);
                }
            }
        }
        i += 1;
    }

    if changed.is_empty() {
        return Err(fs_err!(ErrorCode::NoFilesChanged, "No files changed"));
    }

    Ok(FileChangeset { changed })
}

#[allow(clippy::too_many_arguments)]
async fn try_load_cache_state_and_changeset_by_last_write(
    io: &IoArgs,
    prev_dbt_state: &DbtState,
    prev_resolved_state: &ResolverState,
    dbt_state: &DbtState,
    token: &CancellationToken,
) -> FsResult<Option<CacheState>> {
    // Incremental compilation - compute changeset and cache state
    let changeset = compute_file_changeset(prev_dbt_state, dbt_state, token).await?;

    let prev_resolved_state = PreviousResolvedState::from_resolved_state(prev_resolved_state);

    let cache = determine_cache_state_from_previous_previous_resolved_nodes(
        io,
        prev_resolved_state,
        &changeset.changed,
        dbt_state,
    )?;

    if let Some(cache) = cache {
        Ok(Some(cache))
    } else {
        Ok(None)
    }
}

/// Loads the cache state from previous resolved state (LSP/incremental path).
async fn load_cache(
    loaded_project: &DbtLoadedProject,
    _cache_command: FsCommand,
    io: &IoArgs,
    prev_resolved_state: Option<(&DbtLoadedProject, &ResolverState)>,
    token: &CancellationToken,
) -> FsResult<Option<CacheState>> {
    if let Some((prev_loaded_project, prev_resolved_state)) = prev_resolved_state {
        if !io.out_dir.exists() {
            return Err(fs_err!(
                ErrorCode::CacheError,
                "Target directory does not exist",
            ));
        }

        let prev_dbt_state = prev_loaded_project.dbt_state();
        if let Some(cache_state) = try_load_cache_state_and_changeset_by_last_write(
            io,
            &prev_dbt_state,
            prev_resolved_state,
            &loaded_project.dbt_state,
            token,
        )
        .await?
        {
            Ok(Some(cache_state))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}

impl DbtLoadedProject {
    pub async fn load(
        config: CompilationConfig,
        load_args: LoadArgs,
        invocation_args: Cow<'_, dbt_jinja_utils::invocation_args::InvocationArgs>,
        type_ops_factory: Arc<dyn TypeOpsFactory>,
        adapter_factory: Arc<dyn AdapterFactory>,
        prev_loaded_project: Option<&DbtLoadedProject>,
        tracing_config: Option<&dyn TracingConfigProvider>,
        token: &CancellationToken,
        loader_hooks: Arc<dyn LoaderHooks>,
        private_package_resolver: Arc<dyn PrivatePackageResolver>,
        jinja_factory: Arc<dyn JinjaFactory>,
    ) -> FsResult<DbtLoadedProject> {
        load_phase(
            config,
            load_args,
            invocation_args,
            type_ops_factory,
            adapter_factory,
            prev_loaded_project,
            tracing_config,
            token,
            loader_hooks,
            private_package_resolver,
            jinja_factory,
        )
        .await
    }

    pub fn config(&self) -> &CompilationConfig {
        &self.config
    }

    pub fn type_ops_factory(&self) -> &Arc<dyn TypeOpsFactory> {
        &self.type_ops_factory
    }

    pub fn dbt_state(&self) -> Arc<DbtState> {
        self.dbt_state.clone()
    }

    pub fn dbt_cloud_config(&self) -> Option<&ResolvedCloudConfig> {
        self.dbt_state.cloud_config.as_ref()
    }

    pub fn root_project(&self) -> &DbtProject {
        self.dbt_state.root_project()
    }

    pub fn root_project_name(&self) -> &str {
        self.dbt_state.root_project_name()
    }

    pub fn root_project_id(&self) -> String {
        self.dbt_state.root_project().get_project_id()
    }

    pub fn root_project_quoting(&self) -> DbtQuoting {
        let adapter_type = self.adapter_type();
        resolve_package_quoting(*self.dbt_state.root_project().quoting, adapter_type)
    }

    pub fn adapter_type(&self) -> AdapterType {
        self.dbt_state
            .dbt_profile
            .default_db_config()
            .adapter_type()
    }

    pub fn root_package(&self) -> &DbtPackage {
        self.dbt_state.root_package()
    }

    pub fn from_parts(
        config: CompilationConfig,
        type_ops_factory: Arc<dyn TypeOpsFactory>,
        adapter_factory: Arc<dyn AdapterFactory>,
        dbt_state: Arc<DbtState>,
        jinja_factory: Arc<dyn JinjaFactory>,
    ) -> Self {
        Self {
            config,
            type_ops_factory,
            adapter_factory,
            dbt_state,
            jinja_factory,
        }
    }

    pub fn create_jinja_env(
        &self,
        resolved_state: &ResolverState,
        io: &IoArgs,
        invocation_args: &dbt_jinja_utils::invocation_args::InvocationArgs,
        _token: &CancellationToken,
    ) -> FsResult<JinjaEnv> {
        let dbt_state = self.dbt_state();
        let root_project_name = self.root_project_name();
        let root_project_quoting = self.root_project_quoting();
        let macros = &resolved_state.macros;
        // The factory builds the env (and registers any extra functions it
        // provides) before it is used to render model SQL.
        self.jinja_factory.create_parse_jinja_environment(
            root_project_name,
            &dbt_state.dbt_profile.profile,
            &dbt_state.dbt_profile.target,
            self.adapter_type(),
            dbt_state.dbt_profile.default_db_config().clone(),
            dbt_state.dbt_profile.adapter_types(),
            root_project_quoting,
            build_macro_units(&macros.macros, &io.in_dir),
            dbt_state.vars.clone(),
            dbt_state.cli_vars.clone(),
            {
                // user_settings.yml is lowest-precedence; project flags override it
                let mut flags = dbt_schemas::schemas::UserSettings::load()
                    .flags
                    .map(dbt_schemas::schemas::serde::yml_value_to_minijinja_map)
                    .unwrap_or_default();
                flags.extend(dbt_state.root_project_flags());
                flags
            },
            dbt_state.run_started_at,
            invocation_args,
            dbt_state
                .packages
                .iter()
                .map(|p| p.dbt_project.name.clone())
                .collect(),
            io.clone(),
            dbt_state.catalogs.clone(),
        )
    }

    pub async fn load_cache(
        &self,
        cache_command: FsCommand,
        io: &IoArgs,
        prev_resolved_state: Option<(&DbtLoadedProject, &ResolverState)>,
        token: &CancellationToken,
    ) -> FsResult<Option<CacheState>> {
        load_cache(self, cache_command, io, prev_resolved_state, token).await
    }

    pub async fn resolve(
        &self,
        resolve_args: ResolveArgs,
        invocation_args: &dbt_jinja_utils::invocation_args::InvocationArgs,
        cache_state: Option<&CacheState>,
        token: &CancellationToken,
        jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
        resolver_hooks: Arc<dyn ResolverHooks>,
    ) -> FsResult<(ResolverState, Arc<JinjaEnv>)> {
        resolve_phase(
            self,
            resolve_args,
            invocation_args,
            cache_state,
            token,
            jinja_type_checking_event_listener_factory,
            resolver_hooks,
        )
        .await
    }

    /// The run's adapters, one per adapter type the target declares, built lazily.
    ///
    /// Every caller that used [`Self::init_adapter`] wants the default; a caller
    /// that knows a node's adapter should ask the store for that one instead. See
    /// [`AdapterStore`] for why the adapters are held by type rather than the run
    /// holding a single one.
    #[allow(clippy::too_many_arguments)]
    pub fn init_adapter_store(
        &self,
        resolved_state: &ResolverState,
        replay_mode: Option<ReplayMode>,
        jinja_env: &JinjaEnv,
        schema_store: Option<Arc<dyn SchemaStoreTrait>>,
        token: &CancellationToken,
        sidecar_client: Option<Arc<dyn SidecarClient>>,
        execute: Execute,
        connection_mode: AdapterConnectionMode,
    ) -> FsResult<Arc<AdapterStore>> {
        // Each declared adapter's `DbConfig`, cloned because the builder closure has
        // to be `'static` and so cannot hold `resolved_state`. Cloned rather than
        // *rendered* to a mapping here: rendering is fallible, and doing it up front
        // would fail a run over a malformed connection no node ever selects. It
        // happens inside the builder instead, so such an error arrives only if that
        // adapter is actually asked for.
        let configs: Vec<(AdapterType, DbConfig)> = resolved_state
            .dbt_profile
            .adapters
            .iter()
            .map(|(adapter_type, adapter)| (*adapter_type, adapter.config().clone()))
            .collect();
        let declared: Vec<AdapterType> = configs.iter().map(|(t, _)| *t).collect();
        let default_adapter = resolved_state.dbt_profile.default_adapter;

        let project = self.clone();
        let flags = Self::flags_from_env(jinja_env)?;
        // The *authored* `quoting:` block, not `resolved_state.root_project_quoting`.
        // The latter has already had its unset fields filled from the default
        // adapter's policy, so handing it to a non-default adapter of another type
        // silently applies the wrong one -- quote-everything to an adapter that
        // does not quote renders identifiers it then cannot find. Each adapter
        // resolves the same authored block against its own defaults instead, which
        // is what `resolve_package_quoting` is field-wise for.
        let authored_quoting = *self.dbt_state.root_project().quoting;
        let query_comment = resolved_state.runtime_config.inner.query_comment.clone();
        let cloud_config = self.dbt_cloud_config().cloned();
        // One budget for every adapter for now; per-adapter threads is a follow-up.
        let threads = resolved_state.dbt_profile.threads;
        let token = token.clone();

        let build: AdapterBuilder = Box::new(move |adapter_type| {
            let matched_config = &configs
                .iter()
                .find(|(t, _)| *t == adapter_type)
                .ok_or_else(|| {
                    fs_err!(
                        ErrorCode::Unexpected,
                        "the adapter store asked for '{adapter_type}', which it does not hold a config for"
                    )
                })?
                .1;
            // `lake_compute`, when configured as a secondary (non-default) adapter,
            // inherits `database`/`schema` from the default adapter instead of
            // requiring its own. `threads` is already shared across every adapter
            // via the `threads` parameter passed to `build_adapter` below, so it
            // needs no handling here. See fs#13653.
            let resolved_config = if let DbConfig::LakeCompute(lake_compute) = matched_config {
                let mut lake_compute = lake_compute.clone();
                if lake_compute.database.is_none() || lake_compute.schema.is_none() {
                    let default_config = configs
                        .iter()
                        .find(|(t, _)| *t == default_adapter)
                        .map(|(_, c)| c);
                    if lake_compute.database.is_none() {
                        lake_compute.database =
                            default_config.and_then(|c| c.get_database().cloned());
                    }
                    if lake_compute.schema.is_none() {
                        lake_compute.schema = default_config.and_then(|c| c.get_schema().cloned());
                    }
                }
                DbConfig::LakeCompute(lake_compute)
            } else if let DbConfig::Snowflake(snowflake) = matched_config {
                // dbt-auth has no notion of self_signed_jwt; the connection
                // it builds is identical to keypair's either way.
                let mut snowflake = snowflake.clone();
                if snowflake.method.as_deref() == Some("self_signed_jwt") {
                    snowflake.method = Some("keypair".to_string());
                }
                DbConfig::Snowflake(snowflake)
            } else {
                matched_config.clone()
            };
            let db_config = resolved_config.to_mapping().map_err(|e| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "could not read the '{adapter_type}' connection: {e}"
                )
            })?;
            let quoting = resolve_package_quoting(authored_quoting, adapter_type).try_into()?;
            project.build_adapter(
                adapter_type,
                db_config,
                replay_mode.clone(),
                flags.clone(),
                schema_store.clone(),
                &token,
                sidecar_client.clone(),
                execute,
                quoting,
                query_comment.clone(),
                cloud_config.clone(),
                threads,
                connection_mode,
            )
        });

        Ok(Arc::new(AdapterStore::new(
            declared,
            default_adapter,
            build,
        )?))
    }

    /// The target's default adapter.
    ///
    /// Kept as the single entry point it has always been; it now resolves through
    /// [`Self::init_adapter_store`], so a run that needs another adapter has one
    /// place to ask.
    #[allow(clippy::too_many_arguments)]
    pub fn init_adapter(
        &self,
        resolved_state: &ResolverState,
        replay_mode: Option<ReplayMode>,
        jinja_env: &JinjaEnv,
        schema_store: Option<Arc<dyn SchemaStoreTrait>>,
        token: &CancellationToken,
        sidecar_client: Option<Arc<dyn SidecarClient>>,
        execute: Execute,
        connection_mode: AdapterConnectionMode,
    ) -> FsResult<Arc<Adapter>> {
        self.init_adapter_store(
            resolved_state,
            replay_mode,
            jinja_env,
            schema_store,
            token,
            sidecar_client,
            execute,
            connection_mode,
        )?
        .default_adapter()
    }

    /// Pull `flags` out of the Jinja environment. Separated so the builder closure
    /// can own the result rather than the environment.
    fn flags_from_env(jinja_env: &JinjaEnv) -> FsResult<Arc<Flags>> {
        jinja_env
            .get_global("flags")
            .ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "There must be flags in the global variable",
                )
            })?
            .downcast_object::<Flags>()
            .ok_or_else(|| fs_err!(ErrorCode::InvalidConfig, "Could not downcast flags"))
    }

    /// Build one adapter. The body is what `init_adapter` used to be, with the
    /// adapter type and its config passed in rather than read from the default.
    #[allow(clippy::too_many_arguments)]
    fn build_adapter(
        &self,
        adapter_type: AdapterType,
        db_config: dbt_yaml::Mapping,
        replay_mode: Option<ReplayMode>,
        flags: Arc<Flags>,
        schema_store: Option<Arc<dyn SchemaStoreTrait>>,
        token: &CancellationToken,
        sidecar_client: Option<Arc<dyn SidecarClient>>,
        execute: Execute,
        root_project_quoting: ResolvedQuoting,
        query_comment: Option<QueryComment>,
        cloud_config: Option<ResolvedCloudConfig>,
        threads: Option<usize>,
        connection_mode: AdapterConnectionMode,
    ) -> FsResult<Arc<Adapter>> {
        let adapter_factory = self.adapter_factory.clone();
        let type_ops_factory = self.type_ops_factory.clone();

        let introspect_enabled = flags
            .to_dict()
            .get("introspect")
            .is_none_or(|value| value.is_true());

        // If executing locally, avoid creating a real remote engine entirely to guarantee
        // no network calls are made, even if adapter macros are accidentally invoked.
        // This applies to Local, Sidecar, and Service execution modes - all use local/runner execution.
        // This mode also applies to compile with --no-introspect
        // DuckDB is a local database — use the AdapterFactory for proper adapter creation
        // instead of a MockAdapter, so we get real query logging and telemetry.
        // Lake compute joins it for the same reason from the other direction: it is
        // an execution engine in its own right, and a node routed to it has no
        // fallback path, so a mock would silently do nothing rather than degrade.
        //
        // Under `--dbt-replay`, the mock/sidecar adapter below has no metadata
        // adapter, so unit-test `given` upstream schemas cannot resolve from the
        // recording. Route those runs through the factory so it builds a replay
        // adapter instead; sidecar execution still goes through the db_runner.
        let is_mantle_replay = matches!(&replay_mode, Some(ReplayMode::MantleReplay(_)));
        let executes_locally =
            !introspect_enabled || matches!(execute, Execute::Sidecar | Execute::Service);
        let offline = connection_mode == AdapterConnectionMode::Offline;
        let use_local_mock_adapter = offline || (executes_locally && !is_mantle_replay);
        let adapter = if matches!(adapter_type, AdapterType::DuckDB | AdapterType::LakeCompute) {
            adapter_factory
                .create_adapter(
                    adapter_type,
                    db_config,
                    type_ops_factory,
                    replay_mode,
                    flags.project_flags(),
                    schema_store,
                    root_project_quoting,
                    query_comment,
                    token.clone(),
                    cloud_config.as_ref(),
                    threads,
                )
                .map_err(|e| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "Could not create DuckDB adapter: {}",
                        e
                    )
                })?
        } else if use_local_mock_adapter {
            if !offline && let Some(client) = sidecar_client {
                // For sidecar/service mode with a sidecar client, use AdapterImpl
                // wrapping a SidecarEngine which routes introspection (get_columns_in_relation,
                // list_relations, get_relation) to the sidecar client.
                let sidecar_engine = SidecarEngine::new(
                    adapter_type,
                    Backend::DuckDBExtended,
                    client,
                    root_project_quoting,
                    AdapterConfig::default(),
                    type_ops_factory.create(adapter_type),
                    adapter_factory.stmt_splitter(),
                    QueryCommentConfig::from_query_comment(None, adapter_type, false, None),
                    Arc::new(RelationCache::default()),
                );
                let adapter_impl = AdapterImpl::new(
                    Arc::new(sidecar_engine) as Arc<dyn AdapterEngine>,
                    schema_store,
                );
                Arc::new(Adapter::new(Arc::new(adapter_impl), None, token.clone()))
            } else {
                let type_ops = type_ops_factory.create(adapter_type);
                let mut mock_flags = flags.project_flags();
                if offline {
                    mock_flags.insert("introspect".to_string(), false.into());
                }
                let mock = AdapterImpl::new_mock(
                    adapter_type,
                    mock_flags,
                    root_project_quoting,
                    type_ops,
                    adapter_factory.stmt_splitter(),
                );
                Arc::new(Adapter::new(Arc::new(mock), None, token.clone()))
            }
        } else {
            adapter_factory
                .create_adapter(
                    adapter_type,
                    db_config,
                    Arc::clone(&type_ops_factory),
                    replay_mode,
                    flags.project_flags(),
                    schema_store,
                    root_project_quoting,
                    query_comment,
                    token.clone(),
                    cloud_config.as_ref(),
                    threads,
                )
                .map_err(|e| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "Failed to initialize adapter: {}",
                        e
                    )
                })?
        };

        Ok(adapter)
    }

    pub fn init_base_adapter(
        &self,
        adapter_type: AdapterType,
        config_as_mapping: dbt_yaml::Mapping,
        replay_mode: Option<ReplayMode>,
        token: CancellationToken,
    ) -> FsResult<Arc<Adapter>> {
        let type_ops_factory = self.type_ops_factory.clone();

        self.adapter_factory.create_adapter(
            adapter_type,
            config_as_mapping,
            type_ops_factory,
            replay_mode,
            BTreeMap::new(),
            None,
            DEFAULT_RESOLVED_QUOTING,
            None,
            token,
            None, // cloud_config — debug only runs `select 1`, no cloud connection needed
            None, // threads
        )
    }
}
