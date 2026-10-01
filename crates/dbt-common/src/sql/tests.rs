use super::*;

#[test]
fn test_sql_find_statement_delimiters() {
    let input = "SELECT 1; SELECT 2; SELECT 3;";
    let result = sql_find_statement_delimiters(input, None);
    assert_eq!(result, vec![8, 18, 28]);

    let input = "-- a comment;\nSELECT 1 -- another comment;";
    let result = sql_find_statement_delimiters(input, None);
    assert!(result.is_empty());

    let input = " /* a comment;\n */ SELECT 1 ";
    let result = sql_find_statement_delimiters(input, None);
    assert!(result.is_empty());

    let input = " SELECT '/*' ; ";
    let result = sql_find_statement_delimiters(input, None);
    assert_eq!(result, vec![13]);

    let input = " SELECT '/*' ; SELECT '*/' ; ";
    let result = sql_find_statement_delimiters(input, None);
    assert_eq!(result, vec![13, 27]);

    let input = " SELECT ';' as \";\" ";
    let result = sql_find_statement_delimiters(input, None);
    assert!(result.is_empty());

    let input = " /* ;";
    let result = sql_find_statement_delimiters(input, None);
    assert!(result.is_empty());

    let input = "SELECT \"\"\"...;";
    let result = sql_find_statement_delimiters(input, None);
    assert!(result.is_empty());

    let input = "SELECT ''';";
    let result = sql_find_statement_delimiters(input, None);
    assert!(result.is_empty());
}

#[test]
fn test_jinja_sql_find_statement_delimiters() {
    let input = "{% set x = 1 %}; SELECT 1";
    let result = jinja_sql_find_statement_delimiters(input, None);
    assert_eq!(result, vec![15]);

    let input = "{# a comment; #}\nSELECT 1 -- another comment;";
    let result = jinja_sql_find_statement_delimiters(input, None);
    assert!(result.is_empty());

    let input = "{% set x = 1 %} /* a comment;\n */ SELECT 1 ";
    let result = jinja_sql_find_statement_delimiters(input, None);
    assert!(result.is_empty());

    let input = "{#;";
    let result = jinja_sql_find_statement_delimiters(input, None);
    assert!(result.is_empty());

    let input = "{# -- #};";
    let result = jinja_sql_find_statement_delimiters(input, None);
    assert_eq!(result, vec![8]);

    let input = "{# /* #};\n{# */ #};";
    let result = jinja_sql_find_statement_delimiters(input, None);
    assert_eq!(result, vec![8, 18]);

    let input = "{# \" #};";
    let result = jinja_sql_find_statement_delimiters(input, None);
    assert_eq!(result, vec![7]);

    let input = "/* {#*/#};";
    let result = jinja_sql_find_statement_delimiters(input, None);
    assert!(result.is_empty());
}

#[test]
fn test_sql_split_statements() {
    let input = "";
    let result = do_sql_split_statements(input, None);
    assert!(result.is_empty());

    let input = "SELECT 1; SELECT 2; SELECT 3;";
    let result = do_sql_split_statements(input, None);
    assert_eq!(result, vec!["SELECT 1", " SELECT 2", " SELECT 3"]);

    let input = ";;;";
    let result = do_sql_split_statements(input, None);
    assert_eq!(result, vec!["", "", ""]);
}
