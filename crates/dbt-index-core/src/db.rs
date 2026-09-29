use std::path::Path;

use adbc_core::error::{Error as AdbcError, Status};
use dbt_adbc::driver::Builder as DriverBuilder;
use dbt_adbc::driver::LoadStrategy;
use dbt_adbc::{Backend, Connection, Database, database};
use std::sync::Arc;

use crate::IndexError;
use crate::format::{cell_to_string, first_nonempty};

/// Tables in the `dbt` schema (snapshot semantics).
pub const DBT_TABLES: &[&str] = &[
    "generation",
    "project",
    "packages",
    "project_vars",
    "project_env_vars",
    "nodes",
    "node_columns",
    "edges",
    "column_lineage",
    "node_input_files",
    "test_metadata",
    "exposures",
    "metrics",
    "groups",
    "macros",
    "semantic_models",
    "semantic_entities",
    "semantic_measures",
    "semantic_dimensions",
    "time_spines",
    "semantic_relationships",
    "saved_queries",
    "unit_tests",
    "docs",
    "catalog_tables",
    "catalog_stats",
    "column_stats",
    "sample_data",
    "source_freshness",
    "context",
    "context_terms",
    "context_links",
];

/// Tables in the `dbt_rt` schema (snapshot — overwritten each invocation).
pub const DBT_RT_TABLES: &[&str] = &[
    "invocations",
    "invocation_nodes",
    "run_results",
    "diagnostics",
    "adapter_queries",
    "test_failures",
];

/// Generate a static `views.sql` covering the full schema.
///
/// Every table always has a parquet file, so no conditional logic is needed —
/// the file is the same regardless of what data was ingested. The content is
/// produced by [`crate::view_defs::views_sql_document`], the canonical
/// definition of the view surface.
pub fn write_views_sql(dir: &Path) -> Result<(), IndexError> {
    std::fs::write(
        dir.join("views.sql"),
        crate::view_defs::views_sql_document(),
    )?;
    Ok(())
}

/// Analytical views — always emitted since all base tables always exist.
pub(crate) const ANALYTICAL_VIEWS: &str = "
CREATE OR REPLACE VIEW dbt_rt.run_results_latest AS
SELECT * FROM dbt_rt.run_results
WHERE NOT (status = 'error' AND execution_time = 0)
QUALIFY ROW_NUMBER() OVER (PARTITION BY unique_id ORDER BY created_at DESC) = 1;

CREATE OR REPLACE VIEW dbt_rt.dag_validity AS
WITH latest AS (
    SELECT r.unique_id, r.status, r.invocation_id, i.generated_at
    FROM dbt_rt.run_results r
    JOIN dbt_rt.invocations i ON r.invocation_id = i.invocation_id
    QUALIFY ROW_NUMBER() OVER (PARTITION BY r.unique_id ORDER BY r.created_at DESC) = 1
)
SELECT
    child.unique_id, child.status,
    child.generated_at AS child_built_at,
    parent.unique_id AS parent_unique_id,
    parent.generated_at AS parent_built_at,
    child.generated_at >= parent.generated_at AS parent_is_fresh
FROM latest child
LEFT JOIN dbt.edges e ON e.child_unique_id = child.unique_id
LEFT JOIN latest parent ON e.parent_unique_id = parent.unique_id;

CREATE OR REPLACE VIEW dbt_rt.node_status AS
SELECT
    n.unique_id, n.name, n.resource_type,
    n.ingested_at AS parsed_at, n.compiled_at,
    r.created_at AS run_at, r.invocation_id AS run_invocation_id,
    r.status AS run_status, r.execution_time AS run_execution_time, r.message AS run_message,
    CASE
        WHEN r.created_at IS NOT NULL AND n.compiled_at IS NOT NULL AND r.created_at >= n.compiled_at THEN 'run'
        WHEN n.compiled_at IS NOT NULL THEN 'compiled'
        ELSE 'parsed'
    END AS effective_phase,
    CASE
        WHEN n.compiled_at IS NOT NULL AND n.ingested_at > n.compiled_at THEN true
        WHEN r.created_at IS NOT NULL AND n.compiled_at IS NOT NULL AND n.compiled_at > r.created_at THEN true
        ELSE false
    END AS is_stale
FROM dbt.nodes n
LEFT JOIN dbt_rt.run_results_latest r ON n.unique_id = r.unique_id;

CREATE OR REPLACE VIEW dbt.nodes_enriched AS
SELECT
    n.*, r.status AS last_run_status, r.execution_time AS last_execution_time,
    r.invocation_id AS last_invocation_id,
    ct.table_type AS catalog_table_type, ct.table_owner AS catalog_owner
FROM dbt.nodes n
LEFT JOIN dbt_rt.run_results_latest r ON n.unique_id = r.unique_id
LEFT JOIN dbt.catalog_tables ct ON n.unique_id = ct.unique_id;

CREATE OR REPLACE VIEW dbt.tests_enriched AS
SELECT
    n.unique_id, n.name AS test_name,
    tm.test_name AS test_type, tm.test_namespace, tm.column_name,
    tm.attached_node, tm.severity, tm.kwargs,
    target.name AS target_node_name, target.resource_type AS target_resource_type,
    r.status AS last_run_status, r.message AS last_run_message, r.failures AS last_run_failures,
    tf.failure_rows
FROM dbt.nodes n
JOIN dbt.test_metadata tm ON n.unique_id = tm.unique_id
LEFT JOIN dbt.nodes target ON tm.attached_node = target.unique_id
LEFT JOIN dbt_rt.run_results_latest r ON n.unique_id = r.unique_id
LEFT JOIN dbt_rt.test_failures tf ON n.unique_id = tf.unique_id AND r.invocation_id = tf.invocation_id
WHERE n.resource_type IN ('test');
";

/// Manages an in-memory DuckDB connection via the ADBC adapter.
///
/// The `shared_database` field allows multiple connections to the same DuckDB
/// catalog via [`Db::new_reader`] — enabling MVCC: readers on separate
/// connections see a consistent committed snapshot and never block writers.
pub struct Db {
    /// Shared database handle — multiple connections can be created from it.
    shared_database: Arc<Box<dyn Database>>,
    connection: Box<dyn Connection>,
}

impl Db {
    /// Opens an in-memory DuckDB instance on the extended driver.
    pub fn open_memory() -> Result<Self, IndexError> {
        Self::open_memory_on(Backend::DuckDBExtended)
    }

    /// Opens an in-memory DuckDB instance on the given driver. The vanilla
    /// driver matters to the DDL-portability test; production paths use
    /// [`Db::open_memory`].
    pub fn open_memory_on(driver_kind: Backend) -> Result<Self, IndexError> {
        let mut driver = DriverBuilder::new(driver_kind, LoadStrategy::SystemThenCdnCache)
            .try_load()
            .map_err(|e| AdbcError::with_message_and_status(e.to_string(), Status::Internal))?;

        let mut db_builder = database::Builder::new(driver_kind);
        db_builder
            .with_named_option("path", ":memory:")
            .map_err(|e| AdbcError::with_message_and_status(e.to_string(), Status::Internal))?;

        let database = db_builder.build(&mut driver)?;
        let connection = database.new_connection()?;

        let mut db = Self {
            shared_database: Arc::new(database),
            connection,
        };
        db.disable_checkpoint()?;
        Ok(db)
    }

    /// Opens (or creates) a DuckDB file at `path` and returns a ready connection.
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        let mut driver =
            DriverBuilder::new(Backend::DuckDBExtended, LoadStrategy::SystemThenCdnCache)
                .try_load()
                .map_err(|e| AdbcError::with_message_and_status(e.to_string(), Status::Internal))?;

        let mut db_builder = database::Builder::new(Backend::DuckDBExtended);
        db_builder
            .with_named_option("path", path.to_string_lossy().as_ref())
            .map_err(|e| AdbcError::with_message_and_status(e.to_string(), Status::Internal))?;

        let database = db_builder.build(&mut driver)?;
        let connection = database.new_connection()?;

        let mut db = Self {
            shared_database: Arc::new(database),
            connection,
        };
        db.disable_checkpoint()?;
        Ok(db)
    }

    /// Create a new read-only connection to the **same** DuckDB database.
    ///
    /// Shares the catalog via DuckDB MVCC: sees the last committed snapshot,
    /// never blocks writers, multiple readers can run concurrently.
    pub fn new_reader(&self) -> Result<Self, IndexError> {
        let connection = self.shared_database.new_connection()?;
        Ok(Self {
            shared_database: self.shared_database.clone(),
            connection,
        })
    }

    /// Executes a DDL/DML statement that returns no rows.
    pub fn execute_update(&mut self, sql: &str) -> Result<Option<i64>, IndexError> {
        let conn = &mut self.connection;
        let mut stmt = conn.new_statement()?;
        stmt.set_sql_query(sql)?;
        Ok(stmt.execute_update()?)
    }

    /// Suppress DuckDB's automatic checkpointing. For in-memory databases this
    /// should already be a no-op, but `read_parquet()` scans can trick the buffer
    /// manager into scheduling checkpoints whose internal pthread mutex then races
    /// with catalog DDL.
    fn disable_checkpoint(&mut self) -> Result<(), IndexError> {
        self.execute_update("SET wal_autocheckpoint = '0 bytes'")?;
        self.execute_update("PRAGMA disable_checkpoint_on_shutdown")?;
        Ok(())
    }

    /// Query a single scalar from column `col_idx` of the first row.
    pub fn query_scalar(&mut self, sql: &str, col_idx: usize) -> Option<String> {
        self.execute_query(sql)
            .ok()
            .and_then(|b| first_nonempty(&b).map(|batch| cell_to_string(batch, 0, col_idx)))
    }

    /// Execute a `SELECT count(*)` query and return the scalar count as a string.
    pub fn query_count(&mut self, sql: &str) -> String {
        self.execute_query(sql)
            .ok()
            .and_then(|b| b.into_iter().find(|b| b.num_rows() > 0))
            .map(|b| cell_to_string(&b, 0, 0))
            .unwrap_or_else(|| "0".to_string())
    }

    /// Executes a query and collects all resulting record batches.
    pub fn execute_query(
        &mut self,
        sql: &str,
    ) -> Result<Vec<arrow_array::RecordBatch>, IndexError> {
        let conn = &mut self.connection;
        let mut stmt = conn.new_statement()?;
        stmt.set_sql_query(sql)?;
        let reader = stmt.execute()?;
        let mut batches = Vec::new();
        for batch in reader {
            batches.push(batch.map_err(|e| {
                AdbcError::with_message_and_status(e.to_string(), Status::Internal)
            })?);
        }
        Ok(batches)
    }
}
