use crate::AdapterType;
use crate::relation::config_v2::{
    ComponentConfigChange, ComponentConfigLoader, RelationConfigLoader,
};
use crate::relation::databricks::config::{DatabricksRelationMetadata, components};
use indexmap::IndexMap;

fn requires_full_refresh(components: &IndexMap<&'static str, ComponentConfigChange>) -> bool {
    super::requires_full_refresh(super::MaterializationType::MetricView, components)
}

pub(crate) fn new_loader() -> RelationConfigLoader<'static, DatabricksRelationMetadata> {
    let loaders: [Box<dyn ComponentConfigLoader<DatabricksRelationMetadata>>; 3] = [
        Box::new(components::RelationTagsLoader),
        Box::new(components::TblPropertiesLoader),
        Box::new(components::MetricViewQueryLoader),
    ];

    RelationConfigLoader::new(AdapterType::Databricks, loaders, requires_full_refresh)
}
