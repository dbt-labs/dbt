use dbt_yaml::DbtSchema;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct PostgresIndex {
    columns: Vec<String>,
    unique: Option<bool>,
    #[serde(rename = "type")]
    _type: Option<String>,
}
