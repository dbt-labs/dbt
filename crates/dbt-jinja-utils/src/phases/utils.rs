use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::TargetContext;
use dbt_schemas::state::{DbtProfile, ModelStatus};
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

/// Lazily builds and reuses profile-derived relation contexts by adapter.
#[derive(Default)]
pub struct AdapterTargetContextCache {
    contexts: HashMap<AdapterType, AdapterTargetContext>,
}

impl AdapterTargetContextCache {
    /// Return the selected adapter's context. Disabled or failed nodes whose
    /// adapter is not configured use the default context so they remain
    /// parseable without making that adapter executable.
    pub fn get_for_node(
        &mut self,
        profile: &DbtProfile,
        selected_adapter: AdapterType,
        default_adapter: AdapterType,
        status: ModelStatus,
        base_context: &BTreeMap<String, MinijinjaValue>,
    ) -> FsResult<&AdapterTargetContext> {
        let context_adapter =
            if status == ModelStatus::Enabled || profile.adapter(selected_adapter).is_some() {
                selected_adapter
            } else {
                default_adapter
            };

        if !self.contexts.contains_key(&context_adapter) {
            self.contexts.insert(
                context_adapter,
                build_adapter_target_context(profile, context_adapter, base_context)?,
            );
        }

        Ok(self
            .contexts
            .get(&context_adapter)
            .expect("adapter target context was inserted"))
    }
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
    let database = config.get_database_or_default();
    let schema = if adapter_type == profile.default_adapter {
        profile.schema.clone()
    } else {
        config
            .get_schema()
            .cloned()
            .unwrap_or_else(|| "public".to_string())
    };
    let target_context = TargetContext::try_from(config.clone())
        .map_err(|e| fs_err!(ErrorCode::InvalidConfig, "{e}"))?;
    let mut target_context =
        build_target_context_map(&profile.profile, &profile.target, target_context);
    target_context.insert("database".to_string(), MinijinjaValue::from(&database));
    target_context.insert("schema".to_string(), MinijinjaValue::from(&schema));
    let target_context = Arc::new(target_context);
    let mut adapter_base_context = base_context.clone();
    adapter_base_context.insert(
        "target".to_string(),
        MinijinjaValue::from_serialize(Arc::clone(&target_context)),
    );
    adapter_base_context.insert(
        "env".to_string(),
        MinijinjaValue::from_serialize(target_context),
    );
    adapter_base_context.insert("database".to_string(), MinijinjaValue::from(&database));
    adapter_base_context.insert("schema".to_string(), MinijinjaValue::from(&schema));

    Ok(AdapterTargetContext {
        database,
        schema,
        base_context: adapter_base_context,
    })
}
