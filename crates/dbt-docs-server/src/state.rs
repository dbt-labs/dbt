use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};

use crate::providers::Providers;
pub use dbt_docs_core::{DistInfo, TelemetryHydration};

/// Shared application state held by the axum router.
pub struct AppState {
    /// The information schema's versioned directory, which the site serves as its
    /// data directory.
    pub data_dir: PathBuf,
    pub providers: Providers,
    pub has_dbt_state: bool,
    pub do_not_track: bool,
    pub send_anonymous_usage_stats: bool,
    /// Whether data is actually loaded: `data_dir` exists and holds at least
    /// one `*.parquet` file. Computed once at boot.
    pub project_loaded: bool,
    /// RFC3339 timestamp of the loaded snapshot (`data_dir` mtime), or `None`
    /// on empty-start. Computed once at boot; a staleness signal.
    pub generation: Option<String>,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub fn new(
        data_dir: PathBuf,
        providers: Providers,
        has_dbt_state: bool,
        send_anonymous_usage_stats: bool,
    ) -> Self {
        let do_not_track = std::env::var("DO_NOT_TRACK").as_deref() == Ok("1");
        let project_loaded = Self::compute_project_loaded(&data_dir);
        let generation = if project_loaded {
            Self::compute_generation(&data_dir)
        } else {
            None
        };
        Self {
            data_dir,
            providers,
            has_dbt_state,
            do_not_track,
            send_anonymous_usage_stats,
            project_loaded,
            generation,
        }
    }

    /// `true` when `dir` is a directory holding at least one `*.parquet`
    /// file directly. Returns `false` on any IO error or missing dir.
    fn compute_project_loaded(dir: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".parquet"))
        })
    }

    /// RFC3339 timestamp of `dir`'s modified time (seconds precision, `Z`
    /// suffix), or `None` when the mtime is unavailable.
    fn compute_generation(dir: &Path) -> Option<String> {
        let modified = std::fs::metadata(dir).ok()?.modified().ok()?;
        let dt: DateTime<Utc> = modified.into();
        Some(dt.to_rfc3339_opts(SecondsFormat::Secs, true))
    }

    /// Override `do_not_track` for testing — avoids env var mutation in tests.
    #[cfg(test)]
    pub fn with_do_not_track(mut self, value: bool) -> Self {
        self.do_not_track = value;
        self
    }

    #[cfg(test)]
    pub fn with_send_anonymous_usage_stats(mut self, value: bool) -> Self {
        self.send_anonymous_usage_stats = value;
        self
    }

    pub fn dist_info(&self) -> DistInfo {
        self.providers.dist_info.dist_info()
    }

    pub fn server_version(&self) -> &'static str {
        self.providers.dist_info.server_version()
    }

    /// Telemetry fields the build knows authoritatively. The server no longer
    /// hydrates events with them — the export bakes them into the site's
    /// bootstrap instead (ADR-10). Mirrors [`Self::dist_info`] /
    /// [`Self::server_version`].
    pub fn telemetry_hydration(&self) -> TelemetryHydration {
        self.providers.dist_info.telemetry_hydration()
    }

    pub fn has_column_lineage(&self) -> bool {
        self.providers.column_lineage.is_available()
    }

    pub fn has_dbt_state(&self) -> bool {
        self.has_dbt_state
    }
}
