use dbt_yaml::DbtSchema;
use indexmap::IndexMap;

use serde::{Deserialize, Serialize};

use crate::schemas::Nodes;
use crate::schemas::semantic_layer::metric::SemanticManifestMetric;
use crate::schemas::semantic_layer::project_configuration::SemanticManifestProjectConfiguration;
use crate::schemas::semantic_layer::saved_query::SemanticManifestSavedQuery;
use crate::schemas::semantic_layer::semantic_model::SemanticManifestSemanticModel;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SemanticManifest {
    pub semantic_models: Vec<SemanticManifestSemanticModel>,
    pub metrics: Vec<SemanticManifestMetric>,
    pub project_configuration: SemanticManifestProjectConfiguration,
    pub saved_queries: Vec<SemanticManifestSavedQuery>,
}

impl From<&Nodes> for SemanticManifest {
    fn from(nodes: &Nodes) -> Self {
        let semantic_models = nodes
            .semantic_models
            .values()
            .map(|m| (**m).clone().into())
            .collect();
        let metrics = nodes
            .metrics
            .values()
            .map(|m| {
                // don't hydrate input measures into semantic_manifest.json (only used for manifest.json)
                let mut semantic_manifest_metric: SemanticManifestMetric = (**m).clone().into();
                semantic_manifest_metric.type_params.input_measures = Some(vec![]);
                semantic_manifest_metric
            })
            .collect();
        let project_configuration = SemanticManifestProjectConfiguration {
            dsi_package_version: Default::default(),
            metadata: None,
            time_spines: nodes
                .models
                .values()
                .filter(|m| m.__model_attr__.time_spine.is_some())
                .map(|m| (**m).clone().__model_attr__.time_spine.unwrap())
                .collect(),
            // deprecated fields
            time_spine_table_configurations: vec![],
        };
        let saved_queries = nodes
            .saved_queries
            .values()
            .map(|m| (**m).clone().into())
            .collect();

        SemanticManifest {
            semantic_models,
            metrics,
            project_configuration,
            saved_queries,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, Eq, PartialEq, DbtSchema)]
pub struct SemanticLayerElementConfig {
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_empty_map",
        deserialize_with = "crate::schemas::serde::deserialize_empty_map_as_none"
    )]
    pub meta: Option<IndexMap<String, YmlValue>>,
}
