use dbt_adapter_core::AdapterType;
use dbt_frontend_common::Dialect;

pub fn dialect_of(adapter_type: AdapterType) -> Option<Dialect> {
    use AdapterType::*;
    let dialect = match adapter_type {
        Postgres => Dialect::Postgresql,
        Snowflake => Dialect::Snowflake,
        Bigquery => Dialect::Bigquery,
        // TODO(serramatutu): switch Spark to Spark dialect once frontend looks good
        Databricks | Spark => Dialect::Databricks,
        Redshift => Dialect::Redshift,
        // Salesforce dialect is unclear, it claims ANSI vaguely
        // https://developer.salesforce.com/docs/data/data-cloud-query-guide/references/data-cloud-query-api-reference/c360a-api-query-v2-call-overview.html
        // falls back to Postgresql at the moment
        Salesforce => Dialect::Postgresql,
        // `LakeCompute` defines no dialect of its own, so it falls back to DuckDB's
        DuckDB | LakeCompute => Dialect::Duckdb,
        Trino => Dialect::Trino,
        _ => return None,
    };
    Some(dialect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    fn adapter_type_to_string_via_dialect(adapter_type: AdapterType) -> String {
        dialect_of(adapter_type)
            .map(|dialect| dialect.to_string())
            .unwrap_or_else(|| adapter_type.to_string())
    }

    #[test]
    fn adapter_type_to_string_via_dialect_matches_to_string() {
        for adapter_type in AdapterType::iter() {
            if adapter_type == AdapterType::Spark {
                // no good dialect mapping for Spark, so we skip the invariant check for it
                continue;
            }
            if matches!(
                adapter_type,
                AdapterType::Postgres | AdapterType::Salesforce | AdapterType::LakeCompute
            ) {
                // Postgres serializes as "postgres" but its dialect as "postgresql";
                // Salesforce maps to Dialect::Postgresql and LakeCompute to
                // Dialect::Duckdb, so both diverge by design.
                continue;
            }
            assert_eq!(
                adapter_type_to_string_via_dialect(adapter_type),
                adapter_type.to_string(),
                "adapter_type_to_string_via_dialect() diverges from \
                 AdapterType::to_string() for {adapter_type:?}",
            );
        }
    }
}
