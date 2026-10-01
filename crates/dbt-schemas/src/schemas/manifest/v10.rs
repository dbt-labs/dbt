use serde::Deserialize;
use std::collections::BTreeMap;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use super::{DbtSelector, ManifestGroup};
use crate::schemas::{
    common::{Dimension, SemanticModelDependsOn},
    macros::{DbtDocsMacro, DbtMacro},
    manifest::{DbtNode, ManifestExposure, ManifestMetadata, manifest_nodes::ManifestSource},
    project::SemanticModelConfig,
    ref_and_source::DbtRef,
};
use serde_with::skip_serializing_none;
use std::path::PathBuf;

#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize)]
pub struct DbtMetricV10 {
    pub name: String,
    pub package_name: String,
    pub path: PathBuf,
    pub original_file_path: PathBuf,
    pub unique_id: String,
    pub fqn: Vec<String>,
    pub description: String,
    pub label: Option<String>,
    pub type_params: MetricTypeParamsV10,
    pub filter: Option<MetricFilterV10>,
    pub metadata: Option<YmlValue>,
    pub time_granularity: Option<String>,
    pub unrendered_config: BTreeMap<String, YmlValue>,
    pub sources: Vec<Vec<String>>,
    pub depends_on: MetricDependsOnV10,
    pub refs: Vec<DbtRef>,
    pub metrics: Vec<Vec<String>>,
    pub created_at: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct MetricDependsOnV10 {
    pub macros: Vec<String>,
    pub nodes: Vec<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize)]
pub struct MetricTypeParamsV10 {
    pub measure: Option<MetricMeasureV10>,
    pub input_measures: Option<Vec<YmlValue>>,
    pub numerator: Option<YmlValue>,
    pub denominator: Option<YmlValue>,
    pub expr: Option<String>,
    pub window: Option<YmlValue>,
    pub grain_to_date: Option<YmlValue>,
    pub metrics: Option<Vec<YmlValue>>,
    pub conversion_type_params: Option<YmlValue>,
    pub cumulative_type_params: Option<YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize)]
pub struct MetricMeasureV10 {
    pub name: String,
    pub filter: Option<YmlValue>,
    pub alias: Option<String>,
    pub join_to_timespine: Option<bool>,
    pub fill_nulls_with: Option<YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize)]
pub struct MetricFilterV10 {
    pub where_sql_template: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DbtSemanticModelV10 {
    pub name: String,
    pub package_name: String,
    pub path: PathBuf,
    pub original_file_path: PathBuf,
    pub unique_id: String,
    pub fqn: Vec<String>,
    pub model: String,
    pub node_relation: NodeRelationV10,
    pub description: Option<String>,
    pub label: Option<String>,
    pub defaults: Option<SemanticModelDefaultsV10>,
    pub entities: Vec<SemanticModelEntityV10>,
    pub measures: Vec<SemanticModelMeasureV10>,
    pub dimensions: Vec<Dimension>,
    pub metadata: Option<YmlValue>,
    pub depends_on: SemanticModelDependsOn,
    pub refs: Vec<DbtRef>,
    pub created_at: Option<f64>,
    pub config: SemanticModelConfig,
    pub primary_entity: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct NodeRelationV10 {
    pub alias: String,
    pub schema_name: String,
    pub database: String,
    pub relation_name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SemanticModelDefaultsV10 {
    pub agg_time_dimension: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SemanticModelEntityV10 {
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub description: Option<String>,
    pub label: Option<String>,
    pub role: Option<String>,
    pub expr: Option<String>,
    pub config: Option<EntityConfigV10>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct EntityConfigV10 {
    pub meta: BTreeMap<String, YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize)]
pub struct SemanticModelMeasureV10 {
    pub name: String,
    pub description: Option<String>,
    pub agg: String,
    pub agg_params: Option<YmlValue>,
    pub agg_time_dimension: Option<String>,
    pub label: Option<String>,
    pub expr: Option<String>,
    pub create_metric: Option<bool>,
    pub create_metric_display_name: Option<String>,
    pub config: Option<MeasureConfigV10>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MeasureConfigV10 {
    pub meta: BTreeMap<String, YmlValue>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DbtManifestV10 {
    pub metadata: ManifestMetadata,
    pub nodes: BTreeMap<String, DbtNode>,
    pub sources: BTreeMap<String, ManifestSource>,
    pub macros: BTreeMap<String, DbtMacro>,
    pub docs: BTreeMap<String, DbtDocsMacro>,
    pub semantic_models: BTreeMap<String, DbtSemanticModelV10>,
    pub exposures: BTreeMap<String, ManifestExposure>,
    pub metrics: BTreeMap<String, DbtMetricV10>,
    pub child_map: BTreeMap<String, Vec<String>>,
    pub parent_map: BTreeMap<String, Vec<String>>,
    pub group_map: BTreeMap<String, Vec<String>>,
    pub disabled: BTreeMap<String, Vec<YmlValue>>,
    pub selectors: BTreeMap<String, DbtSelector>,
    pub groups: BTreeMap<String, ManifestGroup>,
}
