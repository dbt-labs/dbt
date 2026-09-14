//! Cross-model registry for user-declared (YAML `columns:`) schema — ground
//! truth the project already states, independent of anything learned from
//! usage or inference.
//!
//! Distinct from `InferSchemaRegistry`: that one records columns *inferred*
//! from how a relation is used during binding; this one records columns the
//! project *already declares*, before binding ever runs. A `CatalogProviderList`
//! wrapper (see `dbt-tasks`) consults this registry to let an otherwise-open
//! relation resolve its declared columns through the binder's normal
//! catalog-lookup path, rather than falling through to placeholder inference.

use crate::collections::{DashMap, HashSet};

#[derive(Debug, Default)]
pub struct UserDefinedSchemaRegistry {
    declared_columns: DashMap<String, HashSet<String>>,
}

impl UserDefinedSchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `column` is declared for `relation`.
    pub fn record_column(&self, relation: &str, column: &str) {
        self.declared_columns
            .entry(relation.to_string())
            .or_default()
            .insert(column.to_string());
    }

    /// Declared columns previously recorded for `relation`, if any.
    pub fn columns_for(&self, relation: &str) -> Option<HashSet<String>> {
        self.declared_columns.get(relation).map(|c| c.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_read_back_a_column() {
        let registry = UserDefinedSchemaRegistry::new();
        registry.record_column("a", "c");
        assert!(registry.columns_for("a").unwrap().contains("c"));
    }

    #[test]
    fn columns_for_unknown_relation_is_none() {
        let registry = UserDefinedSchemaRegistry::new();
        assert!(registry.columns_for("a").is_none());
    }

    #[test]
    fn multiple_columns_accumulate_for_the_same_relation() {
        let registry = UserDefinedSchemaRegistry::new();
        registry.record_column("a", "c1");
        registry.record_column("a", "c2");
        let columns = registry.columns_for("a").unwrap();
        assert!(columns.contains("c1"));
        assert!(columns.contains("c2"));
    }

    #[test]
    fn columns_are_scoped_to_their_own_relation() {
        let registry = UserDefinedSchemaRegistry::new();
        registry.record_column("a", "c");
        assert!(registry.columns_for("b").is_none());
    }

    #[test]
    fn recording_the_same_column_twice_is_idempotent() {
        let registry = UserDefinedSchemaRegistry::new();
        registry.record_column("a", "c");
        registry.record_column("a", "c");
        assert_eq!(registry.columns_for("a").unwrap().len(), 1);
    }
}
