use crate::AdapterType;
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigChange, RelationConfig, RequiresFullRefreshFn,
};
use crate::relation::databricks::config::components;
use crate::relation::databricks::typed_constraint::TypedConstraint;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::dbt_column::ColumnMask;
use dbt_schemas::schemas::properties::ModelConstraint;
use indexmap::{IndexMap, IndexSet};

pub(crate) mod incremental_table;
pub(crate) mod materialized_view;
pub(crate) mod metric_view;
pub(crate) mod streaming_table;
pub(crate) mod view;

/// Build a `Box<dyn ComponentConfig>` from one recorded component dict.
fn component_from_recorded(
    relation_type: RelationType,
    name: &str,
    val: &serde_json::Value,
) -> Option<Box<dyn ComponentConfig>> {
    match name {
        components::tbl_properties::TYPE_NAME => {
            let mut props: IndexMap<String, String> = val
                .get("tblproperties")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            // `to_jinja` splits `pipelines.pipelineId` out into a separate
            // top-level `pipeline_id` key; re-insert it so the round-trip is
            // lossless (Time Machine replay reserializes this component).
            if let Some(pipeline_id) = val
                .get("pipeline_id")
                .and_then(|v| serde_json::from_value::<Option<String>>(v.clone()).ok())
                .flatten()
            {
                props.insert(
                    components::tbl_properties::PIPELINE_ID_KEY.to_string(),
                    pipeline_id,
                );
            }
            Some(components::TblPropertiesLoader::new_component_type_erased(
                props,
            ))
        }
        // {"comment": str|null, "persist": bool}
        components::relation_comment::TYPE_NAME => {
            let comment: Option<String> = val
                .get("comment")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            Some(components::RelationCommentLoader::new_component_type_erased(comment))
        }
        // {"comments": {col: str}, "persist": bool}. Databricks metadata can
        // include a spurious empty-string column key; drop it (fs asserts non-empty names).
        components::column_comments::TYPE_NAME => {
            let comments: IndexMap<String, String> = val
                .get("comments")
                .and_then(|v| serde_json::from_value::<IndexMap<String, String>>(v.clone()).ok())
                .unwrap_or_default()
                .into_iter()
                .filter(|(k, _)| !k.is_empty())
                .collect();
            Some(components::ColumnCommentsLoader::new_component_type_erased(
                comments,
            ))
        }
        // {"set_column_tags": {col: {k: v}}}.
        components::column_tags::TYPE_NAME => {
            let tags: IndexMap<String, IndexMap<String, String>> = val
                .get("set_column_tags")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            Some(components::ColumnTagsLoader::new_component_type_erased(
                tags,
            ))
        }
        // {"set_tags": {k: v}}
        components::relation_tags::TYPE_NAME => {
            let tags: IndexMap<String, String> = val
                .get("set_tags")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            Some(components::RelationTagsLoader::new_component_type_erased(
                tags,
            ))
        }
        // {"auto_cluster": bool, "cluster_by": [str]}
        components::liquid_clustering::TYPE_NAME => {
            let auto_cluster: bool = val
                .get("auto_cluster")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let cluster_by: Vec<String> = val
                .get("cluster_by")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            Some(
                components::LiquidClusteringLoader::new_component_type_erased(
                    auto_cluster,
                    cluster_by,
                ),
            )
        }
        // {"partition_by": [str]}
        // TYPE_NAME is `partitioned_by`, but Core records it as `partition_by`
        "partition_by" | components::partition_by::TYPE_NAME => {
            let partition_by: Vec<String> = val
                .get("partition_by")
                .or_else(|| val.get("partitioned_by"))
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            Some(components::PartitionByLoader::new_component_type_erased(
                partition_by,
            ))
        }
        // {"cron": str|null, "time_zone_value": str|null, ...}
        components::refresh::TYPE_NAME => {
            let cron: Option<String> = val
                .get("cron")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let time_zone_value: Option<String> = val
                .get("time_zone_value")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            Some(components::RefreshLoader::new_component_type_erased(
                cron,
                time_zone_value,
            ))
        }
        // {"query": str}
        components::query::TYPE_NAME => {
            let query: String = val
                .get("query")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            if relation_type == RelationType::MetricView {
                Some(components::MetricViewQueryLoader::new_component_type_erased(&query))
            } else {
                Some(components::QueryLoader::new_component_type_erased(&query))
            }
        }
        // {"set_column_masks": {col: {"function": str, "using_columns": str|null}},
        //  "unset_column_masks": [str]}
        components::column_masks::TYPE_NAME => {
            let set_column_masks: IndexMap<String, ColumnMask> = val
                .get("set_column_masks")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let unset_column_masks: Vec<String> = val
                .get("unset_column_masks")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            Some(components::ColumnMasksLoader::new_component_type_erased(
                set_column_masks,
                unset_column_masks,
            ))
        }
        // {"set_non_nulls": [str], "unset_non_nulls": [str],
        //  "set_constraints": [<flat constraint>], "unset_constraints": [...]}.
        components::constraints::TYPE_NAME => {
            let non_nulls = |key: &str| -> IndexSet<String> {
                val.get(key)
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default()
            };
            let typed = |key: &str| -> IndexSet<TypedConstraint> {
                val.get(key)
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|c| serde_json::from_value::<ModelConstraint>(c.clone()).ok())
                    .filter_map(|mc| TypedConstraint::try_from(&mc).ok())
                    .collect()
            };
            Some(components::ConstraintsLoader::new_component_type_erased(
                non_nulls("set_non_nulls"),
                non_nulls("unset_non_nulls"),
                typed("set_constraints"),
                typed("unset_constraints"),
            ))
        }
        // {"function": str, "columns": [str]}.
        components::row_filter::TYPE_NAME => {
            let function: Option<String> = val
                .get("function")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let columns: Vec<String> = val
                .get("columns")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            Some(components::RowFilterLoader::new_component_type_erased(
                function, columns,
            ))
        }
        _ => None,
    }
}

/// Reconstruct a `RelationConfig` from a recorded `get_relation_config` payload.
///
/// Conformance replay wraps components in `{"config": {...}}`, while Time Machine
/// recordings store the component map directly.
pub(crate) fn relation_config_from_recorded(
    adapter_type: AdapterType,
    relation_type: RelationType,
    recorded: &serde_json::Value,
) -> AdapterResult<RelationConfig> {
    let recorded_components = recorded.get("config").unwrap_or(recorded);
    let components: Vec<Box<dyn ComponentConfig>> = recorded_components
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, val)| component_from_recorded(relation_type, name, val))
        .collect();

    let rff: RequiresFullRefreshFn = match relation_type {
        RelationType::Table => |c| requires_full_refresh(MaterializationType::IncrementalTable, c),
        RelationType::MaterializedView => {
            |c| requires_full_refresh(MaterializationType::MaterializedView, c)
        }
        RelationType::MetricView => |c| requires_full_refresh(MaterializationType::MetricView, c),
        RelationType::StreamingTable => {
            |c| requires_full_refresh(MaterializationType::StreamingTable, c)
        }
        RelationType::View => |c| requires_full_refresh(MaterializationType::View, c),
        other => {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("unsupported relation type for recorded get_relation_config: {other:?}"),
            ));
        }
    };

    Ok(RelationConfig::new(adapter_type, components, rff))
}

/// All Databricks materialization types
///
/// This is only used for the `requires_full_refresh` function below
pub(super) enum MaterializationType {
    IncrementalTable,
    MaterializedView,
    MetricView,
    StreamingTable,
    View,
}

/// Whether a changeset requires a full refresh given the materialization type
///
/// I made this as one function instead of a bunch of little scattered functions for each
/// materialization type so we can have it all in one place. It makes it easier to see possible
/// optimizations.
pub(super) fn requires_full_refresh(
    materialization_type: MaterializationType,
    components: &IndexMap<&'static str, ComponentConfigChange>,
) -> bool {
    use crate::relation::databricks::config::components::*;

    match materialization_type {
        // https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/incremental.py
        MaterializationType::IncrementalTable => false,
        // https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/materialized_view.py
        MaterializationType::MaterializedView => {
            const REFRESH_ON: [&str; 5] = [
                liquid_clustering::TYPE_NAME,
                partition_by::TYPE_NAME,
                query::TYPE_NAME,
                relation_comment::TYPE_NAME,
                tbl_properties::TYPE_NAME,
            ];
            REFRESH_ON.iter().any(|k| components.contains_key(k))
        }
        MaterializationType::MetricView => false,
        // https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/streaming_table.py
        MaterializationType::StreamingTable => components.contains_key(partition_by::TYPE_NAME),
        // https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/view.py
        MaterializationType::View => components.contains_key(relation_comment::TYPE_NAME),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desired_metric_view(definition: &str) -> RelationConfig {
        RelationConfig::new(
            AdapterType::Databricks,
            [components::MetricViewQueryLoader::new_component_type_erased(definition)],
            |changes| requires_full_refresh(MaterializationType::MetricView, changes),
        )
    }

    fn recorded_metric_view(definition: &str) -> RelationConfig {
        relation_config_from_recorded(
            AdapterType::Databricks,
            RelationType::MetricView,
            &serde_json::json!({"query": {"query": definition}}),
        )
        .unwrap()
    }

    #[test]
    fn equivalent_recorded_metric_view_yaml_has_no_diff() {
        let desired = desired_metric_view(
            "version: 1.1\nsource: '`catalog`.`schema`.`orders`'\ndimensions: [a, b]",
        );
        let equivalent = recorded_metric_view(
            "dimensions:\n  - a\n  - b\nsource: \"`catalog`.`schema`.`orders`\"\nversion: 1.1",
        );
        assert!(RelationConfig::diff(&desired, &equivalent).is_empty());
    }

    #[test]
    fn changed_recorded_metric_view_yaml_has_query_diff() {
        let desired = desired_metric_view(
            "version: 1.1\nsource: '`catalog`.`schema`.`orders`'\ndimensions: [a, b]",
        );
        let changed = recorded_metric_view(
            "version: 1.1\nsource: '`catalog`.`schema`.`customers`'\ndimensions: [a, b]",
        );
        let changes = RelationConfig::diff(&desired, &changed);
        assert!(matches!(
            changes.get(components::query::TYPE_NAME),
            ComponentConfigChange::Some(_)
        ));
    }
}
