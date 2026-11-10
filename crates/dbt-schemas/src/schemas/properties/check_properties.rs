use dbt_yaml::DbtSchema;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::schemas::project::CheckConfig;
use crate::schemas::properties::GetConfig;

/// A `checks:` entry in a properties `.yml`, describing one check by name.
///
/// Unlike most resources a check has no `columns:` — it is a query over project metadata, not a
/// relation with a schema.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema)]
pub struct CheckProperties {
    pub name: String,
    pub description: Option<String>,
    pub config: Option<CheckConfig>,
}

impl CheckProperties {
    pub fn empty(name: String) -> Self {
        Self {
            name,
            description: None,
            config: None,
        }
    }
}

impl GetConfig<CheckConfig> for CheckProperties {
    fn get_config(&self) -> Option<&CheckConfig> {
        self.config.as_ref()
    }
}
