use crate::schemas::{
    CommonAttributes, NodeBaseAttributes,
    common::Dimension,
    dbt_column::ColumnPropertiesEntityType,
    manifest::{
        common::SourceFileMetadata,
        metric::{MeasureAggregationParameters, NonAdditiveDimension},
    },
    project::SemanticModelConfig,
    properties::metrics_properties::AggregationType,
    semantic_layer::semantic_manifest::SemanticLayerElementConfig,
};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::collections::BTreeMap;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtSemanticModel {
    pub __common_attr__: CommonAttributes,
    pub __base_attr__: NodeBaseAttributes,
    pub __semantic_model_attr__: DbtSemanticModelAttr,

    // yaml path is `models.$.semantic_model`
    pub deprecated_config: SemanticModelConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DbtSemanticModelAttr {
    // Core semantic model attributes
    pub model: String,
    pub node_relation: Option<NodeRelation>,
    pub label: Option<String>,
    pub defaults: Option<SemanticModelDefaults>,
    pub entities: Vec<SemanticEntity>,
    pub dimensions: Vec<Dimension>,
    pub metadata: Option<SourceFileMetadata>,
    pub primary_entity: Option<String>,

    // measures is not a concept in the Fusion compatible Semantic Layer yaml spec
    // but it is still needed for manifest.json
    //
    // this was hydrated by `.semantic_models.$.measures` but it will now be
    // hydrated by `.models.$.metrics`
    pub measures: Vec<SemanticMeasure>,

    pub created_at: f64,
    pub unrendered_config: BTreeMap<String, YmlValue>,
    pub group: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeRelation {
    pub alias: String,
    pub schema_name: String,
    pub database: Option<String>,
    pub relation_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SemanticModelDefaults {
    pub agg_time_dimension: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SemanticEntity {
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: ColumnPropertiesEntityType,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default",
        deserialize_with = "crate::schemas::serde::deserialize_default_as_none"
    )]
    pub description: Option<String>,
    pub label: Option<String>,
    pub role: Option<String>,
    pub expr: Option<String>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default",
        deserialize_with = "crate::schemas::serde::deserialize_default_as_none"
    )]
    pub config: Option<SemanticLayerElementConfig>,
    pub metadata: Option<SourceFileMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SemanticMeasure {
    pub name: String,
    pub agg: AggregationType,
    pub description: Option<String>,
    pub label: Option<String>,
    pub create_metric: Option<bool>,
    pub expr: Option<String>,
    pub agg_params: Option<MeasureAggregationParameters>,
    pub non_additive_dimension: Option<NonAdditiveDimension>,
    pub agg_time_dimension: Option<String>,
    pub config: Option<SemanticLayerElementConfig>,
}
