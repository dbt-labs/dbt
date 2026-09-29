use std::collections::HashSet;

use dbt_adapter_core::{AdapterType, quote_char};
use once_cell::sync::Lazy;

pub fn format_ident(id: &str, adapter: AdapterType) -> String {
    if !need_quotes(id, adapter) {
        return id.to_string();
    }
    // Fabric/T-SQL delimits identifiers with brackets, not a symmetric quote
    // char. Everything else defers to `quote_char`, the single source of truth
    // for the dialect's identifier delimiter (backtick for BigQuery/Databricks/
    // Spark/ClickHouse, double quote otherwise).
    if adapter == AdapterType::Fabric {
        return format!("[{id}]");
    }
    let quote = quote_char(adapter);
    format!("{quote}{id}{quote}")
}

pub fn need_quotes(id: &str, adapter: AdapterType) -> bool {
    if id.is_empty() {
        return true;
    }

    let mut chars = id.chars();

    // First character rules
    let first_char = chars.next().unwrap();
    let valid_start = match adapter {
        AdapterType::Fabric => {
            first_char == '_'
                || first_char == '@'
                || first_char == '#'
                || first_char.is_ascii_alphabetic()
        }
        _ => first_char == '_' || first_char.is_ascii_alphabetic(),
    };

    if !valid_start {
        return true;
    }

    // Remaining characters
    if chars.any(|c| !is_valid_identifier_char(adapter, c)) {
        return true;
    }

    // Reserved keywords (simple baseline)
    let reserved = reserved_keywords(adapter);
    if reserved.contains(&id.to_ascii_uppercase().as_str()) {
        return true;
    }

    // Optional: T-SQL / Fabric identifier length limit
    if matches!(adapter, AdapterType::Fabric) && id.len() > 128 {
        return true;
    }

    // Snowflake normalizes unquoted identifiers to uppercase, so any lowercase
    // letter forces quoting to preserve case.
    if adapter == AdapterType::Snowflake && id.chars().any(|c| c.is_ascii_lowercase()) {
        return true;
    }

    false
}

fn is_valid_identifier_char(adapter: AdapterType, c: char) -> bool {
    match adapter {
        AdapterType::Fabric => c.is_alphanumeric() || c == '_',
        _ => c.is_alphanumeric() || c == '_',
    }
}

static EMPTY_KEYWORDS: Lazy<HashSet<&'static str>> = Lazy::new(HashSet::new);

pub fn reserved_keywords(adapter: AdapterType) -> &'static HashSet<&'static str> {
    match adapter {
        AdapterType::Fabric => &FABRIC_KEYWORDS,
        AdapterType::Exasol => &EXASOL_KEYWORDS,
        // For other adapters the proprietary TypeOpsImpl handles keyword checks via sdf-frontend;
        // the source-available fallback uses an empty set.
        _ => &EMPTY_KEYWORDS,
    }
}

// Unquoted identifiers matching an Exasol reserved word fail to parse, so they
// force quoting.
static EXASOL_KEYWORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    dbt_sql_keywords::exasol::RESERVED_KEYWORDS
        .iter()
        .copied()
        .collect()
});

//TODO: revisit after Dialect is added for TSQL
static FABRIC_KEYWORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    HashSet::from([
        "ADD",
        "EXTERNAL",
        "PROCEDURE",
        "ALL",
        "FETCH",
        "PUBLIC",
        "ALTER",
        "FILE",
        "RAISERROR",
        "AND",
        "FILLFACTOR",
        "READ",
        "ANY",
        "FOR",
        "READTEXT",
        "AS",
        "FOREIGN",
        "RECONFIGURE",
        "ASC",
        "FREETEXT",
        "REFERENCES",
        "AUTHORIZATION",
        "FREETEXTTABLE",
        "REPLICATION",
        "BACKUP",
        "FROM",
        "RESTORE",
        "BEGIN",
        "FULL",
        "RESTRICT",
        "BETWEEN",
        "FUNCTION",
        "RETURN",
        "BREAK",
        "GOTO",
        "REVERT",
        "BROWSE",
        "GRANT",
        "REVOKE",
        "BULK",
        "GROUP",
        "RIGHT",
        "BY",
        "HAVING",
        "ROLLBACK",
        "CASCADE",
        "HOLDLOCK",
        "ROWCOUNT",
        "CASE",
        "IDENTITY",
        "ROWGUIDCOL",
        "CHECK",
        "IDENTITY_INSERT",
        "RULE",
        "CHECKPOINT",
        "IDENTITYCOL",
        "SAVE",
        "CLOSE",
        "IF",
        "SCHEMA",
        "CLUSTERED",
        "IN",
        "SECURITYAUDIT",
        "COALESCE",
        "INDEX",
        "SELECT",
        "COLLATE",
        "INNER",
        "SEMANTICKEYPHRASETABLE",
        "COLUMN",
        "INSERT",
        "SEMANTICSIMILARITYDETAILSTABLE",
        "COMMIT",
        "INTERSECT",
        "SEMANTICSIMILARITYTABLE",
        "COMPUTE",
        "INTO",
        "SESSION_USER",
        "CONSTRAINT",
        "IS",
        "SET",
        "CONTAINS",
        "JOIN",
        "SETUSER",
        "CONTAINSTABLE",
        "KEY",
        "SHUTDOWN",
        "CONTINUE",
        "KILL",
        "SOME",
        "CONVERT",
        "LEFT",
        "STATISTICS",
        "CREATE",
        "LIKE",
        "SYSTEM_USER",
        "CROSS",
        "LINENO",
        "TABLE",
        "CURRENT",
        "LOAD",
        "TABLESAMPLE",
        "CURRENT_DATE",
        "MERGE",
        "TEXTSIZE",
        "CURRENT_TIME",
        "NATIONAL",
        "THEN",
        "CURRENT_TIMESTAMP",
        "NOCHECK",
        "TO",
        "CURRENT_USER",
        "NONCLUSTERED",
        "TOP",
        "CURSOR",
        "NOT",
        "TRAN",
        "DATABASE",
        "NULL",
        "TRANSACTION",
        "DBCC",
        "NULLIF",
        "TRIGGER",
        "DEALLOCATE",
        "OF",
        "TRUNCATE",
        "DECLARE",
        "OFF",
        "TRY_CONVERT",
        "DEFAULT",
        "OFFSETS",
        "TSEQUAL",
        "DELETE",
        "ON",
        "UNION",
        "DENY",
        "OPEN",
        "UNIQUE",
        "DESC",
        "OPENDATASOURCE",
        "UNPIVOT",
        "DISK",
        "OPENQUERY",
        "UPDATE",
        "DISTINCT",
        "OPENROWSET",
        "UPDATETEXT",
        "DISTRIBUTED",
        "OPENXML",
        "USE",
        "DOUBLE",
        "OPTION",
        "USER",
        "DROP",
        "OR",
        "VALUES",
        "DUMP",
        "ORDER",
        "VARYING",
        "ELSE",
        "OUTER",
        "VIEW",
        "END",
        "OVER",
        "WAITFOR",
        "ERRLVL",
        "PERCENT",
        "WHEN",
        "ESCAPE",
        "PIVOT",
        "WHERE",
        "EXCEPT",
        "PLAN",
        "WHILE",
        "EXEC",
        "PRECISION",
        "WITH",
        "EXECUTE",
        "PRIMARY",
        "WITHIN GROUP",
        "EXISTS",
        "PRINT",
        "WRITETEXT",
        "EXIT",
        "PROC",
        "LABEL", // Synapse-exclusive
        // ISO / ODBC reserved keywords
        "ABSOLUTE",
        "EXECUTE",
        "OVERLAPS",
        "ACTION",
        "PAD",
        "ADA",
        "PARTIAL",
        "PASCAL",
        "EXTRACT",
        "POSITION",
        "ALLOCATE",
        "FALSE",
        "PRECISION",
        "FIRST",
        "PRESERVE",
        "FLOAT",
        "PRIOR",
        "ARE",
        "FOR",
        "PRIVILEGES",
        "ASSERTION",
        "FOUND",
        "PUBLIC",
        "AT",
        "FROM",
        "READ",
        "AUTHORIZATION",
        "FULL",
        "REAL",
        "AVG",
        "GET",
        "REFERENCES",
        "BEGIN",
        "GLOBAL",
        "RELATIVE",
        "BETWEEN",
        "GO",
        "RESTRICT",
        "BIT",
        "GOTO",
        "REVOKE",
        "BIT_LENGTH",
        "GRANT",
        "RIGHT",
        "BOTH",
        "GROUP",
        "ROLLBACK",
        "BY",
        "HAVING",
        "ROWS",
        "CASCADE",
        "HOUR",
        "SCHEMA",
        "CASCADED",
        "IDENTITY",
        "SCROLL",
        "CASE",
        "IMMEDIATE",
        "SECOND",
        "CAST",
        "IN",
        "SECTION",
        "CATALOG",
        "INCLUDE",
        "SELECT",
        "CHAR",
        "INDEX",
        "SESSION",
        "CHAR_LENGTH",
        "INDICATOR",
        "SESSION_USER",
        "CHARACTER",
        "INITIALLY",
        "SET",
        "CHARACTER_LENGTH",
        "INNER",
        "SIZE",
        "CHECK",
        "INPUT",
        "SMALLINT",
        "CLOSE",
        "INSENSITIVE",
        "SOME",
        "COALESCE",
        "INSERT",
        "SPACE",
        "COLLATE",
        "INT",
        "SQL",
        "COLLATION",
        "INTEGER",
        "SQLCA",
        "COLUMN",
        "INTERSECT",
        "SQLCODE",
        "COMMIT",
        "INTERVAL",
        "SQLERROR",
        "CONNECT",
        "INTO",
        "SQLSTATE",
        "CONNECTION",
        "IS",
        "SQLWARNING",
        "CONSTRAINT",
        "ISOLATION",
        "SUBSTRING",
        "CONSTRAINTS",
        "JOIN",
        "SUM",
        "CONTINUE",
        "KEY",
        "SYSTEM_USER",
        "CONVERT",
        "LANGUAGE",
        "TABLE",
        "CORRESPONDING",
        "LAST",
        "TEMPORARY",
        "COUNT",
        "LEADING",
        "THEN",
        "CREATE",
        "LEFT",
        "TIME",
        "CROSS",
        "LEVEL",
        "TIMESTAMP",
        "CURRENT",
        "LIKE",
        "TIMEZONE_HOUR",
        "CURRENT_DATE",
        "LOCAL",
        "TIMEZONE_MINUTE",
        "CURRENT_TIME",
        "LOWER",
        "TO",
        "CURRENT_TIMESTAMP",
        "MATCH",
        "TRAILING",
        "CURRENT_USER",
        "MAX",
        "TRANSACTION",
        "CURSOR",
        "MIN",
        "TRANSLATE",
        "DATE",
        "MINUTE",
        "TRANSLATION",
        "DAY",
        "MODULE",
        "TRIM",
        "DEALLOCATE",
        "MONTH",
        "TRUE",
        "DEC",
        "NAMES",
        "UNION",
        "DECIMAL",
        "NATIONAL",
        "UNIQUE",
        "DECLARE",
        "NATURAL",
        "UNKNOWN",
        "DEFAULT",
        "NCHAR",
        "UPDATE",
        "DEFERRABLE",
        "NEXT",
        "UPPER",
        "DEFERRED",
        "NO",
        "USAGE",
        "DELETE",
        "NONE",
        "USER",
        "DESC",
        "NOT",
        "USING",
        "DESCRIBE",
        "NULL",
        "VALUE",
        "DESCRIPTOR",
        "NULLIF",
        "VALUES",
        "DIAGNOSTICS",
        "NUMERIC",
        "VARCHAR",
        "DISCONNECT",
        "OCTET_LENGTH",
        "VARYING",
        "DISTINCT",
        "OF",
        "VIEW",
        "DOMAIN",
        "ON",
        "WHEN",
        "DOUBLE",
        "ONLY",
        "WHENEVER",
        "DROP",
        "OPEN",
        "WHERE",
        "ELSE",
        "OPTION",
        "WITH",
        "END",
        "OR",
        "WORK",
        "END-EXEC",
        "ORDER",
        "WRITE",
        "ESCAPE",
        "OUTER",
        "YEAR",
        "EXCEPT",
        "OUTPUT",
        "ZONE",
    ])
});

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_adapter_core::AdapterType;

    #[test]
    fn test_format_ident_unquoted() {
        let id = "my_column";
        let formatted = format_ident(id, AdapterType::Fabric);
        assert_eq!(formatted, "my_column");
    }

    #[test]
    fn test_format_ident_quoted_fabric() {
        let id = "select";
        let formatted = format_ident(id, AdapterType::Fabric);
        assert_eq!(formatted, "[select]");
    }

    #[test]
    fn test_format_ident_quoted_clickhouse_does_not_escape_backticks() {
        let id = "a`b";
        let formatted = format_ident(id, AdapterType::ClickHouse);
        assert_eq!(formatted, "`a`b`");
    }

    #[test]
    fn test_format_ident_quoted_bigquery_uses_backticks() {
        // Regression for dbt-core#15561: BigQuery project ids with dashes must be
        // backtick-quoted, matching `quote_char`, not double-quoted.
        assert_eq!(
            format_ident("my-gcp-project", AdapterType::Bigquery),
            "`my-gcp-project`"
        );
    }

    #[test]
    fn test_format_ident_quoted_backtick_dialects() {
        for adapter in [AdapterType::Databricks, AdapterType::Spark] {
            assert_eq!(format_ident("a-b", adapter), "`a-b`");
        }
    }

    #[test]
    fn test_format_ident_quoted_default() {
        // Non-Fabric/ClickHouse adapters use an empty keyword baseline, so reserved
        // words are not detected and the identifier comes back unquoted.
        let id = "select";
        assert_eq!(format_ident(id, AdapterType::Postgres), "select");
    }

    #[test]
    fn test_need_quotes_empty() {
        assert!(need_quotes("", AdapterType::Fabric));
    }

    #[test]
    fn test_need_quotes_invalid_start_char() {
        assert!(need_quotes("1abc", AdapterType::Fabric));
        assert!(!need_quotes("_abc", AdapterType::Fabric));
        assert!(!need_quotes("@abc", AdapterType::Fabric));
        assert!(!need_quotes("#abc", AdapterType::Fabric));
        assert!(need_quotes("&abc", AdapterType::Fabric));
    }

    #[test]
    fn test_need_quotes_reserved_keyword() {
        assert!(need_quotes("SELECT", AdapterType::Fabric));
        assert!(need_quotes("select", AdapterType::Fabric));
    }

    #[test]
    fn test_is_valid_identifier_char() {
        assert!(is_valid_identifier_char(AdapterType::Fabric, 'a'));
        assert!(is_valid_identifier_char(AdapterType::Fabric, 'Z'));
        assert!(is_valid_identifier_char(AdapterType::Fabric, '_'));
        assert!(!is_valid_identifier_char(AdapterType::Fabric, '-'));
    }
}
