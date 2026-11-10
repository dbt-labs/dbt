//! `window.__DBT_DOCS__` — the build-scoped scalars the SPA needs at t=0.
//!
//! These are facts about the *build*, not about the data: the dbt version that
//! produced the site, which distribution produced it, whether analytics is
//! consented, and where to fetch DuckDB-WASM. They are inlined into `index.html`
//! rather than fetched, so they cost no request and are available before any
//! parquet loads.
//!
//! Deliberately excluded: anything derivable from the artifact set. In
//! particular there is no `has_column_lineage` flag — the browser infers that
//! from whether `dbt.column_lineage.parquet` loaded with rows, which keeps one
//! source of truth instead of two that can disagree.

use serde::Serialize;

use crate::providers::Providers;

use super::{DEFAULT_DUCKDB_CDN_BASE, ExportOptions};

/// Bumped when the shape below changes incompatibly, so a stale `index.html`
/// served next to fresh assets fails loudly instead of reading garbage.
const BOOTSTRAP_SCHEMA_VERSION: u32 = 1;

/// Marker the SPA's `index.html` template carries; replaced with the real
/// payload at export time.
const BOOTSTRAP_PLACEHOLDER: &str = "<!--dbt-docs-bootstrap-->";

/// JSON `\uXXXX` forms of the three characters that can steer the HTML
/// tokenizer out of a script body. Written as escapes so the inlined payload
/// is inert; they decode back to the originals, so the value is unchanged.
const ESCAPED_LT: &str = "\\u003c";
const ESCAPED_GT: &str = "\\u003e";
const ESCAPED_AMP: &str = "\\u0026";

#[derive(Debug, Clone, Serialize)]
pub struct SiteBootstrap {
    pub schema_version: u32,
    /// RFC3339, when the site was exported. Distinct from the data's own
    /// generation stamp in `dbt.generation.parquet`.
    pub generated_at: String,
    pub dbt_version: String,
    /// Distribution code, e.g. `"oss"`. Drives the upsell copy.
    pub distribution: String,
    pub is_logged_in: bool,
    /// Base URL for the DuckDB-WASM bundle.
    pub duckdb_cdn_base: String,
    /// Directory holding the parquet, relative to `index.html`. Carried here rather
    /// than hardcoded client-side because it is version-scoped
    /// (`info_schema/v<n>/`) and the version belongs to the writer, not the SPA.
    pub data_dir: String,
    pub telemetry: SiteTelemetry,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteTelemetry {
    /// Consent, resolved at export time from `DO_NOT_TRACK` and the project's
    /// `send_anonymous_usage_stats`. When false the browser emits nothing.
    pub enabled: bool,
    pub dbt_cloud_account_identifier: String,
    pub dbt_cloud_project_id: String,
    pub dbt_cloud_environment_id: String,
}

impl SiteBootstrap {
    pub fn new(providers: &Providers, options: &ExportOptions) -> Self {
        let hydration = providers.dist_info.telemetry_hydration();
        Self {
            schema_version: BOOTSTRAP_SCHEMA_VERSION,
            generated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            dbt_version: hydration.dbt_version,
            distribution: hydration.distribution,
            is_logged_in: hydration.is_logged_in,
            duckdb_cdn_base: options
                .duckdb_cdn_base
                .clone()
                .unwrap_or_else(|| DEFAULT_DUCKDB_CDN_BASE.to_string()),
            data_dir: format!("{}/", crate::export::data_dir()),
            telemetry: SiteTelemetry {
                enabled: options.analytics_enabled,
                dbt_cloud_account_identifier: hydration.dbt_cloud_account_identifier,
                dbt_cloud_project_id: hydration.dbt_cloud_project_id,
                dbt_cloud_environment_id: hydration.dbt_cloud_environment_id,
            },
        }
    }

    /// The `<script>` tag to inline.
    pub fn script_tag(&self) -> String {
        // serde_json cannot fail on this struct (plain scalars and strings), but
        // an empty object is a survivable fallback: the SPA treats a missing
        // bootstrap as "OSS, no telemetry, default CDN".
        let json = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string());
        // Neutralize every character that can steer the HTML tokenizer out of the
        // script body. `</script>` is the obvious one, but a bare `<script` is
        // just as bad: it moves the tokenizer into the double-escaped state,
        // where the *next* `</script>` no longer closes the element. Escaping `<`
        // to its JSON `\uXXXX` form sidesteps the whole class — the escapes decode
        // back to the original characters, so the payload is unchanged. These
        // fields are partly env-derived, so escape rather than trust.
        let json = json
            .replace('<', ESCAPED_LT)
            .replace('>', ESCAPED_GT)
            .replace('&', ESCAPED_AMP);
        format!("<script>window.__DBT_DOCS__ = {json};</script>")
    }

    /// Inline the payload into `index.html`.
    ///
    /// Prefers the explicit placeholder; falls back to inserting before
    /// `</head>` so an `index.html` built before the placeholder existed still
    /// gets a working bootstrap.
    pub fn inject(&self, html: &str) -> String {
        let tag = self.script_tag();
        if html.contains(BOOTSTRAP_PLACEHOLDER) {
            return html.replace(BOOTSTRAP_PLACEHOLDER, &tag);
        }
        match html.find("</head>") {
            Some(idx) => {
                let mut out = String::with_capacity(html.len() + tag.len());
                out.push_str(&html[..idx]);
                out.push_str(&tag);
                out.push_str(&html[idx..]);
                out
            }
            // No `</head>` at all: prepend, so the global is set before the app
            // module runs.
            None => format!("{tag}{html}"),
        }
    }
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
