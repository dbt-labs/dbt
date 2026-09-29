//! Mappings from dbt's internal dialect/adapter enums to `sqlparser` dialects.

use dbt_adapter_core::AdapterType;
use sqlparser::dialect::{
    BigQueryDialect, ClickHouseDialect, DatabricksDialect, Dialect, DuckDbDialect, GenericDialect,
    HiveDialect, MsSqlDialect, PostgreSqlDialect, RedshiftSqlDialect, SnowflakeDialect,
};

static SNOWFLAKE: SnowflakeDialect = SnowflakeDialect {};
static BIGQUERY: BigQueryDialect = BigQueryDialect {};
static DATABRICKS: DatabricksDialect = DatabricksDialect {};
static REDSHIFT: RedshiftSqlDialect = RedshiftSqlDialect {};
static POSTGRES: PostgreSqlDialect = PostgreSqlDialect {};
static DUCKDB: DuckDbDialect = DuckDbDialect {};
static HIVE: HiveDialect = HiveDialect {};
static MSSQL: MsSqlDialect = MsSqlDialect {};
static CLICKHOUSE: ClickHouseDialect = ClickHouseDialect {};
static GENERIC: GenericDialect = GenericDialect {};

/// Maps a dbt [`AdapterType`] to the closest `sqlparser` [`Dialect`].
pub fn sqlparser_dialect_for(adapter_type: AdapterType) -> &'static dyn Dialect {
    use AdapterType::*;
    match adapter_type {
        Snowflake => &SNOWFLAKE,
        Bigquery => &BIGQUERY,
        Databricks => &DATABRICKS,
        Redshift => &REDSHIFT,
        Postgres => &POSTGRES,
        DuckDB => &DUCKDB,
        LakeCompute => &DUCKDB,
        // Spark SQL is closest to Hive / Databricks; HiveDialect is a safe
        // baseline for tokenization (string/comment forms match).
        Spark => &HIVE,
        Fabric => &MSSQL,
        ClickHouse => &CLICKHOUSE,
        // No close sqlparser match — generic SQL tokenizer is permissive enough
        // for statement splitting.
        Trino | Athena | Starburst | Datafusion | Dremio | Oracle | Salesforce | Exasol => &GENERIC,
    }
}
