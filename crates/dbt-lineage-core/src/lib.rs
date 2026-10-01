use dbt_scheduler::node_selector::ColId;
use std::fmt;

/// Flat CLL edge
#[derive(Debug, Clone)]
pub struct CllEdge {
    pub from_node: String,
    pub from_col: String,
    pub to_node: String,
    pub to_col: Option<String>,
    pub op: &'static str,
}

impl CllEdge {
    /// `to_col` treats an empty string as `None`.
    pub fn new(
        from_node: &str,
        from_col: &str,
        to_node: &str,
        to_col: &str,
        op: &'static str,
    ) -> Self {
        Self {
            from_node: from_node.to_string(),
            from_col: from_col.to_string(),
            to_node: to_node.to_string(),
            to_col: if to_col.is_empty() {
                None
            } else {
                Some(to_col.to_string())
            },
            op,
        }
    }
}

/// Grain columns inferred from a logical plan, with the source that produced them.
#[derive(Debug, Clone)]
pub struct PlanGrainInfo {
    /// Column names that form the grain (primary key) of this model.
    pub columns: Vec<String>,
    /// Which plan pattern produced this grain.
    pub source: GrainSource,
}

/// Which logical-plan pattern produced the grain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrainSource {
    /// Outermost GROUP BY columns.
    GroupBy,
    /// SELECT DISTINCT — all projected columns.
    Distinct,
    /// ROW_NUMBER() OVER (PARTITION BY ...) dedup pattern.
    WindowDedup,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColIdWithOp {
    pub col_id: ColId,
    pub op: String,
}
impl fmt::Display for ColIdWithOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.op, self.col_id)
    }
}

impl ColIdWithOp {
    pub fn plain(col_id: ColId) -> Self {
        Self {
            col_id,
            op: "".to_string(),
        }
    }
    pub fn copy(col_id: ColId) -> Self {
        Self {
            col_id,
            op: "copy".to_string(),
        }
    }
    pub fn scan(col_id: ColId) -> Self {
        Self {
            col_id,
            op: "scan".to_string(),
        }
    }
    pub fn modify(col_id: ColId) -> Self {
        Self {
            col_id,
            op: "mod".to_string(),
        }
    }
}
