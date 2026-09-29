use crate::schemas::common::DbtQuoting;
use crate::schemas::common::ExternalTable;
use crate::schemas::common::FreshnessDefinition;
use crate::schemas::common::SchemaOrigin;
use crate::schemas::common::SyncConfig;
use crate::schemas::data_tests::DataTests;
use crate::schemas::dbt_column::ColumnProperties;
use crate::schemas::project::SourceConfig;
use crate::schemas::serde::StringOrArrayOfStrings;
use crate::schemas::serde::bool_or_string_bool;
use crate::schemas::serde::event_time_or_map_to_string;
use dbt_common::serde_utils::Omissible;
use dbt_yaml::{DbtSchema, Verbatim};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct SourceProperties {
    pub config: Option<SourceConfig>,
    #[serde(alias = "project", alias = "data_space")]
    pub database: Option<String>,
    #[serde(alias = "dataset")]
    pub schema: Option<String>,
    pub catalog: Option<String>,
    pub description: Option<String>,
    pub loader: Option<String>,
    pub name: String,
    pub quoting: Option<DbtQuoting>,
    pub tables: Option<Vec<Tables>>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct Tables {
    pub columns: Option<Vec<ColumnProperties>>,
    pub config: Option<TablesConfig>,
    pub data_tests: Option<Vec<DataTests>>,
    pub description: Option<String>,
    pub external: Option<ExternalTable>,
    pub identifier: Option<String>,
    pub loader: Option<String>,
    pub name: String,
    pub quoting: Option<DbtQuoting>,
    pub tests: Option<Vec<DataTests>>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, Default)]
pub struct TablesConfig {
    #[serde(default, deserialize_with = "event_time_or_map_to_string")]
    pub event_time: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    pub meta: Option<IndexMap<String, YmlValue>>,
    pub freshness: Omissible<Option<FreshnessDefinition>>,
    pub tags: Option<StringOrArrayOfStrings>,
    pub loaded_at_field: Option<String>,
    pub loaded_at_query: Verbatim<Option<String>>,
    pub schema_origin: Option<SchemaOrigin>,
    pub sync: Option<SyncConfig>,
    pub external_location: Option<String>,
    pub formatter: Option<String>,
}
