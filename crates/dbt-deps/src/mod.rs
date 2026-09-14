mod steps;

mod add_package;
mod context;
pub(crate) mod git_client;
mod hub_client;
mod network_client;
mod notices;
mod package_installer;
pub mod package_listing;
mod package_resolver;

pub mod private_package;
pub mod semver;
mod tarball_client;
pub mod types;
pub mod utils;

use dbt_common::cancellation::CancellationToken;
use dbt_common::constants::{DBT_PACKAGES_LOCK_FILE, DBT_PROJECT_YML};
use dbt_common::io_args::{FsCommand, IoArgs, ReplayMode, TimeMachineMode};
use dbt_common::path::DbtPath;
use dbt_common::tracing::dbt_emit::{emit_info_log_message, emit_info_progress_message};
use dbt_common::tracing::span_info::{
    SpanStatusRecorder as _, record_span_status, record_span_status_with_attrs,
};
use dbt_common::{ErrorCode, FsResult, err, stdfs};
use dbt_common::{create_info_span, fs_err, lease};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::schemas::ResolvedCloudConfig;
use dbt_schemas::schemas::packages::{DbtPackagesLock, UpstreamProject};
use dbt_telemetry::{DepsAllPackagesInstalled, GenericOpExecuted};
use std::sync::Arc;
use std::{collections::BTreeMap, path::Path};
use steps::{
    SkillInstallInputs, compute_package_lock, install_packages, install_skills, load_dbt_packages,
    load_dbt_packages_lock_without_validation, try_load_valid_dbt_packages_lock,
};
use tracing::Instrument as _;

use crate::context::DepsOperationContext;
use crate::private_package::PrivatePackageResolver;

/// Loads and installs packages, and returns the packages lock and the dependencies map
#[allow(clippy::cognitive_complexity, clippy::too_many_arguments)]
pub async fn get_or_install_packages(
    io: &IoArgs,
    command: FsCommand,
    env: &JinjaEnv,
    packages_install_path: &Path,
    install_deps: bool,
    add_package: Option<String>,
    upgrade: bool,
    lock: bool,
    vars: BTreeMap<String, dbt_yaml::Value>,
    version_check: bool,
    skip_private_deps: bool,
    replay_mode: Option<&ReplayMode>,
    token: &CancellationToken,
    use_v2_compatible_package_downloads: bool,
    require_hub_verified_downloads: bool,
    private_package_resolver: Arc<dyn PrivatePackageResolver>,
    cloud_config: Option<ResolvedCloudConfig>,
    ai_provider: Option<&[String]>,
) -> FsResult<(DbtPackagesLock, Vec<UpstreamProject>)> {
    let Some(packages_relative_dir) =
        DbtPath::from(packages_install_path).get_relative_path(&io.in_dir)
    else {
        return Err(fs_err!(
            ErrorCode::InvalidPath,
            "Unable to get packages relative path for: {}",
            packages_install_path.display(),
        ));
    };
    let _lease_guard = lease::acquire_lease(&io.in_dir, &packages_relative_dir).await?;

    // In Time Machine replay mode, skip all package fetching/installation
    // We should only load the existing package-lock.yml from disk
    let is_time_machine_replay = matches!(
        replay_mode,
        Some(ReplayMode::FsTimeMachine(TimeMachineMode::Replay(_)))
    );

    if is_time_machine_replay {
        // Just load the existing package-lock.yml without any validation or fetching
        let dbt_packages_lock =
            load_dbt_packages_lock_without_validation(io, packages_install_path, env, &vars)
                .await?
                .unwrap_or_default();

        emit_info_progress_message(dbt_telemetry::ProgressMessage::new_from_action_and_target(
            "Loading".to_string(),
            "package-lock.yml".to_string(),
        ));

        // Return empty upstream projects since we're not fetching anything
        return Ok((dbt_packages_lock, vec![]));
    }

    let deps_context = DepsOperationContext::from_entry(
        io,
        &vars,
        env,
        token,
        skip_private_deps,
        version_check,
        use_v2_compatible_package_downloads,
        require_hub_verified_downloads,
        private_package_resolver,
        cloud_config,
    );

    // Add package first if specified, then load the package definition
    if let Some(add_package) = add_package {
        add_package::add_package(&add_package, &io.in_dir)?;
    }

    // This gets the package entries from packages.yml or dependencies.yml
    let (package_def, package_yml_name) = load_dbt_packages(&io.in_dir).await?;

    let dbt_packages_lock = if let Some(ref dbt_packages) = package_def {
        deps_context.notices.collect(dbt_packages);

        let try_cached_lock = if !upgrade && !lock {
            try_load_valid_dbt_packages_lock(
                io,
                packages_install_path,
                dbt_packages,
                env,
                &vars,
                use_v2_compatible_package_downloads,
            )
            .await
            .inspect_err(|_| {
                deps_context.flush_notices(&DbtPackagesLock::default());
            })?
        } else {
            None
        };

        if let Some(dbt_packages_lock) = try_cached_lock {
            emit_info_progress_message(dbt_telemetry::ProgressMessage::new_from_action_and_target(
                "Loading".to_string(),
                package_yml_name.to_string(),
            ));
            dbt_packages_lock
        } else {
            let fetch_span = create_info_span(GenericOpExecuted::new(
                "deps-compute-package-lock".to_string(),
                format!("resolving packages from {}", package_yml_name),
                None,
            ));
            let lock_result = compute_package_lock(&deps_context, dbt_packages)
                .instrument(fetch_span.clone())
                .await;

            match lock_result {
                Ok(lock) => {
                    record_span_status_with_attrs(
                        &fetch_span,
                        |attrs| {
                            if let Some(g) = attrs.downcast_mut::<GenericOpExecuted>() {
                                g.item_count_total = Some(lock.packages.len() as u64);
                            }
                        },
                        None,
                    );
                    lock
                }
                Err(e) => {
                    record_span_status(&fetch_span, Some(&e.to_string()));
                    deps_context.flush_notices(&DbtPackagesLock::default());
                    return Err(e);
                }
            }
        }
    } else {
        // No packages.yml defined - try to load from package-lock.yml if it exists
        // This matches dbt-core behavior where the lock file can be used independently

        // If upgrade flag is set but no packages.yml exists, we can't upgrade
        if upgrade {
            use dbt_common::tracing::dbt_emit::emit_warn_log_message;
            emit_warn_log_message(
                ErrorCode::InvalidConfig,
                "Cannot upgrade packages without packages.yml or dependencies.yml. Using existing package-lock.yml.",
            );
        }

        if let Some(dbt_packages_lock) =
            load_dbt_packages_lock_without_validation(io, packages_install_path, env, &vars).await?
        {
            emit_info_progress_message(dbt_telemetry::ProgressMessage::new_from_action_and_target(
                "Loading".to_string(),
                "package-lock.yml".to_string(),
            ));
            dbt_packages_lock
        } else {
            // No packages.yml and no valid package-lock.yml - return empty
            DbtPackagesLock::default()
        }
    };

    if install_deps && !lock && !dbt_packages_lock.packages.is_empty() {
        // Start install span. Note that actual package count may end up being less
        // then in the package lock due to package incorporation logic
        let install_span = create_info_span(DepsAllPackagesInstalled::start(
            dbt_packages_lock.packages.len() as u64,
        ));

        install_span.in_scope(|| {
            // check if the packages install path exists
            if !packages_install_path.exists() {
                // Create the directory
                stdfs::create_dir_all(packages_install_path).unwrap();
            }
        });

        install_packages(&deps_context, &dbt_packages_lock, packages_install_path)
            .instrument(install_span.clone())
            .await
            .record_status(&install_span)
            .inspect_err(|_| {
                deps_context.flush_notices(&dbt_packages_lock);
            })?;
    }

    // A package is considered "missing" if the 'dbt_project.yml' file for that
    // package does not exist.
    let mut missing_packages = Vec::new();
    if !lock {
        for package in dbt_packages_lock.packages.iter() {
            if !packages_install_path
                .join(package.package_name())
                .join(DBT_PROJECT_YML)
                .exists()
            {
                missing_packages.push(package.package_name());
            }
        }
    }
    let mut missing_packages_after_auto_install = Vec::new();

    // Auto install missing packages if not installing deps
    if !lock && !missing_packages.is_empty() {
        if !install_deps {
            emit_auto_install_without_lock_info(io, command);

            // Start install span. Note that actual package count may end up being less
            // then in the package lock due to package incorporation logic
            let install_span = create_info_span(DepsAllPackagesInstalled::start(
                dbt_packages_lock.packages.len() as u64,
            ));

            install_span.in_scope(|| {
                // check if the packages install path exists
                if !packages_install_path.exists() {
                    // Create the directory
                    stdfs::create_dir_all(packages_install_path).unwrap();
                }
            });

            install_packages(&deps_context, &dbt_packages_lock, packages_install_path)
                .instrument(install_span.clone())
                .await
                .record_status(&install_span)
                .inspect_err(|_| {
                    deps_context.flush_notices(&dbt_packages_lock);
                })?;

            for package in dbt_packages_lock.packages.iter() {
                if !packages_install_path.join(package.package_name()).exists() {
                    missing_packages_after_auto_install.push(package.package_name());
                }
            }
            if !missing_packages_after_auto_install.is_empty() {
                deps_context.flush_notices(&dbt_packages_lock);
                return err!(
                    ErrorCode::InvalidConfig,
                    "The following packages are missing from the packages install path: {:?}. Check you package definition and run 'fs deps' to install the missing packages.",
                    missing_packages_after_auto_install.join(", ")
                );
            }
        } else {
            deps_context.flush_notices(&dbt_packages_lock);
            return err!(
                ErrorCode::InvalidConfig,
                "The following packages are missing from the packages install path: {:?}. Check you package definition and run 'fs deps' to install the missing packages.",
                missing_packages.join(", ")
            );
        }
    }

    deps_context.flush_notices(&dbt_packages_lock);

    // Skills ship inside packages, so install them at the same moment the
    // packages land. Skipped in lock-only mode, where nothing was unpacked. A
    // skill-name collision is the one skill problem that fails `dbt deps`.
    if !lock {
        install_skills(SkillInstallInputs {
            in_dir: &io.in_dir,
            packages_install_path,
            dbt_packages_lock: &dbt_packages_lock,
            ai_provider,
        })?;
    }

    Ok((
        dbt_packages_lock,
        package_def.map(|p| p.projects).unwrap_or_default(),
    ))
}

fn emit_auto_install_without_lock_info(io: &IoArgs, command: FsCommand) {
    if io.in_dir.join(DBT_PACKAGES_LOCK_FILE).exists() {
        return;
    }

    let command = command.as_str();
    let command = if command.is_empty() {
        "dbt".to_string()
    } else {
        format!("dbt {command}")
    };

    emit_info_log_message(format!(
        "Auto installing dependencies while running `{command}`. To ensure dependencies are cached, run `dbt deps` to initialize a valid package-lock.yml."
    ));
}
