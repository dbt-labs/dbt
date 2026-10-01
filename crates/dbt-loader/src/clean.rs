use crate::{
    args::LoadArgs,
    dbt_project_yml_loader::{collect_protected_paths, load_project_yml},
    load_for_clean,
};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

use dbt_common::{
    ErrorCode, FsResult,
    cancellation::CancellationToken,
    constants::{DBT_INDEX_DIR_NAME, DBT_METADATA_DIR_NAME, DBT_PROJECT_YML, DBT_TARGET_DIR_NAME},
    err, fs_err,
    io_args::{EvalArgs, EvalArgsBuilder},
    lease,
    path::DbtPath,
    stdfs,
    tracing::{
        dbt_emit::{
            emit_error_log_from_fs_error, emit_info_progress_message, emit_trace_log_message,
        },
        dbt_metrics::error_count_checkpoint,
        event_info::store_event_attributes,
    },
};
use dbt_jinja_utils::{
    invocation_args::InvocationArgs, phases::load::init::initialize_load_jinja_environment,
};
use dbt_schemas::schemas::project::DbtProject;
use dbt_telemetry::{ExecutionPhase, PhaseExecuted, ProgressMessage};

#[tracing::instrument(
    skip_all,
    fields(
        _e = ?store_event_attributes(PhaseExecuted::start_general(ExecutionPhase::Clean)),
    )
)]
pub async fn execute_clean_command(
    arg: &EvalArgs,
    files: &[String],
    _token: &CancellationToken,
) -> FsResult<()> {
    let load_args = LoadArgs::from_eval_args(arg);
    let dbt_state = load_for_clean(&load_args).await?;
    let invocation_args = InvocationArgs::from_eval_args(arg);
    let flags: BTreeMap<String, minijinja::Value> = invocation_args.to_dict();

    let arg = EvalArgsBuilder::from_eval_args(arg)
        .with_threads(dbt_state.dbt_profile.threads)
        .build();

    let env = initialize_load_jinja_environment(
        &dbt_state.dbt_profile.profile,
        &dbt_state.dbt_profile.target,
        dbt_state.dbt_profile.default_db_config().adapter_type(),
        dbt_state.dbt_profile.default_db_config().clone(),
        dbt_state.run_started_at,
        &flags,
        invocation_args.warn_error_options.clone(),
        arg.io.clone(),
        dbt_state.catalogs,
    )?;

    let dbt_project_path = arg.io.in_dir.join(DBT_PROJECT_YML);
    let (dbt_project, _) = load_project_yml(&env, &dbt_project_path, None, arg.vars.clone())?;

    clean_project(&arg, files, &dbt_project, /* clean_targets */ true).await?;

    error_count_checkpoint()
}

pub async fn clean_project(
    arg: &EvalArgs,
    files: &[String],
    dbt_project: &DbtProject,
    clean_targets: bool,
) -> FsResult<()> {
    let target_guard = lease::acquire_lease(&arg.io.in_dir, Path::new(DBT_TARGET_DIR_NAME)).await?;

    let protected_paths = collect_protected_paths(dbt_project)
        .iter()
        .map(|p| DbtPath::absolute(arg.io.in_dir.join(p)))
        .collect::<Result<Vec<_>, _>>()?;

    let default_relative_target_dir = DbtPath::from(DBT_TARGET_DIR_NAME);
    let mut paths_to_delete = dbt_project
        .clean_targets
        .as_ref()
        .unwrap()
        .iter()
        .filter(|_| clean_targets)
        .chain(files.iter())
        .map(|path| {
            let path = Path::new(path);
            if path.is_absolute() {
                err!(
                    ErrorCode::InvalidPath,
                    "Absolute paths are not allowed: {}",
                    path.display()
                )
            } else {
                DbtPath::absolute(arg.io.in_dir.join(path)).map_err(Into::into)
            }
        })
        .collect::<Result<HashSet<_>, _>>()?;

    let out_dir = DbtPath::from(&arg.io.out_dir);
    paths_to_delete.insert(out_dir.clone());
    // Leftover public copies from before index/metadata moved under `private/`.
    // Wiping `out_dir` already removes them; keep the paths explicit so a custom
    // `clean-targets` that does not delete the whole target still hides them.
    paths_to_delete.insert(out_dir.join(DBT_INDEX_DIR_NAME));
    paths_to_delete.insert(out_dir.join(DBT_METADATA_DIR_NAME));

    let all_safe = paths_to_delete.iter().all(|path_to_delete| {
        // The clean command does not delete anything outside of the project directory
        unrelated_paths(&arg.io.in_dir, path_to_delete)
            // The clean command does not delete protected directories ("models", "macros", etc.)
            && protected_paths
                .iter()
                .all(|protected_path| unrelated_paths(protected_path, path_to_delete))
    });

    if all_safe {
        let default_target_dir = DbtPath::from(&arg.io.in_dir).join(&default_relative_target_dir);

        let mut lease_guards = vec![];

        for path in &paths_to_delete {
            if path.exists() {
                if path.eq(&default_target_dir) {
                    // We have already acquired the lease for this directory at this point.
                    lease_guards.push((
                        path,
                        default_relative_target_dir.display().to_string(),
                        None,
                    ));
                } else {
                    let Some(relative_path) = path.get_relative_path(&arg.io.in_dir) else {
                        return Err(fs_err!(
                            ErrorCode::InvalidPath,
                            "Unable to get relative path for: {}",
                            path.display(),
                        ));
                    };
                    let lease_guard = lease::acquire_lease(&arg.io.in_dir, &relative_path).await?;
                    lease_guards.push((
                        path,
                        relative_path.display().to_string(),
                        Some(lease_guard),
                    ));
                }
            } else {
                emit_trace_log_message(|| {
                    format!("The target directory does not exist: {}", path.display())
                });
            }
        }

        // Sort lease guards by the display path we show in the terminal
        // for deterministic output.
        let lease_guards = {
            let mut lease_guards = lease_guards.into_iter().collect::<Vec<_>>();
            lease_guards.sort_by(|a, b| a.1.cmp(&b.1));
            lease_guards
        };

        for (path, display_path_string, _) in &lease_guards {
            emit_info_progress_message(ProgressMessage::new_from_action_and_target(
                "Removing".to_string(),
                display_path_string.to_string(),
            ));
            stdfs::remove_dir_all(path)?;
        }

        for (_, _, lease_guard) in lease_guards {
            if let Some(lease_guard) = lease_guard {
                // Explicitly release to predictably wait for the lease internally to drop.
                lease_guard.release().await;
            }
        }
    }

    // Explicitly release to predictably wait for the lease internally to drop.
    target_guard.release().await;

    // Remove the agent skills dbt installed into the provider directories. Only
    // dbt's own installs are touched; anything the user authored is left alone.
    if let Err(e) =
        dbt_skills::prune_installed_skills(&arg.io.in_dir, dbt_project, arg.ai_provider.as_deref())
    {
        // A skill-pruning problem should not fail `dbt clean`; warn and move on.
        emit_error_log_from_fs_error(*e);
    }

    Ok(())
}

fn unrelated_paths<P: AsRef<Path>, Q: AsRef<Path>>(to: P, from: Q) -> bool {
    match stdfs::diff_paths(&to, &from).and_then(|diff| {
        // It is safe to delete a directory if the only way to get to a protected directory is to navigate to the parent.
        if diff.components().next() == Some(std::path::Component::ParentDir) {
            Ok(true)
        } else {
            Err(fs_err!(
                ErrorCode::InvalidPath,
                "The target directory is protected: {}",
                from.as_ref().display()
            ))
        }
    }) {
        Ok(_) => true,
        Err(e) => {
            emit_error_log_from_fs_error(*e);
            false
        }
    }
}
