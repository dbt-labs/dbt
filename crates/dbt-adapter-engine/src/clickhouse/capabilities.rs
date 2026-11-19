//! ClickHouse server capabilities, probed once per engine.

use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};

/// ClickHouse server capabilities, probed once per engine and reused for every
/// model/thread of that invocation (as the Python client does at connection
/// init, and for `atomic_exchange` behind a process-global lock).
#[derive(Debug, Clone)]
pub struct ClickHouseCapabilities {
    /// `SELECT version()` result; empty when the probe failed (callers treat
    /// unknown as "modern server").
    pub server_version: String,
    /// Whether atomic `EXCHANGE TABLES` works on this server/database
    /// (dbclient.py `_check_atomic_exchange`).
    pub atomic_exchange: bool,
    /// Whether lightweight deletes are usable. Mirrors dbclient.py
    /// `_check_lightweight_deletes`.
    pub has_lw_deletes: bool,
    /// `has_lw_deletes` gated by the profile `use_lw_deletes` flag; picks the
    /// default incremental strategy.
    pub use_lw_deletes: bool,
    /// The ND-mutation setting is disabled server-side but overridable; see
    /// `statement_options`.
    pub nd_mutation_override: bool,
}

impl ClickHouseCapabilities {
    /// Whether the connected server is older than `version`; an unknown
    /// server version is treated as modern (false).
    pub fn is_before(&self, version: &str) -> AdapterResult<bool> {
        if self.server_version.is_empty() {
            return Ok(false);
        }
        Ok(compare_versions(version, &self.server_version)? > 0)
    }

    /// Inclusive counterpart of [`ClickHouseCapabilities::is_before`]; an
    /// unknown server version is treated as modern (true).
    pub fn is_at_or_after(&self, version: &str) -> AdapterResult<bool> {
        if self.server_version.is_empty() {
            return Ok(true);
        }
        Ok(compare_versions(&self.server_version, version)? >= 0)
    }
}

/// Mirrors dbt-clickhouse util.py `compare_versions`: 1/-1/0 comparing dotted
/// numeric versions segment by segment; errors on non-numeric segments.
fn compare_versions(v1: &str, v2: &str) -> AdapterResult<i32> {
    for (part1, part2) in v1.split('.').zip(v2.split('.')) {
        match (part1.parse::<i64>(), part2.parse::<i64>()) {
            (Ok(a), Ok(b)) => {
                if a != b {
                    return Ok(if a > b { 1 } else { -1 });
                }
            }
            _ => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "Version must consist of only numbers separated by '.'",
                ));
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_versions_mirrors_python() {
        assert_eq!(compare_versions("26.6", "26.3.12.3").unwrap(), 1);
        assert_eq!(compare_versions("22.7.1.2484", "26.3.12.3").unwrap(), -1);
        assert_eq!(compare_versions("26.3", "26.3.12.3").unwrap(), 0); // zip stops at shorter
        assert!(compare_versions("26.x", "26.3").is_err());
    }
}
