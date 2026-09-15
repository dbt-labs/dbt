use dbt_adapter::{enforce_adapter_gating, experimental_adapters_allowed};
use dbt_common::tracing::dbt_emit::{emit_info_progress_message, emit_warn_log_message};
use dbt_telemetry::ProgressMessage;

use dbt_common::stdfs::canonicalize;
use dbt_common::warn_error_options::WarnErrorOptions;
use dbt_common::{ErrorCode, FsResult, err, fs_err};

use dbt_yaml::{Span, Spanned};

use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::register_base_functions;
use dbt_profile::{
    ProfileEnvironment, ProfileError, ResolvedProfile, find_profiles_path, resolve_with_env,
};
use dbt_schemas::schemas::profiles::DbConfig;
use dbt_schemas::schemas::serde::yaml_to_fs_error;
use dbt_schemas::state::{ProfileAdapter, ProfileConnection};

use indexmap::IndexMap;
use pathdiff::diff_paths;
use std::path::PathBuf;

use dbt_schemas::schemas::project::DbtProjectSimplified;
use dbt_schemas::state::DbtProfile;

use dirs::home_dir;

use crate::args::LoadArgs;

pub fn load_profiles(
    arg: &LoadArgs,
    raw_dbt_project: &DbtProjectSimplified,
) -> FsResult<DbtProfile> {
    let profile = get_profile_with_span(arg.profile.as_ref(), raw_dbt_project.profile.clone())?;

    // Locate profiles.yml via dbt-profile's standard search order:
    // --profiles-dir (exclusive) > CWD > ~/.dbt/
    let profile_path = find_profiles_path(arg.profiles_dir.as_deref())
        .map_err(|e| fs_err!(ErrorCode::InvalidConfig, "{}", e))?;

    let abs_profile_path = canonicalize(&profile_path)?;
    let abs_in_dir = canonicalize(&arg.io.in_dir)?;
    let relative_profile_path = diff_paths(&abs_profile_path, &abs_in_dir).ok_or_else(|| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to get relative path from profiles.yml to project directory"
        )
    })?;

    let show_path = if let Some(home_dir) = home_dir() {
        let home_dir = home_dir.join(".dbt");
        if abs_profile_path.starts_with(home_dir) {
            PathBuf::from("~/.dbt/profiles.yml")
        } else {
            relative_profile_path.clone()
        }
    } else {
        relative_profile_path.clone()
    };

    emit_info_progress_message(ProgressMessage::new_from_action_and_target(
        "Loading".to_string(),
        show_path.display().to_string(),
    ));

    let profile_name = profile.clone().into_inner();

    // Resolve the profile using dbt-profile's Jinja environment, plus the same base
    // functions as full dbt Jinja (`tojson`, `fromjson`, etc.) so profiles.yml matches dbt-core.
    let mut penv = ProfileEnvironment::new(arg.vars.clone());
    register_base_functions(&mut penv.env, WarnErrorOptions::default());
    let resolved: ResolvedProfile =
        resolve_with_env(&penv, &profile_path, &profile_name, arg.target.as_deref()).map_err(
            |e| match e {
                ProfileError::Yaml { source, path } => yaml_to_fs_error(source, Some(&path)),
                ProfileError::ProfileMissing { .. } => fs_err!(
                    code => ErrorCode::IoError,
                    loc => profile.span().clone(),
                    "Profile '{}' not found in profiles.yml",
                    profile_name
                ),
                _ => fs_err!(ErrorCode::InvalidConfig, "{}", e),
            },
        )?;

    let defer_to_target = profile_defer_to_target(&resolved.credentials);
    let allow_clones = target_allow_clones(&resolved.credentials);

    // Convert the rendered credentials mapping into a typed DbConfig. Cloned
    // rather than moved because the non-default adapters are read from
    // `resolved` further down.
    let credentials_value = dbt_yaml::Value::Mapping(resolved.credentials.clone(), Span::default());
    let db_config: DbConfig = dbt_yaml::from_value(credentials_value).map_err(|e| {
        fs_err!(
            ErrorCode::InvalidConfig,
            "Failed to parse profiles.yml: {}",
            e
        )
    })?;

    let allow_experimental_adapters = experimental_adapters_allowed();
    enforce_adapter_gating(db_config.adapter_type(), allow_experimental_adapters)?;

    // Parse and gate every connection of every adapter, so a misconfigured or
    // ungated one fails at load rather than at first use. `dbt-profile` carries the
    // adapter key as a string -- it has no `AdapterType` -- so this is also where
    // an unknown key is caught, by `DbConfig` deserialization failing on the `type:`
    // it injected.
    let mut adapters: IndexMap<AdapterType, ProfileAdapter> = IndexMap::new();
    for adapter in &resolved.adapters {
        let mut connections: Vec<ProfileConnection> = Vec::with_capacity(adapter.connections.len());
        for connection in &adapter.connections {
            // The default connection of the default adapter is already parsed as
            // `db_config`; reuse it so the two cannot drift.
            let config = if connection.is_default {
                db_config.clone()
            } else {
                let value =
                    dbt_yaml::Value::Mapping(connection.credentials.clone(), Span::default());
                let config: DbConfig = dbt_yaml::from_value(value).map_err(|e| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "Failed to parse connection '{}' of adapter '{}' in profiles.yml: {}",
                        connection.name,
                        adapter.adapter_type,
                        e
                    )
                })?;
                enforce_adapter_gating(config.adapter_type(), allow_experimental_adapters)?;
                warn_on_ignored_threads(
                    &connection.credentials,
                    &adapter.adapter_type,
                    connection.named.then_some(connection.name.as_str()),
                    &resolved.target_name,
                );
                config
            };
            connections.push(ProfileConnection {
                name: connection.name.clone(),
                config,
            });
        }

        let adapter_type = connections[adapter.default_connection]
            .config
            .adapter_type();
        if adapter.has_unreachable_connections() {
            emit_warn_log_message(
                ErrorCode::InvalidConfig,
                format!(
                    "target '{}' declares {} connections for adapter '{}'; only '{}' is used.                      Selecting a connection is not supported yet, so the others are ignored.",
                    resolved.target_name,
                    adapter.connections.len(),
                    adapter.adapter_type,
                    connections[adapter.default_connection].name,
                ),
            );
        }

        // One entry per adapter type. The parser keys by the map key, so duplicates
        // are impossible; this asserts that rather than silently overwriting.
        if adapters.contains_key(&adapter_type) {
            return err!(
                ErrorCode::InvalidConfig,
                "target '{}' declares adapter '{}' more than once",
                resolved.target_name,
                adapter.adapter_type
            );
        }
        adapters.insert(
            adapter_type,
            ProfileAdapter {
                connections,
                default_connection: adapter.default_connection,
            },
        );
    }
    let default_adapter = db_config.adapter_type();

    if db_config.has_removed_execute_field() {
        emit_warn_log_message(
            ErrorCode::DeprecatedOption,
            "The `execute:` field in profiles.yml is no longer supported and will be ignored. \
             Use the `--compute inline|sidecar|service|remote` CLI flag instead. \
             Please remove `execute:` from your profile.",
        );
    }

    let database = db_config.get_database_or_default();
    let schema = db_config
        .get_schema()
        .map(String::as_str)
        .unwrap_or("public")
        .to_string();

    Ok(DbtProfile {
        database,
        schema,
        profile: profile.into_inner(),
        target: resolved.target_name,
        defer_to_target,
        allow_clones,
        adapters,
        default_adapter,
        relative_profile_path,
        threads: arg.threads,
    })
}

fn profile_defer_to_target(credentials: &dbt_yaml::Mapping) -> Option<String> {
    match credentials.get("defer_to_target") {
        Some(dbt_yaml::Value::String(target, _)) if !target.is_empty() => Some(target.clone()),
        _ => None,
    }
}

/// Reads the active target's `allow_clones` setting straight from its
/// `outputs.<target>` block in profiles.yml, ahead of typed `DbConfig` parsing.
fn target_allow_clones(credentials: &dbt_yaml::Mapping) -> bool {
    match credentials
        .get("allow_clones")
        .or_else(|| credentials.get("run_cache_allow_clones"))
    {
        Some(dbt_yaml::Value::Bool(allow_clones, _)) => *allow_clones,
        Some(dbt_yaml::Value::String(value, _)) => {
            matches!(value.to_ascii_lowercase().as_str(), "true")
        }
        _ => true, //if not specified, defaults to true
    }
}

/// Warn that a connection other than the target's default sets `threads:`, which
/// does nothing.
///
/// Thread count is target-wide: `resolve_and_set_threads` reads it from the
/// target's default connection (or `--threads`) and then overwrites every other
/// connection's with that value, so a `threads:` written anywhere else is dropped
/// before anything reads it. Staying silent would leave the author believing their
/// secondary adapter -- lake compute, typically -- runs at its own concurrency.
///
/// `connection_name` is `None` for a connection whose name was defaulted rather
/// than written: only an author-written name is worth quoting back, and an unnamed
/// connection is identified by its adapter alone.
fn warn_on_ignored_threads(
    credentials: &dbt_yaml::Mapping,
    adapter_type: &str,
    connection_name: Option<&str>,
    target: &str,
) {
    if credentials.get("threads").is_none() {
        return;
    }
    let which = match connection_name {
        Some(name) => format!("connection '{name}' of adapter '{adapter_type}'"),
        None => format!("adapter '{adapter_type}'"),
    };
    emit_warn_log_message(
        ErrorCode::UnusedConfigKey,
        format!(
            "{which} in target '{target}' sets 'threads:', which is ignored. Thread count is \
             target-wide, taken from the target's default connection or --threads. Remove it."
        ),
    );
}

/// Resolve the profile name to use.
fn get_profile_with_span(
    arg_profile: Option<&String>,
    proj_profile: Spanned<Option<String>>,
) -> FsResult<Spanned<String>> {
    match (proj_profile.as_ref(), arg_profile) {
        (None, None) => {
            err!(
                ErrorCode::InvalidConfig,
                "No profile specified in dbt_project.yml"
            )
        }
        (None, Some(prof)) | (Some(_), Some(prof)) => Ok(Spanned::new(prof.to_string())
            .map_span(|_| Span::default().with_filename(PathBuf::from("<cmdline>")))),
        (Some(_), None) => Ok(proj_profile.map(|x| x.unwrap())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use dbt_common::warn_error_options::WarnErrorOptions;
    use dbt_jinja_utils::register_base_functions;
    use dbt_profile::ProfileEnvironment;

    #[test]
    fn enforce_adapter_gating_rejects_unsupported_adapter() {
        let err = enforce_adapter_gating(AdapterType::Trino, false).unwrap_err();
        let msg = err.message();
        assert!(
            msg.contains("not yet supported by dbt"),
            "expected gating message, got: {msg}"
        );
        assert!(
            msg.contains("Supported adapters:"),
            "expected supported list, got: {msg}"
        );
        assert!(
            msg.contains("DBT_ALLOW_EXPERIMENTAL_ADAPTERS=true"),
            "expected env-var hint, got: {msg}"
        );
    }

    #[test]
    fn loader_registers_tojson_function_on_profile_env() {
        let mut penv = ProfileEnvironment::new(Default::default());
        register_base_functions(&mut penv.env, WarnErrorOptions::default());
        let out = penv
            .env
            .render_str("{{ tojson({'a': 1}) }}", &penv.ctx, &[])
            .expect("tojson should be registered for loader profile resolution");
        assert!(out.contains("\"a\""), "unexpected tojson output: {out}");
    }

    #[test]
    fn target_allow_clones_defaults_to_true() {
        let credentials = dbt_yaml::Mapping::new();
        assert!(target_allow_clones(&credentials));
    }

    #[test]
    fn target_allow_clones_reads_bool_value() {
        let credentials = dbt_yaml::Mapping::from_iter([(
            "allow_clones".into(),
            dbt_yaml::Value::Bool(false, Span::default()),
        )]);
        assert!(!target_allow_clones(&credentials));
    }

    #[test]
    fn target_allow_clones_reads_legacy_run_cache_value() {
        let credentials = dbt_yaml::Mapping::from_iter([(
            "run_cache_allow_clones".into(),
            dbt_yaml::Value::Bool(false, Span::default()),
        )]);
        assert!(!target_allow_clones(&credentials));
    }

    #[test]
    fn target_allow_clones_reads_string_value() {
        let credentials = dbt_yaml::Mapping::from_iter([(
            "allow_clones".into(),
            dbt_yaml::Value::String("false".to_string(), Span::default()),
        )]);
        assert!(!target_allow_clones(&credentials));
    }
}
