use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{AdapterError, AdapterErrorKind};
use dbt_schemas::schemas::common::{Constraint, ConstraintSupport, ConstraintType};
use dbt_schemas::schemas::properties::ModelConstraint;

use dbt_adapter_core::AdapterType;

/// Emits `ConstraintNotEnforced`/`ConstraintNotSupported` warnings matching dbt-core's
/// `BaseAdapter.process_parsed_constraint`, gated by the constraint's own
/// `warn_unenforced`/`warn_unsupported` overrides (both default to warning).
///
/// This only emits a warning; it does not affect whether the constraint is rendered — callers
/// decide that independently based on `constraint_support`.
/// https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/adapters/base/impl.py#L1939-L1963
pub fn warn_constraint_support(
    adapter_type: AdapterType,
    constraint_type: ConstraintType,
    constraint_support: ConstraintSupport,
    warn_unsupported: Option<bool>,
    warn_unenforced: Option<bool>,
) {
    let Some(code) = constraint_support_warning(
        constraint_type,
        constraint_support,
        warn_unsupported,
        warn_unenforced,
    ) else {
        return;
    };

    let message = match code {
        ErrorCode::ConstraintNotSupported => format!(
            "The constraint type {} is not supported by {adapter_type}, and will be ignored. Set 'warn_unsupported: false' on this constraint to ignore this warning.",
            constraint_type.as_str()
        ),
        ErrorCode::ConstraintNotEnforced => format!(
            "The constraint type {} is not enforced by {adapter_type}. The constraint will be included in this model's DDL statement, but it will not guarantee anything about the underlying data. Set 'warn_unenforced: false' on this constraint to ignore this warning.",
            constraint_type.as_str()
        ),
        _ => unreachable!("constraint_support_warning only returns constraint-support codes"),
    };
    emit_warn_log_message(code, message);
}

/// Decides which constraint-support warning (if any) applies, per dbt-core's
/// `BaseAdapter.process_parsed_constraint` gating logic. Custom constraints bypass the support
/// check entirely in dbt-core, and each warning is independently gated by its own
/// `warn_unsupported`/`warn_unenforced` override (both default to warning).
fn constraint_support_warning(
    constraint_type: ConstraintType,
    constraint_support: ConstraintSupport,
    warn_unsupported: Option<bool>,
    warn_unenforced: Option<bool>,
) -> Option<ErrorCode> {
    if constraint_type == ConstraintType::Custom {
        return None;
    }

    match constraint_support {
        ConstraintSupport::NotSupported if warn_unsupported.unwrap_or(true) => {
            Some(ErrorCode::ConstraintNotSupported)
        }
        ConstraintSupport::NotEnforced if warn_unenforced.unwrap_or(true) => {
            Some(ErrorCode::ConstraintNotEnforced)
        }
        _ => None,
    }
}

/// Render the given constraint as DDL text. Should be overridden by adapters which need custom constraint
/// default: https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/adapters/base/impl.py#L1849-L1850
/// bigquery override: https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L958-L959
pub fn render_model_constraint(
    adapter_type: AdapterType,
    constraint: ModelConstraint,
) -> Result<Option<String>, AdapterError> {
    // dbt-clickhouse impl.py: only named CHECK constraints render; an unnamed
    // CHECK errors, everything else is dropped.
    if adapter_type == AdapterType::ClickHouse {
        return match (constraint.type_, constraint.expression, constraint.name) {
            (ConstraintType::Check, Some(expr), Some(name)) => {
                Ok(Some(format!("CONSTRAINT {name} CHECK ({expr})")))
            }
            (ConstraintType::Check, Some(_), None) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "CHECK Constraint 'name' is required",
            )),
            _ => Ok(None),
        };
    }

    let constraint_prefix = if let Some(name) = constraint.name {
        format!("constraint {name} ")
    } else {
        String::new()
    };

    let column_list = constraint.columns.unwrap_or_default().join(", ");
    let rendered = match constraint.type_ {
        ConstraintType::Check => constraint
            .expression
            .map(|expr| format!("{constraint_prefix}check ({expr})")),
        ConstraintType::Unique => {
            let expr = constraint
                .expression
                .map_or(String::new(), |e| format!(" {e}"));
            Some(format!("{constraint_prefix}unique{expr} ({column_list})"))
        }
        ConstraintType::PrimaryKey => {
            let expr = constraint
                .expression
                .map_or(String::new(), |e| format!(" {e}"));
            Some(format!(
                "{constraint_prefix}primary key{expr} ({column_list})"
            ))
        }
        ConstraintType::ForeignKey => match (constraint.to, constraint.to_columns) {
            (Some(to), Some(to_columns)) if !to_columns.is_empty() => Some(format!(
                "{}foreign key ({}) references {} ({})",
                constraint_prefix,
                column_list,
                to,
                to_columns.join(", ")
            )),
            _ => constraint.expression.map(|expr| {
                format!("{constraint_prefix}foreign key ({column_list}) references {expr}")
            }),
        },
        ConstraintType::Custom => constraint
            .expression
            .map(|expr| format!("{constraint_prefix}{expr}")),
        ConstraintType::NotNull => None,
    };

    Ok(rendered.and_then(|rendered| match adapter_type {
        AdapterType::Bigquery => match constraint.type_ {
            ConstraintType::PrimaryKey | ConstraintType::ForeignKey => {
                Some(format!("{rendered} not enforced"))
            }
            _ => None,
        },
        _ => Some(rendered),
    }))
}

/// Render the given constraint as DDL text. Should be overridden by adapters which need custom constraint
/// default: https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/adapters/base/impl.py#L1751-L1752
/// bigquery override: https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L958-L959
pub fn render_column_constraint(
    adapter_type: AdapterType,
    constraint: Constraint,
) -> Option<String> {
    let constraint_expression = constraint.expression.unwrap_or_default();

    let rendered = match constraint.type_ {
        ConstraintType::Check if !constraint_expression.is_empty() => {
            Some(format!("check ({constraint_expression})"))
        }
        ConstraintType::NotNull => Some(format!("not null {constraint_expression}")),
        ConstraintType::Unique => Some(format!("unique {constraint_expression}")),
        ConstraintType::PrimaryKey => Some(format!("primary key {constraint_expression}")),
        ConstraintType::ForeignKey => match (constraint.to, constraint.to_columns) {
            (Some(to), Some(to_columns)) if !to_columns.is_empty() => {
                Some(format!("references {} ({})", to, to_columns.join(", ")))
            }
            _ if !constraint_expression.is_empty() => {
                Some(format!("references {constraint_expression}"))
            }
            _ => None,
        },
        ConstraintType::Custom if !constraint_expression.is_empty() => Some(constraint_expression),
        _ => None,
    };

    rendered.and_then(|r| {
        match adapter_type {
            AdapterType::Bigquery => {
                match constraint.type_ {
                    ConstraintType::PrimaryKey | ConstraintType::ForeignKey => {
                        Some(format!("{} not enforced", r.trim()))
                    }
                    // BigQuery enforces NOT NULL (equivalent to mode: REQUIRED on the column).
                    ConstraintType::NotNull => Some(r.trim().to_string()),
                    // BigQuery does not support column-level CHECK or UNIQUE constraints.
                    _ => None,
                }
            }
            _ => Some(r.trim().to_string()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dbt-clickhouse impl.py render_model_constraint: only named CHECK renders;
    /// an unnamed CHECK errors, other types are dropped.
    #[test]
    fn clickhouse_model_constraint_requires_named_check() {
        let named = ModelConstraint {
            type_: ConstraintType::Check,
            expression: Some("id > 100".to_string()),
            name: Some("valid_id".to_string()),
            ..Default::default()
        };
        assert_eq!(
            render_model_constraint(AdapterType::ClickHouse, named).unwrap(),
            Some("CONSTRAINT valid_id CHECK (id > 100)".to_string())
        );

        let unnamed = ModelConstraint {
            type_: ConstraintType::Check,
            expression: Some("id > 100".to_string()),
            ..Default::default()
        };
        let err = render_model_constraint(AdapterType::ClickHouse, unnamed).unwrap_err();
        assert!(
            err.message()
                .contains("CHECK Constraint 'name' is required")
        );

        let foreign_key = ModelConstraint {
            type_: ConstraintType::ForeignKey,
            expression: Some("other_table (id)".to_string()),
            columns: Some(vec!["id".to_string()]),
            ..Default::default()
        };
        assert_eq!(
            render_model_constraint(AdapterType::ClickHouse, foreign_key).unwrap(),
            None
        );
    }

    #[test]
    fn not_supported_warns_by_default() {
        assert_eq!(
            constraint_support_warning(
                ConstraintType::Check,
                ConstraintSupport::NotSupported,
                None,
                None
            ),
            Some(ErrorCode::ConstraintNotSupported)
        );
    }

    #[test]
    fn not_supported_silenced_via_warn_unsupported_false() {
        assert_eq!(
            constraint_support_warning(
                ConstraintType::Check,
                ConstraintSupport::NotSupported,
                Some(false),
                None
            ),
            None
        );
    }

    #[test]
    fn not_enforced_warns_by_default() {
        assert_eq!(
            constraint_support_warning(
                ConstraintType::PrimaryKey,
                ConstraintSupport::NotEnforced,
                None,
                None
            ),
            Some(ErrorCode::ConstraintNotEnforced)
        );
    }

    #[test]
    fn not_enforced_silenced_via_warn_unenforced_false() {
        assert_eq!(
            constraint_support_warning(
                ConstraintType::PrimaryKey,
                ConstraintSupport::NotEnforced,
                None,
                Some(false)
            ),
            None
        );
    }

    #[test]
    fn enforced_never_warns() {
        assert_eq!(
            constraint_support_warning(
                ConstraintType::NotNull,
                ConstraintSupport::Enforced,
                None,
                None
            ),
            None
        );
    }

    #[test]
    fn custom_never_warns_regardless_of_support() {
        for support in [
            ConstraintSupport::Enforced,
            ConstraintSupport::NotEnforced,
            ConstraintSupport::NotSupported,
        ] {
            assert_eq!(
                constraint_support_warning(ConstraintType::Custom, support, None, None),
                None
            );
        }
    }

    #[test]
    fn not_enforced_ignores_warn_unsupported_flag() {
        // warn_unsupported only gates NotSupported; a NotEnforced constraint still warns
        // even if warn_unsupported is explicitly false.
        assert_eq!(
            constraint_support_warning(
                ConstraintType::PrimaryKey,
                ConstraintSupport::NotEnforced,
                Some(false),
                None
            ),
            Some(ErrorCode::ConstraintNotEnforced)
        );
    }
}
