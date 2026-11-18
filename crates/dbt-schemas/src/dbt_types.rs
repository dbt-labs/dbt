use std::fmt;

use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, err, fs_err};
use serde::{Deserialize, Serialize};

/// Enum representing different types of relations.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    /// An enum for table relations.
    Table,
    /// An enum for view relations.
    View,
    /// An enum for CTE relations.
    CTE,
    /// An enum for materialized view relations.
    MaterializedView,
    /// An enum for ephemeral relations.
    Ephemeral,
    /// An enum for any relation that dbt is aware of.
    // Note (copied from dbt-adapters): this is a "catch all" that is better than
    // `None` == external to anything dbt is aware of
    External,
    /// An enum for pointer table
    PointerTable,
    /// An enum for a dynamic table (snowflake only)
    DynamicTable,
    /// An enum for a delta table type that supports streaming or incremental data processing (databricks only)
    StreamingTable,
    /// An enum for a Databricks Unity Catalog Metric View
    MetricView,
    /// An enum for a warehouse function
    Function,
    /// An enum for an interactive table (snowflake only)
    InteractiveTable,
    /// An enum for a ClickHouse dictionary
    Dictionary,
}

impl RelationType {
    /// Convert a warehouse-reported type string for [AdapterType] to a [RelationType].
    /// Unknown types return `NotSupported`.
    pub fn from_adapter_type(adapter_type: AdapterType, type_string: &str) -> FsResult<Self> {
        match adapter_type {
            // https://cloud.google.com/bigquery/docs/information-schema-tables
            // Alternatively, if querying from the Google API:
            // https://docs.cloud.google.com/bigquery/docs/reference/rest/v2/tables
            AdapterType::Bigquery => match type_string.to_uppercase().as_str() {
                "BASE TABLE" | "CLONE" | "SNAPSHOT" | "TABLE" => Ok(RelationType::Table),
                "VIEW" => Ok(RelationType::View),
                "MATERIALIZED VIEW" | "MATERIALIZED_VIEW" => Ok(RelationType::MaterializedView),
                "EXTERNAL" => Ok(RelationType::External),
                "FUNCTION" | "AGGREGATE FUNCTION" | "TABLE FUNCTION" => Ok(RelationType::Function),
                _ => err!(ErrorCode::NotSupported, "unknown table type: {type_string}"),
            },
            // https://docs.databricks.com/aws/en/sql/language-manual/information-schema/tables#table-types
            AdapterType::Databricks => match type_string.to_uppercase().as_str() {
                "TABLE" => Ok(RelationType::Table),
                "VIEW" => Ok(RelationType::View),
                "MATERIALIZED_VIEW" => Ok(RelationType::MaterializedView),
                "EXTERNAL" | "EXTERNAL_SHALLOW_CLONE" | "FOREIGN" => Ok(RelationType::External),
                "STREAMING_TABLE" => Ok(RelationType::StreamingTable),
                "METRIC_VIEW" => Ok(RelationType::MetricView),
                "MANAGED" | "MANAGED_SHALLOW_CLONE" => Ok(RelationType::Table),
                _ => err!(ErrorCode::NotSupported, "unknown table type: {type_string}"),
            },
            AdapterType::Spark => match type_string.to_uppercase().as_str() {
                // These are the only table types Apache Spark's catalog reports
                // (`CatalogTableType`) via `DESCRIBE TABLE EXTENDED`, identical over
                // Thrift/Livy/Spark Connect: MANAGED, EXTERNAL, VIEW (released 3.5/4.0)
                // https://github.com/apache/spark/blob/f0bb2e6a47d0ebda424ffd633fcea8644a597954/sql/catalyst/src/main/scala/org/apache/spark/sql/catalyst/catalog/interface.scala#L1039
                "MANAGED" | "EXTERNAL" => Ok(RelationType::Table),
                "VIEW" => Ok(RelationType::View),
                _ => err!(ErrorCode::NotSupported, "unknown table type: {type_string}"),
            },
            _ => RelationType::try_from(type_string)
                .map_err(|_| fs_err!(ErrorCode::NotSupported, "unknown table type: {type_string}")),
        }
    }
}

// Implement Display so that we can easily get a string representation.
impl fmt::Display for RelationType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            RelationType::Table => "table",
            RelationType::CTE => "cte",
            RelationType::View => "view",
            RelationType::External => "external",
            RelationType::MaterializedView => "materialized_view",
            RelationType::Ephemeral => "ephemeral",
            RelationType::PointerTable => "pointer_table",
            RelationType::DynamicTable => "dynamic_table",
            RelationType::StreamingTable => "streaming_table",
            RelationType::MetricView => "metric_view",
            RelationType::Function => "function",
            RelationType::InteractiveTable => "interactive_table",
            RelationType::Dictionary => "dictionary",
        };
        write!(f, "{s}")
    }
}

impl TryFrom<&str> for RelationType {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "table" => Ok(RelationType::Table),
            "view" => Ok(RelationType::View),
            "cte" => Ok(RelationType::CTE),
            "materialized_view" => Ok(RelationType::MaterializedView),
            "ephemeral" => Ok(RelationType::Ephemeral),
            "external" => Ok(RelationType::External),
            "pointer_table" => Ok(RelationType::PointerTable),
            "dynamic_table" => Ok(RelationType::DynamicTable),
            "streaming_table" => Ok(RelationType::StreamingTable),
            "metric_view" => Ok(RelationType::MetricView),
            "function" => Ok(RelationType::Function),
            "interactive_table" => Ok(RelationType::InteractiveTable),
            "dictionary" => Ok(RelationType::Dictionary),
            _ => Err(format!("Invalid relation type: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn dictionary_relation_type_round_trips() {
        let rt = RelationType::try_from("dictionary").unwrap();
        assert_eq!(rt, RelationType::Dictionary);
        assert_eq!(rt.to_string(), "dictionary");
    }

    #[test]
    fn unknown_relation_type_string_errors() {
        assert!(RelationType::try_from("bogus").is_err());
    }

    #[test]
    fn spark_real_types_resolve_to_table_or_view() {
        // Apache Spark's catalog table types (released 3.5/4.0): MANAGED, EXTERNAL,
        // VIEW. MANAGED/EXTERNAL are tables (Spark 4 reports managed Hive tables as
        // EXTERNAL via TRANSLATED_TO_EXTERNAL); VIEW is a view. All render to valid
        // `drop table` / `drop view` (the bug was EXTERNAL -> `drop external`).
        for t in ["EXTERNAL", "external", "MANAGED"] {
            let rt = RelationType::from_adapter_type(AdapterType::Spark, t).unwrap();
            assert_eq!(rt, RelationType::Table, "Spark {t} should resolve to Table");
            assert_eq!(rt.to_string(), "table");
        }

        let rt = RelationType::from_adapter_type(AdapterType::Spark, "VIEW").unwrap();
        assert_eq!(rt, RelationType::View, "Spark VIEW should resolve to View");
        assert_eq!(rt.to_string(), "view");
    }

    #[test]
    fn spark_rejects_non_spark_relation_types() {
        // Databricks/Unity-Catalog-only types (FOREIGN, STREAMING_TABLE,
        // *_SHALLOW_CLONE) cannot come from an Apache Spark catalog, so the Spark
        // arm rejects them rather than silently coercing.
        let err = RelationType::from_adapter_type(AdapterType::Spark, "FOREIGN").unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported);
        assert!(err.to_string().contains("unknown table type"));
    }

    #[test]
    fn databricks_external_still_resolves_to_external() {
        // Databricks behavior is unchanged: EXTERNAL stays External, FOREIGN stays
        // External, MANAGED is a table.
        assert_eq!(
            RelationType::from_adapter_type(AdapterType::Databricks, "EXTERNAL").unwrap(),
            RelationType::External
        );
        assert_eq!(
            RelationType::from_adapter_type(AdapterType::Databricks, "FOREIGN").unwrap(),
            RelationType::External
        );
        assert_eq!(
            RelationType::from_adapter_type(AdapterType::Databricks, "MANAGED").unwrap(),
            RelationType::Table
        );
    }

    #[test]
    fn bigquery_table_types_resolve_correctly() {
        let map = HashMap::from([
            ("BASE TABLE", (RelationType::Table, "table")),
            ("CLONE", (RelationType::Table, "table")),
            ("SNAPSHOT", (RelationType::Table, "table")),
            ("TABLE", (RelationType::Table, "table")),
            ("VIEW", (RelationType::View, "view")),
            ("EXTERNAL", (RelationType::External, "external")),
            ("FunCTion", (RelationType::Function, "function")),
            ("TABLE FUNCTION", (RelationType::Function, "function")),
            (
                "materialized view",
                (RelationType::MaterializedView, "materialized_view"),
            ),
            (
                "MATERIALIZED_VIEW",
                (RelationType::MaterializedView, "materialized_view"),
            ),
        ]);

        for (key, (rel_type, name)) in map {
            let rt = RelationType::from_adapter_type(AdapterType::Bigquery, key).unwrap();
            assert_eq!(rt, rel_type);
            assert_eq!(rt.to_string(), name);
        }

        let list = ["PROCEDURE", "foobar"];
        for item in list.iter() {
            let err = RelationType::from_adapter_type(AdapterType::Bigquery, item).unwrap_err();
            assert_eq!(err.code, ErrorCode::NotSupported);
            assert!(err.to_string().contains("unknown table type"));
        }
    }

    #[test]
    fn fallback_adapter_unknown_type_errors_without_panic() {
        let err = RelationType::from_adapter_type(AdapterType::Postgres, "bogus_type").unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported);
    }
}
