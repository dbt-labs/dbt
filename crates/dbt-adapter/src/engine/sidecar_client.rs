//! SidecarClient trait for subprocess-based adapter execution. See [`SidecarClient`] for
//! the design rationale.

use std::fmt::Debug;

use arrow::record_batch::RecordBatch;
use dbt_adbc::{Connection, QueryCtx};
use dbt_schemas::{dbt_types::RelationType, schemas::common::ResolvedQuoting};
use minijinja::State;

use crate::errors::AdapterResult;

/// Column information returned by sidecar introspection.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    /// Column name
    pub name: String,
    /// Column data type (backend-specific, e.g., DuckDB types)
    pub data_type: String,
}

/// Delegation interface for adapters that route execution to a sidecar subprocess or
/// HTTP service (e.g. dbt-db-runner), letting Snowflake (and others) run against DuckDB
/// or other engines without pulling proprietary implementation details into SA crates.
/// SA defines the trait only; closed-source implements it, typically by wrapping a
/// subprocess manager or HTTP client, translating calls into its task message protocol,
/// and owning subprocess lifecycle, error recovery, and session/state-directory isolation.
/// Each method call is self-contained (no hidden state), though a client may pool
/// connections internally.
pub trait SidecarClient: Debug + Send + Sync {
    /// `fetch = false` returns `None` without reading rows, which is the fast path for
    /// DDL/DML that has no result set.
    fn execute(&self, ctx: &QueryCtx, sql: &str, fetch: bool)
    -> AdapterResult<Option<RecordBatch>>;

    /// Connections from the same client may share session state but have independent
    /// transaction scope.
    fn new_connection(
        &self,
        state: Option<&State>,
        node_id: Option<String>,
    ) -> AdapterResult<Box<dyn Connection>>;

    /// Should be called when the adapter is dropped or the dbt run completes.
    fn shutdown(&self) -> AdapterResult<()>;

    /// `database`/`schema`/`table` are case-sensitive for DuckDB.
    fn get_relation_type(
        &self,
        quoting: &ResolvedQuoting,
        database: &str,
        schema: &str,
        table: &str,
    ) -> AdapterResult<Option<RelationType>>;

    fn get_columns(&self, relation_name: &str) -> AdapterResult<Vec<ColumnInfo>>;

    fn list_relations(
        &self,
        quoting: &ResolvedQuoting,
        database: &str,
        schema: &str,
    ) -> AdapterResult<Vec<(String, String, String, RelationType)>>;
}
