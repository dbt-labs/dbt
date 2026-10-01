use super::*;

use strum::IntoEnumIterator;

use dbt_frontend_common::dialect::Dialect;

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
