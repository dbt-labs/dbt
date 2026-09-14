use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_cloud_config::resolve_cloud_config;
use dbt_common::cancellation::CancellationToken;
use dbt_common::constants::DBT_PROJECT_YML;
use dbt_common::io_args::EvalArgs;
use dbt_common::tracing::TracingConfigProvider;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::tracing::dbt_metrics::error_count_checkpoint;
use dbt_common::warn_error_options::WarnErrorOptions;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_utils::Var;
use dbt_jinja_utils::phases::load::init::initialize_load_profile_jinja_environment;
use dbt_jinja_utils::phases::load::secret_renderer::secret_context_env_var;
use dbt_jinja_utils::serde::{into_typed_with_jinja, value_from_file};
use dbt_schemas::schemas::project::DbtProjectSimplified;
use fs_deps::get_or_install_packages;
use fs_deps::private_package::PrivatePackageResolver;

use crate::args::LoadArgs;
use crate::loader::{
    get_packages_install_path, resolve_and_reload_weo_from_project, resolve_bool_project_flag,
};

/// Execute `dbt deps` without loading a profile.
///
/// Reads `dbt_project.yml` (to determine the packages install path) and,
/// opportunistically, `dbt_cloud.yml`, then delegates to
/// `get_or_install_packages`. No `profiles.yml` lookup is performed,
/// matching dbt-core's behaviour where `dbt deps` does not require a valid
/// profile.
pub async fn execute_deps_command(
    arg: &EvalArgs,
    cli_warn_error: Option<bool>,
    cli_warn_error_options: Option<WarnErrorOptions>,
    tracing_features: Option<&dyn TracingConfigProvider>,
    token: &CancellationToken,
    private_package_resolver: Arc<dyn PrivatePackageResolver>,
) -> FsResult<()> {
    let load_args = LoadArgs::from_eval_args(arg);

    // Read dbt_project.yml without loading profiles.yml.
    let simplified_dbt_project = load_simplified_project_only(&load_args)?;
    let _resolved_warn_error_options = resolve_and_reload_weo_from_project(
        &simplified_dbt_project,
        cli_warn_error,
        cli_warn_error_options.as_ref(),
        tracing_features,
    )?;

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

    let (packages_install_path, _internal_packages_install_path) = get_packages_install_path(
        &load_args.io.in_dir,
        &load_args.packages_install_path,
        &load_args.internal_packages_install_path,
        &simplified_dbt_project,
    );

    // A minimal Jinja environment is sufficient for rendering any
    // `{{ env_var(...) }}` calls that may appear in packages.yml.
    // No profile context is needed.
    let env = initialize_load_profile_jinja_environment();

    let use_v2_compatible_package_downloads = resolve_bool_project_flag(
        load_args.io.use_v2_compatible_package_downloads,
        simplified_dbt_project.flags.as_ref(),
        "use_v2_compatible_package_downloads",
    );
    let require_hub_verified_downloads = resolve_bool_project_flag(
        load_args.io.require_hub_verified_downloads,
        simplified_dbt_project.flags.as_ref(),
        "require_hub_verified_downloads",
    );

    get_or_install_packages(
        &load_args.io,
        load_args.command,
        &env,
        &packages_install_path,
        true, // install_deps
        load_args.add_package,
        load_args.upgrade,
        load_args.lock,
        load_args.vars,
        load_args.version_check,
        load_args.skip_private_deps,
        None, // replay_mode
        token,
        use_v2_compatible_package_downloads,
        require_hub_verified_downloads,
        private_package_resolver,
        cloud_config,
        load_args.ai_provider.as_deref(),
    )
    .await?;

    error_count_checkpoint()
}

/// Parse `dbt_project.yml` without touching `profiles.yml`.
///
/// This is the first half of `load_simplified_project_and_profiles`, stopping
/// before the `load_profiles` call.
fn load_simplified_project_only(arg: &LoadArgs) -> FsResult<DbtProjectSimplified> {
    let dbt_project_path = arg.io.in_dir.join(DBT_PROJECT_YML);
    let raw = value_from_file(&dbt_project_path, false, None)?;

    let env = initialize_load_profile_jinja_environment();
    let ctx: BTreeMap<String, minijinja::Value> = BTreeMap::from([
        (
            "env_var".to_owned(),
            minijinja::Value::from_func_func("env_var", secret_context_env_var),
        ),
        (
            "var".to_owned(),
            minijinja::Value::from_object(Var::new(arg.vars.clone())),
        ),
        (
            // Empty context object — mirrors dbt-core's BaseContext.to_dict()
            // pattern so that `context.project_name or ''` in profiles.yml
            // doesn't break if someone ever references it.
            "context".to_owned(),
            minijinja::Value::from_serialize(BTreeMap::<String, minijinja::Value>::new()),
        ),
    ]);

    let simplified_dbt_project: DbtProjectSimplified =
        into_typed_with_jinja(raw, true, &env, &ctx, &[], None, true)?;

    if simplified_dbt_project.source_paths.is_some() {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "'source-paths' cannot be specified in dbt_project.yml",
        ));
    }
    if (*simplified_dbt_project.log_path)
        .as_ref()
        .is_some_and(|path| path != "logs")
    {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "'log-path' cannot be specified in dbt_project.yml",
        ));
    }
    if (*simplified_dbt_project.target_path)
        .as_ref()
        .is_some_and(|path| path != "target")
    {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "'target-path' cannot be specified in dbt_project.yml",
        ));
    }

    Ok(simplified_dbt_project)
}
