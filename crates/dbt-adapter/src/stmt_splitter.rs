use std::fmt::Debug;

use dbt_frontend_common::Dialect;
use dbt_sql_utils::{is_empty_or_comment_only, sql_split_statements};

use crate::AdapterType;

/// Trait for SQL statement splitting functionality
pub trait StmtSplitter: Send + Sync + Debug {
    /// Split a SQL string into individual statements
    ///
    /// The implementation should:
    /// - Split the SQL into individual statements based on delimiters
    /// - Handle dialect-specific syntax correctly
    /// - Return subslices of `sql` with the delimiters and the surrounding
    ///   whitespace of the input removed.
    fn split<'i>(&self, sql: &'i str, adapter_type: AdapterType) -> Vec<&'i str>;

    /// Determine if a SQL string is either empty or only contains a comment
    fn is_empty(&self, sql: &str, adapter_type: AdapterType) -> bool;

    /// Drop the trailing statement terminator from a rendered node body so it
    /// can be spliced into a wrapping query.
    ///
    /// `sql` is returned untouched unless it holds exactly one non-empty
    /// statement: multi-statement bodies keep their terminators, and so do
    /// bodies that are entirely empty or comment-only.
    fn strip_trailing_statement_terminator<'i>(
        &self,
        sql: &'i str,
        adapter_type: AdapterType,
    ) -> &'i str {
        // Skip splitting, which dominates the cost of this call.
        if !sql.contains(';') {
            return sql;
        }

        let mut statements = self
            .split(sql, adapter_type)
            .into_iter()
            .filter(|stmt| !self.is_empty(stmt, adapter_type));

        match (statements.next(), statements.next()) {
            // Rewrite only when a terminator was actually dropped, so a body
            // without one keeps its original whitespace byte for byte.
            (Some(only), None) if only != sql.trim() => only,
            _ => sql,
        }
    }
}

#[derive(Debug)]
pub struct DefaultStmtSplitter;

impl StmtSplitter for DefaultStmtSplitter {
    fn split<'i>(&self, sql: &'i str, adapter_type: AdapterType) -> Vec<&'i str> {
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
            // ClickHouse string literals use backslash escapes (\', \\, \xNN),
            // which the Trino fallback lexer cannot tokenize — everything from
            // the escape on is then passed through unsplit. The Databricks
            // (Hive-style) lexer shares ClickHouse's string and backtick lexis.
            // Local to splitting on purpose: dialect_of() also feeds SQL
            // analysis, where ClickHouse stays unsupported.
            ClickHouse => Dialect::Databricks,
            Trino => Dialect::Trino,
            _ => Dialect::Trino,
        };
        sql_split_statements(sql, Some(dialect))
            .into_iter()
            .collect()
    }

    fn is_empty(&self, sql: &str, adapter_type: AdapterType) -> bool {
        use AdapterType::*;

        let by_dialect = |dialect: Dialect| is_empty_or_comment_only(sql, dialect);
        match adapter_type {
            Snowflake => by_dialect(Dialect::Snowflake),
            Bigquery => by_dialect(Dialect::Bigquery),
            Databricks | Spark => by_dialect(Dialect::Databricks),
            Redshift => by_dialect(Dialect::Redshift),
            // ClickHouse routes to Databricks for the same reason as split()
            ClickHouse => by_dialect(Dialect::Databricks),
            // fallback to the Trino lexer for unsupported lexer dialects
            _ => by_dialect(Dialect::Trino),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The splitter is dialect-aware only insofar as the tokenizer is. For
    // most cases the result is identical across dialects, so we run the
    // common cases against this representative set (one variant per
    // sqlparser dialect we map to, plus Trino which routes to Generic).
    const REPRESENTATIVE_DIALECTS: &[AdapterType] = &[
        AdapterType::Snowflake,
        AdapterType::Bigquery,
        AdapterType::Redshift,
        AdapterType::Databricks,
        AdapterType::Postgres,
        AdapterType::DuckDB,
        AdapterType::Spark,
        AdapterType::Fabric,
        AdapterType::ClickHouse,
        AdapterType::Trino,
    ];

    fn split(sql: &str, adapter_type: AdapterType) -> Vec<&str> {
        DefaultStmtSplitter.split(sql, adapter_type)
    }

    fn is_empty(sql: &str, adapter_type: AdapterType) -> bool {
        DefaultStmtSplitter.is_empty(sql, adapter_type)
    }

    fn strip(sql: &str, adapter_type: AdapterType) -> &str {
        DefaultStmtSplitter.strip_trailing_statement_terminator(sql, adapter_type)
    }

    // ---- split: ported from dbt_sql_utils::splitter::tests ----

    #[test]
    fn test_split_basic() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(split("", *d), Vec::<String>::new());
            assert_eq!(
                split("SELECT 1; SELECT 2; SELECT 3;", *d),
                vec!["SELECT 1", " SELECT 2", " SELECT 3"]
            );
        }
    }

    #[test]
    fn test_split_empty_statements_not_filtered() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(split(";;;", *d), vec!["", "", ""]);
        }
    }

    #[test]
    fn test_split_comments_not_filtered() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(
                split("select 1; /* end comment */", *d),
                vec!["select 1", " /* end comment */"]
            );
            assert_eq!(
                split("select 1; -- line comment", *d),
                vec!["select 1", " -- line comment"]
            );
        }
    }

    #[test]
    fn test_split_statement_with_embedded_comments() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(
                split("/* before */ select 1 /* after */", *d),
                vec!["/* before */ select 1 /* after */"]
            );
        }
    }

    // ---- split: behavior unique to a real lexer (vs NaiveStmtSplitter) ----

    #[test]
    fn test_split_semicolon_in_string_literal() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(
                split("select 'a;b'; select 2", *d),
                vec!["select 'a;b'", " select 2"]
            );
        }
    }

    #[test]
    fn test_split_semicolon_in_block_comment() {
        // Mirrors the existing SdfStmtSplitter test.
        assert_eq!(
            split(
                "select 1; /* comment with ; */; select 2",
                AdapterType::Snowflake
            ),
            vec!["select 1", " /* comment with ; */", " select 2"]
        );
    }

    #[test]
    fn test_split_semicolon_in_line_comment() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(
                split("select 1 -- trailing ; comment\n; select 2", *d),
                vec!["select 1 -- trailing ; comment\n", " select 2"]
            );
        }
    }

    /// Regression for https://github.com/dbt-labs/dbt-fusion/issues/1031:
    /// Redshift `persist_docs` generates multiple `COMMENT ON COLUMN` statements
    /// using dollar-quoted strings. Embedded `;` inside `$tag$...$tag$` must
    /// not split a statement.
    #[test]
    fn test_split_redshift_dollar_quoted_strings() {
        let sql = r#"
    comment on column "ci"."fusion_tests_schema"."test_model".id is $dbt_comment_literal_block$The unique identifier$dbt_comment_literal_block$;
    comment on column "ci"."fusion_tests_schema"."test_model".name is $dbt_comment_literal_block$The person's name$dbt_comment_literal_block$;
    comment on column "ci"."fusion_tests_schema"."test_model".age is $dbt_comment_literal_block$The person's age$dbt_comment_literal_block$;
    comment on column "ci"."fusion_tests_schema"."test_model".department is $dbt_comment_literal_block$The person's department$dbt_comment_literal_block$;
  "#;

        let statements = split(sql, AdapterType::Redshift);

        assert_eq!(
            statements.len(),
            4,
            "Expected 4 COMMENT ON COLUMN statements, got {}: {:?}",
            statements.len(),
            statements
        );
        for stmt in &statements {
            assert!(
                stmt.trim().to_lowercase().starts_with("comment on column"),
                "Expected COMMENT ON COLUMN statement, got: {stmt}"
            );
        }
    }

    #[test]
    fn test_split_clickhouse_backslash_escaped_strings() {
        // the Trino fallback lexer cannot split these correctly
        assert_eq!(
            split(
                r#"select 'don\'t; drop'; select '\x3F', 'a\\'; select 2"#,
                AdapterType::ClickHouse
            ),
            vec![
                r#"select 'don\'t; drop'"#,
                r#" select '\x3F', 'a\\'"#,
                " select 2"
            ]
        );
        assert!(!is_empty(r#"select 'don\'t'"#, AdapterType::ClickHouse));
    }

    #[test]
    fn test_split_unpaired_token_retains_partial_trailing() {
        let result = split("select 1; select 'unterminated", AdapterType::Snowflake);
        assert_eq!(result, vec!["select 1", " select 'unterminated"]);

        let result = split("select /* unterminated; select 1", AdapterType::Snowflake);
        assert_eq!(result, vec!["select /* unterminated; select 1"]);
    }

    // ---- is_empty: ported from is_empty_or_comment_only ----

    #[test]
    fn test_is_empty_comment_or_whitespace_only() {
        for d in REPRESENTATIVE_DIALECTS {
            assert!(is_empty("", *d));
            assert!(is_empty("   ", *d));
            assert!(is_empty("/* comment */", *d));
            assert!(is_empty("-- line comment", *d));
            assert!(is_empty("  /* comment */  ", *d));
            assert!(is_empty("  -- comment  ", *d));
            assert!(is_empty("/* comment */ -- line comment", *d));
            assert!(is_empty("/* multi\nline\ncomment */", *d));
        }
    }

    #[test]
    fn test_is_empty_with_sql_content() {
        for d in REPRESENTATIVE_DIALECTS {
            assert!(!is_empty("select 1", *d));
            assert!(!is_empty("select /* comment */ 1", *d));
            assert!(!is_empty("select 1 -- comment", *d));
            assert!(!is_empty("/* comment */ select 1", *d));
            assert!(!is_empty("/* before */ select 1 /* after */", *d));
            assert!(!is_empty("-- comment\nselect 1", *d));
            assert!(!is_empty("select 1; select 2", *d));
            assert!(!is_empty("/* comment */\nselect 1\n-- trailing", *d));
        }
    }

    // ---- strip_trailing_statement_terminator ----

    #[test]
    fn test_strip_terminator() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(strip("select 1;", *d), "select 1");
            assert_eq!(strip("select 1;;", *d), "select 1");
            assert_eq!(strip("select 1 ;  ", *d), "select 1 ");
            assert_eq!(strip("select 1;\n", *d), "select 1");
        }
    }

    #[test]
    fn test_strip_terminator_followed_by_comments() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(strip("select 1; -- trailing", *d), "select 1");
            assert_eq!(strip("select 1;\n-- trailing\n", *d), "select 1");
            assert_eq!(strip("select 1; /* trailing */", *d), "select 1");
            // A leading empty statement is dropped along with the terminator.
            assert_eq!(strip("; select 1", *d), " select 1");
        }
    }

    #[test]
    fn test_strip_terminator_keeps_body_comments() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(strip("-- header\nselect 1;", *d), "-- header\nselect 1");
            assert_eq!(
                strip("/* before */ select 1 /* after */;", *d),
                "/* before */ select 1 /* after */"
            );
        }
    }

    #[test]
    fn test_strip_terminator_absent_is_byte_identical() {
        for d in REPRESENTATIVE_DIALECTS {
            // Whitespace is preserved verbatim when there is nothing to drop.
            assert_eq!(strip("\n  select 1\n", *d), "\n  select 1\n");
            assert_eq!(strip("select 1", *d), "select 1");
            assert_eq!(strip("", *d), "");
            // A `;` that is not a statement terminator must not trigger a rewrite.
            assert_eq!(strip("select 'a;b'\n", *d), "select 'a;b'\n");
            assert_eq!(strip("select 1 -- a ; b\n", *d), "select 1 -- a ; b\n");
        }
    }

    #[test]
    fn test_strip_terminator_leaves_multi_statement_bodies() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(strip("select 1; select 2;", *d), "select 1; select 2;");
            assert_eq!(strip("select 1; select 2", *d), "select 1; select 2");
        }
    }

    #[test]
    fn test_strip_terminator_leaves_bodies_without_statements() {
        for d in REPRESENTATIVE_DIALECTS {
            assert_eq!(strip(";", *d), ";");
            assert_eq!(strip(";;;", *d), ";;;");
            assert_eq!(strip("-- just a comment;", *d), "-- just a comment;");
        }
    }
}
