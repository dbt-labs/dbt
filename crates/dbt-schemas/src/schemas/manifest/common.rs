use std::collections::BTreeMap;

use dbt_yaml::DbtSchema;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::schemas::serde::StringOrArrayOfStrings;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WhereFilterIntersection {
    pub where_filters: Vec<WhereFilter>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WhereFilter {
    pub where_sql_template: String,
}

impl From<Vec<String>> for WhereFilterIntersection {
    fn from(source: Vec<String>) -> Self {
        Self {
            where_filters: source
                .iter()
                .map(|s| WhereFilter {
                    where_sql_template: s.to_string(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct DbtOwner {
    pub email: Option<StringOrArrayOfStrings>,
    pub name: Option<String>,
    #[serde(flatten)]
    pub __other__: BTreeMap<String, YmlValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct SourceFileMetadata {
    pub repo_file_path: String,
    pub file_slice: FileSlice,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct FileSlice {
    pub filename: String,
    pub content: String,
    pub start_line_number: usize,
    pub end_line_number: usize,
}
