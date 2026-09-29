use dbt_adapter_core::AdapterType;

use crate::schemas::common::{DbtQuoting, ResolvedQuoting};

pub mod base;

pub static DEFAULT_RESOLVED_QUOTING: ResolvedQuoting = ResolvedQuoting {
    database: true,
    schema: true,
    identifier: true,
};

pub static SNOWFLAKE_RESOLVED_QUOTING: ResolvedQuoting = ResolvedQuoting {
    database: false,
    schema: false,
    identifier: false,
};

pub static DEFAULT_DBT_QUOTING: DbtQuoting = DbtQuoting {
    database: Some(true),
    schema: Some(true),
    identifier: Some(true),
    snowflake_ignore_case: Some(false),
};

pub static SNOWFLAKE_DBT_QUOTING: DbtQuoting = DbtQuoting {
    database: Some(false),
    schema: Some(false),
    identifier: Some(false),
    snowflake_ignore_case: Some(false),
};

/// `lake_compute` leaves identifiers unquoted, like Snowflake but not *because* of
/// Snowflake: the two agree today and are free to diverge, so lake compute gets its own
/// constant rather than aliasing Snowflake's. DuckDB, whose macros lake compute falls back
/// to, quotes -- which is why lake compute cannot simply take the `_` arm either.
pub static LAKE_COMPUTE_RESOLVED_QUOTING: ResolvedQuoting = ResolvedQuoting {
    database: false,
    schema: false,
    identifier: false,
};

/// See [`LAKE_COMPUTE_RESOLVED_QUOTING`].
pub static LAKE_COMPUTE_DBT_QUOTING: DbtQuoting = DbtQuoting {
    database: Some(false),
    schema: Some(false),
    identifier: Some(false),
    snowflake_ignore_case: Some(false),
};

pub static DEFAULT_DATABRICKS_DATABASE: &str = "hive_metastore";

/// The adapter default, before any user-authored `quoting:` block is applied.
///
/// Snowflake and `lake_compute` leave identifiers unquoted; every other adapter quotes.
/// Each has its own constant -- see [`LAKE_COMPUTE_DBT_QUOTING`] for why lake compute does not
/// share Snowflake's.
#[inline]
pub fn default_dbt_quoting_for(adapter_type: AdapterType) -> DbtQuoting {
    match adapter_type {
        AdapterType::Snowflake => SNOWFLAKE_DBT_QUOTING,
        AdapterType::LakeCompute => LAKE_COMPUTE_DBT_QUOTING,
        _ => DEFAULT_DBT_QUOTING,
    }
}

/// See [`default_dbt_quoting_for`].
#[inline]
pub fn default_resolved_quoting_for(adapter_type: AdapterType) -> ResolvedQuoting {
    match adapter_type {
        AdapterType::Snowflake => SNOWFLAKE_RESOLVED_QUOTING,
        AdapterType::LakeCompute => LAKE_COMPUTE_RESOLVED_QUOTING,
        _ => DEFAULT_RESOLVED_QUOTING,
    }
}

#[cfg(test)]
mod quoting_default_tests {
    use super::*;
    use strum::IntoEnumIterator;

    /// `lake_compute` does not quote, and specifically does *not* inherit that from DuckDB,
    /// whose macros it otherwise falls back to. DuckDB quotes, so the `_` arm would
    /// give lake compute the wrong answer.
    #[test]
    fn lake_compute_does_not_quote_and_does_not_follow_duckdb() {
        assert_eq!(
            default_dbt_quoting_for(AdapterType::LakeCompute),
            LAKE_COMPUTE_DBT_QUOTING
        );
        assert_eq!(
            default_resolved_quoting_for(AdapterType::LakeCompute),
            LAKE_COMPUTE_RESOLVED_QUOTING
        );
        assert_ne!(
            default_dbt_quoting_for(AdapterType::LakeCompute),
            default_dbt_quoting_for(AdapterType::DuckDB),
            "lake compute must not take DuckDB's quoting just because it borrows its macros"
        );
    }

    /// Lake compute agrees with Snowflake today but must not be *implemented* as Snowflake:
    /// they are free to diverge, so each owns its constant. This pins the agreement
    /// while keeping the constants distinct.
    #[test]
    fn lake_compute_agrees_with_snowflake_without_sharing_its_constant() {
        assert_eq!(LAKE_COMPUTE_DBT_QUOTING, SNOWFLAKE_DBT_QUOTING);
        assert_eq!(LAKE_COMPUTE_RESOLVED_QUOTING, SNOWFLAKE_RESOLVED_QUOTING);
        assert!(
            !std::ptr::eq(&LAKE_COMPUTE_DBT_QUOTING, &SNOWFLAKE_DBT_QUOTING),
            "lake compute must have its own constant, not an alias of Snowflake's"
        );
    }

    /// Snowflake and lake compute leave identifiers unquoted; everything else quotes.
    #[test]
    fn only_snowflake_and_lake_compute_default_to_unquoted() {
        for adapter_type in AdapterType::iter() {
            let quoting = default_dbt_quoting_for(adapter_type);
            let expected_unquoted = matches!(
                adapter_type,
                AdapterType::Snowflake | AdapterType::LakeCompute
            );
            assert_eq!(
                quoting.identifier,
                Some(!expected_unquoted),
                "unexpected identifier quoting default for {adapter_type:?}"
            );
        }
    }

    /// The two accessors must not disagree — one seeds the config hierarchy, the
    /// other builds relations.
    #[test]
    fn dbt_and_resolved_defaults_agree() {
        for adapter_type in AdapterType::iter() {
            let dbt = default_dbt_quoting_for(adapter_type);
            let resolved = default_resolved_quoting_for(adapter_type);
            assert_eq!(dbt.database, Some(resolved.database), "{adapter_type:?}");
            assert_eq!(dbt.schema, Some(resolved.schema), "{adapter_type:?}");
            assert_eq!(
                dbt.identifier,
                Some(resolved.identifier),
                "{adapter_type:?}"
            );
        }
    }
}
