/// Name of the [Statement] option that carries the dbt node unique ID.
pub const DBT_NODE_ID: &str = "dbt.node_id";
/// Name of the [Statement] option that carries the dbt execution phase.
pub const DBT_EXECUTION_PHASE: &str = "dbt.execution_phase";
/// Name of the [Statement] option that carries whether the query is for metadata fetch (schema hydration).
pub const DBT_METADATA: &str = "dbt.metadata";
/// Name of the [Statement] option that carries whether the caller expects results
/// (`fetch=true` means a read/SELECT; `fetch=false` typically means DDL/DML).
pub const DBT_FETCH: &str = "dbt.fetch";

/// Name of the [Statement] option that carries the dbt project (package) name, for
/// dbt-compute usage attribution.
pub const DBT_PROJECT: &str = "adbc.dbt.project";
/// Name of the [Statement] option that carries the dbt model (node) name, for
/// dbt-compute usage attribution.
pub const DBT_MODEL: &str = "adbc.dbt.model";
/// Name of the [Statement] option that carries the dbt invocation id, for
/// dbt-compute usage attribution.
pub const DBT_RUN_ID: &str = "adbc.dbt.run_id";
