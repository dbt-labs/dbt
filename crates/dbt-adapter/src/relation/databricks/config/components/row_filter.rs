//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/row_filter.py

use dbt_common::AdapterResult;
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;
use serde::Serialize;

use crate::errors::{AdapterError, AdapterErrorKind};
use crate::relation::{
    config_v2::{ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, impl_loader},
    databricks::config::{DatabricksRelationMetadata, DatabricksRelationMetadataKey},
};

pub(crate) const TYPE_NAME: &str = "row_filter";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Config {
    // Fully qualified function name
    pub function: Option<String>,
    // Column names passed to the filter function
    pub columns: Vec<String>,
    // True when this instance represents a diff meaning "unset/drop the filter"
    pub should_unset: bool,
    // True when this represents an actual change that should trigger ALTER
    // (distinguishes diff result from `diff or value` fallback state)
    pub is_change: bool,
}

// Component for Databricks row filter config
pub type RowFilter = SimpleComponentConfigImpl<Config>;

/// Reference: https://github.com/databricks/dbt-databricks/blob/45351e11517d3f37c5ac7a736b5fcba453d3f368/dbt/adapters/databricks/relation_configs/row_filter.py#L51
fn diff(desired_state: &Config, current_state: &Config) -> Option<Config> {
    // Case 1: No filter desired, no filter exists -> no change
    if desired_state.function.is_none() && current_state.function.is_none() {
        return None;
    }

    // Case 2: No filter desired, filter exists -> unset it
    if desired_state.function.is_none() && current_state.function.is_some() {
        return Some(Config {
            function: None,
            columns: vec![],
            should_unset: true,
            is_change: true,
        });
    }

    let normalize = |s: &str| s.to_lowercase().replace("`", "");

    // Case 3: Filter desired, compare with existing
    let desired_columns: Vec<String> = desired_state.columns.iter().map(|s| normalize(s)).collect();
    let current_columns: Vec<String> = current_state.columns.iter().map(|s| normalize(s)).collect();
    if desired_state.function.as_deref().map(normalize)
        == current_state.function.as_deref().map(normalize)
        && desired_columns == current_columns
    {
        return None;
    }

    // Filter is new or changed -> return new instance with is_change = true
    let mut change = desired_state.clone();
    change.is_change = true;
    Some(change)
}

fn new_component(function: Option<String>, columns: Vec<String>) -> RowFilter {
    RowFilter {
        type_name: TYPE_NAME,
        diff_fn: diff,
        to_jinja_fn: |v| Value::from_serialize(v),
        value: Config {
            function,
            columns,
            should_unset: false,
            is_change: false,
        },
    }
}

fn from_remote_state(results: &DatabricksRelationMetadata) -> AdapterResult<RowFilter> {
    let (filter_name_idx, target_columns_idx) = (Value::from(3), Value::from(4));

    let Some(table) = results.get(&DatabricksRelationMetadataKey::RowFilters) else {
        return Ok(new_component(None, vec![]));
    };

    let rows: Vec<_> = table.rows().into_iter().collect();
    match rows.len() {
        0 => Ok(new_component(None, vec![])),
        1 => {
            let row = &rows[0];
            let function = row
                .get_item(&filter_name_idx)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string));
            let columns = row
                .get_item(&target_columns_idx)
                .ok()
                .and_then(|v| v.as_str().map(parse_target_columns))
                .unwrap_or_default();
            Ok(new_component(function, columns))
        }
        _ => {
            let filter_names = rows
                .iter()
                .map(|row| {
                    let name = row
                        .get_item(&filter_name_idx)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default();
                    format!("'{name}'")
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Multiple row filters found: [{filter_names}]. \
                     This may indicate ABAC-derived filters or a platform issue. \
                     dbt expects a single row filter per table."
                ),
            ))
        }
    }
}

// https://github.com/databricks/dbt-databricks/blob/f65356ef59a5996bebf2c86296f32295815a7bb3/dbt/adapters/databricks/relation_configs/row_filter.py#L193
// TODO: This does not support quoted values with embedded commas. dbt-databricks uses a CSV parser
// to handle strings like '"col1", "col2"'
fn parse_target_columns(target_columns: &str) -> Vec<String> {
    target_columns
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Reference: https://github.com/databricks/dbt-databricks/blob/f65356ef59a5996bebf2c86296f32295815a7bb3/dbt/adapters/databricks/relation_configs/row_filter.py#L152
fn qualify_function_name(function: &str, database: &str, schema: &str) -> AdapterResult<String> {
    let normalized = function.replace('`', "");
    let parts = normalized.split('.').collect::<Vec<_>>();

    match parts.as_slice() {
        [function] => Ok(format!("{database}.{schema}.{function}")),
        [database, schema, function] => Ok(format!("{database}.{schema}.{function}")),
        [_, _] => Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Row filter function '{function}' is ambiguous. Use either an unqualified name \
                 (e.g., 'my_filter') or a fully qualified name \
                 (e.g., 'catalog.schema.my_filter')."
            ),
        )),
        _ => Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Row filter function '{function}' has too many parts. Expected format: \
                 'catalog.schema.function_name'."
            ),
        )),
    }
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<RowFilter> {
    let Some(model) = relation_config.as_any().downcast_ref::<DbtModel>() else {
        return Ok(new_component(None, vec![]));
    };
    let Some(row_filter) = model
        .__adapter_attr__
        .databricks_attr
        .as_ref()
        .and_then(|attr| attr.row_filter.as_ref())
    else {
        return Ok(new_component(None, vec![]));
    };

    let function = row_filter
        .function
        .as_deref()
        .filter(|function| !function.trim().is_empty())
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                "Row filter requires a non-empty 'function' value.",
            )
        })?;
    let columns = row_filter
        .columns
        .as_ref()
        .map(|columns| columns.to_strings())
        .filter(|columns| !columns.is_empty())
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Row filter function '{function}' requires a non-empty 'columns' value. \
                     Example: columns: region OR columns: ['region_id', 'country_code']"
                ),
            )
        })?;

    for (index, column) in columns.iter().enumerate() {
        if column.trim().is_empty() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Row filter column at index {index} must be a non-empty string. Got: \
                     {column:?}"
                ),
            ));
        }
    }

    let function = qualify_function_name(
        function,
        &model.__base_attr__.database,
        &model.__base_attr__.schema,
    )?;
    Ok(new_component(Some(function), columns))
}

impl_loader!(RowFilter, DatabricksRelationMetadata);

impl RowFilterLoader {
    pub fn new_component_type_erased(
        function: Option<String>,
        columns: Vec<String>,
    ) -> Box<dyn ComponentConfig> {
        let should_unset = function.is_none();
        let mut component = new_component(function, columns);
        component.value.should_unset = should_unset;
        component.value.is_change = true;
        Box::new(component)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::databricks::config::test_helpers;
    use dbt_agate::AgateTable;
    use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
    use indexmap::IndexMap;
    use std::sync::Arc;

    fn cfg(function: Option<&str>, columns: &[&str]) -> Config {
        Config {
            function: function.map(str::to_string),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            should_unset: false,
            is_change: false,
        }
    }

    fn create_mock_dbt_model(function: Option<&str>, columns: &[&str]) -> DbtModel {
        let cfg = test_helpers::TestModelConfig {
            row_filter_function: function.map(|s| s.to_string()),
            row_filter_columns: columns.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };

        test_helpers::create_mock_dbt_model(cfg)
    }

    fn create_mock_row_filters_table(rows: &[[&str; 5]]) -> AgateTable {
        use arrow::array::{ArrayRef, StringArray};
        use arrow::record_batch::RecordBatch;
        use arrow_schema::{DataType, Field, Schema};

        let names = [
            "table_catalog",
            "table_schema",
            "table_name",
            "filter_name",
            "target_columns",
        ];
        let schema = Arc::new(Schema::new(
            names
                .iter()
                .map(|n| Field::new(*n, DataType::Utf8, true))
                .collect::<Vec<_>>(),
        ));
        let arrays: Vec<ArrayRef> = (0..names.len())
            .map(|col| {
                let values: Vec<&str> = rows.iter().map(|row| row[col]).collect();
                Arc::new(StringArray::from(values)) as ArrayRef
            })
            .collect();
        let batch = RecordBatch::try_new(schema, arrays).unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    #[test]
    fn test_no_change_when_both_none() {
        assert!(diff(&cfg(None, &[]), &cfg(None, &[])).is_none());
    }

    #[test]
    fn test_unset_when_removed() {
        let change = diff(&cfg(None, &[]), &cfg(Some("cat.schema.fn"), &["col1"])).unwrap();
        assert!(change.should_unset);
        assert!(change.is_change);
        assert_eq!(change.function, None);
    }

    #[test]
    fn test_set_when_new() {
        let change = diff(&cfg(Some("cat.schema.fn"), &["col1"]), &cfg(None, &[])).unwrap();
        assert_eq!(change.function.as_deref(), Some("cat.schema.fn"));
        assert!(!change.should_unset);
        assert!(change.is_change);
    }

    #[test]
    fn test_no_change_when_equal_case_insensitive() {
        assert!(
            diff(
                &cfg(Some("CAT.SCHEMA.FN"), &["COL1"]),
                &cfg(Some("cat.schema.fn"), &["col1"]),
            )
            .is_none()
        );
    }

    #[test]
    fn test_change_when_different_function() {
        let change = diff(
            &cfg(Some("cat.schema.fn2"), &["col1"]),
            &cfg(Some("cat.schema.fn1"), &["col1"]),
        )
        .unwrap();
        assert_eq!(change.function.as_deref(), Some("cat.schema.fn2"));
        assert!(!change.should_unset);
        assert!(change.is_change);
    }

    #[test]
    fn test_change_when_different_columns() {
        let change = diff(
            &cfg(Some("cat.schema.fn"), &["col1", "col2"]),
            &cfg(Some("cat.schema.fn"), &["col1"]),
        )
        .unwrap();
        assert_eq!(change.function.as_deref(), Some("cat.schema.fn"));
        assert_eq!(change.columns, vec!["col1".to_string(), "col2".to_string()]);
        assert!(!change.should_unset);
        assert!(change.is_change);
    }

    #[test]
    fn test_is_change_false_by_default() {
        let config = new_component(Some("cat.schema.fn".to_string()), vec!["col1".to_string()]);
        assert!(!config.value.is_change);
        assert!(!config.value.should_unset);
    }

    #[test]
    fn test_parse_target_columns_simple() {
        assert_eq!(parse_target_columns("col1, col2"), vec!["col1", "col2"]);
    }

    #[test]
    fn test_parse_target_columns_empty() {
        assert!(parse_target_columns("").is_empty());
    }

    #[test]
    fn test_from_remote_state_empty() {
        let table = create_mock_row_filters_table(&[]);
        let results = IndexMap::from([(DatabricksRelationMetadataKey::RowFilters, table)]);
        let config = from_remote_state(&results).unwrap();
        assert_eq!(config.value.function, None);
        assert!(config.value.columns.is_empty());
    }

    #[test]
    fn test_from_remote_state_one_row() {
        let table = create_mock_row_filters_table(&[[
            "cat",
            "schema",
            "my_table",
            "cat.schema.my_filter",
            "col1, col2",
        ]]);
        let results = IndexMap::from([(DatabricksRelationMetadataKey::RowFilters, table)]);
        let config = from_remote_state(&results).unwrap();
        assert_eq!(
            config.value.function.as_deref(),
            Some("cat.schema.my_filter")
        );
        assert_eq!(config.value.columns, vec!["col1", "col2"]);
    }

    #[test]
    fn test_from_remote_state_multiple_rows_raises() {
        let table = create_mock_row_filters_table(&[
            ["cat", "schema", "my_table", "cat.schema.filter1", "col1"],
            ["cat", "schema", "my_table", "cat.schema.filter2", "col2"],
        ]);
        let results = IndexMap::from([(DatabricksRelationMetadataKey::RowFilters, table)]);
        let err = from_remote_state(&results).unwrap_err();
        assert!(err.to_string().contains("Multiple row filters found"));
    }

    #[test]
    fn test_from_local_config_empty() {
        let model = create_mock_dbt_model(None, &[]);
        let config = from_local_config(&model).unwrap();

        assert_eq!(config.value.function, None);
        assert!(config.value.columns.is_empty());
    }

    #[test]
    fn test_from_local_config_function_only() {
        let model = create_mock_dbt_model(Some("my_filter"), &[]);
        let err = from_local_config(&model).unwrap_err();

        assert!(err.to_string().contains("non-empty 'columns' value"));
    }

    #[test]
    fn test_from_local_config_function_and_columns() {
        let model = create_mock_dbt_model(Some("my_filter"), &["col1", "col2"]);
        let config = from_local_config(&model).unwrap();

        assert_eq!(
            config.value.function.as_deref(),
            Some("test_db.test_schema.my_filter")
        );
        assert_eq!(config.value.columns.len(), 2);
        assert_eq!(config.value.columns[0], "col1".to_string());
        assert_eq!(config.value.columns[1], "col2".to_string());
    }

    #[test]
    fn test_from_local_config_missing_function_raises() {
        let mut model = create_mock_dbt_model(Some("my_filter"), &["col1"]);
        model
            .__adapter_attr__
            .databricks_attr
            .as_mut()
            .unwrap()
            .row_filter
            .as_mut()
            .unwrap()
            .function = None;

        let err = from_local_config(&model).unwrap_err();
        assert!(err.to_string().contains("non-empty 'function' value"));
    }

    #[test]
    fn test_from_local_config_empty_function_raises() {
        let model = create_mock_dbt_model(Some("   "), &["col1"]);

        let err = from_local_config(&model).unwrap_err();
        assert!(err.to_string().contains("non-empty 'function' value"));
    }

    #[test]
    fn test_from_local_config_missing_columns_raises() {
        let mut model = create_mock_dbt_model(Some("my_filter"), &["col1"]);
        model
            .__adapter_attr__
            .databricks_attr
            .as_mut()
            .unwrap()
            .row_filter
            .as_mut()
            .unwrap()
            .columns = None;

        let err = from_local_config(&model).unwrap_err();
        assert!(err.to_string().contains("non-empty 'columns' value"));
    }

    #[test]
    fn test_from_local_config_string_column_is_normalized() {
        let mut model = create_mock_dbt_model(Some("my_filter"), &["col1"]);
        model
            .__adapter_attr__
            .databricks_attr
            .as_mut()
            .unwrap()
            .row_filter
            .as_mut()
            .unwrap()
            .columns = Some(StringOrArrayOfStrings::String("region".to_string()));

        let config = from_local_config(&model).unwrap();
        assert_eq!(config.value.columns, vec!["region"]);
    }

    #[test]
    fn test_from_local_config_empty_column_raises() {
        let model = create_mock_dbt_model(Some("my_filter"), &["col1", "   "]);

        let err = from_local_config(&model).unwrap_err();
        assert!(err.to_string().contains("column at index 1"));
    }

    #[test]
    fn test_from_local_config_fully_qualified_function() {
        let model = create_mock_dbt_model(Some("`catalog`.`schema`.`my_filter`"), &["col1"]);
        let config = from_local_config(&model).unwrap();

        assert_eq!(
            config.value.function.as_deref(),
            Some("catalog.schema.my_filter")
        );
    }

    #[test]
    fn test_from_local_config_two_part_function_raises() {
        let model = create_mock_dbt_model(Some("schema.my_filter"), &["col1"]);

        let err = from_local_config(&model).unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
    }

    #[test]
    fn test_from_local_config_four_part_function_raises() {
        let model = create_mock_dbt_model(Some("a.b.c.my_filter"), &["col1"]);

        let err = from_local_config(&model).unwrap_err();
        assert!(err.to_string().contains("too many parts"));
    }
}
