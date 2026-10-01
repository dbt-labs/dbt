use std::sync::Arc;

use dbt_index_core::{
    Backend, ColumnImpactProvider, ColumnLineageProvider, UnavailableBackend,
    UnavailableColumnImpact, UnavailableColumnLineage,
};
use serde::Serialize;

/// Metadata about the running distribution. Returned by `GET /api/v1/distribution`.
#[derive(Debug, Clone, Serialize)]
pub struct DistInfo {
    pub name: String,
    pub version: &'static str,
    pub is_logged_in: bool,
}

/// Server-known telemetry fields hydrated onto every docs analytics event
/// (`POST /api/v1/analytics/events`). The server fills these in authoritatively
/// so the browser can send a slim payload and cannot spoof them.
#[derive(Debug, Clone, Default)]
pub struct TelemetryHydration {
    /// Distribution code (`DistInfo.name`, e.g. `"oss"`).
    pub distribution: String,
    /// Real dbt/Fusion version.
    pub dbt_version: String,
    pub is_logged_in: bool,
    pub dbt_cloud_account_identifier: String,
    pub dbt_cloud_project_id: String,
    pub dbt_cloud_environment_id: String,
}

pub trait DistInfoProvider: Send + Sync {
    fn dist_info(&self) -> DistInfo;

    fn server_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// Server-authoritative telemetry fields for analytics event hydration.
    ///
    /// Default derives distribution/version/login from [`Self::dist_info`] and
    /// leaves the dbt Cloud IDs empty (OSS has no such context). The env-backed
    /// impl overrides this to read real cloud IDs and the dbt version.
    fn telemetry_hydration(&self) -> TelemetryHydration {
        let info = self.dist_info();
        TelemetryHydration {
            distribution: info.name,
            dbt_version: info.version.to_string(),
            is_logged_in: info.is_logged_in,
            ..Default::default()
        }
    }
}

pub struct DefaultDistInfoProvider;

impl DistInfoProvider for DefaultDistInfoProvider {
    fn dist_info(&self) -> DistInfo {
        DistInfo {
            name: "oss".to_string(),
            version: self.server_version(),
            is_logged_in: false,
        }
    }
}

/// Bundle of pluggable providers passed to the docs server at startup.
#[derive(Clone)]
pub struct Providers {
    pub backend: Arc<dyn Backend>,
    pub column_lineage: Arc<ColumnLineageProvider>,
    pub column_impact: Arc<ColumnImpactProvider>,
    pub dist_info: Arc<dyn DistInfoProvider>,
}

impl Default for Providers {
    fn default() -> Self {
        Providers {
            backend: Arc::new(UnavailableBackend),
            column_lineage: Arc::new(UnavailableColumnLineage::new()),
            column_impact: Arc::new(UnavailableColumnImpact::new()),
            dist_info: Arc::new(DefaultDistInfoProvider),
        }
    }
}
