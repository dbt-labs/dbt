use crate::error::ParseError;
use crate::ident::Identifier;
use serde::{Deserialize, Serialize};
use std::{fmt::Display, str::FromStr};

macro_rules! parse_err {
    ($($arg:tt)*) => {
        Err(ParseError::new(format!($($arg)*)))
    }
}

// unfolded constants.rs from sdf.cli
pub const UPPERCASE_DRAFT_SUFFIX: &str = "___DRAFT";
pub const LOWERCASE_DRAFT_SUFFIX: &str = "___draft";
pub const QUOTED_UPPERCASE_DRAFT_SUFFIX: &str = "___DRAFT\"";
pub const QUOTED_LOWERCASE_DRAFT_SUFFIX: &str = "___draft\"";
pub const DRAFT_SUFFIX_LEN: usize = LOWERCASE_DRAFT_SUFFIX.len();
pub const QUOTED_DRAFT_SUFFIX_LEN: usize = QUOTED_LOWERCASE_DRAFT_SUFFIX.len();

/// Represents a SQL dialect.
///
/// This type is the API for common operations that have dialect-specific
/// behavior.
#[repr(u8)]
#[derive(
    Copy,
    Clone,
    Default,
    Debug,
    Serialize,
    Deserialize,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    enum_map::Enum,
    strum_macros::EnumIter,
)]
// Serializes as the PascalCase variant name. The lowercase aliases accept the
// spelling used by the `dialect:` field of the table and function YAML assets.
pub enum Dialect {
    #[serde(alias = "sdf")]
    Sdf,
    #[default]
    #[serde(alias = "trino", alias = "presto", alias = "Presto")]
    Trino,
    #[serde(alias = "snowflake")]
    Snowflake,
    #[serde(alias = "postgresql")]
    Postgresql,
    #[serde(alias = "bigquery")]
    Bigquery,
    #[serde(alias = "datafusion")]
    DataFusion,
    #[serde(alias = "sparksql")]
    SparkSql,
    #[serde(alias = "sparklp")]
    SparkLp,
    #[serde(alias = "redshift")]
    Redshift,
    #[serde(alias = "databricks")]
    Databricks,
    #[serde(alias = "duckdb")]
    Duckdb,
    #[serde(alias = "singlestore")]
    SingleStore,
}

impl Display for Dialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dialect::Sdf => write!(f, "sdf"),
            Dialect::Trino => write!(f, "trino"),
            Dialect::Snowflake => write!(f, "snowflake"),
            Dialect::Postgresql => write!(f, "postgresql"),
            Dialect::Bigquery => write!(f, "bigquery"),
            Dialect::DataFusion => write!(f, "datafusion"),
            Dialect::SparkSql => write!(f, "sparksql"),
            Dialect::SparkLp => write!(f, "spark-lp"),
            Dialect::Redshift => write!(f, "redshift"),
            Dialect::Databricks => write!(f, "databricks"),
            Dialect::Duckdb => write!(f, "duckdb"),
            Dialect::SingleStore => write!(f, "singlestore"),
        }
    }
}

impl FromStr for Dialect {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Dialect, Self::Err> {
        match input.to_ascii_lowercase().as_str() {
            "sdf" => Ok(Dialect::Sdf),
            "presto" => Ok(Dialect::Trino),
            "trino" => Ok(Dialect::Trino),
            "snowflake" => Ok(Dialect::Snowflake),
            "postgresql" | "postgres" | "salesforce" => Ok(Dialect::Postgresql),
            "bigquery" => Ok(Dialect::Bigquery),
            "datafusion" => Ok(Dialect::DataFusion),
            "sparksql" => Ok(Dialect::SparkSql),
            "sparklp" => Ok(Dialect::SparkLp),
            "spark-lp" => Ok(Dialect::SparkLp),
            "redshift" => Ok(Dialect::Redshift),
            "databricks" => Ok(Dialect::Databricks),
            "duckdb" => Ok(Dialect::Duckdb),
            "singlestore" => Ok(Dialect::SingleStore),

            // "passthrough" adapter type is used to disable most local semantic
            // analysis, so we just map it to the default dialect.
            "passthrough" => Ok(Default::default()),

            _ => parse_err!("Invalid dialect value: '{}'", input),
        }
    }
}

// Miscellaneous dialect-specific functions
impl Dialect {
    pub const fn max_value() -> u8 {
        Dialect::SingleStore as u8
    }

    pub fn is_default(&self) -> bool {
        matches!(self, Dialect::Trino)
    }

    pub fn draft_suffix(&self) -> &'static str {
        match self {
            Dialect::Snowflake => UPPERCASE_DRAFT_SUFFIX,
            _ => LOWERCASE_DRAFT_SUFFIX,
        }
    }

    pub fn is_column_case_sensitive(&self) -> bool {
        matches!(self, Dialect::Snowflake)
    }

    pub fn get_default_col(self) -> String {
        match self {
            Dialect::Trino | Dialect::Redshift => "_sdf::col".to_string(), // this column is not seen by the user
            Dialect::Snowflake => "c".to_string(),
            Dialect::Bigquery => "_field_".to_string(),
            Dialect::Databricks | Dialect::Duckdb | Dialect::SingleStore => "col".to_string(),
            _ => todo!("get_default_col not implemented for {self}"),
        }
    }

    pub fn get_default_col_start(&self) -> usize {
        match self {
            Dialect::Snowflake | Dialect::Trino | Dialect::Redshift => 0,
            Dialect::Bigquery | Dialect::Databricks | Dialect::Duckdb | Dialect::SingleStore => 1,
            _ => todo!("get_default_col_start not implemented for {self}"),
        }
    }
}

// Parsing: this implements a fast identifier parser that doesn't rely on
// Antlr. O(n) time with guaranteed O(1) allocations.
impl Dialect {
    /// The character used to quote identifiers in this dialect.
    pub const fn quote_char(&self) -> char {
        match self {
            Dialect::Sdf | Dialect::Trino => '"',
            Dialect::Bigquery | Dialect::Databricks | Dialect::SingleStore => '`',
            Dialect::Snowflake => '"',
            Dialect::Redshift => '"',
            // TODO: SparkSQL, SparkLP
            _ => '"',
        }
    }

    /// The character used to escape the quote character in a quoted identifier
    /// in this dialect.
    pub const fn escape_char(&self) -> char {
        match self {
            Dialect::Sdf | Dialect::Trino => '"',
            Dialect::Bigquery | Dialect::SingleStore => '\\',
            Dialect::Snowflake => '"',
            Dialect::Redshift => '"',
            _ => '"',
        }
    }

    const fn escaped_quote(&self) -> &'static str {
        match self {
            Dialect::Sdf | Dialect::Trino => "\"\"",
            Dialect::Bigquery => "\\`",
            Dialect::Snowflake => "\"\"",
            Dialect::Redshift => "\"\"",
            _ => "\"\"",
        }
    }

    /// Returns the escaped form of the given identifier in this dialect.
    pub fn escape_identifier(&self, name: &str) -> String {
        match self {
            Dialect::Bigquery => {
                let mut result = String::new();

                let chars = name.chars();
                for c in chars {
                    match c {
                        '\\' => result.push_str("\\\\"),
                        '\n' => result.push_str("\\n"),
                        '\t' => result.push_str("\\t"),
                        '\r' => result.push_str("\\r"),
                        '`' => result.push_str("\\`"),
                        _ => result.push(c),
                    }
                }

                result
            }
            _ => name.replace(self.quote_char(), self.escaped_quote()),
        }
    }

    fn unescape_identifier_char(&self, escaped_char: char) -> char {
        match self {
            Dialect::Bigquery => match escaped_char {
                '\\' => '\\',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '`' => '`',
                _ => escaped_char,
            },
            _ => escaped_char,
        }
    }

    fn is_escape_special_identifier_char(&self, c: char) -> bool {
        match self {
            Dialect::Bigquery => ['\\', 'n', 't', 'r', '`'].contains(&c),
            _ => self.quote_char() == c,
        }
    }

    /// Returns true if the given character is a valid character for an
    /// unquoted identifier in this dialect.
    pub fn is_valid_identifier_char(&self, c: char) -> bool {
        match self {
            Dialect::Sdf | Dialect::Trino => c.is_alphanumeric() || c == '_',
            Dialect::Bigquery => c.is_alphanumeric() || ['_', '-', '$', ':'].contains(&c),
            Dialect::Snowflake => {
                // TODO: revert this once
                // https://github.com/sdf-labs/sdf/issues/3328 is fixed:
                // c.is_alphanumeric() || ['_', '`', '@'].contains(&c)
                c != '.' && c != self.quote_char() && !c.is_whitespace() && c != '/' && c != ';'
            }
            Dialect::Redshift => c.is_alphanumeric() || c == '_',
            Dialect::SingleStore => c.is_alphanumeric() || c == '_' || c == '$',
            _ => c.is_alphanumeric() || c == '_',
        }
    }

    fn parse_identifier_partial<'input>(
        &self,
        sql: &'input str,
    ) -> Result<(Identifier, &'input str), ParseError> {
        let (id, rest) = parse_identifier(
            sql,
            self.quote_char(),
            self.escape_char(),
            |c| self.is_valid_identifier_char(c),
            |c| self.is_escape_special_identifier_char(c),
            |c| self.unescape_identifier_char(c),
        )?;
        let id = match self {
            Dialect::Snowflake => {
                // In Snowflake, unquoted identifiers are normalized to
                // uppercase
                if sql.starts_with(self.quote_char()) {
                    id
                } else {
                    id.to_ascii_uppercase()
                }
            }
            _ => id,
        };
        Ok((Identifier::new(id), rest))
    }

    /// Parse the given string as a single identifier.
    pub fn parse_identifier(&self, sql: &str) -> Result<Identifier, ParseError> {
        let (id, rest) = self.parse_identifier_partial(sql)?;
        if !rest.is_empty() {
            return parse_err!("Failed to parse {sql}: unexpected input after identifier {rest}");
        }
        Ok(id)
    }

    /// Parse a catalog/project identifier from the start of the string,
    /// returning the identifier and the remaining unparsed input.
    ///
    /// For BigQuery, handles domain-scoped project ids where the domain
    /// contains dots and a colon separates it from the project id
    /// (e.g. "domain.co.uk:project-id"). For other dialects, delegates
    /// to `parse_identifier_partial`.
    pub fn parse_catalog_identifier_partial<'input>(
        &self,
        sql: &'input str,
    ) -> Result<(Identifier, &'input str), ParseError> {
        if !matches!(self, Dialect::Bigquery) || sql.starts_with(self.quote_char()) {
            return self.parse_identifier_partial(sql);
        }

        // Colons can't appear in unquoted BigQuery identifiers, so a colon
        // in the raw string means this is a domain-scoped project id
        // (e.g. "domain.co.uk:project-id").
        if let Some(colon_pos) = sql.find(':') {
            let domain_str = &sql[..colon_pos];
            let domain_parts = self
                .parse_dot_separated_identifiers(domain_str)
                .map_err(|e| ParseError::new(format!("Failed to parse domain in {sql}: {e}")))?;

            let after_colon = &sql[colon_pos + 1..];
            let (project_id, rest) = self.parse_identifier_partial(after_colon)?;

            let mut catalog = domain_parts
                .iter()
                .map(|id| id.to_value())
                .collect::<Vec<_>>()
                .join(".");
            catalog.push(':');
            catalog.push_str(&project_id.to_value());

            Ok((Identifier::new(catalog), rest))
        } else {
            self.parse_identifier_partial(sql)
        }
    }

    fn parse_dot_separated_identifiers_partial<'input>(
        &self,
        sql: &'input str,
    ) -> Result<(Vec<Identifier>, &'input str), ParseError> {
        let mut idents = vec![];
        let mut rest = sql;
        loop {
            let (id, new_rest) = self.parse_identifier_partial(rest)?;
            idents.push(id);
            match parse_dot(new_rest) {
                Ok(new_rest) => rest = new_rest,
                Err(_) => return Ok((idents, new_rest)),
            }
        }
    }

    /// Parse the given string as a sequence of dot-separated identifiers.
    pub fn parse_dot_separated_identifiers(
        &self,
        sql: &str,
    ) -> Result<Vec<Identifier>, ParseError> {
        let (idents, rest) = self.parse_dot_separated_identifiers_partial(sql)?;
        if !rest.is_empty() {
            return parse_err!("Failed to parse {sql}: unexpected input after identifier {rest}");
        }
        Ok(idents)
    }
}

/// Parse an identifier from the start of the given SQL string. The identifier
/// may be quoted using the specified quote character and escape character. If
/// successful, returns a pair consisting of the parsed identifier as a [String]
/// and a slice of any remaining unparsed input. Otherwise, returns an error.
fn parse_identifier<P, Q, R>(
    sql: &str,
    quote_char: char,
    escape_char: char,
    is_valid_identifier_char: P,
    is_escape_special_char: Q,
    unescaper: R,
) -> Result<(String, &str), ParseError>
where
    P: Fn(char) -> bool,
    Q: Fn(char) -> bool,
    R: Fn(char) -> char,
{
    let is_next_char_special = |chars: &mut std::iter::Peekable<std::str::CharIndices>| -> bool {
        match chars.peek() {
            Some((_, c)) => is_escape_special_char(*c),
            _ => false,
        }
    };

    let mut chars = sql.char_indices().peekable();

    let Some((_, c)) = chars.peek() else {
        // Empty string is not a syntactically valid identifier
        return parse_err!("Expecting identifier but got end of input");
    };

    let is_quoted = *c == quote_char;
    let mut res = String::with_capacity(32);
    let mut escaped = false;

    if is_quoted {
        chars.next();
        while let Some((i, c)) = chars.next() {
            match c {
                _ if c == escape_char && !escaped && is_next_char_special(&mut chars) => {
                    escaped = true;
                }
                _ if c == quote_char && !escaped => {
                    return Ok((res, &sql[i + 1..]));
                }
                _ if escaped => {
                    res.push(unescaper(c));
                    escaped = false;
                }
                _ => {
                    res.push(c);
                    escaped = false;
                }
            }
        }
    } else {
        for (i, c) in chars {
            if is_valid_identifier_char(c) {
                res.push(c);
            } else if res.is_empty() {
                return parse_err!("Expecting identifier but got {c:?}");
            } else {
                return Ok((res, &sql[i..]));
            }
        }
    }

    if is_quoted {
        parse_err!("Unterminated quoted identifier")
    } else {
        Ok((res, ""))
    }
}

/// Consumes a dot character (along with any surrounding whitespaces) from the
/// start of the given SQL string. If successful, returns a slice of any
/// remaining unparsed input. Otherwise, returns an error.
pub fn parse_dot(sql: &str) -> Result<&str, ParseError> {
    let mut chars = sql.char_indices().peekable();
    let consume_whitespaces = |chars: &mut std::iter::Peekable<std::str::CharIndices>| loop {
        match chars.peek() {
            Some((_, c)) if c.is_whitespace() => {
                chars.next();
            }
            _ => break,
        }
    };

    consume_whitespaces(&mut chars);
    let Some((_, c)) = chars.next() else {
        return parse_err!("expecting '.' but got end of input");
    };

    if c == '.' {
        consume_whitespaces(&mut chars);
        if let Some((i, _)) = chars.peek() {
            Ok(&sql[*i..])
        } else {
            Ok("")
        }
    } else {
        parse_err!("expecting '.' but got {c}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::IntoDeserializer;
    use serde::de::value::{Error as DeError, StrDeserializer};
    use strum::IntoEnumIterator;

    /// Every variant with its `Display` form and its lowercase serde alias.
    const VARIANTS: [(Dialect, &str, &str); 12] = [
        (Dialect::Sdf, "sdf", "sdf"),
        (Dialect::Trino, "trino", "trino"),
        (Dialect::Snowflake, "snowflake", "snowflake"),
        (Dialect::Postgresql, "postgresql", "postgresql"),
        (Dialect::Bigquery, "bigquery", "bigquery"),
        (Dialect::DataFusion, "datafusion", "datafusion"),
        (Dialect::SparkSql, "sparksql", "sparksql"),
        (Dialect::SparkLp, "spark-lp", "sparklp"),
        (Dialect::Redshift, "redshift", "redshift"),
        (Dialect::Databricks, "databricks", "databricks"),
        (Dialect::Duckdb, "duckdb", "duckdb"),
        (Dialect::SingleStore, "singlestore", "singlestore"),
    ];

    fn deserialize(s: &str) -> Result<Dialect, DeError> {
        let deserializer: StrDeserializer<'_, DeError> = s.into_deserializer();
        Dialect::deserialize(deserializer)
    }

    #[test]
    fn variants_table_is_exhaustive() {
        let listed: Vec<Dialect> = VARIANTS.iter().map(|(d, _, _)| *d).collect();
        assert_eq!(listed, Dialect::iter().collect::<Vec<_>>());
    }

    #[test]
    fn display_is_pinned() {
        // `Display` names asset directories in `sdf-compiler-assets` and the
        // output directories of `sdf-make-sql-functions`.
        for (dialect, display, _) in VARIANTS {
            assert_eq!(dialect.to_string(), display);
        }
    }

    #[test]
    fn deserializes_pascal_case_and_lowercase() {
        for (dialect, _, lowercase) in VARIANTS {
            assert_eq!(deserialize(&format!("{dialect:?}")).unwrap(), dialect);
            assert_eq!(deserialize(lowercase).unwrap(), dialect);
        }
        assert_eq!(deserialize("Presto").unwrap(), Dialect::Trino);
        assert_eq!(deserialize("presto").unwrap(), Dialect::Trino);
        assert!(deserialize("SNOWFLAKE").is_err());
        assert!(deserialize("spark-lp").is_err());
    }

    #[test]
    fn from_str_accepts_both_former_enums_strings() {
        let accepted = [
            ("sdf", Dialect::Sdf),
            ("trino", Dialect::Trino),
            ("presto", Dialect::Trino),
            ("passthrough", Dialect::Trino),
            ("snowflake", Dialect::Snowflake),
            ("postgresql", Dialect::Postgresql),
            ("postgres", Dialect::Postgresql),
            ("salesforce", Dialect::Postgresql),
            ("bigquery", Dialect::Bigquery),
            ("datafusion", Dialect::DataFusion),
            ("sparksql", Dialect::SparkSql),
            ("sparklp", Dialect::SparkLp),
            ("spark-lp", Dialect::SparkLp),
            ("redshift", Dialect::Redshift),
            ("databricks", Dialect::Databricks),
            ("duckdb", Dialect::Duckdb),
            ("singlestore", Dialect::SingleStore),
        ];
        for (input, dialect) in accepted {
            assert_eq!(input.parse::<Dialect>().unwrap(), dialect, "{input}");
            let upper = input.to_ascii_uppercase();
            assert_eq!(upper.parse::<Dialect>().unwrap(), dialect, "{upper}");
        }
        assert!("mysql".parse::<Dialect>().is_err());
    }

    #[test]
    fn from_str_round_trips_display() {
        for dialect in Dialect::iter() {
            assert_eq!(dialect.to_string().parse::<Dialect>().unwrap(), dialect);
        }
    }

    #[test]
    fn test_singlestore_dialect() {
        use std::str::FromStr;
        let d = Dialect::from_str("singlestore").unwrap();
        assert_eq!(d, Dialect::SingleStore);
        assert_eq!(d.to_string(), "singlestore");
        assert_eq!(d.quote_char(), '`');
        assert_eq!(d.escape_char(), '\\');
        assert_eq!(d.get_default_col(), "col");
        assert_eq!(d.get_default_col_start(), 1);
        assert!(d.is_valid_identifier_char('a'));
        assert!(d.is_valid_identifier_char('_'));
        assert!(d.is_valid_identifier_char('$'));
        assert!(!d.is_valid_identifier_char(' '));
    }
}
