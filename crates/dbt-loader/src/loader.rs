use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use dbt_adapter::load_catalogs;
use dbt_cloud_config::{
    DbtCloudConfig, ResolvedCloudConfig, detect_unlinked_project, resolve_cloud_config,
};
use dbt_common::cancellation::CancellationToken;
use dbt_common::constants::{
    DBT_CATALOGS_YML, DBT_DEPENDENCIES_YML, DBT_PACKAGES_LOCK_FILE, DBT_PACKAGES_YML,
    DBT_PROFILES_YML, DBT_SELECTORS_YML, DBT_VARS_YML,
};
use dbt_common::io_args::{InternalPackageMode, ReplayMode, TimeMachineMode};
use dbt_common::once_cell_vars::DISPATCH_CONFIG;
use dbt_common::path::DbtPath;
use dbt_common::tracing::TracingConfigProvider;
use dbt_common::tracing::dbt_emit::{
    emit_error_log_message, emit_warn_log_from_fs_error, emit_warn_log_message,
};
use dbt_common::tracing::span_info::SpanStatusRecorder;
use dbt_common::warn_error_options::{
    WarnErrorOptions, project_flags_get_value, resolve_warn_error_options,
};
use dbt_jinja_utils::invocation_args::InvocationArgs;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::phases::load::init::initialize_load_jinja_environment;
use dbt_jinja_utils::phases::load::init::initialize_load_profile_jinja_environment;
use dbt_schemas::schemas::serde::{StringOrInteger, yaml_to_fs_error};
use dbt_schemas::schemas::telemetry::{ExecutionPhase, PhaseExecuted};
use dbt_schemas::state::DbtProfile;
use dbt_telemetry::GenericOpItemProcessed;
use dbt_yaml;
use fs_deps::get_or_install_packages;
use fs_deps::private_package::PrivatePackageResolver;
use indexmap::IndexMap;
use pathdiff::diff_paths;
use serde::Deserialize;
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use std::{fs, io};
use tracing::Instrument;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use dbt_common::constants::{
    DBT_INTERNAL_PACKAGES_DIR_NAME, DBT_PACKAGES_DIR_NAME, DBT_PROJECT_YML,
};
use dbt_common::error::LiftableResult;
use project::DbtProject;

use dbt_common::stdfs::last_modified;
use dbt_common::{ErrorCode, create_debug_span, ectx, err, tokiofs};
use dbt_common::{FsResult, fs_err};
use dbt_jinja_vars::DbtVars;
use dbt_schemas::schemas::project::{self, DbtProjectSimplified, ProjectDbtCloudConfig};
use dbt_schemas::state::{DbtAsset, DbtPackage, DbtState, ResourcePathKind};

use crate::args::LoadArgs;
use crate::dbt_project_yml_loader::load_project_yml;
use crate::loader_hooks::LoaderHooks;
use crate::utils::{collect_file_info, identify_package_dependencies};
use crate::{
    construct_internal_packages, load_internal_packages, load_packages, load_profiles, load_vars,
    persist_internal_packages,
};

use dbt_jinja_utils::Var;
use dbt_jinja_utils::phases::load::secret_renderer::secret_context_env_var;
use dbt_jinja_utils::serde::{into_typed_with_jinja, value_from_file};

use dbt_common::tracing::event_info::store_event_attributes;

fn warn_on_unlinked_cloud_project(
    dbt_cloud_yml: Option<&DbtCloudConfig>,
    project_dbt_cloud: Option<&ProjectDbtCloudConfig>,
    cloud_config: Option<&ResolvedCloudConfig>,
) {
    if cloud_config.is_some_and(|c| c.credentials.is_some()) {
        return;
    }
    let Some(project_dbt_cloud) = project_dbt_cloud else {
        return;
    };
    let Some(project_id) = detect_unlinked_project(dbt_cloud_yml, Some(project_dbt_cloud)) else {
        return;
    };

    let err = fs_err!(
        code => ErrorCode::InvalidConfig,
        loc => project_dbt_cloud.project_id.span().clone(),
        "Project '{}' not found in ~/.dbt/dbt_cloud.yml. Run `dbt login` for this project, or \
         download a new dbt_cloud.yml from the dbt platform UI and merge it into your existing \
         one. Features that need dbt platform credentials will not work.",
        project_id
    );
    emit_warn_log_from_fs_error(*err);
}

fn resolve_and_set_threads(
    dbt_profile: &mut DbtProfile,
    iarg: &InvocationArgs,
) -> FsResult<Option<usize>> {
    let final_threads = if iarg.num_threads.is_none() {
        if let Some(threads) = dbt_profile.default_db_config().get_threads() {
            // Convert StringOrInteger to Option<usize>
            match threads {
                StringOrInteger::Integer(n) => Some(*n as usize),
                StringOrInteger::String(s) => Some(s.parse::<usize>().map_err(|_| {
                    fs_err!(
                        ErrorCode::ProfileInvalid,
                        "Invalid number of threads in profiles.yml: {s}",
                    )
                })?),
            }
        } else {
            None
        }
    } else {
        iarg.num_threads
    };

    dbt_profile.threads = final_threads;
    // Thread count is a target-wide concurrency setting, so it applies to every
    // connection of every adapter the target declares -- not just the default one,
    // which would leave a node running on a non-default adapter with none.
    for adapter in dbt_profile.adapters_mut() {
        adapter.set_threads(Some(StringOrInteger::Integer(
            final_threads.unwrap_or(0) as i64
        )));
    }

    Ok(final_threads)
}

pub(crate) struct ResolvedWarnErrorOptions {
    pub warn_error: bool,
    pub warn_error_options: WarnErrorOptions,
}

/// Resolve warn-error options from CLI/env and project flags.
///
/// This has a side effect: when `tracing_features` is provided, it reloads the
/// current tracing config with the resolved warn-error options.
pub(crate) fn resolve_and_reload_weo_from_project(
    simplified_dbt_project: &DbtProjectSimplified,
    from_cli: Option<bool>,
    from_cli_or_env: Option<&WarnErrorOptions>,
    tracing_features: Option<&dyn TracingConfigProvider>,
) -> FsResult<ResolvedWarnErrorOptions> {
    let (warn_error, warn_error_options) = resolve_warn_error_options(
        from_cli,
        from_cli_or_env,
        simplified_dbt_project.flags.as_ref(),
    );

    let (warning_messages, error_message) = warn_error_options.validation_messages();

    for message in warning_messages {
        emit_warn_log_message(ErrorCode::NotSupportedWarnErrorOption, message);
    }

    if let Some(message) = error_message {
        emit_error_log_message(ErrorCode::InvalidOptions, &message);
        return Err(dbt_common::FsError::exit_with_status(1));
    }

    if let Some(msg) = warn_error_options.deprecated_keys_message() {
        emit_warn_log_message(ErrorCode::WEOIncludeExcludeDeprecation, msg);
    }
    if let Some(tracing_handle) = tracing_features {
        tracing_handle.set_warn_error_options(warn_error_options.clone());
    }

    Ok(ResolvedWarnErrorOptions {
        warn_error,
        warn_error_options,
    })
}

/// Enabled by the CLI/env, or by `flags: { <name>: true }` in dbt_project.yml.
pub fn resolve_bool_project_flag(
    from_cli: bool,
    project_flags: Option<&dbt_yaml::Value>,
    name: &str,
) -> bool {
    from_cli
        || project_flags
            .and_then(|flags| project_flags_get_value(flags, name))
            .and_then(dbt_yaml::Value::as_bool)
            .unwrap_or_default()
}

#[tracing::instrument(
    skip_all,
    fields(
        _e = ?store_event_attributes(PhaseExecuted::start_general(ExecutionPhase::LoadProject)),
    )
)]
pub async fn load(
    arg: &LoadArgs,
    iarg: Cow<'_, InvocationArgs>,
    tracing_features: Option<&dyn TracingConfigProvider>,
    token: &CancellationToken,
    loader_hooks: Arc<dyn LoaderHooks>,
    private_package_resolver: Arc<dyn PrivatePackageResolver>,
) -> FsResult<DbtState> {
    let (simplified_dbt_project, mut dbt_profile, vars_from_file) =
        load_simplified_project_and_profiles(arg).await?;

    // Parse dbt_cloud.yml (if it exists)
    let dbt_cloud_yml = match dbt_cloud_config::get_cloud_project_path() {
        Ok(path) => match dbt_cloud_config::parse_cloud_config(&path) {
            Ok(config) => config,
            Err(e) => {
                emit_warn_log_message(
                    ErrorCode::InvalidConfig,
                    format!(
                        "Ignoring dbt_cloud.yml: {}. Cloud credentials will not be available.",
                        e
                    ),
                );
                None
            }
        },
        Err(e) => {
            emit_warn_log_message(
                ErrorCode::InvalidConfig,
                format!(
                    "Could not determine dbt_cloud.yml path: {}. Cloud credentials will not be available.",
                    e
                ),
            );
            None
        }
    };

    // Resolve cloud config with precedence: env > dbt_project.yml > dbt_cloud.yml
    let cloud_config = resolve_cloud_config(
        dbt_cloud_yml.as_ref(),
        simplified_dbt_project.dbt_cloud.as_ref(),
    );

    warn_on_unlinked_cloud_project(
        dbt_cloud_yml.as_ref(),
        simplified_dbt_project.dbt_cloud.as_ref(),
        cloud_config.as_ref(),
    );

    // Check if .gitignore exists and add dbt_internal_packages/ to it if not present
    let gitignore_path = arg.io.in_dir.join(".gitignore");
    if gitignore_path.exists() {
        let gitignore_content = fs::read_to_string(&gitignore_path)?;
        if !gitignore_content.contains(format!("{DBT_INTERNAL_PACKAGES_DIR_NAME}/").as_str()) {
            let mut updated_content = gitignore_content;
            if !updated_content.ends_with('\n') {
                updated_content.push('\n');
            }
            updated_content.push_str(format!("{DBT_INTERNAL_PACKAGES_DIR_NAME}/\n").as_str());
            fs::write(&gitignore_path, updated_content)?;
        }
    }

    // initialize loader into a crate accessible static location
    let env = initialize_load_profile_jinja_environment();
    load_catalogs(arg, &env, simplified_dbt_project.flags.as_ref()).await?;

    let resolved_warn_error_options = resolve_and_reload_weo_from_project(
        &simplified_dbt_project,
        arg.cli_warn_error,
        arg.cli_warn_error_options.as_ref(),
        tracing_features,
    )?;
    let mut iarg = iarg;
    if iarg.warn_error != resolved_warn_error_options.warn_error
        || iarg.warn_error_options != resolved_warn_error_options.warn_error_options
    {
        let iarg_mut = iarg.to_mut();
        iarg_mut.warn_error = resolved_warn_error_options.warn_error;
        iarg_mut.warn_error_options = resolved_warn_error_options.warn_error_options.clone();
    }
    let final_threads = resolve_and_set_threads(&mut dbt_profile, iarg.as_ref())?;

    let use_v2_compatible_package_downloads = resolve_bool_project_flag(
        arg.io.use_v2_compatible_package_downloads,
        simplified_dbt_project.flags.as_ref(),
        "use_v2_compatible_package_downloads",
    );
    let require_hub_verified_downloads = resolve_bool_project_flag(
        arg.io.require_hub_verified_downloads,
        simplified_dbt_project.flags.as_ref(),
        "require_hub_verified_downloads",
    );

    if iarg.num_threads != final_threads {
        iarg.to_mut().num_threads = final_threads;
    }
    // Preserve the original CLI vars for `dbt_state.cli_vars` (used for the
    // partial-parse vars hash); arg.vars itself is replaced with the merged set
    // (vars.yml + CLI, CLI wins) so all downstream Jinja rendering of YAML files
    // (dbt_project.yml in any package, packages.yml, catalogs.yml, …) sees the
    // vars declared in vars.yml.
    let original_cli_vars = arg.vars.clone();
    let merged_vars = merge_vars(&vars_from_file, &arg.vars);
    let arg = LoadArgs {
        threads: final_threads,
        vars: merged_vars,
        root_vars_from_file: vars_from_file,
        ..arg.clone()
    };

    let mut dbt_state = DbtState {
        dbt_profile,
        run_started_at: run_started_at(),
        packages: vec![],
        vars: BTreeMap::new(),
        cli_vars: original_cli_vars,
        catalogs: load_catalogs::fetch_catalogs(),
        cloud_config,
        warn_error: iarg.warn_error,
        warn_error_options: iarg.warn_error_options.clone(),
    };

    // If we are running `dbt debug` we don't need to collect dbt_project.yml files
    if arg.debug_profile {
        return Ok(dbt_state);
    }

    let flags: BTreeMap<String, minijinja::Value> = iarg.to_dict();

    let env = initialize_load_jinja_environment(
        &dbt_state.dbt_profile.profile,
        &dbt_state.dbt_profile.target,
        dbt_state.dbt_profile.default_db_config().adapter_type(),
        dbt_state.dbt_profile.default_db_config().clone(),
        dbt_state.run_started_at,
        &flags,
        iarg.warn_error_options.clone(),
        arg.io.clone(),
        dbt_state.catalogs.clone(),
    )?;

    // Load the packages.yml file, if it exists and install the packages if arg.install_deps is true
    let (packages_install_path, internal_packages_install_path) = get_packages_install_path(
        &arg.io.in_dir,
        &arg.packages_install_path,
        &arg.internal_packages_install_path,
        &simplified_dbt_project,
    );

    let adapter_type = dbt_state.dbt_profile.default_db_config().adapter_type();
    let arg_ref = &arg;
    if let Some(prev_dbt_state) = arg.prev_dbt_state.clone() {
        let prev_root_package = prev_dbt_state.root_package();

        if !prev_root_package.dependencies.is_empty() && !packages_install_path.exists() {
            return Err(fs_err!(
                ErrorCode::CacheError,
                "Packages directory does not exist",
            ));
        }

        // --inline warm path: skip the full WalkDir (load_inner) and reconstruct the
        // root package file lists directly from prev_root_package.all_paths.
        //
        // INVARIANTS that must hold for this path to be correct:
        // 1. prev_dbt_state.packages is non-empty (root package exists at index 0).
        //    Guaranteed by snapshot_packages writing ≥1 user package and the assert below.
        // 2. prev_root_package.all_paths contains entries for every ResourcePathKind
        //    the resolver needs (ModelPaths, MacroPaths, TestPaths, SeedPaths, SnapshotPaths).
        //    Entries may be sparse — find_files_by_kind_and_extension returns [] for missing kinds.
        // 3. No file deletions or renames happened between the previous run and this one.
        //    The inline path does NOT re-stat files; stale entries are harmless (resolver
        //    reads them, fails on missing file, and the error surfaces naturally).
        // 4. DISPATCH_CONFIG may already be set from a prior run in the same process;
        //    OnceLock::set silently no-ops if already set, which is correct (same binary,
        //    same dispatch config).
        // 5. The inline SQL file is NOT in prev_root_package.all_paths. It is injected
        //    separately by prepare_inline_sql after this block, into a dedicated "" package
        //    (appended to dbt_state.packages), never into the root package.
        let new_root_package = if arg.inline_sql.is_some() {
            if DISPATCH_CONFIG.get().is_none() {
                let dbt_project_path = arg.io.in_dir.join(DBT_PROJECT_YML);
                if let Ok((proj, _)) =
                    load_project_yml(&env, &dbt_project_path, None, arg.vars.clone())
                {
                    let map = proj.dispatch.as_ref().map_or_else(BTreeMap::new, |d| {
                        d.iter()
                            .map(|dc| (dc.macro_namespace.clone(), dc.search_order.clone()))
                            .collect()
                    });
                    let _ = DISPATCH_CONFIG.set(RwLock::new(map));
                }
            }
            let pkg_name = &prev_root_package.dbt_project.name;
            let in_dir = &arg.io.in_dir;
            let ap = &prev_root_package.all_paths;
            let mut root = (*prev_root_package).clone();
            root.model_sql_files = find_files_by_kind_and_extension(
                in_dir,
                pkg_name,
                &ResourcePathKind::ModelPaths,
                &["sql", "py"],
                ap,
            );
            root.macro_files = find_files_by_kind_and_extension(
                in_dir,
                pkg_name,
                &ResourcePathKind::MacroPaths,
                &["sql"],
                ap,
            );
            root.test_files = find_files_by_kind_and_extension(
                in_dir,
                pkg_name,
                &ResourcePathKind::TestPaths,
                &["sql"],
                ap,
            );
            root.seed_files = find_files_by_kind_and_extension(
                in_dir,
                pkg_name,
                &ResourcePathKind::SeedPaths,
                &["csv", "parquet", "json"],
                ap,
            );
            root.snapshot_files = find_files_by_kind_and_extension(
                in_dir,
                pkg_name,
                &ResourcePathKind::SnapshotPaths,
                &["sql"],
                ap,
            );
            root
        } else {
            let package_map_lookup = BTreeMap::new();
            let mut dummy_collected_vars = Vec::new();
            let mut r = load_inner(
                arg_ref,
                &env,
                &arg.io.in_dir,
                &dbt_state.dbt_profile,
                false,
                &package_map_lookup,
                true,
                &mut dummy_collected_vars,
            )
            .await?;
            r.dependencies = prev_root_package.dependencies.clone();
            r
        };
        dbt_state.vars = prev_dbt_state.vars.clone();

        let packages = prev_dbt_state
            .packages
            .iter()
            .map(|x| (*x).clone())
            .collect::<Vec<_>>();
        dbt_state.packages.extend(packages);
        // prev_dbt_state always contains at least the root package; assert so a
        // corrupted cache produces a clear message rather than an index panic.
        assert!(
            !dbt_state.packages.is_empty(),
            "prev_dbt_state must contain at least one package (the root)"
        );
        dbt_state.packages[0] = new_root_package;

        // Internal packages (dbt built-ins: get_where_subquery, etc.) are embedded in the
        // binary and never written to the parquet cache, so they are absent from
        // prev_dbt_state.packages. Always load them fresh on the incremental path too.
        let internal_pkgs =
            construct_internal_packages(dbt_state.dbt_profile.adapter_types(), &arg.io.in_dir)?;
        dbt_state.packages.extend(internal_pkgs);

        // Inject inline SQL even on the incremental path.
        if let Some(inline_sql) = &arg.inline_sql {
            let _ = prepare_inline_sql(inline_sql, &arg.io.out_dir, &mut dbt_state).await?;
        }

        return Ok(dbt_state);
    }

    if arg.internal_package_mode == InternalPackageMode::ForceWrite {
        persist_internal_packages(
            &internal_packages_install_path,
            adapter_type,
            &arg,
            loader_hooks.clone(),
        )?;
    }

    let (packages_lock, upstream_projects) = get_or_install_packages(
        &arg.io,
        arg.command,
        &env,
        &packages_install_path,
        arg.install_deps,
        arg.add_package.clone(),
        arg.upgrade,
        arg.lock,
        arg.vars.clone(),
        arg.version_check,
        arg.skip_private_deps,
        iarg.replay.as_ref(),
        token,
        use_v2_compatible_package_downloads,
        require_hub_verified_downloads,
        private_package_resolver,
        dbt_state.cloud_config.clone(),
        arg.ai_provider.as_deref(),
    )
    .await?;

    // Skip downloading publication artifacts in Time Machine replay mode
    // In replay mode, we're replaying a recorded session and don't need to download anything
    let is_time_machine_replay = matches!(
        &iarg.replay,
        Some(ReplayMode::FsTimeMachine(TimeMachineMode::Replay(_)))
    );

    if !is_time_machine_replay {
        loader_hooks
            .download_artifacts(&upstream_projects, &dbt_state.cloud_config, &arg.io)
            .await?;
    }

    // If we are running `dbt deps` we don't need to collect files
    if arg.install_deps {
        return Ok(dbt_state);
    }

    let lookup_map = packages_lock.lookup_map(&arg.io.in_dir);
    let mut collected_vars = vec![];
    {
        // TODO: use a dedicated event. Currently this is tightly coupled with tui_layer and assumes
        // that `ExecutionPhase::LoadProject` will register it's progress spinner with "load" id.
        let span = create_debug_span(GenericOpItemProcessed::new(
            "load".to_string(),
            "loading".to_string(),
            "loaded".to_string(),
            "packages".to_string(),
        ));

        let packages = load_packages(
            &arg,
            &env,
            &dbt_state.dbt_profile,
            &mut collected_vars,
            &lookup_map,
            &packages_install_path,
            token,
        )
        .instrument(span.clone())
        .await
        .record_status(&span)?;
        dbt_state.packages = packages;
    }
    {
        let internal_pkgs = match &arg.internal_package_mode {
            InternalPackageMode::Embedded => {
                #[allow(unused_mut)]
                let mut pkgs = construct_internal_packages(
                    dbt_state.dbt_profile.adapter_types(),
                    &arg.io.in_dir,
                )?;
                loader_hooks
                    .will_load_internal_packages(&arg, &mut pkgs)
                    .await?;
                // Register vars for each internal package
                for pkg in &pkgs {
                    load_vars(&pkg.dbt_project.name, None, &mut collected_vars)?;
                }
                pkgs
            }
            InternalPackageMode::ForceWrite | InternalPackageMode::ReadFromDisk => {
                // TODO: use a dedicated event. Currently this is tightly coupled with tui_layer and assumes
                // that `ExecutionPhase::LoadProject` will register it's progress spinner with "load" id.
                let span = create_debug_span(GenericOpItemProcessed::new(
                    "load".to_string(),
                    "loading".to_string(),
                    "loaded".to_string(),
                    "internal packages".to_string(),
                ));
                load_internal_packages(
                    &arg,
                    &env,
                    &dbt_state.dbt_profile,
                    &mut collected_vars,
                    &internal_packages_install_path,
                    token,
                )
                .instrument(span.clone())
                .await
                .record_status(&span)?
            }
        };
        dbt_state.packages.extend(internal_pkgs);
        dbt_state.vars = collected_vars.into_iter().collect();
    }

    // Handle inline SQL if provided
    if let Some(inline_sql) = &arg.inline_sql {
        let _ = prepare_inline_sql(inline_sql, &arg.io.out_dir, &mut dbt_state).await?;
    }

    Ok(dbt_state)
}

/// Lightweight load function for the `clean` command.
///
/// This function loads only the minimal state needed for cleaning
#[tracing::instrument(
    skip_all,
    fields(
        _e = ?store_event_attributes(PhaseExecuted::start_general(ExecutionPhase::LoadProject)),
    )
)]
pub async fn load_for_clean(arg: &LoadArgs) -> FsResult<DbtState> {
    let (simplified_dbt_project, dbt_profile, vars_from_file) =
        load_simplified_project_and_profiles(arg).await?;

    let original_cli_vars = arg.vars.clone();
    let arg = LoadArgs {
        vars: merge_vars(&vars_from_file, &arg.vars),
        root_vars_from_file: vars_from_file,
        ..arg.clone()
    };

    let resolved_warn_error_options = resolve_and_reload_weo_from_project(
        &simplified_dbt_project,
        arg.cli_warn_error,
        arg.cli_warn_error_options.as_ref(),
        None,
    )?;

    let env = initialize_load_profile_jinja_environment();
    load_catalogs(&arg, &env, simplified_dbt_project.flags.as_ref()).await?;

    // Create minimal DbtState - no packages, no vars
    let dbt_state = DbtState {
        dbt_profile,
        run_started_at: run_started_at(),
        packages: vec![],
        vars: BTreeMap::new(),
        cli_vars: original_cli_vars,
        catalogs: load_catalogs::fetch_catalogs(),
        cloud_config: None,
        warn_error: resolved_warn_error_options.warn_error,
        warn_error_options: resolved_warn_error_options.warn_error_options,
    };

    Ok(dbt_state)
}

pub async fn load_catalogs(
    arg: &LoadArgs,
    env: &JinjaEnv,
    project_flags: Option<&dbt_yaml::Value>,
) -> FsResult<()> {
    let ctx: BTreeMap<String, minijinja::Value> = BTreeMap::from([
        (
            "env_var".to_owned(),
            minijinja::Value::from_func_func("env_var", secret_context_env_var),
        ),
        (
            "var".to_owned(),
            minijinja::Value::from_object(Var::new(arg.vars.clone())),
        ),
    ]);
    // Record the use_catalogs_v2 flag whether or not catalogs.yml exists, so
    // downstream checks can tell "flag set but no catalogs.yml" from "flag unset".
    load_catalogs::set_use_catalogs_v2_from_flags(project_flags);

    let catalogs_yml_path = arg.io.in_dir.join(DBT_CATALOGS_YML);
    match fs::read_to_string(&catalogs_yml_path) {
        Ok(raw_text) => {
            let raw_text_yml: dbt_yaml::Value = dbt_yaml::from_str(&raw_text)
                .map_err(|e| yaml_to_fs_error(e, Some(&catalogs_yml_path)))?;
            let text: dbt_yaml::Value =
                into_typed_with_jinja(raw_text_yml, true, env, &ctx, &[], None, true)?;
            load_catalogs::load_catalogs(text, &catalogs_yml_path, project_flags)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(fs_err!(
            code => ErrorCode::InvalidConfig,
            loc  => PathBuf::from(&catalogs_yml_path),
            "Failed to read '{}': {}", DBT_CATALOGS_YML, e
        )),
    }
}

/// Load `vars.yml` from the project root if it exists.
///
/// Returns the contents of the `vars` key as a map, or an empty map if the
/// file is missing, empty, or has no top-level `vars` key. Invalid types
/// inside `vars` (e.g. non-string keys, non-mapping `vars:`) surface as
/// `dbt1013` YAML errors, matching how `dbt_project.yml` handles vars.
pub fn vars_data_from_root(project_root: &Path) -> FsResult<BTreeMap<String, dbt_yaml::Value>> {
    let vars_yml_path = project_root.join(DBT_VARS_YML);
    if !vars_yml_path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw = value_from_file(&vars_yml_path, false, None)?;
    let vars_value = match raw.get("vars") {
        Some(v) if !v.is_null() => v.clone(),
        _ => return Ok(BTreeMap::new()),
    };
    Deserialize::deserialize(vars_value).map_err(|e| yaml_to_fs_error(e, Some(&vars_yml_path)))
}

/// Error if vars are defined in both `vars.yml` and `dbt_project.yml`.
fn validate_vars_not_in_both(
    raw_dbt_project: &dbt_yaml::Value,
    has_vars_file: bool,
) -> FsResult<()> {
    if !has_vars_file {
        return Ok(());
    }
    let project_has_vars = raw_dbt_project
        .get("vars")
        .is_some_and(|v| !v.is_null() && v.as_mapping().is_some_and(|m| !m.is_empty()));
    if project_has_vars {
        return err!(
            ErrorCode::InvalidConfig,
            "Variables cannot be defined in both {} and {}.",
            DBT_VARS_YML,
            DBT_PROJECT_YML,
        );
    }
    Ok(())
}

/// Merge vars from `vars.yml` and CLI `--vars`. CLI values take precedence,
/// recursively merging mappings so a CLI override of a package-scoped block
/// (e.g. `--vars '{pkg: {y: 2}}'` with `vars.yml` `pkg: {x: 1}`) preserves
/// `x` from the file instead of replacing the whole `pkg` block.
pub fn merge_vars(
    vars_from_file: &BTreeMap<String, dbt_yaml::Value>,
    cli_vars: &BTreeMap<String, dbt_yaml::Value>,
) -> BTreeMap<String, dbt_yaml::Value> {
    let mut merged: BTreeMap<String, dbt_yaml::Value> = vars_from_file.clone();
    for (k, v) in cli_vars {
        if let Some(existing) = merged.get_mut(k) {
            deep_merge_yaml(existing, v);
        } else {
            merged.insert(k.clone(), v.clone());
        }
    }
    merged
}

/// Recursively merge `overlay` into `base`. Mappings are merged key-by-key so
/// nested keys present only in `base` survive; any non-mapping `overlay` (or
/// a type mismatch) replaces `base` wholesale.
fn deep_merge_yaml(base: &mut dbt_yaml::Value, overlay: &dbt_yaml::Value) {
    match (base, overlay) {
        (dbt_yaml::Value::Mapping(base_map, _), dbt_yaml::Value::Mapping(overlay_map, _)) => {
            for (k, v) in overlay_map {
                if let Some(existing) = base_map.get_mut(k) {
                    deep_merge_yaml(existing, v);
                } else {
                    base_map.insert(k.clone(), v.clone());
                }
            }
        }
        (base_slot, overlay) => {
            *base_slot = overlay.clone();
        }
    }
}

pub async fn load_simplified_project_and_profiles(
    arg: &LoadArgs,
) -> FsResult<(
    DbtProjectSimplified,
    DbtProfile,
    BTreeMap<String, dbt_yaml::Value>,
)> {
    // Read the input file
    let dbt_project_path = arg.io.in_dir.join(DBT_PROJECT_YML);

    let raw_dbt_project_in_val = value_from_file(&dbt_project_path, false, None)?;

    // Load vars.yml (if present) and validate mutual exclusivity with dbt_project.yml's vars.
    let vars_from_file = vars_data_from_root(&arg.io.in_dir)?;
    validate_vars_not_in_both(&raw_dbt_project_in_val, !vars_from_file.is_empty())?;

    // Merge for Jinja rendering: CLI overrides vars.yml.
    let merged_vars = merge_vars(&vars_from_file, &arg.vars);

    let env = initialize_load_profile_jinja_environment();
    let ctx: BTreeMap<String, minijinja::Value> = BTreeMap::from([
        (
            "env_var".to_owned(),
            minijinja::Value::from_func_func("env_var", secret_context_env_var),
        ),
        (
            "var".to_owned(),
            minijinja::Value::from_object(Var::new(merged_vars)),
        ),
        (
            // Add empty target object. When we call `load_project_yml` later,
            // target will actually be populated.
            "target".to_owned(),
            minijinja::Value::from_serialize(BTreeMap::<String, minijinja::Value>::new()),
        ),
        (
            // Add empty context object (mimics dbt-core's BaseContext.to_dict() pattern)
            // This allows profiles.yml to use: context.project_name or ''
            "context".to_owned(),
            minijinja::Value::from_serialize(BTreeMap::<String, minijinja::Value>::new()),
        ),
    ]);

    let simplified_dbt_project: DbtProjectSimplified =
        into_typed_with_jinja(raw_dbt_project_in_val, true, &env, &ctx, &[], None, true)?;

    if simplified_dbt_project.source_paths.is_some() {
        return err!(
            ErrorCode::InvalidConfig,
            "'source-paths' cannot be specified in dbt_project.yml",
        );
    }
    if (*simplified_dbt_project.log_path)
        .as_ref()
        .is_some_and(|path| path != "logs")
    {
        return err!(
            ErrorCode::InvalidConfig,
            "'log-path' cannot be specified in dbt_project.yml",
        );
    }
    if (*simplified_dbt_project.target_path)
        .as_ref()
        .is_some_and(|path| path != "target")
    {
        return err!(
            ErrorCode::InvalidConfig,
            "'target-path' cannot be specified in dbt_project.yml",
        );
    }

    let dbt_profile = load_profiles(arg, &simplified_dbt_project)?;

    Ok((simplified_dbt_project, dbt_profile, vars_from_file))
}

#[allow(clippy::too_many_arguments)]
pub async fn load_inner(
    arg: &LoadArgs,
    env: &JinjaEnv,
    package_path: &Path,
    dbt_profile: &DbtProfile,
    // Indicates if we are loading a dependency or a root project
    is_dependency: bool,
    package_lookup_map: &BTreeMap<String, String>,
    skip_dependencies: bool,
    collected_vars: &mut Vec<(String, IndexMap<String, DbtVars>)>,
) -> FsResult<DbtPackage> {
    // Vars loaded from `vars.yml` at the root project (set by the loader rebuild step).
    let root_vars_from_file = &arg.root_vars_from_file;
    // all read files
    let mut all_files: HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>> = HashMap::new();

    let dbt_project_path = package_path.join(DBT_PROJECT_YML);

    let dependency_package_name = if is_dependency {
        Some(
            dbt_project_path
                .parent()
                .and_then(|p| p.file_name())
                .map(|os_str| os_str.to_string_lossy().to_string())
                .ok_or_else(|| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "Failed to get package name from path: {}",
                        &dbt_project_path.display()
                    )
                })?,
        )
    } else {
        None
    };

    let (dbt_project, raw_project_yml) = load_project_yml(
        env,
        &dbt_project_path,
        dependency_package_name.as_deref(),
        arg.vars.clone(),
    )?;
    // For the root project, prefer vars.yml over dbt_project.yml's `vars` field when vars.yml is present.
    let vars_for_load: Option<IndexMap<String, DbtVars>> =
        if !is_dependency && !root_vars_from_file.is_empty() {
            let mut map: IndexMap<String, DbtVars> = IndexMap::new();
            for (k, v) in root_vars_from_file {
                let parsed: DbtVars = Deserialize::deserialize(v.clone())
                    .map_err(|e| yaml_to_fs_error(e, Some(&dbt_project_path)))?;
                map.insert(k.clone(), parsed);
            }
            Some(map)
        } else {
            (*dbt_project.vars)
                .as_ref()
                .map(|vars| Deserialize::deserialize(vars.clone()))
                .transpose()
                .map_err(|e| yaml_to_fs_error(e, Some(&dbt_project_path)))?
        };
    load_vars(&dbt_project.name, vars_for_load, collected_vars)?;
    // Set dispatch config for future use
    if package_path == arg.io.in_dir {
        let dispatch_config_map = if let Some(dispatch_configs) = dbt_project.dispatch.clone() {
            dispatch_configs
                .iter()
                .map(|dispatch_config| {
                    (
                        dispatch_config.macro_namespace.clone(),
                        dispatch_config.search_order.clone(),
                    )
                })
                .collect()
        } else {
            BTreeMap::new()
        };
        // Only set the dispatch config on first load of the project (mainly impacts testing).
        // get_or_init synchronizes internally, so concurrent loads can't race between the
        // check and the set.
        DISPATCH_CONFIG.get_or_init(|| RwLock::new(dispatch_config_map));
    }

    let session_files = find_session_files(package_path)?;
    all_files.insert(ResourcePathKind::SessionPaths, session_files);

    // Collect file paths and their timestamps for fields with a suffix `_paths`
    let all_dirs = collect_paths(&dbt_project);
    let all_included_files: HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>> =
        collect_all_files(all_dirs, package_path)?;
    all_files.extend(all_included_files);

    // make all paths relative to the project directory
    for (_, files) in all_files.iter_mut() {
        for (path, _) in files.iter_mut() {
            *path = DbtPath::from(diff_paths(&path, package_path).unwrap());
        }
        //
        // make deterministic: Sort files based on their relative paths
        files.sort_by(|a, b| a.0.cmp(&b.0));
    }

    // todo: we could optimize here, but for now just take everything,...
    let mut dbt_properties = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::ModelPaths,
        &["yml", "yaml"],
        &all_files,
    );
    // additonal paths can have ym files (add generic tests etc)
    let seed_ymls = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::SeedPaths,
        &["yml", "yaml"],
        &all_files,
    );
    let snapshot_ymls = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::SnapshotPaths,
        &["yml", "yaml"],
        &all_files,
    );
    let analysis_ymls = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::AnalysisPaths,
        &["yml", "yaml"],
        &all_files,
    );

    let test_ymls = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::TestPaths,
        &["yml", "yaml"],
        &all_files,
    );

    let function_ymls = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::FunctionPaths,
        &["yml", "yaml"],
        &all_files,
    );

    let macro_ymls = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::MacroPaths,
        &["yml", "yaml"],
        &all_files,
    );

    let check_ymls = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::CheckPaths,
        &["yml", "yaml"],
        &all_files,
    );

    // todo: change dbt_properties to be BTreeSet, this may require many goldies updates
    for item in seed_ymls
        .iter()
        .chain(&snapshot_ymls)
        .chain(&analysis_ymls)
        .chain(&test_ymls)
        .chain(&function_ymls)
        .chain(&macro_ymls)
        .chain(&check_ymls)
    {
        if !dbt_properties.contains(item) {
            dbt_properties.push(item.clone());
        }
    }
    drop_nested_project_config_files(&mut dbt_properties);

    let analysis_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::AnalysisPaths,
        &["sql"],
        &all_files,
    );
    let check_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::CheckPaths,
        &["sql"],
        &all_files,
    );
    let mut model_sql_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::ModelPaths,
        &["sql"],
        &all_files,
    );
    let python_model_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::ModelPaths,
        &["py"],
        &all_files,
    );
    if !python_model_files.is_empty() {
        model_sql_files.extend(python_model_files);
        model_sql_files.sort_by(|a, b| a.path.cmp(&b.path));
    }
    let mut function_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::FunctionPaths,
        &["sql", "py", "js"],
        &all_files,
    );
    function_files.sort_by(|a, b| a.path.cmp(&b.path));

    let macro_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::MacroPaths,
        &["sql"],
        &all_files,
    );
    let test_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::TestPaths,
        &["sql"],
        &all_files,
    );
    let fixture_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::FixturePaths,
        &["csv", "sql"],
        &all_files,
    );
    let seed_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::SeedPaths,
        &["csv", "parquet", "json"],
        &all_files,
    );
    let docs_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::DocsPaths,
        &["md"],
        &all_files,
    );
    let snapshot_files = find_files_by_kind_and_extension(
        package_path,
        &dbt_project.name,
        &ResourcePathKind::SnapshotPaths,
        &["sql"],
        &all_files,
    );
    let dependencies = if skip_dependencies {
        BTreeSet::new()
    } else {
        identify_package_dependencies(
            &arg.io,
            package_path,
            package_lookup_map,
            dependency_package_name.as_deref(),
        )?
    };
    // Only do this for the root package.
    if !is_dependency {
        collect_profiles_yml_if_exists(dbt_profile, &mut all_files);
    }
    Ok(DbtPackage {
        dbt_project,
        package_root_path: package_path.to_path_buf(),
        dbt_properties,
        analysis_files,
        check_files,
        model_sql_files,
        function_sql_files: function_files,
        test_files,
        fixture_files,
        seed_files,
        macro_files,
        docs_files,
        snapshot_files,
        dependencies,
        all_paths: all_files,
        embedded_file_contents: None,
        raw_project_yml,
    })
}

/// outputs the timestamp that this run started
fn run_started_at() -> DateTime<Tz> {
    // check if we have env var DBT_RUN_STARTED_AT
    if let Ok(run_started_at) = std::env::var("DBT_RUN_STARTED_AT") {
        DateTime::parse_from_rfc3339(&run_started_at)
            .unwrap()
            .with_timezone(&Tz::UTC)
    } else {
        let utc_now = Utc::now();
        let tz_now: DateTime<Tz> = utc_now.with_timezone(&Tz::UTC);
        tz_now
    }
}

/// dbt config files that are only meaningful at a project root, never resource properties.
const PROJECT_ROOT_ONLY_YML_NAMES: [&str; 8] = [
    DBT_PROJECT_YML,
    DBT_DEPENDENCIES_YML,
    DBT_PACKAGES_YML,
    DBT_PACKAGES_LOCK_FILE,
    DBT_CATALOGS_YML,
    DBT_VARS_YML,
    DBT_PROFILES_YML,
    DBT_SELECTORS_YML,
];

/// Drops a project's own config files when an outer project's resource path walks into that
/// project. Only files next to a `dbt_project.yml` are config; elsewhere they are properties.
fn drop_nested_project_config_files(properties: &mut Vec<DbtAsset>) {
    let project_roots: HashSet<PathBuf> = properties
        .iter()
        .filter(|asset| asset.path.file_name() == Some(OsStr::new(DBT_PROJECT_YML)))
        .filter_map(|asset| asset.path.parent().map(Path::to_path_buf))
        .collect();

    properties.retain(|asset| {
        let Some(file_name) = asset.path.file_name().and_then(OsStr::to_str) else {
            return true;
        };
        !PROJECT_ROOT_ONLY_YML_NAMES.contains(&file_name)
            || !asset
                .path
                .parent()
                .is_some_and(|parent| project_roots.contains(parent))
    });
}

fn should_exclude_path(kind: &ResourcePathKind, path: &Path) -> bool {
    match kind {
        ResourcePathKind::TestPaths => {
            // Only exclude paths directly under <test-paths>/generic/
            let components: Vec<_> = path.components().collect();
            components.len() >= 2 && components[1].as_os_str() == "generic"
        }
        _ => false,
    }
}

fn find_files_by_kind_and_extension(
    in_dir: &Path,
    project_name: &str,
    path_kind: &ResourcePathKind,
    extensions: &[&str],
    all_paths: &HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>>,
) -> Vec<DbtAsset> {
    let default = vec![];
    let paths_to_filter: Vec<_> = all_paths
        .get(path_kind)
        .unwrap_or(&default)
        .iter()
        .collect();

    let mut paths = paths_to_filter
        .iter()
        .filter_map(|(path, _)| {
            path.extension()
                .and_then(OsStr::to_str)
                .filter(|ext| extensions.contains(&ext.to_lowercase().as_str()))
                .filter(|_| !should_exclude_path(path_kind, path.as_path()))
                .map(|_| DbtAsset {
                    package_name: project_name.to_string(),
                    base_path: in_dir.to_path_buf(),
                    path: path.to_path_buf(),
                    original_path: path.to_path_buf(),
                })
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    paths.sort_by(|a, b| a.path.cmp(&b.path));
    paths
}

/// Loads the .dbtignore file if it exists in the given path
pub fn load_dbtignore(path: &Path) -> FsResult<Option<Gitignore>> {
    let dbtignore_path = path.join(".dbtignore");
    if dbtignore_path.exists() {
        let mut builder = GitignoreBuilder::new(path);
        // add() returns Option<Error> where None means success and Some(err) is an error
        match builder.add(&dbtignore_path) {
            None => match builder.build() {
                Ok(gitignore) => return Ok(Some(gitignore)),
                Err(err) => {
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        loc => dbtignore_path.clone(),
                        "Error building .dbtignore: {}",
                        err
                    );
                }
            },
            Some(err) => {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    loc => dbtignore_path.clone(),
                    "Failed to add .dbtignore file: {}",
                    err
                );
            }
        }
    }
    Ok(None)
}

fn collect_all_files(
    all_dirs: HashMap<ResourcePathKind, Vec<String>>,
    base_path: &Path,
) -> FsResult<HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>>> {
    // Load .dbtignore file if it exists
    let dbtignore = load_dbtignore(base_path)?;
    // Remove debug statement for tests
    let mut all_paths: HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>> = HashMap::new();
    for (kind, paths) in &all_dirs {
        let mut info_paths = Vec::new();

        collect_file_info(
            base_path,
            paths,
            &mut info_paths,
            dbtignore.as_ref(),
            |path: &Path| {
                path.components().next()
                    != Some(std::path::Component::Normal(OsStr::new("fixtures")))
            },
        )
        .lift(ectx!(
            "Failed to collect file info: {}, {}",
            base_path.display(),
            paths.join(",")
        ))?;
        all_paths.insert(kind.clone(), info_paths);
    }
    Ok(all_paths)
}

fn collect_paths(dbt_project: &DbtProject) -> HashMap<ResourcePathKind, Vec<String>> {
    let mut all_dirs: HashMap<ResourcePathKind, Vec<String>> = HashMap::new();
    all_dirs.insert(
        ResourcePathKind::ModelPaths,
        dbt_project.model_paths.clone().unwrap_or_default(),
    );
    all_dirs.insert(
        ResourcePathKind::AnalysisPaths,
        dbt_project.analysis_paths.clone().unwrap_or_default(),
    );
    all_dirs.insert(
        ResourcePathKind::AssetPaths,
        dbt_project.asset_paths.clone().unwrap_or_default(),
    );
    all_dirs.insert(
        ResourcePathKind::CheckPaths,
        dbt_project.check_paths.clone().unwrap_or_default(),
    );
    all_dirs.insert(
        ResourcePathKind::FunctionPaths,
        dbt_project.function_paths.clone().unwrap_or_default(),
    );
    all_dirs.insert(
        ResourcePathKind::MacroPaths,
        dbt_project.macro_paths.clone().unwrap_or_default(),
    );
    all_dirs.insert(
        ResourcePathKind::SeedPaths,
        dbt_project.seed_paths.clone().unwrap_or_default(),
    );
    all_dirs.insert(
        ResourcePathKind::SnapshotPaths,
        dbt_project.snapshot_paths.clone().unwrap_or_default(),
    );
    all_dirs.insert(
        ResourcePathKind::TestPaths,
        dbt_project.test_paths.clone().unwrap_or_default(),
    );
    all_dirs.insert(
        ResourcePathKind::FixturePaths,
        dbt_project
            .test_paths
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|p| {
                let path = PathBuf::from(p).join("fixtures");
                path.into_os_string().into_string().unwrap_or_default()
            })
            .collect(),
    );
    // Only register docs paths if they are explicitly specified
    if dbt_project.docs_paths.is_some() && !dbt_project.docs_paths.as_ref().unwrap().is_empty() {
        all_dirs.insert(
            ResourcePathKind::DocsPaths,
            dbt_project.docs_paths.clone().unwrap_or_default(),
        );
    } else {
        // The default is to read all files in the following directories for '*.md' files
        let mut result: Vec<String> = vec![];

        result.extend_from_slice(dbt_project.analysis_paths.as_deref().unwrap_or_default());
        result.extend_from_slice(dbt_project.function_paths.as_deref().unwrap_or_default());
        result.extend_from_slice(dbt_project.macro_paths.as_deref().unwrap_or_default());
        result.extend_from_slice(dbt_project.model_paths.as_deref().unwrap_or_default());
        result.extend_from_slice(dbt_project.seed_paths.as_deref().unwrap_or_default());
        result.extend_from_slice(dbt_project.snapshot_paths.as_deref().unwrap_or_default());
        result.extend_from_slice(dbt_project.test_paths.as_deref().unwrap_or_default());

        all_dirs.insert(ResourcePathKind::DocsPaths, result);
    }
    all_dirs
}

// returns (packages_install_path, internal_packages_install_path)
pub(crate) fn get_packages_install_path(
    in_dir: &Path,
    arg_packages_install_path: &Option<PathBuf>,
    arg_internal_packages_install_path: &Option<PathBuf>,
    dbt_project: &DbtProjectSimplified,
) -> (PathBuf, PathBuf) {
    let packages_install_path = if let Some(path) = arg_packages_install_path {
        if path.is_absolute() {
            path.clone()
        } else {
            in_dir.join(path)
        }
    } else if let Some(path) = &dbt_project.packages_install_path {
        let mut path_buf = PathBuf::from(path);
        if !path_buf.is_absolute() {
            path_buf = in_dir.join(path_buf);
        }
        path_buf
    } else {
        in_dir.join(DBT_PACKAGES_DIR_NAME)
    };

    let internal_packages_install_path = if let Some(path) = arg_internal_packages_install_path {
        if path.is_absolute() {
            path.clone()
        } else {
            in_dir.join(path)
        }
    } else {
        packages_install_path.with_file_name(DBT_INTERNAL_PACKAGES_DIR_NAME)
    };

    (packages_install_path, internal_packages_install_path)
}

fn collect_profiles_yml_if_exists(
    dbt_profile: &DbtProfile,
    all_paths: &mut HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>>,
) {
    if let Ok(timestamp) = last_modified(&dbt_profile.relative_profile_path) {
        let entry = all_paths.entry(ResourcePathKind::ProfilePaths).or_default();
        entry.push((DbtPath::from(&dbt_profile.relative_profile_path), timestamp));
    }
}

/// These are the built-in session file paths relative to a project.
pub fn get_session_relative_file_paths() -> Vec<String> {
    vec![
        DBT_PROJECT_YML.into(),
        DBT_DEPENDENCIES_YML.into(),
        DBT_PACKAGES_YML.into(),
        DBT_PACKAGES_LOCK_FILE.into(),
        DBT_CATALOGS_YML.into(),
        DBT_VARS_YML.into(),
    ]
}

fn find_session_files(package_path: &Path) -> FsResult<Vec<(DbtPath, SystemTime)>> {
    let mut result = Vec::new();

    for relative_path in get_session_relative_file_paths() {
        // Heuristic for DBT_PROJECT_YML.
        // We actually want to raise an error if it was not able to be read.
        if relative_path == DBT_PROJECT_YML {
            let dbt_project_path = package_path.join(relative_path);
            let dbt_project_timestamp = last_modified(&dbt_project_path)?;
            result.push((DbtPath::from(dbt_project_path), dbt_project_timestamp));
        } else {
            let path = package_path.join(relative_path);
            if let Ok(timestamp) = last_modified(&path) {
                result.push((DbtPath::from(path), timestamp));
            }
        }
    }

    Ok(result)
}

/// Prepares inline SQL for processing by writing it to a file and adding it to the DbtState.
///
/// This function:
/// 1. Writes the inline SQL to target/inline_<uuid>.sql
/// 2. Creates a DbtAsset for the file
/// 3. Injects the asset into a dedicated "" (empty-named) package, rather than the
///    root package, so that inline SQL handling does not depend on the root package
///    being present or enabled.
/// 4. Returns the generated model name for later reference
///
/// # Arguments
/// * `inline_sql` - The SQL string provided via --inline flag
/// * `out_dir` - The target directory where inline SQL will be written
/// * `dbt_state` - The mutable DbtState to update with the inline asset
async fn prepare_inline_sql(
    inline_sql: &str,
    out_dir: &Path,
    dbt_state: &mut DbtState,
) -> FsResult<String> {
    // Generate unique model name using shortened UUID (first 8 chars)
    let unique_id = uuid::Uuid::new_v4();
    let short_id = &unique_id.to_string()[..8];
    let model_name = format!("inline_{short_id}");
    let filename = format!("{model_name}.sql");

    // Write inline SQL to target directory
    let inline_path = out_dir.join(&filename);
    tokiofs::create_dir_all(out_dir).await?;
    tokiofs::write(&inline_path, inline_sql).await?;

    let path = PathBuf::from(filename);
    // Create DbtAsset for the inline SQL
    let inline_asset = DbtAsset {
        base_path: out_dir.to_path_buf(),
        path: path.clone(),
        original_path: path,
        package_name: String::new(),
    };

    // Inject the inline SQL into a dedicated "" package so that inline handling
    // is independent of the root package (which may be disabled or absent).
    // Create the "" package only if it does not already exist; otherwise reuse it
    // so repeated calls accumulate inline models instead of spawning duplicates.
    let inline_package_name = String::new();
    let needs_vars = !dbt_state.vars.contains_key(&inline_package_name);
    if needs_vars {
        // The "" package needs vars like any other package. Mirror the global (root)
        // vars, which is what non-root packages receive during `load_vars`.
        let global_vars = dbt_state
            .vars
            .get(dbt_state.root_project_name())
            .cloned()
            .unwrap_or_default();
        dbt_state.vars.insert(inline_package_name, global_vars);
    }

    if let Some(inline_package) = dbt_state
        .packages
        .iter_mut()
        .find(|p| p.dbt_project.name.is_empty())
    {
        inline_package.model_sql_files.push(inline_asset);
    } else {
        let inline_package = DbtPackage {
            package_root_path: out_dir.to_path_buf(),
            model_sql_files: vec![inline_asset],
            ..DbtPackage::default()
        };
        dbt_state.packages.push(inline_package);
    }

    Ok(model_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::profiles::DbConfig;
    use dbt_schemas::state::ProfileAdapter;
    use dbt_schemas::state::ResourcePathKind;
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::SystemTime;

    #[test]
    fn resolve_threads_sets_profile_threads_from_target() {
        let db_config: DbConfig = dbt_yaml::from_str(
            "type: duckdb\n\
             path: db.duckdb\n\
             threads: 4",
        )
        .expect("valid DuckDB profile");
        let default_adapter = db_config.adapter_type();
        let adapters = IndexMap::from([(default_adapter, ProfileAdapter::single(db_config))]);
        let mut dbt_profile = DbtProfile {
            profile: "test".to_string(),
            target: "duckdb".to_string(),
            defer_to_target: None,
            allow_clones: true,
            adapters,
            default_adapter,
            schema: "main".to_string(),
            database: "db".to_string(),
            relative_profile_path: PathBuf::from("profiles.yml"),
            threads: None,
        };

        let resolved = resolve_and_set_threads(&mut dbt_profile, &InvocationArgs::default())
            .expect("profile threads should resolve");

        assert_eq!(resolved, Some(4));
        assert_eq!(dbt_profile.threads, Some(4));
    }

    #[test]
    fn test_find_files_by_kind_and_extension_excludes_generic_test_paths() {
        // Setup test data
        let in_dir = PathBuf::from("/project");
        let project_name = "test_project";
        let extensions = &["sql", "yml"];

        // Create mock file paths with timestamps
        let now = SystemTime::now();
        let mut all_paths: HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>> = HashMap::new();

        // Add test files - paths include test directory name as first component
        let test_files = vec![
            (DbtPath::from("tests/test_model.sql"), now),
            (DbtPath::from("tests/integration/test_integration.sql"), now),
            (DbtPath::from("tests/generic/test_generic.sql"), now), // Should be excluded
            (DbtPath::from("tests/generic/nested/test_nested.sql"), now), // Should be excluded
            (DbtPath::from("tests/custom/test_custom.sql"), now),
            (DbtPath::from("tests/schema.yml"), now),
            (DbtPath::from("tests/generic/schema.yml"), now), // Should be excluded
            (DbtPath::from("data-tests/generic/is_even.sql"), now), // Should be excluded
            (DbtPath::from("data-tests/singular/my_test.sql"), now),
        ];

        all_paths.insert(ResourcePathKind::TestPaths, test_files);

        // Call the function under test
        let result = find_files_by_kind_and_extension(
            &in_dir,
            project_name,
            &ResourcePathKind::TestPaths,
            extensions,
            &all_paths,
        );

        // Verify results - should exclude 3 generic files
        assert_eq!(result.len(), 5, "Should have 6 non-generic test files");

        // Check that all returned files are not in generic directories
        for asset in &result {
            let components: Vec<_> = asset.path.components().collect();
            assert!(
                !(components.len() >= 2 && components[1].as_os_str() == "generic"),
                "File {:?} should not have 'generic' as second component",
                asset.path
            );
        }

        // Check specific files that should be included
        let included_paths: Vec<&PathBuf> = result.iter().map(|asset| &asset.path).collect();
        assert!(included_paths.contains(&&PathBuf::from("tests/test_model.sql")));
        assert!(included_paths.contains(&&PathBuf::from("tests/integration/test_integration.sql")));
        assert!(included_paths.contains(&&PathBuf::from("tests/custom/test_custom.sql")));
        assert!(included_paths.contains(&&PathBuf::from("tests/schema.yml")));
        assert!(included_paths.contains(&&PathBuf::from("data-tests/singular/my_test.sql")));

        // Check that generic files are excluded
        assert!(!included_paths.contains(&&PathBuf::from("tests/generic/test_generic.sql")));
        assert!(!included_paths.contains(&&PathBuf::from("tests/generic/nested/test_nested.sql")));
        assert!(!included_paths.contains(&&PathBuf::from("tests/generic/schema.yml")));
        assert!(!included_paths.contains(&&PathBuf::from("data-tests/generic/is_even.sql")));

        // Verify asset properties
        for asset in &result {
            assert_eq!(asset.package_name, project_name);
            assert_eq!(asset.base_path, in_dir);
        }
    }

    #[test]
    fn test_load_dbtignore() {
        use tempfile::TempDir;

        // Create a temporary directory
        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path();

        // Initially no .dbtignore file
        let ignore = load_dbtignore(temp_path).unwrap();
        assert!(ignore.is_none());

        // Create a .dbtignore file
        let dbtignore_path = temp_path.join(".dbtignore");
        let mut file = File::create(dbtignore_path).unwrap();
        writeln!(file, "*.py").unwrap();
        writeln!(file, "/ignored_dir/").unwrap(); // Explicit directory format with slashes
        writeln!(file, "!important.py").unwrap();

        // Now .dbtignore should be loaded
        let ignore = load_dbtignore(temp_path).unwrap();
        assert!(ignore.is_some());

        let ignore = ignore.unwrap();

        // Test patterns
        // Test patterns with file=false parameter
        assert!(ignore.matched("test.py", false).is_ignore()); // Should be ignored
        assert!(!ignore.matched("important.py", false).is_ignore()); // Should NOT be ignored (negated)

        // For this test, let's focus on the patterns we know should work reliably
        assert!(ignore.matched("test.py", false).is_ignore()); // Should be ignored
        assert!(!ignore.matched("important.py", false).is_ignore()); // Should NOT be ignored (negated)
        assert!(!ignore.matched("test.txt", false).is_ignore()); // Should NOT be ignored
    }

    #[test]
    fn test_should_exclude_path_function() {
        // Test the should_exclude_path function directly

        // Test paths should exclude generic directories (second component)
        assert!(should_exclude_path(
            &ResourcePathKind::TestPaths,
            &PathBuf::from("tests/generic/test.sql")
        ));

        assert!(should_exclude_path(
            &ResourcePathKind::TestPaths,
            &PathBuf::from("data-tests/generic/test.sql")
        ));

        assert!(should_exclude_path(
            &ResourcePathKind::TestPaths,
            &PathBuf::from("tests/generic/nested/test.sql")
        ));

        // Test paths should NOT exclude non-generic directories
        assert!(!should_exclude_path(
            &ResourcePathKind::TestPaths,
            &PathBuf::from("tests/integration/test.sql")
        ));

        assert!(!should_exclude_path(
            &ResourcePathKind::TestPaths,
            &PathBuf::from("tests/unit/test.sql")
        ));

        assert!(!should_exclude_path(
            &ResourcePathKind::TestPaths,
            &PathBuf::from("data-tests/singular/test.sql")
        ));

        assert!(!should_exclude_path(
            &ResourcePathKind::TestPaths,
            &PathBuf::from("tests/unit/generic/test.sql") // generic is not second component
        ));

        // Edge cases
        assert!(!should_exclude_path(
            &ResourcePathKind::TestPaths,
            &PathBuf::from("generic/test.sql") // only one component
        ));

        assert!(!should_exclude_path(
            &ResourcePathKind::TestPaths,
            &PathBuf::from("tests") // only one component
        ));

        // Non-test paths should never exclude generic directories
        assert!(!should_exclude_path(
            &ResourcePathKind::ModelPaths,
            &PathBuf::from("models/generic/model.sql")
        ));

        assert!(!should_exclude_path(
            &ResourcePathKind::MacroPaths,
            &PathBuf::from("macros/generic/macro.sql")
        ));

        assert!(!should_exclude_path(
            &ResourcePathKind::SeedPaths,
            &PathBuf::from("seeds/generic/seed.csv")
        ));
    }

    fn dbt_properties(paths: &[&str]) -> Vec<DbtAsset> {
        paths
            .iter()
            .map(|path| DbtAsset {
                package_name: "test_project".to_string(),
                base_path: PathBuf::from("/project"),
                path: PathBuf::from(path),
                original_path: PathBuf::from(path),
            })
            .collect()
    }

    fn surviving_paths(properties: &[DbtAsset]) -> Vec<PathBuf> {
        properties.iter().map(|asset| asset.path.clone()).collect()
    }

    #[test]
    fn test_drop_nested_project_config_files_excludes_nested_project_root() {
        let mut properties = dbt_properties(&[
            "packages/lib/dbt_project.yml",
            "packages/lib/packages.yml",
            "packages/lib/package-lock.yml",
            "packages/lib/macros/schema.yml",
        ]);

        drop_nested_project_config_files(&mut properties);

        // The nested project's config files drop out; its properties files still belong to us.
        assert_eq!(
            surviving_paths(&properties),
            vec![PathBuf::from("packages/lib/macros/schema.yml")]
        );
    }

    #[test]
    fn test_drop_nested_project_config_files_keeps_properties_without_sibling_project() {
        let paths = [
            "models/packages.yml",
            "models/nested/dependencies.yml",
            "models/schema.yml",
        ];
        let mut properties = dbt_properties(&paths);

        drop_nested_project_config_files(&mut properties);

        // Without a sibling dbt_project.yml these are ordinary properties documents.
        assert_eq!(
            surviving_paths(&properties),
            paths.iter().map(PathBuf::from).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_drop_nested_project_config_files_excludes_root_project_configs() {
        let mut properties = dbt_properties(&[
            "dbt_project.yml",
            "profiles.yml",
            "selectors.yml",
            "dependencies.yml",
            "catalogs.yml",
            "vars.yml",
            "one.yml",
        ]);

        drop_nested_project_config_files(&mut properties);

        // A resource path resolving to the project root, e.g. `model-paths: ["."]`.
        assert_eq!(surviving_paths(&properties), vec![PathBuf::from("one.yml")]);
    }

    #[test]
    fn test_drop_nested_project_config_files_excludes_project_yml_without_siblings() {
        let mut properties = dbt_properties(&["models/dbt_project.yml", "models/schema.yml"]);

        drop_nested_project_config_files(&mut properties);

        // A file named dbt_project.yml is always project config, never properties.
        assert_eq!(
            surviving_paths(&properties),
            vec![PathBuf::from("models/schema.yml")]
        );
    }

    #[test]
    fn test_find_files_by_kind_and_extension_empty_paths() {
        // Test with empty paths
        let in_dir = PathBuf::from("/project");
        let project_name = "test_project";
        let extensions = &["sql"];
        let all_paths: HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>> = HashMap::new();

        let result = find_files_by_kind_and_extension(
            &in_dir,
            project_name,
            &ResourcePathKind::TestPaths,
            extensions,
            &all_paths,
        );

        assert!(
            result.is_empty(),
            "Should return empty vector for empty paths"
        );
    }

    #[test]
    fn test_find_files_by_kind_and_extension_extension_filtering() {
        // Test that only files with specified extensions are included
        let in_dir = PathBuf::from("/project");
        let project_name = "test_project";
        let extensions = &["sql"]; // Only SQL files

        let now = SystemTime::now();
        let mut all_paths: HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>> = HashMap::new();

        let test_files = vec![
            (DbtPath::from("tests/test.sql"), now), // Should be included
            (DbtPath::from("tests/test.yml"), now), // Should be excluded (wrong extension)
            (DbtPath::from("tests/test.py"), now),  // Should be excluded (wrong extension)
            (DbtPath::from("tests/test"), now),     // Should be excluded (no extension)
        ];

        all_paths.insert(ResourcePathKind::TestPaths, test_files);

        let result = find_files_by_kind_and_extension(
            &in_dir,
            project_name,
            &ResourcePathKind::TestPaths,
            extensions,
            &all_paths,
        );

        assert_eq!(result.len(), 1, "Should only include SQL files");
        assert_eq!(result[0].path, PathBuf::from("tests/test.sql"));
    }

    #[test]
    fn test_find_files_by_kind_and_extension_non_test_paths_not_excluded() {
        // Setup test data for non-test paths (should not exclude generic directories)
        let in_dir = PathBuf::from("/project");
        let project_name = "test_project";
        let extensions = &["sql"];

        let now = SystemTime::now();
        let mut all_paths: HashMap<ResourcePathKind, Vec<(DbtPath, SystemTime)>> = HashMap::new();

        // Add model files with generic in path (should NOT be excluded for models)
        let model_files = vec![
            (DbtPath::from("models/generic/my_model.sql"), now),
            (DbtPath::from("models/other/model.sql"), now),
        ];

        all_paths.insert(ResourcePathKind::ModelPaths, model_files);

        // Call the function under test for ModelPaths
        let result = find_files_by_kind_and_extension(
            &in_dir,
            project_name,
            &ResourcePathKind::ModelPaths,
            extensions,
            &all_paths,
        );

        // Verify that generic directories are NOT excluded for non-test paths
        assert_eq!(
            result.len(),
            2,
            "Should have 2 model files including generic path"
        );

        let included_paths: Vec<&PathBuf> = result.iter().map(|asset| &asset.path).collect();
        assert!(included_paths.contains(&&PathBuf::from("models/generic/my_model.sql")));
        assert!(included_paths.contains(&&PathBuf::from("models/other/model.sql")));
    }

    fn yaml_value(src: &str) -> dbt_yaml::Value {
        dbt_yaml::from_str(src).expect("valid yaml")
    }

    #[test]
    fn merge_vars_deep_merges_nested_mappings() {
        // vars.yml: { pkg: { x: 1 } }, CLI: { pkg: { y: 2 } }
        // Expected: { pkg: { x: 1, y: 2 } } — CLI must not clobber sibling keys
        // inside a package-scoped block defined in vars.yml.
        let mut file_vars: BTreeMap<String, dbt_yaml::Value> = BTreeMap::new();
        file_vars.insert("pkg".into(), yaml_value("x: 1"));

        let mut cli_vars: BTreeMap<String, dbt_yaml::Value> = BTreeMap::new();
        cli_vars.insert("pkg".into(), yaml_value("y: 2"));

        let merged = merge_vars(&file_vars, &cli_vars);
        let pkg = merged.get("pkg").expect("pkg key present");
        let mapping = pkg.as_mapping().expect("pkg is a mapping");
        let x = mapping
            .get(dbt_yaml::Value::from("x"))
            .expect("x preserved from vars.yml");
        let y = mapping
            .get(dbt_yaml::Value::from("y"))
            .expect("y added from CLI");
        assert_eq!(x.as_i64(), Some(1));
        assert_eq!(y.as_i64(), Some(2));
    }

    #[test]
    fn merge_vars_cli_overrides_overlapping_inner_keys() {
        // Same inner key in both → CLI wins.
        let mut file_vars: BTreeMap<String, dbt_yaml::Value> = BTreeMap::new();
        file_vars.insert("pkg".into(), yaml_value("x: 1\ny: from_file"));

        let mut cli_vars: BTreeMap<String, dbt_yaml::Value> = BTreeMap::new();
        cli_vars.insert("pkg".into(), yaml_value("y: from_cli"));

        let merged = merge_vars(&file_vars, &cli_vars);
        let mapping = merged.get("pkg").unwrap().as_mapping().unwrap();
        assert_eq!(
            mapping
                .get(dbt_yaml::Value::from("x"))
                .and_then(|v| v.as_i64()),
            Some(1)
        );
        assert_eq!(
            mapping
                .get(dbt_yaml::Value::from("y"))
                .and_then(|v| v.as_str()),
            Some("from_cli")
        );
    }

    #[test]
    fn merge_vars_cli_scalar_replaces_file_mapping() {
        // Type mismatch (CLI scalar vs file mapping) → CLI replaces wholesale.
        let mut file_vars: BTreeMap<String, dbt_yaml::Value> = BTreeMap::new();
        file_vars.insert("pkg".into(), yaml_value("x: 1"));

        let mut cli_vars: BTreeMap<String, dbt_yaml::Value> = BTreeMap::new();
        cli_vars.insert("pkg".into(), yaml_value("42"));

        let merged = merge_vars(&file_vars, &cli_vars);
        assert_eq!(merged.get("pkg").and_then(|v| v.as_i64()), Some(42));
    }

    #[test]
    fn vars_data_from_root_errors_on_non_mapping_vars() {
        // `vars: 1` should surface as a YAML type error rather than silently
        // returning an empty map (mirrors how `dbt_project.yml` rejects this).
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(DBT_VARS_YML);
        fs::write(&path, "vars: 1\n").unwrap();

        let err = vars_data_from_root(tmp.path()).expect_err("non-mapping vars value should error");
        let msg = format!("{err}");
        assert!(
            msg.contains("invalid type") || msg.contains("expected"),
            "expected a YAML type-mismatch error, got: {msg}"
        );
    }

    #[test]
    fn vars_data_from_root_returns_empty_when_no_vars_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(DBT_VARS_YML);
        fs::write(&path, "other: 1\n").unwrap();

        let map = vars_data_from_root(tmp.path()).expect("no vars key is OK");
        assert!(map.is_empty());
    }
}
