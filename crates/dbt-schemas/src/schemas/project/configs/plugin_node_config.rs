use dbt_proc_macros::{DefaultTo, Resolvable};
use dbt_yaml::DbtSchema;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use crate::schemas::project::ResolvableConfig;
use crate::schemas::project::configs::config_merge::Tags;
use crate::schemas::serde::bool_or_string_bool;

/// A plugin node's `config:`: what dbt itself reads of it.
// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(
    Resolvable, DefaultTo, Deserialize, Serialize, Debug, Clone, Default, DbtSchema, PartialEq,
)]
pub struct PluginNodeConfig {
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(serialize_with = "crate::schemas::serde::serialize_none_as_empty_map")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(default)]
    pub tags: Tags,
}

impl ResolvableConfig<PluginNodeConfig> for PluginNodeConfig {
    type Resolved = ResolvedPluginNodeConfig;
    type PackageDefaults = ();
    type ResolveDefaults = ();

    fn get_enabled_with_default(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn disable(&mut self) {
        self.enabled = Some(false);
    }

    fn apply_package_defaults(&mut self, _: ()) {}

    fn finalize(self) -> ResolvedPluginNodeConfig {
        self.finalize_resolved()
    }

    fn default_to(&mut self, parent: &PluginNodeConfig) {
        self.default_to_fields(parent);
    }
}
