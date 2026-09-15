// ----------------------------------------------------------------------------------------------
// dbt brand name (shown in --version banners and JSON log lines)
pub const DBT_BRAND_NAME: &str = "dbt";
pub const DBT_SA_CLI: &str = "dbt-sa-cli";

// ----------------------------------------------------------------------------------------------
// dbt inputs
pub const DBT_MIN_SUPPORTED_VERSION: &str = "1.8.0";
// (versusfacit): Load state from catalogs.yml. We only permit a single
// catalogs.yml. A possible future direction will be to move this to
// ~/.dbt directory. This depends on read catalog and Xplat decisions.
pub const DBT_CONFIG_DIR: &str = ".dbt";
pub const DBT_CATALOGS_YML: &str = "catalogs.yml";
pub const DBT_PROJECT_YML: &str = "dbt_project.yml";
pub const DBT_PROFILES_YML: &str = "profiles.yml";
pub const DBT_CLOUD_YML: &str = "dbt_cloud.yml";
pub const DBT_VARS_YML: &str = "vars.yml";

// ----------------------------------------------------------------------------------------------
// dbt outputs

//   target/
//   ├── compiled/
//   │   ├── model_fs_example.sql
//   │   └── other_files.sql
//   ├── run/
//   │   ├── model_fs_example.sql
//   │   └── other_files.sql
//   ├── generic_tests/
//   │   ├── test.sql
//   ├── private/
//   │   ├── index/          engine snapshot (not a user API)
//   │   └── metadata/       epoch parquet (not a user API)
//   ├── info_schema/        public queryable contract (opt-in)
//   ├── manifest.json
//   ├── catalog.json
//   └── logs/
//       └── fs_run_log.txt
//   └── db/
//       └── database/schema/table.parquet
pub const DBT_TARGET_DIR_NAME: &str = "target";
pub const DBT_PACKAGES_DIR_NAME: &str = "dbt_packages";
pub const DBT_INTERNAL_PACKAGES_DIR_NAME: &str = "dbt_internal_packages";
pub const DBT_MANIFEST_JSON: &str = "manifest.json";
pub const DBT_MANIFEST_INFO: &str = "manifest.info";
pub const DBT_SEMANTIC_MANIFEST_JSON: &str = "semantic_manifest.json";
pub const DBT_CATALOG_JSON: &str = "catalog.json";
pub const DBT_SOURCES_JSON: &str = "sources.json";
pub const DBT_FRESHNESS_JSON: &str = "freshness.json";
pub const DBT_COMPILED_DIR_NAME: &str = "compiled";
/// Internal engine artifacts under `<target>/private/` — not a user API.
pub const DBT_PRIVATE_DIR_NAME: &str = "private";
pub const DBT_INDEX_DIR_NAME: &str = "index";
pub const DBT_METADATA_DIR_NAME: &str = "metadata";
pub const DBT_INFO_SCHEMA_DIR_NAME: &str = "info_schema";
pub const DBT_INFO_SCHEMA_STAGING_DIR_NAME: &str = ".info_schema_staging";
pub const DBT_EPHEMERAL_DIR_NAME: &str = "ephemeral";
pub const DBT_HOOKS_DIR_NAME: &str = "hooks";
pub const DBT_CTE_PREFIX: &str = "__dbt__cte__";
pub const DBT_RUN_DIR_NAME: &str = "run";
pub const DBT_DB_DIR_NAME: &str = "db";
pub const DBT_LOG_DIR_NAME: &str = "logs";
pub const DBT_STATE_DIR_NAME: &str = "state";
pub const DBT_DEFAULT_LOG_FILE_NAME: &str = "dbt.log";
pub const DBT_DEFAULT_QUERY_LOG_FILE_NAME: &str = "query_log.sql";
pub const DBT_DEFAULT_OTEL_PARQUET_FILE_NAME: &str = "otel.parquet";
pub const DBT_DEFAULT_LOG_FILE_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const DBT_DEFAULT_LOG_FILE_BACKUP_COUNT: usize = 5;
pub const DBT_ROOT_PACKAGE_VAR_PREFIX: &str = "__root__";
pub const DBT_GENERIC_TESTS_DIR_NAME: &str = "generic_tests";
pub const DBT_SNAPSHOTS_DIR_NAME: &str = "snapshots";
// ----------------------------------------------------------------------------------------------
pub const DBT_MODELS_DIR_NAME: &str = "models";
pub const DBT_SELECTORS_YML: &str = "selectors.yml";

// ----------------------------------------------------------------------------------------------
// dbt packages
pub const DBT_PACKAGES_LOCK_FILE: &str = "package-lock.yml";
pub const DBT_PACKAGES_YML: &str = "packages.yml";
pub const DBT_DEPENDENCIES_YML: &str = "dependencies.yml";

// ----------------------------------------------------------------------------------------------
// aggregated generic tests
pub const DBT_AGGREGATED_GENERIC_TEST_CONTEXT: &str = "aggregated_test_skip_column_names";

// ----------------------------------------------------------------------------------------------
// dbt console output
pub const ERROR: &str = "error";
pub const WARNING: &str = "warning";
pub const PANIC: &str = "panic";

// ----------------------------------------------------------------------------------------------
// actions in order of appearance

pub const LOADING: &str = "   Loading";
pub const FETCHING: &str = "  Fetching";
pub const INSTALLING: &str = "Installing";
pub const RESOLVING: &str = " Resolving";
pub const PARSING: &str = "   Parsing";
// not being issued right now
pub const SCHEDULING: &str = "Scheduling";
//
pub const RENDERING: &str = " Rendering";
pub const RUNNING: &str = "   Running";
pub const COMPARING: &str = "   Comparing";
pub const CLONING: &str = "   Cloning";
pub const SUCCEEDED: &str = " Succeeded";
pub const PASSED: &str = "    Passed";
pub const WARNED: &str = "    Warned";
pub const FAILED: &str = "    Failed";
pub const REUSED: &str = "    Reused";
pub const SKIPPED: &str = "   Skipped";

// debug command
pub const VALIDATING: &str = "Validating";

// other
pub const NOOP: &str = "noop";

// cas/node read/write

pub const CAS_RD: &str = "   Reading";
pub const CAS_WR: &str = "   Writing";
pub const NODES_RD: &str = "   Reading";
pub const NODES_WR: &str = "   Writing";
pub const COLUMNS_RD: &str = "   Reading";
pub const COLUMNS_WR: &str = "   Writing";
pub const COLUMN_LINEAGE_WR: &str = "   Writing";

pub const DBT_CDN_URL: &str = "https://public.cdn.getdbt.com/fs";

// ----------------------------------------------------------------------------------------------
// dbt custom environment variables

/// Prefix for dbt custom environment variables that appear in metadata and structured logs.
pub const DBT_ENV_CUSTOM_ENV_PREFIX: &str = "DBT_ENV_CUSTOM_ENV_";

/// Collects all `DBT_ENV_CUSTOM_ENV_*` environment variables into a map,
/// stripping the prefix from keys. Matches dbt-core behavior where
/// e.g. `DBT_ENV_CUSTOM_ENV_FOO=bar` becomes `{"FOO": "bar"}`.
pub fn collect_dbt_custom_envs() -> std::collections::BTreeMap<String, String> {
    std::env::vars()
        .filter_map(|(k, v)| {
            k.strip_prefix(DBT_ENV_CUSTOM_ENV_PREFIX)
                .map(|s| (s.to_string(), v))
        })
        .collect()
}

/// `<target>/private/` — engine-internal artifacts, not a user API.
pub fn default_private_dir(out_dir: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    out_dir.as_ref().join(DBT_PRIVATE_DIR_NAME)
}

/// `<target>/private/index/` — default snapshot index directory.
pub fn default_index_dir(out_dir: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    default_private_dir(out_dir).join(DBT_INDEX_DIR_NAME)
}

/// `<target>/private/metadata/` — default epoch parquet directory.
pub fn default_metadata_dir(out_dir: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    default_private_dir(out_dir).join(DBT_METADATA_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_dbt_custom_envs() {
        const VAR_NAME: &str = "DBT_ENV_CUSTOM_ENV_TEST_KEY";
        const VAR_VALUE: &str = "test_value";

        // Save any pre-existing value, then set our test var
        let prev = std::env::var(VAR_NAME).ok();
        unsafe {
            #[allow(clippy::disallowed_methods)]
            std::env::set_var(VAR_NAME, VAR_VALUE);
        }

        let envs = collect_dbt_custom_envs();

        // Cleanup before assertions so we don't leak on failure
        unsafe {
            #[allow(clippy::disallowed_methods)]
            if let Some(v) = &prev {
                std::env::set_var(VAR_NAME, v);
            } else {
                std::env::remove_var(VAR_NAME);
            }
        }

        // The prefix should be stripped: key is "TEST_KEY", not the full var name
        assert_eq!(
            envs.get("TEST_KEY").map(String::as_str),
            Some(VAR_VALUE),
            "Expected collect_dbt_custom_envs to contain TEST_KEY={VAR_VALUE}"
        );
        assert!(
            !envs.contains_key(VAR_NAME),
            "Full prefixed key should not appear. The prefix should be stripped"
        );
    }

    #[test]
    fn default_index_and_metadata_dirs_live_under_private() {
        let out = std::path::Path::new("/repo/target");
        assert_eq!(
            default_index_dir(out),
            std::path::PathBuf::from("/repo/target/private/index")
        );
        assert_eq!(
            default_metadata_dir(out),
            std::path::PathBuf::from("/repo/target/private/metadata")
        );
    }
}
