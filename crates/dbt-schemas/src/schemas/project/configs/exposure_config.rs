use crate::schemas::project::TypedRecursiveConfig;
use dbt_proc_macros::Resolvable;
use dbt_yaml::{DbtSchema, ShouldBe};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::btree_map::Iter;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use crate::schemas::project::configs::config_merge::Tags;
use crate::schemas::{
    project::ResolvableConfig,
    serde::{StringOrArrayOfStrings, bool_or_string_bool},
};
use dbt_proc_macros::DefaultTo;

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq)]
pub struct ProjectExposureConfig {
    #[serde(rename = "+meta")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+tags")]
    pub tags: Option<StringOrArrayOfStrings>,
    #[serde(default, rename = "+enabled", deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    pub __additional_properties__: BTreeMap<String, ShouldBe<ProjectExposureConfig>>,
}

impl TypedRecursiveConfig for ProjectExposureConfig {
    fn type_name() -> &'static str {
        "exposure"
    }

    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>> {
        self.__additional_properties__.iter()
    }

    fn has_set_fields(&self) -> bool {
        self.meta.is_some() || self.tags.is_some() || self.enabled.is_some()
    }
}

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(
    Resolvable, DefaultTo, Deserialize, Serialize, Debug, Clone, Default, DbtSchema, PartialEq,
)]
pub struct ExposureConfig {
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(serialize_with = "crate::schemas::serde::serialize_none_as_empty_map")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(default)]
    pub tags: Tags,
}

impl From<ProjectExposureConfig> for ExposureConfig {
    fn from(config: ProjectExposureConfig) -> Self {
        Self {
            enabled: config.enabled,
            meta: config.meta,
            tags: Tags(config.tags),
        }
    }
}

impl From<ExposureConfig> for ProjectExposureConfig {
    fn from(config: ExposureConfig) -> Self {
        Self {
            meta: config.meta,
            tags: config.tags.into_inner(),
            enabled: config.enabled,
            __additional_properties__: BTreeMap::new(),
        }
    }
}

impl ResolvableConfig<ExposureConfig> for ExposureConfig {
    type Resolved = ResolvedExposureConfig;
    type PackageDefaults = ();
    type ResolveDefaults = ();

    fn get_enabled_with_default(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn disable(&mut self) {
        self.enabled = Some(false);
    }

    fn apply_package_defaults(&mut self, _: ()) {}

    fn finalize(self) -> ResolvedExposureConfig {
        self.finalize_resolved()
    }

    fn default_to(&mut self, parent: &ExposureConfig) {
        self.default_to_fields(parent);
    }
}
