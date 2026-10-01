//! Adapter credentials' config-key alias map.
//!
//! dbt-core canonicalizes each config source's keys through the active adapter's
//! `Credentials._ALIASES` before merging it — in both the rendered
//! (`context/context_config.py:222`, `ContextConfigGenerator._update_from_config`) and the
//! unrendered (`context/context_config.py:302`, `UnrenderedConfigGenerator._update_from_config`)
//! config generator. So a node authored with `+catalog:` on Databricks is stored under
//! `database`, not `catalog`. This module transcribes those maps so Fusion can do the same.

use std::collections::BTreeMap;

use crate::AdapterType;

/// Returns `adapter_type`'s config-key alias map: `(alias, canonical)` pairs.
pub fn config_aliases(adapter_type: AdapterType) -> &'static [(&'static str, &'static str)] {
    match adapter_type {
        // dbt-databricks 1.9.8 and 1.10.9 (identical), `dbt/adapters/databricks/credentials.py:69-72`.
        AdapterType::Databricks => &[
            ("catalog", "database"),
            ("target_catalog", "target_database"),
        ],
        // dbt-bigquery 1.9.2, `dbt/adapters/bigquery/credentials.py:115-124`.
        AdapterType::Bigquery => &[
            ("project", "database"),
            ("dataset", "schema"),
            ("target_project", "target_database"),
            ("target_dataset", "target_schema"),
            ("retries", "job_retries"),
            ("timeout_seconds", "job_execution_timeout_seconds"),
            ("dataproc_region", "compute_region"),
        ],
        // dbt-postgres 1.9.1, `dbt/adapters/postgres/connections.py:40`.
        AdapterType::Postgres => &[("dbname", "database"), ("pass", "password")],
        // dbt-redshift 1.9.0 and 1.9.5 (identical), `dbt/adapters/redshift/connections.py:156`.
        AdapterType::Redshift => &[("dbname", "database"), ("pass", "password")],
        // Verified: these adapters' `Credentials` declare no `_ALIASES`.
        AdapterType::Snowflake | AdapterType::Spark | AdapterType::DuckDB => &[],
        // TODO(fs#13424): `_ALIASES` not transcribed for this adapter -- its Python package was
        // not vendored in the local cache used to populate this map.
        AdapterType::Salesforce
        | AdapterType::Fabric
        | AdapterType::ClickHouse
        | AdapterType::Exasol
        | AdapterType::Athena
        | AdapterType::Starburst
        | AdapterType::Trino
        | AdapterType::Datafusion
        | AdapterType::Dremio
        | AdapterType::Oracle
        | AdapterType::LakeCompute => &[],
    }
}

/// The canonical key for `key` under `adapter_type`'s alias map.
pub fn canonical_config_key(adapter_type: AdapterType, key: &str) -> &str {
    config_aliases(adapter_type)
        .iter()
        .find(|(alias, _)| *alias == key)
        .map_or(key, |(_, canonical)| canonical)
}

/// Two authored keys in one config source mapped to the same canonical key -- mirroring
/// `Translator.translate_mapping`'s `DuplicateAliasError` (`core/dbt/utils/utils.py:185-192`).
/// Carries the second colliding value so the caller can locate it (e.g. via its span) to build a
/// diagnostic; this module has no opinion on how that location is reported.
#[derive(Debug)]
pub struct DuplicateAliasKey<V> {
    pub key_a: String,
    pub key_b: String,
    pub canonical: String,
    pub value_b: V,
}

/// Applies `adapter_type`'s alias map to one config source mapping.
/// (Mirrors `Translator.translate_mapping` in `core/dbt/utils/utils.py:185-192`)
pub fn canonicalize_config_keys<V>(
    adapter_type: AdapterType,
    map: BTreeMap<String, V>,
) -> Result<BTreeMap<String, V>, DuplicateAliasKey<V>> {
    let mut result = BTreeMap::new();
    let mut original_for_canonical: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in map {
        let canonical = canonical_config_key(adapter_type, &key).to_string();
        if let Some(first_key) = original_for_canonical.get(&canonical) {
            return Err(DuplicateAliasKey {
                key_a: first_key.clone(),
                key_b: key,
                canonical,
                value_b: value,
            });
        }
        original_for_canonical.insert(canonical.clone(), key);
        result.insert(canonical, value);
    }
    Ok(result)
}

/// Applies `adapter_type`'s alias map to a previous manifest node's `unrendered_config`.
///
/// Unlike [`canonicalize_config_keys`], this never errors on a duplicate: a previous manifest is
/// data this run did not write (an older Fusion, or a manifest a project checked in by hand), so a
/// manifest carrying both an alias and its canonical spelling is foreign input, not a bug in a
/// parse we control. Resolve it deterministically instead -- the canonical spelling wins, since it
/// is at least as likely to be the authoritative value as the alias is.
///
/// Suppress-only and idempotent, like every Stage-1 normalization: on a manifest that already
/// carries only canonical keys, or one that predates this fix (all-alias spelling), this can only
/// ever rename a key onto its canonical form -- it never invents or drops a value.
pub fn canonicalize_previous_manifest_config_keys<V>(
    adapter_type: AdapterType,
    map: BTreeMap<String, V>,
) -> BTreeMap<String, V> {
    let mut result: BTreeMap<String, V> = BTreeMap::new();
    for (key, value) in map {
        let canonical = canonical_config_key(adapter_type, &key);
        let is_already_canonical = canonical == key;
        let canonical = canonical.to_string();
        if is_already_canonical || !result.contains_key(&canonical) {
            result.insert(canonical, value);
        }
        // else: an alias arriving after its canonical spelling is already resolved -- drop it,
        // the canonical value wins.
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_config_key_renames_an_alias() {
        assert_eq!(
            canonical_config_key(AdapterType::Databricks, "catalog"),
            "database"
        );
    }

    #[test]
    fn canonical_config_key_passes_through_a_non_alias() {
        // Not an alias at all.
        assert_eq!(
            canonical_config_key(AdapterType::Databricks, "catalog_name"),
            "catalog_name"
        );
        // An alias on one adapter is inert on another with no such map (D1).
        assert_eq!(
            canonical_config_key(AdapterType::Snowflake, "catalog"),
            "catalog"
        );
    }

    fn map(pairs: &[(&str, i32)]) -> BTreeMap<String, i32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn canonicalize_config_keys_renames_an_alias() {
        let result =
            canonicalize_config_keys(AdapterType::Databricks, map(&[("catalog", 1)])).unwrap();
        assert_eq!(result, map(&[("database", 1)]));
    }

    #[test]
    fn canonicalize_config_keys_passes_through_non_aliases() {
        let result = canonicalize_config_keys(
            AdapterType::Databricks,
            map(&[("catalog_name", 1), ("materialized", 2)]),
        )
        .unwrap();
        assert_eq!(result, map(&[("catalog_name", 1), ("materialized", 2)]));

        // The same input is untouched on an adapter with no alias map at all.
        let result =
            canonicalize_config_keys(AdapterType::Snowflake, map(&[("catalog", 1)])).unwrap();
        assert_eq!(result, map(&[("catalog", 1)]));
    }

    #[test]
    fn canonicalize_config_keys_errors_on_duplicate_canonical_key() {
        let err = canonicalize_config_keys(
            AdapterType::Databricks,
            map(&[("catalog", 1), ("database", 2)]),
        )
        .unwrap_err();
        assert_eq!(err.key_a, "catalog");
        assert_eq!(err.key_b, "database");
        assert_eq!(err.canonical, "database");
        assert_eq!(err.value_b, 2);
    }

    #[test]
    fn canonicalize_previous_manifest_config_keys_renames_an_alias() {
        let result = canonicalize_previous_manifest_config_keys(
            AdapterType::Databricks,
            map(&[("catalog", 1)]),
        );
        assert_eq!(result, map(&[("database", 1)]));
    }

    #[test]
    fn canonicalize_previous_manifest_config_keys_is_idempotent() {
        let once = canonicalize_previous_manifest_config_keys(
            AdapterType::Databricks,
            map(&[("catalog", 1), ("materialized", 2)]),
        );
        let twice =
            canonicalize_previous_manifest_config_keys(AdapterType::Databricks, once.clone());
        assert_eq!(once, twice);
    }

    /// Residual (fs#13424): these functions are **flat** -- they rename only top-level keys.
    /// That matches dbt-core for every node-config caller (`translate_aliases` defaults to
    /// `recurse=False`), but *not* for the `sources:` block, which `SourceParser.parse`
    /// translates with `recurse=True` (`core/dbt/parser/schemas.py:544`); and
    /// `Translator.translate_value` (`core/dbt/utils/utils.py:198-203`) then recurses into every
    /// nested mapping and sequence unconditionally, `meta:`/`columns:`/`freshness:` included. So
    /// dbt-core renames a key that merely happens to be spelled `catalog` inside a Databricks
    /// source's `meta:`, and Fusion does not. Deliberate: Fusion canonicalizes the source
    /// positions that carry real config (`SourceProperties` in `resolve_sources.rs`, and the
    /// source/table `config:` dicts in `build_source_unrendered_config`) and leaves a user's
    /// arbitrary nested keys alone. `#[ignore]`d because it pins the divergence, not a behavior to
    /// preserve -- if a real project is ever bitten by it, the fix is a recursive variant used only
    /// on the source-properties path, not a change to these functions.
    #[test]
    #[ignore = "fs#13424 residual: dbt-core translates the sources: block recursively \
                (recurse=True), so a nested meta.catalog key is renamed there and not here"]
    fn canonicalize_config_keys_is_flat_unlike_dbt_cores_source_block_translation() {
        let nested: BTreeMap<String, BTreeMap<String, i32>> = [(
            "meta".to_string(),
            [("catalog".to_string(), 1)].into_iter().collect(),
        )]
        .into_iter()
        .collect();
        let result = canonicalize_config_keys(AdapterType::Databricks, nested).unwrap();
        // Desired (not delivered), matching `recurse=True`: `meta` -> `{"database": 1}`.
        assert_eq!(
            result.get("meta").and_then(|m| m.get("catalog")),
            Some(&1),
            "a nested `catalog` key is left untouched"
        );
    }

    #[test]
    fn canonicalize_previous_manifest_config_keys_merges_duplicate_deterministically() {
        // A manifest carrying both spellings (data we did not write this run) merges to exactly
        // one key -- the canonical spelling wins -- rather than erroring.
        let result = canonicalize_previous_manifest_config_keys(
            AdapterType::Databricks,
            map(&[("catalog", 1), ("database", 2)]),
        );
        assert_eq!(result, map(&[("database", 2)]));

        // Same outcome regardless of which spelling sorts first in the input map.
        let result = canonicalize_previous_manifest_config_keys(
            AdapterType::Bigquery,
            map(&[("database", 2), ("project", 1)]),
        );
        assert_eq!(result, map(&[("database", 2)]));
    }
}
