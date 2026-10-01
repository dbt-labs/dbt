//! Public types for view-definition fetching.
//!
//! `ViewDefinition` is the per-view payload that the adapter's
//! `fetch_view_definitions` returns. Recursive traversal that parses these
//! definitions to discover upstream references is performed by callers.

use std::collections::BTreeSet;

use dbt_adapter_core::AdapterType;

/// A single fetched view definition.
#[derive(Debug, Clone)]
pub struct ViewDefinition {
    /// Fully-qualified, quoted name of the view.
    pub fqn: String,

    /// Verbatim DDL as returned by the adapter. For Snowflake this is the
    /// output of `GET_DDL('VIEW', <fqn>)` (or the user-supplied override).
    pub definition: String,

    /// SQL dialect to use when parsing `definition`.
    pub dialect: AdapterType,

    /// Catalog used to qualify any unqualified references inside `definition`.
    pub default_catalog: String,

    /// Schema used to qualify any unqualified references inside `definition`.
    pub default_schema: String,
}

/// Result of fetching view definitions for a batch of relations.
#[derive(Debug, Clone, Default)]
pub struct ViewDefinitionFetchResult {
    /// Definitions fetched for inputs that are views.
    pub definitions: Vec<ViewDefinition>,

    /// Fully-qualified relation names whose view definition could not be fetched.
    pub unresolvable: BTreeSet<String>,
}
