//! ClickHouse SQL helpers shared by the engine and the metadata adapter.

pub mod bootstrap;
pub mod capabilities;

pub use capabilities::ClickHouseCapabilities;

use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::ident::quote_identifier;

/// dbclient.py `_ensure_database` parity (incl. its clause spacing).
/// Divergence: identifiers are double-quoted via the shared ident helper
/// instead of query.py's backtick quoting — equivalent for ClickHouse.
pub fn exists_database_sql(database: &str) -> String {
    format!(
        "EXISTS DATABASE {}",
        quote_identifier(database, AdapterType::ClickHouse)
    )
}

/// See [`exists_database_sql`].
pub fn create_database_sql(
    database: &str,
    database_engine: Option<&str>,
    cluster: Option<&str>,
) -> String {
    let engine_clause = match database_engine {
        Some(engine) if !engine.is_empty() => format!(" ENGINE {engine} "),
        _ => String::new(),
    };
    let cluster_clause = match cluster {
        Some(cluster) if !cluster.trim().is_empty() => format!(" ON CLUSTER \"{cluster}\" "),
        _ => String::new(),
    };
    format!(
        "CREATE DATABASE IF NOT EXISTS {}{cluster_clause}{engine_clause}",
        quote_identifier(database, AdapterType::ClickHouse)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_database_sql_mirrors_python() {
        assert_eq!(exists_database_sql("my_db"), "EXISTS DATABASE \"my_db\"");
        // '"' doubling in quoted identifiers verified against ClickHouse 26.3
        assert_eq!(
            exists_database_sql("we\"ird"),
            "EXISTS DATABASE \"we\"\"ird\""
        );
        assert_eq!(
            create_database_sql("my_db", None, None),
            "CREATE DATABASE IF NOT EXISTS \"my_db\""
        );
        assert_eq!(
            create_database_sql("my_db", Some(""), Some("  ")),
            "CREATE DATABASE IF NOT EXISTS \"my_db\""
        );
        assert_eq!(
            create_database_sql("my_db", Some("Replicated"), Some("test_shard")),
            "CREATE DATABASE IF NOT EXISTS \"my_db\" ON CLUSTER \"test_shard\"  ENGINE Replicated "
        );
    }
}
