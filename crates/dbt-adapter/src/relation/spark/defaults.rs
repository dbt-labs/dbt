/// Default Spark database
///
/// Spark relations have no catalog/database, so an absent database resolves to empty. Note,
/// the terms `database` and `schema` are interchangeable in Spark
/// https://spark.apache.org/docs/latest/sql-ref-syntax-aux-show-databases.html
pub const DEFAULT_SPARK_DATABASE: &str = "";
