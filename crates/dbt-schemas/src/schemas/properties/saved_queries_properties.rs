use dbt_yaml::DbtSchema;
use dbt_yaml::Verbatim;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::schemas::project::ExportConfigExportAs;
use crate::schemas::project::SavedQueryConfig;

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct SavedQueriesProperties {
    pub config: Option<SavedQueryConfig>,
    pub description: Option<String>,
    pub exports: Option<Vec<Export>>,
    pub label: Option<String>,
    pub name: String,
    pub query_params: SavedQueriesQueryParams,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct SavedQueriesQueryParams {
    pub group_by: Option<Vec<String>>,
    pub metrics: Option<Vec<String>>,
    #[serde(rename = "where")]
    pub where_: Verbatim<Option<Vec<String>>>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct Export {
    pub name: String,
    pub config: Option<ExportConfig>,
}

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ExportConfig {
    pub alias: Option<String>,
    pub export_as: Option<ExportConfigExportAs>,
    pub schema: Option<String>,
}
