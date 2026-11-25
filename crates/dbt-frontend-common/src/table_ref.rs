//! Where a statement's table references sit in its text.

/// A table reference in a `FROM` clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    /// Byte offset where the table reference starts (inclusive).
    pub start: usize,
    /// Byte offset where the table reference ends (exclusive).
    pub stop: usize,
    /// The last segment of the table name as written, when the reference has no
    /// alias of its own. Replacing the reference with a subquery under this
    /// alias keeps qualified column references such as `my_table.col_a`
    /// resolving.
    pub implicit_alias: Option<String>,
}
