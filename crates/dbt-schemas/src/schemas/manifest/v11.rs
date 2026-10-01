use serde::Deserialize;
use std::collections::BTreeMap;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use super::{DbtSelector, DbtSemanticModel, ManifestGroup};
use crate::schemas::{
    macros::{DbtDocsMacro, DbtMacro},
    manifest::{
        DbtNode, ManifestExposure, ManifestMetadata,
        manifest_nodes::{ManifestMetric, ManifestSource},
    },
};

#[derive(Debug, Default, Deserialize)]
pub struct DbtManifestV11 {
    pub metadata: ManifestMetadata,
    pub nodes: BTreeMap<String, DbtNode>,
    pub sources: BTreeMap<String, ManifestSource>,
    pub macros: BTreeMap<String, DbtMacro>,
    pub docs: BTreeMap<String, DbtDocsMacro>,
    pub semantic_models: BTreeMap<String, DbtSemanticModel>,
    pub exposures: BTreeMap<String, ManifestExposure>,
    pub metrics: BTreeMap<String, ManifestMetric>,
    pub child_map: BTreeMap<String, Vec<String>>,
    pub parent_map: BTreeMap<String, Vec<String>>,
    pub group_map: BTreeMap<String, Vec<String>>,
    pub disabled: BTreeMap<String, Vec<YmlValue>>,
    pub selectors: BTreeMap<String, DbtSelector>,
    pub groups: BTreeMap<String, ManifestGroup>,
}
