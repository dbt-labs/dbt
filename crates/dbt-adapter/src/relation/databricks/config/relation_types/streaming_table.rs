//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/streaming_table.py

use crate::AdapterType;
use crate::relation::config_v2::ComponentConfigChange;
use crate::relation::config_v2::{ComponentConfigLoader, RelationConfigLoader};
use crate::relation::databricks::config::{DatabricksRelationMetadata, components};
use indexmap::IndexMap;

fn requires_full_refresh(components: &IndexMap<&'static str, ComponentConfigChange>) -> bool {
    super::requires_full_refresh(super::MaterializationType::StreamingTable, components)
}

fn complete_changeset_key(component_type_name: &'static str) -> Option<&'static str> {
    match component_type_name {
        components::partition_by::TYPE_NAME => Some("partition_by"),
        components::relation_comment::TYPE_NAME
        | components::tbl_properties::TYPE_NAME
        | components::refresh::TYPE_NAME
        | components::relation_tags::TYPE_NAME => Some(component_type_name),
        _ => None,
    }
}

/// Create a `RelationConfigLoader` for Databricks streaming tables
pub(crate) fn new_loader() -> RelationConfigLoader<'static, DatabricksRelationMetadata> {
    let loaders: [Box<dyn ComponentConfigLoader<DatabricksRelationMetadata>>; 8] = [
        Box::new(components::LiquidClusteringLoader),
        Box::new(components::PartitionByLoader),
        Box::new(components::RelationCommentLoader),
        Box::new(components::TblPropertiesLoader),
        Box::new(components::RefreshLoader),
        Box::new(components::RelationTagsLoader),
        Box::new(components::RowFilterLoader),
        Box::new(components::ColumnMasksLoader),
    ];

    RelationConfigLoader::new(AdapterType::Databricks, loaders, requires_full_refresh)
        .with_complete_changeset(complete_changeset_key)
}

#[cfg(test)]
mod tests {
    use super::new_loader;
    use crate::AdapterType;
    use crate::relation::config_v2::{
        ComponentConfigChange, ComponentConfigLoader, RelationComponentConfigChangeSet,
        RelationConfig,
    };
    use crate::relation::databricks::config::{
        DatabricksRelationMetadata, components,
        test_helpers::{TestModelConfig, create_mock_dbt_model, run_test_cases},
    };
    use crate::relation::test_helpers::TestCase;
    use indexmap::IndexMap;

    const COMPONENT_CHANGE_JINJA: &str = r#"
<partition_by>
    <partition_by>
    </partition_by>
</partition_by>
<comment>
    <comment>
        new comment
    </comment>
    <persist>
        True
    </persist>
</comment>
<tblproperties>
    <tblproperties>
        <delta.enableRowTracking>
            true
        </delta.enableRowTracking>
        <customKey>
            new
        </customKey>
        <customKey2>
            value
        </customKey2>
    </tblproperties>
    <pipeline_id>
        my_new_pipeline
    </pipeline_id>
</tblproperties>
<refresh>
    <cron>
        */60 * * * *
    </cron>
    <time_zone_value>
        UTC
    </time_zone_value>
    <every>
        None
    </every>
    <on_update>
        False
    </on_update>
    <at_most_every>
        None
    </at_most_every>
    <auto_refreshed>
        False
    </auto_refreshed>
    <is_altered>
        True
    </is_altered>
</refresh>
<tags>
    <set_tags>
        <a_tag>
            new
        </a_tag>
    </set_tags>
</tags>
<liquid_clustering>
    <auto_cluster>
        False
    </auto_cluster>
    <cluster_by>
        cluster_by_new
    </cluster_by>
</liquid_clustering>
<row_filter>
    <function>
        None
    </function>
    <columns>
    </columns>
    <should_unset>
        True
    </should_unset>
    <is_change>
        True
    </is_change>
</row_filter>
                    "#;

    const PARTITION_CHANGE_JINJA: &str = r#"
<partition_by>
    <partition_by>
        partition_by_new
    </partition_by>
</partition_by>
<comment>
    <comment>
        None
    </comment>
    <persist>
        False
    </persist>
</comment>
<tblproperties>
    <tblproperties>
    </tblproperties>
    <pipeline_id>
        None
    </pipeline_id>
</tblproperties>
<refresh>
    <cron>
        None
    </cron>
    <time_zone_value>
        None
    </time_zone_value>
    <every>
        None
    </every>
    <on_update>
        False
    </on_update>
    <at_most_every>
        None
    </at_most_every>
    <auto_refreshed>
        False
    </auto_refreshed>
    <is_altered>
        False
    </is_altered>
</refresh>
<tags>
    <set_tags>
    </set_tags>
</tags>
                    "#;

    fn component_change_current_state() -> TestModelConfig {
        TestModelConfig {
            persist_relation_comments: true,
            relation_comment: Some("old comment".to_string()),
            cluster_by: vec!["cluster_by_old".to_string()],
            cron: Some("* * * * *".to_string()),
            time_zone: Some("UTC".to_string()),
            row_filter_function: Some("row_filter_fn".to_string()),
            row_filter_columns: vec!["col1".to_string()],
            tags: IndexMap::from_iter([
                ("a_tag".to_string(), "old".to_string()),
                ("b_tag".to_string(), "old".to_string()),
            ]),
            tbl_properties: IndexMap::from_iter([
                ("delta.enableRowTracking".to_string(), "false".to_string()),
                (
                    "pipelines.pipelineId".to_string(),
                    "my_old_pipeline".to_string(),
                ),
                ("customKey".to_string(), "old".to_string()),
            ]),
            ..Default::default()
        }
    }

    fn component_change_desired_state() -> TestModelConfig {
        let mut state = component_change_current_state();
        state.relation_comment = Some("new comment".to_string());
        state.cluster_by = vec!["cluster_by_new".to_string()];
        state.cron = Some("*/60 * * * *".to_string());
        state.row_filter_function = None;
        state.row_filter_columns = vec![];
        state.tags.insert("a_tag".to_string(), "new".to_string());
        state
            .tbl_properties
            .insert("delta.enableRowTracking".to_string(), "true".to_string());
        state.tbl_properties.insert(
            "pipelines.pipelineId".to_string(),
            "my_new_pipeline".to_string(),
        );
        state
            .tbl_properties
            .insert("customKey".to_string(), "new".to_string());
        state
            .tbl_properties
            .insert("customKey2".to_string(), "value".to_string());
        state
    }

    fn expected_component_changeset() -> RelationComponentConfigChangeSet {
        RelationComponentConfigChangeSet::new_with_requires_full_refresh(
            AdapterType::Databricks,
            [
                (
                    "partition_by",
                    ComponentConfigChange::Some(
                        components::PartitionByLoader::new_component_type_erased(vec![]),
                    ),
                ),
                (
                    components::RelationCommentLoader.type_name(),
                    ComponentConfigChange::Some(
                        components::RelationCommentLoader::new_component_type_erased(Some(
                            "new comment".to_string(),
                        )),
                    ),
                ),
                (
                    components::TblPropertiesLoader.type_name(),
                    ComponentConfigChange::Some(
                        components::TblPropertiesLoader::new_component_type_erased(
                            IndexMap::from_iter([
                                ("delta.enableRowTracking".to_string(), "true".to_string()),
                                (
                                    "pipelines.pipelineId".to_string(),
                                    "my_new_pipeline".to_string(),
                                ),
                                ("customKey".to_string(), "new".to_string()),
                                ("customKey2".to_string(), "value".to_string()),
                            ]),
                        ),
                    ),
                ),
                (
                    components::RefreshLoader.type_name(),
                    ComponentConfigChange::Some(
                        components::RefreshLoader::new_component_type_erased(
                            Some("*/60 * * * *".to_string()),
                            Some("UTC".to_string()),
                            None,
                            false,
                            None,
                        ),
                    ),
                ),
                (
                    components::RelationTagsLoader.type_name(),
                    ComponentConfigChange::Some(
                        components::RelationTagsLoader::new_component_type_erased(
                            IndexMap::from_iter([("a_tag".to_string(), "new".to_string())]),
                        ),
                    ),
                ),
                (
                    components::LiquidClusteringLoader.type_name(),
                    ComponentConfigChange::Some(
                        components::LiquidClusteringLoader::new_component_type_erased(
                            false,
                            vec!["cluster_by_new".to_string()],
                        ),
                    ),
                ),
                (
                    components::RowFilterLoader.type_name(),
                    ComponentConfigChange::Some(
                        components::RowFilterLoader::new_component_type_erased(None, vec![]),
                    ),
                ),
            ],
            false,
        )
    }

    fn expected_partition_changeset() -> RelationComponentConfigChangeSet {
        RelationComponentConfigChangeSet::new_with_requires_full_refresh(
            AdapterType::Databricks,
            [
                (
                    "partition_by",
                    ComponentConfigChange::Some(
                        components::PartitionByLoader::new_component_type_erased(vec![
                            "partition_by_new".to_string(),
                        ]),
                    ),
                ),
                (
                    components::RelationCommentLoader.type_name(),
                    ComponentConfigChange::Some(
                        components::RelationCommentLoader::new_component_type_erased(None),
                    ),
                ),
                (
                    components::TblPropertiesLoader.type_name(),
                    ComponentConfigChange::Some(
                        components::TblPropertiesLoader::new_component_type_erased(IndexMap::new()),
                    ),
                ),
                (
                    components::RefreshLoader.type_name(),
                    ComponentConfigChange::Some(
                        components::RefreshLoader::new_component_type_erased(
                            None, None, None, false, None,
                        ),
                    ),
                ),
                (
                    components::RelationTagsLoader.type_name(),
                    ComponentConfigChange::Some(
                        components::RelationTagsLoader::new_component_type_erased(IndexMap::new()),
                    ),
                ),
            ],
            true,
        )
    }

    fn create_test_cases() -> Vec<TestCase<DatabricksRelationMetadata, TestModelConfig>> {
        vec![
            TestCase {
                description: "changing any streaming table components except partition by should not trigger a full refresh",
                relation_loader: new_loader(),
                current_state: component_change_current_state(),
                desired_state: component_change_desired_state(),
                expected_changeset: expected_component_changeset(),
                changeset_jinja: COMPONENT_CHANGE_JINJA,
                requires_full_refresh: false,
            },
            TestCase {
                description: "changing streaming table partition by should trigger a full refresh",
                relation_loader: new_loader(),
                current_state: TestModelConfig {
                    partition_by: vec!["partition_by_old".to_string()],
                    ..Default::default()
                },
                desired_state: TestModelConfig {
                    partition_by: vec!["partition_by_new".to_string()],
                    ..Default::default()
                },
                expected_changeset: expected_partition_changeset(),
                changeset_jinja: PARTITION_CHANGE_JINJA,
                requires_full_refresh: true,
            },
        ]
    }

    #[test]
    fn test_cases() {
        run_test_cases(create_test_cases());
    }

    #[test]
    fn comment_only_diff_carries_supported_v1_desired_components() {
        let stable_properties =
            IndexMap::from([("feature_marker".to_string(), "stable".to_string())]);
        let stable_tags = IndexMap::from([("owner".to_string(), "analytics".to_string())]);
        let current = create_mock_dbt_model(TestModelConfig {
            persist_relation_comments: true,
            relation_comment: Some("old comment".to_string()),
            tags: stable_tags.clone(),
            tbl_properties: stable_properties.clone(),
            ..Default::default()
        });
        let desired = create_mock_dbt_model(TestModelConfig {
            persist_relation_comments: true,
            relation_comment: Some("updated comment".to_string()),
            tags: stable_tags,
            tbl_properties: stable_properties,
            ..Default::default()
        });
        let loader = new_loader();
        let current = loader
            .from_local_config(&current)
            .expect("load current streaming-table config");
        let desired = loader
            .from_local_config(&desired)
            .expect("load desired streaming-table config");
        let changeset = RelationConfig::diff(&desired, &current);

        assert_eq!(
            changeset.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            vec![
                "partition_by",
                "comment",
                "tblproperties",
                "refresh",
                "tags",
            ],
            "a partial streaming-table change must carry every supported component consumed by the v1 ALTER renderer",
        );
        let ComponentConfigChange::Some(properties) = changeset.get("tblproperties") else {
            panic!("changeset must carry stable tblproperties");
        };
        assert_eq!(
            properties
                .to_jinja()
                .get_attr("tblproperties")
                .expect("tblproperties component shape")
                .get_attr("feature_marker")
                .expect("stable feature marker")
                .as_str(),
            Some("stable"),
        );
    }
}
