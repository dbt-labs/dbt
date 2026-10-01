use crate::{
    Dialect, Qualified, QualifiedName,
    ident::{Ident, Identifier},
};
use counter::Counter;
use itertools::Itertools;
use regex::Regex;
use std::sync::{Arc, LazyLock};

/// Represents a candidate name in a reference context. Used to format error
/// messages.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NameCandidate {
    pub name: Identifier,
    pub qualifier: Option<QualifiedName>,
}

impl NameCandidate {
    pub fn new(name: impl Into<Identifier>, qualifier: Vec<String>) -> Self {
        NameCandidate {
            name: name.into(),
            qualifier: QualifiedName::try_from(strip_internal_components(qualifier)).ok(),
        }
    }
}

impl
    From<(
        Option<&datafusion_common::TableReference>,
        &Arc<arrow_schema::Field>,
    )> for NameCandidate
{
    fn from(
        (qualifier, field): (
            Option<&datafusion_common::TableReference>,
            &Arc<arrow_schema::Field>,
        ),
    ) -> Self {
        NameCandidate::new(
            field.name().to_string(),
            qualifier
                .into_iter()
                .flat_map(|q| q.to_vec())
                .collect::<Vec<_>>(),
        )
    }
}

impl From<String> for NameCandidate {
    fn from(name: String) -> Self {
        NameCandidate {
            name: name.into(),
            qualifier: None,
        }
    }
}

impl std::fmt::Display for NameCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.qualifier {
            None => write!(f, "{}", format_ident_display(&self.name)),
            Some(Qualified::Bare { table, .. }) => {
                write!(
                    f,
                    "{}.{}",
                    format_ident_display(table),
                    format_ident_display(&self.name)
                )
            }
            Some(Qualified::Partial { schema, table }) => {
                write!(
                    f,
                    "{}.{}.{}",
                    format_ident_display(schema),
                    format_ident_display(table),
                    format_ident_display(&self.name)
                )
            }
            Some(Qualified::Full {
                catalog,
                schema,
                table,
            }) => {
                write!(
                    f,
                    "{}.{}.{}.{}",
                    format_ident_display(catalog),
                    format_ident_display(schema),
                    format_ident_display(table),
                    format_ident_display(&self.name)
                )
            }
        }
    }
}

pub fn format_candidates(
    candidates: Vec<NameCandidate>,
    target: &str,
    limit: Option<usize>,
) -> String {
    let candidates = candidates.into_iter().collect::<Counter<_>>();
    let target = target.to_ascii_lowercase();
    let truncated = if let Some(limit) = limit {
        if candidates.len() > limit {
            Some(candidates.len() - limit)
        } else {
            None
        }
    } else {
        None
    };

    let mut candidates = candidates
        .iter()
        .sorted_by_cached_key(|(c, _)| {
            (
                stringmetrics::levenshtein(c.name.to_ascii_lowercase().as_str(), target.as_str()),
                *c,
            )
        })
        .map(|(c, n)| {
            if *n > 1 {
                format!("{c} (x{n})")
            } else {
                c.to_string()
            }
        });
    let candidates = if let Some(limit) = limit {
        candidates.take(limit).join(", ")
    } else {
        candidates.join(", ")
    };

    if let Some(truncated) = truncated {
        format!("Available are {candidates} ..(and {truncated} more)")
    } else {
        format!("Available are {candidates}")
    }
}

fn strip_internal_components(mut qualifier: Vec<String>) -> Vec<Identifier> {
    static INTERNAL_QUALIFIER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        // TODO: clean up our internal naming conventions
        Regex::new(r"^_\d+$|^__sdf_|^_sdf::").expect("Failed to compile regex")
    });

    // Strip the first component if it matches the internal name regex
    if let Some(first) = qualifier.first()
        && INTERNAL_QUALIFIER_REGEX.is_match(first)
    {
        qualifier.remove(0);
    }

    qualifier.into_iter().map(|s| s.into()).collect::<Vec<_>>()
}

/// Formats an identifier for display, escaping it if necessary.
///
/// Note: This function is only suitable for the purpose of presenting the
/// identifier in a user-friendly format, for example in error messages. It is
/// not guaranteed to be produce valid SQL syntax, and thus should not be used
/// for the purpose of generating SQL queries.
pub fn format_ident_display(id: &Ident<'_>) -> String {
    let dialect = Dialect::default();
    if need_quotes_approx(id, &dialect) {
        format!(
            "{quote}{name}{quote}",
            quote = dialect.quote_char(),
            name = dialect.escape_identifier(id.name())
        )
    } else {
        id.name().to_string()
    }
}

fn need_quotes_approx(id: &Ident<'_>, dialect: &Dialect) -> bool {
    // Empty identifiers have to be quoted
    id.is_empty()
        // If the first character is not alphabetic, the identifier has to be quoted
        || !id.name().chars().next().is_some_and( |c| c=='_' || c.is_ascii_alphabetic())
        // Invalid characters has to be quoted
        || id.name().chars().any(|c| !dialect.is_valid_identifier_char(c)
            // BigQuery allows hyphens in unquoted identifiers in certain
            // contexts (e.g. table names), but we still quote them here
            || (matches!(dialect, &Dialect::Bigquery) && c == '-'))
        // In Snowflake, unquoted identifiers are normalized to uppercase,
        // therefore if the identifier contains any lowercase characters, it
        // needs to be quoted to preserve the original casing.
        || (dialect == &Dialect::Snowflake && id.name().chars().any(|c| c.is_ascii_lowercase()))
        // In Redshift, unquoted identifiers are normalized to lowercase,
        // therefore if the identifier contains any uppercase characters, it
        // needs to be quoted to preserve the original casing.
        || (dialect == &Dialect::Redshift && id.name().chars().any(|c| c.is_ascii_uppercase()))
}
