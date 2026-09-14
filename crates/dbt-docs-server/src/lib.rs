//! HTTP server that powers dbt docs v2.
//!
//! Two jobs: turn a parquet index into a statically hostable site
//! ([`export_site`]), and serve that site locally (`dbt docs serve`). There is no
//! API — the browser queries the parquet itself with DuckDB-WASM, so everything
//! this once computed server-side now runs client-side.
//!
//! Surfaces that interact with the artifact store, or that need analysis this
//! crate does not perform itself, sit behind dyn-compatible traits in
//! [`providers`]. Implementations are injected by the caller, so this crate
//! never names one and never depends on one.
//!
//! The CLI entry is `dbt docs serve`; see `run_docs_serve` in
//! `crates/dbt-main/src/dbt_lib.rs` for how this crate is invoked.
//!
//! The SPA lives in `web/`; `web/dist/` is a committed build embedded at compile
//! time and copied into the exported site. See the crate README for how to rebuild it.

use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct DocsServeArgs {
    pub target_path: Option<PathBuf>,
    pub host: String,
    pub port: u16,
    pub no_open: bool,
    pub has_dbt_state: bool,
    pub send_anonymous_usage_stats: bool,
    /// Generated site to serve, as written by [`export_site`].
    ///
    /// When set, files here take precedence over the embedded bundle. Only a
    /// generated site carries the `window.__DBT_DOCS__` bootstrap and the
    /// `data/*.parquet` artifacts, so this is what makes the app work; the
    /// embedded bundle alone cannot. `None` falls back to it anyway, which is
    /// useful for developing the server against a bundle with no site built.
    pub site_dir: Option<PathBuf>,
}

mod assets;
#[cfg(feature = "embed-ui")]
mod embed;
pub mod export;
pub mod providers;
mod server;
pub mod state;

pub use export::{ExportError, ExportOptions, ExportSummary, export_site, has_artifacts};
pub use providers::Providers;
pub use server::run_with_args;
pub use state::DistInfo;

/// Resolve the versioned information-schema directory the site reads.
///
/// Order of resolution:
/// 1. `args.target_path` (if provided) → `<target_path>/info_schema/v<n>/`.
/// 2. `./target/info_schema/v<n>/` in the current working directory.
///
/// Unlike the index this is not under `target/private/`: it is the site's own data
/// directory, served over HTTP, so it has to be publishable.
pub fn resolve_info_schema_dir(args: &DocsServeArgs) -> PathBuf {
    let target = match &args.target_path {
        Some(p) => p.clone(),
        None => PathBuf::from("./target"),
    };
    dbt_index_core::versioned_dir(&target.join(dbt_index_core::INFO_SCHEMA_DIR_NAME))
}

/// Convenience entry that just wraps args in an `Arc`. Mostly useful for
/// tests; binaries should call [`run_with_args`] directly.
pub async fn run(args: DocsServeArgs, providers: Providers) -> std::io::Result<()> {
    run_with_args(Arc::new(args), providers).await
}
