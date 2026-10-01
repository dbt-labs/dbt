//! Sourced from https://learn.microsoft.com/en-us/sql/t-sql/language-elements/reserved-keywords-transact-sql?view=sql-server-ver17

pub static RESERVED_KEYWORDS: &[&str] = &[
    "ABSOLUTE", // ODBC
    "ACTION",   // ODBC
    "ADA",      // ODBC
    "ADD",
    "ALL",
    "ALLOCATE", // ODBC
    "ALTER",
    "AND",
    "ANY",
    "ARE", // ODBC
    "AS",
    "ASC",
    "ASSERTION", // ODBC
    "AT",        // ODBC
    "AUTHORIZATION",
    "AVG", // ODBC
    "BACKUP",
    "BEGIN",
    "BETWEEN",
    "BIT",        // ODBC
    "BIT_LENGTH", // ODBC
    "BOTH",       // ODBC
    "BREAK",
    "BROWSE",
    "BULK",
    "BY",
    "CASCADE",
    "CASCADED", // ODBC
    "CASE",
    "CAST",             // ODBC
    "CATALOG",          // ODBC
    "CHAR",             // ODBC
    "CHARACTER",        // ODBC
    "CHARACTER_LENGTH", // ODBC
    "CHAR_LENGTH",      // ODBC
    "CHECK",
    "CHECKPOINT",
    "CLOSE",
    "CLUSTERED",
    "COALESCE",
    "COLLATE",
    "COLLATION", // ODBC
    "COLUMN",
    "COMMIT",
    "COMPUTE",
    "CONNECT",    // ODBC
    "CONNECTION", // ODBC
    "CONSTRAINT",
    "CONSTRAINTS", // ODBC
    "CONTAINS",
    "CONTAINSTABLE",
    "CONTINUE",
    "CONVERT",
    "CORRESPONDING", // ODBC
    "COUNT",         // ODBC
    "CREATE",
    "CROSS",
    "CURRENT",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "CURSOR",
    "DATABASE",
    "DATE", // ODBC
    "DAY",  // ODBC
    "DBCC",
    "DEALLOCATE",
    "DEC",     // ODBC
    "DECIMAL", // ODBC
    "DECLARE",
    "DEFAULT",
    "DEFERRABLE", // ODBC
    "DEFERRED",   // ODBC
    "DELETE",
    "DENY",
    "DESC",
    "DESCRIBE",    // ODBC
    "DESCRIPTOR",  // ODBC
    "DIAGNOSTICS", // ODBC
    "DISCONNECT",  // ODBC
    "DISK",
    "DISTINCT",
    "DISTRIBUTED",
    "DOMAIN", // ODBC
    "DOUBLE",
    "DROP",
    "DUMP",
    "ELSE",
    "END",
    "END-EXEC", // ODBC
    "ERRLVL",
    "ESCAPE",
    "EXCEPT",
    "EXCEPTION", // ODBC
    "EXEC",
    "EXECUTE",
    "EXISTS",
    "EXIT",
    "EXTERNAL",
    "EXTRACT", // ODBC
    "FALSE",   // ODBC
    "FETCH",
    "FILE",
    "FILLFACTOR",
    "FIRST", // ODBC
    "FLOAT", // ODBC
    "FOR",
    "FOREIGN",
    "FORTRAN", // ODBC
    "FOUND",   // ODBC
    "FREETEXT",
    "FREETEXTTABLE",
    "FROM",
    "FULL",
    "FUNCTION",
    "GET",    // ODBC
    "GLOBAL", // ODBC
    "GO",     // ODBC
    "GOTO",
    "GRANT",
    "GROUP",
    "HAVING",
    "HOLDLOCK",
    "HOUR", // ODBC
    "IDENTITY",
    "IDENTITYCOL",
    "IDENTITY_INSERT",
    "IF",
    "IMMEDIATE", // ODBC
    "IN",
    "INCLUDE", // ODBC
    "INDEX",
    "INDICATOR", // ODBC
    "INITIALLY", // ODBC
    "INNER",
    "INPUT",       // ODBC
    "INSENSITIVE", // ODBC
    "INSERT",
    "INT",     // ODBC
    "INTEGER", // ODBC
    "INTERSECT",
    "INTERVAL", // ODBC
    "INTO",
    "IS",
    "ISOLATION", // ODBC
    "JOIN",
    "KEY",
    "KILL",
    "LABEL",
    "LANGUAGE", // ODBC
    "LAST",     // ODBC
    "LEADING",  // ODBC
    "LEFT",
    "LEVEL", // ODBC
    "LIKE",
    "LINENO",
    "LOAD",
    "LOCAL", // ODBC
    "LOWER", // ODBC
    "MATCH", // ODBC
    "MAX",   // ODBC
    "MERGE",
    "MIN",    // ODBC
    "MINUTE", // ODBC
    "MODULE", // ODBC
    "MONTH",  // ODBC
    "NAMES",  // ODBC
    "NATIONAL",
    "NATURAL", // ODBC
    "NCHAR",   // ODBC
    "NEXT",    // ODBC
    "NO",      // ODBC
    "NOCHECK",
    "NONCLUSTERED",
    "NONE", // ODBC
    "NOT",
    "NULL",
    "NULLIF",
    "NUMERIC",      // ODBC
    "OCTET_LENGTH", // ODBC
    "OF",
    "OFF",
    "OFFSETS",
    "ON",
    "ONLY", // ODBC
    "OPEN",
    "OPENDATASOURCE",
    "OPENQUERY",
    "OPENROWSET",
    "OPENXML",
    "OPTION",
    "OR",
    "ORDER",
    "OUTER",
    "OUTPUT", // ODBC
    "OVER",
    "OVERLAPS", // ODBC
    "PAD",      // ODBC
    "PARTIAL",  // ODBC
    "PASCAL",   // ODBC
    "PERCENT",
    "PIVOT",
    "PLAN",
    "POSITION", // ODBC
    "PRECISION",
    "PREPARE",  // ODBC
    "PRESERVE", // ODBC
    "PRIMARY",
    "PRINT",
    "PRIOR",      // ODBC
    "PRIVILEGES", // ODBC
    "PROC",
    "PROCEDURE",
    "PUBLIC",
    "RAISERROR",
    "READ",
    "READTEXT",
    "REAL", // ODBC
    "RECONFIGURE",
    "REFERENCES",
    "RELATIVE", // ODBC
    "REPLICATION",
    "RESTORE",
    "RESTRICT",
    "RETURN",
    "REVERT",
    "REVOKE",
    "RIGHT",
    "ROLLBACK",
    "ROWCOUNT",
    "ROWGUIDCOL",
    "ROWS", // ODBC
    "RULE",
    "SAVE",
    "SCHEMA",
    "SCROLL",  // ODBC
    "SECOND",  // ODBC
    "SECTION", // ODBC
    "SECURITYAUDIT",
    "SELECT",
    "SEMANTICKEYPHRASETABLE",
    "SEMANTICSIMILARITYDETAILSTABLE",
    "SEMANTICSIMILARITYTABLE",
    "SESSION", // ODBC
    "SESSION_USER",
    "SET",
    "SETUSER",
    "SHUTDOWN",
    "SIZE",     // ODBC
    "SMALLINT", // ODBC
    "SOME",
    "SPACE",      // ODBC
    "SQL",        // ODBC
    "SQLCA",      // ODBC
    "SQLCODE",    // ODBC
    "SQLERROR",   // ODBC
    "SQLSTATE",   // ODBC
    "SQLWARNING", // ODBC
    "STATISTICS",
    "SUBSTRING", // ODBC
    "SUM",       // ODBC
    "SYSTEM_USER",
    "TABLE",
    "TABLESAMPLE",
    "TEMPORARY", // ODBC
    "TEXTSIZE",
    "THEN",
    "TIME",            // ODBC
    "TIMESTAMP",       // ODBC
    "TIMEZONE_HOUR",   // ODBC
    "TIMEZONE_MINUTE", // ODBC
    "TO",
    "TOP",
    "TRAILING", // ODBC
    "TRAN",
    "TRANSACTION",
    "TRANSLATE",   // ODBC
    "TRANSLATION", // ODBC
    "TRIGGER",
    "TRIM", // ODBC
    "TRUE", // ODBC
    "TRUNCATE",
    "TRY_CONVERT",
    "TSEQUAL",
    "UNION",
    "UNIQUE",
    "UNKNOWN", // ODBC
    "UNPIVOT",
    "UPDATE",
    "UPDATETEXT",
    "UPPER", // ODBC
    "USAGE", // ODBC
    "USE",
    "USER",
    "USING", // ODBC
    "VALUE", // ODBC
    "VALUES",
    "VARCHAR", // ODBC
    "VARYING",
    "VIEW",
    "WAITFOR",
    "WHEN",
    "WHENEVER", // ODBC
    "WHERE",
    "WHILE",
    "WITH",
    "WITHIN GROUP",
    "WORK",  // ODBC
    "WRITE", // ODBC
    "WRITETEXT",
    "YEAR", // ODBC
    "ZONE", // ODBC
];

pub static STRICT_NON_RESERVED_KEYWORDS: &[&str] = &[];
pub static NON_RESERVED_KEYWORDS: &[&str] = &[];

pub static FUTURE_KEYWORDS: &[&str] = &[
    "ABSOLUTE",
    "ACTION",
    "ADMIN",
    "AFTER",
    "AGGREGATE",
    "ALIAS",
    "ALLOCATE",
    "ARE",
    "ARRAY",
    "ASENSITIVE",
    "ASSERTION",
    "ASYMMETRIC",
    "AT",
    "ATOMIC",
    "BEFORE",
    "BINARY",
    "BIT",
    "BLOB",
    "BOOLEAN",
    "BOTH",
    "BREADTH",
    "CALL",
    "CALLED",
    "CARDINALITY",
    "CASCADED",
    "CAST",
    "CATALOG",
    "CHAR",
    "CHARACTER",
    "CLASS",
    "CLOB",
    "COLLATION",
    "COLLECT",
    "COMPLETION",
    "CONDITION",
    "CONNECT",
    "CONNECTION",
    "CONSTRAINTS",
    "CONSTRUCTOR",
    "CORR",
    "CORRESPONDING",
    "COVAR_POP",
    "COVAR_SAMP",
    "CUBE",
    "CUME_DIST",
    "CURRENT_CATALOG",
    "CURRENT_DEFAULT_TRANSFORM_GROUP",
    "CURRENT_PATH",
    "CURRENT_ROLE",
    "CURRENT_SCHEMA",
    "CURRENT_TRANSFORM_GROUP_FOR_TYPE",
    "CYCLE",
    "DATA",
    "DATE",
    "DAY",
    "DEC",
    "DECIMAL",
    "DEFERRABLE",
    "DEFERRED",
    "DEPTH",
    "DEREF",
    "DESCRIBE",
    "DESCRIPTOR",
    "DESTROY",
    "DESTRUCTOR",
    "DETERMINISTIC",
    "DICTIONARY",
    "DIAGNOSTICS",
    "DISCONNECT",
    "DOMAIN",
    "DYNAMIC",
    "EACH",
    "ELEMENT",
    "END-EXEC",
    "EQUALS",
    "EVERY",
    "EXCEPTION",
    "FALSE",
    "FILTER",
    "FIRST",
    "FLOAT",
    "FOUND",
    "FREE",
    "FULLTEXTTABLE",
    "FUSION",
    "GENERAL",
    "GET",
    "GLOBAL",
    "GO",
    "GROUPING",
    "HOLD",
    "HOST",
    "HOUR",
    "IGNORE",
    "IMMEDIATE",
    "INDICATOR",
    "INITIALIZE",
    "INITIALLY",
    "INOUT",
    "INPUT",
    "INT",
    "INTEGER",
    "INTERSECTION",
    "INTERVAL",
    "ISOLATION",
    "ITERATE",
    "LANGUAGE",
    "LARGE",
    "LAST",
    "LATERAL",
    "LEADING",
    "LESS",
    "LEVEL",
    "LIKE_REGEX",
    "LIMIT",
    "LN",
    "LOCAL",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "LOCATOR",
    "MAP",
    "MATCH",
    "MEMBER",
    "METHOD",
    "MINUTE",
    "MOD",
    "MODIFIES",
    "MODIFY",
    "MODULE",
    "MONTH",
    "MULTISET",
    "NAMES",
    "NATURAL",
    "NCHAR",
    "NCLOB",
    "NEW",
    "NEXT",
    "NO",
    "NONE",
    "NORMALIZE",
    "NUMERIC",
    "OBJECT",
    "OCCURRENCES_REGEX",
    "OLD",
    "ONLY",
    "OPERATION",
    "ORDINALITY",
    "OUT",
    "OVERLAY",
    "OUTPUT",
    "PAD",
    "PARAMETER",
    "PARAMETERS",
    "PARTIAL",
    "PARTITION",
    "PATH",
    "PERCENT_RANK",
    "PERCENTILE_CONT",
    "PERCENTILE_DISC",
    "POSITION_REGEX",
    "POSTFIX",
    "PREFIX",
    "PREORDER",
    "PREPARE",
    "PRESERVE",
    "PRIOR",
    "PRIVILEGES",
    "RANGE",
    "READS",
    "REAL",
    "RECURSIVE",
    "REF",
    "REFERENCING",
    "REGR_AVGX",
    "REGR_AVGY",
    "REGR_COUNT",
    "REGR_INTERCEPT",
    "REGR_R2",
    "REGR_SLOPE",
    "REGR_SXX",
    "REGR_SXY",
    "REGR_SYY",
    "RELATIVE",
    "RELEASE",
    "RESULT",
    "RETURNS",
    "ROLE",
    "ROLLUP",
    "ROUTINE",
    "ROW",
    "ROWS",
    "SAVEPOINT",
    "SCOPE",
    "SCROLL",
    "SEARCH",
    "SECOND",
    "SECTION",
    "SENSITIVE",
    "SEQUENCE",
    "SESSION",
    "SETS",
    "SIMILAR",
    "SIZE",
    "SMALLINT",
    "SPACE",
    "SPECIFIC",
    "SPECIFICTYPE",
    "SQL",
    "SQLEXCEPTION",
    "SQLSTATE",
    "SQLWARNING",
    "START",
    "STATE",
    "STATEMENT",
    "STATIC",
    "STDDEV_POP",
    "STDDEV_SAMP",
    "STRUCTURE",
    "SUBMULTISET",
    "SUBSTRING_REGEX",
    "SYMMETRIC",
    "SYSTEM",
    "TEMPORARY",
    "TERMINATE",
    "THAN",
    "TIME",
    "TIMESTAMP",
    "TIMEZONE_HOUR",
    "TIMEZONE_MINUTE",
    "TRAILING",
    "TRANSLATE_REGEX",
    "TRANSLATION",
    "TREAT",
    "TRUE",
    "UESCAPE",
    "UNDER",
    "UNKNOWN",
    "UNNEST",
    "USAGE",
    "USING",
    "VALUE",
    "VAR_POP",
    "VAR_SAMP",
    "VARCHAR",
    "VARIABLE",
    "WHENEVER",
    "WIDTH_BUCKET",
    "WINDOW",
    "WITHIN",
    "WITHOUT",
    "WORK",
    "WRITE",
    "XMLAGG",
    "XMLATTRIBUTES",
    "XMLBINARY",
    "XMLCAST",
    "XMLCOMMENT",
    "XMLCONCAT",
    "XMLDOCUMENT",
    "XMLELEMENT",
    "XMLEXISTS",
    "XMLFOREST",
    "XMLITERATE",
    "XMLNAMESPACES",
    "XMLPARSE",
    "XMLPI",
    "XMLQUERY",
    "XMLSERIALIZE",
    "XMLTABLE",
    "XMLTEXT",
    "XMLVALIDATE",
    "YEAR",
    "ZONE",
];
