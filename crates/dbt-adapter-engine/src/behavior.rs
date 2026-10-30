//! Behavior flags: the base set every adapter has, the adapter-specific ones,
//! and the flags Fusion accepts but ignores.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use dbt_adapter_core::AdapterType;
use dbt_common::behavior_flags::{Behavior, BehaviorFlag};

use AdapterType::*;

// https://github.com/dbt-labs/dbt-adapters/blob/3ed165d452a0045887a5032c621e605fd5c57447/dbt-adapters/src/dbt/adapters/base/impl.py#L117
pub(crate) static DEFAULT_BASE_BEHAVIOR_FLAGS: LazyLock<[BehaviorFlag; 4]> = LazyLock::new(|| {
    [
        BehaviorFlag::new(
            "require_batched_execution_for_custom_microbatch_strategy",
            true,
            Some("https://docs.getdbt.com/docs/build/incremental-microbatch"),
            None,
            None,
        ),
        BehaviorFlag::new("enable_truthy_nulls_equals_macro", false, None, None, None),
        BehaviorFlag::new(
            "use_catalogs_v2",
            true,
            None,
            Some(
                "Validate catalogs.yml against the v2 schema. Set to `false` to keep the deprecated v1 schema.",
            ),
            Some("https://github.com/dbt-labs/dbt-core/discussions/12723"),
        ),
        BehaviorFlag::new(
            "require_resource_names_without_plus_prefix",
            false,
            None,
            Some(
                "When enabled, the + prefix in dbt_project.yml always indicates a configuration key. Folder and file names may not start with the + prefix.",
            ),
            None,
        ),
    ]
});

/// Get adapter-specific behavior flags for a given adapter type
/// This is a standalone function to avoid needing to create adapter instances
/// just to get the flags
pub fn adapter_specific_behavior_flags(adapter_type: AdapterType) -> Vec<BehaviorFlag> {
    match adapter_type {
        Snowflake => {
            // https://github.com/dbt-labs/dbt-adapters/blob/c4c04de76d5a6c56c95965041a93156fdeaf4641/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L46
            let flag = BehaviorFlag::new(
                "snowflake_default_transient_dynamic_tables",
                false,
                Some(
                    "When enabled, dynamic tables default to transient (matching regular table behavior). This is a breaking change from previous behavior where dynamic tables were non-transient.",
                ),
                None,
                None,
            );
            vec![flag]
        }
        Databricks => {
            let use_user_folder_for_python = BehaviorFlag::new(
                "use_user_folder_for_python",
                true,
                Some(
                    "Use the user's home folder for uploading python notebooks. Shared folder use is deprecated due to governance concerns.",
                ),
                None,
                None,
            );

            let use_materialization_v2 = BehaviorFlag::new(
                "use_materialization_v2",
                false,
                Some(
                    "Use revamped materializations based on separating create and insert. This allows more performant column comments, as well as new column features.",
                ),
                None,
                None,
            );

            let use_replace_on_for_insert_overwrite = BehaviorFlag::new(
                "use_replace_on_for_insert_overwrite",
                true,
                Some(
                    "Use INSERT INTO ... REPLACE ON syntax for insert_overwrite on SQL Warehouses. When enabled, only matching partitions are overwritten; historical partitions are preserved.",
                ),
                None,
                None,
            );

            let use_managed_iceberg = BehaviorFlag::new(
                "use_managed_iceberg",
                false,
                Some(
                    "Use managed Iceberg tables when table_format is iceberg. When this flag is disabled, UniForm is used instead.",
                ),
                None,
                None,
            );

            // https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L160
            let use_describe_as_json_for_relation_metadata = BehaviorFlag::new(
                "use_describe_as_json_for_relation_metadata",
                false,
                Some(
                    "Use DESCRIBE TABLE EXTENDED AS JSON when supported to fetch relation metadata like constraints, column masks, row filters, view definition, etc. When disabled, falls back to information_schema queries.",
                ),
                None,
                None,
            );

            vec![
                use_user_folder_for_python,
                use_materialization_v2,
                use_replace_on_for_insert_overwrite,
                use_managed_iceberg,
                use_describe_as_json_for_relation_metadata,
            ]
        }
        Bigquery => {
            // https://github.com/dbt-labs/dbt-adapters/blob/b9ebd240e39882a8c43ed659de423c7504d4642a/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L109-L110
            let flag = BehaviorFlag::new(
                "bigquery_noop_alter_relation_comment",
                false,
                Some(
                    "Make bigquery__alter_relation_comment a no-op. This is useful when relation descriptions are already set in DDL (e.g. via OPTIONS(description=...)) to avoid an unnecessary update.",
                ),
                None,
                None,
            );
            vec![flag]
        }
        Fabric => {
            let flag = BehaviorFlag::new(
                "empty",
                false,
                Some(
                    "When enabled, table and view materializations will be created as empty structures (no data).",
                ),
                None,
                None,
            );
            vec![flag]
        }
        Redshift => {
            // TODO: https://github.com/dbt-labs/fs/issues/11871
            let skip_autocommit_transaction_statements = BehaviorFlag::new(
                "redshift_skip_autocommit_transaction_statements",
                false,
                Some(
                    "When enabled, skip BEGIN/COMMIT wrapping so statements that cannot run in a transaction block (e.g. ALTER COLUMN TYPE for VARCHAR/VARBYTE size changes) can be issued.",
                ),
                None,
                None,
            );
            // https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-redshift/src/dbt/adapters/redshift/impl.py#L56-L66
            let grants_extended = BehaviorFlag::new(
                "redshift_grants_extended",
                false,
                Some(
                    "Enable Redshift grants for groups and roles using 'user:', 'group:', and 'role:' prefixes. Unprefixed grantees remain users for backward compatibility.",
                ),
                None,
                None,
            );
            vec![skip_autocommit_transaction_statements, grants_extended]
        }
        Postgres | Salesforce | Spark | DuckDB | LakeCompute | ClickHouse | Exasol | Starburst
        | Athena | Trino | Datafusion | Dremio | Oracle => vec![],
    }
}

/// Behavior flags from dbt-core that have been removed in Fusion.
/// The new behavior is always enabled; these flags are accepted but ignored.
/// See: https://docs.getdbt.com/reference/global-configs/behavior-changes
const REMOVED_IN_FUSION: &[&str] = &[
    // dbt-core flags
    // ---
    "use_info_schema_for_columns",
    "require_explicit_package_overrides_for_builtin_materializations",
    "require_resource_names_without_spaces",
    "source_freshness_run_project_hooks",
    "skip_nodes_if_on_run_start_fails",
    "state_modified_compare_more_unrendered_values",
    "require_yaml_configuration_for_mf_time_spines",
    "require_nested_cumulative_type_params",
    "require_generic_test_arguments_property",
    "require_unique_project_resource_names",
    "require_valid_schema_from_generate_schema_name",
    "require_sql_header_in_test_configs",
    "require_batched_execution_for_custom_microbatch_strategy",
    // dbt-redshift flags
    // ---
    // metadata adapter already uses SVV views
    "restrict_direct_pg_catalog_access",
    // dbt-bigquery flags
    // ---
    // metadata adapter already uses batched __TABLES__ queries for freshness.
    "bigquery_use_batch_source_freshness",
];

pub fn make_behavior(
    adapter_type: AdapterType,
    behavior_flag_overrides: &BTreeMap<String, bool>,
) -> Arc<Behavior> {
    let mut behavior_flags = adapter_specific_behavior_flags(adapter_type);
    for flag in DEFAULT_BASE_BEHAVIOR_FLAGS.iter() {
        behavior_flags.push(flag.clone());
    }
    for key in behavior_flag_overrides.keys() {
        if !behavior_flags.iter().any(|f| f.name == key)
            && REMOVED_IN_FUSION.contains(&key.as_str())
        {
            // Suppressed until dbt Core v1.12 ships support for Fusion warning names in
            // warn_error_options, so users running both engines can silence cross-engine
            // warnings without breaking Core.
            // emit_warn_log_message(
            //     ErrorCode::InvalidConfig,
            //     format!(
            //         "Behavior flag '{key}' has been removed in dbt Fusion. \
            //          This flag can be safely removed from your dbt_project.yml."
            //     ),
            //     None,
            // );
        }
    }
    Arc::new(Behavior::new(behavior_flags, behavior_flag_overrides))
}
