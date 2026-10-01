//! Parsing of `DESCRIBE TABLE EXTENDED ... AS JSON` metadata for Databricks.
//!
//! This module is the Rust home of the parsers and gating predicate introduced
//! upstream in dbt-databricks. The parsers replace the per-relation
//! `information_schema` queries with a single `DESCRIBE TABLE EXTENDED ... AS
//! JSON` call.
//!
//! Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1387

use arrow_array::StringArray;
use dbt_schemas::schemas::relations::base::BaseRelation;
use minijinja::State;
use serde_json::Value as JsonValue;

use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::macro_exec::{convert_macro_result_to_record_batch, execute_macro_with_package};
use crate::metadata::databricks::dbr_capabilities::{
    DbrCapability, DbrComputeContext, has_capability,
};
use crate::record_batch::RecordBatchExt;
use crate::relation::RelationObject;

/// One `(constraint_name, column_name)` pair of a PRIMARY KEY constraint.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1569
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrimaryKeyConstraintRow {
    pub constraint_name: String,
    pub column_name: String,
}

/// One `(constraint_name, from_column, to_catalog, to_schema, to_table, to_column)`
/// tuple of a FOREIGN KEY constraint.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1454
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForeignKeyConstraintRow {
    pub constraint_name: String,
    pub from_column: String,
    pub to_catalog: String,
    pub to_schema: String,
    pub to_table: String,
    pub to_column: String,
}

/// A column carrying an implicit NOT NULL constraint.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1541
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NonNullConstraintRow {
    pub column_name: String,
}

/// A column mask: `mask_name` is `{catalog}.{schema}.{function}`, and
/// `using_columns` is the comma-joined list of using columns, or `None` when
/// the list is empty or absent.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1420
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnMaskRow {
    pub column_name: String,
    pub mask_name: String,
    pub using_columns: Option<String>,
}

/// A row filter applied to a relation. At most one per relation.
///
/// FIXME: parsed but not yet consumed downstream (no row_filter config
/// component or DDL support); tracked in dbt-core#15763.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1634
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowFilterRow {
    pub table_catalog: String,
    pub table_schema: String,
    pub table_name: String,
    pub filter_name: String,
    pub target_columns: String,
}

/// The view definition of a view or materialized view.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1610
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewDescriptionRow {
    pub view_definition: String,
}

/// The six pieces of relation metadata the AS-JSON path replaces.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1387
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DatabricksDescribeJsonMetadata {
    pub primary_key_constraints: Vec<PrimaryKeyConstraintRow>,
    pub foreign_key_constraints: Vec<ForeignKeyConstraintRow>,
    pub non_null_constraints: Vec<NonNullConstraintRow>,
    pub column_masks: Vec<ColumnMaskRow>,
    pub row_filters: Vec<RowFilterRow>,
    pub view_description: Option<ViewDescriptionRow>,
}

impl DatabricksDescribeJsonMetadata {
    /// Fan out to the six parsers.
    ///
    /// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1396
    pub fn from_json_metadata(metadata: &JsonValue) -> AdapterResult<Self> {
        // Abstracted into a single parse. Upstream chooses to reparse the same payloads multiple
        // times.
        let (primary_key_constraints, foreign_key_constraints) =
            match table_constraints_str(metadata, "table")? {
                Some(raw) => parse_table_constraints(raw)?,
                None => (Vec::new(), Vec::new()),
            };

        Ok(Self {
            primary_key_constraints,
            foreign_key_constraints,
            non_null_constraints: parse_non_null_constraints(metadata)?,
            column_masks: parse_column_masks(metadata)?,
            row_filters: parse_row_filter(metadata)?,
            view_description: parse_view_description(metadata)?,
        })
    }
}

// Stage 1 types and utils

enum BlockSplitState {
    BetweenBlocks,
    InName,
    InBody,
    InBodyBacktick,
}

#[derive(Clone, Copy)]
enum ConstraintKind {
    PrimaryKey,
    ForeignKey,
}

/// Finds where a constraint name ends and its PRIMARY KEY or FOREIGN KEY body begins.
///
/// `rest` is the text right after a name's trailing comma. Skipping leading
/// whitespace, it must start with PRIMARY or FOREIGN, then (possibly zero)
/// whitespace, then KEY. Processing ends at a word boundary, defined as a
/// character that is not a letter, a digit, or an underscore, or the end of
/// the string.
fn key_boundary(rest: &str) -> Option<&str> {
    let after_leading_ws = rest.trim_start();
    let after_keyword = after_leading_ws
        .strip_prefix("PRIMARY")
        .or_else(|| after_leading_ws.strip_prefix("FOREIGN"))?;
    let after_key = after_keyword.trim_start().strip_prefix("KEY")?;
    if after_key
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None; // "KEY" must end at a word boundary, not e.g. "KEYSTONE"
    }
    Some(&rest[..rest.len() - after_key.len()])
}

// Stage 2: lex a block's body text into tokens

#[derive(Debug)]
enum ConstraintToken {
    LParen,
    RParen,
    Comma,
    Dot,
    Ident(String),
    References,
}

fn lex_constraint_body(mut body: &str) -> AdapterResult<Vec<ConstraintToken>> {
    let mut tokens = Vec::new();
    loop {
        body = body.trim_start();
        let Some(c) = body.chars().next() else {
            return Ok(tokens);
        };
        match c {
            '(' => {
                tokens.push(ConstraintToken::LParen);
                body = &body[1..];
            }
            ')' => {
                tokens.push(ConstraintToken::RParen);
                body = &body[1..];
            }
            ',' => {
                tokens.push(ConstraintToken::Comma);
                body = &body[1..];
            }
            '.' => {
                tokens.push(ConstraintToken::Dot);
                body = &body[1..];
            }
            '`' => {
                body = &body[1..];
                let mut ident = String::with_capacity(16);
                loop {
                    let Some(c) = body.chars().next() else {
                        return Err(AdapterError::new(
                            AdapterErrorKind::Internal,
                            "unterminated backtick identifier",
                        ));
                    };
                    body = &body[c.len_utf8()..];
                    if c != '`' {
                        ident.push(c);
                    } else if body.starts_with('`') {
                        body = &body[1..]; // doubled backtick: a literal `, still inside
                        ident.push('`');
                    } else {
                        break;
                    }
                }
                tokens.push(ConstraintToken::Ident(ident));
            }
            'R' if body.starts_with("REFERENCES") => {
                body = &body["REFERENCES".len()..];
                tokens.push(ConstraintToken::References);
            }
            other => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Internal,
                    format!("unexpected character {other:?} in constraint body"),
                ));
            }
        }
    }
}

// Stage 3: a small recursive-descent parser over a block's tokens

type TokenStream = std::iter::Peekable<std::vec::IntoIter<ConstraintToken>>;

enum ParsedConstraint {
    PrimaryKey {
        name: String,
        columns: Vec<String>,
    },
    ForeignKey {
        name: String,
        from_columns: Vec<String>,
        to_catalog: String,
        to_schema: String,
        to_table: String,
        to_columns: Vec<String>,
    },
}

fn parse_constraint_body(
    kind: ConstraintKind,
    name: String,
    tokens: Vec<ConstraintToken>,
) -> AdapterResult<ParsedConstraint> {
    let mut tokens: TokenStream = tokens.into_iter().peekable();
    match kind {
        ConstraintKind::PrimaryKey => {
            let columns = parenthesized_ident_list(&mut tokens)?;
            Ok(ParsedConstraint::PrimaryKey { name, columns })
        }
        ConstraintKind::ForeignKey => {
            let from_columns = parenthesized_ident_list(&mut tokens)?;

            if !matches!(tokens.next(), Some(ConstraintToken::References)) {
                return Err(AdapterError::new(
                    AdapterErrorKind::Internal,
                    format!("FOREIGN KEY constraint '{name}' is missing a REFERENCES clause"),
                ));
            }

            let mut ref_name_parts = vec![expect_ident(&mut tokens)?];
            while matches!(tokens.peek(), Some(ConstraintToken::Dot)) {
                tokens.next();
                ref_name_parts.push(expect_ident(&mut tokens)?);
            }
            let to_columns = parenthesized_ident_list(&mut tokens)?;

            if ref_name_parts.len() != 3 || to_columns.len() != from_columns.len() {
                return Err(AdapterError::new(
                    AdapterErrorKind::Internal,
                    format!(
                        "FOREIGN KEY constraint '{name}' must reference a 3-part \
                         `catalog`.`schema`.`table` with matching column counts \
                         (from={}, ref tokens={})",
                        from_columns.len(),
                        ref_name_parts.len() + to_columns.len()
                    ),
                ));
            }

            Ok(ParsedConstraint::ForeignKey {
                name,
                from_columns,
                to_catalog: ref_name_parts[0].clone(),
                to_schema: ref_name_parts[1].clone(),
                to_table: ref_name_parts[2].clone(),
                to_columns,
            })
        }
    }
}

/// A `(ident, ident, ...)` column list
///
/// e.g. `` (`id`) `` or `` (`a`, `b`) ``.
fn parenthesized_ident_list(tokens: &mut TokenStream) -> AdapterResult<Vec<String>> {
    match tokens.next() {
        Some(ConstraintToken::LParen) => {}
        other => {
            return Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!("expected '(', found {other:?}"),
            ));
        }
    }

    let mut idents = vec![expect_ident(tokens)?];
    while matches!(tokens.peek(), Some(ConstraintToken::Comma)) {
        tokens.next();
        idents.push(expect_ident(tokens)?);
    }

    match tokens.next() {
        Some(ConstraintToken::RParen) => {}
        other => {
            return Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!("expected ')', found {other:?}"),
            ));
        }
    }

    Ok(idents)
}

fn expect_ident(tokens: &mut TokenStream) -> AdapterResult<String> {
    match tokens.next() {
        Some(ConstraintToken::Ident(ident)) => Ok(ident),
        other => Err(AdapterError::new(
            AdapterErrorKind::Internal,
            format!("expected an identifier, found {other:?}"),
        )),
    }
}

/// Extracts `metadata["table_constraints"]` as a `&str`, or `None` when the
/// key is absent, erroring with a parser-specific `label` when present but
/// not a string.
fn table_constraints_str<'a>(
    metadata: &'a JsonValue,
    label: &str,
) -> AdapterResult<Option<&'a str>> {
    match metadata.get("table_constraints") {
        None => Ok(None),
        Some(JsonValue::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(AdapterError::new(
            AdapterErrorKind::Internal,
            format!("Failed to parse {label} constraints from describe table extended as json"),
        )),
    }
}

/// Parse a `table_constraints` string into its PRIMARY KEY rows and FOREIGN
/// KEY rows, running all three stages in turn. Subsumes what upstream splits
/// into two separate methods:
///
/// - https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1557
/// - https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1442
fn parse_table_constraints(
    raw: &str,
) -> AdapterResult<(Vec<PrimaryKeyConstraintRow>, Vec<ForeignKeyConstraintRow>)> {
    let Some(mut rest) = raw.trim().strip_prefix('[') else {
        return Ok((Vec::new(), Vec::new()));
    };

    // Stage 1: split into (name, kind, body) blocks.
    let mut state = BlockSplitState::BetweenBlocks;
    let mut depth = 0i32;
    let mut kind = ConstraintKind::PrimaryKey;
    let mut name = String::with_capacity(16);
    let mut body_start: &str = "";
    let mut blocks: Vec<(String, ConstraintKind, &str)> = Vec::new();

    let blocks = loop {
        let Some(c) = rest.chars().next() else {
            return Err(AdapterError::new(
                AdapterErrorKind::Internal,
                "unterminated table_constraints",
            ));
        };
        // Drop `c` (1-4 bytes, however wide its UTF-8 encoding is) from the front of `rest`.
        rest = &rest[c.len_utf8()..];

        match state {
            BlockSplitState::BetweenBlocks => match c {
                // closed string; end lexing phase and return
                ']' => break blocks,

                c if c.is_whitespace() || c == ',' => {}

                // string continues into the next block
                '(' => {
                    name.clear();
                    state = BlockSplitState::InName;
                }
                other => {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Internal,
                        format!("expected '(' to start a constraint block, found {other:?}"),
                    ));
                }
            },
            BlockSplitState::InName => {
                if c == ','
                    && let Some(matched) = key_boundary(rest)
                {
                    kind = if matched.trim_start().starts_with("PRIMARY") {
                        ConstraintKind::PrimaryKey
                    } else {
                        ConstraintKind::ForeignKey
                    };
                    rest = &rest[matched.len()..];
                    depth = 0;
                    body_start = rest;
                    state = BlockSplitState::InBody;
                } else {
                    name.push(c);
                }
            }
            BlockSplitState::InBody => match c {
                '`' => state = BlockSplitState::InBodyBacktick,
                '(' => depth += 1,

                // `c` (the closing ')') is already dropped from `rest`.
                ')' if depth == 0 => {
                    let body = &body_start[..body_start.len() - rest.len() - 1];
                    blocks.push((name.trim().to_string(), kind, body));
                    state = BlockSplitState::BetweenBlocks;
                }
                ')' => depth -= 1,
                _ => {}
            },
            BlockSplitState::InBodyBacktick => {
                if c == '`' && rest.starts_with('`') {
                    rest = &rest[1..]; // doubled backtick: a literal `, still inside
                } else if c == '`' {
                    state = BlockSplitState::InBody;
                }
            }
        }
    };

    // Stage 2: lex every block's body text into tokens.
    let lexed = blocks
        .into_iter()
        .map(|(name, kind, body)| -> AdapterResult<_> {
            Ok((name, kind, lex_constraint_body(body)?))
        });

    // Stage 3: parse every block's tokens into a constraint, splitting into
    // PRIMARY KEY / FOREIGN KEY rows as each one comes off the parser.
    let mut primary_keys = Vec::new();
    let mut foreign_keys = Vec::new();
    for block in lexed {
        let (name, kind, tokens) = block?;
        match parse_constraint_body(kind, name, tokens)? {
            ParsedConstraint::PrimaryKey { name, columns } => {
                primary_keys.extend(columns.into_iter().map(|column_name| {
                    PrimaryKeyConstraintRow {
                        constraint_name: name.clone(),
                        column_name,
                    }
                }));
            }
            ParsedConstraint::ForeignKey {
                name,
                from_columns,
                to_catalog,
                to_schema,
                to_table,
                to_columns,
            } => {
                foreign_keys.extend(from_columns.into_iter().zip(to_columns).map(
                    |(from_column, to_column)| ForeignKeyConstraintRow {
                        constraint_name: name.clone(),
                        from_column,
                        to_catalog: to_catalog.clone(),
                        to_schema: to_schema.clone(),
                        to_table: to_table.clone(),
                        to_column,
                    },
                ));
            }
        }
    }

    Ok((primary_keys, foreign_keys))
}

/// Parse `metadata["columns"]` into NOT NULL rows.
///
/// One row per column where `nullable` is falsy; a missing `nullable` key
/// counts as non-null, matching `not column.get("nullable")`.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1529
pub fn parse_non_null_constraints(
    metadata: &JsonValue,
) -> AdapterResult<Vec<NonNullConstraintRow>> {
    let Some(columns) = metadata.get("columns").and_then(JsonValue::as_array) else {
        return Ok(Vec::new());
    };

    columns
        .iter()
        .filter(|column| {
            !column
                .get("nullable")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false)
        })
        .map(|column| {
            let column_name = column
                .get("name")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorKind::Internal,
                        "Failed to parse non-null constraints from describe table extended as json",
                    )
                })?;
            Ok(NonNullConstraintRow {
                column_name: column_name.to_string(),
            })
        })
        .collect()
}

/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1431
#[derive(serde::Deserialize)]
struct FunctionRef {
    catalog_name: String,
    schema_name: String,
    function_name: String,
}

impl FunctionRef {
    fn qualified_name(&self) -> String {
        format!(
            "{}.{}.{}",
            self.catalog_name, self.schema_name, self.function_name
        )
    }
}

/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1429
#[derive(serde::Deserialize)]
struct ColumnMaskEntry {
    column_name: String,
    function_name: FunctionRef,
    #[serde(default)]
    using_column_names: Vec<String>,
}

/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1659
#[derive(serde::Deserialize)]
struct RowFilterEntry {
    function_name: FunctionRef,
    #[serde(default)]
    column_names: Vec<String>,
}

/// Parse `metadata["column_masks"]` into column mask rows.
///
/// `mask_name` is `{catalog}.{schema}.{function}`; `using_columns` is the
/// comma-joined `using_column_names`, or `None` when empty or absent.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1408
pub fn parse_column_masks(metadata: &JsonValue) -> AdapterResult<Vec<ColumnMaskRow>> {
    let Some(masks) = metadata.get("column_masks") else {
        return Ok(Vec::new());
    };

    let err = |e: serde_json::Error| {
        AdapterError::new(
            AdapterErrorKind::Internal,
            format!(
                "The column mask metadata Databricks returned wasn't in the expected structure: {e}"
            ),
        )
    };
    let entries: Vec<ColumnMaskEntry> = serde_json::from_value(masks.clone()).map_err(err)?;

    Ok(entries
        .into_iter()
        .map(|entry| ColumnMaskRow {
            column_name: entry.column_name,
            mask_name: entry.function_name.qualified_name(),
            using_columns: (!entry.using_column_names.is_empty())
                .then(|| entry.using_column_names.join(",")),
        })
        .collect())
}

/// Parse `metadata["row_filter"]` (plus the top-level catalog/schema/table
/// names) into at most one row-filter row.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1622
pub fn parse_row_filter(metadata: &JsonValue) -> AdapterResult<Vec<RowFilterRow>> {
    let Some(row_filter) = metadata.get("row_filter") else {
        return Ok(Vec::new());
    };

    const MESSAGE: &str =
        "The row filter metadata Databricks returned wasn't in the expected structure";
    let entry: RowFilterEntry = serde_json::from_value(row_filter.clone())
        .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, format!("{MESSAGE}: {e}")))?;

    let missing = || AdapterError::new(AdapterErrorKind::Internal, MESSAGE);
    let table_catalog = metadata
        .get("catalog_name")
        .and_then(JsonValue::as_str)
        .ok_or_else(missing)?;
    let table_schema = metadata
        .get("schema_name")
        .and_then(JsonValue::as_str)
        .ok_or_else(missing)?;
    let table_name = metadata
        .get("table_name")
        .and_then(JsonValue::as_str)
        .ok_or_else(missing)?;

    Ok(vec![RowFilterRow {
        table_catalog: table_catalog.to_string(),
        table_schema: table_schema.to_string(),
        table_name: table_name.to_string(),
        filter_name: entry.function_name.qualified_name(),
        target_columns: entry.column_names.join(","),
    }])
}

/// Parse `metadata["view_text"]` into a view description.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L1598
pub fn parse_view_description(metadata: &JsonValue) -> AdapterResult<Option<ViewDescriptionRow>> {
    Ok(metadata
        .get("view_text")
        .and_then(JsonValue::as_str)
        .map(|view_definition| ViewDescriptionRow {
            view_definition: view_definition.to_string(),
        }))
}

/// Whether the `DESCRIBE TABLE EXTENDED ... AS JSON` path may be used for
/// `relation`.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L421
pub fn is_describe_as_json_supported(
    relation: &dyn BaseRelation,
    is_foreign_table: bool,
    compute: DbrComputeContext,
    behavior_flag_enabled: bool,
) -> bool {
    !relation.is_hive_metastore()
        && !is_foreign_table
        && has_capability(DbrCapability::DescribeTableExtendedAsJson, compute)
        && behavior_flag_enabled
}

/// Runs the `describe_table_extended_as_json` macro for `relation` and parses
/// its `json_metadata` column into a [`JsonValue`].
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L426
pub fn fetch_json_metadata(state: &State, relation: &dyn BaseRelation) -> AdapterResult<JsonValue> {
    let result = execute_macro_with_package(
        state,
        &[RelationObject::new(relation.to_owned()).into_value()],
        "describe_table_extended_as_json",
        "dbt_databricks",
    )?;
    let batch = convert_macro_result_to_record_batch(&result)?;
    let json_metadata_col = batch.column_values::<StringArray>("json_metadata")?;
    let json_metadata = json_metadata_col.value(0);
    serde_json::from_str(json_metadata).map_err(|e| {
        AdapterError::new(
            AdapterErrorKind::Internal,
            format!("Failed to parse json metadata from describe table extended as json: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    //! Fixtures and cases ported from: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/tests/unit/test_describe_json_metadata.py
    use super::*;

    use std::sync::Arc;

    use dbt_adapter_core::AdapterType;
    use dbt_schemas::dbt_types::RelationType;
    use serde_json::json;

    use crate::metadata::databricks::version::EngineVersion;
    use crate::relation::Relation;

    // ----------------------------------------------------------------------
    // Helpers
    // ----------------------------------------------------------------------

    fn parse_primary_key_constraints(
        metadata: &JsonValue,
    ) -> AdapterResult<Vec<PrimaryKeyConstraintRow>> {
        let Some(raw) = table_constraints_str(metadata, "primary key")? else {
            return Ok(Vec::new());
        };
        Ok(parse_table_constraints(raw)?.0)
    }

    fn parse_foreign_key_constraints(
        metadata: &JsonValue,
    ) -> AdapterResult<Vec<ForeignKeyConstraintRow>> {
        let Some(raw) = table_constraints_str(metadata, "foreign key")? else {
            return Ok(Vec::new());
        };
        Ok(parse_table_constraints(raw)?.1)
    }

    fn constraints_json(table_constraints: &str) -> JsonValue {
        json!({ "table_constraints": table_constraints })
    }

    fn pk(constraint_name: &str, column_name: &str) -> PrimaryKeyConstraintRow {
        PrimaryKeyConstraintRow {
            constraint_name: constraint_name.to_string(),
            column_name: column_name.to_string(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fk(
        constraint_name: &str,
        from_column: &str,
        to_catalog: &str,
        to_schema: &str,
        to_table: &str,
        to_column: &str,
    ) -> ForeignKeyConstraintRow {
        ForeignKeyConstraintRow {
            constraint_name: constraint_name.to_string(),
            from_column: from_column.to_string(),
            to_catalog: to_catalog.to_string(),
            to_schema: to_schema.to_string(),
            to_table: to_table.to_string(),
            to_column: to_column.to_string(),
        }
    }

    fn non_null(column_name: &str) -> NonNullConstraintRow {
        NonNullConstraintRow {
            column_name: column_name.to_string(),
        }
    }

    fn mask(column_name: &str, mask_name: &str, using_columns: Option<&str>) -> ColumnMaskRow {
        ColumnMaskRow {
            column_name: column_name.to_string(),
            mask_name: mask_name.to_string(),
            using_columns: using_columns.map(|s| s.to_string()),
        }
    }

    /// Assert a parser returned an error whose message contains `needle`,
    /// without panicking when it (wrongly) returned `Ok`.
    fn assert_err_contains<T: std::fmt::Debug>(result: AdapterResult<T>, needle: &str) {
        match result {
            Ok(value) => {
                panic!("expected an error containing {needle:?}, got Ok({value:?})")
            }
            Err(err) => {
                let message = err.to_string();
                assert!(
                    message.contains(needle),
                    "expected error message to contain {needle:?}, got {message:?}"
                );
            }
        }
    }

    // ----------------------------------------------------------------------
    // Shared fixtures (4i)
    // ----------------------------------------------------------------------

    fn email_addresses_json() -> JsonValue {
        json!({
            "table_name": "email_addresses",
            "catalog_name": "main",
            "schema_name": "default",
            "type": "MANAGED",
            "columns": [
                {"name": "address_id", "type": {"name": "int"}, "nullable": false},
                {"name": "email", "type": {"name": "string"}, "nullable": true},
            ],
            "table_constraints": "[(pk1,PRIMARY KEY (`address_id`))]",
        })
    }

    fn column_mask_json() -> JsonValue {
        json!({
            "table_name": "table_with_masks",
            "catalog_name": "main",
            "schema_name": "db",
            "columns": [
                {"name": "phone_number", "type": {"name": "string"}, "nullable": true},
            ],
            "column_masks": [
                {
                    "column_name": "phone_number",
                    "function_name": {
                        "catalog_name": "main",
                        "schema_name": "db",
                        "function_name": "mask_phone",
                    },
                    "using_column_names": ["city"],
                }
            ],
        })
    }

    fn row_filter_json() -> JsonValue {
        json!({
            "table_name": "table_with_row_filter",
            "catalog_name": "default_catalog",
            "schema_name": "default",
            "columns": [
                {"name": "region", "type": {"name": "string"}, "nullable": true},
            ],
            "row_filter": {
                "function_name": {
                    "catalog_name": "default_catalog",
                    "schema_name": "default",
                    "function_name": "filter_by_region",
                },
                "column_names": ["region"],
            },
        })
    }

    fn row_filter_multi_column_json() -> JsonValue {
        json!({
            "table_name": "table_with_row_filter",
            "catalog_name": "default_catalog",
            "schema_name": "default",
            "row_filter": {
                "function_name": {
                    "catalog_name": "default_catalog",
                    "schema_name": "default",
                    "function_name": "filter_by_dept_and_region",
                },
                "column_names": ["department", "region"],
            },
        })
    }

    fn materialized_view_json() -> JsonValue {
        json!({
            "table_name": "my_mv",
            "catalog_name": "main",
            "schema_name": "default",
            "type": "MATERIALIZED_VIEW",
            "view_text": "SELECT id, name FROM main.default.source_table",
        })
    }

    fn regular_view_json() -> JsonValue {
        json!({
            "table_name": "my_view",
            "catalog_name": "main",
            "schema_name": "default",
            "type": "VIEW",
            "view_text": "SELECT id, name FROM main.default.source_table",
        })
    }

    fn plain_table_json() -> JsonValue {
        json!({
            "table_name": "plain_table",
            "catalog_name": "main",
            "schema_name": "default",
            "type": "MANAGED",
            "columns": [
                {"name": "id", "type": {"name": "int"}, "nullable": true},
                {"name": "name", "type": {"name": "string"}, "nullable": true},
            ],
        })
    }

    fn composite_pk_json() -> JsonValue {
        constraints_json("[(pk1,PRIMARY KEY (`col_a`, `col_b`))]")
    }

    fn composite_fk_json() -> JsonValue {
        constraints_json(
            "[(fk1,FOREIGN KEY (`from_a`, `from_b`) REFERENCES `cat`.`sch`.`tbl` (`to_a`, `to_b`))]",
        )
    }

    fn mixed_pk_fk_json() -> JsonValue {
        constraints_json(
            "[(pk1,PRIMARY KEY (`id`)), (fk1,FOREIGN KEY (`ref_id`) REFERENCES `main`.`default`.`users` (`user_id`))]",
        )
    }

    fn all_fields_json() -> JsonValue {
        json!({
            "table_name": "everything",
            "catalog_name": "main",
            "schema_name": "default",
            "type": "MATERIALIZED_VIEW",
            "view_text": "SELECT id, name FROM main.default.source_table",
            "columns": [
                {"name": "id", "type": {"name": "int"}, "nullable": false},
                {"name": "name", "type": {"name": "string"}, "nullable": true},
            ],
            "table_constraints":
                "[(pk1,PRIMARY KEY (`id`)), (fk1,FOREIGN KEY (`ref_id`) REFERENCES `main`.`default`.`users` (`user_id`))]",
            "column_masks": [
                {
                    "column_name": "name",
                    "function_name": {
                        "catalog_name": "main",
                        "schema_name": "default",
                        "function_name": "mask_name",
                    },
                    "using_column_names": [],
                }
            ],
            "row_filter": {
                "function_name": {
                    "catalog_name": "main",
                    "schema_name": "default",
                    "function_name": "filter_rows",
                },
                "column_names": ["id"],
            },
        })
    }

    /// The PR's own worked example, packing four edge cases into one string.
    const WORKED_EXAMPLE: &str = "[(p-a,b@c(d,PRIMARY KEY (`id``a`)), (fk1,FOREIGN KEY (`x`, `y`) REFERENCES `cat`.`sch`.`tbl` (`a`, `b`))]";

    // ----------------------------------------------------------------------
    // 4a — parse_primary_key_constraints
    // ----------------------------------------------------------------------

    #[test]
    fn test_parse_primary_key_constraints_single() {
        let result =
            parse_primary_key_constraints(&constraints_json("[(pk1,PRIMARY KEY (`address_id`))]"));
        assert_eq!(result.ok(), Some(vec![pk("pk1", "address_id")]));
    }

    #[test]
    fn test_parse_primary_key_constraints_fk_only() {
        let result = parse_primary_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`ref_id`) REFERENCES `main`.`default`.`t` (`id`))]",
        ));
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_primary_key_constraints_key_absent() {
        let result = parse_primary_key_constraints(&json!({}));
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_primary_key_constraints_empty_string() {
        let result = parse_primary_key_constraints(&constraints_json(""));
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_primary_key_constraints_whitespace_robustness() {
        for n in 0..40 {
            let s = " ".repeat(n);
            let input = format!("[{s}({s}pk1,{s}PRIMARY{s}KEY{s}({s}`col_1`{s}){s}){s}]");
            let result = parse_primary_key_constraints(&constraints_json(&input));
            assert_eq!(
                result.ok(),
                Some(vec![pk("pk1", "col_1")]),
                "failed with {n} extra spaces: {input:?}"
            );
        }
    }

    #[test]
    fn test_parse_primary_key_constraints_many_constraints() {
        let entries: Vec<String> = (0..20)
            .map(|i| format!("(pk{i},PRIMARY KEY (`col_{i}`))"))
            .collect();
        let input = format!("[{}]", entries.join(", "));
        let expected: Vec<PrimaryKeyConstraintRow> = (0..20)
            .map(|i| pk(&format!("pk{i}"), &format!("col_{i}")))
            .collect();

        let result = parse_primary_key_constraints(&constraints_json(&input));
        assert_eq!(result.ok(), Some(expected));
    }

    #[test]
    fn test_parse_primary_key_constraints_composite() {
        for n in 1..=20 {
            let cols: Vec<String> = (0..n).map(|i| format!("`col_{i}`")).collect();
            let input = format!("[(pk1,PRIMARY KEY ({}))]", cols.join(", "));
            let expected: Vec<PrimaryKeyConstraintRow> =
                (0..n).map(|i| pk("pk1", &format!("col_{i}"))).collect();

            let result = parse_primary_key_constraints(&constraints_json(&input));
            assert_eq!(result.ok(), Some(expected), "failed with {n} columns");
        }
    }

    #[test]
    fn test_parse_primary_key_constraints_underscore_padding() {
        for n in 0..20 {
            let pad = "_".repeat(n);
            let column = format!("{pad}col{pad}");
            let input = format!("[(pk1,PRIMARY KEY (`{column}`))]");

            let result = parse_primary_key_constraints(&constraints_json(&input));
            assert_eq!(
                result.ok(),
                Some(vec![pk("pk1", &column)]),
                "failed with {n} underscores"
            );
        }
    }

    #[test]
    fn test_parse_primary_key_constraints_hyphen_in_name() {
        let result =
            parse_primary_key_constraints(&constraints_json("[(my-pk,PRIMARY KEY (`id`))]"));
        assert_eq!(result.ok(), Some(vec![pk("my-pk", "id")]));
    }

    #[test]
    fn test_parse_primary_key_constraints_at_sign_in_name() {
        let result =
            parse_primary_key_constraints(&constraints_json("[(pk@1,PRIMARY KEY (`id`))]"));
        assert_eq!(result.ok(), Some(vec![pk("pk@1", "id")]));
    }

    #[test]
    fn test_parse_primary_key_constraints_comma_in_name() {
        let result = parse_primary_key_constraints(&constraints_json("[(a,b,PRIMARY KEY (`id`))]"));
        assert_eq!(result.ok(), Some(vec![pk("a,b", "id")]));
    }

    #[test]
    fn test_parse_primary_key_constraints_paren_in_name() {
        let result = parse_primary_key_constraints(&constraints_json("[(a(b,PRIMARY KEY (`id`))]"));
        assert_eq!(result.ok(), Some(vec![pk("a(b", "id")]));
    }

    #[test]
    fn test_parse_primary_key_constraints_backtick_in_name() {
        let result = parse_primary_key_constraints(&constraints_json("[(p`a,PRIMARY KEY (`id`))]"));
        assert_eq!(result.ok(), Some(vec![pk("p`a", "id")]));
    }

    #[test]
    fn test_parse_primary_key_constraints_escaped_backtick_in_column() {
        let result =
            parse_primary_key_constraints(&constraints_json("[(p-a4,PRIMARY KEY (`id``a`))]"));
        assert_eq!(result.ok(), Some(vec![pk("p-a4", "id`a")]));
    }

    #[test]
    fn test_parse_primary_key_constraints_cjk_constraint_name() {
        let result =
            parse_primary_key_constraints(&constraints_json("[(用户_pk,PRIMARY KEY (`id`))]"));
        assert_eq!(result.ok(), Some(vec![pk("用户_pk", "id")]));
    }

    #[test]
    fn test_parse_primary_key_constraints_acute_accent_column() {
        let result =
            parse_primary_key_constraints(&constraints_json("[(pk1,PRIMARY KEY (`prénom`))]"));
        assert_eq!(result.ok(), Some(vec![pk("pk1", "prénom")]));
    }

    #[test]
    fn test_parse_primary_key_constraints_diaeresis_column() {
        let result =
            parse_primary_key_constraints(&constraints_json("[(pk1,PRIMARY KEY (`cliënt_id`))]"));
        assert_eq!(result.ok(), Some(vec![pk("pk1", "cliënt_id")]));
    }

    #[test]
    fn test_parse_primary_key_constraints_non_string_input() {
        let result = parse_primary_key_constraints(&json!({"table_constraints": 123}));
        assert_err_contains(
            result,
            "Failed to parse primary key constraints from describe table extended as json",
        );
    }

    #[test]
    fn test_parse_primary_key_constraints_worked_example() {
        let result = parse_primary_key_constraints(&constraints_json(WORKED_EXAMPLE));
        assert_eq!(result.ok(), Some(vec![pk("p-a,b@c(d", "id`a")]));
    }

    // ----------------------------------------------------------------------
    // 4b — parse_foreign_key_constraints
    // ----------------------------------------------------------------------

    #[test]
    fn test_parse_foreign_key_constraints_single() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`ref_id`) REFERENCES `main`.`default`.`users` (`user_id`))]",
        ));
        assert_eq!(
            result.ok(),
            Some(vec![fk(
                "fk1", "ref_id", "main", "default", "users", "user_id"
            )])
        );
    }

    #[test]
    fn test_parse_foreign_key_constraints_composite() {
        for n in 1..=20 {
            let from: Vec<String> = (0..n).map(|i| format!("`from_{i}`")).collect();
            let to: Vec<String> = (0..n).map(|i| format!("`to_{i}`")).collect();
            let input = format!(
                "[(fk1,FOREIGN KEY ({}) REFERENCES `cat`.`sch`.`tbl` ({}))]",
                from.join(", "),
                to.join(", ")
            );
            let expected: Vec<ForeignKeyConstraintRow> = (0..n)
                .map(|i| {
                    fk(
                        "fk1",
                        &format!("from_{i}"),
                        "cat",
                        "sch",
                        "tbl",
                        &format!("to_{i}"),
                    )
                })
                .collect();

            let result = parse_foreign_key_constraints(&constraints_json(&input));
            assert_eq!(result.ok(), Some(expected), "failed with {n} columns");
        }
    }

    #[test]
    fn test_parse_foreign_key_constraints_hyphenated_schema() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`ref_id`) REFERENCES `main`.`my-schema`.`users` (`user_id`))]",
        ));
        assert_eq!(
            result.ok(),
            Some(vec![fk(
                "fk1",
                "ref_id",
                "main",
                "my-schema",
                "users",
                "user_id"
            )])
        );
    }

    #[test]
    fn test_parse_foreign_key_constraints_mixed_pk_fk() {
        let result = parse_foreign_key_constraints(&mixed_pk_fk_json());
        assert_eq!(
            result.ok(),
            Some(vec![fk(
                "fk1", "ref_id", "main", "default", "users", "user_id"
            )])
        );
    }

    #[test]
    fn test_parse_foreign_key_constraints_pk_only() {
        let result = parse_foreign_key_constraints(&constraints_json("[(pk1,PRIMARY KEY (`id`))]"));
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_foreign_key_constraints_key_absent() {
        let result = parse_foreign_key_constraints(&json!({}));
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_foreign_key_constraints_empty_string() {
        let result = parse_foreign_key_constraints(&constraints_json(""));
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_foreign_key_constraints_whitespace_robustness() {
        for n in 0..40 {
            let s = " ".repeat(n);
            let input = format!(
                "[{s}({s}fk1,{s}FOREIGN{s}KEY{s}({s}`ref_id`{s}){s}REFERENCES{s}`main`{s}.{s}`default`{s}.{s}`users`{s}({s}`user_id`{s}){s}){s}]"
            );
            let result = parse_foreign_key_constraints(&constraints_json(&input));
            assert_eq!(
                result.ok(),
                Some(vec![fk(
                    "fk1", "ref_id", "main", "default", "users", "user_id"
                )]),
                "failed with {n} extra spaces: {input:?}"
            );
        }
    }

    #[test]
    fn test_parse_foreign_key_constraints_many_constraints() {
        let entries: Vec<String> = (0..20)
            .map(|i| {
                format!(
                    "(fk{i},FOREIGN KEY (`from_{i}`) REFERENCES `cat`.`sch`.`tbl_{i}` (`to_{i}`))"
                )
            })
            .collect();
        let input = format!("[{}]", entries.join(", "));
        let expected: Vec<ForeignKeyConstraintRow> = (0..20)
            .map(|i| {
                fk(
                    &format!("fk{i}"),
                    &format!("from_{i}"),
                    "cat",
                    "sch",
                    &format!("tbl_{i}"),
                    &format!("to_{i}"),
                )
            })
            .collect();

        let result = parse_foreign_key_constraints(&constraints_json(&input));
        assert_eq!(result.ok(), Some(expected));
    }

    #[test]
    fn test_parse_foreign_key_constraints_underscore_padding() {
        for n in 0..20 {
            let pad = "_".repeat(n);
            let name = format!("{pad}fk{pad}");
            let from = format!("{pad}from{pad}");
            let catalog = format!("{pad}cat{pad}");
            let schema = format!("{pad}sch{pad}");
            let table = format!("{pad}tbl{pad}");
            let to = format!("{pad}to{pad}");
            let input = format!(
                "[({name},FOREIGN KEY (`{from}`) REFERENCES `{catalog}`.`{schema}`.`{table}` (`{to}`))]"
            );

            let result = parse_foreign_key_constraints(&constraints_json(&input));
            assert_eq!(
                result.ok(),
                Some(vec![fk(&name, &from, &catalog, &schema, &table, &to)]),
                "failed with {n} underscores"
            );
        }
    }

    #[test]
    fn test_parse_foreign_key_constraints_special_characters_in_name() {
        for name in ["my-fk", "fk@1", "a,b", "a(b", "p`a"] {
            let input = format!(
                "[({name},FOREIGN KEY (`ref_id`) REFERENCES `main`.`default`.`users` (`user_id`))]"
            );
            let result = parse_foreign_key_constraints(&constraints_json(&input));
            assert_eq!(
                result.ok(),
                Some(vec![fk(
                    name, "ref_id", "main", "default", "users", "user_id"
                )]),
                "failed for constraint name {name:?}"
            );
        }
    }

    #[test]
    fn test_parse_foreign_key_constraints_diaeresis_constraint_name() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(cliënt_fk,FOREIGN KEY (`ref_id`) REFERENCES `main`.`default`.`users` (`user_id`))]",
        ));
        assert_eq!(
            result.ok(),
            Some(vec![fk(
                "cliënt_fk",
                "ref_id",
                "main",
                "default",
                "users",
                "user_id"
            )])
        );
    }

    #[test]
    fn test_parse_foreign_key_constraints_cjk_identifiers() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`用户_id`) REFERENCES `主目录`.`架构`.`用户` (`编号`))]",
        ));
        assert_eq!(
            result.ok(),
            Some(vec![fk("fk1", "用户_id", "主目录", "架构", "用户", "编号")])
        );
    }

    #[test]
    fn test_parse_foreign_key_constraints_escaped_backtick_in_from_column() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`ref``id`) REFERENCES `main`.`default`.`users` (`user_id`))]",
        ));
        assert_eq!(
            result.ok(),
            Some(vec![fk(
                "fk1", "ref`id", "main", "default", "users", "user_id"
            )])
        );
    }

    #[test]
    fn test_parse_foreign_key_constraints_escaped_backtick_in_referenced_table() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`ref_id`) REFERENCES `main`.`default`.`weird``tbl` (`user_id`))]",
        ));
        assert_eq!(
            result.ok(),
            Some(vec![fk(
                "fk1",
                "ref_id",
                "main",
                "default",
                "weird`tbl",
                "user_id"
            )])
        );
    }

    #[test]
    fn test_parse_foreign_key_constraints_column_named_references() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`REFERENCES`) REFERENCES `c`.`s`.`t` (`id`))]",
        ));
        assert_eq!(
            result.ok(),
            Some(vec![fk("fk1", "REFERENCES", "c", "s", "t", "id")])
        );
    }

    #[test]
    fn test_parse_foreign_key_constraints_missing_references_clause() {
        let result =
            parse_foreign_key_constraints(&constraints_json("[(fk1,FOREIGN KEY (`ref_id`))]"));
        assert_err_contains(result, "missing a REFERENCES");
    }

    #[test]
    fn test_parse_foreign_key_constraints_one_part_referenced_name() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`ref_id`) REFERENCES `t` (`id`))]",
        ));
        assert_err_contains(result, "3-part");
    }

    #[test]
    fn test_parse_foreign_key_constraints_two_part_referenced_name() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`ref_id`) REFERENCES `s`.`t` (`id`))]",
        ));
        assert_err_contains(result, "3-part");
    }

    #[test]
    fn test_parse_foreign_key_constraints_four_part_referenced_name() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`ref_id`) REFERENCES `a`.`b`.`c`.`d` (`id`))]",
        ));
        assert_err_contains(result, "3-part");
    }

    #[test]
    fn test_parse_foreign_key_constraints_mismatched_column_counts() {
        let result = parse_foreign_key_constraints(&constraints_json(
            "[(fk1,FOREIGN KEY (`a`, `b`) REFERENCES `c`.`s`.`t` (`x`))]",
        ));
        assert_err_contains(result, "3-part");
    }

    #[test]
    fn test_parse_foreign_key_constraints_non_string_input() {
        let result = parse_foreign_key_constraints(&json!({"table_constraints": 123}));
        assert_err_contains(
            result,
            "Failed to parse foreign key constraints from describe table extended as json",
        );
    }

    #[test]
    fn test_parse_foreign_key_constraints_composite_fixture() {
        let result = parse_foreign_key_constraints(&composite_fk_json());
        assert_eq!(
            result.ok(),
            Some(vec![
                fk("fk1", "from_a", "cat", "sch", "tbl", "to_a"),
                fk("fk1", "from_b", "cat", "sch", "tbl", "to_b"),
            ])
        );
    }

    #[test]
    fn test_parse_foreign_key_constraints_worked_example() {
        let result = parse_foreign_key_constraints(&constraints_json(WORKED_EXAMPLE));
        assert_eq!(
            result.ok(),
            Some(vec![
                fk("fk1", "x", "cat", "sch", "tbl", "a"),
                fk("fk1", "y", "cat", "sch", "tbl", "b"),
            ])
        );
    }

    #[test]
    fn test_parse_primary_key_constraints_composite_fixture() {
        let result = parse_primary_key_constraints(&composite_pk_json());
        assert_eq!(
            result.ok(),
            Some(vec![pk("pk1", "col_a"), pk("pk1", "col_b")])
        );
    }

    // ----------------------------------------------------------------------
    // 4c — parse_non_null_constraints
    // ----------------------------------------------------------------------

    #[test]
    fn test_parse_non_null_constraints_mixed() {
        let result = parse_non_null_constraints(&json!({
            "columns": [
                {"name": "id", "nullable": false},
                {"name": "email", "nullable": true},
            ]
        }));
        assert_eq!(result.ok(), Some(vec![non_null("id")]));
    }

    #[test]
    fn test_parse_non_null_constraints_all_nullable() {
        let result = parse_non_null_constraints(&json!({
            "columns": [
                {"name": "a", "nullable": true},
                {"name": "b", "nullable": true},
            ]
        }));
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_non_null_constraints_multiple_non_null() {
        let result = parse_non_null_constraints(&json!({
            "columns": [
                {"name": "id", "nullable": false},
                {"name": "email", "nullable": false},
                {"name": "msg", "nullable": true},
            ]
        }));
        assert_eq!(result.ok(), Some(vec![non_null("id"), non_null("email")]));
    }

    #[test]
    fn test_parse_non_null_constraints_no_columns_key() {
        let result = parse_non_null_constraints(&json!({}));
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_non_null_constraints_column_missing_name() {
        let result = parse_non_null_constraints(&json!({"columns": [{"nullable": false}]}));
        assert_err_contains(
            result,
            "Failed to parse non-null constraints from describe table extended as json",
        );
    }

    #[test]
    fn test_parse_non_null_constraints_missing_nullable_key_is_non_null() {
        let result = parse_non_null_constraints(&json!({"columns": [{"name": "id"}]}));
        assert_eq!(result.ok(), Some(vec![non_null("id")]));
    }

    // ----------------------------------------------------------------------
    // 4d — parse_column_masks
    // ----------------------------------------------------------------------

    #[test]
    fn test_parse_column_masks_with_using_columns() {
        let result = parse_column_masks(&column_mask_json());
        assert_eq!(
            result.ok(),
            Some(vec![mask(
                "phone_number",
                "main.db.mask_phone",
                Some("city")
            )])
        );
    }

    #[test]
    fn test_parse_column_masks_empty_using_columns() {
        let result = parse_column_masks(&json!({
            "column_masks": [{
                "column_name": "ssn",
                "function_name": {
                    "catalog_name": "main",
                    "schema_name": "db",
                    "function_name": "mask_ssn",
                },
                "using_column_names": [],
            }]
        }));
        assert_eq!(
            result.ok(),
            Some(vec![mask("ssn", "main.db.mask_ssn", None)])
        );
    }

    #[test]
    fn test_parse_column_masks_multiple_masks() {
        let result = parse_column_masks(&json!({
            "column_masks": [
                {
                    "column_name": "col_a",
                    "function_name": {
                        "catalog_name": "main",
                        "schema_name": "db",
                        "function_name": "mask_a",
                    },
                    "using_column_names": ["x"],
                },
                {
                    "column_name": "col_b",
                    "function_name": {
                        "catalog_name": "main",
                        "schema_name": "db",
                        "function_name": "mask_b",
                    },
                    "using_column_names": [],
                },
            ]
        }));
        assert_eq!(
            result.ok(),
            Some(vec![
                mask("col_a", "main.db.mask_a", Some("x")),
                mask("col_b", "main.db.mask_b", None),
            ])
        );
    }

    #[test]
    fn test_parse_column_masks_no_column_masks_key() {
        let result = parse_column_masks(&json!({}));
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_column_masks_empty_column_masks() {
        let result = parse_column_masks(&json!({"column_masks": []}));
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_column_masks_multiple_using_columns() {
        let result = parse_column_masks(&json!({
            "column_masks": [{
                "column_name": "col",
                "function_name": {
                    "catalog_name": "main",
                    "schema_name": "db",
                    "function_name": "mask",
                },
                "using_column_names": ["col1", "col2", "col3"],
            }]
        }));
        assert_eq!(
            result.ok(),
            Some(vec![mask("col", "main.db.mask", Some("col1,col2,col3"))])
        );
    }

    #[test]
    fn test_parse_column_masks_using_column_names_absent() {
        let result = parse_column_masks(&json!({
            "column_masks": [{
                "column_name": "col",
                "function_name": {
                    "catalog_name": "main",
                    "schema_name": "db",
                    "function_name": "mask",
                },
            }]
        }));
        assert_eq!(result.ok(), Some(vec![mask("col", "main.db.mask", None)]));
    }

    #[test]
    fn test_parse_column_masks_missing_function_name() {
        let result = parse_column_masks(&json!({"column_masks": [{"column_name": "x"}]}));
        assert_err_contains(
            result,
            "The column mask metadata Databricks returned wasn't in the expected structure",
        );
    }

    // ----------------------------------------------------------------------
    // 4e — parse_row_filter
    // ----------------------------------------------------------------------

    #[test]
    fn test_parse_row_filter_single_target_column() {
        let result = parse_row_filter(&row_filter_json());
        assert_eq!(
            result.ok(),
            Some(vec![RowFilterRow {
                table_catalog: "default_catalog".to_string(),
                table_schema: "default".to_string(),
                table_name: "table_with_row_filter".to_string(),
                filter_name: "default_catalog.default.filter_by_region".to_string(),
                target_columns: "region".to_string(),
            }])
        );
    }

    #[test]
    fn test_parse_row_filter_multiple_target_columns() {
        let result = parse_row_filter(&row_filter_multi_column_json());
        assert_eq!(
            result.ok(),
            Some(vec![RowFilterRow {
                table_catalog: "default_catalog".to_string(),
                table_schema: "default".to_string(),
                table_name: "table_with_row_filter".to_string(),
                filter_name: "default_catalog.default.filter_by_dept_and_region".to_string(),
                target_columns: "department,region".to_string(),
            }])
        );
    }

    #[test]
    fn test_parse_row_filter_no_row_filter_key() {
        let result = parse_row_filter(&plain_table_json());
        assert_eq!(result.ok(), Some(vec![]));
    }

    #[test]
    fn test_parse_row_filter_missing_function_name() {
        let result = parse_row_filter(&json!({
            "catalog_name": "main",
            "schema_name": "default",
            "table_name": "t",
            "row_filter": {"column_names": ["x"]},
        }));
        assert_err_contains(
            result,
            "The row filter metadata Databricks returned wasn't in the expected structure",
        );
    }

    // ----------------------------------------------------------------------
    // 4f — parse_view_description
    // ----------------------------------------------------------------------

    #[test]
    fn test_parse_view_description_present() {
        let result = parse_view_description(&regular_view_json());
        assert_eq!(
            result.ok(),
            Some(Some(ViewDescriptionRow {
                view_definition: "SELECT id, name FROM main.default.source_table".to_string(),
            }))
        );
    }

    #[test]
    fn test_parse_view_description_absent() {
        let result = parse_view_description(&plain_table_json());
        assert_eq!(result.ok(), Some(None));
    }

    #[test]
    fn test_parse_view_description_null() {
        let result = parse_view_description(&json!({"view_text": null}));
        assert_eq!(result.ok(), Some(None));
    }

    // ----------------------------------------------------------------------
    // 4g — from_json_metadata
    // ----------------------------------------------------------------------

    #[test]
    fn test_from_json_metadata_table_with_column_masks() {
        let parsed = DatabricksDescribeJsonMetadata::from_json_metadata(&column_mask_json());
        assert_eq!(
            parsed.ok(),
            Some(DatabricksDescribeJsonMetadata {
                column_masks: vec![mask("phone_number", "main.db.mask_phone", Some("city"))],
                ..Default::default()
            })
        );
    }

    #[test]
    fn test_from_json_metadata_table_with_row_filter() {
        let parsed = DatabricksDescribeJsonMetadata::from_json_metadata(&row_filter_json());
        assert_eq!(
            parsed.ok(),
            Some(DatabricksDescribeJsonMetadata {
                row_filters: vec![RowFilterRow {
                    table_catalog: "default_catalog".to_string(),
                    table_schema: "default".to_string(),
                    table_name: "table_with_row_filter".to_string(),
                    filter_name: "default_catalog.default.filter_by_region".to_string(),
                    target_columns: "region".to_string(),
                }],
                ..Default::default()
            })
        );
    }

    #[test]
    fn test_from_json_metadata_materialized_view() {
        let parsed = DatabricksDescribeJsonMetadata::from_json_metadata(&materialized_view_json());
        assert_eq!(
            parsed.ok(),
            Some(DatabricksDescribeJsonMetadata {
                view_description: Some(ViewDescriptionRow {
                    view_definition: "SELECT id, name FROM main.default.source_table".to_string(),
                }),
                ..Default::default()
            })
        );
    }

    #[test]
    fn test_from_json_metadata_all_fields() {
        let parsed = DatabricksDescribeJsonMetadata::from_json_metadata(&all_fields_json());
        assert_eq!(
            parsed.ok(),
            Some(DatabricksDescribeJsonMetadata {
                primary_key_constraints: vec![pk("pk1", "id")],
                foreign_key_constraints: vec![fk(
                    "fk1", "ref_id", "main", "default", "users", "user_id"
                )],
                non_null_constraints: vec![non_null("id")],
                column_masks: vec![mask("name", "main.default.mask_name", None)],
                row_filters: vec![RowFilterRow {
                    table_catalog: "main".to_string(),
                    table_schema: "default".to_string(),
                    table_name: "everything".to_string(),
                    filter_name: "main.default.filter_rows".to_string(),
                    target_columns: "id".to_string(),
                }],
                view_description: Some(ViewDescriptionRow {
                    view_definition: "SELECT id, name FROM main.default.source_table".to_string(),
                }),
            })
        );
    }

    #[test]
    fn test_from_json_metadata_plain_table() {
        let parsed = DatabricksDescribeJsonMetadata::from_json_metadata(&plain_table_json());
        assert_eq!(parsed.ok(), Some(DatabricksDescribeJsonMetadata::default()));
    }

    #[test]
    fn test_from_json_metadata_email_addresses_fixture() {
        let parsed = DatabricksDescribeJsonMetadata::from_json_metadata(&email_addresses_json());
        assert_eq!(
            parsed.ok(),
            Some(DatabricksDescribeJsonMetadata {
                primary_key_constraints: vec![pk("pk1", "address_id")],
                non_null_constraints: vec![non_null("address_id")],
                ..Default::default()
            })
        );
    }

    #[test]
    fn test_from_json_metadata_pk_with_column_named_foreign_key() {
        let metadata = constraints_json("[(pk1,PRIMARY KEY (`foreign_key`))]");
        let parsed = DatabricksDescribeJsonMetadata::from_json_metadata(&metadata);
        assert_eq!(
            parsed.ok(),
            Some(DatabricksDescribeJsonMetadata {
                primary_key_constraints: vec![pk("pk1", "foreign_key")],
                foreign_key_constraints: vec![],
                ..Default::default()
            })
        );
    }

    #[test]
    fn test_from_json_metadata_fk_with_column_named_primary_key() {
        let metadata = constraints_json(
            "[(fk1,FOREIGN KEY (`primary_key`) REFERENCES `main`.`default`.`users` (`user_id`))]",
        );
        let parsed = DatabricksDescribeJsonMetadata::from_json_metadata(&metadata);
        assert_eq!(
            parsed.ok(),
            Some(DatabricksDescribeJsonMetadata {
                primary_key_constraints: vec![],
                foreign_key_constraints: vec![fk(
                    "fk1",
                    "primary_key",
                    "main",
                    "default",
                    "users",
                    "user_id"
                )],
                ..Default::default()
            })
        );
    }

    // ----------------------------------------------------------------------
    // 2 / 3a — is_describe_as_json_supported gate matrix
    // ----------------------------------------------------------------------

    fn databricks_relation(database: &str, relation_type: RelationType) -> Arc<dyn BaseRelation> {
        Arc::new(
            Relation::new(
                AdapterType::Databricks,
                database.to_string(),
                "some_schema".to_string(),
                "some_table".to_string(),
            )
            .with_relation_type(relation_type),
        )
    }

    const SUPPORTED_CLUSTER: DbrComputeContext =
        DbrComputeContext::Cluster(EngineVersion::Full(17, 3));
    const UNSUPPORTED_CLUSTER: DbrComputeContext =
        DbrComputeContext::Cluster(EngineVersion::Full(17, 2));

    #[test]
    fn test_is_describe_as_json_supported_all_conditions_hold() {
        let relation = databricks_relation("main", RelationType::Table);
        assert!(is_describe_as_json_supported(
            relation.as_ref(),
            false,
            SUPPORTED_CLUSTER,
            true
        ));
    }

    #[test]
    fn test_is_describe_as_json_supported_hive_metastore() {
        let relation = databricks_relation("hive_metastore", RelationType::Table);
        assert!(!is_describe_as_json_supported(
            relation.as_ref(),
            false,
            SUPPORTED_CLUSTER,
            true
        ));
    }

    #[test]
    fn test_is_describe_as_json_supported_foreign_table() {
        let relation = databricks_relation("main", RelationType::Table);
        assert!(!is_describe_as_json_supported(
            relation.as_ref(),
            true,
            SUPPORTED_CLUSTER,
            true
        ));
    }

    #[test]
    fn test_is_describe_as_json_supported_capability_missing() {
        let relation = databricks_relation("main", RelationType::Table);
        assert!(!is_describe_as_json_supported(
            relation.as_ref(),
            false,
            UNSUPPORTED_CLUSTER,
            true
        ));
    }

    #[test]
    fn test_is_describe_as_json_supported_behavior_flag_off() {
        let relation = databricks_relation("main", RelationType::Table);
        assert!(!is_describe_as_json_supported(
            relation.as_ref(),
            false,
            SUPPORTED_CLUSTER,
            false
        ));
    }
}
