//! Constraint configuration and processing for Databricks relations
//!
//! Handles extraction and diffing of constraint configurations from database metadata
//! and dbt node definitions. Supports non-null constraints and typed constraints.
//!
//! Reference: https://github.com/databricks/dbt-databricks/blob/e7099a2c75a92fa5240989b19d246a0ca8a313ef/dbt/adapters/databricks/relation_configs/constraints.py

use crate::errors::AdapterResult;
use crate::relation::config_v2::{ComponentConfig, ComponentConfigLoader, impl_loader};
use crate::relation::databricks::config::{
    DatabricksRelationMetadata, DatabricksRelationMetadataKey,
};
use crate::relation::databricks::typed_constraint::TypedConstraint;

use dbt_schemas::schemas::InternalDbtNodeAttributes;
use indexmap::{IndexMap, IndexSet};
use minijinja::Value;
use minijinja::value::{Enumerator, Object};
use serde::Serialize;
use std::sync::Arc;

pub(crate) const TYPE_NAME: &str = "constraints";

#[derive(Debug, Default, Clone, Serialize)]
pub struct Constraints {
    pub set_non_nulls: IndexSet<String>,
    pub unset_non_nulls: IndexSet<String>,
    pub set_constraints: IndexSet<TypedConstraint>,
    pub unset_constraints: IndexSet<TypedConstraint>,
}

impl Constraints {
    fn new(
        set_non_nulls: IndexSet<String>,
        unset_non_nulls: IndexSet<String>,
        set_constraints: IndexSet<TypedConstraint>,
        unset_constraints: IndexSet<TypedConstraint>,
    ) -> Self {
        Self {
            set_non_nulls,
            unset_non_nulls,
            set_constraints,
            unset_constraints,
        }
    }

    /// Normalize expression for comparison by standardizing format
    ///
    /// This function standardizes SQL expressions for consistent comparison.
    /// Reference: https://raw.githubusercontent.com/databricks/dbt-databricks/refs/tags/v1.10.9/dbt/adapters/databricks/relation_configs/constraints.py
    fn normalize_expression(expression: &str) -> String {
        if expression.is_empty() {
            return expression.to_string();
        }

        // TODO: Implement proper SQL formatting similar to sqlparse
        // The Python implementation uses sqlparse with specific formatting options:
        // - reindent=True
        // - keyword_case="lower"
        // - identifier_case="lower"
        //
        // For now, do basic normalization
        expression
            .trim()
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Normalize a constraint for comparison by standardizing format and removing irrelevant fields
    ///
    /// This is necessary because Databricks:
    /// - Reformats expressions for check constraints
    /// - Does not persist the `columns` in check constraints
    /// - Stores `custom` PK expressions (e.g. `primary key (\`id\`) rely`) as native PK constraints
    ///   in `information_schema`, so a local `Custom` PK must normalize to `PrimaryKey` for the
    ///   diff to be stable across incremental runs.
    ///
    /// Reference: https://raw.githubusercontent.com/databricks/dbt-databricks/refs/tags/v1.10.9/dbt/adapters/databricks/relation_configs/constraints.py
    fn normalize_constraint(constraint: &TypedConstraint) -> TypedConstraint {
        match constraint {
            TypedConstraint::Check {
                name, expression, ..
            } => TypedConstraint::Check {
                name: name.clone(),
                expression: Self::normalize_expression(expression),
                columns: None, // Databricks doesn't persist columns for check constraints
            },
            TypedConstraint::Custom {
                name, expression, ..
            } => {
                let normalized = Self::normalize_expression(expression);
                Self::try_parse_custom_as_pk(name.clone(), &normalized).unwrap_or_else(|| {
                    TypedConstraint::Custom {
                        name: name.clone(),
                        expression: normalized,
                        columns: None,
                    }
                })
            }
            _ => constraint.clone(),
        }
    }

    /// Attempt to reinterpret a normalized custom expression as a PrimaryKey constraint.
    ///
    /// Databricks stores `primary key (\`id\`) rely` as a native PK visible in
    /// `information_schema.key_column_usage`, so normalizing Custom→PrimaryKey lets the
    /// diff treat them as equal and avoids a spurious DROP + re-ADD on every incremental run.
    fn try_parse_custom_as_pk(
        name: Option<String>,
        normalized_expr: &str,
    ) -> Option<TypedConstraint> {
        let rest = normalized_expr.strip_prefix("primary key")?.trim();
        if !rest.starts_with('(') {
            return None;
        }
        let paren_end = rest.find(')')?;
        let cols_str = &rest[1..paren_end];
        let columns: Vec<String> = cols_str
            .split(',')
            .map(|s| s.trim().trim_matches('`').trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if columns.is_empty() {
            return None;
        }
        Some(TypedConstraint::PrimaryKey {
            name,
            columns,
            expression: None,
        })
    }

    /// Process check constraints from table properties
    /// Based on: https://github.com/databricks/dbt-databricks/blob/e7099a2c75a92fa5240989b19d246a0ca8a313ef/dbt/adapters/databricks/relation_configs/constraints.py#L53
    fn process_check_constraints(
        table_properties: Option<&dbt_agate::AgateTable>,
    ) -> IndexSet<TypedConstraint> {
        let mut check_constraints = IndexSet::new();
        if let Some(table) = table_properties {
            for row in table.rows() {
                if let (Ok(property_name), Ok(property_value)) = (
                    row.get_attr("key")
                        .or_else(|_| row.get_attr("property_name")),
                    row.get_attr("value")
                        .or_else(|_| row.get_attr("property_value")),
                ) && let (Some(name_str), Some(value_str)) =
                    (property_name.as_str(), property_value.as_str())
                    && name_str.starts_with("delta.constraints.")
                {
                    let constraint_name = name_str.strip_prefix("delta.constraints.").unwrap();
                    check_constraints.insert(TypedConstraint::Check {
                        name: Some(constraint_name.to_string()),
                        expression: value_str.to_string(),
                        columns: None,
                    });
                }
            }
        }

        check_constraints
    }

    /// Process primary key constraints
    /// Based on: https://github.com/databricks/dbt-databricks/blob/e7099a2c75a92fa5240989b19d246a0ca8a313ef/dbt/adapters/databricks/relation_configs/constraints.py#L69
    fn process_primary_key_constraints(
        pk_table: Option<&dbt_agate::AgateTable>,
    ) -> IndexSet<TypedConstraint> {
        let mut pk_constraints = IndexSet::new();
        if let Some(table) = pk_table {
            let mut constraint_columns: IndexMap<String, Vec<String>> = IndexMap::new();

            for row in table.rows() {
                if let (Ok(constraint_name), Ok(column_name)) =
                    (row.get_attr("constraint_name"), row.get_attr("column_name"))
                    && let (Some(name_str), Some(col_str)) =
                        (constraint_name.as_str(), column_name.as_str())
                {
                    constraint_columns
                        .entry(name_str.to_string())
                        .or_default()
                        .push(col_str.to_string());
                }
            }

            for (constraint_name, columns) in constraint_columns {
                pk_constraints.insert(TypedConstraint::PrimaryKey {
                    name: Some(constraint_name),
                    columns,
                    expression: None,
                });
            }
        }

        pk_constraints
    }

    /// Process foreign key constraints
    /// Based on: https://github.com/databricks/dbt-databricks/blob/e7099a2c75a92fa5240989b19d246a0ca8a313ef/dbt/adapters/databricks/relation_configs/constraints.py#L87
    fn process_foreign_key_constraints(
        fk_table: Option<&dbt_agate::AgateTable>,
    ) -> IndexSet<TypedConstraint> {
        let mut fk_constraints = IndexSet::new();
        if let Some(table) = fk_table {
            let mut fk_data: IndexMap<String, FkData> = IndexMap::new();

            for row in table.rows() {
                if let (
                    Ok(constraint_name),
                    Ok(column_name),
                    Ok(to_catalog),
                    Ok(to_schema),
                    Ok(to_table),
                    Ok(to_column),
                ) = (
                    row.get_attr("constraint_name"),
                    row.get_attr("from_column"),
                    row.get_attr("to_catalog"),
                    row.get_attr("to_schema"),
                    row.get_attr("to_table"),
                    row.get_attr("to_column"),
                ) && let (
                    Some(name_str),
                    Some(col_str),
                    Some(catalog_str),
                    Some(schema_str),
                    Some(table_str),
                    Some(to_col_str),
                ) = (
                    constraint_name.as_str(),
                    column_name.as_str(),
                    to_catalog.as_str(),
                    to_schema.as_str(),
                    to_table.as_str(),
                    to_column.as_str(),
                ) {
                    let entry = fk_data
                        .entry(name_str.to_string())
                        .or_insert_with(|| FkData {
                            columns: Vec::new(),
                            to: format!("`{catalog_str}`.`{schema_str}`.`{table_str}`"),
                            to_columns: Vec::new(),
                        });
                    entry.columns.push(col_str.to_string());
                    entry.to_columns.push(to_col_str.to_string());
                }
            }

            for (constraint_name, data) in fk_data {
                fk_constraints.insert(TypedConstraint::ForeignKey {
                    name: Some(constraint_name),
                    columns: data.columns,
                    to: Some(data.to),
                    to_columns: Some(data.to_columns),
                    expression: None,
                });
            }
        }

        fk_constraints
    }

    fn from_remote_state(results: &DatabricksRelationMetadata) -> Self {
        let mut non_null_columns = results
            .get(&DatabricksRelationMetadataKey::NonNullConstraints)
            .map(|table| {
                table
                    .rows()
                    .into_iter()
                    .filter_map(|row| {
                        // Different metadata sources use "column_name" or "col_name"; try both.
                        row.get_attr("column_name")
                            .ok()
                            .map(|v| v.to_string())
                            .filter(|s| !s.is_empty() && s != "undefined")
                            .or_else(|| {
                                row.get_attr("col_name")
                                    .ok()
                                    .map(|v| v.to_string())
                                    .filter(|s| !s.is_empty() && s != "undefined")
                            })
                    })
                    .collect::<IndexSet<_>>()
            })
            .unwrap_or_default();

        // The engine might return them in random order so sorting to always be consistent
        non_null_columns.sort();

        let check_constraints = Self::process_check_constraints(
            results.get(&DatabricksRelationMetadataKey::ShowTblProperties),
        );

        let pk_constraints = Self::process_primary_key_constraints(
            results.get(&DatabricksRelationMetadataKey::PrimaryKeyConstraints),
        );

        let fk_constraints = Self::process_foreign_key_constraints(
            results.get(&DatabricksRelationMetadataKey::ForeignKeyConstraints),
        );

        let mut all_constraints = IndexSet::new();
        all_constraints.extend(check_constraints);
        all_constraints.extend(pk_constraints);
        all_constraints.extend(fk_constraints);

        // The engine might return them in random order so sorting to always be consistent
        all_constraints.sort();

        Self::new(
            non_null_columns,
            IndexSet::new(),
            all_constraints,
            IndexSet::new(),
        )
    }

    fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> Self {
        let contract_enforced = relation_config
            .as_any()
            .downcast_ref::<dbt_schemas::schemas::nodes::DbtModel>()
            .and_then(|model| model.deprecated_config.contract.as_ref())
            .is_some_and(|contract| contract.enforced);
        if !contract_enforced {
            return Self::default();
        }

        let columns = &relation_config.base().columns;

        let model_constraints = if let Some(model) = relation_config
            .as_any()
            .downcast_ref::<dbt_schemas::schemas::nodes::DbtModel>(
        ) {
            model.__model_attr__.constraints.as_slice()
        } else {
            &[]
        };

        let Ok((not_null_columns, other_constraints)) =
            crate::relation::databricks::typed_constraint::parse_constraints(
                columns,
                model_constraints,
            )
        else {
            return Self::default();
        };

        let constraints_set: IndexSet<_> = other_constraints.into_iter().collect();

        // FIXME(serramatutu): make `parse_constraints` return `IndexSet` directly once old code
        // that relies on `BTreeSet` is deleted
        Self::new(
            IndexSet::from_iter(not_null_columns),
            IndexSet::new(),
            IndexSet::from_iter(constraints_set),
            IndexSet::new(),
        )
    }
}

fn from_remote_state(state: &DatabricksRelationMetadata) -> AdapterResult<Constraints> {
    Ok(Constraints::from_remote_state(state))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<Constraints> {
    Ok(Constraints::from_local_config(relation_config))
}

impl_loader!(Constraints, DatabricksRelationMetadata);

impl ConstraintsLoader {
    pub fn new_component_type_erased(
        set_non_nulls: IndexSet<String>,
        unset_non_nulls: IndexSet<String>,
        set_constraints: IndexSet<TypedConstraint>,
        unset_constraints: IndexSet<TypedConstraint>,
    ) -> Box<dyn ComponentConfig> {
        Box::new(Constraints::new(
            set_non_nulls,
            unset_non_nulls,
            set_constraints,
            unset_constraints,
        ))
    }
}

impl ComponentConfig for Constraints {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    /// Uses normalization for proper constraint comparison, matching the Python implementation.
    ///
    /// Reference: https://raw.githubusercontent.com/databricks/dbt-databricks/refs/tags/v1.10.9/dbt/adapters/databricks/relation_configs/constraints.py
    fn diff_from(
        &self,
        current_state: Option<&dyn ComponentConfig>,
    ) -> Option<Box<dyn ComponentConfig>> {
        // If the config was just introduced, we want to apply the entirety of `self`
        let Some(current_state) = current_state else {
            return Some(Box::new(self.clone()));
        };

        // The config is not if type `Constraints`, so we can't diff
        let current_state = current_state.as_any().downcast_ref::<Self>()?;

        // Normalize constraints for comparison (like Python implementation)
        let next_set_constraints_normalized: IndexSet<_> = self
            .set_constraints
            .iter()
            .map(Self::normalize_constraint)
            .collect();

        let prev_set_constraints_normalized: IndexSet<_> = current_state
            .set_constraints
            .iter()
            .map(Self::normalize_constraint)
            .collect();

        // Find constraints that need to be unset (exist in other but not in self)
        let constraints_to_unset =
            &prev_set_constraints_normalized - &next_set_constraints_normalized;

        // Find non-nulls that need to be unset (exist in other but not in self)
        let non_nulls_to_unset = &current_state.set_non_nulls - &self.set_non_nulls;

        // Find constraints that need to be set (exist in self but not in other)
        let set_constraints = &next_set_constraints_normalized - &prev_set_constraints_normalized;

        // Find non-nulls that need to be set (exist in self but not in other)
        let set_non_nulls = &self.set_non_nulls - &current_state.set_non_nulls;

        if !set_constraints.is_empty()
            || !set_non_nulls.is_empty()
            || !constraints_to_unset.is_empty()
            || !non_nulls_to_unset.is_empty()
        {
            Some(Box::new(Self::new(
                set_non_nulls,
                non_nulls_to_unset,
                set_constraints,
                constraints_to_unset,
            )))
        } else {
            None
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn to_jinja(&self) -> Value {
        Value::from_object(self.clone())
    }
}

/// Expose `Constraints` fields to Jinja as a proper Object so each `TypedConstraint` in the
/// sets is wrapped via `Value::from_object` rather than serde-serialized with external tagging.
impl Object for Constraints {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str() {
            Some("set_constraints") => Some(Value::from_iter(
                self.set_constraints
                    .iter()
                    .map(|c| Value::from_object(c.clone())),
            )),
            Some("unset_constraints") => Some(Value::from_iter(
                self.unset_constraints
                    .iter()
                    .map(|c| Value::from_object(c.clone())),
            )),
            Some("set_non_nulls") => {
                Some(Value::from_iter(self.set_non_nulls.iter().map(Value::from)))
            }
            Some("unset_non_nulls") => Some(Value::from_iter(
                self.unset_non_nulls.iter().map(Value::from),
            )),
            _ => None,
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&[
            "set_non_nulls",
            "unset_non_nulls",
            "set_constraints",
            "unset_constraints",
        ])
    }
}

#[derive(Debug)]
struct FkData {
    columns: Vec<String>,
    to: String,
    to_columns: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::config_v2::ComponentConfig;
    use crate::relation::databricks::config::test_helpers;
    use crate::relation::databricks::typed_constraint::TypedConstraint;
    use arrow::array::{RecordBatch, StringArray};
    use arrow::csv::ReaderBuilder;
    use arrow_schema::{DataType, Field, Schema};
    use dbt_agate::AgateTable;
    use dbt_schemas::schemas::{
        common::{Constraint, ConstraintType},
        nodes::DbtModel,
        properties::ModelConstraint,
    };
    use dbt_test_primitives::assert_contains;
    use indexmap::{IndexMap, IndexSet};
    use std::io;
    use std::sync::Arc;

    fn create_mock_non_null_constraints_table() -> AgateTable {
        let schema = Schema::new(vec![Field::new("col_name", DataType::Utf8, false)]);
        let col_name = StringArray::from(vec!["id", "name"]);
        let record_batch =
            RecordBatch::try_new(Arc::new(schema), vec![Arc::new(col_name)]).unwrap();
        AgateTable::from_record_batch(Arc::new(record_batch))
    }

    fn create_mock_check_constraints_table() -> AgateTable {
        let schema = Schema::new(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ]);
        let csv = io::Cursor::new(
            r#"key,value
delta.constraints.valid_id,id > 0
delta.constraints.name_length,length(name) > 2
table.comment,This is a table comment"#,
        );
        let mut reader = ReaderBuilder::new(Arc::new(schema))
            .with_header(true)
            .build(csv)
            .unwrap();
        let batch = reader.next().unwrap().unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    fn create_mock_primary_key_constraints_table() -> AgateTable {
        let schema = Schema::new(vec![
            Field::new("constraint_name", DataType::Utf8, false),
            Field::new("column_name", DataType::Utf8, false),
        ]);
        let csv = io::Cursor::new(
            r#"constraint_name,column_name
pk_users,id
pk_composite,org_id
pk_composite,user_id"#,
        );
        let mut reader = ReaderBuilder::new(Arc::new(schema))
            .with_header(true)
            .build(csv)
            .unwrap();
        let batch = reader.next().unwrap().unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    fn create_mock_foreign_key_constraints_table() -> AgateTable {
        let schema = Schema::new(vec![
            Field::new("constraint_name", DataType::Utf8, false),
            Field::new("from_column", DataType::Utf8, false),
            Field::new("to_catalog", DataType::Utf8, false),
            Field::new("to_schema", DataType::Utf8, false),
            Field::new("to_table", DataType::Utf8, false),
            Field::new("to_column", DataType::Utf8, false),
        ]);
        let csv = io::Cursor::new(
            r#"constraint_name,from_column,to_catalog,to_schema,to_table,to_column
fk_user_org,org_id,main,default,organizations,id
fk_composite,parent_id,main,default,parents,id
fk_composite,parent_type,main,default,parents,type
"#,
        );
        let mut reader = ReaderBuilder::new(Arc::new(schema))
            .with_header(true)
            .build(csv)
            .unwrap();
        let batch = reader.next().unwrap().unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    fn create_mock_dbt_model_with_constraints(
        constraints: IndexMap<&str, Vec<Constraint>>,
    ) -> DbtModel {
        let cfg = test_helpers::TestModelConfig {
            columns: constraints
                .into_iter()
                .map(|(name, constraints)| test_helpers::TestModelColumn {
                    name: name.to_string(),
                    constraints,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        test_helpers::create_mock_dbt_model(cfg)
    }

    fn create_mock_unenforced_dbt_model_with_constraints(
        constraints: IndexMap<&str, Vec<Constraint>>,
    ) -> DbtModel {
        let cfg = test_helpers::TestModelConfig {
            columns: constraints
                .into_iter()
                .map(|(name, constraints)| test_helpers::TestModelColumn {
                    name: name.to_string(),
                    constraints,
                    ..Default::default()
                })
                .collect(),
            contract_enforced: Some(false),
            ..Default::default()
        };
        test_helpers::create_mock_dbt_model(cfg)
    }

    #[test]
    fn test_constraints_config_diff_no_changes() {
        let non_nulls = IndexSet::from(["id".to_string()]);
        let constraints = IndexSet::from([TypedConstraint::Check {
            name: Some("positive_id".to_string()),
            expression: "id > 0".to_string(),
            columns: None,
        }]);
        let a = Constraints::new(non_nulls, IndexSet::new(), constraints, IndexSet::new());
        let diff = Constraints::diff_from(&a, Some(&a));

        assert!(diff.is_none());
    }

    #[test]
    fn test_constraints_config_diff_with_changes() {
        let old_non_nulls = IndexSet::from(["id".to_string()]);
        let old_constraints = IndexSet::from([TypedConstraint::Check {
            name: Some("old_check".to_string()),
            expression: "id > 0".to_string(),
            columns: None,
        }]);

        let new_non_nulls = IndexSet::from(["id".to_string(), "name".to_string()]);
        let new_constraints = IndexSet::from([
            TypedConstraint::Check {
                name: Some("positive_id".to_string()),
                expression: "id > 0".to_string(),
                columns: None,
            },
            TypedConstraint::PrimaryKey {
                name: Some("pk_users".to_string()),
                columns: vec!["id".to_string()],
                expression: None,
            },
        ]);

        let old_config = Constraints::new(
            old_non_nulls,
            IndexSet::new(),
            old_constraints,
            IndexSet::new(),
        );
        let new_config = Constraints::new(
            new_non_nulls,
            IndexSet::new(),
            new_constraints,
            IndexSet::new(),
        );

        let diff = Constraints::diff_from(&new_config, Some(&old_config));
        assert!(diff.is_some());

        let diff_config = diff.unwrap();
        let diff_config = diff_config.as_any().downcast_ref::<Constraints>().unwrap();

        assert_contains!(diff_config.set_non_nulls, "name");
        assert_eq!(diff_config.set_constraints.len(), 2);
        assert_eq!(diff_config.unset_constraints.len(), 1);
    }

    #[test]
    fn test_from_remote_state_with_non_null_constraints() {
        let non_null_table = create_mock_non_null_constraints_table();
        let results = IndexMap::from([(
            DatabricksRelationMetadataKey::NonNullConstraints,
            non_null_table,
        )]);
        let config = Constraints::from_remote_state(&results);

        assert_eq!(config.set_non_nulls.len(), 2);
        assert_contains!(config.set_non_nulls, "id");
        assert_contains!(config.set_non_nulls, "name");
        assert!(config.set_constraints.is_empty());
    }

    #[test]
    fn test_from_remote_state_with_check_constraints() {
        let check_table = create_mock_check_constraints_table();

        let results = IndexMap::from([(
            DatabricksRelationMetadataKey::ShowTblProperties,
            check_table,
        )]);
        let config = Constraints::from_remote_state(&results);

        assert_eq!(config.set_constraints.len(), 2);
        assert!(config.set_non_nulls.is_empty());

        let constraints: Vec<_> = config.set_constraints.iter().collect();
        assert!(constraints.iter().any(|c| match c {
            TypedConstraint::Check {
                name, expression, ..
            } => name.as_deref() == Some("valid_id") && expression == "id > 0",
            _ => false,
        }));
        assert!(constraints.iter().any(|c| match c {
            TypedConstraint::Check {
                name, expression, ..
            } => name.as_deref() == Some("name_length") && expression == "length(name) > 2",
            _ => false,
        }));
    }

    #[test]
    fn test_from_remote_state_with_primary_key_constraints() {
        let pk_table = create_mock_primary_key_constraints_table();

        let results = IndexMap::from([(
            DatabricksRelationMetadataKey::PrimaryKeyConstraints,
            pk_table,
        )]);
        let config = Constraints::from_remote_state(&results);

        assert_eq!(config.set_constraints.len(), 2);

        let constraints: Vec<_> = config.set_constraints.iter().collect();
        assert!(constraints.iter().any(|c| match c {
            TypedConstraint::PrimaryKey { name, columns, .. } =>
                name.as_deref() == Some("pk_users") && columns == &vec!["id".to_string()],
            _ => false,
        }));
        assert!(constraints.iter().any(|c| match c {
            TypedConstraint::PrimaryKey { name, columns, .. } =>
                name.as_deref() == Some("pk_composite") && columns.len() == 2,
            _ => false,
        }));
    }

    #[test]
    fn test_from_remote_state_with_foreign_key_constraints() {
        let fk_table = create_mock_foreign_key_constraints_table();

        let results = IndexMap::from([(
            DatabricksRelationMetadataKey::ForeignKeyConstraints,
            fk_table,
        )]);
        let config = Constraints::from_remote_state(&results);

        assert_eq!(config.set_constraints.len(), 2);

        let constraints: Vec<_> = config.set_constraints.iter().collect();
        assert!(constraints.iter().any(|c| match c {
            TypedConstraint::ForeignKey { name, to, .. } =>
                name.as_deref() == Some("fk_user_org")
                    && to.as_deref() == Some("`main`.`default`.`organizations`"),
            _ => false,
        }));
        assert!(constraints.iter().any(|c| match c {
            TypedConstraint::ForeignKey {
                name,
                columns,
                to_columns,
                ..
            } =>
                name.as_deref() == Some("fk_composite")
                    && columns.len() == 2
                    && to_columns.as_ref().map(|tc| tc.len()) == Some(2),
            _ => false,
        }));
    }

    #[test]
    fn test_from_remote_state_all_constraint_types() {
        let non_null_table = create_mock_non_null_constraints_table();
        let check_table = create_mock_check_constraints_table();
        let pk_table = create_mock_primary_key_constraints_table();
        let fk_table = create_mock_foreign_key_constraints_table();

        let results = IndexMap::from([
            (
                DatabricksRelationMetadataKey::NonNullConstraints,
                non_null_table,
            ),
            (
                DatabricksRelationMetadataKey::ShowTblProperties,
                check_table,
            ),
            (
                DatabricksRelationMetadataKey::PrimaryKeyConstraints,
                pk_table,
            ),
            (
                DatabricksRelationMetadataKey::ForeignKeyConstraints,
                fk_table,
            ),
        ]);
        let config = Constraints::from_remote_state(&results);

        assert_eq!(config.set_non_nulls.len(), 2);
        assert_eq!(config.set_constraints.len(), 6); // 2 check + 2 pk + 2 fk
    }

    #[test]
    fn test_from_local_config_with_column_constraints() {
        let constraints = IndexMap::from_iter([
            (
                "id",
                Vec::from([Constraint {
                    type_: ConstraintType::NotNull,
                    ..Default::default()
                }]),
            ),
            (
                "name",
                Vec::from([Constraint {
                    type_: ConstraintType::NotNull,
                    ..Default::default()
                }]),
            ),
        ]);

        let mock_node = create_mock_dbt_model_with_constraints(constraints);
        let config = Constraints::from_local_config(&mock_node);

        assert_eq!(config.set_non_nulls.len(), 2);
        assert_contains!(config.set_non_nulls, "id");
        assert_contains!(config.set_non_nulls, "name");
        // Only not-null constraints from columns (model constraints would also be processed here)
        assert!(config.set_constraints.is_empty());
    }

    #[test]
    fn test_from_local_config_ignores_declared_constraints_when_contract_is_unenforced() {
        let columns = IndexMap::from_iter([(
            "id",
            vec![Constraint {
                type_: ConstraintType::NotNull,
                ..Default::default()
            }],
        )]);
        let mut mock_node = create_mock_unenforced_dbt_model_with_constraints(columns);
        mock_node.__model_attr__.constraints = vec![ModelConstraint {
            type_: ConstraintType::PrimaryKey,
            columns: Some(vec!["id".to_string()]),
            ..Default::default()
        }];

        let config = Constraints::from_local_config(&mock_node);

        assert!(config.set_non_nulls.is_empty());
        assert!(config.set_constraints.is_empty());
    }

    #[test]
    fn test_from_local_config_no_constraints() {
        let columns = IndexMap::new();
        let mock_node = create_mock_dbt_model_with_constraints(columns);
        let config = Constraints::from_local_config(&mock_node);

        assert!(config.set_non_nulls.is_empty());
        assert!(config.set_constraints.is_empty());
    }

    #[test]
    fn test_custom_pk_normalizes_to_primary_key() {
        // A custom constraint like `primary key (\`id\`) rely` must normalize to PrimaryKey
        // because Databricks stores it as a native PK in information_schema.
        let custom_pk = TypedConstraint::Custom {
            name: Some("pk_id".to_string()),
            expression: "primary key (`id`) rely".to_string(),
            columns: None,
        };
        let normalized = Constraints::normalize_constraint(&custom_pk);
        assert!(
            matches!(normalized, TypedConstraint::PrimaryKey { ref name, ref columns, .. }
                if name.as_deref() == Some("pk_id") && columns == &["id"]),
            "Expected PrimaryKey, got {normalized:?}"
        );
    }

    #[test]
    fn test_custom_pk_diff_is_stable_against_remote_pk() {
        // Bug 2: on every incremental run, local Custom("primary key (`id`) rely") should
        // compare equal to remote PrimaryKey{pk_id, [id]} after normalization → no churn.
        let remote = Constraints::new(
            IndexSet::new(),
            IndexSet::new(),
            IndexSet::from([TypedConstraint::PrimaryKey {
                name: Some("pk_id".to_string()),
                columns: vec!["id".to_string()],
                expression: None,
            }]),
            IndexSet::new(),
        );
        let local = Constraints::new(
            IndexSet::new(),
            IndexSet::new(),
            IndexSet::from([TypedConstraint::Custom {
                name: Some("pk_id".to_string()),
                expression: "primary key (`id`) rely".to_string(),
                columns: None,
            }]),
            IndexSet::new(),
        );

        let diff = Constraints::diff_from(&local, Some(&remote));
        assert!(
            diff.is_none(),
            "Custom PK expression should match remote PK after normalization, but got diff: {diff:?}"
        );
    }

    #[test]
    fn test_constraints_to_jinja_exposes_object_attributes() {
        // Bug 1: constraint.type and constraint.name must be accessible in Jinja templates.
        let pk = TypedConstraint::PrimaryKey {
            name: Some("pk_id".to_string()),
            columns: vec!["id".to_string()],
            expression: None,
        };
        let constraints = Constraints::new(
            IndexSet::new(),
            IndexSet::new(),
            IndexSet::new(),
            IndexSet::from([pk]),
        );

        let jinja_val = constraints.to_jinja();
        let unset = jinja_val
            .get_attr("unset_constraints")
            .expect("unset_constraints should be accessible");
        let mut iter = unset
            .try_iter()
            .expect("unset_constraints should be iterable");
        let constraint_val = iter.next().expect("should have one constraint");

        let type_val = constraint_val
            .get_attr("type")
            .expect("type should be accessible");
        assert_eq!(type_val.as_str(), Some("primary_key"));

        let name_val = constraint_val
            .get_attr("name")
            .expect("name should be accessible");
        assert_eq!(name_val.as_str(), Some("pk_id"));

        let render_val = constraint_val
            .get_attr("render")
            .expect("render should be accessible");
        assert!(
            !render_val.is_undefined(),
            "render should be a callable function"
        );
    }

    #[test]
    fn test_constraint_normalization_and_diff() {
        let prev_constraint = TypedConstraint::Check {
            name: Some("test_check".to_string()),
            expression: "value > 0".to_string(),
            columns: Some(vec!["value".to_string()]), // With columns (from dbt definition)
        };
        let next_constraint = TypedConstraint::Check {
            name: Some("test_check".to_string()),
            expression: "value > 0".to_string(),
            columns: None, // No columns (like Databricks stores it)
        };

        let prev = Constraints::new(
            IndexSet::new(),
            IndexSet::new(),
            [prev_constraint].into_iter().collect(),
            IndexSet::new(),
        );
        let next = Constraints::new(
            IndexSet::new(),
            IndexSet::new(),
            [next_constraint].into_iter().collect(),
            IndexSet::new(),
        );

        let diff = Constraints::diff_from(&next, Some(&prev));
        assert!(
            diff.is_none(),
            "Normalized constraints should be considered equal"
        );

        let normalized_expr1 = Constraints::normalize_expression("value > 0");
        let normalized_expr2 = Constraints::normalize_expression("  value > 0  ");
        assert_eq!(
            normalized_expr1, normalized_expr2,
            "Whitespace should be normalized"
        );

        let quoted_expr = Constraints::normalize_expression(r#""user name" IS NOT NULL"#);
        assert!(
            quoted_expr.contains(r#""user name""#),
            "Quoted identifiers should be preserved"
        );
    }

    #[test]
    fn test_as_json_path_produces_no_false_diff_vs_info_schema_path_for_constraints() {
        use crate::metadata::databricks::describe_json::{
            ForeignKeyConstraintRow, PrimaryKeyConstraintRow,
        };
        use crate::metadata::databricks::{
            foreign_key_constraints_to_agate, primary_key_constraints_to_agate,
        };

        let info_schema_results = IndexMap::from([
            (
                DatabricksRelationMetadataKey::PrimaryKeyConstraints,
                create_mock_primary_key_constraints_table(),
            ),
            (
                DatabricksRelationMetadataKey::ForeignKeyConstraints,
                create_mock_foreign_key_constraints_table(),
            ),
        ]);
        let info_schema_config = Constraints::from_remote_state(&info_schema_results);

        let pk_rows = vec![
            PrimaryKeyConstraintRow {
                constraint_name: "pk_users".to_string(),
                column_name: "id".to_string(),
            },
            PrimaryKeyConstraintRow {
                constraint_name: "pk_composite".to_string(),
                column_name: "org_id".to_string(),
            },
            PrimaryKeyConstraintRow {
                constraint_name: "pk_composite".to_string(),
                column_name: "user_id".to_string(),
            },
        ];
        let fk_rows = vec![
            ForeignKeyConstraintRow {
                constraint_name: "fk_user_org".to_string(),
                from_column: "org_id".to_string(),
                to_catalog: "main".to_string(),
                to_schema: "default".to_string(),
                to_table: "organizations".to_string(),
                to_column: "id".to_string(),
            },
            ForeignKeyConstraintRow {
                constraint_name: "fk_composite".to_string(),
                from_column: "parent_id".to_string(),
                to_catalog: "main".to_string(),
                to_schema: "default".to_string(),
                to_table: "parents".to_string(),
                to_column: "id".to_string(),
            },
            ForeignKeyConstraintRow {
                constraint_name: "fk_composite".to_string(),
                from_column: "parent_type".to_string(),
                to_catalog: "main".to_string(),
                to_schema: "default".to_string(),
                to_table: "parents".to_string(),
                to_column: "type".to_string(),
            },
        ];
        let as_json_results = IndexMap::from([
            (
                DatabricksRelationMetadataKey::PrimaryKeyConstraints,
                primary_key_constraints_to_agate(&pk_rows).unwrap(),
            ),
            (
                DatabricksRelationMetadataKey::ForeignKeyConstraints,
                foreign_key_constraints_to_agate(&fk_rows).unwrap(),
            ),
        ]);
        let as_json_config = Constraints::from_remote_state(&as_json_results);

        assert!(
            Constraints::diff_from(&as_json_config, Some(&info_schema_config)).is_none(),
            "AS-JSON path should not produce a diff against the info_schema path for identical constraints"
        );
        assert!(
            Constraints::diff_from(&info_schema_config, Some(&as_json_config)).is_none(),
            "info_schema path should not produce a diff against the AS-JSON path for identical constraints"
        );
    }
}
