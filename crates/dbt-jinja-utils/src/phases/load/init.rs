//! This module contains the functions for initializing the Jinja environment for the load phase.

use std::{collections::BTreeMap, sync::Arc};

use chrono::DateTime;
use chrono_tz::Tz;
use dbt_adapter::{Adapter, sql_types::DefaultTypeOps};

use dbt_adapter_core::AdapterType;
use dbt_common::{
    ErrorCode, FsResult, fs_err, io_args::IoArgs, warn_error_options::WarnErrorOptions,
};
use dbt_jinja_ctx::{LoadCtx, to_jinja_btreemap};
use dbt_schemas::{
    dbt_utils::resolve_package_quoting,
    schemas::dbt_catalogs_deprecated::DbtCatalogs,
    schemas::profiles::{DbConfig, TargetContext},
};
use minijinja::value::Value as MinijinjaValue;

use crate::{
    environment_builder::JinjaEnvBuilder, jinja_environment::JinjaEnv,
    phases::utils::build_target_context_map,
};

/// Initialize load_profile jinja environment
pub fn initialize_load_profile_jinja_environment() -> JinjaEnv {
    JinjaEnvBuilder::new().build()
}

/// Initialize a Jinja environment for the load phase.
#[allow(clippy::too_many_arguments)]
pub fn initialize_load_jinja_environment(
    profile: &str,
    target: &str,
    adapter_type: AdapterType,
    db_config: DbConfig,
    run_started_at: DateTime<Tz>,
    flags: &BTreeMap<String, MinijinjaValue>,
    warn_error_options: WarnErrorOptions,
    io_args: IoArgs,
    catalogs: Option<Arc<DbtCatalogs>>,
) -> FsResult<JinjaEnv> {
    let adapter_config_mapping = db_config.to_mapping().unwrap();
    let target_context = TargetContext::try_from(db_config)
        .map_err(|e| fs_err!(ErrorCode::InvalidConfig, "{}", &e))?;
    let target_context = Arc::new(build_target_context_map(profile, target, target_context));

    let package_quoting = resolve_package_quoting(None, adapter_type);
    let type_ops = Arc::new(DefaultTypeOps::new(adapter_type));

    // TODO: change this to use the AdapterFactory instead
    let adapter = Adapter::new_parse_phase_adapter(
        adapter_type,
        adapter_config_mapping,
        package_quoting,
        type_ops,
        catalogs,
    );

    let load_ctx = LoadCtx::new(run_started_at, target_context, flags.clone());

    Ok(JinjaEnvBuilder::new()
        .with_adapter(Arc::new(adapter))
        .with_root_package("dbt".to_string())
        .with_globals(to_jinja_btreemap(&load_ctx))
        .with_warn_error_options(warn_error_options)
        .with_io_args(io_args)
        .build())
}
