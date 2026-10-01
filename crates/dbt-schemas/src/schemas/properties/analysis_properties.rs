use dbt_common::io_args::StaticAnalysisOffReason;
use dbt_yaml::DbtSchema;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::schemas::dbt_column::ColumnProperties;
use crate::schemas::project::AnalysesConfig;
use crate::schemas::properties::GetConfig;

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema)]
pub struct AnalysesProperties {
    pub name: String,
    pub description: Option<String>,
    pub config: Option<AnalysesConfig>,
    pub columns: Option<Vec<ColumnProperties>>, // this column is much more permissive than whats actually allowed on the analysis
    #[serde(skip_deserializing, default)]
    pub static_analysis_off_reason: Option<StaticAnalysisOffReason>,
}

impl AnalysesProperties {
    pub fn empty(name: String) -> Self {
        Self {
            name,
            description: None,
            config: None,
            columns: None,
            static_analysis_off_reason: None,
        }
    }
}

impl GetConfig<AnalysesConfig> for AnalysesProperties {
    fn get_config(&self) -> Option<&AnalysesConfig> {
        self.config.as_ref()
    }
}
