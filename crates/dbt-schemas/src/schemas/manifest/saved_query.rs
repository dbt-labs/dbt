use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::collections::BTreeMap;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use crate::schemas::{
    CommonAttributes, NodeBaseAttributes,
    manifest::common::SourceFileMetadata,
    project::{ExportConfigExportAs, SavedQueryCache, SavedQueryConfig},
};

use super::common::WhereFilterIntersection;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtSavedQuery {
    pub __common_attr__: CommonAttributes,
    pub __base_attr__: NodeBaseAttributes,
    pub __saved_query_attr__: DbtSavedQueryAttr,

    // To be deprecated
    pub deprecated_config: SavedQueryConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DbtSavedQueryAttr {
    pub query_params: SavedQueryParams,
    pub exports: Vec<SavedQueryExport>,
    pub label: Option<String>,
    pub metadata: Option<SourceFileMetadata>,
    pub unrendered_config: BTreeMap<String, YmlValue>,
    pub created_at: f64,
    pub group: Option<String>,
    pub cache: Option<SavedQueryCache>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedQueryParams {
    pub metrics: Vec<String>,
    pub group_by: Vec<String>,
    #[serde(rename = "where")]
    pub where_: Option<WhereFilterIntersection>,
    #[serde(default)]
    pub order_by: Vec<String>,
    pub limit: Option<i32>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SavedQueryExport {
    pub name: String,
    #[serde(default)]
    pub config: SavedQueryExportConfig,
    pub unrendered_config: BTreeMap<String, YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedQueryExportConfig {
    pub export_as: ExportConfigExportAs,
    pub schema_name: Option<String>,
    pub alias: Option<String>,
    pub database: Option<String>,
}

// Fusion resolves the database and schema during resolve, wheras mantle
// resolves those at runtime. Since these will likely always be different
// between the deferral manifest and the current nodes, we ignore them
// for modified comparisons.
impl PartialEq for SavedQueryExportConfig {
    fn eq(&self, other: &Self) -> bool {
        self.export_as == other.export_as && self.alias == other.alias
    }
}
