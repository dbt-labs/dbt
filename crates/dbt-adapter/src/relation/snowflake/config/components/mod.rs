use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::nodes::SnowflakeAttr;
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};

pub(super) fn snowflake_attr(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<&SnowflakeAttr> {
    relation_config
        .as_any()
        .downcast_ref::<DbtModel>()
        .ok_or_else(|| {
            AdapterError::new(
                dbt_common::AdapterErrorKind::UnexpectedResult,
                "relation config needs to be a model",
            )
        })?
        .__adapter_attr__
        .snowflake_attr
        .as_deref()
        .ok_or_else(|| {
            AdapterError::new(
                dbt_common::AdapterErrorKind::Configuration,
                "relation config needs to be Snowflake model",
            )
        })
}

/// Treats a blank or whitespace-only warehouse string as unset, so an env var resolving to
/// `""` doesn't win a `refresh_warehouse -> snowflake_warehouse` fallback, or slip through
/// as a bogus non-empty `WAREHOUSE =` value.
pub(super) fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

/// Strips a warehouse name's double-quote delimiters, unescaping a doubled `""` into a
/// literal `"`. `SHOW` never echoes delimiters back -- only the resolved name -- so a
/// quoted config value must be reduced to that same delimiter-free form before comparison.
fn resolved_warehouse_name(name: &str) -> std::borrow::Cow<'_, str> {
    if name.len() >= 2 && name.starts_with('"') && name.ends_with('"') {
        std::borrow::Cow::Owned(name[1..name.len() - 1].replace("\"\"", "\""))
    } else {
        std::borrow::Cow::Borrowed(name)
    }
}

/// True when two warehouse identifiers name the same Snowflake object. Unquoted names are
/// case-insensitive; a quoted name's delimiters are stripped first so a config value like
/// `"Init_WH"` compares equal to its own quote-free `SHOW` readback of `Init_WH`.
pub(super) fn warehouse_names_match(a: &str, b: &str) -> bool {
    resolved_warehouse_name(a).eq_ignore_ascii_case(&resolved_warehouse_name(b))
}

#[cfg(test)]
mod warehouse_names_match_tests {
    use super::warehouse_names_match;

    #[test]
    fn unquoted_names_fold_case() {
        assert!(warehouse_names_match("wh", "WH"));
    }

    #[test]
    fn quoted_config_matches_unquoted_readback() {
        assert!(warehouse_names_match("\"Init_WH\"", "Init_WH"));
    }

    #[test]
    fn doubled_quote_escape_is_unescaped() {
        assert!(warehouse_names_match("\"a\"\"b\"", "a\"b"));
    }

    #[test]
    fn genuinely_different_names_do_not_match() {
        assert!(!warehouse_names_match("wh_one", "wh_two"));
    }
}

pub(crate) mod cluster_by;
pub(crate) use cluster_by::ClusterByLoader;
pub(crate) mod copy_grants;
pub(crate) use copy_grants::CopyGrantsLoader;
pub(crate) mod immutable_where;
pub(crate) use immutable_where::ImmutableWhereLoader;
pub(crate) mod initialize;
pub(crate) use initialize::InitializeLoader;
pub(crate) mod interactive_table_initialization_warehouse;
pub(crate) use interactive_table_initialization_warehouse::InteractiveTableInitializationWarehouseLoader;
pub(crate) mod interactive_table_warehouse;
pub(crate) use interactive_table_warehouse::InteractiveTableWarehouseLoader;
pub(crate) mod refresh_mode;
pub(crate) use refresh_mode::RefreshModeLoader;
pub(crate) mod refresh_warehouse;
pub(crate) use refresh_warehouse::RefreshWarehouseLoader;
pub(crate) mod row_access_policy;
pub(crate) use row_access_policy::RowAccessPolicyLoader;
pub(crate) mod scheduler;
pub(crate) use scheduler::SchedulerLoader;
pub(crate) mod snowflake_initialization_warehouse;
pub(crate) use snowflake_initialization_warehouse::SnowflakeInitializationWarehouseLoader;
pub(crate) mod snowflake_warehouse;
pub(crate) use snowflake_warehouse::SnowflakeWarehouseLoader;
pub(crate) mod table_tag;
pub(crate) use table_tag::TableTagLoader;
pub(crate) mod target_lag;
pub(crate) use target_lag::{TargetLagLoader, TargetLagWithoutSchedulerLoader};
pub(crate) mod transient;
pub(crate) use transient::TransientLoader;
