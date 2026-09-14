//! Source-available feature trait surfaces for `dbt-index`.
//!
//! This crate defines the **abstract interfaces** for features whose
//! implementation lives in the proprietary `dbt-index` crate
//! (column lineage, column impact, etc.). It does **not** ship
//! implementations or any infrastructure (no DuckDB primitives, no HTTP
//! server) — those live elsewhere:
//!
//! - **Trait surface** (this crate): one generic [`Provider<Args, Output>`]
//!   trait + per-feature `Args` / `Output` types, plus a single
//!   [`UnavailableProvider`] stub that's reused as the no-op default for
//!   every feature.
//! - **Implementations** (`dbt-index`): `DuckDb*Provider` structs that
//!   wrap an in-process `Db` connection and run the SQL.
//! - **Injection** (`dbt-cli`): the only binary crate that knows how to
//!   construct the impls and inject them into consumers (CLI commands,
//!   HTTP handlers, etc.).
//!
//! Adding a new feature:
//! 1. Define `FooArgs` and a `Result<FooOk, FooError>` output shape in this
//!    crate. Add a `FooProvider` dyn-alias and `UnavailableFoo`
//!    type-alias pointing at [`UnavailableProvider`]. If `FooError` is
//!    new, give it an `impl ProviderOutput for Result<_, FooError>` so
//!    the stub works.
//! 2. Provide a real `DuckDbFooProvider` impl in `dbt-index`.
//! 3. Wire it from `dbt-cli` at the call sites.

pub mod db;
pub use db::{DBT_RT_TABLES, DBT_TABLES, write_views_sql};

pub mod view_defs;

pub mod ingest;

pub mod info_schema;
pub use info_schema::{
    INFO_SCHEMA_DIR_NAME, INFO_SCHEMA_VERSION, Materializer, STAGING_DIR_NAME, versioned_dir,
    write_info_schema,
};

pub mod parquet;
pub use parquet::{IndexWriter, WriteMode};

pub mod epoch_layers;

pub mod error;
pub use error::IndexError;

pub mod format;

pub mod backend;
pub mod column_impact;
pub mod column_lineage;
pub mod freshness;
pub mod provider;

pub use backend::{
    Backend, BackendError, DuckDbInfoSchemaBackend, RegistrationReport, UnavailableBackend,
    register_index_views, register_info_schema_views,
};
pub use column_impact::{
    ColumnImpactArgs, ColumnImpactNode, ColumnImpactProvider, UnavailableColumnImpact,
};
pub use column_lineage::{
    ColumnLineageArgs, ColumnLineageEdge, ColumnLineageProvider, LineageError,
    UnavailableColumnLineage,
};
pub use freshness::{ArtifactMetaError, WriteSource, save_artifact_meta};
pub use provider::{Provider, ProviderOutput, UnavailableProvider};

/// Simple non-cryptographic hash for raw_code.
pub fn hash_str(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
