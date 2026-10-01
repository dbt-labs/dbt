use dbt_common::collections::DashMap;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::{
    DiscreteEventEmitter, FsResult,
    stats::{NodeStatus, Stat},
};
use dbt_env::env::InternalEnv;
use dbt_jinja_utils::invocation_args::InvocationArgs;
use dbt_schemas::{
    schemas::{InternalDbtNodeAttributes, ResolvedCloudConfig},
    state::ResolverState,
};
use proto_rust::v1::public::events::fusion::{
    AdapterInfo, AdapterInfoV2, Invocation, InvocationEnv, Login, PackageInstall, ResourceCounts,
    RunModel, StaticAnalysisInvocation,
};
pub use proto_rust::v1::public::events::fusion::{LoginErrorCode, LoginType};
use proto_rust::v1::public::fields::core_types::ObservabilityMetric;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use uuid::Uuid;

use dbt_vortex::{log_proto, log_proto_and_shutdown};

/// Structure for the .user.yml file format: {id: <uuid>}
#[derive(Debug, Serialize, Deserialize)]
struct UserFile {
    id: String,
}

pub fn noop_event_emitter() -> Box<dyn DiscreteEventEmitter> {
    Box::new(NoopEventEmitter {})
}

struct NoopEventEmitter;

impl DiscreteEventEmitter for NoopEventEmitter {
    fn configure(&mut self, _send_anonymous_usage_stats: bool) {
        // No-op implementation
    }

    fn dbt_distribution(&self) -> &'static str {
        ""
    }

    fn invocation_start_event(
        &self,
        _invocation_id: &Uuid,
        _root_project_name: &str,
        _profile_path: Option<&Path>,
        _command: String,
    ) {
        // No-op implementation
    }
}

pub fn default_event_emitter(
    enabled: Option<bool>,
    dbt_distribution: &'static str,
) -> Box<dyn DiscreteEventEmitter> {
    Box::new(DefaultEventEmitter {
        enabled,
        dbt_distribution,
    })
}

/// Source-available implementation of the DiscreteEventEmitter.
struct DefaultEventEmitter {
    enabled: Option<bool>,
    dbt_distribution: &'static str,
}

impl DiscreteEventEmitter for DefaultEventEmitter {
    fn configure(&mut self, send_anonymous_usage_stats: bool) {
        self.enabled = Some(send_anonymous_usage_stats);
    }

    fn dbt_distribution(&self) -> &'static str {
        self.dbt_distribution
    }

    fn invocation_start_event(
        &self,
        invocation_id: &Uuid,
        root_project_name: &str,
        profile_path: Option<&Path>,
        command: String,
    ) {
        match self.enabled {
            Some(explicitly_enabled) => {
                if !explicitly_enabled {
                    return; // disabled, do nothing
                }
            }
            None => {
                debug_assert!(
                    false,
                    "DiscreteEventEmitter must be configured with `configure()` before use"
                );
                return; // not explicitly configured, do nothing on release builds
            }
        }
        let env = InternalEnv::global();
        // Some commands don't load dbt_project
        let project_id = if !root_project_name.is_empty() {
            format!("{:x}", md5::compute(root_project_name))
        } else {
            "".to_string()
        };
        // Create Invocation start message
        let message = Invocation {
            //  REQUIRED invocation_id - globally unique identifier
            invocation_id: invocation_id.to_string(),
            // REQUIRED  event_id - unique identifier for this event (uuid)
            event_id: Uuid::new_v4().to_string(),
            //  progress - start/end
            progress: "start".to_string(),
            //  version - dbt version
            version: env.invocation_config().dbt_version.clone(),
            //  project_id - MD5 hash of the project name
            project_id,
            //  user_id - UUID generated to identify a unique user (~/.dbt/.user.yml)
            user_id: profile_path.map(get_user_id).unwrap_or_default(),
            //  command - full string of the command that was run
            command,
            //  result_type - ok/error.
            //  only provided on invocation_end
            result_type: "".to_string(),
            //  git_commit_sha - SHA of the git commit of the dbt project being run
            git_commit_sha: "".to_string(),
            //  distribution - dbt distribution that ran this command
            distribution: self.dbt_distribution.to_string(),
            //  enrichment - toggle enrichment of message by vortex
            enrichment: None,
            common_context: None,
        };

        let _ = log_proto(message);

        let environment = env.invocation_config().environment.clone();
        // dbt-core: core/dbt/tracking.py::get_dbt_env_context
        let message = InvocationEnv {
            // REQUIRED invocation_id - globally unique identifier
            invocation_id: invocation_id.to_string(),
            // REQUIRED  event_id - unique identifier for this event (uuid)
            event_id: Uuid::new_v4().to_string(),
            // This is a string that indicates the environment in which the invocation is
            environment,
            // This field is a toggle to enable enrichment of the message by the Vortex service.
            enrichment: None,
            common_context: None,
        };

        let _ = log_proto(message);
    }
}

pub fn build_result_string(result: &FsResult<()>) -> String {
    match result {
        Ok(()) => "ok",
        Err(err) => match err.exit_status() {
            Some(0) => "ok",
            Some(_) => "error",
            None => "unknown",
        },
    }
    .to_string()
}

/// Logs the `Invocation` event and shuts down the Vortex client.
pub fn invocation_end_event(
    invocation_id: String,
    result_string: String,
    dbt_distribution: &str,
    shutdown: bool,
) {
    let env = InternalEnv::global();
    // Create Invocation start message
    let message = Invocation {
        //  REQUIRED invocation_id - globally unique identifier
        invocation_id,
        // REQUIRED  event_id - unique identifier for this event (uuid)
        event_id: Uuid::new_v4().to_string(),
        //  progress - start/end
        progress: "end".to_string(),
        //  version - dbt version
        version: env.invocation_config().dbt_version.clone(),
        //  project_id - MD5 hash of the project name
        project_id: "".to_string(),
        //  user_id - UUID generated to identify a unique user (~/.dbt/.user.yml)
        user_id: "".to_string(),
        //  command - full string of the command that was run
        command: "".to_string(),
        //  result_type - ok/error.
        //  only provided on invocation_end
        result_type: result_string,
        //  git_commit_sha - SHA of the git commit of the dbt project being run
        git_commit_sha: "".to_string(),
        //  distribution - dbt distribution that ran this command
        distribution: dbt_distribution.to_string(),
        //  enrichment - toggle enrichment of message by vortex
        enrichment: None,
        common_context: None,
    };

    if shutdown {
        #[allow(unused_variables)]
        let _ = log_proto_and_shutdown(message).map_err(|e| {
            #[cfg(debug_assertions)]
            {
                use vortex_client::client::ProducerError;
                match e {
                    ProducerError::DevModeError(e) => eprintln!("{e}"),
                    ProducerError::SendError(e) => eprintln!("{e}"),
                    ProducerError::ShutdownError(e) => panic!("{e:?}"),
                }
            }
        });
    } else {
        let _ = log_proto(message);
    }
}

/// In dbt-core, this is in core/dbt/task/run.py::track_model_run
#[allow(clippy::too_many_arguments)]
pub fn run_model_event(
    invocation_id: String,
    run_stats: &DashMap<String, Stat>,
    node: &dyn InternalDbtNodeAttributes,
    maybe_incremental_strategy: Option<String>,
    is_contract_enforced: bool,
    has_group: bool,
    table_format: Option<String>,
    catalog_name: Option<String>,
    catalog_type: Option<String>,
    compute_mode: String,
) {
    let unique_id = node.unique_id();
    if !run_stats.contains_key(&unique_id) {
        // Seeds are expected to not have stats. Any other node type missing
        // stats indicates a bug (see PR #9146 regression).
        debug_assert!(
            node.resource_type().as_static_ref() == "seed",
            "run_stats missing for non-seed node {unique_id} — telemetry will be skipped"
        );
        return;
    }
    let stat = run_stats.get(&unique_id).unwrap();
    let access = node
        .get_access()
        .as_ref()
        .map_or_else(|| "".to_string(), |a| a.to_string());

    let hashed_contents = node
        .common()
        .raw_code
        .as_ref()
        .map_or_else(|| "".to_string(), |a| format!("{:x}", md5::compute(a)));

    let mut skipped = false;
    let mut skipped_reason = "".to_string();
    match stat.status {
        NodeStatus::Succeeded
        | NodeStatus::SucceededWithWarning
        | NodeStatus::TestWarned
        | NodeStatus::TestPassed
        | NodeStatus::Errored => {}
        NodeStatus::SkippedUpstreamFailed => {
            skipped = true;
            skipped_reason = "upstream_failed".to_string();
        }
        NodeStatus::ReusedNoChanges(_) => {
            skipped = true;
            skipped_reason = "reused_no_changes".to_string();
        }
        NodeStatus::ReusedStillFresh(_, _, _) => {
            skipped = true;
            skipped_reason = "reused_still_fresh".to_string();
        }
        NodeStatus::ReusedStillFreshNoChanges(_) => {
            skipped = true;
            skipped_reason = "reused_still_fresh".to_string();
        }
        NodeStatus::ReusedCloned(None) => {
            skipped = true;
            skipped_reason = "reused_cloned_from_cache".to_string();
        }
        NodeStatus::ReusedCloned(Some(_)) => {
            skipped = true;
            skipped_reason = "reused_cloned_from_cache_still_fresh".to_string();
        }
        NodeStatus::StaticallyCheckedDataTest => {
            skipped = true;
            skipped_reason = "statically_checked_data_test".to_string();
        }
        NodeStatus::NoOp => {
            skipped = true;
            skipped_reason = "no-op".to_string();
        }
    }

    let resource_type = node.resource_type().as_static_ref().to_string();

    let message = RunModel {
        // REQUIRED invocation_id - globally unique identifier
        invocation_id,
        // REQUIRED  event_id - unique identifier for this event (uuid)
        event_id: Uuid::new_v4().to_string(),
        // Numerical index of the model being run in this invocation
        index: 0,
        // Total number of models being run in this invocation
        total: 0,
        // the time it took for a model to execute in seconds
        execution_time: stat.get_duration().as_secs_f64(),
        // success or failure status of that model's run
        run_status: stat.result_status_string(),
        // whether or not the model was skipped
        run_skipped: skipped,
        // the materialization strategy used for that model
        model_materialization: node.materialized().to_string(),
        // the incremental strategy used for that model (ex. append, merge, etc.)
        model_incremental_strategy: maybe_incremental_strategy.unwrap_or_default(),
        // unique identifier for the model
        model_id: format!("{:x}", md5::compute(unique_id)),
        // MD5 hash of the model's contents
        hashed_contents,
        // the language used to write the model (ex. sql, python)
        language: node
            .common()
            .language
            .clone()
            .unwrap_or_else(|| "sql".to_string()),
        // whether or not the model is in a group
        has_group,
        // whether or not the model is contract enforced
        contract_enforced: is_contract_enforced,
        // the access level of the model (ex. public, private, etc.)
        access,
        // whether or not the model is versioned
        versioned: node.is_versioned(),
        // A reason for why the model was skipped. cost_avoidance/upstream_failed
        run_skipped_reason: skipped_reason,
        // A globally unique ID that is emitted at each instance of an individual
        run_model_id: Uuid::new_v4().to_string(),
        //  enrichment - toggle enrichment of message by vortex
        enrichment: None,
        // The resource type of the node (model, test, etc.)
        resource_type,
        table_format: table_format.unwrap_or_default(),
        catalog_name: catalog_name.unwrap_or_default(),
        catalog_type: catalog_type.unwrap_or_default(),
        common_context: None,
        compute_mode,
    };

    let _ = log_proto(message);
}

/// Emit ObservabilityMetric for private package resolution visibility (Datadog/Vortex).
/// Called when resolving a private package via cloud providers - on success and failure.
pub fn private_package_usage_event(
    cloud_config: &Option<ResolvedCloudConfig>,
    package_name: &str,
    provider: Option<&str>,
    matched: bool,
    matched_provider: Option<&str>,
) {
    let mut tags = vec![
        format!("package:{package_name}"),
        format!("provider:{}", provider.unwrap_or("unspecified")),
        format!("matched:{}", matched),
    ];
    if let Some(mp) = matched_provider {
        tags.push(format!("matched_provider:{mp}"));
    }
    tags.extend([
        format!(
            "account:{}",
            cloud_config
                .as_ref()
                .and_then(|c| c.credentials.as_ref().map(|cr| cr.account_id.as_str()))
                .unwrap_or("unknown")
        ),
        format!(
            "project:{}",
            cloud_config
                .as_ref()
                .and_then(|c| c.project_id.as_deref())
                .unwrap_or("unknown")
        ),
        format!(
            "environment:{}",
            cloud_config
                .as_ref()
                .and_then(|c| c.environment_id.as_deref())
                .unwrap_or("unknown")
        ),
    ]);

    let message = ObservabilityMetric {
        label: "private_package_usage".to_string(),
        value: if matched { 1.0 } else { 0.0 },
        tags,
        debug_message: String::new(),
    };

    let _ = log_proto(message);
}

/// core/dbt/task/deps.py::track_package_install
pub fn package_install_event(invocation_id: String, name: String, version: String, source: String) {
    let message = PackageInstall {
        // REQUIRED invocation_id - globally unique identifier
        invocation_id,
        // REQUIRED  event_id - unique identifier for this event (uuid)
        event_id: Uuid::new_v4().to_string(),
        // plain string name of a package that was installed. This is often the same
        // or similar to the git repository name of that package.
        name,
        // plain string source of the package. This is also referred to as the
        // installation method in our internal analytics. This is based on the syntax
        // used in packages.yml file.
        source,
        // either a semantic version of the package (if installed through the hub) or
        // a git commit hash (if installed through git).
        version,
        // This field is a toggle to enable enrichment of the message by the Vortex service.
        enrichment: None,
        common_context: None,
    };

    let _ = log_proto(message);
}

/// dbt-core core/dbt/compilation.py::print_compile_stats, track_resource_counts
pub fn resource_counts_event(
    args: InvocationArgs,
    resolved_state: &ResolverState,
    catalog_count: i32,
) {
    let model_count = resolved_state.nodes.models.len() as i32;
    let seed_count = resolved_state.nodes.seeds.len() as i32;
    let data_test_count = resolved_state.nodes.tests.len() as i32;
    let snapshot_count = resolved_state.nodes.snapshots.len() as i32;
    let operation_count = (resolved_state.operations.on_run_start.len()
        + resolved_state.operations.on_run_end.len()) as i32;
    let analysis_count = resolved_state.nodes.analyses.len() as i32;
    // We need to add functions to ResourceCounts proto
    let _function_count = resolved_state.nodes.functions.len() as i32;

    let source_count = resolved_state.nodes.sources.len() as i32;
    let macro_count = resolved_state.macros.macros.len() as i32;
    let group_count = resolved_state.nodes.groups.len() as i32;
    let unit_test_count = resolved_state.nodes.unit_tests.len() as i32;
    let exposure_count = resolved_state.nodes.exposures.len() as i32;

    // to-be-implemented
    let metric_count = 0;
    let semantic_model_count = 0;
    let saved_query_count = 0;

    let message = ResourceCounts {
        // REQUIRED invocation_id - globally unique identifier
        invocation_id: args.invocation_id.to_string(),
        // REQUIRED  event_id - unique identifier for this event (uuid)
        event_id: Uuid::new_v4().to_string(),
        // total count of models in the project.
        models: model_count,
        // total count of data tests (originally just tests) in the project.
        tests: data_test_count,
        // total count of snapshots in the project.
        snapshots: snapshot_count,
        // total count of analysis queries in the project.
        analyses: analysis_count,
        // total count of macros in the project.
        macros: macro_count,
        // total count of operations in the project.
        operations: operation_count,
        // total count of seeds in the project.
        seeds: seed_count,
        // total count of sources in the project.
        sources: source_count,
        // total count of exposures in the project.
        exposures: exposure_count,
        // total count of metrics in the project.
        metrics: metric_count,
        // total count of groups in the project.
        groups: group_count,
        // total count of unit tests in the project.
        unit_tests: unit_test_count,
        // total count of semantic models in the project.
        semantic_models: semantic_model_count,
        // total count of saved queries in the project.
        saved_queries: saved_query_count,
        // total count of catalogs in the project.
        catalogs: catalog_count,
        // This field is a toggle to enable enrichment of the message by the Vortex service.
        enrichment: None,
        common_context: None,
    };

    let _ = log_proto(message);
}

pub fn adapter_info_event(invocation_id: String, adapter_type: String, adapter_unique_id: String) {
    let message = AdapterInfo {
        // REQUIRED invocation_id - globally unique identifier
        invocation_id,
        // REQUIRED  event_id - unique identifier for this event (uuid)
        event_id: Uuid::new_v4().to_string(),
        // adapter_type is the plain string name for the dbt adapter that's used by the
        // project. Examples could include: "bigquery", "snowflake", "redshift", "postgres", etc.
        adapter_type,
        // adapter_unique_id is the unique identifier of a project's warehouse credentials.
        // For supported warehouses, we create an MD5 hash of the connection string.
        // The specific string varies per adapter_type.
        adapter_unique_id,
        // This field is a toggle to enable enrichment of the message by the Vortex service.
        enrichment: None,
        common_context: None,
    };

    let _ = log_proto(message);
}

/// Not yet implemented, all stubbed
pub fn adapter_info_v2_event() {
    let message = AdapterInfoV2 {
        // REQUIRED  event_id - unique identifier for this event (uuid)
        event_id: Uuid::new_v4().to_string(),
        // A foreign key to the RunModel message that was emitted at each instance
        // of an individual model being run.
        run_model_id: "".to_string(),
        // This reflects the adapter name used when they ran a given model.
        adapter_name: "".to_string(),
        // This reflects the simplified semantic version of an adapter that was used
        // when they ran a given model. ex. 1.9.0
        base_adapter_version: "".to_string(),
        // This reflects the full adapter version used when they ran a given model.
        adapter_version: "".to_string(),
        // This is a flexible key-value pair that can be used to store any additional
        // model adapter information. Today this is used to store two pieces of
        // information: the model_adapter_type (the adapter_name that was used for
        // that specific model) and the model_adapter_table_format (Iceberg or
        // something else).
        model_adapter_details: HashMap::new(),
        // This field is a toggle to enable enrichment of the message by the Vortex service.
        enrichment: None,
        common_context: None,
    };

    let _ = log_proto(message);
}

/// This looks for or creates a .user.yml file in the same directory
/// as the profiles.yml file, which stores a uuid user_id.
pub fn get_user_id(profile_path: &Path) -> String {
    let profiles_dir = profile_path.parent();
    if let Some(profiles_dir) = profiles_dir {
        let cookie_path = profiles_dir.join(".user.yml");
        if cookie_path.exists() {
            match fs::read_to_string(&cookie_path) {
                Ok(contents) => match dbt_yaml::from_str::<UserFile>(&contents) {
                    Ok(user_file) => user_file.id,
                    Err(_) => set_user_cookie(profiles_dir, &cookie_path),
                },
                Err(_) => set_user_cookie(profiles_dir, &cookie_path),
            }
        } else {
            set_user_cookie(profiles_dir, &cookie_path)
        }
    } else {
        "".to_string()
    }
}

fn set_user_cookie(profiles_dir: &Path, cookie_path: &Path) -> String {
    let user_id = Uuid::new_v4().to_string();
    let profiles_file = profiles_dir.join("profiles.yml");
    // Check if profiles_dir is empty (current directory) or exists
    if (profiles_dir.as_os_str().is_empty() || profiles_dir.exists()) && profiles_file.exists() {
        let user_file = UserFile {
            id: user_id.clone(),
        };
        if let Ok(yaml) = dbt_yaml::to_string(&user_file) {
            let _ = fs::write(cookie_path, yaml);
        }
    }
    // even if we weren't able to store the user_id in the .user.yml file, return it anyway
    user_id
}

struct LoginJwtClaims {
    platform_user_id: Option<u64>,
    platform_account_id: Option<u64>,
    platform_account_identifier: Option<String>,
}

fn extract_login_jwt_claims(access_token: &str) -> Option<LoginJwtClaims> {
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let parts: Vec<&str> = access_token.split('.').collect();
    let payload_bytes = URL_SAFE_NO_PAD.decode(parts.get(1)?).ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    let parse_u64 =
        |v: &serde_json::Value| -> Option<u64> { v.as_u64().or_else(|| v.as_str()?.parse().ok()) };
    Some(LoginJwtClaims {
        platform_user_id: parse_u64(&payload["sub"]),
        platform_account_id: parse_u64(&payload["https://dbt.com/account_id"]),
        platform_account_identifier: payload["https://dbt.com/account_identifier"]
            .as_str()
            .map(str::to_owned),
    })
}

/// Walk from the current directory toward the filesystem root looking for a
/// `dbt_project.yml`. Returns the MD5 hex of the project `name` field, or
/// `None` if no project file is found or the name cannot be parsed.
fn discover_project_id() -> Option<String> {
    #[derive(serde::Deserialize)]
    struct ProjectName {
        name: String,
    }
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("dbt_project.yml");
        if candidate.exists() {
            let contents = fs::read_to_string(candidate).ok()?;
            let p: ProjectName = dbt_yaml::from_str(&contents).ok()?;
            return Some(format!("{:x}", md5::compute(&p.name)));
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Emit a `Login` Vortex event. Call once per `dbt login` invocation,
/// after the login attempt completes (success or failure).
///
/// `access_token` should be the platform OAuth access token when available —
/// on success it comes directly from the new session; on failure it can be
/// read from the cached session on disk so that expired-JWT identity fields
/// are still populated.
///
/// `error_code`/`error_message` should be populated whenever `success` is `false`.
pub fn login_event(
    invocation_id: &Uuid,
    success: bool,
    login_type: LoginType,
    access_token: Option<&str>,
    error_code: Option<LoginErrorCode>,
    error_message: Option<String>,
) {
    let claims = access_token.and_then(extract_login_jwt_claims);
    let c = claims.as_ref();

    let default_profiles_path = dirs::home_dir().map(|h| h.join(".dbt").join("profiles.yml"));
    let user_cookie = default_profiles_path
        .as_deref()
        .map(get_user_id)
        .unwrap_or_default();

    let message = Login {
        enrichment: None,
        event_id: Uuid::new_v4().to_string(),
        invocation_id: invocation_id.to_string(),
        success,
        login_type: login_type as i32,
        user_cookie,
        project_id: discover_project_id(),
        platform_user_id: c.and_then(|x| x.platform_user_id),
        platform_account_id: c.and_then(|x| x.platform_account_id),
        platform_account_identifier: c.and_then(|x| x.platform_account_identifier.clone()),
        error_code: error_code.map(|c| c as i32),
        error_message,
        common_context: None,
    };

    let _ = log_proto(message);
}

pub fn static_analysis_propagation_event(
    invocation_id: String,
    project_id: String,
    max_sa_level: Option<StaticAnalysisKind>,
    source: &str,
    strict_count: usize,
    baseline_count: usize,
    off_count: usize,
) {
    let max_static_analysis_level = match max_sa_level {
        Some(StaticAnalysisKind::Strict | StaticAnalysisKind::On | StaticAnalysisKind::Unsafe) => {
            "strict".to_string()
        }
        Some(StaticAnalysisKind::Baseline) => "baseline".to_string(),
        Some(StaticAnalysisKind::Off) => "off".to_string(),
        None => "".to_string(),
    };

    let message = StaticAnalysisInvocation {
        enrichment: None,
        event_id: Uuid::new_v4().to_string(),
        invocation_id,
        project_id,
        max_static_analysis_level,
        source: source.to_string(),
        strict_node_count: strict_count as i32,
        baseline_node_count: baseline_count as i32,
        off_node_count: off_count as i32,
        common_context: None,
    };

    let _ = log_proto(message);
}
