//! Column-level downstream-impact feature surface.
//!
//! Multi-hop BFS through `dbt.column_lineage` starting at a specific
//! `(node, column)` pair, walking downstream until the reachable set
//! stops growing. Returns every `(node, column, distance)` triple the
//! source column flows into. Distinct from [`super::column_lineage`],
//! which is single-hop and supports both directions but doesn't walk
//! transitively.
//!
//! The proprietary `DuckDbColumnImpactProvider` lives in `dbt-index`;
//! this crate ships only the abstract `Args` / `Output` types and
//! reuses [`super::UnavailableProvider`] as the no-op default.

use serde::{Deserialize, Serialize};

use crate::column_lineage::LineageError;
use crate::provider::{Provider, UnavailableProvider};

/// Inputs to the column impact query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnImpactArgs {
    /// Source node `unique_id`.
    pub unique_id: String,
    /// Source column on `unique_id`. Required — impact is always
    /// rooted at a specific column.
    pub column: String,
}

/// One node downstream of the source column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnImpactNode {
    /// Downstream node that depends on the source column.
    pub node_unique_id: String,
    /// The column in that node that depends on the source.
    pub column_name: String,
    /// Number of hops from the source column. `1` = direct dependent,
    /// `2` = depends on a direct dependent, etc.
    pub distance: u32,
}

pub type ColumnImpactProvider =
    dyn Provider<Args = ColumnImpactArgs, Output = Result<Vec<ColumnImpactNode>, LineageError>>;

pub type UnavailableColumnImpact =
    UnavailableProvider<ColumnImpactArgs, Result<Vec<ColumnImpactNode>, LineageError>>;
