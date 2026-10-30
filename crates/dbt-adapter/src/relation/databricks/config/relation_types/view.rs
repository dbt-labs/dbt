//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/view.py

use crate::AdapterType;
use crate::relation::config_v2::ComponentConfigChange;
use crate::relation::config_v2::{ComponentConfigLoader, RelationConfigLoader};
use crate::relation::databricks::config::{DatabricksRelationMetadata, components};
use indexmap::IndexMap;

fn requires_full_refresh(components: &IndexMap<&'static str, ComponentConfigChange>) -> bool {
    super::requires_full_refresh(super::MaterializationType::View, components)
}

/// Create a `RelationConfigLoader` for Databricks views
pub(crate) fn new_loader() -> RelationConfigLoader<'static, DatabricksRelationMetadata> {
    let loaders: [Box<dyn ComponentConfigLoader<DatabricksRelationMetadata>>; 6] = [
        Box::new(components::ColumnCommentsLoader),
        Box::new(components::QueryLoader),
        Box::new(components::RelationCommentLoader),
        Box::new(components::RelationTagsLoader),
        Box::new(components::TblPropertiesLoader),
        Box::new(components::ColumnTagsLoader),
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
        test_helpers::{TestModelColumn, TestModelConfig, run_test_cases},
    };
    use crate::relation::test_helpers::TestCase;
    use indexmap::IndexMap;

    fn create_test_cases() -> Vec<TestCase<DatabricksRelationMetadata, TestModelConfig>> {
        vec![
            TestCase {
                description: "changing all of view's components except relation comment should not trigger a full refresh",
                relation_loader: new_loader(),
                current_state: TestModelConfig {
                    persist_relation_comments: true,
                    persist_column_comments: true,
                    query: Some("SELECT 1".to_string()),
                    columns: vec![
                        TestModelColumn {
                            name: "a_column".to_string(),
                            comment: Some("old comment".to_string()),
                            ..Default::default()
                        },
                        TestModelColumn {
                            name: "b_column".to_string(),
                            comment: Some("old comment".to_string()),
                            ..Default::default()
                        },
                    ],
                    tags: IndexMap::from_iter([
                        ("a_tag".to_string(), "old".to_string()),
                        ("b_tag".to_string(), "old".to_string()),
                    ]),
                    tbl_properties: IndexMap::from_iter([
                        ("data.owner".to_string(), "data-engineering".to_string()),
                        ("data.quality".to_string(), "bronze".to_string()),
                    ]),
                    ..Default::default()
                },
                desired_state: TestModelConfig {
                    persist_relation_comments: true,
                    persist_column_comments: true,
                    query: Some("SELECT 1000".to_string()),
                    columns: vec![
                        TestModelColumn {
                            name: "a_column".to_string(),
                            comment: Some("new comment".to_string()),
                            ..Default::default()
                        },
                        TestModelColumn {
                            name: "b_column".to_string(),
                            comment: Some("old comment".to_string()),
                            ..Default::default()
                        },
                    ],
                    tags: IndexMap::from_iter([
                        ("a_tag".to_string(), "new".to_string()),
                        ("b_tag".to_string(), "old".to_string()),
                    ]),
                    tbl_properties: IndexMap::from_iter([
                        (
                            "data.owner".to_string(),
                            "analytics-engineering".to_string(),
                        ),
                        ("data.quality".to_string(), "silver".to_string()),
                    ]),
                    ..Default::default()
                },
                expected_changeset: RelationComponentConfigChangeSet::new(
                    AdapterType::Databricks,
                    [
                        (
                            components::ColumnCommentsLoader.type_name(),
                            ComponentConfigChange::Some(
                                components::ColumnCommentsLoader::new_component_type_erased(
                                    IndexMap::from_iter([(
                                        "`a_column`".to_string(),
                                        "new comment".to_string(),
                                    )]),
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
                            components::QueryLoader.type_name(),
                            ComponentConfigChange::Some(
                                components::QueryLoader::new_component_type_erased("SELECT 1000"),
                            ),
                        ),
                        (
                            components::TblPropertiesLoader.type_name(),
                            ComponentConfigChange::Some(
                                components::TblPropertiesLoader::new_component_type_erased(
                                    IndexMap::from_iter([
                                        (
                                            "data.owner".to_string(),
                                            "analytics-engineering".to_string(),
                                        ),
                                        ("data.quality".to_string(), "silver".to_string()),
                                    ]),
                                ),
                            ),
                        ),
                    ],
                    requires_full_refresh,
                ),
                changeset_jinja: "
<column_comments>
    <comments>
        <a_column>
            new comment
        </a_column>
    </comments>
    <persist>
        True
    </persist>
</column_comments>
<query>
    <query>
        SELECT 1000
    </query>
</query>
<tags>
    <set_tags>
        <a_tag>
            new
        </a_tag>
    </set_tags>
</tags>
<tblproperties>
    <tblproperties>
        <data.owner>
            analytics-engineering
        </data.owner>
        <data.quality>
            silver
        </data.quality>
    </tblproperties>
    <pipeline_id>
        None
    </pipeline_id>
</tblproperties>
                    ",
                requires_full_refresh: false,
            },
            TestCase {
                // Databricks doesnt have an API to update relation comments
                description: "changing a view's relation comment should trigger a full refresh",
                relation_loader: new_loader(),
                current_state: TestModelConfig {
                    relation_comment: Some("old comment".to_string()),
                    persist_relation_comments: true,
                    // A view always has SQL; holding it constant keeps `query` out of the changeset
                    query: Some("SELECT 1".to_string()),
                    ..Default::default()
                },
                desired_state: TestModelConfig {
                    relation_comment: Some("new comment".to_string()),
                    persist_relation_comments: true,
                    query: Some("SELECT 1".to_string()),
                    ..Default::default()
                },
                expected_changeset: RelationComponentConfigChangeSet::new(
                    AdapterType::Databricks,
                    [(
                        components::RelationCommentLoader.type_name(),
                        ComponentConfigChange::Some(
                            components::RelationCommentLoader::new_component_type_erased(Some(
                                "new comment".to_string(),
                            )),
                        ),
                    )],
                    requires_full_refresh,
                ),
                changeset_jinja: "
<comment>
    <comment>
        new comment
    </comment>
    <persist>
        True
    </persist>
</comment>
                    ",
                requires_full_refresh: true,
            },
        ]
    }

    #[test]
    fn test_cases() {
        run_test_cases(create_test_cases());
    }
}
