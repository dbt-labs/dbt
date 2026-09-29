use dbt_adapter_core::AdapterType;
use dbt_frontend_common::dialect::Dialect;

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
