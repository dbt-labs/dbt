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
        Box::new(components::ViewQueryLoader),
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
        test_helpers::{TestModelColumn, TestModelConfig, create_mock_dbt_model, run_test_cases},
    };
    use crate::relation::test_helpers::TestCase;
    use indexmap::IndexMap;
    use minijinja::value::Value;
    use minijinja_contrib::testing::jinja_assert;
    use std::collections::BTreeMap;

    fn changeset_context(
        current: TestModelConfig,
        desired: TestModelConfig,
    ) -> BTreeMap<String, Value> {
        let loader = new_loader();
        BTreeMap::from([
            (
                "existing".into(),
                Value::from_object(
                    loader
                        .from_local_config(&create_mock_dbt_model(current))
                        .unwrap(),
                ),
            ),
            (
                "desired".into(),
                Value::from_object(
                    loader
                        .from_local_config(&create_mock_dbt_model(desired))
                        .unwrap(),
                ),
            ),
        ])
    }

    #[test]
    fn unchanged_queries_containing_star_are_reapplied() {
        for query in [
            "select * from foo",
            "select foo.* from foo",
            "with src as (select * from foo) select id from src",
            "select * except (secret) from foo",
            "select count(*) from foo",
            "select id * 2 from foo",
            "select '*' as literal from foo",
            "select id from foo -- *",
        ] {
            let mut context = changeset_context(
                TestModelConfig {
                    query: Some(query.into()),
                    ..Default::default()
                },
                TestModelConfig {
                    query: Some(query.into()),
                    ..Default::default()
                },
            );
            context.insert("query".into(), Value::from(query));
            jinja_assert(
                context,
                "
                {% set changeset = obj.desired.get_changeset(obj.existing) %}
                {{ changeset is not none }}
                {{ changeset.requires_full_refresh }}
                {{ changeset.changes | length }}
                {{ changeset.changes.query.query == obj.query }}
            ",
                "True\nFalse\n1\nTrue",
            );
        }
    }

    #[test]
    fn unchanged_explicit_query_has_no_changeset() {
        let config = || TestModelConfig {
            query: Some("select id from foo".into()),
            ..Default::default()
        };
        jinja_assert(
            changeset_context(config(), config()),
            "
            {{ obj.desired.get_changeset(obj.existing) is none }}
        ",
            "True",
        );
    }

    #[test]
    fn query_refresh_preserves_configuration_changes() {
        for (query, expected) in [
            ("select id from foo", "False\nFalse\nnew comment\nvalue"),
            ("select * from foo", "True\nFalse\nnew comment\nvalue"),
        ] {
            let config = || TestModelConfig {
                query: Some(query.into()),
                persist_column_comments: true,
                columns: vec![TestModelColumn {
                    name: "id".into(),
                    comment: Some("old comment".into()),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let desired = TestModelConfig {
                columns: vec![TestModelColumn {
                    name: "id".into(),
                    comment: Some("new comment".into()),
                    ..Default::default()
                }],
                tbl_properties: IndexMap::from([("key".into(), "value".into())]),
                ..config()
            };
            jinja_assert(
                changeset_context(config(), desired),
                "
                {% set changeset = obj.desired.get_changeset(obj.existing) %}
                {{ changeset.changes.query is defined }}
                {{ changeset.requires_full_refresh }}
                {{ changeset.changes.column_comments.comments.id }}
                {{ changeset.changes.tblproperties.tblproperties.key }}
            ",
                expected,
            );
        }
    }

    #[test]
    fn description_change_still_requires_replacement() {
        for query in ["select id from foo", "select * from foo"] {
            let current = TestModelConfig {
                query: Some(query.into()),
                persist_relation_comments: true,
                ..Default::default()
            };
            let desired = TestModelConfig {
                query: Some(query.into()),
                persist_relation_comments: true,
                relation_comment: Some("new description".into()),
                ..Default::default()
            };
            jinja_assert(
                changeset_context(current, desired),
                "
                {% set changeset = obj.desired.get_changeset(obj.existing) %}
                {{ changeset.requires_full_refresh }}
                {{ changeset.changes.comment.comment }}
            ",
                "True\nnew description",
            );
        }
    }

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
