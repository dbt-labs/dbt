//! The [SourcesExtractor] trait: parse a SQL statement and return the upstream
//! table references it depends on.

use std::collections::BTreeSet;

use dbt_adapter_core::AdapterType;
use dbt_frontend_common::{
    FullyQualifiedName, error::FrontendResult, named_reference::NamedReference,
};

/// What an extraction found upstream of a statement.
#[derive(Debug, Clone, Default)]
pub struct ExtractedUpstreams {
    /// Upstream table references.
    pub tables: Vec<NamedReference<FullyQualifiedName>>,
    /// Names of table-valued functions in FROM, as written, other than pure
    /// row generators such as `range` (see `dbt_frontend_common::table_functions::is_pure_generator`).
    pub table_functions: BTreeSet<String>,
}

impl From<Vec<NamedReference<FullyQualifiedName>>> for ExtractedUpstreams {
    fn from(tables: Vec<NamedReference<FullyQualifiedName>>) -> Self {
        Self {
            tables,
            ..Default::default()
        }
    }
}

pub trait SourcesExtractor: Send + Sync {
    /// Parse `sql` and return the upstream table references, qualified
    /// against `default_catalog`/`default_schema`. In-scope CTE aliases are
    /// excluded.
    fn extract_upstreams(
        &self,
        adapter_type: AdapterType,
        sql: &str,
        default_catalog: &str,
        default_schema: &str,
        quoted_name_ignore_case: bool,
    ) -> FrontendResult<ExtractedUpstreams>;

    /// Parse `sql` as a complete standalone SQL expression and return its
    /// upstream table references. Custom materializations may wrap model SQL
    /// expressions in executable statements, so expression subqueries still
    /// need dependency extraction even though the expression is not itself a
    /// full SQL statement.
    fn extract_standalone_expression_upstreams(
        &self,
        adapter_type: AdapterType,
        sql: &str,
        default_catalog: &str,
        default_schema: &str,
        quoted_name_ignore_case: bool,
    ) -> FrontendResult<ExtractedUpstreams>;
}
