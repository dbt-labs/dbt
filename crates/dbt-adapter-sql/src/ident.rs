use core::fmt;
use std::num::NonZero;

use dbt_adapter_core::{AdapterType, quote_char};

use super::is_keyword_ignore_ascii_case;
use super::tokenizer::QuotingStyle;

/// An identifier and its provenance (from quoted or an plain identifier in the input source).
///
/// The name [Ident::Unquoted] means that the identifier was *quoted* in the
/// input source, but we call it "unquoted" because we have removed the
/// quoting characters and unescaped any escaped characters in the token.
///
/// This makes manipulating identifiers easier, because we don't have to
/// worry about escaping and quoting until we want to format the identifier
/// using the [Ident::display] method that takes a [AdapterType] and quotes the
/// identifier if necessary according to the specific dialect rules.
///
/// This also lets us preserve intentional quoting in the input source, even
/// if the identifier could have been represented as a plain identifier. For
/// example, the identifier `"MyTable"` (with quotes) will be represented
/// as `Unquoted('"', "MyTable")`, even though it could have been represented
/// as `Plain("MyTable")`. This is important for dialects like Snowflake
/// where unquoted identifiers are normalized to uppercase, so the quotes
/// are necessary to preserve the original casing.
#[derive(Debug, Clone)]
pub enum Ident {
    /// Identifier that was not quoted in the input source.
    Plain(String),
    /// Identifier that was quoted in the input source and has been unescaped.
    Unquoted(QuotingStyle, String),
}

impl AsRef<str> for Ident {
    fn as_ref(&self) -> &str {
        match self {
            Ident::Unquoted(_, s) => s.as_ref(),
            Ident::Plain(s) => s.as_ref(),
        }
    }
}

impl Ident {
    pub fn new(s: impl Into<String>, backend: AdapterType) -> Self {
        let s: String = s.into();
        if must_be_quoted(&s, backend) {
            Ident::Unquoted(canonical_quote(backend), s)
        } else {
            Ident::Plain(s)
        }
    }

    pub fn plain(s: impl Into<String>) -> Self {
        Ident::Plain(s.into())
    }

    pub fn unquoted(quote: QuotingStyle, s: impl Into<String>) -> Self {
        Ident::Unquoted(quote, s.into())
    }

    pub fn display(&self, backend: AdapterType) -> IdentDisplay<'_> {
        IdentDisplay(self, backend)
    }

    /// Converts the identifier to a string losing any quoting information.
    ///
    /// This is bad because quoting governs how an identifier is interpreted
    /// regarding case-sensitivity and allowed characters, so you should worry
    /// every time you use this method. [Ident::display] will render the identifier
    /// with quotes if necessary according to the backend's dialect rules.
    pub(crate) fn to_string_lossy(&self) -> &String {
        match self {
            Ident::Unquoted(_, s) => s,
            Ident::Plain(s) => s,
        }
    }
}

pub struct IdentDisplay<'a>(&'a Ident, AdapterType);

impl fmt::Display for IdentDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Ident::Unquoted(quote, s) => _render_ident_in_quotes(*quote, s, self.1, f),
            Ident::Plain(s) => write!(f, "{s}"),
        }
    }
}

fn _render_ident_in_quotes(
    quote: QuotingStyle,
    s: &str,
    _backend: AdapterType,
    f: &mut fmt::Formatter<'_>,
) -> fmt::Result {
    // TODO: use backend to determine how to escape the quote character
    write!(f, "{}", quote.opening())?;
    let closing: &str = quote.closing();
    let q = closing.chars().next().unwrap(); // `, ", or '
    for c in s.chars() {
        if c == q {
            // Escape the quote character by doubling it
            write!(f, "{closing}{closing}")?;
        } else {
            write!(f, "{c}")?;
        }
    }
    write!(f, "{closing}")
}

pub fn max_identifier_length(adapter_type: AdapterType) -> Option<NonZero<usize>> {
    use AdapterType::*;
    match adapter_type {
        Postgres => {
            // SAFETY: literal 63 is never 0
            Some(unsafe { NonZero::new_unchecked(63) })
        }
        Redshift => {
            // SAFETY: literal 127 is never 0
            Some(unsafe { NonZero::new_unchecked(127) })
        }
        Snowflake | Bigquery | Databricks | Spark | DuckDB | Salesforce | Fabric | ClickHouse
        | Exasol | Athena | Starburst | Trino | Datafusion | Dremio | Oracle | LakeCompute => None,
    }
}

// TODO: implement a separate struct Idents that can be used as (BTree|Hash)(Map|Set) keys
// (a separate struct that binds the backend is needed because comparing Idents is backend-dependent)
//
// ## PostgreSQL
//
// The identifiers FOO, foo, and "foo" are considered the same by PostgreSQL, but "Foo" and "FOO"
// are different from these three and each other.
//
// https://www.postgresql.org/docs/current/sql-syntax-lexical.html#SQL-SYNTAX-IDENTIFIERS

/// The canonical quoting style used to quote identifiers in this backend's dialect.
pub const fn canonical_quote(backend: AdapterType) -> QuotingStyle {
    use AdapterType::*;
    match backend {
        Bigquery | Databricks | Spark | Athena => QuotingStyle::Backtick,
        ClickHouse | Exasol | Snowflake | Redshift | Postgres | Salesforce | DuckDB => {
            QuotingStyle::Double
        }
        // https://learn.microsoft.com/en-us/sql/t-sql/statements/set-quoted-identifier-transact-sql?view=sql-server-ver17
        Fabric => QuotingStyle::Double,
        // Fabric => QuotingStyle::Bracketed,
        _ => QuotingStyle::Double,
    }
}

/// Returns true if the given character is a valid character for an
/// unquoted identifier in this backend's dialect.
pub fn is_valid_ident_char(c: char, backend: AdapterType) -> bool {
    use AdapterType::*;
    match backend {
        Bigquery => c.is_alphanumeric() || ['_', '-', '$'].contains(&c),
        Snowflake => {
            // TODO: revert this once
            // https://github.com/sdf-labs/sdf/issues/3328 is fixed:
            // c.is_alphanumeric() || ['_', '`', '@'].contains(&c)
            c != '.' && c != quote_char(backend) && !c.is_whitespace() && c != '/' && c != ';'
        }
        Athena // TODO: check these fallbacks against documentation of these dialects
            | Postgres
            | Databricks
            | Spark
            | Redshift
            | Salesforce
            | DuckDB
            | Fabric
            | ClickHouse
            | Starburst
            | Trino
            | Datafusion
            | Dremio
            | Oracle
            | LakeCompute
            | Exasol => c.is_alphanumeric() || c == '_',
    }
}

/// Returns true iff the identifier absolutely MUST be quoted when formatting
/// to source code form in this backend's dialect.
///
/// For instance, if an column name contains a `-` character, it must be quoted,
/// because `-` is not a valid character in an unquoted identifier. It would be
/// a syntax error to write:
///
///     SELECT my-column FROM tbl;
///
/// But in PostgreSQL, the query above is valid if the identifier is quoted:
///
///    SELECT "my-column" FROM tbl;
///
/// IMPORTANT: an identifier can't represent user intention to quote or not quote.
/// If the user wrote `"MyTable"` (with quotes) in the input source, we should carry
/// that intention in a `Indent::Unquoted(Double, "MyTable")` value, even though
/// `must_be_quoted("MyTable", _)` returns false for all backends.
pub fn must_be_quoted(id: &str, backend: AdapterType) -> bool {
    let mut chars = id.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return true, // Empty identifiers MUST be quoted
    };

    // If the first character is not [_a-zA-Z], the identifier MUST be quoted
    if first != '_' && !first.is_ascii_alphabetic() {
        return true;
    }

    // Look for invalid characters on the rest of the identifier.
    // The first character is already checked above.
    let has_invalid_char = chars.any(|c| {
        !is_valid_ident_char(c, backend)
            // BigQuery allows hyphens in unquoted identifiers in certain
            // contexts (e.g. table names), but we still quote them here.
            // Snowflake requires hyphenated identifiers to be quoted.
            || (matches!(backend, AdapterType::Bigquery | AdapterType::Snowflake) && c == '-')
    });
    // Invalid characters MUST be in a quoted identifier (sometimes escaped)
    if has_invalid_char {
        return true;
    }

    // Reserved keywords MUST to be quoted to avoid confusing the lexer/parser
    is_keyword_ignore_ascii_case(id, backend).is_some()
}

/// Reduce `name` to a bare, injection-safe identifier for `backend`, dropping any
/// character the backend won't accept unquoted.
///
/// DuckDB: keep only ASCII alphanumerics and `_` — used for ATTACH aliases so the
/// routing database name matches the attached alias exactly, and so a catalog/alias
/// name can never break out of a quoted identifier or inject SQL. We filter over
/// `bytes()`: the continuation bytes of a multi-byte UTF-8 scalar always have their
/// high bit set, so `is_ascii_alphanumeric` rejects them and no interior byte leaks
/// through. Other backends currently pass the name through unchanged.
pub fn sanitize_identifier(name: &str, backend: AdapterType) -> String {
    match backend {
        AdapterType::DuckDB => name
            .bytes()
            .filter(|&b| b.is_ascii_alphanumeric() || b == b'_')
            .map(char::from)
            .collect(),
        _ => name.to_string(),
    }
}

/// Render `id` as an unconditionally quoted identifier for `backend`, escaping
/// embedded quote characters by doubling. Unlike [`Ident::new`], which quotes
/// only when required, this always quotes — for composing SQL over identifiers
/// of unknown provenance (e.g. DuckDB `DESCRIBE "db"."schema"."table"`), where
/// an unquoted rendering could re-interpret case or special characters.
pub fn quote_identifier(id: &str, backend: AdapterType) -> String {
    Ident::unquoted(canonical_quote(backend), id)
        .display(backend)
        .to_string()
}

/// Escape `s` for inclusion inside a single-quoted SQL *string literal*, so a
/// value can never terminate the literal early. The literal-value member of
/// the family next to [`sanitize_identifier`] (strip) and [`quote_identifier`]
/// (quote), which handle identifiers.
pub fn escape_string_literal(s: &str, _backend: AdapterType) -> String {
    // ANSI '' doubling — correct for every currently supported backend.
    // Dialects with different literal-escape rules (e.g. backslash-escaped
    // strings) grow a match arm on `_backend` when they arrive.
    s.replace('\'', "''")
}

#[cfg(test)]
mod sanitize_tests {
    use super::*;

    #[test]
    fn snowflake_hyphen_must_be_quoted() {
        assert!(must_be_quoted("GIFT-CARDS", AdapterType::Snowflake));
        assert!(!must_be_quoted("GIFT_CARDS", AdapterType::Snowflake));
    }

    #[test]
    fn quote_identifier_always_quotes_and_escapes() {
        assert_eq!(
            quote_identifier("orders", AdapterType::DuckDB),
            "\"orders\""
        );
        assert_eq!(
            quote_identifier("od\"d", AdapterType::DuckDB),
            "\"od\"\"d\""
        );
        assert_eq!(quote_identifier("col", AdapterType::Bigquery), "`col`");
    }

    #[test]
    fn escape_string_literal_doubles_single_quotes() {
        let e = |s: &str| escape_string_literal(s, AdapterType::DuckDB);
        assert_eq!(e("o'brien"), "o''brien");
        assert_eq!(e("'; DROP TABLE x; --"), "''; DROP TABLE x; --");
        assert_eq!(e("plain"), "plain");
    }

    #[test]
    fn duckdb_sanitize_strips_sql_metacharacters() {
        let s = |n: &str| sanitize_identifier(n, AdapterType::DuckDB);
        assert_eq!(s("foo\";DROP TABLE bar;--"), "fooDROPTABLEbar");
        assert_eq!(s("my.catalog.name"), "mycatalogname");
        assert_eq!(s("'); DROP"), "DROP");
        assert_eq!(s("a b\tc\n"), "abc");
        assert_eq!(s("under_score123"), "under_score123");
        // Multi-byte Unicode is dropped entirely (no stray continuation bytes).
        assert_eq!(s("café—x"), "cafx");
    }
}
