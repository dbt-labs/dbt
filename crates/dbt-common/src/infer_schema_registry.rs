//! Cross-model registry for `--infer-schemas`: records, for each relation
//! referenced by the SQL being bound, which columns were confidently
//! inferred from usage. Shared across every model bound in a run.
//!
//! Plan-tree-based column lineage (`dbt-lineage`) can't see these columns on
//! its own: an inferred column becomes an opaque placeholder in the bound
//! plan, with no real column reference left to trace back to its source.
//! This registry is the only place that still has that information,
//! captured at the moment inference happens. Lineage computation reads it
//! back (keyed by the relation name recorded as metadata on the
//! placeholder) to fill placeholders back in before walking the plan.

use crate::collections::{DashMap, HashSet};

#[derive(Debug, Default)]
pub struct InferSchemaRegistry {
    inferred_columns: DashMap<String, HashSet<String>>,
}

impl InferSchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `column` was confidently inferred to belong to `relation`.
    pub fn record_column(&self, relation: &str, column: &str) {
        self.inferred_columns
            .entry(relation.to_string())
            .or_default()
            .insert(column.to_string());
    }

    /// Columns previously recorded for `relation`, if any.
    pub fn columns_for(&self, relation: &str) -> Option<HashSet<String>> {
        self.inferred_columns.get(relation).map(|c| c.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_read_back_a_column() {
        let registry = InferSchemaRegistry::new();
        registry.record_column("a", "c");
        assert!(registry.columns_for("a").unwrap().contains("c"));
    }

    #[test]
    fn columns_for_unknown_relation_is_none() {
        let registry = InferSchemaRegistry::new();
        assert!(registry.columns_for("a").is_none());
    }

    #[test]
    fn multiple_columns_accumulate_for_the_same_relation() {
        let registry = InferSchemaRegistry::new();
        registry.record_column("a", "c1");
        registry.record_column("a", "c2");
        let columns = registry.columns_for("a").unwrap();
        assert!(columns.contains("c1"));
        assert!(columns.contains("c2"));
    }

    #[test]
    fn columns_are_scoped_to_their_own_relation() {
        let registry = InferSchemaRegistry::new();
        registry.record_column("a", "c");
        assert!(registry.columns_for("b").is_none());
    }

    #[test]
    fn recording_the_same_column_twice_is_idempotent() {
        let registry = InferSchemaRegistry::new();
        registry.record_column("a", "c");
        registry.record_column("a", "c");
        assert_eq!(registry.columns_for("a").unwrap().len(), 1);
    }
}
