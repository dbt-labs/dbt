//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/comment.py

use crate::errors::AdapterResult;
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};
use crate::relation::databricks::config::{
    DatabricksRelationMetadata, DatabricksRelationMetadataKey,
};
use crate::value::none_value;

use dbt_schemas::schemas::InternalDbtNodeAttributes;
use minijinja::value::{Value, ValueMap};

pub(crate) const TYPE_NAME: &str = "comment";

pub(crate) type RelationComment = SimpleComponentConfigImpl<Option<String>>;

fn to_jinja(v: &Option<String>) -> Value {
    Value::from(ValueMap::from([
        (
            Value::from("comment"),
            v.as_ref().map(Value::from).unwrap_or_else(none_value),
        ),
        (Value::from("persist"), Value::from(v.is_some())),
    ]))
}

fn new_component(comment: Option<String>) -> RelationComment {
    RelationComment {
        type_name: TYPE_NAME,
        diff_fn: diff::desired_state,
        to_jinja_fn: to_jinja,
        value: comment,
    }
}

fn from_remote_state(results: &DatabricksRelationMetadata) -> AdapterResult<RelationComment> {
    let Some(describe_extended) = results.get(&DatabricksRelationMetadataKey::DescribeExtended)
    else {
        return Ok(new_component(None));
    };

    for row in describe_extended.rows() {
        if let (Ok(key_val), Ok(value_val)) =
            (row.get_item(&Value::from(0)), row.get_item(&Value::from(1)))
            && let (Some(key_str), Some(value_str)) = (key_val.as_str(), value_val.as_str())
            && key_str == "Comment"
        {
            let comment = if !value_str.is_empty() {
                Some(value_str.to_string())
            } else {
                None
            };

            return Ok(new_component(comment));
        }
    }

    Ok(new_component(None))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<RelationComment> {
    let persist = relation_config
        .base()
        .persist_docs
        .as_ref()
        .map(|pd| pd.relation.unwrap_or(false))
        .unwrap_or(false);

    let comment_str = if persist {
        relation_config
            .common()
            .description
            .clone()
            .filter(|comment| !comment.is_empty())
    } else {
        None
    };

    Ok(new_component(comment_str))
}

impl_loader!(RelationComment, DatabricksRelationMetadata);

impl RelationCommentLoader {
    pub fn new_component_type_erased(comment: Option<String>) -> Box<dyn ComponentConfig> {
        Box::new(new_component(comment))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::databricks::config::test_helpers;
    use dbt_agate::AgateTable;
    use dbt_schemas::schemas::DbtModel;
    use indexmap::IndexMap;

    fn create_mock_describe_extended_table(comment: Option<&str>) -> AgateTable {
        let mut rows = Vec::new();
        if let Some(comment_text) = comment {
            rows.push(("Comment", comment_text));
        }

        test_helpers::create_mock_describe_extended_table([], rows)
    }

    fn create_mock_dbt_model(persist: bool, comment: Option<&str>) -> DbtModel {
        let cfg = test_helpers::TestModelConfig {
            relation_comment: comment.map(|c| c.to_string()),
            persist_relation_comments: persist,
            ..Default::default()
        };
        test_helpers::create_mock_dbt_model(cfg)
    }

    #[test]
    fn test_from_remote_state_with_comment() {
        let table = create_mock_describe_extended_table(Some("DESC"));
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        let comment_config = from_remote_state(&results).unwrap();

        assert_eq!(comment_config.value.unwrap(), "DESC");
    }

    #[test]
    fn test_from_remote_state_no_comment() {
        let table = create_mock_describe_extended_table(None);
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        let comment_config = from_remote_state(&results).unwrap();

        assert_eq!(comment_config.value, None);
    }

    #[test]
    fn test_from_local_config_with_comment_no_persist() {
        let model = create_mock_dbt_model(false, Some("DESC"));
        let comment_config = from_local_config(&model).unwrap();

        assert!(comment_config.value.is_none());
    }

    #[test]
    fn test_from_local_config_with_comment_persist() {
        let model = create_mock_dbt_model(true, Some("DESC"));
        let comment_config = from_local_config(&model).unwrap();

        assert_eq!(comment_config.value.unwrap(), "DESC");
    }

    #[test]
    fn test_from_local_config_no_comment_persist() {
        let model = create_mock_dbt_model(true, None);
        let comment_config = from_local_config(&model).unwrap();

        assert_eq!(comment_config.value, None);
    }

    #[test]
    fn test_from_local_config_empty_comment_persist() {
        let model = create_mock_dbt_model(true, Some(""));
        let comment_config = from_local_config(&model).unwrap();

        assert_eq!(
            comment_config.value, None,
            "v1 normalizes an empty persisted description to no relation comment"
        );
    }
}
