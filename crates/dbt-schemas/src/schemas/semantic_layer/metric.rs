use serde::{Deserialize, Serialize};

use crate::schemas::{
    dbt_column::Granularity,
    manifest::{
        DbtMetric,
        common::{SourceFileMetadata, WhereFilterIntersection},
        metric::MetricTypeParams,
    },
    properties::metrics_properties::MetricType,
    semantic_layer::semantic_manifest::SemanticLayerElementConfig,
};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SemanticManifestMetric {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub type_: MetricType,
    pub type_params: MetricTypeParams,
    pub filter: Option<WhereFilterIntersection>,
    pub metadata: Option<SourceFileMetadata>,
    pub label: Option<String>,
    pub config: Option<SemanticLayerElementConfig>,
    pub time_granularity: Option<Granularity>,
}

// Decide whether to map from DbtMetric or DbtManifest
// since DbtManifest is going be legacy it's probably a good idea to map from DbtMetric
impl From<DbtMetric> for SemanticManifestMetric {
    fn from(metric: DbtMetric) -> Self {
        let mut config: Option<SemanticLayerElementConfig> = None;
        if let Some(meta) = metric.deprecated_config.meta
            && !meta.is_empty()
        {
            config = Some(SemanticLayerElementConfig { meta: Some(meta) });
        }

        SemanticManifestMetric {
            name: metric.__common_attr__.name,
            description: metric.__common_attr__.description,
            type_: metric.__metric_attr__.metric_type,
            type_params: metric.__metric_attr__.type_params,
            filter: metric.__metric_attr__.filter,
            metadata: metric.__metric_attr__.metadata,
            label: metric.__metric_attr__.label,
            config,
            time_granularity: metric.__metric_attr__.time_granularity,
        }
    }
}
