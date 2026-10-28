use std::collections::BTreeMap;

use dbt_schemas::schemas::profiles::{DbConfig, TargetContext};
use minijinja::value::Value as MinijinjaValue;

/// The parse env globals a connection sets: `target` (also `env`), `database`, `schema`.
pub struct TargetGlobals {
    pub target: BTreeMap<String, MinijinjaValue>,
    pub database: Option<String>,
    pub schema: Option<String>,
}

/// The [`TargetGlobals`] of `db_config` as target `target` of `profile`.
pub fn build_target_globals(
    profile: &str,
    target: &str,
    db_config: DbConfig,
) -> Result<TargetGlobals, String> {
    let database = db_config.get_database().cloned();
    let schema = db_config.get_schema().cloned();
    let target_context = TargetContext::try_from(db_config)?;
    Ok(TargetGlobals {
        target: build_target_context_map(profile, target, target_context),
        database,
        schema,
    })
}

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
