use indexmap::IndexMap;
use std::collections::{BTreeMap, btree_map::Iter};

use dbt_yaml::{DbtSchema, ShouldBe};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use dbt_proc_macros::Resolvable;

use crate::schemas::project::configs::config_merge::Tags;
use crate::schemas::{
    project::{ResolvableConfig, TypedRecursiveConfig},
    serde::{StringOrArrayOfStrings, bool_or_string_bool},
};
use dbt_proc_macros::DefaultTo;

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ProjectSavedQueryConfig {
    #[serde(rename = "+cache")]
    pub cache: Option<SavedQueryCache>,
    #[serde(default, rename = "+enabled", deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(rename = "+export_as")]
    pub export_as: Option<ExportConfigExportAs>,
    #[serde(rename = "+schema")]
    pub schema: Option<String>,
    #[serde(rename = "+group")]
    pub group: Option<String>,
    #[serde(rename = "+meta")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+tags")]
    pub tags: Option<StringOrArrayOfStrings>,
    // Flattened fields
    pub __additional_properties__: BTreeMap<String, ShouldBe<ProjectSavedQueryConfig>>,
}

impl TypedRecursiveConfig for ProjectSavedQueryConfig {
    fn type_name() -> &'static str {
        "saved_query"
    }

    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>> {
        self.__additional_properties__.iter()
    }

    fn has_set_fields(&self) -> bool {
        self.cache.is_some()
            || self.enabled.is_some()
            || self.export_as.is_some()
            || self.schema.is_some()
            || self.group.is_some()
            || self.meta.is_some()
            || self.tags.is_some()
    }
}

#[derive(Resolvable, DefaultTo, Deserialize, Serialize, Debug, Clone, PartialEq, DbtSchema)]
pub struct SavedQueryConfig {
    pub cache: Option<SavedQueryCache>,
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    pub export_as: Option<ExportConfigExportAs>,
    pub schema: Option<String>,
    pub group: Option<String>,
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(default)]
    pub tags: Tags,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, DbtSchema)]
pub struct SavedQueryCache {
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq, DbtSchema)]
#[allow(non_camel_case_types)]
pub enum ExportConfigExportAs {
    #[default]
    table,
    view,
    cache,
}

impl Default for SavedQueryConfig {
    fn default() -> Self {
        Self {
            cache: Some(SavedQueryCache {
                enabled: Some(false),
            }),
            enabled: Some(true),
            export_as: None,
            schema: None,
            group: None,
            meta: Some(IndexMap::new()),
            tags: Tags(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![]))),
        }
    }
}

impl From<ProjectSavedQueryConfig> for SavedQueryConfig {
    fn from(config: ProjectSavedQueryConfig) -> Self {
        Self {
            cache: config.cache,
            enabled: config.enabled,
            export_as: config.export_as,
            schema: config.schema,
            group: config.group,
            meta: config.meta,
            tags: Tags(config.tags),
        }
    }
}

impl From<SavedQueryConfig> for ProjectSavedQueryConfig {
    fn from(config: SavedQueryConfig) -> Self {
        Self {
            cache: config.cache,
            enabled: config.enabled,
            export_as: config.export_as,
            schema: config.schema,
            group: config.group,
            meta: config.meta,
            tags: config.tags.into_inner(),
            __additional_properties__: BTreeMap::new(),
        }
    }
}

impl ResolvableConfig<SavedQueryConfig> for SavedQueryConfig {
    type Resolved = ResolvedSavedQueryConfig;
    type PackageDefaults = ();
    type ResolveDefaults = ();

    fn get_enabled_with_default(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn disable(&mut self) {
        self.enabled = Some(false);
    }

    fn apply_package_defaults(&mut self, _: ()) {}

    fn finalize(self) -> ResolvedSavedQueryConfig {
        self.finalize_resolved()
    }

    fn default_to(&mut self, parent: &SavedQueryConfig) {
        self.default_to_fields(parent);
    }
}
