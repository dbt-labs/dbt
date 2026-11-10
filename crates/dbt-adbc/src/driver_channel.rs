//! Opting a backend into its ADBC Driver Foundry driver.
//!
//! ```text
//! DBT_ADBC_BIGQUERY_USE_FOUNDRY=1
//! ```
//!
//! The Foundry build is published under its own name (`bigquery_foundry`), so
//! it has separate CDN paths, cache files and checksums from the canonical
//! driver and either can be rolled back without disturbing the other.
//!
//! Variables are read once per process: the loader caches drivers by
//! `(backend, adbc_version)`, which has no channel, so honouring a mid-run
//! change could leave one process holding two drivers for the same backend.
//!
//! Anything unusable — unset, unparseable, or a backend with no Foundry driver
//! published — falls back to the canonical driver with a warning, rather than
//! failing a run from deep inside the loader.

use std::sync::OnceLock;

use crate::driver::Backend;
use crate::env_var::env_var_bool_or_warn;

/// Backend, the variable that opts it in, and its driver as
/// `(CDN name, version)` — `None` until one is published.
type FoundryRow = (Backend, &'static str, Option<(&'static str, &'static str)>);

/// To publish a Foundry driver: upload it to `<backend>_foundry` on the CDN,
/// add a `*_FOUNDRY_DRIVER_VERSION` constant beside the other versions, add the
/// `drivers.toml` entry, regenerate `checksums.rs`, then fill in the row here.
/// One row per backend keeps the variable, name and version from drifting apart.
const FOUNDRY_DRIVERS: &[FoundryRow] = &[
    (
        Backend::BigQuery,
        "DBT_ADBC_BIGQUERY_USE_FOUNDRY",
        Some(("bigquery_foundry", crate::BIGQUERY_FOUNDRY_DRIVER_VERSION)),
    ),
    (Backend::Snowflake, "DBT_ADBC_SNOWFLAKE_USE_FOUNDRY", None),
    (Backend::Databricks, "DBT_ADBC_DATABRICKS_USE_FOUNDRY", None),
];

/// The published driver regardless of opt-in, for checks that must see every
/// channel — currently only the checksum coverage test.
#[cfg(test)]
pub(crate) fn published_foundry_driver(backend: Backend) -> Option<(&'static str, &'static str)> {
    FOUNDRY_DRIVERS
        .iter()
        .find(|&&(b, ..)| b == backend)
        .and_then(|&(_, _, driver)| driver)
}

/// The Foundry driver for `backend`, if published and opted into. Callers fall
/// back to the canonical driver on `None`.
pub(crate) fn foundry_driver(backend: Backend) -> Option<(&'static str, &'static str)> {
    // Most backends have no row, so don't make them touch the cache at all.
    if !FOUNDRY_DRIVERS.iter().any(|&(b, ..)| b == backend) {
        return None;
    }
    static ENABLED: OnceLock<Vec<(Backend, &'static str, &'static str)>> = OnceLock::new();
    ENABLED
        .get_or_init(resolve_enabled)
        .iter()
        .find(|&&(b, ..)| b == backend)
        .map(|&(_, name, version)| (name, version))
}

/// Reads every variable once. Usually empty, which allocates nothing.
fn resolve_enabled() -> Vec<(Backend, &'static str, &'static str)> {
    FOUNDRY_DRIVERS
        .iter()
        .filter(|&&(_, var, _)| env_var_bool_or_warn(var))
        .filter_map(|&(backend, var, driver)| match driver {
            Some((name, version)) => Some((backend, name, version)),
            None => {
                tracing::warn!(
                    "Ignoring {var}: no Foundry driver is published for this backend yet. \
                     Using the default driver."
                );
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Opting in a reserved backend must not select a driver that was never
    /// uploaded.
    #[test]
    fn reserved_backends_have_no_driver() {
        for backend in [Backend::Snowflake, Backend::Databricks] {
            let &(_, _, driver) = FOUNDRY_DRIVERS
                .iter()
                .find(|&&(b, ..)| b == backend)
                .expect("reserved backend should have a row");
            assert_eq!(driver, None);
        }
    }
}
