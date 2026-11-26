//! Snowflake named stages as dbt State dependencies.
//!
//! A model that reads a named stage depends on the files the stage reference
//! reads. The dependency is named by the normalized stage reference, and its
//! last modified time is that of the newest file `LIST` returns.
//!
//! The dbt State service extracts the same dependency from the model's SQL,
//! so the name built here must be byte-for-byte the name it builds: the stage
//! name qualified with the defaults and normalized like a table name, then the
//! path (case-sensitive) and the `PATTERN`, rendered as Snowflake SQL. For
//! example `@raw.stg/2024/ (PATTERN => '.*csv')` with default database `DB` is
//! `@"DB"."RAW"."STG"/2024/ (PATTERN => '.*csv')`.

use std::collections::{BTreeMap, BTreeSet};

use dbt_adapter::AdapterType;
use dbt_adapter::metadata::StageListing;
use dbt_adapter_sql::ident::snowflake::Ident;
use dbt_adapter_sql::join_dotted;
use dbt_common::sources_extractor::ExtractedUpstreams;
use dbt_frontend_common::stage_ref::StageReference;

/// A named stage a model reads, as a dbt State dependency.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StageDependency {
    /// The dependency name, for example
    /// `@"DB"."RAW"."STG"/2024/ (PATTERN => '.*csv')`.
    pub name: String,
    /// The `LIST` statement that returns the files the reference reads, for
    /// example `LIST @"DB"."RAW"."STG"/2024/ PATTERN = '.*csv'`.
    pub list_sql: String,
}

impl StageDependency {
    pub fn listing(&self) -> StageListing {
        StageListing {
            name: self.name.clone(),
            list_sql: self.list_sql.clone(),
        }
    }
}

/// The stages in `upstreams`, keyed by dependency name. Unqualified stage
/// names are qualified with `default_catalog` and `default_schema`.
///
/// Adds to `unsupported_references` the stages that can't be checked for
/// updates: user (`@~`) and table (`@%t`) stages, and every stage when the
/// extractor did not report stage details.
pub fn stage_dependencies(
    adapter_type: AdapterType,
    upstreams: &ExtractedUpstreams,
    default_catalog: &str,
    default_schema: &str,
    unsupported_references: &mut BTreeSet<String>,
) -> BTreeMap<String, StageDependency> {
    let mut stages = BTreeMap::new();
    if adapter_type != AdapterType::Snowflake {
        return stages;
    }
    if upstreams.stages.is_empty() {
        // An extractor without stage details reports stages only as tables
        // named by their location.
        unsupported_references.extend(
            upstreams
                .tables
                .iter()
                .map(|table| table.table().name())
                .filter(|name| name.starts_with('@'))
                .map(str::to_string),
        );
        return stages;
    }
    for stage in &upstreams.stages {
        match normalize_stage_reference(stage, default_catalog, default_schema) {
            Some(dependency) => {
                stages.insert(dependency.name.clone(), dependency);
            }
            None => {
                unsupported_references.insert(stage.location.clone());
            }
        }
    }
    stages
}

/// Normalizes a Snowflake named stage reference into its dependency.
///
/// The stage name is qualified with `default_catalog` and `default_schema`
/// and normalized like a table name: unquoted parts are uppercased, and every
/// part is quoted. The path (case-sensitive) and the pattern are kept. The
/// location is written as a string only when the path needs it, for example
/// `'@"DB"."SCH"."STG"/my folder/'`.
///
/// Returns `None` if `stage` is not a named stage: a user stage (`@~`), a
/// table stage (`@%t`), or a name that is not a valid identifier.
pub fn normalize_stage_reference(
    stage: &StageReference,
    default_catalog: &str,
    default_schema: &str,
) -> Option<StageDependency> {
    let location = stage.location.strip_prefix('@')?;
    if location.starts_with(['~', '%']) {
        return None;
    }

    // The stage name ends at the first "/" outside double quotes
    let mut in_quotes = false;
    let name_end = location
        .char_indices()
        .find(|&(_, c)| {
            if c == '"' {
                in_quotes = !in_quotes;
            }
            c == '/' && !in_quotes
        })
        .map_or(location.len(), |(i, _)| i);
    let (name, path) = location.split_at(name_end);

    let mut parts = parse_name(name).or_else(|| parse_name_leniently(name))?;
    let table = parts.pop()?.filter(|table| !table.is_empty())?;
    let mut schema = parts.pop().flatten().filter(|schema| !schema.is_empty());
    let mut catalog = parts.pop().flatten().filter(|catalog| !catalog.is_empty());
    if schema.is_none() && !default_schema.is_empty() {
        schema = Some(parse_identifier(default_schema));
    }
    if schema.is_some() && catalog.is_none() {
        catalog = Some(parse_identifier(default_catalog));
    }
    // Parts before the database, if any, are kept as they are
    let qualified = parts
        .into_iter()
        .map(Option::unwrap_or_default)
        .chain(catalog)
        .chain(schema)
        .chain(std::iter::once(table))
        .map(Ident::from_unquoted)
        .collect::<Vec<_>>();
    let qualified = join_dotted(qualified.iter());

    let location = format!("@{qualified}{path}");
    let location_sql = if path_needs_quotes(path) {
        string_literal(&location)
    } else {
        location
    };
    let pattern_sql = stage.pattern.as_deref().map(string_literal);
    Some(StageDependency {
        name: match &pattern_sql {
            Some(pattern) => format!("{location_sql} (PATTERN => {pattern})"),
            None => location_sql.clone(),
        },
        list_sql: match &pattern_sql {
            Some(pattern) => format!("LIST {location_sql} PATTERN = {pattern}"),
            None => format!("LIST {location_sql}"),
        },
    })
}

/// Splits a stage name into its dot-separated parts, each normalized: a
/// quoted part loses its quotes, an unquoted part is uppercased. An empty part
/// (`db..stg`) is `Some("")`. Returns `None` if a part is not an identifier.
fn parse_name(name: &str) -> Option<Vec<Option<String>>> {
    let mut parts = Vec::new();
    let mut rest = name;
    loop {
        let (part, after) = if let Some(quoted) = rest.strip_prefix('"') {
            let (value, len) = unquote(quoted)?;
            (Some(value), &quoted[len..])
        } else {
            let end = rest.find('.').unwrap_or(rest.len());
            let unquoted = &rest[..end];
            if unquoted.is_empty() {
                (Some(String::new()), &rest[end..])
            } else if is_unquoted_identifier(unquoted) {
                (Some(unquoted.to_uppercase()), &rest[end..])
            } else {
                return None;
            }
        };
        parts.push(part);
        match after.strip_prefix('.') {
            Some(next) => rest = next,
            None if after.is_empty() => return Some(parts),
            None => return None,
        }
    }
}

/// Splits a stage name that [`parse_name`] rejects, such as `my-stage`, the
/// way the dbt State service does: on every dot, into at most three parts. A
/// part that is a simple identifier is uppercased; any other part is taken as
/// written. Returns `None` for more than three parts.
fn parse_name_leniently(name: &str) -> Option<Vec<Option<String>>> {
    let parts = name.split('.').collect::<Vec<_>>();
    if parts.len() > 3 {
        return None;
    }
    Some(
        parts
            .into_iter()
            .map(|part| {
                // Narrower than `is_unquoted_identifier` on purpose: the
                // service's fallback treats only an ASCII letter or `_`
                // followed by word characters as unquoted
                let mut chars = part.chars();
                let simple = chars
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                    && chars.all(|c| c.is_alphanumeric() || c == '_');
                Some(if simple {
                    part.to_uppercase()
                } else {
                    part.to_string()
                })
            })
            .collect(),
    )
}

/// The value of a quoted identifier whose opening quote is already consumed,
/// and the length of `text` it took, closing quote included.
fn unquote(text: &str) -> Option<(String, usize)> {
    let mut value = String::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '"' {
            if chars.peek().is_some_and(|&(_, next)| next == '"') {
                chars.next();
                value.push('"');
            } else {
                return Some((value, i + 1));
            }
        } else {
            value.push(c);
        }
    }
    None
}

fn is_unquoted_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

/// The value of a configured database or schema name: a quoted identifier
/// loses its quotes, an unquoted one is uppercased, and anything else (such
/// as `my-db`) is taken as written.
fn parse_identifier(text: &str) -> String {
    if let Some(quoted) = text.strip_prefix('"')
        && let Some((value, len)) = unquote(quoted)
        && len == quoted.len()
    {
        return value;
    }
    if is_unquoted_identifier(text) {
        text.to_uppercase()
    } else {
        text.to_string()
    }
}

/// Whether the path can't be written without quotes: it has whitespace, a
/// comma, a parenthesis, a quote, or a comment start.
fn path_needs_quotes(path: &str) -> bool {
    path.contains("--")
        || path.contains("/*")
        || path.chars().any(|c| {
            // Python's `\s`, which also matches the ASCII separators
            c.is_whitespace()
                || ('\x1c'..='\x1f').contains(&c)
                || matches!(c, ',' | '(' | ')' | '\'')
        })
}

/// `value` as a single-quoted Snowflake string, with backslash escapes for
/// quotes, backslashes and control characters.
fn string_literal(value: &str) -> String {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('\'');
    for c in value.chars() {
        match c {
            '\'' => literal.push_str("\\'"),
            '\\' => literal.push_str("\\\\"),
            '\x08' => literal.push_str("\\b"),
            '\x0c' => literal.push_str("\\f"),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            '\x00'..='\x1f' | '\x7f' => literal.push_str(&format!("\\x{:02x}", c as u32)),
            _ => literal.push(c),
        }
    }
    literal.push('\'');
    literal
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_frontend_common::FullyQualifiedName;
    use dbt_frontend_common::named_reference::NamedReference;

    fn stage(location: &str, pattern: Option<&str>) -> StageReference {
        StageReference {
            location: location.to_string(),
            pattern: pattern.map(str::to_string),
        }
    }

    fn name(location: &str, pattern: Option<&str>) -> Option<String> {
        normalize_stage_reference(&stage(location, pattern), "db", "sch").map(|stage| stage.name)
    }

    /// Each case's expected name is what the dbt State service's
    /// `normalize_stage_reference` returns for the same reference, with
    /// default database `db` and schema `sch`.
    #[test]
    fn names_match_the_service() {
        for (location, pattern, expected) in [
            // Every spelling of the same stage gives the same name
            ("@db.sch.stg", None, Some(r#"@"DB"."SCH"."STG""#)),
            (r#"@"DB"."SCH"."STG""#, None, Some(r#"@"DB"."SCH"."STG""#)),
            ("@sch.stg", None, Some(r#"@"DB"."SCH"."STG""#)),
            ("@stg", None, Some(r#"@"DB"."SCH"."STG""#)),
            ("@db..stg", None, Some(r#"@"DB"."SCH"."STG""#)),
            ("@Stg_1", None, Some(r#"@"DB"."SCH"."STG_1""#)),
            ("@_stg", None, Some(r#"@"DB"."SCH"."_STG""#)),
            // The path keeps its case, and the pattern is kept
            (
                "@stg/Year=2024/",
                Some(".*csv"),
                Some(r#"@"DB"."SCH"."STG"/Year=2024/ (PATTERN => '.*csv')"#),
            ),
            (
                "@stg/a-b/c=d/x.csv",
                None,
                Some(r#"@"DB"."SCH"."STG"/a-b/c=d/x.csv"#),
            ),
            ("@stg/", None, Some(r#"@"DB"."SCH"."STG"/"#)),
            ("@stg/über/", None, Some(r#"@"DB"."SCH"."STG"/über/"#)),
            (r"@stg/x\y/", None, Some(r#"@"DB"."SCH"."STG"/x\y/"#)),
            (
                "@db.sch.stg",
                Some(""),
                Some(r#"@"DB"."SCH"."STG" (PATTERN => '')"#),
            ),
            // Quoted identifiers keep their case, a "/" inside quotes is part of the name
            (r#"@"Db"."a/b".stg/p"#, None, Some(r#"@"Db"."a/b"."STG"/p"#)),
            (
                r#"@"my stage"/x"#,
                None,
                Some(r#"@"DB"."SCH"."my stage"/x"#),
            ),
            (
                r#"@"My""Stage"/x"#,
                None,
                Some(r#"@"DB"."SCH"."My""Stage"/x"#),
            ),
            (r#"@"q".s/x"#, None, Some(r#"@"DB"."q"."S"/x"#)),
            ("@a.b.c.d", None, Some(r#"@"A"."B"."C"."D""#)),
            // A path that can't be unquoted keeps its quotes
            (
                "@stg/my folder/",
                None,
                Some(r#"'@"DB"."SCH"."STG"/my folder/'"#),
            ),
            ("@stg/it's/", None, Some(r#"'@"DB"."SCH"."STG"/it\'s/'"#)),
            ("@stg/a,b/", None, Some(r#"'@"DB"."SCH"."STG"/a,b/'"#)),
            ("@stg/a(b)/", None, Some(r#"'@"DB"."SCH"."STG"/a(b)/'"#)),
            ("@stg/a--b/", None, Some(r#"'@"DB"."SCH"."STG"/a--b/'"#)),
            ("@stg/a/*b/", None, Some(r#"'@"DB"."SCH"."STG"/a/*b/'"#)),
            (
                "@stg/tab\there/",
                None,
                Some(r#"'@"DB"."SCH"."STG"/tab\there/'"#),
            ),
            ("@stg/a\nb/", None, Some(r#"'@"DB"."SCH"."STG"/a\nb/'"#)),
            (
                "@stg/a b/",
                Some(".*"),
                Some(r#"'@"DB"."SCH"."STG"/a b/' (PATTERN => '.*')"#),
            ),
            // Patterns are written with backslash escapes
            (
                "@stg",
                Some("a'b"),
                Some(r#"@"DB"."SCH"."STG" (PATTERN => 'a\'b')"#),
            ),
            (
                "@stg",
                Some(r"a\.csv"),
                Some(r#"@"DB"."SCH"."STG" (PATTERN => 'a\\.csv')"#),
            ),
            (
                "@stg",
                Some(".*\n"),
                Some(r#"@"DB"."SCH"."STG" (PATTERN => '.*\n')"#),
            ),
            // Not a named stage
            ("@~/f.csv", None, None),
            ("@%tbl", None, None),
            ("@", None, None),
            ("@/x", None, None),
            // A name that isn't an identifier keeps its case
            ("@my-stage", None, Some(r#"@"DB"."SCH"."my-stage""#)),
            ("@db.my-stage", None, Some(r#"@"DB"."DB"."my-stage""#)),
            ("@my-db.sch.stg", None, Some(r#"@"my-db"."SCH"."STG""#)),
            ("@A1.2B", None, Some(r#"@"DB"."A1"."2B""#)),
            ("@x=y", None, Some(r#"@"DB"."SCH"."x=y""#)),
            ("@stg$1", None, Some(r#"@"DB"."SCH"."STG$1""#)),
            ("@Ünï", None, Some(r#"@"DB"."SCH"."ÜNÏ""#)),
            ("@.stg", None, Some(r#"@"DB"."SCH"."STG""#)),
            ("@stg.", None, None),
            ("stg", None, None),
        ] {
            assert_eq!(
                name(location, pattern).as_deref(),
                expected,
                "{location} {pattern:?}"
            );
        }
    }

    #[test]
    fn defaults_are_normalized_like_identifiers() {
        for (catalog, schema, expected) in [
            ("MyDb", "sch", r#"@"MYDB"."SCH"."STG""#),
            (r#""MyDb""#, r#""My Sch""#, r#"@"MyDb"."My Sch"."STG""#),
            ("my-db", "sch", r#"@"my-db"."SCH"."STG""#),
            ("db1", "my sch", r#"@"DB1"."my sch"."STG""#),
        ] {
            let stage = normalize_stage_reference(&stage("@stg", None), catalog, schema).unwrap();
            assert_eq!(stage.name, expected, "{catalog} {schema}");
        }
    }

    #[test]
    fn list_statement_reads_the_same_files() {
        let with_pattern =
            normalize_stage_reference(&stage("@s.stg/2024/", Some(".*csv")), "db", "sch").unwrap();
        assert_eq!(
            with_pattern.list_sql,
            r#"LIST @"DB"."S"."STG"/2024/ PATTERN = '.*csv'"#
        );

        let quoted =
            normalize_stage_reference(&stage("@s.stg/my folder/", None), "db", "sch").unwrap();
        assert_eq!(quoted.list_sql, r#"LIST '@"DB"."S"."STG"/my folder/'"#);
    }

    fn upstreams(tables: &[&str], stages: Vec<StageReference>) -> ExtractedUpstreams {
        ExtractedUpstreams {
            tables: tables
                .iter()
                .map(|table| {
                    NamedReference::new(FullyQualifiedName::new("DB", "SCH", table), false)
                })
                .collect(),
            stages: stages.into_iter().collect(),
            ..Default::default()
        }
    }

    #[test]
    fn stage_dependencies_keep_named_stages() {
        let mut unsupported = BTreeSet::new();
        let found = upstreams(
            &["@s.stg", "@~/f.csv", "T"],
            vec![stage("@s.stg", Some("a")), stage("@~/f.csv", None)],
        );

        let stages = stage_dependencies(
            AdapterType::Snowflake,
            &found,
            "db",
            "sch",
            &mut unsupported,
        );

        assert_eq!(
            stages.keys().collect::<Vec<_>>(),
            [r#"@"DB"."S"."STG" (PATTERN => 'a')"#]
        );
        assert_eq!(unsupported, BTreeSet::from(["@~/f.csv".to_string()]));
    }

    #[test]
    fn stage_dependencies_without_stage_details_are_unsupported() {
        let mut unsupported = BTreeSet::new();
        let found = upstreams(&["@s.stg", "T"], vec![]);

        let stages = stage_dependencies(
            AdapterType::Snowflake,
            &found,
            "db",
            "sch",
            &mut unsupported,
        );

        assert!(stages.is_empty());
        assert_eq!(unsupported, BTreeSet::from(["@s.stg".to_string()]));
    }

    #[test]
    fn stage_dependencies_are_snowflake_only() {
        let mut unsupported = BTreeSet::new();
        let found = upstreams(&["@s.stg"], vec![stage("@s.stg", None)]);

        let stages = stage_dependencies(
            AdapterType::Databricks,
            &found,
            "db",
            "sch",
            &mut unsupported,
        );

        assert!(stages.is_empty());
        assert!(unsupported.is_empty());
    }
}
