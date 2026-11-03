use super::*;

use strum::IntoEnumIterator;

use dbt_frontend_common::Dialect;

use crate::SUPPORTED_DIALECTS;

#[test]
fn test_sql_split_statements() {
    // Test basic splitting - no filtering happens
    assert_eq!(do_sql_split_statements("", None), Vec::<&str>::new());
    assert_eq!(
        do_sql_split_statements("SELECT 1; SELECT 2; SELECT 3;", None),
        vec!["SELECT 1", " SELECT 2", " SELECT 3"]
    );

    // Empty statements are NOT filtered
    assert_eq!(do_sql_split_statements(";;;", None), vec!["", "", ""]);

    // Comments are NOT filtered (filtering happens in adapter layer)
    assert_eq!(
        do_sql_split_statements("select 1; /* end comment */", None),
        vec!["select 1", " /* end comment */"]
    );
    assert_eq!(
        do_sql_split_statements("select 1; -- line comment", None),
        vec!["select 1", " -- line comment"]
    );

    // Statements with embedded comments are kept as-is
    assert_eq!(
        do_sql_split_statements("/* before */ select 1 /* after */", None),
        vec!["/* before */ select 1 /* after */"]
    );

    // SingleStore statement splitting with backticks and :> operator
    assert_eq!(
        do_sql_split_statements(
            "SELECT `col;name` :> signed FROM `tab`; SELECT '42' :> int;",
            Some(Dialect::SingleStore),
        ),
        vec![
            "SELECT `col;name` :> signed FROM `tab`",
            " SELECT '42' :> int"
        ]
    );
}

#[test]
fn test_singlestore_mysql_splitter() {
    // 1. # comments: semicolon inside # comment should not split
    let hash_comment = "SELECT 1; # comment ; not split\nSELECT 2;";
    assert_eq!(
        do_sql_split_statements(hash_comment, Some(Dialect::SingleStore)),
        vec!["SELECT 1", " # comment ; not split\nSELECT 2"]
    );

    // 2. --x not a comment: semicolon after --x should split
    let minus_minus_x = "SELECT 1 --x;\nSELECT 2;";
    assert_eq!(
        do_sql_split_statements(minus_minus_x, Some(Dialect::SingleStore)),
        vec!["SELECT 1 --x", "\nSELECT 2"]
    );

    // 3. /*! ... */ executable comments
    let version_comment = "/*!40101 SET @saved_cs_client = @@character_set_client */; SELECT 2;";
    assert_eq!(
        do_sql_split_statements(version_comment, Some(Dialect::SingleStore)),
        vec![
            "/*!40101 SET @saved_cs_client = @@character_set_client */",
            " SELECT 2"
        ]
    );

    // 4. Semicolon inside "..." double quoted string
    let double_quoted = "SELECT \"hello;world\"; SELECT 2;";
    assert_eq!(
        do_sql_split_statements(double_quoted, Some(Dialect::SingleStore)),
        vec!["SELECT \"hello;world\"", " SELECT 2"]
    );

    // 5. Escaped characters: string containing escaped quote and semicolon '\';'
    let escaped_quote_semi = "SELECT '\\';'; SELECT 2;";
    assert_eq!(
        do_sql_split_statements(escaped_quote_semi, Some(Dialect::SingleStore)),
        vec!["SELECT '\\';'", " SELECT 2"]
    );

    // And literal trailing backslash inside string followed by closing quote and semicolon: '\\';
    let escaped_slash = "SELECT '\\\\'; SELECT 2;";
    assert_eq!(
        do_sql_split_statements(escaped_slash, Some(Dialect::SingleStore)),
        vec!["SELECT '\\\\'", " SELECT 2"]
    );

    // 6. Semicolon inside backticks
    let backtick_semi = "SELECT `col;name` FROM t; SELECT 2;";
    assert_eq!(
        do_sql_split_statements(backtick_semi, Some(Dialect::SingleStore)),
        vec!["SELECT `col;name` FROM t", " SELECT 2"]
    );

    // 7. -- comment at EOF without newline
    let dash_eof = "SELECT 1; -- comment at eof";
    assert_eq!(
        do_sql_split_statements(dash_eof, Some(Dialect::SingleStore)),
        vec!["SELECT 1", " -- comment at eof"]
    );
    assert!(is_empty_or_comment_only(
        "-- comment at eof",
        Dialect::SingleStore
    ));

    // 8. Unterminated string
    let unterminated = "SELECT 'unterminated; SELECT 2;";
    assert_eq!(
        do_sql_split_statements(unterminated, Some(Dialect::SingleStore)),
        vec!["SELECT 'unterminated; SELECT 2;"]
    );

    // 9. /*! ... */ containing semicolon
    let version_with_semi = "SELECT /*!40101 1; */ + 2; SELECT 3;";
    assert_eq!(
        do_sql_split_statements(version_with_semi, Some(Dialect::SingleStore)),
        vec!["SELECT /*!40101 1; */ + 2", " SELECT 3"]
    );

    // 10. DELIMITER command: not supported to change delimiter, splits on semicolon
    let delimiter_cmd = "DELIMITER //;\nSELECT 1;\n//\nDELIMITER ;";
    assert_eq!(
        do_sql_split_statements(delimiter_cmd, Some(Dialect::SingleStore)),
        vec!["DELIMITER //", "\nSELECT 1", "\n//\nDELIMITER "]
    );

    // Empty/comment-only checks for MySQL semantics
    assert!(is_empty_or_comment_only(
        "# hash comment",
        Dialect::SingleStore
    ));
    assert!(is_empty_or_comment_only(
        "-- space comment",
        Dialect::SingleStore
    ));
    assert!(is_empty_or_comment_only(
        "/* block comment */",
        Dialect::SingleStore
    ));
    assert!(!is_empty_or_comment_only(
        "--x not comment",
        Dialect::SingleStore
    ));
    assert!(!is_empty_or_comment_only(
        "/*!40101 SELECT 1 */",
        Dialect::SingleStore
    ));
}

fn is_empty(sql: &str, dialect: Dialect) -> bool {
    let dialect = if SUPPORTED_DIALECTS.contains(&dialect) {
        dialect
    } else {
        Dialect::Trino
    };
    is_empty_or_comment_only(sql, dialect)
}

#[test]
fn test_snowflake_terminal_flow_statement() {
    assert_eq!(
        snowflake_terminal_flow_statement("SHOW TABLES"),
        "SHOW TABLES"
    );
    assert_eq!(
        snowflake_terminal_flow_statement(
            "SHOW TABLES ->> SELECT 'first' ->> SELECT \"name\" FROM $1"
        ),
        " SELECT \"name\" FROM $1"
    );
    assert_eq!(
        snowflake_terminal_flow_statement(
            "SHOW TABLES LIKE '->>' /* ->> SELECT */ ->> SELECT $$->>$$, \"->>\" FROM $1"
        ),
        " SELECT $$->>$$, \"->>\" FROM $1"
    );
    assert_eq!(
        snowflake_terminal_flow_statement("SHOW TABLES LIKE 'café' ->> SELECT \"naïve\" FROM $1"),
        " SELECT \"naïve\" FROM $1"
    );
}

#[test]
fn test_is_empty_or_comment_only() {
    // Test the comment detection helper function across all dialects
    for dialect in Dialect::iter() {
        // These should be considered empty/comment-only
        assert!(is_empty("", dialect));
        assert!(is_empty("   ", dialect));
        assert!(is_empty("/* comment */", dialect));
        assert!(is_empty("-- line comment", dialect));
        assert!(is_empty("  /* comment */  ", dialect));
        assert!(is_empty("  -- comment  ", dialect));
        assert!(is_empty("/* comment */ -- line comment", dialect));
        assert!(is_empty("/* multi\nline\ncomment */", dialect));

        // These should NOT be considered empty - SQL with comments should be preserved
        assert!(!is_empty("select 1", dialect));
        assert!(!is_empty("select /* comment */ 1", dialect));
        assert!(!is_empty("select 1 -- comment", dialect));
        assert!(!is_empty("/* comment */ select 1", dialect));

        // Additional critical cases that should NOT be filtered
        assert!(!is_empty("/* before */ select 1 /* after */", dialect));
        assert!(!is_empty("-- comment\nselect 1", dialect));
        assert!(!is_empty("select 1; select 2", dialect));
        assert!(!is_empty("/* comment */\nselect 1\n-- trailing", dialect));
    }
}
