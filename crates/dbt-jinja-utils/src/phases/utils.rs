use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::TargetContext;
use dbt_schemas::state::DbtProfile;
use minijinja::value::Value as MinijinjaValue;

/// Profile-derived names and Jinja globals for one adapter.
pub struct AdapterTargetContext {
    /// Default database for relations executed by the adapter.
    pub database: String,
    /// Default schema for relations executed by the adapter.
    pub schema: String,
    /// Base context with `target`, `env`, `database`, and `schema` rebound.
    pub base_context: BTreeMap<String, MinijinjaValue>,
}

/// Convert a typed profile target into the mapping exposed to Jinja as
/// `target` and `env`.
pub fn build_target_context_map(
    profile: &str,
    target: &str,
    target_context: TargetContext,
) -> BTreeMap<String, MinijinjaValue> {
    let target_context_val = dbt_yaml::to_value(&target_context).unwrap();
    let mut target_context_map: BTreeMap<String, MinijinjaValue> =
        dbt_yaml::from_value(target_context_val).unwrap();
    target_context_map.insert("profile_name".to_string(), MinijinjaValue::from(profile));
    target_context_map.insert("name".to_string(), MinijinjaValue::from(target));
    target_context_map.insert("target_name".to_string(), MinijinjaValue::from(target));
    target_context_map
}

/// Rebind profile-derived Jinja globals to the selected adapter.
pub fn build_adapter_target_context(
    profile: &DbtProfile,
    adapter_type: AdapterType,
    base_context: &BTreeMap<String, MinijinjaValue>,
) -> FsResult<AdapterTargetContext> {
    let config = profile.adapter(adapter_type).ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidConfig,
            "no profile connection is configured for adapter '{adapter_type}'"
        )
    })?;
    let target_context = TargetContext::try_from(config.clone())
        .map_err(|e| fs_err!(ErrorCode::InvalidConfig, "{e}"))?;
    let target_context = Arc::new(build_target_context_map(
        &profile.profile,
        &profile.target,
        target_context,
    ));
    let mut adapter_base_context = base_context.clone();
    adapter_base_context.insert(
        "target".to_string(),
        MinijinjaValue::from_serialize(Arc::clone(&target_context)),
    );
    adapter_base_context.insert(
        "env".to_string(),
        MinijinjaValue::from_serialize(target_context),
    );
    adapter_base_context.insert(
        "database".to_string(),
        MinijinjaValue::from(config.get_database().cloned()),
    );
    adapter_base_context.insert(
        "schema".to_string(),
        MinijinjaValue::from(config.get_schema().cloned()),
    );

    Ok(AdapterTargetContext {
        database: config.get_database_or_default(),
        schema: config.get_schema().cloned().unwrap_or_default(),
        base_context: adapter_base_context,
    })
}
