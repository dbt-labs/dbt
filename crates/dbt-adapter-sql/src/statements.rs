use dbt_adapter_core::AdapterType;

use crate::tokenizer::{Token, Tokenizer};

pub fn clean_sql(sql: &str, adapter_type: AdapterType) -> String {
    match adapter_type {
        AdapterType::Databricks => {
            let sql = sql.trim();
            sql.strip_suffix(';').unwrap_or(sql).to_string()
        }
        _ => sql.to_string(),
    }
}

fn trim_leading_sql_comments(mut sql: &str) -> &str {
    loop {
        let trimmed = sql.trim_start_matches(char::is_whitespace);
        if let Some(rest) = trimmed.strip_prefix("--") {
            match rest.split_once('\n') {
                Some((_, rest)) => {
                    sql = rest;
                    continue;
                }
                None => return "",
            }
        }
        if let Some(rest) = trimmed.strip_prefix("/*") {
            match rest.split_once("*/") {
                Some((_, rest)) => {
                    sql = rest;
                    continue;
                }
                None => return "",
            }
        }
        return trimmed;
    }
}

const CLICKHOUSE_READ_KEYWORDS: [&str; 10] = [
    "SELECT", "WITH", "SHOW", "DESCRIBE", "DESC", "EXPLAIN", "EXISTS", "CHECK", "WATCH", "KILL",
];

// NOTE: I (@serramatutu) manually validated on 2026-09-08 that statements beginning with each
// of the following keywords don't return any rows themselves.
const BIGQUERY_UPDATE_KEYWORDS: [&str; 14] = [
    "INSERT", "UPDATE", "DELETE", "MERGE", "TRUNCATE", "CREATE", "ALTER", "DROP", "UNDROP",
    "GRANT", "REVOKE", "EXPORT", "LOAD", "ASSERT",
];

/// Detect whether `sql` is an UPDATE statement
///
/// One important thing to note is that this is only based on the first keyword in a query.
/// If that query is a SCRIPT or contains multiple statements, then this could accidentally
/// classify something as an update.
pub fn is_update_statement(sql: &str, adapter_type: AdapterType) -> bool {
    let sql = trim_leading_sql_comments(sql);
    let mut tokenizer = Tokenizer::new(sql);
    let Some(Token::Word(first_keyword)) = tokenizer.next() else {
        return false;
    };

    match adapter_type {
        AdapterType::ClickHouse => !CLICKHOUSE_READ_KEYWORDS
            .iter()
            .any(|keyword| keyword.eq_ignore_ascii_case(first_keyword)),
        AdapterType::Bigquery => BIGQUERY_UPDATE_KEYWORDS
            .iter()
            .any(|keyword| keyword.eq_ignore_ascii_case(first_keyword)),
        AdapterType::Snowflake
        | AdapterType::Databricks
        | AdapterType::Redshift
        | AdapterType::Postgres
        | AdapterType::Salesforce
        | AdapterType::Spark
        | AdapterType::DuckDB
        | AdapterType::Fabric
        | AdapterType::Exasol
        | AdapterType::GizmoSQL
        | AdapterType::Starburst
        | AdapterType::Athena
        | AdapterType::Trino
        | AdapterType::Datafusion
        | AdapterType::Dremio
        | AdapterType::Oracle
        | AdapterType::LakeCompute => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_sql, is_update_statement};
    use dbt_adapter_core::AdapterType;

    #[test]
    fn clean_sql_applies_adapter_specific_normalization() {
        assert_eq!(
            clean_sql(" select 1; ", AdapterType::Databricks),
            "select 1"
        );
        assert_eq!(
            clean_sql("select 1;;", AdapterType::Databricks),
            "select 1;"
        );
        assert_eq!(clean_sql(" select 1; ", AdapterType::DuckDB), " select 1; ");
    }

    #[test]
    fn bigquery_dml_and_ddl_are_updates() {
        for sql in [
            "INSERT INTO `proj`.`ds`.`target` (id) VALUES (1)",
            "/* metadata */\ninsert into `proj`.`ds`.`target` values (1)",
            "UPDATE t SET x = 1 WHERE true",
            "DELETE FROM t WHERE true",
            "MERGE t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET x = 1",
            "TRUNCATE TABLE t",
            "CREATE TABLE t AS SELECT 1 AS x",
            "ALTER TABLE t SET OPTIONS (labels = [('a','b')])",
            "DROP TABLE t",
            "UNDROP TABLE t",
            "GRANT `roles/bigquery.dataViewer` ON SCHEMA ds TO 'user:a@b.com'",
            "REVOKE `roles/bigquery.dataViewer` ON SCHEMA ds FROM 'user:a@b.com'",
            "EXPORT DATA OPTIONS (uri = 'gs://bucket/*.csv') AS SELECT 1",
            "LOAD DATA INTO t FROM FILES (uris = ['gs://bucket/f.csv'])",
            "ASSERT (SELECT COUNT(*) FROM t) > 0",
        ] {
            assert!(
                is_update_statement(sql, AdapterType::Bigquery),
                "expected update statement: {sql}"
            );
        }
    }

    #[test]
    fn bigquery_procedural_statements_are_not_updates() {
        for sql in [
            "DECLARE cutoff DATE DEFAULT CURRENT_DATE()",
            "SET cutoff = CURRENT_DATE()",
            "IF true THEN SELECT 1; END IF",
            "CASE WHEN true THEN SELECT 1; END CASE",
            "LOOP BREAK; END LOOP",
            "WHILE true DO BREAK; END WHILE",
            "REPEAT SET x = 1; UNTIL true END REPEAT",
            "FOR row IN (SELECT 1 AS x) DO BREAK; END FOR",
            "BREAK",
            "LEAVE",
            "CONTINUE",
            "ITERATE",
            "BEGIN SELECT 1; END",
            "END",
            "RETURN",
            "RAISE USING MESSAGE = 'boom'",
            "COMMIT TRANSACTION",
            "ROLLBACK TRANSACTION",
        ] {
            assert!(
                !is_update_statement(sql, AdapterType::Bigquery),
                "expected update statement: {sql}"
            );
        }
    }

    #[test]
    fn bigquery_queries_are_not_updates() {
        for sql in [
            "SELECT 1",
            "/* comment */\nSELECT table_name FROM `proj`.`ds`.INFORMATION_SCHEMA.TABLES",
            "-- comment\nselect 1",
            "WITH cte AS (SELECT 1 AS x) SELECT * FROM cte",
            "FROM `proj`.`ds`.`target` |> SELECT id",
            "(SELECT 1) UNION ALL (SELECT 2)",
            "EXECUTE IMMEDIATE 'SELECT 1'",
            "CALL ds.my_proc()",
        ] {
            assert!(
                !is_update_statement(sql, AdapterType::Bigquery),
                "expected query, not update statement: {sql}"
            );
        }
    }

    #[test]
    fn bigquery_unrecognized_input_is_not_an_update() {
        assert!(!is_update_statement("VACUUM t", AdapterType::Bigquery));
        assert!(!is_update_statement("", AdapterType::Bigquery));
        assert!(!is_update_statement("/* dbt */", AdapterType::Bigquery));
    }

    #[test]
    fn clickhouse_reads_are_not_updates() {
        for sql in [
            "/* dbt */\nSELECT 1",
            "SHOW TABLES",
            "DESC TABLE foo",
            "DESCRIBE TABLE foo",
            "WATCH live_view",
            "KILL QUERY WHERE query_id = 'abc'",
        ] {
            assert!(
                !is_update_statement(sql, AdapterType::ClickHouse),
                "expected read statement: {sql}"
            );
        }
    }

    #[test]
    fn clickhouse_writes_are_updates() {
        for sql in [
            "/* dbt */\nCREATE TABLE foo (id Int32)",
            "-- dbt\nINSERT INTO foo VALUES (1)",
            "ALTER TABLE foo DELETE WHERE id = 1",
            "DROP TABLE foo",
        ] {
            assert!(
                is_update_statement(sql, AdapterType::ClickHouse),
                "expected update statement: {sql}"
            );
        }
    }

    #[test]
    fn other_adapters_never_report_update_statements() {
        for adapter_type in [
            AdapterType::Snowflake,
            AdapterType::Databricks,
            AdapterType::Postgres,
            AdapterType::DuckDB,
        ] {
            assert!(!is_update_statement(
                "CREATE TABLE foo (id int)",
                adapter_type
            ));
            assert!(!is_update_statement(
                "INSERT INTO t VALUES (1)",
                adapter_type
            ));
        }
    }
}
