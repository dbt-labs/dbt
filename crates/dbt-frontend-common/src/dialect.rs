use crate::ident::{
    ColumnRef, FullyQualifiedName, LOWERCASE_DRAFT_SUFFIX, QualifiedName, UPPERCASE_DRAFT_SUFFIX,
};

use super::error::{InternalError, InternalResult, internal_err};
use super::ident::Identifier;
use crate::make_internal_err;
use serde::{Deserialize, Serialize};
use std::{fmt::Display, str::FromStr};

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
pub enum Dialect {
    Sdf,
    #[default]
    #[serde(alias = "Presto")]
    Trino,
    Snowflake,
    Postgresql,
    Bigquery,
    DataFusion,
    SparkSql,
    SparkLp,
    Redshift,
    Databricks,
    Duckdb,
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
        }
    }
}

impl FromStr for Dialect {
    type Err = Box<InternalError>;

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

            // "passthrough" adapter type is used to disable most local semantic
            // analysis, so we just map it to the default dialect.
            "passthrough" => Ok(Default::default()),

            _ => internal_err!("Invalid dialect value: '{}'", input),
        }
    }
}

// Miscellaneous dialect-specific functions
impl Dialect {
    pub const fn max_value() -> u8 {
        Dialect::Duckdb as u8
    }

    pub fn is_default(&self) -> bool {
        matches!(self, Dialect::Trino)
    }

    /// The default file extension for this dialect.
    pub fn extension(&self) -> String {
        match self {
            Dialect::SparkLp => "json".to_owned(),
            _ => "sql".to_owned(),
        }
    }

    pub fn draft_suffix(&self) -> &str {
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
            Dialect::Databricks | Dialect::Duckdb => "col".to_string(),
            _ => todo!("get_default_col not implemented for {self}"),
        }
    }

    pub fn get_default_col_start(&self) -> usize {
        match self {
            Dialect::Snowflake | Dialect::Trino | Dialect::Redshift => 0,
            Dialect::Bigquery | Dialect::Databricks | Dialect::Duckdb => 1,
            _ => todo!("get_default_col_start not implemented for {self}"),
        }
    }
}

impl From<&Dialect> for Dialect {
    fn from(value: &Dialect) -> Self {
        *value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeFormattingContext {
    Production,
    Slt,
}

// Parsing: this implements a fast identifier/fqn parser that doesn't rely on
// Antlr. O(n) time with guaranteed O(1) allocations.
impl Dialect {
    /// The character used to quote identifiers in this dialect.
    pub const fn quote_char(&self) -> char {
        match self {
            Dialect::Sdf | Dialect::Trino => '"',
            Dialect::Bigquery | Dialect::Databricks => '`',
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
            Dialect::Bigquery => '\\',
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
            _ => c.is_alphanumeric() || c == '_',
        }
    }

    fn parse_identifier_partial<'input>(
        &self,
        sql: &'input str,
    ) -> InternalResult<(Identifier, &'input str)> {
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
    pub fn parse_identifier(&self, sql: &str) -> InternalResult<Identifier> {
        let (id, rest) = self.parse_identifier_partial(sql)?;
        if !rest.is_empty() {
            return internal_err!(
                "Failed to parse {sql}: unexpected input after identifier {rest}"
            );
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
    fn parse_catalog_identifier_partial<'input>(
        &self,
        sql: &'input str,
    ) -> InternalResult<(Identifier, &'input str)> {
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
                .map_err(|e| make_internal_err!("Failed to parse domain in {sql}: {e}"))?;

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
    ) -> InternalResult<(Vec<Identifier>, &'input str)> {
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
    pub fn parse_dot_separated_identifiers(&self, sql: &str) -> InternalResult<Vec<Identifier>> {
        let (idents, rest) = self.parse_dot_separated_identifiers_partial(sql)?;
        if !rest.is_empty() {
            return internal_err!(
                "Failed to parse {sql}: unexpected input after identifier {rest}"
            );
        }
        Ok(idents)
    }

    /// Parse the given string as a qualified name.
    pub fn parse_qualified_name(&self, sql: &str) -> InternalResult<QualifiedName> {
        let (first, rest) = self
            .parse_catalog_identifier_partial(sql)
            .map_err(|e| make_internal_err!("Failed to parse {sql} as qualified name: {e}"))?;

        let mut idents = vec![first];

        if !rest.is_empty() {
            let rest_after_dot = parse_dot(rest)
                .map_err(|e| make_internal_err!("Failed to parse {sql} as qualified name: {e}"))?;
            let remaining = self
                .parse_dot_separated_identifiers(rest_after_dot)
                .map_err(|e| make_internal_err!("Failed to parse {sql} as qualified name: {e}"))?;
            idents.extend(remaining);
        }

        // BigQuery INFORMATION_SCHEMA queries can be region-qualified, producing
        // 4 idents: [catalog, region, "information_schema", table]. Collapse the
        // region into the catalog so we get 3.
        if matches!(self, Dialect::Bigquery)
            && idents.len() == 4
            && idents[2].matches("information_schema")
        {
            let database = format!("{}.{}", idents[0].to_value(), idents[1].to_value());
            idents = vec![
                Identifier::new(database),
                idents[2].clone(),
                idents[3].clone(),
            ];
        }

        QualifiedName::try_from(idents)
    }

    /// Parse the given string as a fully qualified name.
    pub fn parse_fqn(&self, sql: &str) -> InternalResult<FullyQualifiedName> {
        let qn = self.parse_qualified_name(sql)?;
        qn.try_into()
    }

    /// Parse the given string as a column reference.
    pub fn parse_column_ref(&self, sql: &str) -> InternalResult<ColumnRef> {
        let idvec = self
            .parse_dot_separated_identifiers(sql)
            .map_err(|e| make_internal_err!("Failed to parse {sql} as column reference: {e}"))?;
        if idvec.len() != 4 {
            return internal_err!(
                "Failed to parse {sql} as column reference:
                 expecting exactly 4 dot-separated components but got {}",
                idvec.len()
            );
        }
        ColumnRef::try_from(idvec)
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
) -> InternalResult<(String, &str)>
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
        return internal_err!("Expecting identifier but got end of input");
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
                return internal_err!("Expecting identifier but got {c:?}");
            } else {
                return Ok((res, &sql[i..]));
            }
        }
    }

    if is_quoted {
        internal_err!("Unterminated quoted identifier")
    } else {
        Ok((res, ""))
    }
}

/// Consumes a dot character (along with any surrounding whitespaces) from the
/// start of the given SQL string. If successful, returns a slice of any
/// remaining unparsed input. Otherwise, returns an error.
fn parse_dot(sql: &str) -> InternalResult<&str> {
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
        return internal_err!("expecting '.' but got end of input");
    };

    if c == '.' {
        consume_whitespaces(&mut chars);
        if let Some((i, _)) = chars.peek() {
            Ok(&sql[*i..])
        } else {
            Ok("")
        }
    } else {
        internal_err!("expecting '.' but got {c}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_adapter_core::AdapterType;
    use strum::IntoEnumIterator;

    /// Replicates the `dialect_of()` mapping from `dbt-common` so we can
    /// test the invariant without a circular dependency.
    fn dialect_of(adapter_type: AdapterType) -> Option<Dialect> {
        use AdapterType::*;
        let dialect = match adapter_type {
            Postgres => Dialect::Postgresql,
            Snowflake => Dialect::Snowflake,
            Bigquery => Dialect::Bigquery,
            Databricks | Spark => Dialect::Databricks,
            Redshift => Dialect::Redshift,
            Salesforce => Dialect::Postgresql,
            DuckDB => Dialect::Duckdb,
            Trino => Dialect::Trino,
            _ => return None,
        };
        Some(dialect)
    }

    fn adapter_type_to_string_via_dialect(adapter_type: AdapterType) -> String {
        dialect_of(adapter_type)
            .map(|dialect| dialect.to_string())
            .unwrap_or_else(|| adapter_type.to_string())
    }

    #[test]
    fn adapter_type_to_string_via_dialect_matches_to_string() {
        for adapter_type in AdapterType::iter() {
            if adapter_type == AdapterType::Spark {
                // no good dialect mapping for Spark, so we skip the invariant check for it
                continue;
            }
            if matches!(
                adapter_type,
                AdapterType::Postgres | AdapterType::Salesforce
            ) {
                // Postgres serializes as "postgres" but its dialect as "postgresql";
                // Salesforce maps to Dialect::Postgresql so it diverges by design.
                continue;
            }
            assert_eq!(
                adapter_type_to_string_via_dialect(adapter_type),
                adapter_type.to_string(),
                "adapter_type_to_string_via_dialect() diverges from \
                 AdapterType::to_string() for {adapter_type:?}",
            );
        }
    }
}
