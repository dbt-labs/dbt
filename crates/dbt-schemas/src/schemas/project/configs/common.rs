use dbt_adapter_core::AdapterType;
use dbt_proc_macros::DefaultTo;
use dbt_yaml::DbtSchema;
use serde::{Deserialize, Serialize};
// Type aliases for clarity
type YmlValue = dbt_yaml::Value;
use indexmap::IndexMap;
use serde_with::skip_serializing_none;
use std::collections::BTreeMap;

use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::emit_error_log_message;
use dbt_common::tracing::emit::emit_trace_event;
use dbt_telemetry::StateModifiedDiff;

use crate::schemas::common::{ClusterConfig, DocsConfig, Schedule};
use crate::schemas::common::{PartitionConfig, RowFilterConfig};
use crate::schemas::manifest::GrantAccessToTarget;
use crate::schemas::project::configs::model_config::DataLakeObjectCategory;
use crate::schemas::project::dbt_project::{ResolvableConfig, ResolvedConfig};
use crate::schemas::serde::PartitionsConfig;
use crate::schemas::serde::QueryTag;
use crate::schemas::serde::RefreshableConfig;
use crate::schemas::serde::StringOrArrayOfStrings;
use crate::schemas::serde::{
    IndexesConfig, OmissibleGrantConfig, PrimaryKeyConfig, StringOrInteger, bool_or_string_bool,
    f64_or_string_f64, hours_to_expiration_or_string_omissible, u64_or_string_u64,
};

#[track_caller]
pub fn log_state_mod_diff<I>(unique_id: impl AsRef<str>, node_type: impl AsRef<str>, checks: I)
where
    I: IntoIterator<Item = (&'static str, bool, Option<(String, String)>)>,
{
    let unique_id = unique_id.as_ref();
    let node_type = node_type.as_ref();

    for check in checks {
        let (check_name, check_result, values) = check;
        if check_result {
            continue;
        }

        let (self_value, other_value) = values
            .map(|(self_value, other_value)| (Some(self_value), Some(other_value)))
            .unwrap_or((None, None));

        emit_trace_event(|| {
            (
                StateModifiedDiff {
                    unique_id: Some(unique_id.to_string()),
                    node_type_or_category: node_type.to_string(),
                    check: check_name.to_string(),
                    self_value,
                    other_value,
                }
                .into(),
                None,
            )
        });
    }
}

/// Compare Option<StringOrArrayOfStrings>, treating None and empty array as equal
pub fn array_of_strings_eq(
    a: &Option<StringOrArrayOfStrings>,
    b: &Option<StringOrArrayOfStrings>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a_val), Some(b_val)) => a_val == b_val,
        (None, Some(StringOrArrayOfStrings::ArrayOfStrings(values))) => values.is_empty(),
        (Some(StringOrArrayOfStrings::ArrayOfStrings(values)), None) => values.is_empty(),
        _ => false,
    }
}

/// Compare plain `Vec<String>` tag fields (e.g. `CommonAttributes.tags`) with set semantics.
///
/// dbt-core builds tag lists by *concatenating* inherited tags (project + model +
/// column + test level), which produces duplicates in the manifest — e.g. a column
/// with `tags: [weekly]` under a model with `tags: [weekly]` ends up serialized as
/// `tags: ['weekly', 'weekly']`. Fusion deduplicates. For `state:modified` parity
/// against dbt-core-produced manifests, tag equality must ignore both ordering and
/// multiplicity, since tags are conceptually a set (selection via `tag:foo` is set
/// membership, not a count).
///
/// Use this only for tag-shaped fields. For ordered/multiset fields like Python
/// `packages` (where order or duplicates can be meaningful), use
/// `array_of_strings_eq` instead.
pub fn tags_eq_vec(a: &[String], b: &[String]) -> bool {
    use std::collections::BTreeSet;
    a.iter().cloned().collect::<BTreeSet<_>>() == b.iter().cloned().collect::<BTreeSet<_>>()
}

/// This configuration is a superset of all warehouse specific configurations
/// that users can set
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, DbtSchema, DefaultTo)]
pub struct WarehouseSpecificNodeConfig {
    // Shared
    pub partition_by: Option<PartitionConfig>,
    pub cluster_by: Option<ClusterConfig>,
    pub adapter_properties: Option<BTreeMap<String, YmlValue>>,

    // BigQuery
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "hours_to_expiration_or_string_omissible",
        skip_serializing_if = "Omissible::is_omitted"
    )]
    pub hours_to_expiration: Omissible<Option<StringOrInteger>>,
    #[serde(default, deserialize_with = "u64_or_string_u64")]
    pub job_execution_timeout_seconds: Option<u64>,
    pub reservation: Option<String>,
    pub labels: Option<IndexMap<String, String>>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub labels_from_meta: Option<bool>,
    pub kms_key_name: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub require_partition_filter: Option<bool>,
    #[serde(default, deserialize_with = "u64_or_string_u64")]
    pub partition_expiration_days: Option<u64>,
    pub grant_access_to: Option<Vec<GrantAccessToTarget>>,
    pub partitions: Option<PartitionsConfig>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enable_refresh: Option<bool>,
    #[serde(default, deserialize_with = "f64_or_string_f64")]
    pub refresh_interval_minutes: Option<f64>,
    pub resource_tags: Option<IndexMap<String, String>>,
    pub max_staleness: Option<String>,
    pub jar_file_uri: Option<String>,
    pub timeout: Option<u64>,
    pub batch_id: Option<String>,
    pub dataproc_cluster_name: Option<String>,
    #[serde(default, deserialize_with = "u64_or_string_u64")]
    pub notebook_template_id: Option<u64>,
    pub intermediate_format: Option<String>,
    pub enable_list_inference: Option<bool>,
    pub storage_uri: Option<String>,

    // Used by both Databricks and Bigquery
    pub file_format: Option<String>,

    // Databricks
    pub catalog_name: Option<String>,
    pub location_root: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub use_uniform: Option<bool>,
    pub tblproperties: Option<IndexMap<String, YmlValue>>,
    // this config is introduced here https://github.com/databricks/dbt-databricks/pull/823
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub include_full_name_in_path: Option<bool>,
    pub liquid_clustered_by: Option<StringOrArrayOfStrings>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub auto_liquid_cluster: Option<bool>,
    pub zorder: Option<StringOrArrayOfStrings>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub skip_optimize: Option<bool>,
    pub clustered_by: Option<StringOrArrayOfStrings>,
    pub buckets: Option<i64>,
    pub catalog: Option<String>,
    pub databricks_tags: Option<IndexMap<String, YmlValue>>,
    pub query_tags: Option<String>,
    pub compression: Option<String>,
    pub databricks_compute: Option<String>,
    pub target_alias: Option<String>,
    pub source_alias: Option<String>,
    pub matched_condition: Option<String>,
    pub not_matched_condition: Option<String>,
    pub not_matched_by_source_condition: Option<String>,
    pub not_matched_by_source_action: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub merge_with_schema_evolution: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub skip_matched_step: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub skip_not_matched_step: Option<bool>,
    pub schedule: Option<Schedule>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub incremental_apply_config_changes: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub persist_constraints: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub use_safer_relation_operations: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub view_update_via_alter: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub unique_tmp_table_suffix: Option<bool>,
    pub row_filter: Option<RowFilterConfig>,

    // Snowflake
    pub table_tag: Option<String>,
    pub row_access_policy: Option<String>,
    pub external_volume: Option<String>,
    pub base_location_root: Option<String>,
    pub base_location_subpath: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub change_tracking: Option<bool>,
    #[serde(default, deserialize_with = "u64_or_string_u64")]
    pub data_retention_time_in_days: Option<u64>,
    #[serde(default, deserialize_with = "u64_or_string_u64")]
    pub max_data_extension_time_in_days: Option<u64>,
    pub storage_serialization_policy: Option<String>,
    pub target_file_size: Option<String>,
    pub target_lag: Option<String>,
    pub snowflake_initialization_warehouse: Option<String>,
    pub snowflake_warehouse: Option<String>,
    pub refresh_warehouse: Option<String>,
    pub immutable_where: Option<String>,
    pub refresh_mode: Option<String>,
    pub initialize: Option<String>,
    pub scheduler: Option<String>,
    pub tmp_relation_type: Option<String>,
    pub query_tag: Option<QueryTag>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub automatic_clustering: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub copy_grants: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub copy_tags: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub secure: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub transient: Option<bool>,
    #[serde(default, deserialize_with = "u64_or_string_u64")]
    pub iceberg_version: Option<u64>,

    // Redshift
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub auto_refresh: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub backup: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub bind: Option<bool>,
    pub dist: Option<StringOrArrayOfStrings>,
    pub sort: Option<StringOrArrayOfStrings>,
    pub sort_type: Option<String>,

    // MsSql
    // XXX: This is an incomplete set of configs
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub as_columnstore: Option<bool>,

    // Athena
    // XXX: This is an incomplete set of configs
    pub table_type: Option<String>,

    // Postgres
    // XXX: This is an incomplete set of configs
    #[serde(default, skip_serializing_if = "IndexesConfig::is_none")]
    pub indexes: IndexesConfig,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub unlogged: Option<bool>,

    // Salesforce
    #[serde(default, skip_serializing_if = "PrimaryKeyConfig::is_none")]
    pub primary_key: PrimaryKeyConfig,
    pub category: Option<DataLakeObjectCategory>,

    // ClickHouse
    // table materialization
    pub engine: Option<String>,
    pub order_by: Option<StringOrArrayOfStrings>,
    pub ttl: Option<String>,
    pub settings: Option<BTreeMap<String, YmlValue>>,
    pub query_settings: Option<BTreeMap<String, YmlValue>>,
    pub projections: Option<Vec<YmlValue>>,
    // incremental materialization
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub inserts_only: Option<bool>,
    // dictionary materialization
    pub connection_overrides: Option<BTreeMap<String, YmlValue>>,
    pub fields: Option<Vec<YmlValue>>,
    pub source_type: Option<String>,
    pub url: Option<String>,
    pub format: Option<String>,
    pub layout: Option<String>,
    pub lifetime: Option<YmlValue>,
    pub range: Option<YmlValue>,
    pub table: Option<String>,
    pub update_field: Option<String>,
    pub update_lag: Option<YmlValue>,
    // view materialization
    pub definer: Option<String>,
    pub sql_security: Option<String>,
    // materialized-view materialization
    pub refreshable: Option<RefreshableConfig>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub catchup: Option<bool>,
    pub mv_on_schema_change: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub repopulate_from_mvs_on_full_refresh: Option<bool>,

    // Exasol
    // Key names match the Python dbt-exasol adapter so existing projects
    // migrate without config changes.
    pub partition_by_config: Option<StringOrArrayOfStrings>,
    pub distribute_by_config: Option<StringOrArrayOfStrings>,
    pub primary_key_config: Option<StringOrArrayOfStrings>,
}

impl ResolvedConfig for WarehouseSpecificNodeConfig {
    fn enabled(&self) -> bool {
        true
    }
}

/// Takes the Databricks `catalog` alias value out of `warehouse_specific`, for the caller to
/// move into its own `database` field -- mirroring dbt-core's `Credentials._ALIASES`
/// (`catalog` -> `database`, `dbt/adapters/databricks/credentials.py:69-72`). `None` on every
/// other adapter (D1) or when `catalog` was not set at this config layer.
///
/// Shared by every config type that embeds [`WarehouseSpecificNodeConfig`] and has its own
/// `database` field, since that field's exact type (`Option<String>` vs
/// `Omissible<Option<String>>`) differs by config type -- callers pass whether their own
/// `database` is already set and wrap the returned value themselves.
pub fn take_databricks_catalog_alias(
    adapter_type: AdapterType,
    warehouse_specific: &mut WarehouseSpecificNodeConfig,
    database_already_set: bool,
) -> Option<String> {
    if adapter_type != AdapterType::Databricks {
        return None;
    }
    if database_already_set {
        if warehouse_specific.catalog.is_some() {
            emit_error_log_message(
                ErrorCode::InvalidConfig,
                "Config keys `catalog` and `database` both resolve to `database` for adapter \
                 'databricks'; a project cannot set the same underlying config key two different \
                 ways in the same place.",
            );
        }
        return None;
    }
    warehouse_specific.catalog.take()
}

impl ResolvableConfig<WarehouseSpecificNodeConfig> for WarehouseSpecificNodeConfig {
    type Resolved = Self;
    type PackageDefaults = ();
    type ResolveDefaults = ();

    fn get_enabled_with_default(&self) -> bool {
        true
    }

    fn disable(&mut self) {}

    fn apply_package_defaults(&mut self, _: ()) {}

    fn finalize(self) -> Self {
        self
    }

    fn default_to(&mut self, parent: &WarehouseSpecificNodeConfig) {
        // Per-field inheritance is generated by `#[derive(DefaultTo)]`. Omissible
        // fields (e.g. `hours_to_expiration`) inherit only when omitted, so an
        // explicit `null` clears the inherited value (dbt-core#15473).
        self.default_to_fields(parent);
    }
}

// Shared comparison helper functions
use crate::schemas::common::Access;
use dbt_common::serde_utils::Omissible;

/// Helper function to compare Omissible<Option<T>> fields
pub fn omissible_option_eq<T: PartialEq>(
    a: &Omissible<Option<T>>,
    b: &Omissible<Option<T>>,
) -> bool {
    match (a, b) {
        // Both omitted
        (Omissible::Omitted, Omissible::Omitted) => true,
        // Both present
        (Omissible::Present(a_val), Omissible::Present(b_val)) => a_val == b_val,
        // One omitted, one present with None - treat as equivalent
        (Omissible::Omitted, Omissible::Present(None)) => true,
        (Omissible::Present(None), Omissible::Omitted) => true,
        // Any other combination is not equal
        _ => false,
    }
}

/// Helper function to compare docs fields, treating None and default DocsConfig as equivalent
pub fn docs_eq(a: &Option<DocsConfig>, b: &Option<DocsConfig>) -> bool {
    // Default value in dbt-core
    // See https://github.com/dbt-labs/dbt-core/blob/b75d5e701ef4dc2d7a98c5301ef63ecfc02eae15/core/dbt/artifacts/resources/base.py#L65
    let default_docs = DocsConfig {
        show: true,
        node_color: None,
    };

    match (a, b) {
        // Both None
        (None, None) => true,
        // Both Some - direct comparison
        (Some(a_docs), Some(b_docs)) => a_docs == b_docs,
        // One None, one Some - check if the Some value equals default
        (None, Some(b_docs)) => b_docs == &default_docs,
        (Some(a_docs), None) => a_docs == &default_docs,
    }
}

/// Helper function to compare access fields, treating None and default Access as equivalent
pub fn access_eq(a: &Option<Access>, b: &Option<Access>) -> bool {
    // Default value in dbt-core is "protected"
    // See https://github.com/dbt-labs/dbt-core/blob/main/core/dbt/artifacts/resources/v1/model.py#L72-L75
    let default_access = Access::Protected;

    match (a, b) {
        // Both None
        (None, None) => true,
        // Both Some - direct comparison
        (Some(a_val), Some(b_val)) => a_val == b_val,
        // One None, one Some - check if the Some value equals default
        (None, Some(b_val)) => b_val == &default_access,
        (Some(a_val), None) => a_val == &default_access,
    }
}

/// Helper function to compare meta fields, treating None and empty IndexMap as equivalent
pub fn meta_eq(
    a: &Option<IndexMap<String, YmlValue>>,
    b: &Option<IndexMap<String, YmlValue>>,
) -> bool {
    match (a, b) {
        // Both None
        (None, None) => true,
        // Both Some - direct comparison
        (Some(a_val), Some(b_val)) => a_val == b_val,
        // One None, one Some - check if the Some value is empty (equals default)
        (None, Some(b_val)) => b_val.is_empty(),
        (Some(a_val), None) => a_val.is_empty(),
    }
}

/// Helper function to compare grants fields, treating Omitted and empty as equivalent
pub fn grants_equal(a: &OmissibleGrantConfig, b: &OmissibleGrantConfig) -> bool {
    match (a.as_ref(), b.as_ref()) {
        (None, None) => true,
        (Some(a_val), Some(b_val)) => a_val == b_val,
        (None, Some(b_val)) => b_val.is_empty(),
        (Some(a_val), None) => a_val.is_empty(),
    }
}

/// Compare warehouse-specific configurations field by field
pub fn same_warehouse_config(
    self_wh: &WarehouseSpecificNodeConfig,
    other_wh: &WarehouseSpecificNodeConfig,
) -> bool {
    let partition_by_eq = self_wh.partition_by == other_wh.partition_by;
    let cluster_by_eq = self_wh.cluster_by == other_wh.cluster_by;
    let hours_to_expiration_eq = self_wh.hours_to_expiration == other_wh.hours_to_expiration;
    let job_execution_timeout_seconds_eq =
        self_wh.job_execution_timeout_seconds == other_wh.job_execution_timeout_seconds;
    let reservation_eq = self_wh.reservation == other_wh.reservation;
    let labels_eq = self_wh.labels == other_wh.labels;
    let labels_from_meta_eq = self_wh.labels_from_meta == other_wh.labels_from_meta;
    let kms_key_name_eq = self_wh.kms_key_name == other_wh.kms_key_name;
    let require_partition_filter_eq =
        self_wh.require_partition_filter == other_wh.require_partition_filter;
    let partition_expiration_days_eq =
        self_wh.partition_expiration_days == other_wh.partition_expiration_days;
    let grant_access_to_eq = self_wh.grant_access_to == other_wh.grant_access_to;
    let partitions_eq = self_wh.partitions == other_wh.partitions;
    let enable_refresh_eq = self_wh.enable_refresh == other_wh.enable_refresh;
    let refresh_interval_minutes_eq =
        self_wh.refresh_interval_minutes == other_wh.refresh_interval_minutes;
    let max_staleness_eq = self_wh.max_staleness == other_wh.max_staleness;
    let file_format_eq = self_wh.file_format == other_wh.file_format;
    let catalog_name_eq = self_wh.catalog_name == other_wh.catalog_name;
    let location_root_eq = self_wh.location_root == other_wh.location_root;
    let tblproperties_eq = self_wh.tblproperties == other_wh.tblproperties;
    let include_full_name_in_path_eq =
        self_wh.include_full_name_in_path == other_wh.include_full_name_in_path;
    let liquid_clustered_by_eq = self_wh.liquid_clustered_by == other_wh.liquid_clustered_by;
    let auto_liquid_cluster_eq = self_wh.auto_liquid_cluster == other_wh.auto_liquid_cluster;
    let zorder_eq = self_wh.zorder == other_wh.zorder;
    let skip_optimize_eq = self_wh.skip_optimize == other_wh.skip_optimize;
    let clustered_by_eq = self_wh.clustered_by == other_wh.clustered_by;
    let buckets_eq = self_wh.buckets == other_wh.buckets;
    let catalog_eq = self_wh.catalog == other_wh.catalog;
    let databricks_tags_eq = self_wh.databricks_tags == other_wh.databricks_tags;
    let query_tags_eq = self_wh.query_tags == other_wh.query_tags;
    let compression_eq = self_wh.compression == other_wh.compression;
    let databricks_compute_eq = self_wh.databricks_compute == other_wh.databricks_compute;
    let target_alias_eq = self_wh.target_alias == other_wh.target_alias;
    let source_alias_eq = self_wh.source_alias == other_wh.source_alias;
    let matched_condition_eq = self_wh.matched_condition == other_wh.matched_condition;
    let not_matched_condition_eq = self_wh.not_matched_condition == other_wh.not_matched_condition;
    let not_matched_by_source_condition_eq =
        self_wh.not_matched_by_source_condition == other_wh.not_matched_by_source_condition;
    let not_matched_by_source_action_eq =
        self_wh.not_matched_by_source_action == other_wh.not_matched_by_source_action;
    let merge_with_schema_evolution_eq =
        self_wh.merge_with_schema_evolution == other_wh.merge_with_schema_evolution;
    let skip_matched_step_eq = self_wh.skip_matched_step == other_wh.skip_matched_step;
    let skip_not_matched_step_eq = self_wh.skip_not_matched_step == other_wh.skip_not_matched_step;
    let persist_constraints_eq = self_wh.persist_constraints == other_wh.persist_constraints;
    let unique_tmp_table_suffix_eq =
        self_wh.unique_tmp_table_suffix == other_wh.unique_tmp_table_suffix;
    let schedule_eq = self_wh.schedule == other_wh.schedule;
    let adapter_properties_eq = self_wh.adapter_properties == other_wh.adapter_properties;
    let table_tag_eq = self_wh.table_tag == other_wh.table_tag;
    let row_access_policy_eq = self_wh.row_access_policy == other_wh.row_access_policy;
    let external_volume_eq = self_wh.external_volume == other_wh.external_volume;
    let base_location_root_eq = self_wh.base_location_root == other_wh.base_location_root;
    let base_location_subpath_eq = self_wh.base_location_subpath == other_wh.base_location_subpath;
    let target_lag_eq = self_wh.target_lag == other_wh.target_lag;
    let snowflake_initialization_warehouse_eq =
        self_wh.snowflake_initialization_warehouse == other_wh.snowflake_initialization_warehouse;
    let refresh_warehouse_eq = self_wh.refresh_warehouse == other_wh.refresh_warehouse;
    let immutable_where_eq = self_wh.immutable_where == other_wh.immutable_where;
    let refresh_mode_eq = self_wh.refresh_mode == other_wh.refresh_mode;
    let initialize_eq = self_wh.initialize == other_wh.initialize;
    let scheduler_eq = self_wh.scheduler == other_wh.scheduler;
    let tmp_relation_type_eq = self_wh.tmp_relation_type == other_wh.tmp_relation_type;
    let query_tag_eq = self_wh.query_tag == other_wh.query_tag;
    let automatic_clustering_eq = self_wh.automatic_clustering == other_wh.automatic_clustering;
    let copy_grants_eq = self_wh.copy_grants == other_wh.copy_grants;
    let copy_tags_eq = self_wh.copy_tags == other_wh.copy_tags;
    let secure_eq = self_wh.secure == other_wh.secure;
    let transient_eq = self_wh.transient == other_wh.transient;
    let iceberg_version_eq = self_wh.iceberg_version == other_wh.iceberg_version;
    let auto_refresh_eq = self_wh.auto_refresh == other_wh.auto_refresh;
    let backup_eq = self_wh.backup == other_wh.backup;
    let bind_eq = self_wh.bind == other_wh.bind;
    let dist_eq = self_wh.dist == other_wh.dist;
    let sort_eq = self_wh.sort == other_wh.sort;
    let sort_type_eq = self_wh.sort_type == other_wh.sort_type;
    let as_columnstore_eq = self_wh.as_columnstore == other_wh.as_columnstore;
    let table_type_eq = self_wh.table_type == other_wh.table_type;
    let indexes_eq = self_wh.indexes == other_wh.indexes;
    let primary_key_eq = self_wh.primary_key == other_wh.primary_key;
    let category_eq = self_wh.category == other_wh.category;
    let engine_eq = self_wh.engine == other_wh.engine;
    let order_by_eq = self_wh.order_by == other_wh.order_by;
    let ttl_eq = self_wh.ttl == other_wh.ttl;
    let settings_eq = self_wh.settings == other_wh.settings;
    let query_settings_eq = self_wh.query_settings == other_wh.query_settings;
    let projections_eq = self_wh.projections == other_wh.projections;
    let inserts_only_eq = self_wh.inserts_only == other_wh.inserts_only;
    let connection_overrides_eq = self_wh.connection_overrides == other_wh.connection_overrides;
    let fields_eq = self_wh.fields == other_wh.fields;
    let source_type_eq = self_wh.source_type == other_wh.source_type;
    let url_eq = self_wh.url == other_wh.url;
    let format_eq = self_wh.format == other_wh.format;
    let layout_eq = self_wh.layout == other_wh.layout;
    let lifetime_eq = self_wh.lifetime == other_wh.lifetime;
    let range_eq = self_wh.range == other_wh.range;
    let table_eq = self_wh.table == other_wh.table;
    let update_field_eq = self_wh.update_field == other_wh.update_field;
    let update_lag_eq = self_wh.update_lag == other_wh.update_lag;
    let definer_eq = self_wh.definer == other_wh.definer;
    let sql_security_eq = self_wh.sql_security == other_wh.sql_security;
    let refreshable_eq = self_wh.refreshable == other_wh.refreshable;
    let catchup_eq = self_wh.catchup == other_wh.catchup;
    let mv_on_schema_change_eq = self_wh.mv_on_schema_change == other_wh.mv_on_schema_change;
    let repopulate_from_mvs_on_full_refresh_eq =
        self_wh.repopulate_from_mvs_on_full_refresh == other_wh.repopulate_from_mvs_on_full_refresh;

    let result = partition_by_eq
        && cluster_by_eq
        && hours_to_expiration_eq
        && job_execution_timeout_seconds_eq
        && reservation_eq
        && labels_eq
        && labels_from_meta_eq
        && kms_key_name_eq
        && require_partition_filter_eq
        && partition_expiration_days_eq
        && grant_access_to_eq
        && partitions_eq
        && enable_refresh_eq
        && refresh_interval_minutes_eq
        && max_staleness_eq
        && file_format_eq
        && catalog_name_eq
        && location_root_eq
        && tblproperties_eq
        && include_full_name_in_path_eq
        && liquid_clustered_by_eq
        && auto_liquid_cluster_eq
        && zorder_eq
        && skip_optimize_eq
        && clustered_by_eq
        && buckets_eq
        && catalog_eq
        && databricks_tags_eq
        && query_tags_eq
        && compression_eq
        && databricks_compute_eq
        && target_alias_eq
        && source_alias_eq
        && matched_condition_eq
        && not_matched_condition_eq
        && not_matched_by_source_condition_eq
        && not_matched_by_source_action_eq
        && merge_with_schema_evolution_eq
        && skip_matched_step_eq
        && skip_not_matched_step_eq
        && persist_constraints_eq
        && unique_tmp_table_suffix_eq
        && schedule_eq
        && adapter_properties_eq
        && table_tag_eq
        && row_access_policy_eq
        && external_volume_eq
        && base_location_root_eq
        && base_location_subpath_eq
        && target_lag_eq
        && snowflake_initialization_warehouse_eq
        && refresh_warehouse_eq
        && immutable_where_eq
        && refresh_mode_eq
        && initialize_eq
        && scheduler_eq
        && tmp_relation_type_eq
        && query_tag_eq
        && automatic_clustering_eq
        && copy_grants_eq
        && copy_tags_eq
        && secure_eq
        && transient_eq
        && iceberg_version_eq
        && auto_refresh_eq
        && backup_eq
        && bind_eq
        && dist_eq
        && sort_eq
        && sort_type_eq
        && as_columnstore_eq
        && table_type_eq
        && indexes_eq
        && primary_key_eq
        && category_eq
        && engine_eq
        && order_by_eq
        && ttl_eq
        && settings_eq
        && query_settings_eq
        && projections_eq
        && inserts_only_eq
        && connection_overrides_eq
        && fields_eq
        && source_type_eq
        && url_eq
        && format_eq
        && layout_eq
        && lifetime_eq
        && range_eq
        && table_eq
        && update_field_eq
        && update_lag_eq
        && definer_eq
        && sql_security_eq
        && refreshable_eq
        && catchup_eq
        && mv_on_schema_change_eq
        && repopulate_from_mvs_on_full_refresh_eq;

    if !result {
        log_state_mod_diff(
            "unique_id in next config log",
            "warehouse_config",
            [
                (
                    "partition_by",
                    partition_by_eq,
                    Some((
                        format!("{:?}", &self_wh.partition_by),
                        format!("{:?}", &other_wh.partition_by),
                    )),
                ),
                (
                    "cluster_by",
                    cluster_by_eq,
                    Some((
                        format!("{:?}", &self_wh.cluster_by),
                        format!("{:?}", &other_wh.cluster_by),
                    )),
                ),
                (
                    "hours_to_expiration",
                    hours_to_expiration_eq,
                    Some((
                        format!("{:?}", &self_wh.hours_to_expiration),
                        format!("{:?}", &other_wh.hours_to_expiration),
                    )),
                ),
                (
                    "job_execution_timeout_seconds",
                    job_execution_timeout_seconds_eq,
                    Some((
                        format!("{:?}", &self_wh.job_execution_timeout_seconds),
                        format!("{:?}", &other_wh.job_execution_timeout_seconds),
                    )),
                ),
                (
                    "reservation",
                    reservation_eq,
                    Some((
                        format!("{:?}", &self_wh.reservation),
                        format!("{:?}", &other_wh.reservation),
                    )),
                ),
                (
                    "labels",
                    labels_eq,
                    Some((
                        format!("{:?}", &self_wh.labels),
                        format!("{:?}", &other_wh.labels),
                    )),
                ),
                (
                    "labels_from_meta",
                    labels_from_meta_eq,
                    Some((
                        format!("{:?}", &self_wh.labels_from_meta),
                        format!("{:?}", &other_wh.labels_from_meta),
                    )),
                ),
                (
                    "kms_key_name",
                    kms_key_name_eq,
                    Some((
                        format!("{:?}", &self_wh.kms_key_name),
                        format!("{:?}", &other_wh.kms_key_name),
                    )),
                ),
                (
                    "require_partition_filter",
                    require_partition_filter_eq,
                    Some((
                        format!("{:?}", &self_wh.require_partition_filter),
                        format!("{:?}", &other_wh.require_partition_filter),
                    )),
                ),
                (
                    "partition_expiration_days",
                    partition_expiration_days_eq,
                    Some((
                        format!("{:?}", &self_wh.partition_expiration_days),
                        format!("{:?}", &other_wh.partition_expiration_days),
                    )),
                ),
                (
                    "grant_access_to",
                    grant_access_to_eq,
                    Some((
                        format!("{:?}", &self_wh.grant_access_to),
                        format!("{:?}", &other_wh.grant_access_to),
                    )),
                ),
                (
                    "partitions",
                    partitions_eq,
                    Some((
                        format!("{:?}", &self_wh.partitions),
                        format!("{:?}", &other_wh.partitions),
                    )),
                ),
                (
                    "enable_refresh",
                    enable_refresh_eq,
                    Some((
                        format!("{:?}", &self_wh.enable_refresh),
                        format!("{:?}", &other_wh.enable_refresh),
                    )),
                ),
                (
                    "refresh_interval_minutes",
                    refresh_interval_minutes_eq,
                    Some((
                        format!("{:?}", &self_wh.refresh_interval_minutes),
                        format!("{:?}", &other_wh.refresh_interval_minutes),
                    )),
                ),
                (
                    "max_staleness",
                    max_staleness_eq,
                    Some((
                        format!("{:?}", &self_wh.max_staleness),
                        format!("{:?}", &other_wh.max_staleness),
                    )),
                ),
                (
                    "file_format",
                    file_format_eq,
                    Some((
                        format!("{:?}", &self_wh.file_format),
                        format!("{:?}", &other_wh.file_format),
                    )),
                ),
                (
                    "catalog_name",
                    catalog_name_eq,
                    Some((
                        format!("{:?}", &self_wh.catalog_name),
                        format!("{:?}", &other_wh.catalog_name),
                    )),
                ),
                (
                    "location_root",
                    location_root_eq,
                    Some((
                        format!("{:?}", &self_wh.location_root),
                        format!("{:?}", &other_wh.location_root),
                    )),
                ),
                (
                    "tblproperties",
                    tblproperties_eq,
                    Some((
                        format!("{:?}", &self_wh.tblproperties),
                        format!("{:?}", &other_wh.tblproperties),
                    )),
                ),
                (
                    "include_full_name_in_path",
                    include_full_name_in_path_eq,
                    Some((
                        format!("{:?}", &self_wh.include_full_name_in_path),
                        format!("{:?}", &other_wh.include_full_name_in_path),
                    )),
                ),
                (
                    "liquid_clustered_by",
                    liquid_clustered_by_eq,
                    Some((
                        format!("{:?}", &self_wh.liquid_clustered_by),
                        format!("{:?}", &other_wh.liquid_clustered_by),
                    )),
                ),
                (
                    "auto_liquid_cluster",
                    auto_liquid_cluster_eq,
                    Some((
                        format!("{:?}", &self_wh.auto_liquid_cluster),
                        format!("{:?}", &other_wh.auto_liquid_cluster),
                    )),
                ),
                (
                    "zorder",
                    zorder_eq,
                    Some((
                        format!("{:?}", &self_wh.zorder),
                        format!("{:?}", &other_wh.zorder),
                    )),
                ),
                (
                    "skip_optimize",
                    skip_optimize_eq,
                    Some((
                        format!("{:?}", &self_wh.skip_optimize),
                        format!("{:?}", &other_wh.skip_optimize),
                    )),
                ),
                (
                    "clustered_by",
                    clustered_by_eq,
                    Some((
                        format!("{:?}", &self_wh.clustered_by),
                        format!("{:?}", &other_wh.clustered_by),
                    )),
                ),
                (
                    "buckets",
                    buckets_eq,
                    Some((
                        format!("{:?}", &self_wh.buckets),
                        format!("{:?}", &other_wh.buckets),
                    )),
                ),
                (
                    "catalog",
                    catalog_eq,
                    Some((
                        format!("{:?}", &self_wh.catalog),
                        format!("{:?}", &other_wh.catalog),
                    )),
                ),
                (
                    "databricks_tags",
                    databricks_tags_eq,
                    Some((
                        format!("{:?}", &self_wh.databricks_tags),
                        format!("{:?}", &other_wh.databricks_tags),
                    )),
                ),
                (
                    "compression",
                    compression_eq,
                    Some((
                        format!("{:?}", &self_wh.compression),
                        format!("{:?}", &other_wh.compression),
                    )),
                ),
                (
                    "databricks_compute",
                    databricks_compute_eq,
                    Some((
                        format!("{:?}", &self_wh.databricks_compute),
                        format!("{:?}", &other_wh.databricks_compute),
                    )),
                ),
                (
                    "target_alias",
                    target_alias_eq,
                    Some((
                        format!("{:?}", &self_wh.target_alias),
                        format!("{:?}", &other_wh.target_alias),
                    )),
                ),
                (
                    "source_alias",
                    source_alias_eq,
                    Some((
                        format!("{:?}", &self_wh.source_alias),
                        format!("{:?}", &other_wh.source_alias),
                    )),
                ),
                (
                    "matched_condition",
                    matched_condition_eq,
                    Some((
                        format!("{:?}", &self_wh.matched_condition),
                        format!("{:?}", &other_wh.matched_condition),
                    )),
                ),
                (
                    "not_matched_condition",
                    not_matched_condition_eq,
                    Some((
                        format!("{:?}", &self_wh.not_matched_condition),
                        format!("{:?}", &other_wh.not_matched_condition),
                    )),
                ),
                (
                    "not_matched_by_source_condition",
                    not_matched_by_source_condition_eq,
                    Some((
                        format!("{:?}", &self_wh.not_matched_by_source_condition),
                        format!("{:?}", &other_wh.not_matched_by_source_condition),
                    )),
                ),
                (
                    "not_matched_by_source_action",
                    not_matched_by_source_action_eq,
                    Some((
                        format!("{:?}", &self_wh.not_matched_by_source_action),
                        format!("{:?}", &other_wh.not_matched_by_source_action),
                    )),
                ),
                (
                    "merge_with_schema_evolution",
                    merge_with_schema_evolution_eq,
                    Some((
                        format!("{:?}", &self_wh.merge_with_schema_evolution),
                        format!("{:?}", &other_wh.merge_with_schema_evolution),
                    )),
                ),
                (
                    "skip_matched_step",
                    skip_matched_step_eq,
                    Some((
                        format!("{:?}", &self_wh.skip_matched_step),
                        format!("{:?}", &other_wh.skip_matched_step),
                    )),
                ),
                (
                    "skip_not_matched_step",
                    skip_not_matched_step_eq,
                    Some((
                        format!("{:?}", &self_wh.skip_not_matched_step),
                        format!("{:?}", &other_wh.skip_not_matched_step),
                    )),
                ),
                (
                    "persist_constraints",
                    persist_constraints_eq,
                    Some((
                        format!("{:?}", &self_wh.persist_constraints),
                        format!("{:?}", &other_wh.persist_constraints),
                    )),
                ),
                (
                    "unique_tmp_table_suffix",
                    unique_tmp_table_suffix_eq,
                    Some((
                        format!("{:?}", &self_wh.unique_tmp_table_suffix),
                        format!("{:?}", &other_wh.unique_tmp_table_suffix),
                    )),
                ),
                (
                    "schedule",
                    schedule_eq,
                    Some((
                        format!("{:?}", &self_wh.schedule),
                        format!("{:?}", &other_wh.schedule),
                    )),
                ),
                (
                    "adapter_properties",
                    adapter_properties_eq,
                    Some((
                        format!("{:?}", &self_wh.adapter_properties),
                        format!("{:?}", &other_wh.adapter_properties),
                    )),
                ),
                (
                    "table_tag",
                    table_tag_eq,
                    Some((
                        format!("{:?}", &self_wh.table_tag),
                        format!("{:?}", &other_wh.table_tag),
                    )),
                ),
                (
                    "row_access_policy",
                    row_access_policy_eq,
                    Some((
                        format!("{:?}", &self_wh.row_access_policy),
                        format!("{:?}", &other_wh.row_access_policy),
                    )),
                ),
                (
                    "external_volume",
                    external_volume_eq,
                    Some((
                        format!("{:?}", &self_wh.external_volume),
                        format!("{:?}", &other_wh.external_volume),
                    )),
                ),
                (
                    "base_location_root",
                    base_location_root_eq,
                    Some((
                        format!("{:?}", &self_wh.base_location_root),
                        format!("{:?}", &other_wh.base_location_root),
                    )),
                ),
                (
                    "base_location_subpath",
                    base_location_subpath_eq,
                    Some((
                        format!("{:?}", &self_wh.base_location_subpath),
                        format!("{:?}", &other_wh.base_location_subpath),
                    )),
                ),
                (
                    "target_lag",
                    target_lag_eq,
                    Some((
                        format!("{:?}", &self_wh.target_lag),
                        format!("{:?}", &other_wh.target_lag),
                    )),
                ),
                (
                    "snowflake_initialization_warehouse",
                    snowflake_initialization_warehouse_eq,
                    Some((
                        format!("{:?}", &self_wh.snowflake_initialization_warehouse),
                        format!("{:?}", &other_wh.snowflake_initialization_warehouse),
                    )),
                ),
                (
                    "refresh_warehouse",
                    refresh_warehouse_eq,
                    Some((
                        format!("{:?}", &self_wh.refresh_warehouse),
                        format!("{:?}", &other_wh.refresh_warehouse),
                    )),
                ),
                (
                    "immutable_where",
                    immutable_where_eq,
                    Some((
                        format!("{:?}", &self_wh.immutable_where),
                        format!("{:?}", &other_wh.immutable_where),
                    )),
                ),
                (
                    "refresh_mode",
                    refresh_mode_eq,
                    Some((
                        format!("{:?}", &self_wh.refresh_mode),
                        format!("{:?}", &other_wh.refresh_mode),
                    )),
                ),
                (
                    "initialize",
                    initialize_eq,
                    Some((
                        format!("{:?}", &self_wh.initialize),
                        format!("{:?}", &other_wh.initialize),
                    )),
                ),
                (
                    "scheduler",
                    scheduler_eq,
                    Some((
                        format!("{:?}", &self_wh.scheduler),
                        format!("{:?}", &other_wh.scheduler),
                    )),
                ),
                (
                    "tmp_relation_type",
                    tmp_relation_type_eq,
                    Some((
                        format!("{:?}", &self_wh.tmp_relation_type),
                        format!("{:?}", &other_wh.tmp_relation_type),
                    )),
                ),
                (
                    "query_tags",
                    query_tags_eq,
                    Some((
                        format!("{:?}", &self_wh.query_tags),
                        format!("{:?}", &other_wh.query_tags),
                    )),
                ),
                (
                    "query_tag",
                    query_tag_eq,
                    Some((
                        format!("{:?}", &self_wh.query_tag),
                        format!("{:?}", &other_wh.query_tag),
                    )),
                ),
                (
                    "automatic_clustering",
                    automatic_clustering_eq,
                    Some((
                        format!("{:?}", &self_wh.automatic_clustering),
                        format!("{:?}", &other_wh.automatic_clustering),
                    )),
                ),
                (
                    "copy_grants",
                    copy_grants_eq,
                    Some((
                        format!("{:?}", &self_wh.copy_grants),
                        format!("{:?}", &other_wh.copy_grants),
                    )),
                ),
                (
                    "copy_tags",
                    copy_tags_eq,
                    Some((
                        format!("{:?}", &self_wh.copy_tags),
                        format!("{:?}", &other_wh.copy_tags),
                    )),
                ),
                (
                    "secure",
                    secure_eq,
                    Some((
                        format!("{:?}", &self_wh.secure),
                        format!("{:?}", &other_wh.secure),
                    )),
                ),
                (
                    "transient",
                    transient_eq,
                    Some((
                        format!("{:?}", &self_wh.transient),
                        format!("{:?}", &other_wh.transient),
                    )),
                ),
                (
                    "iceberg_version",
                    iceberg_version_eq,
                    Some((
                        format!("{:?}", &self_wh.iceberg_version),
                        format!("{:?}", &other_wh.iceberg_version),
                    )),
                ),
                (
                    "auto_refresh",
                    auto_refresh_eq,
                    Some((
                        format!("{:?}", &self_wh.auto_refresh),
                        format!("{:?}", &other_wh.auto_refresh),
                    )),
                ),
                (
                    "backup",
                    backup_eq,
                    Some((
                        format!("{:?}", &self_wh.backup),
                        format!("{:?}", &other_wh.backup),
                    )),
                ),
                (
                    "bind",
                    bind_eq,
                    Some((
                        format!("{:?}", &self_wh.bind),
                        format!("{:?}", &other_wh.bind),
                    )),
                ),
                (
                    "dist",
                    dist_eq,
                    Some((
                        format!("{:?}", &self_wh.dist),
                        format!("{:?}", &other_wh.dist),
                    )),
                ),
                (
                    "sort",
                    sort_eq,
                    Some((
                        format!("{:?}", &self_wh.sort),
                        format!("{:?}", &other_wh.sort),
                    )),
                ),
                (
                    "sort_type",
                    sort_type_eq,
                    Some((
                        format!("{:?}", &self_wh.sort_type),
                        format!("{:?}", &other_wh.sort_type),
                    )),
                ),
                (
                    "as_columnstore",
                    as_columnstore_eq,
                    Some((
                        format!("{:?}", &self_wh.as_columnstore),
                        format!("{:?}", &other_wh.as_columnstore),
                    )),
                ),
                (
                    "table_type",
                    table_type_eq,
                    Some((
                        format!("{:?}", &self_wh.table_type),
                        format!("{:?}", &other_wh.table_type),
                    )),
                ),
                (
                    "indexes",
                    indexes_eq,
                    Some((
                        format!("{:?}", &self_wh.indexes),
                        format!("{:?}", &other_wh.indexes),
                    )),
                ),
                (
                    "primary_key",
                    primary_key_eq,
                    Some((
                        format!("{:?}", &self_wh.primary_key),
                        format!("{:?}", &other_wh.primary_key),
                    )),
                ),
                (
                    "category",
                    category_eq,
                    Some((
                        format!("{:?}", &self_wh.category),
                        format!("{:?}", &other_wh.category),
                    )),
                ),
                (
                    "engine",
                    engine_eq,
                    Some((
                        format!("{:?}", &self_wh.engine),
                        format!("{:?}", &other_wh.engine),
                    )),
                ),
                (
                    "order_by",
                    order_by_eq,
                    Some((
                        format!("{:?}", &self_wh.order_by),
                        format!("{:?}", &other_wh.order_by),
                    )),
                ),
                (
                    "ttl",
                    ttl_eq,
                    Some((
                        format!("{:?}", &self_wh.ttl),
                        format!("{:?}", &other_wh.ttl),
                    )),
                ),
                (
                    "settings",
                    settings_eq,
                    Some((
                        format!("{:?}", &self_wh.settings),
                        format!("{:?}", &other_wh.settings),
                    )),
                ),
                (
                    "query_settings",
                    query_settings_eq,
                    Some((
                        format!("{:?}", &self_wh.query_settings),
                        format!("{:?}", &other_wh.query_settings),
                    )),
                ),
                (
                    "projections",
                    projections_eq,
                    Some((
                        format!("{:?}", &self_wh.projections),
                        format!("{:?}", &other_wh.projections),
                    )),
                ),
                (
                    "inserts_only",
                    inserts_only_eq,
                    Some((
                        format!("{:?}", &self_wh.inserts_only),
                        format!("{:?}", &other_wh.inserts_only),
                    )),
                ),
                (
                    "connection_overrides",
                    connection_overrides_eq,
                    Some((
                        format!("{:?}", &self_wh.connection_overrides),
                        format!("{:?}", &other_wh.connection_overrides),
                    )),
                ),
                (
                    "fields",
                    fields_eq,
                    Some((
                        format!("{:?}", &self_wh.fields),
                        format!("{:?}", &other_wh.fields),
                    )),
                ),
                (
                    "source_type",
                    source_type_eq,
                    Some((
                        format!("{:?}", &self_wh.source_type),
                        format!("{:?}", &other_wh.source_type),
                    )),
                ),
                (
                    "url",
                    url_eq,
                    Some((
                        format!("{:?}", &self_wh.url),
                        format!("{:?}", &other_wh.url),
                    )),
                ),
                (
                    "format",
                    format_eq,
                    Some((
                        format!("{:?}", &self_wh.format),
                        format!("{:?}", &other_wh.format),
                    )),
                ),
                (
                    "layout",
                    layout_eq,
                    Some((
                        format!("{:?}", &self_wh.layout),
                        format!("{:?}", &other_wh.layout),
                    )),
                ),
                (
                    "lifetime",
                    lifetime_eq,
                    Some((
                        format!("{:?}", &self_wh.lifetime),
                        format!("{:?}", &other_wh.lifetime),
                    )),
                ),
                (
                    "range",
                    range_eq,
                    Some((
                        format!("{:?}", &self_wh.range),
                        format!("{:?}", &other_wh.range),
                    )),
                ),
                (
                    "table",
                    table_eq,
                    Some((
                        format!("{:?}", &self_wh.table),
                        format!("{:?}", &other_wh.table),
                    )),
                ),
                (
                    "update_field",
                    update_field_eq,
                    Some((
                        format!("{:?}", &self_wh.update_field),
                        format!("{:?}", &other_wh.update_field),
                    )),
                ),
                (
                    "update_lag",
                    update_lag_eq,
                    Some((
                        format!("{:?}", &self_wh.update_lag),
                        format!("{:?}", &other_wh.update_lag),
                    )),
                ),
                (
                    "definer",
                    definer_eq,
                    Some((
                        format!("{:?}", &self_wh.definer),
                        format!("{:?}", &other_wh.definer),
                    )),
                ),
                (
                    "sql_security",
                    sql_security_eq,
                    Some((
                        format!("{:?}", &self_wh.sql_security),
                        format!("{:?}", &other_wh.sql_security),
                    )),
                ),
                (
                    "refreshable",
                    refreshable_eq,
                    Some((
                        format!("{:?}", &self_wh.refreshable),
                        format!("{:?}", &other_wh.refreshable),
                    )),
                ),
                (
                    "catchup",
                    catchup_eq,
                    Some((
                        format!("{:?}", &self_wh.catchup),
                        format!("{:?}", &other_wh.catchup),
                    )),
                ),
                (
                    "mv_on_schema_change",
                    mv_on_schema_change_eq,
                    Some((
                        format!("{:?}", &self_wh.mv_on_schema_change),
                        format!("{:?}", &other_wh.mv_on_schema_change),
                    )),
                ),
                (
                    "repopulate_from_mvs_on_full_refresh",
                    repopulate_from_mvs_on_full_refresh_eq,
                    Some((
                        format!("{:?}", &self_wh.repopulate_from_mvs_on_full_refresh),
                        format!("{:?}", &other_wh.repopulate_from_mvs_on_full_refresh),
                    )),
                ),
            ],
        );
    }

    result
}

/// Compare two `unrendered_config` values, treating absent/`null`/empty as equivalent and
/// canonicalizing trailing newlines on strings. Mirrors the semantics used by
/// `check_configs_modified`'s unrendered path in `prev_state`.
pub(crate) fn unrendered_value_eq(a: Option<&YmlValue>, b: Option<&YmlValue>) -> bool {
    fn is_effectively_empty(v: &YmlValue) -> bool {
        match v {
            YmlValue::Null(_) => true,
            YmlValue::Sequence(seq, _) => seq.is_empty(),
            YmlValue::Mapping(map, _) => map.is_empty(),
            _ => false,
        }
    }

    fn canonicalize_str(s: &str) -> &str {
        s.strip_suffix("\r\n")
            .or_else(|| s.strip_suffix('\n'))
            .unwrap_or(s)
    }

    match (a, b) {
        (None, None) => true,
        (None, Some(v)) | (Some(v), None) => is_effectively_empty(v),
        (Some(YmlValue::String(sa, _)), Some(YmlValue::String(sb, _))) => {
            canonicalize_str(sa) == canonicalize_str(sb)
        }
        (Some(va), Some(vb)) => va == vb,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_take_databricks_catalog_alias_moves_catalog_when_database_unset() {
        let mut wh = WarehouseSpecificNodeConfig {
            catalog: Some("my_catalog".to_string()),
            ..Default::default()
        };
        let catalog = take_databricks_catalog_alias(AdapterType::Databricks, &mut wh, false);
        assert_eq!(catalog, Some("my_catalog".to_string()));
        assert_eq!(
            wh.catalog, None,
            "catalog must be cleared once moved into database"
        );
    }

    /// An explicit `database` at the *same* config layer takes precedence over the `catalog`
    /// alias (ordinary same-source-dict alias precedence) -- `catalog` is left untouched rather
    /// than allowed to clobber the explicit value. This is also the D4 same-layer-duplicate case,
    /// which now additionally emits a hard parse error (not observable from this test, which
    /// installs no tracing subscriber); see
    /// `test_databricks_catalog_alias_duplicate_at_same_layer_errors` (`dbt-parser/src/tests.rs`)
    /// for that assertion.
    #[test]
    fn test_take_databricks_catalog_alias_defers_to_an_explicit_database_at_the_same_layer() {
        let mut wh = WarehouseSpecificNodeConfig {
            catalog: Some("my_catalog".to_string()),
            ..Default::default()
        };
        let catalog = take_databricks_catalog_alias(AdapterType::Databricks, &mut wh, true);
        assert_eq!(catalog, None);
        assert_eq!(
            wh.catalog,
            Some("my_catalog".to_string()),
            "catalog must be left untouched when database is already explicitly set"
        );
    }

    #[test]
    fn test_array_of_strings_eq_none_and_empty_array() {
        let none_val: Option<StringOrArrayOfStrings> = None;
        let empty_array = Some(StringOrArrayOfStrings::ArrayOfStrings(vec![]));

        assert!(array_of_strings_eq(&none_val, &empty_array));
        assert!(array_of_strings_eq(&empty_array, &none_val));
    }

    #[test]
    fn test_array_of_strings_eq_same_values() {
        let left = Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
            "alpha".to_string(),
            "beta".to_string(),
        ]));
        let right = Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
            "alpha".to_string(),
            "beta".to_string(),
        ]));

        assert!(array_of_strings_eq(&left, &right));
    }

    #[test]
    fn test_array_of_strings_eq_different_values() {
        let left = Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
            "alpha".to_string(),
            "beta".to_string(),
        ]));
        let right = Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
            "alpha".to_string(),
            "gamma".to_string(),
        ]));

        assert!(!array_of_strings_eq(&left, &right));
    }

    #[test]
    fn test_array_of_strings_eq_string_and_array_equal() {
        let string_val = Some(StringOrArrayOfStrings::String("alpha".to_string()));
        let array_val = Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
            "alpha".to_string(),
        ]));

        assert!(array_of_strings_eq(&string_val, &array_val));
    }

    #[test]
    fn test_tags_eq_vec_set_semantics() {
        // Plain Vec<String> tag form (e.g. CommonAttributes.tags) — set semantics
        // (ordering and multiplicity ignored). Saved queries store tags as Vec<String>.
        let with_dupes = vec!["weekly".to_string(), "weekly".to_string()];
        let dedup = vec!["weekly".to_string()];
        assert!(tags_eq_vec(&with_dupes, &dedup));
        assert!(tags_eq_vec(&dedup, &with_dupes));
        assert!(tags_eq_vec(&[], &[]));

        // Order-insensitive
        let abc = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let cab = vec!["c".to_string(), "a".to_string(), "b".to_string()];
        assert!(tags_eq_vec(&abc, &cab));

        // Real differences still flagged
        let with_extra = vec!["weekly".to_string(), "critical".to_string()];
        assert!(!tags_eq_vec(&with_extra, &dedup));
    }

    #[test]
    fn test_tags_default_to_parent_first_order() {
        // Nested dbt_project.yml +tags: parent folder INTERMEDIATE, child DAILY
        // must resolve to [INTERMEDIATE, DAILY] like dbt-core (issue #15590).
        use crate::schemas::project::configs::config_merge::{DefaultTo, Tags};

        let mut child_tags = Tags(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
            "DAILY".to_string(),
        ])));
        let parent_tags = Tags(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
            "INTERMEDIATE".to_string(),
        ])));
        child_tags.inherit_from(&parent_tags);
        assert_eq!(
            child_tags.into_inner(),
            Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "INTERMEDIATE".to_string(),
                "DAILY".to_string(),
            ]))
        );
    }
}
