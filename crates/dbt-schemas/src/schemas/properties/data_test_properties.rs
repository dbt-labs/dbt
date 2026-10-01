use crate::schemas::{project::DataTestConfig, properties::GetConfig};
use dbt_common::io_args::StaticAnalysisOffReason;
use dbt_yaml::DbtSchema;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct DataTestProperties {
    pub config: Option<DataTestConfig>,
    pub description: Option<String>,
    pub name: String,
    #[serde(skip_deserializing, default)]
    pub static_analysis_off_reason: Option<StaticAnalysisOffReason>,
}

impl GetConfig<DataTestConfig> for DataTestProperties {
    fn get_config(&self) -> Option<&DataTestConfig> {
        self.config.as_ref()
    }
}

impl DataTestProperties {
    pub fn empty(model_name: String) -> Self {
        Self {
            config: None,
            description: None,
            name: model_name,
            static_analysis_off_reason: None,
        }
    }
}
