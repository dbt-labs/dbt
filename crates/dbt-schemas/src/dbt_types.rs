use std::fmt;

use dbt_adapter_core::AdapterType;
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
    /// Convert a given type string for a given [AdapterType] to a dbt RelationType
    // TODO: This should return an error instead of panicking.
    pub fn from_adapter_type(adapter_type: AdapterType, type_string: &str) -> Self {
        match adapter_type {
            // https://cloud.google.com/bigquery/docs/information-schema-tables
            // Alternatively, if querying from the Google API:
            // https://docs.cloud.google.com/bigquery/docs/reference/rest/v2/tables
            AdapterType::Bigquery => match type_string.to_uppercase().as_str() {
                "BASE TABLE" | "CLONE" | "SNAPSHOT" | "TABLE" => RelationType::Table,
                "VIEW" => RelationType::View,
                "MATERIALIZED VIEW" | "MATERIALIZED_VIEW" => RelationType::MaterializedView,
                "EXTERNAL" => RelationType::External,
                "FUNCTION" | "AGGREGATE FUNCTION" => RelationType::Function,
                _ => panic!("unknown table type: {type_string}"),
            },
            // https://docs.databricks.com/aws/en/sql/language-manual/information-schema/tables#table-types
            AdapterType::Databricks => match type_string.to_uppercase().as_str() {
                "TABLE" => RelationType::Table,
                "VIEW" => RelationType::View,
                "MATERIALIZED_VIEW" => RelationType::MaterializedView,
                "EXTERNAL" | "EXTERNAL_SHALLOW_CLONE" | "FOREIGN" => RelationType::External,
                "STREAMING_TABLE" => RelationType::StreamingTable,
                "METRIC_VIEW" => RelationType::MetricView,
                "MANAGED" | "MANAGED_SHALLOW_CLONE" => RelationType::Table,
                _ => panic!("unknown table type: {type_string}"),
            },
            AdapterType::Spark => match type_string.to_uppercase().as_str() {
                // These are the only table types Apache Spark's catalog reports
                // (`CatalogTableType`) via `DESCRIBE TABLE EXTENDED`, identical over
                // Thrift/Livy/Spark Connect: MANAGED, EXTERNAL, VIEW (released 3.5/4.0)
                // https://github.com/apache/spark/blob/f0bb2e6a47d0ebda424ffd633fcea8644a597954/sql/catalyst/src/main/scala/org/apache/spark/sql/catalyst/catalog/interface.scala#L1039
                "MANAGED" | "EXTERNAL" => RelationType::Table,
                "VIEW" => RelationType::View,
                _ => panic!("unknown table type: {type_string}"),
            },
            _ => RelationType::from(type_string),
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

impl From<&str> for RelationType {
    fn from(s: &str) -> Self {
        match s {
            "table" => RelationType::Table,
            "view" => RelationType::View,
            "cte" => RelationType::CTE,
            "materialized_view" => RelationType::MaterializedView,
            "ephemeral" => RelationType::Ephemeral,
            "external" => RelationType::External,
            "pointer_table" => RelationType::PointerTable,
            "dynamic_table" => RelationType::DynamicTable,
            "streaming_table" => RelationType::StreamingTable,
            "metric_view" => RelationType::MetricView,
            "function" => RelationType::Function,
            "interactive_table" => RelationType::InteractiveTable,
            "dictionary" => RelationType::Dictionary,
            _ => panic!("Invalid relation type: {s}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn dictionary_relation_type_round_trips() {
        let rt = RelationType::from("dictionary");
        assert_eq!(rt, RelationType::Dictionary);
        assert_eq!(rt.to_string(), "dictionary");
    }

    #[test]
    fn spark_real_types_resolve_to_table_or_view() {
        // Apache Spark's catalog table types (released 3.5/4.0): MANAGED, EXTERNAL,
        // VIEW. MANAGED/EXTERNAL are tables (Spark 4 reports managed Hive tables as
        // EXTERNAL via TRANSLATED_TO_EXTERNAL); VIEW is a view. All render to valid
        // `drop table` / `drop view` (the bug was EXTERNAL -> `drop external`).
        for t in ["EXTERNAL", "external", "MANAGED"] {
            let rt = RelationType::from_adapter_type(AdapterType::Spark, t);
            assert_eq!(rt, RelationType::Table, "Spark {t} should resolve to Table");
            assert_eq!(rt.to_string(), "table");
        }

        let rt = RelationType::from_adapter_type(AdapterType::Spark, "VIEW");
        assert_eq!(rt, RelationType::View, "Spark VIEW should resolve to View");
        assert_eq!(rt.to_string(), "view");
    }

    #[test]
    #[should_panic(expected = "unknown table type")]
    fn spark_rejects_non_spark_relation_types() {
        // Databricks/Unity-Catalog-only types (FOREIGN, STREAMING_TABLE,
        // *_SHALLOW_CLONE) cannot come from an Apache Spark catalog, so the Spark
        // arm rejects them rather than silently coercing.
        RelationType::from_adapter_type(AdapterType::Spark, "FOREIGN");
    }

    #[test]
    fn databricks_external_still_resolves_to_external() {
        // Databricks behavior is unchanged: EXTERNAL stays External, FOREIGN stays
        // External, MANAGED is a table.
        assert_eq!(
            RelationType::from_adapter_type(AdapterType::Databricks, "EXTERNAL"),
            RelationType::External
        );
        assert_eq!(
            RelationType::from_adapter_type(AdapterType::Databricks, "FOREIGN"),
            RelationType::External
        );
        assert_eq!(
            RelationType::from_adapter_type(AdapterType::Databricks, "MANAGED"),
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
            let rt = RelationType::from_adapter_type(AdapterType::Bigquery, key);
            assert_eq!(rt, rel_type);
            assert_eq!(rt.to_string(), name);
        }

        let list = ["PROCEDURE", "foobar"];
        for item in list.iter() {
            let result = std::panic::catch_unwind(|| {
                RelationType::from_adapter_type(AdapterType::Bigquery, item)
            });
            assert!(result.is_err());

            let panic_payload = result.unwrap_err();
            assert!(
                panic_payload
                    .downcast_ref::<String>()
                    .unwrap()
                    .contains("unknown table type")
            );
        }
    }
}
