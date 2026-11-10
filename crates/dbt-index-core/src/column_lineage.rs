//! Column-level lineage feature surface.
//!
//! Single-hop column lineage — every direct edge in `dbt.column_lineage`
//! that touches the requested node (and optionally a specific column).
//! Multi-hop downstream traversal for a column is a *different* surface;
//! see [`super::column_impact`].
//!
//! The proprietary `DuckDbColumnLineageProvider` lives in `dbt-index`;
//! this crate ships only the abstract `Args` / `Output` types and a
//! reuses [`super::UnavailableProvider`] as the no-op default.

use serde::{Deserialize, Serialize};

use crate::provider::{Provider, ProviderOutput, UnavailableProvider};

/// Inputs to the column lineage query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnLineageArgs {
    /// Node `unique_id` whose lineage we want.
    pub unique_id: String,
    /// Specific column on `unique_id`. `None` = every column of the
    /// node — useful for "show me all column edges touching this model"
    /// (single-hop bidirectional).
    pub column: Option<String>,
    /// Include edges where the node is on the receiving side
    /// (`to_node_unique_id = unique_id`). Default when neither
    /// upstream nor downstream is requested: both directions.
    pub upstream: bool,
    /// Include edges where the node is on the producing side
    /// (`from_node_unique_id = unique_id`). Default when neither
    /// upstream nor downstream is requested: both directions.
    pub downstream: bool,
}

/// One column-to-column lineage edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnLineageEdge {
    pub from_node: String,
    pub from_column: String,
    pub to_node: String,
    pub to_column: String,
    /// Free-form classification — `"copy"`, `"passthrough"`, `"transform"`,
    /// `"join_key"`, etc. Mirrors the `lineage_kind` column in the
    /// underlying parquet table.
    pub kind: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LineageError {
    #[error("column lineage is not available; rerun `dbt build --static-analysis strict`")]
    NotAvailable,
    #[error("node {0} not found")]
    NodeNotFound(String),
    #[error(
        "column '{column}' not found in lineage for node '{node}'; available columns: {available}"
    )]
    ColumnNotFound {
        node: String,
        column: String,
        /// Comma-separated list of columns that *do* have lineage data
        /// for this node — surfaced so the caller can suggest one.
        available: String,
    },
    #[error("lineage backend error: {0}")]
    Backend(String),
}

/// Blanket impl: every `Result<_, LineageError>` is a valid `Provider`
/// output, so `UnavailableProvider<_, Result<_, LineageError>>` works for
/// any lineage-flavoured feature without a per-feature impl.
impl<T> ProviderOutput for Result<T, LineageError> {
    fn unavailable() -> Self {
        Err(LineageError::NotAvailable)
    }
}

pub type ColumnLineageProvider =
    dyn Provider<Args = ColumnLineageArgs, Output = Result<Vec<ColumnLineageEdge>, LineageError>>;

pub type UnavailableColumnLineage =
    UnavailableProvider<ColumnLineageArgs, Result<Vec<ColumnLineageEdge>, LineageError>>;
