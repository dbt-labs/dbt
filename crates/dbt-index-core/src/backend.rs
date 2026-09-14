//! Read-only abstraction over the parquet artifact set.
//!
//! `Backend` is the gated "untyped SQL access" capability the docs server
//! uses to power its non-feature endpoints (node listing, project info,
//! catalog stats, etc.). The trait surface lives in this OSS crate; the
//! real DuckDB-backed implementation lives in proprietary `dbt-index`
//! and is injected by `dbt-cli` at startup. Without an injected impl,
//! [`UnavailableBackend`] is the default and every method reports the
//! feature is unavailable so callers can render a PLG upsell rather than
//! crashing.
//!
//! Methods are synchronous; HTTP handlers should call them from inside
//! `tokio::task::spawn_blocking` so they don't stall the async runtime.
//!
//! **Streaming:** today this trait collects all batches into a `Vec` before
//! returning. When endpoints whose result sets can grow large land
//! (column-lineage graph, full edge dump), add a sibling
//! `query_arrow_stream` returning a `RecordBatchReader` so handlers can
//! pipe batches directly into the HTTP response body without buffering.
//!
//! Opens an in-memory DuckDB and registers the canonical view surface
//! (see [`crate::view_defs`]) over `index_dir`: one base view per parquet
//! file present, plus the derived views whose prerequisites registered.
//! No inserts. Used by the in-binary docs server for untyped SQL access
//! (node listings, project info, etc.) and reused by the typed feature
//! providers in `crate::providers::*` so a single DuckDB connection
//! backs every gated capability.
//!
//! Empty/unreadable parquet files are skipped rather than failing
//! startup — capability detection is the job of [`Backend::table_has_rows`].

use arrow_array::RecordBatch;

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error(
        "index backend is not available; rerun `dbt --use-index <run|build|compile|parse>` and ensure the proprietary distribution is installed"
    )]
    NotAvailable,
    #[error("query failed: {0}")]
    Query(String),
    #[error("invalid result shape: {0}")]
    Shape(String),
}

/// SQL access over the dbt parquet index.
///
/// Default impls report "not available" so an empty impl is a valid
/// no-op stub; see [`UnavailableBackend`]. The proprietary distribution
/// overrides every method.
pub trait Backend: Send + Sync {
    /// Whether this backend is wired to a real data source. Hosts use
    /// this for PLG gating before calling [`query_arrow`] etc.
    fn is_available(&self) -> bool {
        false
    }

    /// Whether the named fully-qualified parquet table exists and has rows.
    /// Used for capability detection at server startup.
    fn table_has_rows(&self, _table: &str) -> bool {
        false
    }

    /// Execute a query that returns a single scalar in column 0 of row 0.
    /// Returns `None` if the query produces no rows, fails, or the
    /// backend is unavailable.
    fn query_scalar(&self, _sql: &str) -> Option<String> {
        None
    }

    /// Execute a query and return all result batches as Arrow `RecordBatch`es.
    /// Bounded results only — see streaming note in the module docs.
    fn query_arrow(&self, _sql: &str) -> Result<Vec<RecordBatch>, BackendError> {
        Err(BackendError::NotAvailable)
    }

    /// Execute a statement that returns no rows — DDL, view registration.
    ///
    /// Registration and reads must go through the *same* instance: a DuckDB
    /// view exists only on the connection that created it, so an executor
    /// that registers views is the one that can query them afterwards.
    fn execute(&self, _sql: &str) -> Result<(), BackendError> {
        Err(BackendError::NotAvailable)
    }
}

/// No-op default. Inherits the trait's defaults (everything reports
/// unavailable). Use behind `Arc<dyn Backend>` in an injection bundle
/// when the proprietary impl isn't wired in.
pub struct UnavailableBackend;

impl Backend for UnavailableBackend {}

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::db::Db;
use crate::view_defs;

pub struct DuckDbViewsBackend {
    inner: Mutex<Db>,
    index_dir: PathBuf,
}

impl DuckDbViewsBackend {
    pub fn open(index_dir: &Path) -> Result<Self, BackendError> {
        if !index_dir.exists() {
            return Err(BackendError::Query(format!(
                "index directory does not exist: {}\n\n\
                 Run `dbt build` or `dbt docs generate` to generate parquet artifacts, \
                 or pass --target-path <DIR> pointing at a directory whose `private/index/` subdirectory contains them.",
                index_dir.display()
            )));
        }
        let db = Db::open_memory().map_err(|e| BackendError::Query(e.to_string()))?;
        let backend = Self {
            inner: Mutex::new(db),
            index_dir: index_dir.to_path_buf(),
        };
        register_index_views(&backend, index_dir)?;
        Ok(backend)
    }

    pub fn index_dir(&self) -> &Path {
        &self.index_dir
    }
}

impl Backend for DuckDbViewsBackend {
    fn is_available(&self) -> bool {
        true
    }

    fn table_has_rows(&self, table: &str) -> bool {
        let Ok(mut db) = self.inner.lock() else {
            return false;
        };
        db.query_count(&format!("SELECT COUNT(*) FROM {table}")) != "0"
    }

    fn query_scalar(&self, sql: &str) -> Option<String> {
        let mut db = self.inner.lock().ok()?;
        db.query_scalar(sql, 0)
    }

    fn query_arrow(&self, sql: &str) -> Result<Vec<RecordBatch>, BackendError> {
        let mut db = self
            .inner
            .lock()
            .map_err(|e| BackendError::Query(format!("db lock poisoned: {e}")))?;
        db.execute_query(sql)
            .map_err(|e| BackendError::Query(e.to_string()))
    }

    fn execute(&self, sql: &str) -> Result<(), BackendError> {
        let mut db = self
            .inner
            .lock()
            .map_err(|e| BackendError::Query(format!("db lock poisoned: {e}")))?;
        db.execute_update(sql)
            .map(|_| ())
            .map_err(|e| BackendError::Query(e.to_string()))
    }
}

/// Outcome of a registration pass over an index directory.
///
/// A skipped view is *absent*, never an empty stand-in: an absent view fails
/// loudly at query time, while an empty one returns zero rows, which a check
/// would report as a pass.
#[derive(Debug, Default)]
pub struct RegistrationReport {
    /// Schema-qualified names created on the executor's connection, in order.
    pub registered: Vec<String>,
    /// Schema-qualified names not created, each with the reason.
    pub skipped: Vec<(String, String)>,
}

/// Register the canonical view surface (see [`crate::view_defs`]) over
/// `index_dir`, through `executor`.
///
/// The one registration path shared by every consumer. Semantics:
/// - a base table whose parquet file is not on disk is skipped, not stubbed —
///   an absent view fails loudly at query time, an empty one silently passes
///   a check;
/// - a per-view execution failure (stale parquet, a derived view whose base
///   did not register) skips that view without taking down the rest.
pub fn register_index_views(
    executor: &dyn Backend,
    index_dir: &Path,
) -> Result<RegistrationReport, BackendError> {
    // An unreadable directory must fail loudly here, not degrade into a
    // backend that reports available with zero views. Per-file `exists()`
    // checks below cannot distinguish "missing" from "unreadable", so probe
    // the directory itself first.
    std::fs::read_dir(index_dir).map_err(|e| {
        BackendError::Query(format!(
            "index directory is not readable: {}: {e}",
            index_dir.display()
        ))
    })?;

    executor.execute("CREATE SCHEMA IF NOT EXISTS dbt")?;
    executor.execute("CREATE SCHEMA IF NOT EXISTS dbt_rt")?;

    let mut report = RegistrationReport::default();
    for table in view_defs::base_tables() {
        let qualified = format!("{}.{}", table.schema, table.name);
        if !index_dir.join(format!("{qualified}.parquet")).exists() {
            report
                .skipped
                .push((qualified, "parquet file not present".to_string()));
            continue;
        }
        let sql = view_defs::base_view_statement(table, Some(index_dir));
        match executor.execute(&sql) {
            Ok(()) => report.registered.push(qualified),
            Err(e) => report.skipped.push((qualified, e.to_string())),
        }
    }
    // Derived views bind at creation, so one whose base is missing fails
    // there and lands in `skipped` with the binder's reason. Creation order
    // in DERIVED_VIEWS puts prerequisites first.
    for (view, statement) in view_defs::derived_statements() {
        let qualified = format!("{}.{}", view.schema, view.name);
        match executor.execute(statement) {
            Ok(()) => report.registered.push(qualified),
            Err(e) => report.skipped.push((qualified, e.to_string())),
        }
    }
    Ok(report)
}

/// An in-memory DuckDB over the *information schema* at
/// `target/info_schema/v<n>/`, rather than over the flat index.
///
/// Same `Backend` surface, different artifact set and different vocabulary:
/// one table per resource type, `dbt.docs_blocks` rather than `dbt.docs`,
/// `dbt_rt.freshness` rather than `dbt.source_freshness`, and no `dbt.nodes`
/// at all. A caller swaps the constructor and nothing else.
pub struct DuckDbInfoSchemaBackend {
    inner: Mutex<Db>,
    info_schema_dir: PathBuf,
}

impl DuckDbInfoSchemaBackend {
    /// `info_schema_dir` is the *versioned* directory — the one holding the
    /// parquet and `views.sql`. Resolve it with
    /// [`crate::info_schema::versioned_dir`] rather than composing the version
    /// into a path by hand.
    pub fn open(info_schema_dir: &Path) -> Result<Self, BackendError> {
        if !info_schema_dir.exists() {
            return Err(BackendError::Query(format!(
                "information schema directory does not exist: {}\n\n\
                 Run `dbt build`, `dbt compile` or `dbt docs generate` with \
                 `--generate-info-schema` to write it.",
                info_schema_dir.display()
            )));
        }
        let db = Db::open_memory().map_err(|e| BackendError::Query(e.to_string()))?;
        let backend = Self {
            inner: Mutex::new(db),
            info_schema_dir: info_schema_dir.to_path_buf(),
        };
        register_info_schema_views(&backend, info_schema_dir)?;
        Ok(backend)
    }

    pub fn info_schema_dir(&self) -> &Path {
        &self.info_schema_dir
    }
}

impl Backend for DuckDbInfoSchemaBackend {
    fn is_available(&self) -> bool {
        true
    }

    fn table_has_rows(&self, table: &str) -> bool {
        let Ok(mut db) = self.inner.lock() else {
            return false;
        };
        db.query_count(&format!("SELECT COUNT(*) FROM {table}")) != "0"
    }

    fn query_scalar(&self, sql: &str) -> Option<String> {
        let mut db = self.inner.lock().ok()?;
        db.query_scalar(sql, 0)
    }

    fn query_arrow(&self, sql: &str) -> Result<Vec<RecordBatch>, BackendError> {
        let mut db = self
            .inner
            .lock()
            .map_err(|e| BackendError::Query(format!("db lock poisoned: {e}")))?;
        db.execute_query(sql)
            .map_err(|e| BackendError::Query(e.to_string()))
    }

    fn execute(&self, sql: &str) -> Result<(), BackendError> {
        let mut db = self
            .inner
            .lock()
            .map_err(|e| BackendError::Query(format!("db lock poisoned: {e}")))?;
        db.execute_update(sql)
            .map(|_| ())
            .map_err(|e| BackendError::Query(e.to_string()))
    }
}

/// Register the information schema's view surface (see
/// [`crate::info_schema::views`]) over `info_schema_dir`, through `executor`.
///
/// The `views.sql` counterpart of [`register_index_views`], with the same
/// semantics: an absent parquet file is skipped rather than stubbed empty, and
/// a per-view failure skips that view without taking down the rest.
///
/// Unlike the adapter-backed walk the checks gate uses, this registers
/// `dbt_internal` too. The docs site is the consumer that needs
/// `dbt_internal.resources`, and it executes the whole shipped document
/// anyway; a check, which runs user SQL, deliberately cannot see that
/// namespace.
pub fn register_info_schema_views(
    executor: &dyn Backend,
    info_schema_dir: &Path,
) -> Result<RegistrationReport, BackendError> {
    use crate::info_schema::spec::Ns;
    use crate::info_schema::{schema::INFO_SCHEMA, views};

    // An unreadable directory fails loudly here rather than degrading into a
    // backend that reports available with zero views: the per-file `exists()`
    // checks below cannot tell "missing" from "unreadable".
    std::fs::read_dir(info_schema_dir).map_err(|e| {
        BackendError::Query(format!(
            "information schema directory is not readable: {}: {e}",
            info_schema_dir.display()
        ))
    })?;

    for ns in Ns::ALL {
        executor.execute(&format!("CREATE SCHEMA IF NOT EXISTS {}", ns.prefix()))?;
    }

    let mut report = RegistrationReport::default();
    for table in INFO_SCHEMA {
        let qualified = table.qualified_name();
        if !info_schema_dir.join(table.file_name()).exists() {
            report
                .skipped
                .push((qualified, "parquet file not present".to_string()));
            continue;
        }
        match executor.execute(&views::base_view_statement(table, Some(info_schema_dir))) {
            Ok(()) => report.registered.push(qualified),
            Err(e) => report.skipped.push((qualified, e.to_string())),
        }
    }
    // Derived views bind at creation, so one whose base is missing fails there
    // and lands in `skipped` with the binder's reason. `derived_statements` is
    // ordered so prerequisites come first.
    for (qualified, statement) in views::derived_statements() {
        match executor.execute(&statement) {
            Ok(()) => report.registered.push(qualified),
            Err(e) => report.skipped.push((qualified, e.to_string())),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod registration_tests {
    use super::*;

    /// Records every executed statement; fails any whose SQL contains a
    /// configured marker, to exercise per-view tolerance.
    struct FakeExecutor {
        executed: Mutex<Vec<String>>,
        fail_marker: Option<&'static str>,
    }

    impl FakeExecutor {
        fn new(fail_marker: Option<&'static str>) -> Self {
            Self {
                executed: Mutex::new(Vec::new()),
                fail_marker,
            }
        }
    }

    impl Backend for FakeExecutor {
        fn execute(&self, sql: &str) -> Result<(), BackendError> {
            if let Some(marker) = self.fail_marker {
                if sql.contains(marker) {
                    return Err(BackendError::Query(format!("injected failure: {marker}")));
                }
            }
            self.executed.lock().unwrap().push(sql.to_string());
            Ok(())
        }
    }

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(format!("{name}.parquet")), b"").unwrap();
    }

    /// Only parquet actually on disk gets a base view; everything else is
    /// reported skipped with a reason, never stubbed empty. Base-view paths
    /// are absolute.
    #[test]
    fn missing_files_are_skipped_not_stubbed() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(tmp.path(), "dbt.nodes");
        touch(tmp.path(), "dbt.edges");

        let exec = FakeExecutor::new(None);
        let report = register_index_views(&exec, tmp.path()).unwrap();

        assert!(report.registered.contains(&"dbt.nodes".to_string()));
        assert!(report.registered.contains(&"dbt.edges".to_string()));
        assert!(
            report
                .skipped
                .iter()
                .any(|(n, r)| n == "dbt.node_columns" && r.contains("not present"))
        );

        let executed = exec.executed.lock().unwrap();
        let base_views: Vec<&String> = executed
            .iter()
            .filter(|s| s.contains("read_parquet"))
            .collect();
        // Exactly the two present files got base views — nothing stubbed.
        assert_eq!(base_views.len(), 2);
        // Absolute paths: the executor's working directory is not the index
        // directory.
        let dir = view_defs::quote_sql_string(&tmp.path().to_string_lossy());
        for sql in base_views {
            assert!(sql.contains(&dir), "expected absolute path in: {sql}");
        }
    }

    /// A single failing view must not take down the rest of the surface.
    #[test]
    fn per_view_failure_is_tolerated() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(tmp.path(), "dbt.nodes");

        // Marker matches only the CREATE statement, not other views' bodies
        // that reference this view in a JOIN.
        let exec = FakeExecutor::new(Some("VIEW dbt_rt.run_results_latest AS"));
        let report = register_index_views(&exec, tmp.path()).unwrap();

        assert!(
            report
                .skipped
                .iter()
                .any(|(n, r)| n == "dbt_rt.run_results_latest" && r.contains("injected failure"))
        );
        // Registration continued past the failure.
        assert!(
            report
                .registered
                .contains(&"dbt.nodes_enriched".to_string())
        );
    }

    /// The information-schema pass registers the shipped document: a base view
    /// per parquet present, `dbt_internal` included, and the derived views on
    /// top. Same skip-don't-stub rule as the index pass.
    #[test]
    fn info_schema_registers_the_shipped_document() {
        let tmp = tempfile::TempDir::new().unwrap();
        touch(tmp.path(), "dbt.models");
        touch(tmp.path(), "dbt.seeds");
        touch(tmp.path(), "dbt_internal.node_input_files");

        let exec = FakeExecutor::new(None);
        let report = register_info_schema_views(&exec, tmp.path()).unwrap();

        for present in ["dbt.models", "dbt.seeds", "dbt_internal.node_input_files"] {
            assert!(
                report.registered.contains(&present.to_string()),
                "{present} should be registered"
            );
        }
        assert!(
            report
                .skipped
                .iter()
                .any(|(n, r)| n == "dbt.edges" && r.contains("not present")),
            "an absent parquet is skipped, not stubbed"
        );
        // Both derived views are attempted, and the union is the one this
        // consumer exists for.
        assert!(
            report
                .registered
                .contains(&"dbt_internal.resources".to_string())
        );
        assert!(
            report
                .registered
                .contains(&"dbt_rt.run_results_latest".to_string())
        );

        let executed = exec.executed.lock().unwrap();
        // `dbt_internal` is declared here, unlike on the index pass.
        assert!(
            executed
                .iter()
                .any(|s| s == "CREATE SCHEMA IF NOT EXISTS dbt_internal")
        );
        // Absolute paths: the executor's working directory is not the artifact
        // directory.
        let dir = view_defs::quote_sql_string(&tmp.path().to_string_lossy());
        for sql in executed.iter().filter(|s| s.contains("read_parquet")) {
            assert!(sql.contains(&dir), "expected absolute path in: {sql}");
        }
        // The index vocabulary must not leak in.
        assert!(
            !executed.iter().any(|s| s.contains("VIEW dbt.nodes AS")),
            "the information schema has no dbt.nodes"
        );
    }
}
