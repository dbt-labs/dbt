//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/column_comments.py

use crate::errors::AdapterResult;
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, impl_loader,
};
use crate::relation::databricks::config::{
    DatabricksRelationMetadata, DatabricksRelationMetadataKey,
};

use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use minijinja::value::{Value, ValueMap};

use indexmap::IndexMap;

pub(crate) const TYPE_NAME: &str = "column_comments";

/// Component for Databricks column comments
///
/// Holds a mapping of lowercase column names to the column comment.
pub(crate) type ColumnComments = SimpleComponentConfigImpl<IndexMap<String, String>>;

fn to_jinja(v: &IndexMap<String, String>) -> Value {
    Value::from(ValueMap::from([
        (Value::from("comments"), Value::from_serialize(v)),
        (Value::from("persist"), Value::from_serialize(!v.is_empty())),
    ]))
}

fn normalized_keys(map: &IndexMap<String, String>) -> IndexMap<String, &str> {
    map.iter()
        .map(|(k, v)| {
            let unquoted = if k.starts_with('`') && k.ends_with('`') {
                &k[1..k.len() - 1]
            } else {
                k.as_str()
            };

            let lower = unquoted.to_lowercase();

            (lower, v.as_ref())
        })
        .collect()
}
// dbt-databricks does a case/quote-insensitive match of keys
// https://github.com/databricks/dbt-databricks/blob/11f7cf7b54e410a1dca05f6f6add8cd1ff8d42d2/dbt/adapters/databricks/relation_configs/column_comments.py#L23
//
// Small deviation from Core: it performs case/quote-insensitive comparison of keys. In the final map, it keeps the original casing and quotes all columns.
// Fusion is lowercasing everything and unquoting everything to simplify. We can do that because `COMMENT ON` queries are case insensitive anyways.
//
// See: https://docs.databricks.com/aws/en/sql/language-manual/sql-ref-names
fn normalized_keys_diff(
    desired_state: &IndexMap<String, String>,
    current_state: &IndexMap<String, String>,
) -> Option<IndexMap<String, String>> {
    // Only emit COMMENT ON for columns present in the desired (model) state whose
    // comment differs from the current warehouse comment. Columns that exist
    // remotely but are undocumented in the model are left untouched, never cleared.
    // Reference: https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/column_comments.py
    let desired = normalized_keys(desired_state);
    let current = normalized_keys(current_state);

    let missing: Vec<&str> = desired
        .keys()
        .filter(|k| !current.contains_key(k.as_str()))
        .map(|k| k.as_str())
        .collect();
    if !missing.is_empty() {
        emit_warn_log_message(
            ErrorCode::JinjaWarn,
            format!(
                "The following columns are specified in the schema but are not present in the \
                 database and will be skipped: {}",
                missing.join(", ")
            ),
        );
    }

    let diff: IndexMap<String, String> = desired
        .into_iter()
        .filter(|(k, v)| current.get(k).is_some_and(|cur| cur != v))
        .map(|(k, v)| (k, v.to_string()))
        .collect();

    if diff.is_empty() { None } else { Some(diff) }
}

fn new_component(column_comments: IndexMap<String, String>) -> ColumnComments {
    let normalized_comments = column_comments
        .into_iter()
        .map(|(column_name, comment)| {
            debug_assert!(!column_name.is_empty());
            (column_name.to_lowercase(), comment)
        })
        .collect();

    ColumnComments {
        type_name: TYPE_NAME,
        diff_fn: normalized_keys_diff,
        to_jinja_fn: to_jinja,
        value: normalized_comments,
    }
}

fn from_remote_state(results: &DatabricksRelationMetadata) -> AdapterResult<ColumnComments> {
    let Some(describe_extended) = results.get(&DatabricksRelationMetadataKey::DescribeExtended)
    else {
        return Ok(new_component(IndexMap::new()));
    };
    let mut comments = IndexMap::new();

    for row in describe_extended.rows().into_iter() {
        // Get col_name - if it starts with #, we've reached the end of columns
        if let Ok(col_name_value) = row.get_attr("col_name")
            && let Some(col_name_str) = col_name_value.as_str()
        {
            if col_name_str.starts_with('#') {
                break;
            }

            // Skip empty column names (metadata rows)
            if col_name_str.trim().is_empty() {
                continue;
            }

            let comment = if let Ok(comment_value) = row.get_attr("comment") {
                comment_value.as_str().unwrap_or("").to_string()
            } else {
                String::new()
            };

            comments.insert(col_name_str.to_lowercase(), comment);
        }
    }

    // The engine might return them in random order so sorting to always be consistent
    comments.sort_keys();

    Ok(new_component(comments))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<ColumnComments> {
    let columns = &relation_config.base().columns;

    let persist = relation_config
        .base()
        .persist_docs
        .as_ref()
        .map(|pd| pd.relation.unwrap_or(false))
        .unwrap_or(false);

    let mut comments = IndexMap::new();
    if persist {
        for column in columns {
            // NOTE: ignore quote config here, dbt-databricks seems to normalize everything at the
            // end anyways
            comments.insert(
                column.name.clone(),
                column
                    .description
                    .as_ref()
                    .unwrap_or(&String::new())
                    .clone(),
            );
        }
    }

    Ok(new_component(comments))
}

impl_loader!(ColumnComments, DatabricksRelationMetadata);

impl ColumnCommentsLoader {
    pub fn new_component_type_erased(
        column_comments: IndexMap<String, String>,
    ) -> Box<dyn ComponentConfig> {
        Box::new(new_component(column_comments))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::config_v2::ComponentConfig;
    use crate::relation::databricks::config::test_helpers;
    use arrow::array::{ArrayRef, RecordBatch, StringArray};
    use arrow::csv::{Reader, ReaderBuilder};
    use arrow_schema::{DataType, Field, Schema};
    use dbt_agate::AgateTable;
    use dbt_schemas::schemas::DbtModel;
    use regex::Regex;
    use std::io;
    use std::sync::{Arc, LazyLock};

    static SCHEMA: LazyLock<Arc<Schema>> = LazyLock::new(|| {
        Arc::new(Schema::new(vec![
            Field::new("col_name", DataType::Utf8, false),
            Field::new("data_type", DataType::Utf8, false),
            Field::new("comment", DataType::Utf8, false),
        ]))
    });

    static NULL_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new("NULL").unwrap());

    fn create_reader(file: &str) -> Reader<io::Cursor<&str>> {
        ReaderBuilder::new(Arc::clone(&SCHEMA))
            .with_header(true)
            .with_null_regex(NULL_REGEX.clone())
            .build(io::Cursor::new(file))
            .unwrap()
    }

    fn create_mock_describe_extended_table() -> AgateTable {
        let file = "col_name,data_type,comment\n\
id,int,Primary key identifier\n\
name,string,User name\n\
email,string,\n\
# Detailed Table Information,,\n";
        let mut reader = create_reader(file);
        let batch = reader.next().unwrap().unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    fn create_mock_dbt_model(comments: IndexMap<&str, &str>, persist_relation: bool) -> DbtModel {
        let cfg = test_helpers::TestModelConfig {
            columns: comments
                .into_iter()
                .map(|(name, comment)| test_helpers::TestModelColumn {
                    name: name.to_string(),
                    comment: Some(comment.to_string()),
                    ..Default::default()
                })
                .collect(),
            persist_relation_comments: persist_relation,
            persist_column_comments: false,
            ..Default::default()
        };
        test_helpers::create_mock_dbt_model(cfg)
    }

    #[test]
    fn test_from_remote_state() {
        let table = create_mock_describe_extended_table();
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        let config = from_remote_state(&results).unwrap();

        assert_eq!(config.value.len(), 3);
        assert_eq!(
            config.value.get("id"),
            Some(&"Primary key identifier".to_string())
        );
        assert_eq!(config.value.get("name"), Some(&"User name".to_string()));
        assert_eq!(config.value.get("email"), Some(&"".to_string()));
    }

    #[test]
    fn test_from_remote_state_missing_describe_extended() {
        let results = IndexMap::new();
        let config = from_remote_state(&results).unwrap();

        assert!(config.value.is_empty());
    }

    #[test]
    fn test_from_remote_state_empty_table() {
        let record_batch = RecordBatch::try_new(
            Arc::clone(&SCHEMA),
            vec![
                Arc::new(StringArray::new_null(0)) as ArrayRef,
                Arc::new(StringArray::new_null(0)) as ArrayRef,
                Arc::new(StringArray::new_null(0)) as ArrayRef,
            ],
        )
        .unwrap();
        let table = AgateTable::from_record_batch(Arc::new(record_batch));
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        let config = from_remote_state(&results).unwrap();

        assert_eq!(config.value.len(), 0);
    }

    #[test]
    fn test_from_remote_state_mixed_case_columns() {
        let file = "col_name,data_type,comment\n\
    ID,int,Primary key\n\
    Name,string,User name\n\
    EMAIL,string,Email address\n\
    # Detailed Table Information,,\n";
        let mut reader = create_reader(file);
        let batch = reader.next().unwrap().unwrap();
        let table = AgateTable::from_record_batch(Arc::new(batch));
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        let config = from_remote_state(&results).unwrap();

        assert_eq!(config.value.len(), 3);
        assert_eq!(config.value.get("id"), Some(&"Primary key".to_string()));
        assert_eq!(config.value.get("name"), Some(&"User name".to_string()));
        assert_eq!(
            config.value.get("email"),
            Some(&"Email address".to_string())
        );
    }

    #[test]
    fn test_from_remote_state_delimiter_variations() {
        let file = "col_name,data_type,comment\n\
    id,int,Primary key\n\
    #Detailed Table Information,,\n\
    name,string,Should not be included\n";
        let mut reader = create_reader(file);
        let batch = reader.next().unwrap().unwrap();
        let table = AgateTable::from_record_batch(Arc::new(batch));
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        let config = from_remote_state(&results).unwrap();

        assert_eq!(config.value.len(), 1);
        assert_eq!(config.value.get("id"), Some(&"Primary key".to_string()));
        assert!(!config.value.contains_key("name"));
    }

    #[test]
    fn test_from_remote_state_missing_comment_column() {
        let file = "col_name,data_type\n\
    id,int\n\
    name,string\n";
        let mut reader = ReaderBuilder::new(Arc::new(Schema::new(
            [
                SCHEMA.fields()[0].clone(),
                SCHEMA.fields()[1].clone(),
                // Missing comment column
            ]
            .to_vec(),
        )))
        .with_header(true)
        .with_null_regex(NULL_REGEX.clone())
        .build(io::Cursor::new(file))
        .unwrap();
        let batch = reader.next().unwrap().unwrap();
        let table = AgateTable::from_record_batch(Arc::new(batch));
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        let config = from_remote_state(&results).unwrap();

        assert_eq!(config.value.len(), 2);
        assert_eq!(config.value.get("id"), Some(&"".to_string()));
        assert_eq!(config.value.get("name"), Some(&"".to_string()));
    }

    #[test]
    fn test_from_remote_state_skips_empty_column_names() {
        let file = "col_name,data_type,comment\n\
    id,int,Primary key\n\
    ,,\n\
    name,string,User name\n\
      ,,\n\
    # Detailed Table Information,'',''\n";
        let mut reader = create_reader(file);
        let batch = reader.next().unwrap().unwrap();
        let table = AgateTable::from_record_batch(Arc::new(batch));
        let results = IndexMap::from([(DatabricksRelationMetadataKey::DescribeExtended, table)]);
        let config = from_remote_state(&results).unwrap();

        assert_eq!(config.value.len(), 2);
        assert_eq!(config.value.get("id"), Some(&"Primary key".to_string()));
        assert_eq!(config.value.get("name"), Some(&"User name".to_string()));
        assert!(!config.value.contains_key(""));
    }

    #[test]
    fn test_from_local_config_with_persist() {
        let columns = IndexMap::from_iter([("id", "Primary key"), ("name", "User name")]);
        let mock_node = create_mock_dbt_model(columns, true);
        let config = from_local_config(&mock_node).unwrap();

        assert_eq!(config.value.len(), 2);
        assert_eq!(config.value.get("id"), Some(&"Primary key".to_string()));
        assert_eq!(config.value.get("name"), Some(&"User name".to_string()));
    }

    #[test]
    fn test_from_local_config_without_persist() {
        let columns = IndexMap::from_iter([("id", "Primary key")]);
        let mock_node = create_mock_dbt_model(columns, false);
        let config = from_local_config(&mock_node).unwrap();

        assert_eq!(config.value.len(), 0);
    }

    #[test]
    fn test_column_comments_diff_no_changes() {
        let mut comments = IndexMap::new();
        comments.insert("id".to_string(), "Primary key".to_string());
        comments.insert("name".to_string(), "User name".to_string());
        let config = new_component(comments);
        let diff = ColumnComments::diff_from(&config, Some(&config));

        assert!(diff.is_none());
    }

    #[test]
    fn test_column_comments_diff_with_changes() {
        let mut new_comments = IndexMap::new();
        new_comments.insert("id".to_string(), "Updated primary key".to_string());
        new_comments.insert("NAME".to_string(), "User name".to_string());
        new_comments.insert("age".to_string(), "10".to_string());
        let new_config = new_component(new_comments);

        let mut old_comments = IndexMap::new();
        old_comments.insert("id".to_string(), "Primary key".to_string());
        old_comments.insert("name".to_string(), "User name".to_string());
        old_comments.insert("`age`".to_string(), "20".to_string());
        let old_config = new_component(old_comments);

        let diff = ColumnComments::diff_from(&new_config, Some(&old_config));
        assert!(diff.is_some());
        let diff_config = diff.unwrap();
        let diff_config = diff_config
            .as_any()
            .downcast_ref::<ColumnComments>()
            .unwrap();
        assert_eq!(diff_config.value.len(), 2);
        assert_eq!(
            diff_config.value.get("id"),
            Some(&"Updated primary key".to_string())
        );
        assert_eq!(diff_config.value.get("age"), Some(&"10".to_string()));
    }

    #[test]
    fn test_column_comments_diff_skips_column_missing_from_relation() {
        let mut desired = IndexMap::new();
        desired.insert("id".to_string(), "New primary key".to_string());
        desired.insert("email".to_string(), "Email address".to_string());
        let new_config = new_component(desired);

        let mut current = IndexMap::new();
        current.insert("id".to_string(), "Primary key".to_string());
        let old_config = new_component(current);

        let diff = ColumnComments::diff_from(&new_config, Some(&old_config));
        assert!(diff.is_some());
        let diff_config = diff.unwrap();
        let diff_config = diff_config
            .as_any()
            .downcast_ref::<ColumnComments>()
            .unwrap();
        assert_eq!(diff_config.value.len(), 1);
        assert_eq!(
            diff_config.value.get("id"),
            Some(&"New primary key".to_string())
        );
        assert!(!diff_config.value.contains_key("email"));
    }

    #[test]
    fn test_column_comments_diff_all_columns_missing_returns_none() {
        let mut desired = IndexMap::new();
        desired.insert("ghost".to_string(), "not in relation".to_string());
        let new_config = new_component(desired);

        let old_config = new_component(IndexMap::new());

        let diff = ColumnComments::diff_from(&new_config, Some(&old_config));
        assert!(diff.is_none());
    }

    #[test]
    fn test_column_comments_diff_with_dropped_comment() {
        // A comment present remotely but absent from the model is left untouched,
        // so the diff is empty.
        let mut new_comments = IndexMap::new();
        new_comments.insert("id".to_string(), "Primary key".to_string());
        let new_config = new_component(new_comments);

        let mut old_comments = IndexMap::new();
        old_comments.insert("id".to_string(), "Primary key".to_string());
        old_comments.insert("name".to_string(), "User name".to_string());
        let old_config = new_component(old_comments);

        let diff = ColumnComments::diff_from(&new_config, Some(&old_config));
        assert!(diff.is_none());
    }

    #[test]
    fn test_column_comments_diff_explicit_clear() {
        // Setting a documented column's comment to "" clears an existing remote comment.
        let mut new_comments = IndexMap::new();
        new_comments.insert("id".to_string(), "".to_string());
        let new_config = new_component(new_comments);

        let mut old_comments = IndexMap::new();
        old_comments.insert("id".to_string(), "Primary key".to_string());
        let old_config = new_component(old_comments);

        let diff = ColumnComments::diff_from(&new_config, Some(&old_config));
        assert!(diff.is_some());
        let diff_config = diff.unwrap();
        let diff_config = diff_config
            .as_any()
            .downcast_ref::<ColumnComments>()
            .unwrap();
        assert_eq!(diff_config.value.len(), 1);
        assert_eq!(diff_config.value.get("id"), Some(&"".to_string()));
    }
}
