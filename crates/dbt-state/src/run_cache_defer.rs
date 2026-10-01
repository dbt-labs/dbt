//! dbt State auto-deferral: synthesizes profile-target defer nodes from the
//! dbt State service config when no manifest-backed defer state is in play.
//!
//! The synthesized nodes mimic previous-state defer nodes — cloned from the
//! current project and rewritten as if resolved against the configured
//! `defer_to_target` profile target — so the standard defer pipeline can treat
//! them identically.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use crate::{
    proto::query_cache::ResolveDeferredRelationsRequest,
    service_client::SharedRunCacheServiceClient, service_config::RunCacheServiceConfig,
};
use dbt_adapter::AdapterType;
use dbt_common::{
    ErrorCode, FsResult,
    adapter::dialect_of,
    fs_err,
    io_args::{EvalArgs, FsCommand},
    tracing::dbt_emit::{emit_trace_log_message, emit_warn_log_message},
    warn_error_options::WarnErrorOptions,
};
use dbt_frontend_common::{Dialect, FullyQualifiedName};
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv, phases::build_operation_context_btreemap, register_base_functions,
};
use dbt_parser::utils::{RelationComponents, update_node_relation_components};
use dbt_profile::{
    ProfileEnvironment, ProfileError, ResolvedProfile, find_profiles_path, resolve_with_env,
};
use dbt_schemas::{
    schemas::{
        DbtFunction, DbtModel, DbtSeed, DbtSnapshot, InternalDbtNodeAttributes, Nodes,
        common::DbtMaterialization,
        profiles::{DbConfig, Execute, TargetContext, canonicalize_bigquery_region_alias},
        serde::yaml_to_fs_error,
    },
    state::{DbtProfile, ResolverState},
};

use minijinja::Value;

pub struct RunCacheProfileResolver;

impl RunCacheProfileResolver {
    /// Builds manifest-like defer nodes for dbt State auto-deferral without
    /// loading a prior manifest artifact.
    ///
    /// The returned nodes are cloned from the current project and rewritten as
    /// if they were resolved against the configured `defer_to_target` profile
    /// target.
    /// Existing defer machinery can then treat them like previous-state nodes
    /// when resolving unselected upstreams.
    pub async fn synthesize_defer_nodes(
        arg: &EvalArgs,
        resolved_state: &ResolverState,
        jinja_env: &JinjaEnv,
        unselected_node_ids: &BTreeSet<String>,
        run_cache_client: Option<SharedRunCacheServiceClient>,
    ) -> FsResult<Option<Nodes>> {
        let Some(auto_defer) = run_cache_auto_defer_config(arg, &resolved_state.dbt_profile) else {
            return Ok(None);
        };

        let db_config = match resolve_run_cache_defer_target_profile(
            arg,
            &resolved_state.dbt_profile.profile,
            &auto_defer.defer_to_target,
        ) {
            Ok(db_config) => db_config,
            Err(ProfileError::Yaml { source, path }) => {
                return Err(yaml_to_fs_error(source, Some(&path)));
            }
            Err(err) => {
                let defer_to = &auto_defer.defer_to_target;
                emit_trace_log_message(|| {
                    format!("dbt State auto-deferral could not resolve target '{defer_to}': {err}")
                });
                return Ok(None);
            }
        };

        let defer_jinja_env = jinja_env_for_run_cache_target(
            jinja_env,
            &resolved_state.dbt_profile.profile,
            &auto_defer.defer_to_target,
            db_config.clone(),
        )?;

        let defer_base_context = run_cache_defer_base_context(resolved_state, jinja_env);
        let mut defer_nodes = resolved_state.nodes.deep_clone();

        // Compile must keep profile-resolved relations. State FQNs apply to
        // non-compile commands only.
        let state_resolved_ids = if arg.command == FsCommand::Compile {
            BTreeSet::new()
        } else {
            Self::get_state_resolved_ids(
                run_cache_client,
                unselected_node_ids,
                &auto_defer,
                resolved_state.adapter_type,
                &resolved_state.dbt_profile.profile,
                &resolved_state.root_project_name,
                resolved_state
                    .cloud_config
                    .as_ref()
                    .and_then(|c| c.project_id.as_deref()),
                &mut defer_nodes,
            )
            .await
        };

        update_run_cache_defer_nodes(
            &mut defer_nodes,
            &defer_jinja_env,
            &defer_base_context,
            resolved_state,
            &db_config,
            Some(&state_resolved_ids),
        )?;

        Ok(Some(defer_nodes))
    }

    async fn get_state_resolved_ids(
        run_cache_client: Option<SharedRunCacheServiceClient>,
        unselected_node_ids: &BTreeSet<String>,
        auto_defer: &RunCacheAutoDeferConfig,
        adapter_type: AdapterType,
        profile_name: &str,
        project_name: &str,
        project_id: Option<&str>,
        defer_nodes: &mut Nodes,
    ) -> BTreeSet<String> {
        let Some(dialect) = dialect_of(adapter_type) else {
            return BTreeSet::new();
        };

        match Self::resolve_defer_nodes_via_state(
            run_cache_client,
            unselected_node_ids,
            auto_defer,
            adapter_type,
            profile_name,
            project_name,
            project_id,
        )
        .await
        {
            Some(fqn_by_unique_id) => {
                Self::patch_nodes_with_state_resolved_fqns(defer_nodes, dialect, &fqn_by_unique_id)
            }
            None => BTreeSet::new(),
        }
    }

    async fn resolve_defer_nodes_via_state(
        run_cache_client: Option<SharedRunCacheServiceClient>,
        node_unique_ids: &BTreeSet<String>,
        auto_defer: &RunCacheAutoDeferConfig,
        adapter_type: AdapterType,
        profile_name: &str,
        project_name: &str,
        project_id: Option<&str>,
    ) -> Option<HashMap<String, String>> {
        let run_cache_client = run_cache_client?;

        dialect_of(adapter_type)?;

        if node_unique_ids.is_empty() {
            return None;
        }

        let request = ResolveDeferredRelationsRequest {
            profile_name: profile_name.to_string(),
            target_name: auto_defer.defer_to_target.clone(),
            project_name: project_name.to_string(),
            project_id: project_id.map(str::to_string),
            node_unique_ids: node_unique_ids.iter().cloned().collect(),
        };

        match run_cache_client.resolve_deferred_relations(request).await {
            Ok(fqn_by_unique_id) => Some(fqn_by_unique_id),
            Err(err) => {
                emit_trace_log_message(|| {
                    format!(
                        "dbt State auto-deferral state-backed relation resolution failed: {err}; falling back to rendered defer targets"
                    )
                });
                None
            }
        }
    }

    fn patch_nodes_with_state_resolved_fqns(
        defer_nodes: &mut Nodes,
        dialect: Dialect,
        fqn_by_unique_id: &HashMap<String, String>,
    ) -> BTreeSet<String> {
        let mut resolved_unique_ids = BTreeSet::new();

        for (unique_id, raw_fqn) in fqn_by_unique_id {
            let parsed_fqn = match FullyQualifiedName::parse(raw_fqn, dialect) {
                Ok(parsed_fqn) => parsed_fqn,
                Err(err) => {
                    emit_trace_log_message(|| {
                        format!(
                            "dbt State auto-deferral could not parse target relation '{raw_fqn}' \
                            for '{unique_id}': {err}; falling back to rendered defer target"
                        )
                    });
                    continue;
                }
            };

            if let Some(model) = defer_nodes.models.get_mut(unique_id) {
                let model = Arc::make_mut(model);
                Self::patch_node_base_from_fqn(model, &parsed_fqn, raw_fqn);
            } else if let Some(seed) = defer_nodes.seeds.get_mut(unique_id) {
                let seed = Arc::make_mut(seed);
                Self::patch_node_base_from_fqn(seed, &parsed_fqn, raw_fqn);
            } else if let Some(snapshot) = defer_nodes.snapshots.get_mut(unique_id) {
                let snapshot = Arc::make_mut(snapshot);
                Self::patch_node_base_from_fqn(snapshot, &parsed_fqn, raw_fqn);
            } else {
                continue;
            }

            resolved_unique_ids.insert(unique_id.clone());
        }

        resolved_unique_ids
    }

    fn patch_node_base_from_fqn(
        node: &mut dyn InternalDbtNodeAttributes,
        parsed_fqn: &FullyQualifiedName,
        raw_fqn: &str,
    ) {
        let base = node.base_mut();
        base.database = parsed_fqn.catalog().as_str().to_string();
        base.schema = parsed_fqn.schema().as_str().to_string();
        base.alias = parsed_fqn.table().as_str().to_string();
        base.relation_name = Some(raw_fqn.to_string());
    }
}

#[derive(Debug, PartialEq, Eq)]
struct RunCacheAutoDeferConfig {
    defer_to_target: String,
}

fn run_cache_auto_defer_config(
    arg: &EvalArgs,
    active_profile: &DbtProfile,
) -> Option<RunCacheAutoDeferConfig> {
    if !run_cache_auto_defer_requested(arg) {
        return None;
    }

    let config = match RunCacheServiceConfig::from_env() {
        Ok(config) => config,
        Err(err) => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State auto-deferral config failed: {err}; continuing without synthesized defer state"
                ),
            );
            return None;
        }
    };

    if !config.enabled || config.is_defer_to_target(active_profile) {
        return None;
    }

    Some(RunCacheAutoDeferConfig {
        defer_to_target: config.defer_to_target(active_profile),
    })
}

fn run_cache_auto_defer_requested(arg: &EvalArgs) -> bool {
    if !run_cache_auto_defer_command(arg.command) {
        return false;
    }
    if Execute::from_compute_flag(arg.local_execution_backend) != Execute::Remote {
        return false;
    }

    // Respect an explicit --no-defer. The clap layer defaults EvalArgs.defer
    // to true when no flag is passed, so `arg.defer == true` cannot tell us
    // the user opted in — but `arg.defer == false` only ever comes from
    // --no-defer, so it is a reliable opt-out signal.
    if !arg.defer {
        return false;
    }
    // `run_cache_service` already folds in DBT_ENGINE_MANAGE_STATE; consulting the
    // environment again here would let the env var dbt platform injects on
    // production deployments override an explicit `--no-manage-state`.
    arg.run_cache_service
}

fn run_cache_auto_defer_command(command: FsCommand) -> bool {
    matches!(
        command,
        FsCommand::Compile
            | FsCommand::Run
            | FsCommand::Build
            | FsCommand::Test
            | FsCommand::Seed
            | FsCommand::Snapshot
            | FsCommand::Show
    )
}

fn resolve_run_cache_defer_target_profile(
    arg: &EvalArgs,
    profile_name: &str,
    defer_to_target: &str,
) -> Result<DbConfig, ProfileError> {
    let profile_path = find_profiles_path(arg.profiles_dir.as_deref())?;
    let mut penv = ProfileEnvironment::new(arg.vars.clone());
    register_base_functions(&mut penv.env, WarnErrorOptions::default());
    let resolved: ResolvedProfile =
        resolve_with_env(&penv, &profile_path, profile_name, Some(defer_to_target))?;

    let mut credentials = resolved.credentials;
    canonicalize_bigquery_region_alias(&mut credentials);
    let credentials_value = dbt_yaml::Value::Mapping(credentials, dbt_yaml::Span::default());
    dbt_yaml::from_value(credentials_value).map_err(|source| ProfileError::Yaml {
        source,
        path: profile_path,
    })
}

fn jinja_env_for_run_cache_target(
    jinja_env: &JinjaEnv,
    profile_name: &str,
    target_name: &str,
    db_config: DbConfig,
) -> FsResult<JinjaEnv> {
    if db_config.has_removed_execute_field() {
        emit_warn_log_message(
            ErrorCode::DeprecatedOption,
            "The `execute:` field in profiles.yml is no longer supported and will be ignored. \
             Use the `--compute inline|sidecar|service|remote` CLI flag instead. \
             Please remove `execute:` from your profile.",
        );
    }

    let database = db_config.get_database().cloned();
    let schema = db_config.get_schema().cloned();
    let target_context = TargetContext::try_from(db_config)
        .map_err(|e| fs_err!(ErrorCode::InvalidConfig, "{}", e))?;
    let target_context = run_cache_target_context_map(profile_name, target_name, target_context);
    let target_context = Arc::new(target_context);

    let mut defer_jinja_env = jinja_env.clone();
    defer_jinja_env
        .env
        .add_global("target", Value::from_serialize(target_context.clone()));
    defer_jinja_env
        .env
        .add_global("env", Value::from_serialize(target_context));
    defer_jinja_env
        .env
        .add_global("database", Value::from(database));
    defer_jinja_env
        .env
        .add_global("schema", Value::from(schema));
    Ok(defer_jinja_env)
}

fn run_cache_target_context_map(
    profile_name: &str,
    target_name: &str,
    target_context: TargetContext,
) -> BTreeMap<String, Value> {
    let target_context_value =
        dbt_yaml::to_value(&target_context).expect("TargetContext should serialize to YAML");
    let mut target_context_map: BTreeMap<String, Value> =
        dbt_yaml::from_value(target_context_value).expect("TargetContext should convert to Jinja");
    target_context_map.insert("profile_name".to_string(), Value::from(profile_name));
    target_context_map.insert("name".to_string(), Value::from(target_name));
    target_context_map.insert("target_name".to_string(), Value::from(target_name));
    target_context_map
}

fn run_cache_defer_base_context(
    resolved_state: &ResolverState,
    jinja_env: &JinjaEnv,
) -> BTreeMap<String, Value> {
    let namespace_keys: Vec<String> = jinja_env
        .env
        .get_macro_namespace_registry()
        .map(|registry| registry.keys().map(|key| key.to_string()).collect())
        .unwrap_or_default();

    build_operation_context_btreemap(
        resolved_state.node_resolver.clone(),
        &resolved_state.root_project_name,
        &resolved_state.nodes,
        resolved_state.defer_nodes.as_ref(),
        resolved_state.runtime_config.clone(),
        namespace_keys,
        None,
    )
}

/// Rewrites cloned current-project nodes so they can stand in for
/// previous-state defer nodes at the dbt State `defer_to` target.
///
/// Each cacheable node gets relation components rendered with the target
/// profile's database/schema/Jinja context, which lets standard defer handling
/// resolve unselected upstreams to that target. Ephemeral and inline models are
/// removed because they do not materialize to warehouse relations.
fn update_run_cache_defer_nodes(
    defer_nodes: &mut Nodes,
    jinja_env: &JinjaEnv,
    base_context: &BTreeMap<String, Value>,
    resolved_state: &ResolverState,
    db_config: &DbConfig,
    exclude_unique_ids: Option<&BTreeSet<String>>,
) -> FsResult<()> {
    let default_database = db_config.get_database_or_default();
    let default_schema = db_config
        .get_schema()
        .map(String::as_str)
        .unwrap_or("public")
        .to_string();

    for (unique_id, model) in defer_nodes.models.iter_mut() {
        if !should_update_defer_node(unique_id, exclude_unique_ids) {
            continue;
        }
        update_run_cache_defer_model(
            Arc::make_mut(model),
            jinja_env,
            base_context,
            resolved_state,
            &default_database,
            &default_schema,
        )?;
    }
    for (unique_id, seed) in defer_nodes.seeds.iter_mut() {
        if !should_update_defer_node(unique_id, exclude_unique_ids) {
            continue;
        }
        update_run_cache_defer_seed(
            Arc::make_mut(seed),
            jinja_env,
            base_context,
            resolved_state,
            &default_database,
            &default_schema,
        )?;
    }
    for (unique_id, snapshot) in defer_nodes.snapshots.iter_mut() {
        if !should_update_defer_node(unique_id, exclude_unique_ids) {
            continue;
        }
        update_run_cache_defer_snapshot(
            Arc::make_mut(snapshot),
            jinja_env,
            base_context,
            resolved_state,
            &default_database,
            &default_schema,
        )?;
    }
    for (unique_id, function) in defer_nodes.functions.iter_mut() {
        if !should_update_defer_node(unique_id, exclude_unique_ids) {
            continue;
        }
        update_run_cache_defer_function(
            Arc::make_mut(function),
            jinja_env,
            base_context,
            resolved_state,
            &default_database,
            &default_schema,
        )?;
    }

    defer_nodes.models.retain(|_, model| {
        model.__base_attr__.materialized != DbtMaterialization::Ephemeral
            && model.__base_attr__.materialized != DbtMaterialization::Inline
    });
    Ok(())
}

fn update_run_cache_defer_model(
    model: &mut DbtModel,
    jinja_env: &JinjaEnv,
    base_context: &BTreeMap<String, Value>,
    resolved_state: &ResolverState,
    default_database: &str,
    default_schema: &str,
) -> FsResult<()> {
    set_run_cache_default_relation_target(model, default_database, default_schema);
    let components = RelationComponents {
        database: model
            .deprecated_config
            .database
            .clone()
            .into_inner()
            .unwrap_or(None),
        schema: model
            .deprecated_config
            .schema
            .clone()
            .into_inner()
            .unwrap_or(None),
        alias: model.deprecated_config.alias.clone(),
        store_failures: None,
    };
    let package_name = model.__common_attr__.package_name.clone();
    update_node_relation_components(
        model,
        jinja_env,
        &resolved_state.root_project_name,
        &package_name,
        base_context,
        &components,
        resolved_state.adapter_type,
    )
}

fn update_run_cache_defer_seed(
    seed: &mut DbtSeed,
    jinja_env: &JinjaEnv,
    base_context: &BTreeMap<String, Value>,
    resolved_state: &ResolverState,
    default_database: &str,
    default_schema: &str,
) -> FsResult<()> {
    set_run_cache_default_relation_target(seed, default_database, default_schema);
    let components = RelationComponents {
        database: seed.deprecated_config.database.clone(),
        schema: seed.deprecated_config.schema.clone(),
        alias: seed.deprecated_config.alias.clone(),
        store_failures: None,
    };
    let package_name = seed.__common_attr__.package_name.clone();
    update_node_relation_components(
        seed,
        jinja_env,
        &resolved_state.root_project_name,
        &package_name,
        base_context,
        &components,
        resolved_state.adapter_type,
    )
}

fn update_run_cache_defer_snapshot(
    snapshot: &mut DbtSnapshot,
    jinja_env: &JinjaEnv,
    base_context: &BTreeMap<String, Value>,
    resolved_state: &ResolverState,
    default_database: &str,
    default_schema: &str,
) -> FsResult<()> {
    set_run_cache_default_relation_target(snapshot, default_database, default_schema);
    let components = RelationComponents {
        database: snapshot
            .deprecated_config
            .target_database
            .clone()
            .or_else(|| snapshot.deprecated_config.database.clone()),
        schema: snapshot
            .deprecated_config
            .target_schema
            .clone()
            .or_else(|| snapshot.deprecated_config.schema.clone()),
        alias: snapshot.deprecated_config.alias.clone(),
        store_failures: None,
    };
    let package_name = snapshot.__common_attr__.package_name.clone();
    update_node_relation_components(
        snapshot,
        jinja_env,
        &resolved_state.root_project_name,
        &package_name,
        base_context,
        &components,
        resolved_state.adapter_type,
    )
}

fn update_run_cache_defer_function(
    function: &mut DbtFunction,
    jinja_env: &JinjaEnv,
    base_context: &BTreeMap<String, Value>,
    resolved_state: &ResolverState,
    default_database: &str,
    default_schema: &str,
) -> FsResult<()> {
    set_run_cache_default_relation_target(function, default_database, default_schema);
    let components = RelationComponents {
        database: function
            .deprecated_config
            .database
            .clone()
            .into_inner()
            .unwrap_or(None),
        schema: function
            .deprecated_config
            .schema
            .clone()
            .into_inner()
            .unwrap_or(None),
        alias: function.deprecated_config.alias.clone(),
        store_failures: None,
    };
    let package_name = function.__common_attr__.package_name.clone();
    update_node_relation_components(
        function,
        jinja_env,
        &resolved_state.root_project_name,
        &package_name,
        base_context,
        &components,
        resolved_state.adapter_type,
    )
}

fn set_run_cache_default_relation_target(
    node: &mut dyn InternalDbtNodeAttributes,
    default_database: &str,
    default_schema: &str,
) {
    let base = node.base_mut();
    base.database = default_database.to_string();
    base.schema = default_schema.to_string();
}

fn should_update_defer_node(
    unique_id: &str,
    exclude_unique_ids: Option<&BTreeSet<String>>,
) -> bool {
    !exclude_unique_ids.is_some_and(|ids| ids.contains(unique_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::query_cache::{
        ConfirmExecutionRequest, ConfirmExecutionResponse, SubmitEnrichedSqlRequest,
        SubmitSqlResponse, SubmitValuesRequest,
    };
    use crate::service_client::{ClientVersionStatus, RunCacheServiceClient, RunCacheServiceError};
    use async_trait::async_trait;
    use dbt_schemas::schemas::CommonAttributes;

    enum ResolveOutcome {
        Ok(HashMap<String, String>),
        Err,
    }

    struct MockResolveClient(ResolveOutcome);

    #[async_trait]
    impl RunCacheServiceClient for MockResolveClient {
        async fn validate_client_version(
            &self,
        ) -> Result<ClientVersionStatus, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn submit_enriched_sql(
            &self,
            _request: SubmitEnrichedSqlRequest,
        ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn submit_values(
            &self,
            _request: SubmitValuesRequest,
        ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn confirm_execution(
            &self,
            _request: ConfirmExecutionRequest,
        ) -> Result<ConfirmExecutionResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn resolve_deferred_relations(
            &self,
            _request: ResolveDeferredRelationsRequest,
        ) -> Result<HashMap<String, String>, RunCacheServiceError> {
            match &self.0 {
                ResolveOutcome::Ok(map) => Ok(map.clone()),
                ResolveOutcome::Err => Err(RunCacheServiceError::Disabled),
            }
        }
    }

    fn auto_defer_config() -> RunCacheAutoDeferConfig {
        RunCacheAutoDeferConfig {
            defer_to_target: "prod".to_string(),
        }
    }

    fn base_attr_with_old_alias() -> dbt_schemas::schemas::NodeBaseAttributes {
        dbt_schemas::schemas::NodeBaseAttributes {
            database: "dev_db".to_string(),
            schema: "dev_schema".to_string(),
            alias: "old_alias".to_string(),
            relation_name: Some("dev_db.dev_schema.old_alias".to_string()),
            ..Default::default()
        }
    }

    fn model_with_unique_id(unique_id: &str) -> Arc<DbtModel> {
        Arc::new(DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: unique_id.to_string(),
                ..Default::default()
            },
            __base_attr__: base_attr_with_old_alias(),
            ..Default::default()
        })
    }

    fn seed_with_unique_id(unique_id: &str) -> Arc<DbtSeed> {
        Arc::new(DbtSeed {
            __common_attr__: CommonAttributes {
                unique_id: unique_id.to_string(),
                ..Default::default()
            },
            __base_attr__: base_attr_with_old_alias(),
            ..Default::default()
        })
    }

    fn snapshot_with_unique_id(unique_id: &str) -> Arc<DbtSnapshot> {
        Arc::new(DbtSnapshot {
            __common_attr__: CommonAttributes {
                unique_id: unique_id.to_string(),
                ..Default::default()
            },
            __base_attr__: base_attr_with_old_alias(),
            ..Default::default()
        })
    }

    #[test]
    fn test_patch_nodes_with_state_resolved_fqns_updates_alias() {
        let unresolved_model_id = "model.test.unresolved_model".to_string();
        let resolved_model_id = "model.test.resolved_model".to_string();
        let resolved_seed_id = "seed.test.resolved_seed".to_string();
        let resolved_snapshot_id = "snapshot.test.resolved_snapshot".to_string();

        let mut defer_nodes = Nodes::default();
        defer_nodes.models.insert(
            unresolved_model_id.clone(),
            model_with_unique_id(&unresolved_model_id),
        );
        defer_nodes.models.insert(
            resolved_model_id.clone(),
            model_with_unique_id(&resolved_model_id),
        );
        defer_nodes.seeds.insert(
            resolved_seed_id.clone(),
            seed_with_unique_id(&resolved_seed_id),
        );
        defer_nodes.snapshots.insert(
            resolved_snapshot_id.clone(),
            snapshot_with_unique_id(&resolved_snapshot_id),
        );

        let fqn_by_unique_id = HashMap::from([
            (
                resolved_model_id.clone(),
                "prod_db.prod_schema.resolved_model".to_string(),
            ),
            (
                resolved_seed_id.clone(),
                "prod_db.prod_schema.resolved_seed".to_string(),
            ),
            (
                resolved_snapshot_id.clone(),
                "prod_db.prod_schema.resolved_snapshot".to_string(),
            ),
        ]);

        let resolved = RunCacheProfileResolver::patch_nodes_with_state_resolved_fqns(
            &mut defer_nodes,
            Dialect::Snowflake,
            &fqn_by_unique_id,
        );

        assert_eq!(
            resolved,
            BTreeSet::from([
                resolved_model_id.clone(),
                resolved_seed_id.clone(),
                resolved_snapshot_id.clone(),
            ])
        );

        let patched_model = defer_nodes.models.get(&resolved_model_id).unwrap();
        assert_eq!(patched_model.__base_attr__.database, "PROD_DB");
        assert_eq!(patched_model.__base_attr__.schema, "PROD_SCHEMA");
        assert_eq!(patched_model.__base_attr__.alias, "RESOLVED_MODEL");
        assert_eq!(
            patched_model.__base_attr__.relation_name,
            Some("prod_db.prod_schema.resolved_model".to_string())
        );

        let patched_seed = defer_nodes.seeds.get(&resolved_seed_id).unwrap();
        assert_eq!(patched_seed.__base_attr__.database, "PROD_DB");
        assert_eq!(patched_seed.__base_attr__.schema, "PROD_SCHEMA");
        assert_eq!(patched_seed.__base_attr__.alias, "RESOLVED_SEED");
        assert_eq!(
            patched_seed.__base_attr__.relation_name,
            Some("prod_db.prod_schema.resolved_seed".to_string())
        );

        let patched_snapshot = defer_nodes.snapshots.get(&resolved_snapshot_id).unwrap();
        assert_eq!(patched_snapshot.__base_attr__.database, "PROD_DB");
        assert_eq!(patched_snapshot.__base_attr__.schema, "PROD_SCHEMA");
        assert_eq!(patched_snapshot.__base_attr__.alias, "RESOLVED_SNAPSHOT");
        assert_eq!(
            patched_snapshot.__base_attr__.relation_name,
            Some("prod_db.prod_schema.resolved_snapshot".to_string())
        );

        // No FQN was returned for this id
        let unresolved_model = defer_nodes.models.get(&unresolved_model_id).unwrap();
        assert_eq!(unresolved_model.__base_attr__.database, "dev_db");
        assert_eq!(unresolved_model.__base_attr__.alias, "old_alias");
    }

    #[test]
    fn test_patch_nodes_with_state_resolved_fqns_unparseable() {
        let unparseable_model_id = "model.test.bad_fqn".to_string();
        let ok_model_id = "model.test.ok".to_string();

        let mut defer_nodes = Nodes::default();
        defer_nodes.models.insert(
            unparseable_model_id.clone(),
            model_with_unique_id(&unparseable_model_id),
        );
        defer_nodes
            .models
            .insert(ok_model_id.clone(), model_with_unique_id(&ok_model_id));

        let fqn_by_unique_id = HashMap::from([
            // no catalog/schema component -> fails FullyQualifiedName::parse
            (
                unparseable_model_id.clone(),
                "not_a_full_relation".to_string(),
            ),
            (ok_model_id.clone(), "prod_db.prod_schema.ok".to_string()),
        ]);
        let result = RunCacheProfileResolver::patch_nodes_with_state_resolved_fqns(
            &mut defer_nodes,
            Dialect::Snowflake,
            &fqn_by_unique_id,
        );

        assert_eq!(result, BTreeSet::from([ok_model_id.clone()]));

        // valid entry patched
        let ok_model = defer_nodes.models.get(&ok_model_id).unwrap();
        assert_eq!(ok_model.__base_attr__.database, "PROD_DB");
        assert_eq!(ok_model.__base_attr__.alias, "OK");

        // unparseable entry not patched
        let unparseable_model = defer_nodes.models.get(&unparseable_model_id).unwrap();
        assert_eq!(unparseable_model.__base_attr__.database, "dev_db");
        assert_eq!(unparseable_model.__base_attr__.alias, "old_alias");
    }

    #[test]
    fn test_should_update_defer_node_filters_ids() {
        let exclude = BTreeSet::from(["model.test.state_resolved".to_string()]);
        assert!(!should_update_defer_node(
            "model.test.state_resolved",
            Some(&exclude)
        ));
        assert!(should_update_defer_node(
            "model.test.manual_fallback",
            Some(&exclude)
        ));
        assert!(should_update_defer_node("model.test.manual_fallback", None));
    }

    #[dbt_runtime::test]
    async fn test_resolve_defer_nodes_via_state_returns_mapping_on_success() {
        let expected = HashMap::from([(
            "model.test.a".to_string(),
            "prod_db.prod_schema.a".to_string(),
        )]);
        let client: SharedRunCacheServiceClient =
            Arc::new(MockResolveClient(ResolveOutcome::Ok(expected.clone())));
        let node_ids = BTreeSet::from(["model.test.a".to_string()]);

        let result = RunCacheProfileResolver::resolve_defer_nodes_via_state(
            Some(client),
            &node_ids,
            &auto_defer_config(),
            AdapterType::Snowflake,
            "profile",
            "project",
            Some("proj-123"),
        )
        .await;

        assert_eq!(result, Some(expected));
    }

    #[dbt_runtime::test]
    async fn test_resolve_defer_nodes_via_state_returns_none_empty_node_ids() {
        let client: SharedRunCacheServiceClient =
            Arc::new(MockResolveClient(ResolveOutcome::Ok(HashMap::new())));
        let node_ids = BTreeSet::new();
        let result = RunCacheProfileResolver::resolve_defer_nodes_via_state(
            Some(client),
            &node_ids,
            &auto_defer_config(),
            AdapterType::Snowflake,
            "profile",
            "project",
            Some("proj-123"),
        )
        .await;

        assert!(result.is_none());
    }

    #[dbt_runtime::test]
    async fn test_resolve_defer_nodes_via_state_fallsback_on_client_error() {
        let client: SharedRunCacheServiceClient = Arc::new(MockResolveClient(ResolveOutcome::Err));
        let node_ids = BTreeSet::from(["model.test.a".to_string()]);

        let result = RunCacheProfileResolver::resolve_defer_nodes_via_state(
            Some(client),
            &node_ids,
            &auto_defer_config(),
            AdapterType::Snowflake,
            "profile",
            "project",
            Some("proj-123"),
        )
        .await;

        assert!(result.is_none());
    }

    #[dbt_runtime::test]
    async fn test_get_state_resolved_ids_skips_unsupported_dialect() {
        let model_id = "model.test.a".to_string();
        let mut defer_nodes = Nodes::default();
        defer_nodes
            .models
            .insert(model_id.clone(), model_with_unique_id(&model_id));
        let client: SharedRunCacheServiceClient = Arc::new(MockResolveClient(ResolveOutcome::Ok(
            HashMap::from([(model_id.clone(), "prod_db.prod_schema.a".to_string())]),
        )));
        let node_ids = BTreeSet::from([model_id.clone()]);

        let result = RunCacheProfileResolver::get_state_resolved_ids(
            Some(client),
            &node_ids,
            &auto_defer_config(),
            AdapterType::Fabric,
            "profile",
            "project",
            Some("proj-123"),
            &mut defer_nodes,
        )
        .await;

        assert!(result.is_empty());
        // node left un-patched
        let model = defer_nodes.models.get(&model_id).unwrap();
        assert_eq!(model.__base_attr__.database, "dev_db");
    }

    #[dbt_runtime::test]
    async fn test_get_state_resolved_ids_empty_on_client_error() {
        let model_id = "model.test.a".to_string();
        let mut defer_nodes = Nodes::default();
        defer_nodes
            .models
            .insert(model_id.clone(), model_with_unique_id(&model_id));
        let client: SharedRunCacheServiceClient = Arc::new(MockResolveClient(ResolveOutcome::Err));
        let node_ids = BTreeSet::from([model_id.clone()]);

        let result = RunCacheProfileResolver::get_state_resolved_ids(
            Some(client),
            &node_ids,
            &auto_defer_config(),
            AdapterType::Snowflake,
            "profile",
            "project",
            Some("proj-123"),
            &mut defer_nodes,
        )
        .await;

        assert!(result.is_empty());
        // node left un-patched
        let model = defer_nodes.models.get(&model_id).unwrap();
        assert_eq!(model.__base_attr__.database, "dev_db");
    }

    #[dbt_runtime::test]
    async fn test_get_state_resolved_ids_success() {
        let model_id = "model.test.a".to_string();
        let mut defer_nodes = Nodes::default();
        defer_nodes
            .models
            .insert(model_id.clone(), model_with_unique_id(&model_id));
        let client: SharedRunCacheServiceClient = Arc::new(MockResolveClient(ResolveOutcome::Ok(
            HashMap::from([(model_id.clone(), "prod_db.prod_schema.a".to_string())]),
        )));
        let node_ids = BTreeSet::from([model_id.clone()]);

        let result = RunCacheProfileResolver::get_state_resolved_ids(
            Some(client),
            &node_ids,
            &auto_defer_config(),
            AdapterType::Snowflake,
            "profile",
            "project",
            Some("proj-123"),
            &mut defer_nodes,
        )
        .await;

        assert_eq!(result, BTreeSet::from([model_id.clone()]));
        let model = defer_nodes.models.get(&model_id).unwrap();
        assert_eq!(model.__base_attr__.database, "PROD_DB");
    }
}
