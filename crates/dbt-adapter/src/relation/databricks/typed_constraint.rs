//! Databricks constraint utilities
//!
//! Provides constraint types and parsing utilities for the Databricks adapter.
//! Supports check, primary key, foreign key, and custom constraints with validation and DDL rendering.
//!
//! Reference: https://github.com/databricks/dbt-databricks/blob/24325a3195171d36972804e545b2ccf967ab575d/dbt/adapters/databricks/constraints.py

use dbt_schemas::schemas::{common::ConstraintType, properties::ModelConstraint};
use dbt_yaml::Spanned;
use minijinja::Value;
use minijinja::value::{Enumerator, Object};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Database constraint representation with validation and DDL rendering capabilities
///
/// Supports check constraints with expressions, primary/foreign key constraints with column references,
/// and custom constraints. Each variant includes optional naming and validation logic.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/24325a3195171d36972804e545b2ccf967ab575d/dbt/adapters/databricks/constraints.py#L33-L138
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TypedConstraint {
    Check {
        name: Option<String>,
        expression: String,
        columns: Option<Vec<String>>,
    },
    PrimaryKey {
        name: Option<String>,
        columns: Vec<String>,
        expression: Option<String>,
    },
    ForeignKey {
        name: Option<String>,
        columns: Vec<String>,
        to: Option<String>,
        to_columns: Option<Vec<String>>,
        expression: Option<String>,
    },
    Custom {
        name: Option<String>,
        expression: String,
        columns: Option<Vec<String>>,
    },
}

impl TypedConstraint {
    pub fn constraint_type(&self) -> ConstraintType {
        match self {
            TypedConstraint::Check { .. } => ConstraintType::Check,
            TypedConstraint::PrimaryKey { .. } => ConstraintType::PrimaryKey,
            TypedConstraint::ForeignKey { .. } => ConstraintType::ForeignKey,
            TypedConstraint::Custom { .. } => ConstraintType::Custom,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            TypedConstraint::Check { name, .. } => name.as_deref(),
            TypedConstraint::PrimaryKey { name, .. } => name.as_deref(),
            TypedConstraint::ForeignKey { name, .. } => name.as_deref(),
            TypedConstraint::Custom { name, .. } => name.as_deref(),
        }
    }

    /// Validates constraint configuration and returns errors for invalid states
    ///
    /// Reference: https://github.com/databricks/dbt-databricks/blob/24325a3195171d36972804e545b2ccf967ab575d/dbt/adapters/databricks/constraints.py#L85-L138
    pub fn validate(&self) -> Result<(), String> {
        match self {
            TypedConstraint::Check { expression, .. } => {
                if expression.is_empty() {
                    return Err(
                        "check constraint is missing required field: 'expression'".to_string()
                    );
                }
                Ok(())
            }
            TypedConstraint::PrimaryKey { columns, .. } => {
                if columns.is_empty() {
                    return Err(
                        "primary_key constraint is missing required field: 'columns'".to_string(),
                    );
                }
                Ok(())
            }
            TypedConstraint::ForeignKey {
                columns,
                to,
                to_columns,
                expression,
                ..
            } => {
                if columns.is_empty() {
                    return Err(
                        "foreign_key constraint is missing required field: 'columns'".to_string(),
                    );
                }
                if expression.is_none()
                    && (to.is_none() || to_columns.as_ref().is_none_or(|v| v.is_empty()))
                {
                    return Err("foreign_key constraint is missing required fields: ('to', 'to_columns') or 'expression'".to_string());
                }
                Ok(())
            }
            TypedConstraint::Custom { expression, .. } => {
                if expression.is_empty() {
                    return Err(
                        "custom constraint is missing required field: 'expression'".to_string()
                    );
                }
                Ok(())
            }
        }
    }

    /// Renders constraint as DDL SQL for use in CREATE/ALTER TABLE statements
    ///
    /// Reference: https://github.com/databricks/dbt-databricks/blob/24325a3195171d36972804e545b2ccf967ab575d/dbt/adapters/databricks/constraints.py#L47-L51
    pub fn render(&self) -> String {
        let prefix = self.render_prefix();
        let suffix = self.render_suffix();
        format!("{prefix}{suffix}")
    }

    fn render_prefix(&self) -> String {
        if let Some(name) = self.name() {
            format!("CONSTRAINT {name} ")
        } else {
            String::new()
        }
    }

    fn render_suffix(&self) -> String {
        match self {
            TypedConstraint::Check { expression, .. } => {
                let expr = if expression.starts_with('(') && expression.ends_with(')') {
                    expression.clone()
                } else {
                    format!("({expression})")
                };
                format!("CHECK {expr}")
            }
            TypedConstraint::PrimaryKey {
                columns,
                expression,
                ..
            } => {
                let mut suffix = format!("PRIMARY KEY ({})", columns.join(", "));
                if let Some(expr) = expression {
                    suffix.push_str(&format!(" {expr}"));
                }
                suffix
            }
            TypedConstraint::ForeignKey {
                columns,
                to,
                to_columns,
                expression,
                ..
            } => {
                if let Some(expr) = expression {
                    if expr.trim_start().starts_with('(') {
                        format!("FOREIGN KEY {expr}")
                    } else {
                        format!("FOREIGN KEY ({}) {expr}", columns.join(", "))
                    }
                } else if let (Some(to_table), Some(to_cols)) = (to, to_columns)
                    && !to_cols.is_empty()
                {
                    format!(
                        "FOREIGN KEY ({}) REFERENCES {} ({})",
                        columns.join(", "),
                        to_table,
                        to_cols.join(", ")
                    )
                } else {
                    format!("FOREIGN KEY ({})", columns.join(", "))
                }
            }
            TypedConstraint::Custom { expression, .. } => expression.clone(),
        }
    }
}

/// Expose `TypedConstraint` fields and `render()` to Jinja templates as a proper Object.
///
/// Without this, `Value::from_serialize` uses serde's external tagging
/// (`{"PrimaryKey": {...}}`) which makes `.type` and `.name` return Undefined in Jinja,
/// producing broken SQL like `ALTER TABLE x DROP CONSTRAINT IF EXISTS ;`.
impl Object for TypedConstraint {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str() {
            Some("type") => Some(Value::from(match self.as_ref() {
                TypedConstraint::Check { .. } => "check",
                TypedConstraint::PrimaryKey { .. } => "primary_key",
                TypedConstraint::ForeignKey { .. } => "foreign_key",
                TypedConstraint::Custom { .. } => "custom",
            })),
            Some("name") => Some(Value::from_serialize(self.name())),
            Some("expression") => Some(match self.as_ref() {
                TypedConstraint::Check { expression, .. }
                | TypedConstraint::Custom { expression, .. } => Value::from(expression.as_str()),
                TypedConstraint::PrimaryKey { expression, .. }
                | TypedConstraint::ForeignKey { expression, .. } => {
                    Value::from_serialize(expression)
                }
            }),
            Some("columns") => Some(match self.as_ref() {
                TypedConstraint::PrimaryKey { columns, .. }
                | TypedConstraint::ForeignKey { columns, .. } => {
                    Value::from_iter(columns.iter().map(|c| Value::from(c.as_str())))
                }
                TypedConstraint::Check { columns, .. }
                | TypedConstraint::Custom { columns, .. } => {
                    Value::from_iter(columns.iter().flatten().map(|c| Value::from(c.as_str())))
                }
            }),
            // "render" is exposed as a callable so `constraint.render()` works in templates
            Some("render") => {
                let this = Arc::clone(self);
                Some(Value::from_func_func("render", move |_, _| {
                    Ok(Value::from(TypedConstraint::render(&this)))
                }))
            }
            _ => None,
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        // "render" excluded: it's a callable, not a data attribute to iterate
        Enumerator::Str(&["type", "name", "expression", "columns"])
    }
}

/// Convert a ModelConstraint to a TypedConstraint with validation
///
/// This provides the bridge between raw YAML constraint definitions and our processed constraint representation,
/// similar to how Python's TypedConstraint.from_constraint() works.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/24325a3195171d36972804e545b2ccf967ab575d/dbt/adapters/databricks/constraints.py#L238-L243
impl TryFrom<&ModelConstraint> for TypedConstraint {
    type Error = String;

    fn try_from(constraint: &ModelConstraint) -> Result<Self, Self::Error> {
        let typed_constraint = match constraint.type_ {
            ConstraintType::Check => {
                if let Some(expression) = &constraint.expression {
                    TypedConstraint::Check {
                        name: constraint.name.clone(),
                        expression: expression.clone(),
                        columns: constraint.columns.clone(),
                    }
                } else {
                    return Err("Check constraint missing required field: 'expression'".to_string());
                }
            }
            ConstraintType::PrimaryKey => {
                if let Some(columns) = &constraint.columns {
                    if columns.is_empty() {
                        return Err(
                            "Primary key constraint missing required field: 'columns'".to_string()
                        );
                    }
                    TypedConstraint::PrimaryKey {
                        name: constraint.name.clone(),
                        columns: columns.clone(),
                        expression: constraint.expression.clone(),
                    }
                } else {
                    return Err(
                        "Primary key constraint missing required field: 'columns'".to_string()
                    );
                }
            }
            ConstraintType::ForeignKey => {
                if let Some(columns) = &constraint.columns {
                    if columns.is_empty() {
                        return Err(
                            "Foreign key constraint missing required field: 'columns'".to_string()
                        );
                    }
                    if constraint.expression.is_none()
                        && (constraint.to.is_none()
                            || constraint.to_columns.as_ref().is_none_or(|v| v.is_empty()))
                    {
                        return Err("Foreign key constraint missing required fields: ('to', 'to_columns') or 'expression'".to_string());
                    }
                    TypedConstraint::ForeignKey {
                        name: constraint.name.clone(),
                        columns: columns.clone(),
                        to: constraint.to.as_deref().map(|s| s.to_string()),
                        to_columns: constraint.to_columns.clone(),
                        expression: constraint.expression.clone(),
                    }
                } else {
                    return Err(
                        "Foreign key constraint missing required field: 'columns'".to_string()
                    );
                }
            }
            ConstraintType::Custom => {
                if let Some(expression) = &constraint.expression {
                    TypedConstraint::Custom {
                        name: constraint.name.clone(),
                        expression: expression.clone(),
                        columns: constraint.columns.clone(),
                    }
                } else {
                    return Err(
                        "Custom constraint missing required field: 'expression'".to_string()
                    );
                }
            }
            _ => {
                return Err(format!(
                    "Unsupported model-level constraint type: {:?}",
                    constraint.type_
                ));
            }
        };

        typed_constraint.validate()?;
        Ok(typed_constraint)
    }
}

/// Convert a TypedConstraint back to a ModelConstraint
///
/// This is useful for serialization or when interacting with the base dbt constraint system.
impl From<&TypedConstraint> for ModelConstraint {
    fn from(constraint: &TypedConstraint) -> Self {
        match constraint {
            TypedConstraint::Check {
                name,
                expression,
                columns,
            } => ModelConstraint {
                type_: ConstraintType::Check,
                name: name.clone(),
                expression: Some(expression.clone()),
                columns: columns.clone(),
                to: None,
                to_columns: None,
                warn_unsupported: None,
                warn_unenforced: None,
            },
            TypedConstraint::PrimaryKey {
                name,
                columns,
                expression,
            } => ModelConstraint {
                type_: ConstraintType::PrimaryKey,
                name: name.clone(),
                expression: expression.clone(),
                columns: Some(columns.clone()),
                to: None,
                to_columns: None,
                warn_unsupported: None,
                warn_unenforced: None,
            },
            TypedConstraint::ForeignKey {
                name,
                columns,
                to,
                to_columns,
                expression,
            } => ModelConstraint {
                type_: ConstraintType::ForeignKey,
                name: name.clone(),
                expression: expression.clone(),
                columns: Some(columns.clone()),
                to: to.clone().map(Spanned::new),
                to_columns: to_columns.clone(),
                warn_unsupported: None,
                warn_unenforced: None,
            },
            TypedConstraint::Custom {
                name,
                expression,
                columns,
            } => ModelConstraint {
                type_: ConstraintType::Custom,
                name: name.clone(),
                expression: Some(expression.clone()),
                columns: columns.clone(),
                to: None,
                to_columns: None,
                warn_unsupported: None,
                warn_unenforced: None,
            },
        }
    }
}

/// Extracts constraints from dbt model and column configurations
///
/// Returns a tuple of (not_null_columns, typed_constraints) parsed from both column-level
/// and model-level constraint definitions in the dbt node.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/24325a3195171d36972804e545b2ccf967ab575d/dbt/adapters/databricks/constraints.py#L184-L190
pub fn parse_constraints(
    columns: &Vec<dbt_schemas::schemas::dbt_column::DbtColumnRef>,
    model_constraints: &[ModelConstraint],
) -> Result<(std::collections::BTreeSet<String>, Vec<TypedConstraint>), String> {
    let (not_nulls_from_columns, constraints_from_columns) = parse_column_constraints(columns)?;
    let (not_nulls_from_models, constraints_from_models) =
        parse_model_constraints(model_constraints)?;

    let mut all_not_nulls = not_nulls_from_columns;
    all_not_nulls.extend(not_nulls_from_models);

    let mut all_constraints = constraints_from_columns;
    all_constraints.extend(constraints_from_models);

    Ok((all_not_nulls, all_constraints))
}

/// Processes column-level constraint definitions
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/24325a3195171d36972804e545b2ccf967ab575d/dbt/adapters/databricks/constraints.py#L193-L208
pub fn parse_column_constraints(
    columns: &Vec<dbt_schemas::schemas::dbt_column::DbtColumnRef>,
) -> Result<(std::collections::BTreeSet<String>, Vec<TypedConstraint>), String> {
    let mut column_names = std::collections::BTreeSet::new();
    let mut constraints = Vec::new();

    for column in columns {
        let column_name = &column.name;
        for constraint in &column.constraints {
            if constraint.type_ == ConstraintType::Unique {
                return Err("Unique constraints are not supported on Databricks".to_string());
            }

            if constraint.type_ == ConstraintType::NotNull {
                column_names.insert(column_name.clone());
            } else {
                let quoted_column = if column.quote.unwrap_or(false) {
                    format!("`{column_name}`")
                } else {
                    column_name.clone()
                };

                let typed_constraint = parse_constraint_from_column(constraint, quoted_column)?;
                constraints.push(typed_constraint);
            }
        }
    }

    Ok((column_names, constraints))
}

/// Processes model-level constraint definitions
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/24325a3195171d36972804e545b2ccf967ab575d/dbt/adapters/databricks/constraints.py#L211-L225
pub fn parse_model_constraints(
    model_constraints: &[ModelConstraint],
) -> Result<(std::collections::BTreeSet<String>, Vec<TypedConstraint>), String> {
    let mut column_names = std::collections::BTreeSet::new();
    let mut constraints = Vec::new();

    for constraint in model_constraints {
        if constraint.type_ == ConstraintType::Unique {
            return Err("Unique constraints are not supported on Databricks".to_string());
        }

        if constraint.type_ == ConstraintType::NotNull {
            if let Some(cols) = &constraint.columns {
                if cols.is_empty() {
                    return Err(
                        "not_null constraint on model must have 'columns' defined".to_string()
                    );
                }
                column_names.extend(cols.iter().cloned());
            } else {
                return Err("not_null constraint on model must have 'columns' defined".to_string());
            }
        } else {
            let typed_constraint = TypedConstraint::try_from(constraint)?;
            constraints.push(typed_constraint);
        }
    }

    Ok((column_names, constraints))
}

/// Parse a constraint from column-level definition
fn parse_constraint_from_column(
    constraint: &dbt_schemas::schemas::common::Constraint,
    column_name: String,
) -> Result<TypedConstraint, String> {
    match constraint.type_ {
        ConstraintType::Check => {
            if let Some(expression) = &constraint.expression {
                Ok(TypedConstraint::Check {
                    name: constraint.name.clone(),
                    expression: expression.clone(),
                    columns: Some(vec![column_name]),
                })
            } else {
                Err("Check constraint missing required field: 'expression'".to_string())
            }
        }
        ConstraintType::PrimaryKey => Ok(TypedConstraint::PrimaryKey {
            name: constraint.name.clone(),
            columns: vec![column_name],
            expression: constraint.expression.clone(),
        }),
        ConstraintType::ForeignKey => {
            if constraint.expression.is_none()
                && (constraint.to.is_none()
                    || constraint.to_columns.as_ref().is_none_or(|v| v.is_empty()))
            {
                return Err("Foreign key constraint missing required fields: ('to', 'to_columns') or 'expression'".to_string());
            }
            Ok(TypedConstraint::ForeignKey {
                name: constraint.name.clone(),
                columns: vec![column_name],
                to: constraint.to.as_deref().map(|s| s.to_string()),
                to_columns: constraint.to_columns.clone(),
                expression: constraint.expression.clone(),
            })
        }
        ConstraintType::Custom => {
            if let Some(expression) = &constraint.expression {
                Ok(TypedConstraint::Custom {
                    name: constraint.name.clone(),
                    expression: expression.clone(),
                    columns: Some(vec![column_name]),
                })
            } else {
                Err("Custom constraint missing required field: 'expression'".to_string())
            }
        }
        _ => Err(format!(
            "Unsupported column-level constraint type: {:?}",
            constraint.type_
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_test_primitives::assert_contains;
    use std::sync::Arc;

    #[test]
    fn test_typed_constraint_jinja_attributes() {
        let pk = Arc::new(TypedConstraint::PrimaryKey {
            name: Some("pk_id".to_string()),
            columns: vec!["id".to_string()],
            expression: None,
        });

        let type_val = pk.get_value(&Value::from("type")).unwrap();
        assert_eq!(type_val.as_str(), Some("primary_key"));

        let name_val = pk.get_value(&Value::from("name")).unwrap();
        assert_eq!(name_val.as_str(), Some("pk_id"));

        let custom = Arc::new(TypedConstraint::Custom {
            name: Some("pk_id".to_string()),
            expression: "primary key (`id`) rely".to_string(),
            columns: None,
        });

        let type_val = custom.get_value(&Value::from("type")).unwrap();
        assert_eq!(type_val.as_str(), Some("custom"));

        let name_val = custom.get_value(&Value::from("name")).unwrap();
        assert_eq!(name_val.as_str(), Some("pk_id"));
    }

    #[test]
    fn test_typed_constraint_jinja_render_callable() {
        let pk = Arc::new(TypedConstraint::PrimaryKey {
            name: Some("pk_id".to_string()),
            columns: vec!["id".to_string()],
            expression: None,
        });

        let render_val = pk.get_value(&Value::from("render")).unwrap();
        assert!(!render_val.is_undefined());

        // Verify render works via TypedConstraint::render (not Object::render)
        let rendered = TypedConstraint::render(&pk);
        assert_eq!(rendered, "CONSTRAINT pk_id PRIMARY KEY (id)");
    }

    #[test]
    fn test_typed_constraint_validation() {
        let check = TypedConstraint::Check {
            name: Some("positive_id".to_string()),
            expression: "id > 0".to_string(),
            columns: None,
        };
        assert!(check.validate().is_ok());

        // Empty expression should fail validation
        let invalid_check = TypedConstraint::Check {
            name: Some("empty".to_string()),
            expression: "".to_string(),
            columns: None,
        };
        assert!(invalid_check.validate().is_err());

        let custom = TypedConstraint::Custom {
            name: Some("my_custom".to_string()),
            expression: "some_custom_logic".to_string(),
            columns: None,
        };
        assert!(custom.validate().is_ok());
    }

    #[test]
    fn test_parse_constraints_unique_validation() {
        let columns = vec![
            (Arc::new(dbt_schemas::schemas::dbt_column::DbtColumn {
                constraints: vec![dbt_schemas::schemas::common::Constraint {
                    type_: ConstraintType::Unique,
                    name: None,
                    expression: None,
                    to: None,
                    to_columns: None,
                    warn_unsupported: None,
                    warn_unenforced: None,
                }],
                ..Default::default()
            })),
        ];

        let result = parse_constraints(&columns, &[]);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Unique constraints are not supported on Databricks"
        );
    }

    #[test]
    fn test_parse_constraints_column_quoting() {
        let columns = vec![
            (Arc::new(dbt_schemas::schemas::dbt_column::DbtColumn {
                name: "special column".to_string(),
                quote: Some(true),
                constraints: vec![dbt_schemas::schemas::common::Constraint {
                    type_: ConstraintType::Check,
                    name: Some("check_special".to_string()),
                    expression: Some("`special column` > 0".to_string()),
                    to: None,
                    to_columns: None,
                    warn_unsupported: None,
                    warn_unenforced: None,
                }],
                ..Default::default()
            })),
        ];

        let (not_nulls, constraints) = parse_constraints(&columns, &[]).unwrap();
        assert!(not_nulls.is_empty());
        assert_eq!(constraints.len(), 1);

        if let TypedConstraint::Check {
            columns: Some(cols),
            ..
        } = &constraints[0]
        {
            assert_eq!(cols[0], "`special column`");
        } else {
            panic!("Expected Check constraint with quoted column");
        }
    }

    #[test]
    fn test_typed_constraint_rendering() {
        let check = TypedConstraint::Check {
            name: Some("positive_id".to_string()),
            expression: "id > 0".to_string(),
            columns: None,
        };
        assert_eq!(check.render(), "CONSTRAINT positive_id CHECK (id > 0)");

        let pk = TypedConstraint::PrimaryKey {
            name: Some("pk_users".to_string()),
            columns: vec!["id".to_string()],
            expression: None,
        };
        assert_eq!(pk.render(), "CONSTRAINT pk_users PRIMARY KEY (id)");

        let fk = TypedConstraint::ForeignKey {
            name: Some("fk_user_org".to_string()),
            columns: vec!["org_id".to_string()],
            to: Some("`main`.`default`.`organizations`".to_string()),
            to_columns: Some(vec!["id".to_string()]),
            expression: None,
        };
        assert_eq!(
            fk.render(),
            "CONSTRAINT fk_user_org FOREIGN KEY (org_id) REFERENCES `main`.`default`.`organizations` (id)"
        );

        // Foreign key with expression starting with '(' (expression includes columns)
        let fk_expr_paren = TypedConstraint::ForeignKey {
            name: Some("fk_test".to_string()),
            columns: vec!["col_a".to_string()],
            to: None,
            to_columns: None,
            expression: Some("(col_a) REFERENCES other_table (id)".to_string()),
        };
        assert_eq!(
            fk_expr_paren.render(),
            "CONSTRAINT fk_test FOREIGN KEY (col_a) REFERENCES other_table (id)"
        );

        // Foreign key with non-parenthesized expression
        let fk_expr = TypedConstraint::ForeignKey {
            name: Some("fk_inline".to_string()),
            columns: vec!["col_b".to_string()],
            to: None,
            to_columns: None,
            expression: Some("REFERENCES target_table (pk)".to_string()),
        };
        assert_eq!(
            fk_expr.render(),
            "CONSTRAINT fk_inline FOREIGN KEY (col_b) REFERENCES target_table (pk)"
        );

        let custom = TypedConstraint::Custom {
            name: Some("my_custom".to_string()),
            expression: "VALIDATE(column_a, column_b)".to_string(),
            columns: None,
        };
        assert_eq!(
            custom.render(),
            "CONSTRAINT my_custom VALIDATE(column_a, column_b)"
        );
    }

    #[test]
    fn test_typed_constraint_methods() {
        let check_constraint = TypedConstraint::Check {
            name: Some("positive_id".to_string()),
            expression: "id > 0".to_string(),
            columns: None,
        };
        assert_eq!(check_constraint.constraint_type(), ConstraintType::Check);
        assert_eq!(check_constraint.name(), Some("positive_id"));

        let pk_constraint = TypedConstraint::PrimaryKey {
            name: Some("pk_users".to_string()),
            columns: vec!["id".to_string()],
            expression: None,
        };
        assert_eq!(pk_constraint.constraint_type(), ConstraintType::PrimaryKey);
        assert_eq!(pk_constraint.name(), Some("pk_users"));

        let fk_constraint = TypedConstraint::ForeignKey {
            name: Some("fk_user_org".to_string()),
            columns: vec!["org_id".to_string()],
            to: Some("`main`.`default`.`organizations`".to_string()),
            to_columns: Some(vec!["id".to_string()]),
            expression: None,
        };
        assert_eq!(fk_constraint.constraint_type(), ConstraintType::ForeignKey);
        assert_eq!(fk_constraint.name(), Some("fk_user_org"));
    }

    #[test]
    fn test_from_model_constraint_conversions() {
        let model_constraint = ModelConstraint {
            type_: ConstraintType::Check,
            name: Some("email_check".to_string()),
            expression: Some("email LIKE '%@%'".to_string()),
            columns: None,
            to: None,
            to_columns: None,
            warn_unsupported: None,
            warn_unenforced: None,
        };

        let typed_constraint = TypedConstraint::try_from(&model_constraint).unwrap();
        assert_eq!(typed_constraint.constraint_type(), ConstraintType::Check);
        assert_eq!(typed_constraint.name(), Some("email_check"));

        if let TypedConstraint::Check { expression, .. } = &typed_constraint {
            assert_eq!(expression, "email LIKE '%@%'");
        } else {
            panic!("Expected Check constraint");
        }

        let back_to_model = ModelConstraint::from(&typed_constraint);
        assert_eq!(back_to_model.type_, ConstraintType::Check);
        assert_eq!(back_to_model.name, Some("email_check".to_string()));
        assert_eq!(
            back_to_model.expression,
            Some("email LIKE '%@%'".to_string())
        );
        assert_eq!(back_to_model.columns, None);
    }

    #[test]
    fn test_primary_key_from_conversions() {
        let model_constraint = ModelConstraint {
            type_: ConstraintType::PrimaryKey,
            name: Some("pk_users".to_string()),
            expression: None,
            columns: Some(vec!["id".to_string(), "tenant_id".to_string()]),
            to: None,
            to_columns: None,
            warn_unsupported: None,
            warn_unenforced: None,
        };

        let typed_constraint = TypedConstraint::try_from(&model_constraint).unwrap();
        if let TypedConstraint::PrimaryKey { name, columns, .. } = &typed_constraint {
            assert_eq!(name, &Some("pk_users".to_string()));
            assert_eq!(columns, &vec!["id".to_string(), "tenant_id".to_string()]);
        } else {
            panic!("Expected PrimaryKey constraint");
        }

        let back_to_model = ModelConstraint::from(&typed_constraint);
        assert_eq!(back_to_model.type_, ConstraintType::PrimaryKey);
        assert_eq!(
            back_to_model.columns,
            Some(vec!["id".to_string(), "tenant_id".to_string()])
        );
    }

    #[test]
    fn test_foreign_key_from_conversions() {
        let model_constraint = ModelConstraint {
            type_: ConstraintType::ForeignKey,
            name: Some("fk_user_org".to_string()),
            expression: None,
            columns: Some(vec!["org_id".to_string()]),
            to: Some(Spanned::new("organizations".to_string())),
            to_columns: Some(vec!["id".to_string()]),
            warn_unsupported: None,
            warn_unenforced: None,
        };

        let typed_constraint = TypedConstraint::try_from(&model_constraint).unwrap();
        if let TypedConstraint::ForeignKey {
            name,
            columns,
            to,
            to_columns,
            ..
        } = &typed_constraint
        {
            assert_eq!(name, &Some("fk_user_org".to_string()));
            assert_eq!(columns, &vec!["org_id".to_string()]);
            assert_eq!(to, &Some("organizations".to_string()));
            assert_eq!(to_columns, &Some(vec!["id".to_string()]));
        } else {
            panic!("Expected ForeignKey constraint");
        }
    }

    #[test]
    fn test_invalid_model_constraint_conversion() {
        let invalid_constraint = ModelConstraint {
            type_: ConstraintType::Check,
            name: Some("invalid_check".to_string()),
            expression: None, // Missing required expression
            columns: None,
            to: None,
            to_columns: None,
            warn_unsupported: None,
            warn_unenforced: None,
        };

        let result = TypedConstraint::try_from(&invalid_constraint);
        assert!(result.is_err());
        assert_contains!(
            result.unwrap_err(),
            "Check constraint missing required field: 'expression'"
        );
    }

    #[test]
    fn test_validation_called_during_conversion() {
        // This creates a constraint that passes initial parsing but fails validation
        let constraint_with_empty_expression = ModelConstraint {
            type_: ConstraintType::Check,
            name: Some("empty_check".to_string()),
            expression: Some("".to_string()), // Empty expression should fail validation
            columns: None,
            to: None,
            to_columns: None,
            warn_unsupported: None,
            warn_unenforced: None,
        };

        let result = TypedConstraint::try_from(&constraint_with_empty_expression);
        assert!(result.is_err());
        assert_contains!(
            result.unwrap_err(),
            "check constraint is missing required field: 'expression'"
        );
    }
}
