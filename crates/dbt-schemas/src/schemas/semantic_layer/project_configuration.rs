use serde::{Deserialize, Serialize};

use crate::schemas::{TimeSpine, dbt_column::Granularity, manifest::common::SourceFileMetadata};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SemanticManifestProjectConfiguration {
    pub time_spine_table_configurations: Vec<TimeSpineTableConfiguration>,
    pub metadata: Option<SourceFileMetadata>,
    pub dsi_package_version: SemanticVersion,
    pub time_spines: Vec<TimeSpine>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TimeSpineTableConfiguration {
    pub location: String,
    pub column_name: String,
    pub grain: Granularity,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SemanticVersion {
    pub major_version: String,
    pub minor_version: String,
    pub patch_version: Option<String>,
}

impl Default for SemanticVersion {
    fn default() -> Self {
        Self {
            major_version: "0".to_string(),
            minor_version: "0".to_string(),
            patch_version: Some("0".to_string()),
        }
    }
}
