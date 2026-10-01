use crate::schemas::common::DocsConfig;
use crate::schemas::common::Versions;
use crate::schemas::manifest::common::DbtOwner;
use crate::schemas::serde::{FloatOrString, bool_or_string_bool, string_or_array};
use dbt_yaml::DbtSchema;
use dbt_yaml::Spanned;
use dbt_yaml::Verbatim;
use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::collections::BTreeMap;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use super::AnalysesProperties;
use super::CheckProperties;
use super::DataTestProperties;
use super::ExposureProperties;
use super::FunctionProperties;
use super::MetricsProperties;
use super::ModelProperties;
use super::SavedQueriesProperties;
use super::SeedProperties;
use super::SnapshotProperties;
use super::SourceProperties;
use super::unit_test_properties::UnitTestProperties;

#[derive(Deserialize, Debug, Clone)]
pub struct DbtPropertiesFileValues {
    pub version: Option<FloatOrString>,
    pub analyses: Option<Vec<dbt_yaml::Value>>,
    pub checks: Option<Vec<dbt_yaml::Value>>,
    pub exposures: Option<Vec<dbt_yaml::Value>>,
    pub groups: Option<Vec<dbt_yaml::Value>>,
    pub macros: Option<Vec<dbt_yaml::Value>>,
    pub metrics: Option<Vec<dbt_yaml::Value>>,
    pub models: Option<Vec<dbt_yaml::Value>>,
    pub functions: Option<Vec<dbt_yaml::Value>>,
    pub saved_queries: Option<Vec<dbt_yaml::Value>>,
    pub seeds: Option<Vec<dbt_yaml::Value>>,
    // semantic_models cannot be removed for backward compatibility
    // removal would result in many regression tests failing
    pub semantic_models: Option<Vec<dbt_yaml::Value>>,
    pub snapshots: Option<Vec<dbt_yaml::Value>>,
    pub sources: Option<Vec<dbt_yaml::Value>>,
    pub unit_tests: Option<Vec<dbt_yaml::Value>>,
    pub tests: Option<Vec<dbt_yaml::Value>>,
    pub data_tests: Option<Vec<dbt_yaml::Value>>,
    pub anchors: Verbatim<Option<dbt_yaml::Value>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct MinimalSchemaValue {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<FloatOrString>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<Vec<Versions>>,
    pub tables: Verbatim<Option<Vec<dbt_yaml::Value>>>,
    pub __additional_properties__: Verbatim<BTreeMap<String, dbt_yaml::Value>>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct MinimalTableValue {
    pub name: Spanned<String>,
    pub __additional_properties__: Verbatim<BTreeMap<String, dbt_yaml::Value>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct MinimalUnitTestValue {
    pub name: Spanned<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versions: Option<dbt_yaml::Value>,
    pub __additional_properties__: Verbatim<BTreeMap<String, dbt_yaml::Value>>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct DbtPropertiesFile {
    pub models: Option<Vec<ModelProperties>>,
    pub snapshots: Option<Vec<SnapshotProperties>>,
    pub seeds: Option<Vec<SeedProperties>>,
    pub saved_queries: Option<Vec<SavedQueriesProperties>>,
    pub sources: Option<Vec<SourceProperties>>,
    pub unit_tests: Option<Vec<UnitTestProperties>>,
    pub tests: Option<Vec<DataTestProperties>>,
    pub data_tests: Option<Vec<DataTestProperties>>,
    pub analyses: Option<Vec<AnalysesProperties>>,
    pub checks: Option<Vec<CheckProperties>>,
    pub exposures: Option<Vec<ExposureProperties>>,
    pub groups: Option<Vec<GroupProperties>>,
    pub macros: Option<Vec<MacrosProperties>>,
    pub metrics: Option<Vec<MetricsProperties>>,
    pub functions: Option<Vec<FunctionProperties>>,
    // semantic_models cannot be removed for backward compatibility
    // removal would result in many regression tests failing
    pub semantic_models: Option<Vec<dbt_yaml::Value>>,
    pub version: Option<FloatOrString>,
    pub anchors: Verbatim<Option<dbt_yaml::Value>>,
}

// -- Additional Properties
#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
#[allow(dead_code)]
pub struct AnalysesConfig {
    #[serde(default, deserialize_with = "string_or_array")]
    pub tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    pub meta: Option<IndexMap<String, YmlValue>>,
    pub docs: Option<DocsConfig>,
    pub group: Option<String>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct GroupProperties {
    pub name: String,
    pub owner: DbtOwner,
    pub description: Option<String>,
    pub config: Option<GroupConfig>,
}

#[skip_serializing_none]
#[derive(Deserialize, Default, Serialize, Debug, Clone, PartialEq, DbtSchema)]
pub struct GroupConfig {
    pub meta: Option<IndexMap<String, YmlValue>>,
}

#[skip_serializing_none]
#[derive(Deserialize, Default, Serialize, Debug, Clone, DbtSchema)]
pub struct MacrosConfig {
    pub docs: Option<DocsConfig>,
    pub meta: Option<IndexMap<String, YmlValue>>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct MacrosProperties {
    pub arguments: Option<Vec<MacrosArguments>>,
    pub config: Option<MacrosConfig>,
    pub description: Option<String>,
    pub docs: Option<DocsConfig>,
    pub meta: Option<IndexMap<String, YmlValue>>,
    pub name: String,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct MacrosArguments {
    pub description: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: Option<String>,
}

pub trait GetConfig<T>: DeserializeOwned + Send + Sync {
    fn get_config(&self) -> Option<&T>;
}
