use dbt_frontend_common::Dialect;

pub mod input_streams;
pub mod splitter;
pub mod tokens;

pub use input_streams::CaseInsensitiveInputStream;
pub use splitter::{
    is_empty_or_comment_only, jinja_sql_find_statement_spans, snowflake_terminal_flow_statement,
    sql_split_statements,
};
pub use tokens::{SqlToken, sql_lex_tokens};

/// List of [Dialect]s that are truly supported by this library.
///
/// Users of `dbt-sql-utils` are advices to explicitly fallback to [Dialect::Trino]
/// for any dialects not in this list before it's explicitly supported.
pub const SUPPORTED_DIALECTS: &[Dialect] = &[
    Dialect::Bigquery,
    Dialect::Databricks,
    Dialect::Redshift,
    Dialect::Snowflake,
    Dialect::Trino,
];
