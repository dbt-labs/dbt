use dbt_schemas::schemas::profiles::DbConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileTarget {
    pub target: String,
    pub outputs: HashMap<String, DbConfig>,
}

pub type Profiles = HashMap<String, ProfileTarget>;
