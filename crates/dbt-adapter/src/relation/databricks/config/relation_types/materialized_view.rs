//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/materialized_view.py

use crate::AdapterType;
use crate::relation::config_v2::ComponentConfigChange;
use crate::relation::config_v2::{ComponentConfigLoader, RelationConfigLoader};
use crate::relation::databricks::config::{DatabricksRelationMetadata, components};
use indexmap::IndexMap;

fn requires_full_refresh(components: &IndexMap<&'static str, ComponentConfigChange>) -> bool {
    super::requires_full_refresh(super::MaterializationType::MaterializedView, components)
}

/// Create a `RelationConfigLoader` for Databricks materialized views
pub(crate) fn new_loader() -> RelationConfigLoader<'static, DatabricksRelationMetadata> {
    let loaders: [Box<dyn ComponentConfigLoader<DatabricksRelationMetadata>>; 9] = [
        Box::new(components::LiquidClusteringLoader),
        Box::new(components::RelationCommentLoader),
        Box::new(components::PartitionByLoader),
        Box::new(components::QueryLoader),
        Box::new(components::RefreshLoader),
        Box::new(components::RelationTagsLoader),
        Box::new(components::RowFilterLoader),
        Box::new(components::TblPropertiesLoader),
        Box::new(components::ColumnMasksLoader),
    ];

    RelationConfigLoader::new(AdapterType::Databricks, loaders, requires_full_refresh)
}

#[cfg(test)]
mod tests {
    use super::{new_loader, requires_full_refresh};
    use crate::AdapterType;
    use crate::relation::config_v2::{
        ComponentConfigChange, ComponentConfigLoader, RelationComponentConfigChangeSet,
    };
    use crate::relation::databricks::config::{
        DatabricksRelationMetadata, components,
        test_helpers::{TestModelConfig, run_test_cases},
    };
    use crate::relation::test_helpers::TestCase;
    use indexmap::IndexMap;

    fn create_test_cases() -> Vec<TestCase<DatabricksRelationMetadata, TestModelConfig>> {
        vec![
            TestCase {
                description: "changing any of materialized view's components except refresh or tags should trigger a full refresh",
                relation_loader: new_loader(),
                current_state: TestModelConfig {
                    persist_relation_comments: true,
                    query: Some("SELECT 1".to_string()),
                    tbl_properties: IndexMap::from_iter([
                        (
                            "pipelines.pipelineId".to_string(),
                            "dlt-pipeline-1".to_string(),
                        ),
                        ("data.quality".to_string(), "silver".to_string()),
                        ("reporting.audience".to_string(), "internal".to_string()),
                    ]),
                    partition_by: vec!["partition_column_old".to_string()],
                    ..Default::default()
                },
                desired_state: TestModelConfig {
                    persist_relation_comments: true,
                    query: Some("SELECT 1000".to_string()),
                    tbl_properties: IndexMap::from_iter([
                        (
                            "pipelines.pipelineId".to_string(),
                            "dlt-pipeline-1".to_string(),
                        ),
                        ("data.quality".to_string(), "gold".to_string()),
                        ("reporting.audience".to_string(), "company-wide".to_string()),
                    ]),
                    partition_by: vec!["partition_column_new".to_string()],
                    ..Default::default()
                },
                expected_changeset: RelationComponentConfigChangeSet::new(
                    AdapterType::Databricks,
                    [
                        (
                            components::TblPropertiesLoader.type_name(),
                            ComponentConfigChange::Some(
                                components::TblPropertiesLoader::new_component_type_erased(
                                    IndexMap::from_iter([
                                        (
                                            "pipelines.pipelineId".to_string(),
                                            "dlt-pipeline-1".to_string(),
                                        ),
                                        ("data.quality".to_string(), "gold".to_string()),
                                        (
                                            "reporting.audience".to_string(),
                                            "company-wide".to_string(),
                                        ),
                                    ]),
                                ),
                            ),
                        ),
                        (
                            components::PartitionByLoader.type_name(),
                            ComponentConfigChange::Some(
                                components::PartitionByLoader::new_component_type_erased(vec![
                                    "partition_column_new".to_string(),
                                ]),
                            ),
                        ),
                        (
                            components::QueryLoader.type_name(),
                            ComponentConfigChange::Some(
                                components::QueryLoader::new_component_type_erased("SELECT 1000"),
                            ),
                        ),
                    ],
                    requires_full_refresh,
                ),
                changeset_jinja: "
<partitioned_by>
    <partition_by>
        partition_column_new
    </partition_by>
</partitioned_by>
<query>
    <query>
        SELECT 1000
    </query>
</query>
<tblproperties>
    <tblproperties>
        <data.quality>
            gold
        </data.quality>
        <reporting.audience>
            company-wide
        </reporting.audience>
    </tblproperties>
    <pipeline_id>
        dlt-pipeline-1
    </pipeline_id>
</tblproperties>
                    ",
                requires_full_refresh: true,
            },
            TestCase {
                description: "changing only a materialized view's compiled SQL should require a full refresh",
                relation_loader: new_loader(),
                current_state: TestModelConfig {
                    query: Some("SELECT 1".to_string()),
                    ..Default::default()
                },
                desired_state: TestModelConfig {
                    query: Some("SELECT 1000".to_string()),
                    ..Default::default()
                },
                expected_changeset: RelationComponentConfigChangeSet::new(
                    AdapterType::Databricks,
                    [(
                        components::QueryLoader.type_name(),
                        ComponentConfigChange::Some(
                            components::QueryLoader::new_component_type_erased("SELECT 1000"),
                        ),
                    )],
                    requires_full_refresh,
                ),
                changeset_jinja: "
<query>
    <query>
        SELECT 1000
    </query>
</query>
                    ",
                requires_full_refresh: true,
            },
            TestCase {
                description: "changing a materialized view's refresh cron or tags should not trigger a full refresh",
                relation_loader: new_loader(),
                current_state: TestModelConfig {
                    query: Some("SELECT 1".to_string()),
                    cron: Some("* * * * *".to_string()),
                    time_zone: Some("UTC".to_string()),
                    tags: IndexMap::from_iter([
                        ("a_tag".to_string(), "old".to_string()),
                        ("b_tag".to_string(), "old".to_string()),
                    ]),
                    ..Default::default()
                },
                desired_state: TestModelConfig {
                    query: Some("SELECT 1".to_string()),
                    cron: Some("*/60 * * * *".to_string()),
                    time_zone: Some("UTC".to_string()),
                    tags: IndexMap::from_iter([("a_tag".to_string(), "new".to_string())]),
                    row_filter_function: Some("new_row_filter_fn".to_string()),
                    row_filter_columns: vec!["col1".to_string(), "col3".to_string()],
                    ..Default::default()
                },
                expected_changeset: RelationComponentConfigChangeSet::new(
                    AdapterType::Databricks,
                    [
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
                            components::RowFilterLoader.type_name(),
                            ComponentConfigChange::Some(
                                components::RowFilterLoader::new_component_type_erased(
                                    Some("test_db.test_schema.new_row_filter_fn".to_string()),
                                    vec!["col1".to_string(), "col3".to_string()],
                                ),
                            ),
                        ),
                    ],
                    requires_full_refresh,
                ),
                changeset_jinja: "
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
<row_filter>
    <function>
        test_db.test_schema.new_row_filter_fn
    </function>
    <columns>
        col1
        col3
    </columns>
    <should_unset>
        False
    </should_unset>
    <is_change>
        True
    </is_change>
</row_filter>
                    ",
                requires_full_refresh: false,
            },
        ]
    }

    #[test]
    fn test_cases() {
        run_test_cases(create_test_cases());
    }
}
