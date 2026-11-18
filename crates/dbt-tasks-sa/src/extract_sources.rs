//! Extract upstream table references from a SQL statement using DataFusion `sqlparser`.
//!
//! Implemented by walking the `sqlparser` AST looking for `TableFactor::Table` nodes,
//! while filtering out any that match in-scope CTE aliases.

use std::collections::{BTreeSet, HashSet};

use core::ops::ControlFlow;
use dbt_adapter::AdapterType;
use dbt_frontend_common::{FullyQualifiedName, ident::Identifier};
use sqlparser::ast::{ObjectName, Query, TableFactor, Visit, Visitor};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::Token;

use crate::sql::dialect::sqlparser_dialect_for;

/// Errors returned by [`extract_sources_from_str`].
#[derive(Debug)]
pub enum ExtractSourcesError {
    /// `sqlparser` failed to parse the input.
    Parse(String),
}

impl std::fmt::Display for ExtractSourcesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(msg) => write!(f, "failed to parse SQL: {msg}"),
        }
    }
}

impl std::error::Error for ExtractSourcesError {}

/// Extract the upstream table references from `sql`. Names that are not
/// fully qualified are completed using `default_catalog`/`default_schema`.
///
/// References to in-scope CTE aliases are excluded from the result.
pub fn extract_sources_from_str(
    sql: &str,
    adapter_type: AdapterType,
    default_catalog: &str,
    default_schema: &str,
) -> Result<BTreeSet<FullyQualifiedName>, ExtractSourcesError> {
    let statements = Parser::parse_sql(sqlparser_dialect_for(adapter_type), sql)
        .map_err(|e| ExtractSourcesError::Parse(e.to_string()))?;

    let mut visitor = SourceExtractor {
        adapter_type,
        default_catalog,
        default_schema,
        cte_scopes: Vec::new(),
        sources: BTreeSet::new(),
    };
    let _ = statements.visit(&mut visitor);
    Ok(visitor.sources)
}

/// Extract upstream table references from a complete SQL expression rather
/// than a full statement. Custom materializations may wrap expression-only
/// model SQL in their own executable statement.
pub(crate) fn extract_sources_from_standalone_expression(
    sql: &str,
    adapter_type: AdapterType,
    default_catalog: &str,
    default_schema: &str,
) -> Result<BTreeSet<FullyQualifiedName>, ExtractSourcesError> {
    let mut parser = Parser::new(sqlparser_dialect_for(adapter_type))
        .try_with_sql(sql)
        .map_err(|e| ExtractSourcesError::Parse(e.to_string()))?;
    let expr = parser
        .parse_expr()
        .map_err(|e| ExtractSourcesError::Parse(e.to_string()))?;
    match parser.peek_token().token {
        Token::EOF => {}
        other => {
            return Err(ExtractSourcesError::Parse(format!(
                "expected end of expression, found {other}"
            )));
        }
    }
    let mut visitor = SourceExtractor {
        adapter_type,
        default_catalog,
        default_schema,
        cte_scopes: Vec::new(),
        sources: BTreeSet::new(),
    };
    let _ = expr.visit(&mut visitor);
    Ok(visitor.sources)
}

struct SourceExtractor<'a> {
    adapter_type: AdapterType,
    default_catalog: &'a str,
    default_schema: &'a str,
    /// Stack of CTE name sets, one per enclosing query scope. A name appears
    /// in `sources` only if it doesn't match a CTE in any enclosing scope.
    ///
    /// Names are stored as [`Identifier`]s so equality is case-insensitive,
    /// matching the existing extractor's behavior across dialects.
    cte_scopes: Vec<HashSet<Identifier>>,
    sources: BTreeSet<FullyQualifiedName>,
}

impl Visitor for SourceExtractor<'_> {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        // Register all CTE names defined by this query up-front, so any
        // reference to one (in any CTE body OR the main body) is filtered.
        //
        // This is equivalent to treating every WITH as if RECURSIVE — which
        // matches Snowflake's actual semantics and is the conservative call
        // for the rare shadowing case (a later CTE named the same as an
        // external table referenced from an earlier CTE body).
        let mut cte_names: HashSet<Identifier> = HashSet::new();
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                cte_names.insert(Identifier::new(cte.alias.name.value.as_str()));
            }
        }
        self.cte_scopes.push(cte_names);
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _query: &Query) -> ControlFlow<()> {
        self.cte_scopes.pop();
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, table_factor: &TableFactor) -> ControlFlow<()> {
        if let TableFactor::Table { name, .. } = table_factor {
            self.record_source(name);
        }
        ControlFlow::Continue(())
    }
}

impl SourceExtractor<'_> {
    fn record_source(&mut self, name: &ObjectName) {
        let parts = object_name_parts(name, self.adapter_type);
        if parts.is_empty() {
            return;
        }
        // Bare references that match an in-scope CTE are not upstreams.
        if parts.len() == 1 && self.cte_in_scope(&parts[0]) {
            return;
        }
        let fqn = resolve_fqn(&parts, self.default_catalog, self.default_schema);
        self.sources.insert(fqn);
    }

    fn cte_in_scope(&self, name: &str) -> bool {
        let id = Identifier::new(name);
        self.cte_scopes.iter().any(|scope| scope.contains(&id))
    }
}

/// Returns the string parts of a sqlparser `ObjectName`, applying
/// dialect-specific case folding to each part (see [`fold_identifier_case`]).
/// Falls back to splitting on `.` when the parser produced a single quoted
/// identifier containing dots (BigQuery's `` `proj.ds.tbl` `` is one such form).
fn object_name_parts(name: &ObjectName, adapter_type: AdapterType) -> Vec<String> {
    if should_split_dotted_quoted_ident(name, adapter_type) {
        // Treat the inner dotted parts as quoted (they were inside backticks),
        // matching BigQuery's behavior where backtick-quoted segments preserve
        // case verbatim.
        if let Some(only) = name.0.first().and_then(|p| p.as_ident()) {
            return only
                .value
                .split('.')
                .map(|s| fold_identifier_case(s, Some('`'), adapter_type))
                .collect();
        }
    }

    name.0
        .iter()
        .filter_map(|p| p.as_ident())
        .map(|i| fold_identifier_case(&i.value, i.quote_style, adapter_type))
        .collect()
}

/// Apply the dialect's case-folding rule to an identifier as it appears in
/// SQL.  Snowflake unquoted identifiers fold to ASCII uppercase; all other
/// dialects preserve case verbatim. Quoted identifiers always preserve case.
///
/// Kept narrow on purpose: extends only what's needed to match the existing
/// extractor's output, without touching the shared identifier-parsing path.
///
/// TODO: rely on functions from the adapter crates that perform this operation
fn fold_identifier_case(
    value: &str,
    quote_style: Option<char>,
    adapter_type: AdapterType,
) -> String {
    if quote_style.is_some() {
        return value.to_owned();
    }
    match adapter_type {
        AdapterType::Snowflake => value.to_ascii_uppercase(),
        _ => value.to_owned(),
    }
}

/// BigQuery permits `` `proj.ds.tbl` `` as a single backtick-quoted token whose
/// value contains dots; we split it for multi-part FQN construction. Limited
/// to that dialect/shape so we don't accidentally split legitimate
/// dot-containing identifiers in other dialects.
fn should_split_dotted_quoted_ident(name: &ObjectName, adapter_type: AdapterType) -> bool {
    use AdapterType::*;
    match adapter_type {
        Bigquery => {
            let Some(only) = name.0.first().and_then(|p| p.as_ident()) else {
                return false;
            };
            name.0.len() == 1 && only.quote_style.is_some() && only.value.contains('.')
        }
        _ => false,
    }
}

fn resolve_fqn(
    parts: &[String],
    default_catalog: &str,
    default_schema: &str,
) -> FullyQualifiedName {
    match parts.len() {
        1 => FullyQualifiedName::new(default_catalog, default_schema, &parts[0]),
        2 => FullyQualifiedName::new(default_catalog, &parts[0], &parts[1]),
        _ => {
            // 3 or more — take the last three parts. (4-part references like
            // BigQuery `region-us`.`information_schema`.`tables` qualified by a
            // project are uncommon; we keep the trailing three.)
            let n = parts.len();
            FullyQualifiedName::new(&parts[n - 3], &parts[n - 2], &parts[n - 1])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fqn(c: &str, s: &str, t: &str) -> FullyQualifiedName {
        FullyQualifiedName::new(c, s, t)
    }

    fn extract(
        sql: &str,
        adapter_type: AdapterType,
        default_catalog: &str,
        default_schema: &str,
    ) -> BTreeSet<FullyQualifiedName> {
        extract_sources_from_str(sql, adapter_type, default_catalog, default_schema).unwrap()
    }

    #[test]
    fn simple_bare_reference_gets_qualified() {
        let sources = extract(
            "CREATE TABLE x AS SELECT a FROM b",
            AdapterType::Trino,
            "datafusion",
            "public",
        );
        assert!(sources.contains(&fqn("datafusion", "public", "b")));
    }

    #[test]
    fn partial_qualification_uses_default_catalog() {
        let sources = extract(
            "SELECT a FROM b.c",
            AdapterType::Trino,
            "datafusion",
            "public",
        );
        assert!(sources.contains(&fqn("datafusion", "b", "c")));
    }

    #[test]
    fn fully_qualified_kept_as_is() {
        let sources = extract(
            "CREATE TABLE x AS SELECT a FROM b.c.d",
            AdapterType::Trino,
            "datafusion",
            "public",
        );
        assert!(sources.contains(&fqn("b", "c", "d")));
    }

    #[test]
    fn bigquery_backtick_multipart_splits() {
        let sources = extract(
            "CREATE TABLE x AS SELECT a FROM `b.c.d`",
            AdapterType::Bigquery,
            "datafusion",
            "public",
        );
        assert!(sources.contains(&fqn("b", "c", "d")));
    }

    #[test]
    fn cte_alias_not_upstream() {
        let sources = extract(
            "with a as (select * from b) select * from a",
            AdapterType::Trino,
            "datafusion",
            "public",
        );
        assert_eq!(sources.len(), 1);
        assert!(sources.contains(&fqn("datafusion", "public", "b")));
    }

    #[test]
    fn cte_alias_match_is_case_insensitive() {
        let sources = extract(
            "with A as (select * from b), C as (select * from a) select * from A,c",
            AdapterType::Trino,
            "datafusion",
            "public",
        );
        assert_eq!(sources.len(), 1);
        assert!(sources.contains(&fqn("datafusion", "public", "b")));
    }

    #[test]
    fn ctas_with_nested_ctes() {
        let sources = extract(
            "CREATE TABLE x AS with a as (select * from b), c as (select * from a, b) select * from c",
            AdapterType::Trino,
            "datafusion",
            "public",
        );
        assert_eq!(sources.len(), 1);
        assert!(sources.contains(&fqn("datafusion", "public", "b")));
    }

    #[test]
    fn subqueries_contribute_sources() {
        let sources = extract(
            "CREATE TABLE x AS SELECT j1_string, j2_string, \
             (SELECT count(*) FROM J1, J3) as c FROM j1, j2",
            AdapterType::Trino,
            "dataFusion",
            "PUBLIC",
        );
        assert_eq!(sources.len(), 3);
        assert!(sources.contains(&fqn("datafusion", "public", "j1")));
        assert!(sources.contains(&fqn("datafusion", "public", "j2")));
        assert!(sources.contains(&fqn("datafusion", "public", "j3")));
    }

    #[test]
    fn snowflake_implicit_recursive_cte_not_in_upstreams() {
        let sources = extract(
            "WITH r AS (
                SELECT 1 AS x FROM mydb.myschema.t
                UNION ALL
                SELECT r.x + 1 FROM r WHERE r.x < 10
             )
             SELECT * FROM r",
            AdapterType::Snowflake,
            "mydb",
            "myschema",
        );
        assert_eq!(sources.len(), 1);
        assert!(sources.contains(&fqn("mydb", "myschema", "t")));
    }

    #[test]
    fn duplicate_references_deduplicated() {
        let sources = extract(
            "select * from c union all select * from a.b.c",
            AdapterType::Snowflake,
            "a",
            "b",
        );
        assert_eq!(sources.len(), 1);
        assert!(sources.contains(&fqn("a", "b", "c")));
    }

    #[test]
    fn snowflake_unquoted_identifiers_fold_to_uppercase() {
        // Snowflake's parser normalizes unquoted identifiers to uppercase;
        // we mirror that so the downstream `BaseRelation::semantic_fqn()`
        // cache key matches.
        let sources = extract(
            "CREATE VIEW v AS SELECT * FROM upstream_t",
            AdapterType::Snowflake,
            "DB",
            "S",
        );
        assert_eq!(sources.len(), 1);
        let only = sources.into_iter().next().unwrap();
        assert_eq!(only.catalog().name(), "DB");
        assert_eq!(only.schema().name(), "S");
        assert_eq!(only.table().name(), "UPSTREAM_T");
    }

    #[test]
    fn snowflake_quoted_identifiers_preserve_case() {
        let sources = extract(
            r#"SELECT * FROM "mydb"."myschema"."mytbl""#,
            AdapterType::Snowflake,
            "DB",
            "S",
        );
        let only = sources.into_iter().next().unwrap();
        assert_eq!(only.catalog().name(), "mydb");
        assert_eq!(only.schema().name(), "myschema");
        assert_eq!(only.table().name(), "mytbl");
    }

    #[test]
    fn bigquery_unquoted_identifiers_preserve_case() {
        let sources = extract(
            "SELECT * FROM proj.ds.MyTbl",
            AdapterType::Bigquery,
            "datafusion",
            "public",
        );
        let only = sources.into_iter().next().unwrap();
        assert_eq!(only.catalog().name(), "proj");
        assert_eq!(only.schema().name(), "ds");
        assert_eq!(only.table().name(), "MyTbl");
    }

    #[test]
    fn create_view_yields_inner_query_sources() {
        let sources = extract(
            "CREATE OR REPLACE VIEW mydb.myschema.v AS SELECT * FROM mydb.myschema.t",
            AdapterType::Snowflake,
            "mydb",
            "myschema",
        );
        assert_eq!(sources.len(), 1);
        assert!(sources.contains(&fqn("mydb", "myschema", "t")));
    }

    #[test]
    fn standalone_literal_expression_is_valid() {
        let sources = extract_sources_from_standalone_expression(
            "'alice'",
            AdapterType::Snowflake,
            "db",
            "s",
        )
        .unwrap();
        assert!(sources.is_empty());
    }

    #[test]
    fn standalone_subquery_expression_extracts_sources() {
        let sources = extract_sources_from_standalone_expression(
            "(select max(id) from raw.users)",
            AdapterType::Snowflake,
            "db",
            "s",
        )
        .unwrap();
        assert_eq!(sources.len(), 1);
        assert!(sources.contains(&fqn("db", "RAW", "USERS")));
    }

    #[test]
    fn standalone_expression_must_consume_full_input() {
        assert!(
            extract_sources_from_standalone_expression(
                "'alice' from raw.users",
                AdapterType::Snowflake,
                "db",
                "s",
            )
            .is_err()
        );
    }
}
