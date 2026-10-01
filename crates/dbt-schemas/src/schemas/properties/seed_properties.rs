use crate::schemas::data_tests::DataTests;
use crate::schemas::dbt_column::ColumnProperties;
use crate::schemas::project::SeedConfig;
use dbt_common::io_args::StaticAnalysisOffReason;
use dbt_yaml::DbtSchema;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct SeedProperties {
    pub columns: Option<Vec<ColumnProperties>>,
    pub config: Option<SeedConfig>,
    pub data_tests: Option<Vec<DataTests>>,
    pub description: Option<String>,
    pub name: String,
    #[serde(skip_deserializing, default)]
    pub static_analysis_off_reason: Option<StaticAnalysisOffReason>,
    pub tests: Option<Vec<DataTests>>,
}

impl SeedProperties {
    pub fn empty(name: String) -> Self {
        Self {
            name,
            columns: None,
            config: None,
            data_tests: None,
            description: None,
            static_analysis_off_reason: None,
            tests: None,
        }
    }
}
