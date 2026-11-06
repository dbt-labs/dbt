use std::borrow::Cow;
use std::fmt::Debug;

use dbt_frontend_common::Dialect;
use dbt_sql_utils::{is_empty_or_comment_only, sql_split_statements};

use crate::AdapterType;

mod clickhouse;

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
            // Trino fallback has no dollar-quote token. persist_docs uses $tag$.
            Postgres => return split_respecting_dollar_quotes(sql, Dialect::Postgresql),
            Snowflake => Dialect::Snowflake,
            Bigquery => Dialect::Bigquery,
            // TODO(serramatutu): switch Spark to Spark dialect once frontend looks good
            Databricks | Spark => Dialect::Databricks,
            Redshift => Dialect::Redshift,
            // Salesforce dialect is unclear, it claims ANSI vaguely
            // https://developer.salesforce.com/docs/data/data-cloud-query-guide/references/data-cloud-query-api-reference/c360a-api-query-v2-call-overview.html
            // falls back to Postgresql at the moment
            Salesforce => Dialect::Postgresql,
            // LakeCompute has no dialect of its own, so it uses DuckDB's.
            // Trino fallback has no dollar-quote token. Mask `$tag$...$tag$` first.
            DuckDB | LakeCompute => {
                return split_respecting_dollar_quotes(sql, Dialect::Duckdb);
            }
            ClickHouse => return clickhouse::split_statements(sql),
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
            ClickHouse => clickhouse::is_empty_or_comment_only(sql),
            // fallback to the Trino lexer for unsupported lexer dialects
            _ => by_dialect(Dialect::Trino),
        }
    }
}

/// Split `sql`, treating `$tag$...$tag$` as a literal so a `;` or `'` inside
/// it cannot split or merge statements. The masked copy is the same length, so
/// statement slices map back onto `sql`.
fn split_respecting_dollar_quotes<'i>(sql: &'i str, dialect: Dialect) -> Vec<&'i str> {
    let masked = mask_dollar_quoted_blocks(sql);
    if matches!(masked, Cow::Borrowed(_)) {
        return sql_split_statements(sql, Some(dialect));
    }
    let masked = masked.as_ref();
    let base = masked.as_ptr() as usize;
    sql_split_statements(masked, Some(dialect))
        .into_iter()
        .map(|part| {
            let start = (part.as_ptr() as usize) - base;
            &sql[start..start + part.len()]
        })
        .collect()
}

/// Replace each closed dollar quote with a same-length single-quoted literal.
/// Quotes, comments, backtick identifiers, and ordinary string literals are left alone.
fn mask_dollar_quoted_blocks(sql: &str) -> Cow<'_, str> {
    let bytes = sql.as_bytes();
    let mut masks = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => i = skip_quoted(bytes, i, bytes[i]),
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                if i + 1 < bytes.len() {
                    i += 2;
                }
            }
            b'$' => match closed_dollar_quote(bytes, i) {
                Some(end) => {
                    masks.push((i, end));
                    i = end;
                }
                None => i += 1,
            },
            _ => i += 1,
        }
    }
    if masks.is_empty() {
        return Cow::Borrowed(sql);
    }

    let mut out = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    for (start, end) in masks {
        out.extend_from_slice(&bytes[cursor..start]);
        let width = end - start;
        // Same byte length so split offsets still address `sql`.
        out.push(b'\'');
        out.extend(std::iter::repeat_n(b' ', width - 2));
        out.push(b'\'');
        cursor = end;
    }
    out.extend_from_slice(&bytes[cursor..]);
    debug_assert_eq!(out.len(), bytes.len());
    Cow::Owned(String::from_utf8(out).expect("dollar-quote mask stays valid utf-8"))
}

fn skip_quoted(bytes: &[u8], start: usize, quote: u8) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == quote {
            if bytes.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    i
}

/// `$tag$...$tag$` or `$$...$$`. A tag is empty, or a letter, `_`, or any byte
/// `>= 0x80`, then those or digits. Same rule as DuckDB and Postgres.
fn closed_dollar_quote(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'$') {
        return None;
    }
    let mut j = start + 1;
    if bytes.get(j) != Some(&b'$') {
        let first = *bytes.get(j)?;
        if !is_dollar_tag_start(first) {
            return None;
        }
        j += 1;
        while j < bytes.len() && is_dollar_tag_cont(bytes[j]) {
            j += 1;
        }
        if bytes.get(j) != Some(&b'$') {
            return None;
        }
    }
    j += 1;
    let tag = &bytes[start..j];
    let mut i = j;
    while i + tag.len() <= bytes.len() {
        if &bytes[i..i + tag.len()] == tag {
            return Some(i + tag.len());
        }
        i += 1;
    }
    None
}

fn is_dollar_tag_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn is_dollar_tag_cont(byte: u8) -> bool {
    is_dollar_tag_start(byte) || byte.is_ascii_digit()
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

    /// A `;` must not split a statement, and a `'` must not merge the next one,
    /// when both sit inside a dollar quote.
    #[test]
    fn test_split_duckdb_dollar_quoted_strings() {
        let sql = "comment on column t.c is $dbt_comment_literal_block$it's a; test$dbt_comment_literal_block$; select 2";
        let expected = vec![
            "comment on column t.c is $dbt_comment_literal_block$it's a; test$dbt_comment_literal_block$",
            " select 2",
        ];
        for adapter in [
            AdapterType::DuckDB,
            AdapterType::LakeCompute,
            AdapterType::Postgres,
        ] {
            assert_eq!(split(sql, adapter), expected, "{adapter:?}");
        }
        assert_eq!(
            split("select $$it's a; test$$; select 2", AdapterType::DuckDB),
            vec!["select $$it's a; test$$", " select 2"]
        );
        assert_eq!(
            split("select $é$it's a; test$é$; select 2", AdapterType::DuckDB),
            vec!["select $é$it's a; test$é$", " select 2"]
        );
        assert_eq!(
            split("select '$a$x$a$'; select 2", AdapterType::DuckDB),
            vec!["select '$a$x$a$'", " select 2"]
        );
        // `$$` inside backtick identifiers is not a dollar quote.
        assert_eq!(
            split(
                "select `a$$` from t; select `b$$` from u",
                AdapterType::DuckDB
            ),
            vec!["select `a$$` from t", " select `b$$` from u"]
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

    /// A Databricks `metric_view`'s compiled DDL wraps its YAML spec in
    /// `$$...$$`. A `;` inside a YAML block-literal comment must not split
    /// the statement before the closing `$$`.
    #[test]
    fn test_split_databricks_dollar_quoted_strings() {
        let sql = r#"
    create or replace view `catalog`.`schema`.`mv_example`
    with metrics
    language yaml
    as $$
    version: 1.1

    source: "`catalog`.`schema`.`fct_example`"

    comment: |
      Some description; more text after the semicolon.

    fields:
      - name: id
        expr: enterprise_id
    $$
  "#;

        let statements = split(sql, AdapterType::Databricks);

        assert_eq!(
            statements.len(),
            1,
            "Expected a single create-view statement, got {}: {:?}",
            statements.len(),
            statements
        );
        assert!(
            statements[0]
                .trim_start()
                .to_lowercase()
                .starts_with("create or replace view")
        );
        assert!(
            statements[0].contains("Some description; more text after the semicolon."),
            "Expected the full YAML body to survive unsplit, got: {}",
            statements[0]
        );
    }

    #[test]
    fn test_split_clickhouse_backslash_escaped_strings() {
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
    fn test_split_clickhouse_dollar_quoted_strings() {
        assert_eq!(
            split("select $tag$a;b$tag$; select 2", AdapterType::ClickHouse),
            vec!["select $tag$a;b$tag$", " select 2"]
        );
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
