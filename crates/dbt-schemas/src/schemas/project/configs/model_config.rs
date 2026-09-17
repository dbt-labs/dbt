use crate::schemas::common::ClusterConfig;
use crate::schemas::serde::AdapterTypeOrArray;
use crate::schemas::serde::OmissibleGrantConfig;
use crate::schemas::serde::PartitionsConfig;
use crate::schemas::serde::QueryTag;
use dbt_adapter_core::AdapterType;
use dbt_common::io_args::ComputeArg;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::serde_utils::Omissible;
use dbt_yaml::DbtSchema;
use dbt_yaml::Spanned;
use dbt_yaml::Verbatim;
use serde::{Deserialize, Serialize};
// Type aliases for clarity
type YmlValue = dbt_yaml::Value;
use indexmap::IndexMap;
use std::collections::btree_map::Iter;
use std::collections::{BTreeMap, HashSet};

use super::config_keys::ConfigKeys;
use crate::schemas::common::DbtBatchSize;
use crate::schemas::common::DbtContract;
use crate::schemas::common::DbtIncrementalStrategy;
use crate::schemas::common::DbtMaterialization;
use crate::schemas::common::DbtUniqueKey;
use crate::schemas::common::PartitionConfig;
use crate::schemas::common::PersistDocsConfig;
use crate::schemas::common::RowFilterConfig;
use crate::schemas::common::SyncConfig;
use crate::schemas::common::{Access, DbtQuoting, Schedule};
use crate::schemas::common::{DocsConfig, OnConfigurationChange, OnError};
use crate::schemas::common::{Hooks, OnSchemaChange, hooks_equal};
use crate::schemas::manifest::GrantAccessToTarget;
use crate::schemas::project::configs::common::log_state_mod_diff;
use crate::schemas::project::configs::common::{
    WarehouseSpecificNodeConfig, access_eq, docs_eq, grants_equal, meta_eq, omissible_option_eq,
    same_warehouse_config, take_databricks_catalog_alias,
};
use crate::schemas::project::configs::config_merge::{Classifiers, Packages, Tags};
use crate::schemas::project::dbt_project::ResolvableConfig;
use crate::schemas::project::dbt_project::TypedRecursiveConfig;
use crate::schemas::properties::model_properties::ModelConstraint;
use crate::schemas::properties::{ModelFreshness, ModelState};
use crate::schemas::serde::StringOrArrayOfStrings;
use crate::schemas::serde::{
    IndexesConfig, PrimaryKeyConfig, RefreshableConfig, StringOrInteger, bool_or_string_bool,
    column_types_map, default_type, event_time_or_map_to_string, f64_or_string_f64,
    hours_to_expiration_or_string_omissible, model_constraints_or_map, string_or_number_to_string,
    u64_or_string_u64,
};
use dbt_proc_macros::{DefaultTo, Resolvable};
use dbt_yaml::ShouldBe;

/// Represents the latest version view configuration for versioned models.
/// Supports shorthand (bool) and full form ({enabled: bool, alias: string}).
#[derive(Deserialize, Serialize, Debug, Default, Clone, PartialEq, DbtSchema)]
pub struct LatestVersionPointer {
    pub enabled: Option<bool>,
    pub alias: Option<String>,
}

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ProjectModelConfig {
    #[serde(rename = "+access")]
    pub access: Option<Access>,
    #[serde(rename = "+adapter_properties")]
    pub adapter_properties: Option<BTreeMap<String, YmlValue>>,
    #[serde(rename = "+alias")]
    pub alias: Option<String>,
    #[serde(
        default,
        rename = "+automatic_clustering",
        deserialize_with = "bool_or_string_bool"
    )]
    pub automatic_clustering: Option<bool>,
    #[serde(
        default,
        rename = "+auto_refresh",
        deserialize_with = "bool_or_string_bool"
    )]
    pub auto_refresh: Option<bool>,
    #[serde(
        default,
        rename = "+auto_liquid_cluster",
        deserialize_with = "bool_or_string_bool"
    )]
    pub auto_liquid_cluster: Option<bool>,
    #[serde(default, rename = "+backup", deserialize_with = "bool_or_string_bool")]
    pub backup: Option<bool>,
    #[serde(rename = "+base_location_root")]
    pub base_location_root: Option<String>,
    #[serde(rename = "+base_location_subpath")]
    pub base_location_subpath: Option<String>,
    #[serde(
        default,
        rename = "+iceberg_version",
        deserialize_with = "u64_or_string_u64"
    )]
    pub iceberg_version: Option<u64>,
    #[serde(
        default,
        rename = "+change_tracking",
        deserialize_with = "bool_or_string_bool"
    )]
    pub change_tracking: Option<bool>,
    #[serde(rename = "+batch_size")]
    pub batch_size: Option<DbtBatchSize>,
    #[serde(rename = "+begin")]
    pub begin: Option<String>,
    #[serde(default, rename = "+bind", deserialize_with = "bool_or_string_bool")]
    pub bind: Option<bool>,
    #[serde(rename = "+buckets")]
    pub buckets: Option<i64>,
    #[serde(rename = "+catalog")]
    pub catalog: Option<String>,
    #[serde(rename = "+catalog_name")]
    pub catalog_name: Option<String>,
    #[serde(rename = "+adapter")]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    #[serde(rename = "+propagate")]
    #[schemars(with = "Option<StringOrArrayOfStrings>")]
    pub propagate: Option<AdapterTypeOrArray>,
    #[serde(rename = "+cluster_by")]
    pub cluster_by: Option<ClusterConfig>,
    #[serde(rename = "+clustered_by")]
    pub clustered_by: Option<StringOrArrayOfStrings>,
    #[serde(
        default,
        rename = "+column_types",
        deserialize_with = "column_types_map"
    )]
    pub column_types: Option<BTreeMap<Spanned<String>, String>>,
    #[serde(rename = "+compute")]
    pub compute: Option<ComputeArg>,
    #[serde(
        default,
        rename = "+concurrent_batches",
        deserialize_with = "bool_or_string_bool"
    )]
    pub concurrent_batches: Option<bool>,
    #[serde(rename = "+contract")]
    pub contract: Option<DbtContract>,
    #[serde(rename = "+compression")]
    pub compression: Option<String>,
    #[serde(
        default,
        rename = "+copy_grants",
        deserialize_with = "bool_or_string_bool"
    )]
    pub copy_grants: Option<bool>,
    #[serde(
        default,
        rename = "+copy_tags",
        deserialize_with = "bool_or_string_bool"
    )]
    pub copy_tags: Option<bool>,
    #[serde(rename = "+database", alias = "+project", alias = "+data_space")]
    pub database: Omissible<Option<String>>,
    #[serde(rename = "+databricks_compute")]
    pub databricks_compute: Option<String>,
    #[serde(rename = "+databricks_tags")]
    pub databricks_tags: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+submission_method")]
    pub submission_method: Option<String>,
    #[serde(rename = "+job_cluster_config")]
    pub job_cluster_config: Option<BTreeMap<String, YmlValue>>,
    #[serde(rename = "+python_job_config")]
    pub python_job_config: Option<BTreeMap<String, YmlValue>>,
    #[serde(rename = "+cluster_id")]
    pub cluster_id: Option<String>,
    #[serde(rename = "+http_path")]
    pub http_path: Option<String>,
    #[serde(
        default,
        rename = "+create_notebook",
        deserialize_with = "bool_or_string_bool"
    )]
    pub create_notebook: Option<bool>,
    #[serde(rename = "+index_url")]
    pub index_url: Option<String>,
    #[serde(rename = "+additional_libs")]
    pub additional_libs: Option<Vec<YmlValue>>,
    #[serde(
        default,
        rename = "+user_folder_for_python",
        deserialize_with = "bool_or_string_bool"
    )]
    pub user_folder_for_python: Option<bool>,
    #[serde(rename = "+environment_key")]
    pub environment_key: Option<String>,
    #[serde(rename = "+environment_dependencies")]
    pub environment_dependencies: Option<Vec<String>>,
    #[serde(
        default,
        rename = "+incremental_apply_config_changes",
        deserialize_with = "bool_or_string_bool"
    )]
    pub incremental_apply_config_changes: Option<bool>,
    #[serde(
        default,
        rename = "+persist_constraints",
        deserialize_with = "bool_or_string_bool"
    )]
    pub persist_constraints: Option<bool>,
    #[serde(
        default,
        rename = "+use_safer_relation_operations",
        deserialize_with = "bool_or_string_bool"
    )]
    pub use_safer_relation_operations: Option<bool>,
    #[serde(
        default,
        rename = "+view_update_via_alter",
        deserialize_with = "bool_or_string_bool"
    )]
    pub view_update_via_alter: Option<bool>,
    #[serde(
        default,
        rename = "+unique_tmp_table_suffix",
        deserialize_with = "bool_or_string_bool"
    )]
    pub unique_tmp_table_suffix: Option<bool>,

    // NOTE: This is only for BigQuery materialized views
    #[serde(rename = "+description")]
    pub description: Option<String>,
    #[serde(rename = "+dist")]
    pub dist: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+docs")]
    pub docs: Option<DocsConfig>,
    #[serde(
        default,
        rename = "+enable_refresh",
        deserialize_with = "bool_or_string_bool"
    )]
    pub enable_refresh: Option<bool>,
    #[serde(default, rename = "+enabled", deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(
        default,
        rename = "+event_time",
        deserialize_with = "event_time_or_map_to_string"
    )]
    pub event_time: Option<String>,
    #[serde(rename = "+external_volume")]
    pub external_volume: Option<String>,
    #[serde(rename = "+file_format")]
    pub file_format: Option<String>,
    #[serde(rename = "+freshness")]
    pub freshness: Option<ModelFreshness>,
    #[serde(rename = "+loaded_at_query")]
    pub loaded_at_query: Verbatim<Option<String>>,
    #[serde(rename = "+loaded_at_field")]
    pub loaded_at_field: Option<String>,
    #[serde(rename = "+state")]
    pub state: Option<ModelState>,
    #[serde(rename = "+latest_version_pointer")]
    pub latest_version_pointer: Option<LatestVersionPointer>,
    #[serde(
        default,
        rename = "+full_refresh",
        deserialize_with = "bool_or_string_bool"
    )]
    pub full_refresh: Option<bool>,
    #[serde(rename = "+grant_access_to")]
    pub grant_access_to: Option<Vec<GrantAccessToTarget>>,
    #[serde(rename = "+grants")]
    pub grants: OmissibleGrantConfig,
    #[serde(rename = "+group")]
    pub group: Option<String>,
    #[serde(
        default,
        rename = "+hours_to_expiration",
        deserialize_with = "hours_to_expiration_or_string_omissible"
    )]
    pub hours_to_expiration: Omissible<Option<StringOrInteger>>,
    #[serde(
        default,
        rename = "+job_execution_timeout_seconds",
        deserialize_with = "u64_or_string_u64"
    )]
    pub job_execution_timeout_seconds: Option<u64>,
    #[serde(rename = "+reservation")]
    pub reservation: Option<String>,
    #[serde(
        default,
        rename = "+include_full_name_in_path",
        deserialize_with = "bool_or_string_bool"
    )]
    pub include_full_name_in_path: Option<bool>,
    #[serde(rename = "+incremental_predicates")]
    pub incremental_predicates: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+incremental_strategy")]
    pub incremental_strategy: Option<DbtIncrementalStrategy>,
    #[serde(
        default,
        rename = "+constraints",
        deserialize_with = "model_constraints_or_map"
    )]
    pub constraints: Option<Vec<ModelConstraint>>,
    #[serde(rename = "+initialize")]
    pub initialize: Option<String>,
    #[serde(rename = "+scheduler")]
    pub scheduler: Option<String>,
    #[serde(
        default,
        rename = "+data_retention_time_in_days",
        deserialize_with = "u64_or_string_u64"
    )]
    pub data_retention_time_in_days: Option<u64>,
    #[serde(rename = "+kms_key_name")]
    pub kms_key_name: Option<String>,
    #[serde(rename = "+labels")]
    pub labels: Option<IndexMap<String, String>>,
    #[serde(
        default,
        rename = "+labels_from_meta",
        deserialize_with = "bool_or_string_bool"
    )]
    pub labels_from_meta: Option<bool>,
    #[serde(rename = "+liquid_clustered_by")]
    pub liquid_clustered_by: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+zorder")]
    pub zorder: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+location")]
    pub location: Option<String>,
    #[serde(rename = "+location_root")]
    pub location_root: Option<String>,
    #[serde(
        default,
        rename = "+use_uniform",
        deserialize_with = "bool_or_string_bool"
    )]
    pub use_uniform: Option<bool>,
    #[serde(rename = "+lookback")]
    pub lookback: Option<i32>,
    #[serde(rename = "+matched_condition")]
    pub matched_condition: Option<String>,
    #[serde(rename = "+materialized")]
    pub materialized: Option<DbtMaterialization>,
    #[serde(rename = "+max_staleness")]
    pub max_staleness: Option<String>,
    #[serde(
        default,
        rename = "+max_data_extension_time_in_days",
        deserialize_with = "u64_or_string_u64"
    )]
    pub max_data_extension_time_in_days: Option<u64>,
    #[serde(rename = "+jar_file_uri")]
    pub jar_file_uri: Option<String>,
    #[serde(rename = "+timeout")]
    pub timeout: Option<u64>,
    #[serde(rename = "+batch_id")]
    pub batch_id: Option<String>,
    #[serde(rename = "+dataproc_cluster_name")]
    pub dataproc_cluster_name: Option<String>,
    #[serde(
        default,
        rename = "+notebook_template_id",
        deserialize_with = "u64_or_string_u64"
    )]
    pub notebook_template_id: Option<u64>,
    #[serde(
        default,
        rename = "+enable_list_inference",
        deserialize_with = "bool_or_string_bool"
    )]
    pub enable_list_inference: Option<bool>,
    #[serde(rename = "+intermediate_format")]
    pub intermediate_format: Option<String>,
    #[serde(rename = "+storage_uri")]
    pub storage_uri: Option<String>,
    #[serde(rename = "+merge_exclude_columns")]
    pub merge_exclude_columns: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+merge_update_columns")]
    pub merge_update_columns: Option<StringOrArrayOfStrings>,
    #[serde(
        default,
        rename = "+merge_with_schema_evolution",
        deserialize_with = "bool_or_string_bool"
    )]
    pub merge_with_schema_evolution: Option<bool>,
    // Verbatim: skips the whole-document `into_typed` render pass for this field. A render
    // failure nested inside a `meta` value (e.g. an unknown macro call under a misplaced
    // `+meta` block) would otherwise fail the entire enclosing directory-path node -- not just
    // that key -- silently dropping unrelated sibling directories' config too (fs#14217).
    // `dbt_project_yml_loader::render_meta_tolerantly` re-renders this field on its own,
    // afterward, falling back to the unrendered value on failure instead of propagating.
    #[serde(rename = "+meta")]
    pub meta: Verbatim<Option<IndexMap<String, YmlValue>>>,
    #[serde(rename = "+not_matched_by_source_action")]
    pub not_matched_by_source_action: Option<String>,
    #[serde(rename = "+not_matched_by_source_condition")]
    pub not_matched_by_source_condition: Option<String>,
    #[serde(rename = "+not_matched_condition")]
    pub not_matched_condition: Option<String>,
    #[serde(rename = "+source_alias")]
    pub source_alias: Option<String>,
    #[serde(rename = "+target_alias")]
    pub target_alias: Option<String>,
    #[serde(rename = "+on_configuration_change")]
    pub on_configuration_change: Option<OnConfigurationChange>,
    #[serde(rename = "+on_error")]
    pub on_error: Option<OnError>,
    #[serde(rename = "+on_schema_change")]
    pub on_schema_change: Option<OnSchemaChange>,
    #[serde(rename = "+packages")]
    pub packages: Option<StringOrArrayOfStrings>,
    #[serde(
        rename = "+python_version",
        default,
        deserialize_with = "string_or_number_to_string"
    )]
    #[schemars(with = "Option<String>", skip_serializing_if = "Option::is_none")]
    pub python_version: Option<String>,
    #[serde(rename = "+imports")]
    pub imports: Option<StringOrArrayOfStrings>,
    /// Snowflake Python model config: secrets to pass to stored procedure
    #[serde(rename = "+secrets")]
    pub secrets: Option<BTreeMap<String, YmlValue>>,
    /// Snowflake Python model config: external access integrations for network access
    #[serde(rename = "+external_access_integrations")]
    pub external_access_integrations: Option<StringOrArrayOfStrings>,
    /// Snowflake Python model config: use anonymous stored procedure (default: true)
    #[serde(
        default,
        rename = "+use_anonymous_sproc",
        deserialize_with = "bool_or_string_bool"
    )]
    pub use_anonymous_sproc: Option<bool>,
    #[serde(rename = "+partition_by")]
    pub partition_by: Option<PartitionConfig>,
    /// Exasol: PARTITION BY column list (name matches the Python dbt-exasol adapter)
    #[serde(rename = "+partition_by_config")]
    pub partition_by_config: Option<StringOrArrayOfStrings>,
    /// Exasol: DISTRIBUTE BY column list (name matches the Python dbt-exasol adapter)
    #[serde(rename = "+distribute_by_config")]
    pub distribute_by_config: Option<StringOrArrayOfStrings>,
    /// Exasol: advisory PRIMARY KEY column list (name matches the Python dbt-exasol adapter)
    #[serde(rename = "+primary_key_config")]
    pub primary_key_config: Option<StringOrArrayOfStrings>,
    #[serde(
        default,
        rename = "+partition_expiration_days",
        deserialize_with = "u64_or_string_u64"
    )]
    pub partition_expiration_days: Option<u64>,
    #[serde(rename = "+partitions")]
    pub partitions: Option<PartitionsConfig>,
    #[serde(rename = "+persist_docs")]
    pub persist_docs: Option<PersistDocsConfig>,
    #[serde(rename = "+post-hook", alias = "+post_hook")]
    pub post_hook: Verbatim<Option<Hooks>>,
    #[serde(rename = "+pre-hook", alias = "+pre_hook")]
    pub pre_hook: Verbatim<Option<Hooks>>,
    #[serde(rename = "+predicates")]
    pub predicates: Option<Vec<String>>,
    #[serde(rename = "+query_tag")]
    pub query_tag: Option<QueryTag>,
    #[serde(rename = "+query_tags")]
    pub query_tags: Option<String>,
    #[serde(rename = "+table_tag")]
    pub table_tag: Option<String>,
    #[serde(rename = "+row_access_policy")]
    pub row_access_policy: Option<String>,
    #[serde(rename = "+storage_serialization_policy")]
    pub storage_serialization_policy: Option<String>,
    #[serde(rename = "+quoting")]
    pub quoting: Option<DbtQuoting>,
    #[serde(rename = "+refresh_mode")]
    pub refresh_mode: Option<String>,
    #[serde(
        default,
        rename = "+refresh_interval_minutes",
        deserialize_with = "f64_or_string_f64"
    )]
    pub refresh_interval_minutes: Option<f64>,
    #[serde(rename = "+resource_tags")]
    pub resource_tags: Option<IndexMap<String, String>>,
    #[serde(
        default,
        rename = "+require_partition_filter",
        deserialize_with = "bool_or_string_bool"
    )]
    pub require_partition_filter: Option<bool>,
    #[serde(rename = "+schema", alias = "+dataset")]
    pub schema: Omissible<Option<String>>,
    #[serde(
        default,
        rename = "+skip_matched_step",
        deserialize_with = "bool_or_string_bool"
    )]
    pub skip_matched_step: Option<bool>,
    #[serde(
        default,
        rename = "+skip_not_matched_step",
        deserialize_with = "bool_or_string_bool"
    )]
    pub skip_not_matched_step: Option<bool>,
    #[serde(
        default,
        rename = "+skip_optimize",
        deserialize_with = "bool_or_string_bool"
    )]
    pub skip_optimize: Option<bool>,
    #[serde(default, rename = "+secure", deserialize_with = "bool_or_string_bool")]
    pub secure: Option<bool>,
    #[serde(rename = "+sort")]
    pub sort: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+sort_type")]
    pub sort_type: Option<String>,
    #[serde(rename = "+snowflake_initialization_warehouse")]
    pub snowflake_initialization_warehouse: Option<String>,
    #[serde(rename = "+snowflake_warehouse")]
    pub snowflake_warehouse: Option<String>,
    #[serde(rename = "+refresh_warehouse")]
    pub refresh_warehouse: Option<String>,
    #[serde(rename = "+immutable_where")]
    pub immutable_where: Option<String>,
    #[serde(rename = "+sql_header")]
    pub sql_header: Option<String>,
    #[serde(rename = "+static_analysis")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    #[serde(rename = "+table_format")]
    pub table_format: Option<String>,
    #[serde(rename = "+tags")]
    pub tags: Omissible<StringOrArrayOfStrings>,
    #[serde(rename = "+classifiers")]
    pub classifiers: Omissible<StringOrArrayOfStrings>,
    #[serde(rename = "+target_lag")]
    pub target_lag: Option<String>,
    #[serde(rename = "+target_file_size")]
    pub target_file_size: Option<String>,
    #[serde(rename = "+tblproperties")]
    pub tblproperties: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+tmp_relation_type")]
    pub tmp_relation_type: Option<String>,
    #[serde(
        default,
        rename = "+transient",
        deserialize_with = "bool_or_string_bool"
    )]
    pub transient: Option<bool>,
    #[serde(rename = "+unique_key")]
    pub unique_key: Option<DbtUniqueKey>,
    #[serde(
        default,
        rename = "+as_columnstore",
        deserialize_with = "bool_or_string_bool"
    )]
    pub as_columnstore: Option<bool>,

    #[serde(default, rename = "+table_type")]
    pub table_type: Option<String>,

    #[serde(default, rename = "+indexes")]
    pub indexes: IndexesConfig,

    // Postgres unlogged table
    #[serde(
        default,
        rename = "+unlogged",
        deserialize_with = "bool_or_string_bool"
    )]
    pub unlogged: Option<bool>,

    // Schedule (Databricks streaming tables)
    #[serde(rename = "+schedule")]
    pub schedule: Option<Schedule>,

    // Row filter (Databricks)
    #[serde(rename = "+row_filter")]
    pub row_filter: Option<RowFilterConfig>,

    // Primary Key (Salesforce)
    #[serde(default, rename = "+primary_key")]
    pub primary_key: PrimaryKeyConfig,
    #[serde(rename = "+category")]
    pub category: Option<DataLakeObjectCategory>,

    /// Schema synchronization configuration
    #[serde(rename = "+sync")]
    pub sync: Option<SyncConfig>,

    // ClickHouse
    // table materialization
    #[serde(rename = "+engine")]
    pub engine: Option<String>,
    #[serde(rename = "+order_by")]
    pub order_by: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+ttl")]
    pub ttl: Option<String>,
    #[serde(rename = "+settings")]
    pub settings: Option<BTreeMap<String, YmlValue>>,
    #[serde(rename = "+query_settings")]
    pub query_settings: Option<BTreeMap<String, YmlValue>>,
    // list of `{name, query}` maps rendered as `ADD PROJECTION` by table.sql
    #[serde(rename = "+projections")]
    pub projections: Option<Vec<YmlValue>>,
    // incremental materialization
    #[serde(
        default,
        rename = "+inserts_only",
        deserialize_with = "bool_or_string_bool"
    )]
    pub inserts_only: Option<bool>,
    // dictionary materialization
    #[serde(rename = "+connection_overrides")]
    pub connection_overrides: Option<BTreeMap<String, YmlValue>>,
    #[serde(rename = "+fields")]
    pub fields: Option<Vec<YmlValue>>,
    #[serde(rename = "+source_type")]
    pub source_type: Option<String>,
    #[serde(rename = "+url")]
    pub url: Option<String>,
    #[serde(rename = "+format")]
    pub format: Option<String>,
    #[serde(rename = "+layout")]
    pub layout: Option<String>,
    #[serde(rename = "+lifetime")]
    pub lifetime: Option<YmlValue>,
    #[serde(rename = "+range")]
    pub range: Option<YmlValue>,
    #[serde(rename = "+table")]
    pub table: Option<String>,
    #[serde(rename = "+update_field")]
    pub update_field: Option<String>,
    #[serde(rename = "+update_lag")]
    pub update_lag: Option<YmlValue>,
    #[serde(rename = "+definer")]
    pub definer: Option<String>,
    #[serde(rename = "+sql_security")]
    pub sql_security: Option<String>,
    // materialized-view materialization
    #[serde(rename = "+refreshable")]
    pub refreshable: Option<RefreshableConfig>,
    #[serde(default, rename = "+catchup", deserialize_with = "bool_or_string_bool")]
    pub catchup: Option<bool>,
    #[serde(rename = "+mv_on_schema_change")]
    pub mv_on_schema_change: Option<String>,
    #[serde(
        default,
        rename = "+repopulate_from_mvs_on_full_refresh",
        deserialize_with = "bool_or_string_bool"
    )]
    pub repopulate_from_mvs_on_full_refresh: Option<bool>,

    // Flattened field:
    pub __additional_properties__: BTreeMap<String, ShouldBe<ProjectModelConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, DbtSchema)]
#[serde(rename_all = "PascalCase")]
/// See `category` from https://developer.salesforce.com/docs/data/connectapi/references/spec?meta=postDataLakeObject
pub enum DataLakeObjectCategory {
    Profile,
    Engagement,
    #[serde(rename = "Directory_Table")]
    DirectoryTable,
    Insights,
    Other,
}

impl TypedRecursiveConfig for ProjectModelConfig {
    fn type_name() -> &'static str {
        "model"
    }

    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>> {
        self.__additional_properties__.iter()
    }

    fn has_set_fields(&self) -> bool {
        self.access.is_some()
            || self.adapter_properties.is_some()
            || self.alias.is_some()
            || self.automatic_clustering.is_some()
            || self.auto_refresh.is_some()
            || self.auto_liquid_cluster.is_some()
            || self.backup.is_some()
            || self.base_location_root.is_some()
            || self.base_location_subpath.is_some()
            || self.iceberg_version.is_some()
            || self.change_tracking.is_some()
            || self.batch_size.is_some()
            || self.begin.is_some()
            || self.bind.is_some()
            || self.buckets.is_some()
            || self.catalog.is_some()
            || self.catalog_name.is_some()
            || self.adapter.is_some()
            || self.propagate.is_some()
            || self.cluster_by.is_some()
            || self.clustered_by.is_some()
            || self.column_types.is_some()
            || self.compute.is_some()
            || self.concurrent_batches.is_some()
            || self.contract.is_some()
            || self.compression.is_some()
            || self.copy_grants.is_some()
            || self.copy_tags.is_some()
            || self.database.is_present()
            || self.databricks_compute.is_some()
            || self.databricks_tags.is_some()
            || self.submission_method.is_some()
            || self.job_cluster_config.is_some()
            || self.python_job_config.is_some()
            || self.cluster_id.is_some()
            || self.http_path.is_some()
            || self.create_notebook.is_some()
            || self.index_url.is_some()
            || self.additional_libs.is_some()
            || self.user_folder_for_python.is_some()
            || self.environment_key.is_some()
            || self.environment_dependencies.is_some()
            || self.incremental_apply_config_changes.is_some()
            || self.persist_constraints.is_some()
            || self.use_safer_relation_operations.is_some()
            || self.view_update_via_alter.is_some()
            || self.description.is_some()
            || self.dist.is_some()
            || self.docs.is_some()
            || self.enable_refresh.is_some()
            || self.enabled.is_some()
            || self.event_time.is_some()
            || self.external_volume.is_some()
            || self.file_format.is_some()
            || self.freshness.is_some()
            || self.state.is_some()
            || self.latest_version_pointer.is_some()
            || self.full_refresh.is_some()
            || self.grant_access_to.is_some()
            || self.grants.0.is_present()
            || self.group.is_some()
            || self.hours_to_expiration.is_present()
            || self.job_execution_timeout_seconds.is_some()
            || self.reservation.is_some()
            || self.include_full_name_in_path.is_some()
            || self.incremental_predicates.is_some()
            || self.incremental_strategy.is_some()
            || self.initialize.is_some()
            || self.scheduler.is_some()
            || self.data_retention_time_in_days.is_some()
            || self.kms_key_name.is_some()
            || self.labels.is_some()
            || self.labels_from_meta.is_some()
            || self.liquid_clustered_by.is_some()
            || self.location.is_some()
            || self.location_root.is_some()
            || self.use_uniform.is_some()
            || self.lookback.is_some()
            || self.matched_condition.is_some()
            || self.materialized.is_some()
            || self.max_staleness.is_some()
            || self.max_data_extension_time_in_days.is_some()
            || self.jar_file_uri.is_some()
            || self.timeout.is_some()
            || self.batch_id.is_some()
            || self.dataproc_cluster_name.is_some()
            || self.notebook_template_id.is_some()
            || self.enable_list_inference.is_some()
            || self.intermediate_format.is_some()
            || self.storage_uri.is_some()
            || self.merge_exclude_columns.is_some()
            || self.merge_update_columns.is_some()
            || self.merge_with_schema_evolution.is_some()
            || self.meta.is_some()
            || self.not_matched_by_source_action.is_some()
            || self.not_matched_by_source_condition.is_some()
            || self.not_matched_condition.is_some()
            || self.source_alias.is_some()
            || self.target_alias.is_some()
            || self.on_configuration_change.is_some()
            || self.on_error.is_some()
            || self.on_schema_change.is_some()
            || self.packages.is_some()
            || self.python_version.is_some()
            || self.imports.is_some()
            || self.secrets.is_some()
            || self.external_access_integrations.is_some()
            || self.use_anonymous_sproc.is_some()
            || self.partition_by.is_some()
            || self.partition_expiration_days.is_some()
            || self.partitions.is_some()
            || self.persist_docs.is_some()
            || self.post_hook.is_some()
            || self.pre_hook.is_some()
            || self.predicates.is_some()
            || self.query_tag.is_some()
            || self.table_tag.is_some()
            || self.row_access_policy.is_some()
            || self.storage_serialization_policy.is_some()
            || self.quoting.is_some()
            || self.refresh_mode.is_some()
            || self.refresh_interval_minutes.is_some()
            || self.resource_tags.is_some()
            || self.require_partition_filter.is_some()
            || self.schema.is_present()
            || self.skip_matched_step.is_some()
            || self.skip_not_matched_step.is_some()
            || self.skip_optimize.is_some()
            || self.secure.is_some()
            || self.sort.is_some()
            || self.sort_type.is_some()
            || self.snowflake_initialization_warehouse.is_some()
            || self.snowflake_warehouse.is_some()
            || self.refresh_warehouse.is_some()
            || self.immutable_where.is_some()
            || self.sql_header.is_some()
            || self.static_analysis.is_some()
            || self.table_format.is_some()
            || self.tags.is_present()
            || self.classifiers.is_present()
            || self.target_lag.is_some()
            || self.target_file_size.is_some()
            || self.tblproperties.is_some()
            || self.tmp_relation_type.is_some()
            || self.transient.is_some()
            || self.unique_key.is_some()
            || self.as_columnstore.is_some()
            || self.table_type.is_some()
            || self.indexes.is_some()
            || self.unlogged.is_some()
            || self.schedule.is_some()
            || self.row_filter.is_some()
            || self.primary_key.is_some()
            || self.category.is_some()
            || self.sync.is_some()
    }
}

#[derive(
    Resolvable, DefaultTo, Deserialize, Serialize, Debug, Default, Clone, PartialEq, DbtSchema,
)]
pub struct ModelConfig {
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    pub alias: Option<String>,
    // These aliases (like the `+`-prefixed variants above) are ungated: accepted on every
    // adapter, unlike dbt-core's per-adapter `_ALIASES` map. Deliberate, see fs#13424.
    #[serde(alias = "project", alias = "data_space")]
    pub database: Omissible<Option<String>>,
    #[serde(alias = "dataset")]
    pub schema: Omissible<Option<String>>,
    #[serde(default)]
    pub tags: Tags,
    #[serde(default)]
    pub classifiers: Classifiers,
    pub catalog_name: Option<String>,
    // Internal placement hint; kept out of serialized config/telemetry output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    // Internal placement hint; kept out of serialized config/telemetry output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<StringOrArrayOfStrings>")]
    pub propagate: Option<AdapterTypeOrArray>,
    // need default to ensure None if field is not set
    // serialize_with ensures meta is always present (as {} when None) for Jinja macros
    // that access node.config.meta.get(...)
    #[serde(
        default,
        deserialize_with = "default_type",
        serialize_with = "crate::schemas::serde::serialize_none_as_empty_map"
    )]
    pub meta: Option<IndexMap<String, YmlValue>>,
    pub group: Option<String>,
    #[resolved(promote, default = DbtMaterialization::View)]
    pub materialized: Option<DbtMaterialization>,
    pub incremental_strategy: Option<DbtIncrementalStrategy>,
    pub incremental_predicates: Option<StringOrArrayOfStrings>,
    // Hashed into the run-cache key only; the functional source of a model's constraints is the
    // schema.yml `constraints:` property (`resolve_models.rs:1163`). Matches dbt-core.
    #[serde(default, deserialize_with = "model_constraints_or_map")]
    pub constraints: Option<Vec<ModelConstraint>>,
    pub batch_size: Option<DbtBatchSize>,
    #[resolved(promote, default = 1)]
    pub lookback: Option<i32>,
    pub begin: Option<String>,
    pub persist_docs: Option<PersistDocsConfig>,
    #[serde(alias = "post-hook")]
    pub post_hook: Verbatim<Option<Hooks>>,
    #[serde(alias = "pre-hook")]
    pub pre_hook: Verbatim<Option<Hooks>>,
    #[resolved(promote, expect = "apply_package_defaults guarantees quoting is set")]
    pub quoting: Option<DbtQuoting>,
    #[serde(default, deserialize_with = "column_types_map")]
    pub column_types: Option<BTreeMap<Spanned<String>, String>>,
    pub compute: Option<ComputeArg>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub full_refresh: Option<bool>,
    pub unique_key: Option<DbtUniqueKey>,
    pub on_schema_change: Option<OnSchemaChange>,
    pub on_configuration_change: Option<OnConfigurationChange>,
    pub on_error: Option<OnError>,
    pub grants: OmissibleGrantConfig,
    pub packages: Packages,
    #[serde(default, deserialize_with = "string_or_number_to_string")]
    #[schemars(with = "Option<String>", skip_serializing_if = "Option::is_none")]
    pub python_version: Option<String>,
    pub docs: Option<DocsConfig>,
    pub imports: Option<StringOrArrayOfStrings>,
    pub secrets: Option<BTreeMap<String, YmlValue>>,
    pub external_access_integrations: Option<StringOrArrayOfStrings>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub use_anonymous_sproc: Option<bool>,
    pub contract: Option<DbtContract>,
    #[serde(default, deserialize_with = "event_time_or_map_to_string")]
    pub event_time: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub concurrent_batches: Option<bool>,
    pub merge_update_columns: Option<StringOrArrayOfStrings>,
    pub merge_exclude_columns: Option<StringOrArrayOfStrings>,
    pub access: Option<Access>,
    pub table_format: Option<String>,
    #[resolved(promote, expect = "static_analysis set by apply_resolve_defaults")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    pub freshness: Option<ModelFreshness>,
    pub loaded_at_field: Option<String>,
    pub loaded_at_query: Verbatim<Option<String>>,
    pub state: Option<ModelState>,
    #[resolved(promote)]
    pub latest_version_pointer: Option<LatestVersionPointer>,
    pub sql_header: Option<String>,
    pub location: Option<String>,
    pub predicates: Option<Vec<String>>,
    /// Schema synchronization configuration
    pub sync: Option<SyncConfig>,
    // Adapter specific configs
    pub __warehouse_specific_config__: WarehouseSpecificNodeConfig,
    pub submission_method: Option<String>,
    pub job_cluster_config: Option<BTreeMap<String, YmlValue>>,
    pub python_job_config: Option<BTreeMap<String, YmlValue>>,
    pub cluster_id: Option<String>,
    pub http_path: Option<String>,
    pub create_notebook: Option<bool>,
    pub index_url: Option<String>,
    pub additional_libs: Option<Vec<YmlValue>>,
    pub user_folder_for_python: Option<bool>,
    pub environment_key: Option<String>,
    pub environment_dependencies: Option<Vec<String>>,
    /// Config keys accessed via dbt.config.get() in Python models
    /// Used to populate config_dict at runtime
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub config_keys_used: Option<Vec<String>>,
    /// Default values for config keys in the same order as config_keys_used
    /// Stored as minijinja Values which render as Python literals
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub config_keys_defaults: Option<Vec<minijinja::value::Value>>,
    /// Meta keys accessed via dbt.config.meta_get() in Python models
    /// Used to populate config_dict at runtime
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta_keys_used: Option<Vec<String>>,
    /// Default values for meta keys in the same order as meta_keys_used
    /// Stored as minijinja Values which render as Python literals
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta_keys_defaults: Option<Vec<minijinja::value::Value>>,
}

impl From<ProjectModelConfig> for ModelConfig {
    fn from(config: ProjectModelConfig) -> Self {
        Self {
            access: config.access,
            alias: config.alias,
            batch_size: config.batch_size,
            begin: config.begin,
            submission_method: config.submission_method.clone(),
            job_cluster_config: config.job_cluster_config.clone(),
            python_job_config: config.python_job_config.clone(),
            cluster_id: config.cluster_id.clone(),
            http_path: config.http_path.clone(),
            create_notebook: config.create_notebook,
            index_url: config.index_url.clone(),
            additional_libs: config.additional_libs.clone(),
            user_folder_for_python: config.user_folder_for_python,
            environment_key: config.environment_key.clone(),
            environment_dependencies: config.environment_dependencies.clone(),
            catalog_name: config.catalog_name.clone(),
            adapter: config.adapter,
            propagate: config.propagate,
            column_types: config.column_types,
            compute: config.compute,
            concurrent_batches: config.concurrent_batches,
            contract: config.contract,
            database: config.database,
            docs: config.docs,
            enabled: config.enabled,
            event_time: config.event_time,
            freshness: config.freshness,
            loaded_at_field: config.loaded_at_field,
            loaded_at_query: config.loaded_at_query,
            state: config.state,
            latest_version_pointer: config.latest_version_pointer,
            full_refresh: config.full_refresh,
            grants: config.grants,
            group: config.group,
            incremental_predicates: config.incremental_predicates,
            constraints: config.constraints,
            incremental_strategy: config.incremental_strategy,
            location: config.location,
            lookback: config.lookback,
            materialized: config.materialized,
            merge_exclude_columns: config.merge_exclude_columns,
            merge_update_columns: config.merge_update_columns,
            meta: config.meta.0,
            on_configuration_change: config.on_configuration_change,
            on_error: config.on_error,
            on_schema_change: config.on_schema_change,
            packages: Packages(config.packages),
            python_version: config.python_version,
            imports: config.imports,
            secrets: config.secrets,
            external_access_integrations: config.external_access_integrations,
            use_anonymous_sproc: config.use_anonymous_sproc,
            persist_docs: config.persist_docs,
            post_hook: config.post_hook,
            pre_hook: config.pre_hook,
            predicates: config.predicates,
            quoting: config.quoting,
            schema: config.schema,
            sql_header: config.sql_header,
            static_analysis: config.static_analysis,
            sync: config.sync,
            table_format: config.table_format,
            tags: Tags(config.tags.into_inner()),
            classifiers: Classifiers(config.classifiers.into_inner()),
            unique_key: config.unique_key,
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                description: config.description,
                adapter_properties: config.adapter_properties,
                external_volume: config.external_volume,
                base_location_root: config.base_location_root,
                base_location_subpath: config.base_location_subpath,
                change_tracking: config.change_tracking,
                data_retention_time_in_days: config.data_retention_time_in_days,
                max_data_extension_time_in_days: config.max_data_extension_time_in_days,
                storage_serialization_policy: config.storage_serialization_policy,
                target_file_size: config.target_file_size,
                target_lag: config.target_lag,
                snowflake_initialization_warehouse: config.snowflake_initialization_warehouse,
                snowflake_warehouse: config.snowflake_warehouse,
                refresh_warehouse: config.refresh_warehouse,
                immutable_where: config.immutable_where,
                refresh_mode: config.refresh_mode,
                initialize: config.initialize,
                scheduler: config.scheduler,
                tmp_relation_type: config.tmp_relation_type,
                query_tag: config.query_tag,
                query_tags: config.query_tags,
                table_tag: config.table_tag,
                row_access_policy: config.row_access_policy,
                automatic_clustering: config.automatic_clustering,
                copy_grants: config.copy_grants,
                copy_tags: config.copy_tags,
                secure: config.secure,
                transient: config.transient,
                iceberg_version: config.iceberg_version,

                partition_by: config.partition_by,
                partition_by_config: config.partition_by_config,
                distribute_by_config: config.distribute_by_config,
                primary_key_config: config.primary_key_config,
                cluster_by: config.cluster_by,
                hours_to_expiration: config.hours_to_expiration,
                job_execution_timeout_seconds: config.job_execution_timeout_seconds,
                reservation: config.reservation,
                labels: config.labels,
                labels_from_meta: config.labels_from_meta,
                kms_key_name: config.kms_key_name,
                require_partition_filter: config.require_partition_filter,
                partition_expiration_days: config.partition_expiration_days,
                grant_access_to: config.grant_access_to,
                partitions: config.partitions,
                enable_refresh: config.enable_refresh,
                resource_tags: config.resource_tags,
                refresh_interval_minutes: config.refresh_interval_minutes,
                max_staleness: config.max_staleness,
                jar_file_uri: config.jar_file_uri,
                timeout: config.timeout,
                batch_id: config.batch_id,
                dataproc_cluster_name: config.dataproc_cluster_name,
                notebook_template_id: config.notebook_template_id,
                enable_list_inference: config.enable_list_inference,
                intermediate_format: config.intermediate_format,
                storage_uri: config.storage_uri,
                incremental_apply_config_changes: config.incremental_apply_config_changes,
                persist_constraints: config.persist_constraints,
                use_safer_relation_operations: config.use_safer_relation_operations,
                view_update_via_alter: config.view_update_via_alter,

                file_format: config.file_format,
                catalog_name: config.catalog_name,
                location_root: config.location_root,
                use_uniform: config.use_uniform,
                tblproperties: config.tblproperties,
                include_full_name_in_path: config.include_full_name_in_path,
                liquid_clustered_by: config.liquid_clustered_by,
                auto_liquid_cluster: config.auto_liquid_cluster,
                zorder: config.zorder,
                clustered_by: config.clustered_by,
                buckets: config.buckets,
                catalog: config.catalog,
                databricks_tags: config.databricks_tags,
                compression: config.compression,
                databricks_compute: config.databricks_compute,
                target_alias: config.target_alias,
                source_alias: config.source_alias,
                matched_condition: config.matched_condition,
                not_matched_condition: config.not_matched_condition,
                not_matched_by_source_condition: config.not_matched_by_source_condition,
                not_matched_by_source_action: config.not_matched_by_source_action,
                merge_with_schema_evolution: config.merge_with_schema_evolution,
                skip_matched_step: config.skip_matched_step,
                skip_not_matched_step: config.skip_not_matched_step,
                skip_optimize: config.skip_optimize,
                unique_tmp_table_suffix: config.unique_tmp_table_suffix,
                schedule: config.schedule,
                row_filter: config.row_filter,

                auto_refresh: config.auto_refresh,
                backup: config.backup,
                bind: config.bind,
                dist: config.dist,
                sort: config.sort,
                sort_type: config.sort_type,

                as_columnstore: config.as_columnstore,

                table_type: config.table_type,
                indexes: config.indexes,
                unlogged: config.unlogged,

                primary_key: config.primary_key,
                category: config.category,

                engine: config.engine,
                order_by: config.order_by,
                ttl: config.ttl,
                settings: config.settings,
                query_settings: config.query_settings,
                projections: config.projections,
                inserts_only: config.inserts_only,
                connection_overrides: config.connection_overrides,
                fields: config.fields,
                source_type: config.source_type,
                url: config.url,
                format: config.format,
                layout: config.layout,
                lifetime: config.lifetime,
                range: config.range,
                table: config.table,
                update_field: config.update_field,
                update_lag: config.update_lag,
                definer: config.definer,
                sql_security: config.sql_security,
                refreshable: config.refreshable,
                catchup: config.catchup,
                mv_on_schema_change: config.mv_on_schema_change,
                repopulate_from_mvs_on_full_refresh: config.repopulate_from_mvs_on_full_refresh,
            },
            // Python-specific fields - initialized to None here, set during Python AST analysis
            config_keys_used: None,
            config_keys_defaults: None,
            meta_keys_used: None,
            meta_keys_defaults: None,
        }
    }
}

impl From<ModelConfig> for ProjectModelConfig {
    fn from(config: ModelConfig) -> Self {
        Self {
            access: config.access,
            alias: config.alias,
            auto_refresh: config.__warehouse_specific_config__.auto_refresh,
            backup: config.__warehouse_specific_config__.backup,
            batch_size: config.batch_size,
            begin: config.begin,
            bind: config.__warehouse_specific_config__.bind,
            catalog_name: config.catalog_name,
            adapter: config.adapter,
            propagate: config.propagate,
            column_types: config.column_types,
            compute: config.compute,
            concurrent_batches: config.concurrent_batches,
            contract: config.contract,
            database: config.database,
            description: config.__warehouse_specific_config__.description,
            docs: config.docs,
            enabled: config.enabled,
            event_time: config.event_time,
            freshness: config.freshness,
            loaded_at_field: config.loaded_at_field,
            loaded_at_query: config.loaded_at_query,
            state: config.state,
            latest_version_pointer: config.latest_version_pointer,
            full_refresh: config.full_refresh,
            grants: config.grants,
            group: config.group,
            incremental_predicates: config.incremental_predicates,
            constraints: config.constraints,
            incremental_strategy: config.incremental_strategy,
            location: config.location,
            lookback: config.lookback,
            materialized: config.materialized,
            merge_exclude_columns: config.merge_exclude_columns,
            merge_update_columns: config.merge_update_columns,
            meta: config.meta.into(),
            submission_method: config.submission_method.clone(),
            job_cluster_config: config.job_cluster_config.clone(),
            python_job_config: config.python_job_config.clone(),
            cluster_id: config.cluster_id.clone(),
            http_path: config.http_path.clone(),
            create_notebook: config.create_notebook,
            index_url: config.index_url.clone(),
            additional_libs: config.additional_libs.clone(),
            user_folder_for_python: config.user_folder_for_python,
            environment_key: config.environment_key.clone(),
            environment_dependencies: config.environment_dependencies.clone(),
            on_configuration_change: config.on_configuration_change,
            on_error: config.on_error,
            on_schema_change: config.on_schema_change,
            packages: config.packages.into_inner(),
            python_version: config.python_version,
            imports: config.imports,
            secrets: config.secrets,
            external_access_integrations: config.external_access_integrations,
            use_anonymous_sproc: config.use_anonymous_sproc,
            persist_docs: config.persist_docs,
            post_hook: config.post_hook,
            pre_hook: config.pre_hook,
            predicates: config.predicates,
            quoting: config.quoting,
            schema: config.schema,
            sql_header: config.sql_header,
            static_analysis: config.static_analysis,
            table_format: config.table_format,
            tags: config.tags.into_inner().into(),
            classifiers: config.classifiers.into_inner().into(),
            transient: config.__warehouse_specific_config__.transient,
            unique_key: config.unique_key,
            adapter_properties: config.__warehouse_specific_config__.adapter_properties,
            external_volume: config.__warehouse_specific_config__.external_volume,
            base_location_root: config.__warehouse_specific_config__.base_location_root,
            base_location_subpath: config.__warehouse_specific_config__.base_location_subpath,
            iceberg_version: config.__warehouse_specific_config__.iceberg_version,
            change_tracking: config.__warehouse_specific_config__.change_tracking,
            data_retention_time_in_days: config
                .__warehouse_specific_config__
                .data_retention_time_in_days,
            target_lag: config.__warehouse_specific_config__.target_lag,
            snowflake_initialization_warehouse: config
                .__warehouse_specific_config__
                .snowflake_initialization_warehouse,
            snowflake_warehouse: config.__warehouse_specific_config__.snowflake_warehouse,
            refresh_warehouse: config.__warehouse_specific_config__.refresh_warehouse,
            immutable_where: config.__warehouse_specific_config__.immutable_where,
            refresh_mode: config.__warehouse_specific_config__.refresh_mode,
            initialize: config.__warehouse_specific_config__.initialize,
            scheduler: config.__warehouse_specific_config__.scheduler,
            tmp_relation_type: config.__warehouse_specific_config__.tmp_relation_type,
            query_tag: config.__warehouse_specific_config__.query_tag,
            query_tags: config.__warehouse_specific_config__.query_tags,
            table_tag: config.__warehouse_specific_config__.table_tag,
            row_access_policy: config.__warehouse_specific_config__.row_access_policy,
            automatic_clustering: config.__warehouse_specific_config__.automatic_clustering,
            jar_file_uri: config.__warehouse_specific_config__.jar_file_uri,
            timeout: config.__warehouse_specific_config__.timeout,
            batch_id: config.__warehouse_specific_config__.batch_id,
            dataproc_cluster_name: config.__warehouse_specific_config__.dataproc_cluster_name,
            notebook_template_id: config.__warehouse_specific_config__.notebook_template_id,
            enable_list_inference: config.__warehouse_specific_config__.enable_list_inference,
            intermediate_format: config.__warehouse_specific_config__.intermediate_format,
            storage_uri: config.__warehouse_specific_config__.storage_uri,
            copy_grants: config.__warehouse_specific_config__.copy_grants,
            copy_tags: config.__warehouse_specific_config__.copy_tags,
            secure: config.__warehouse_specific_config__.secure,
            partition_by: config.__warehouse_specific_config__.partition_by,
            partition_by_config: config.__warehouse_specific_config__.partition_by_config,
            distribute_by_config: config.__warehouse_specific_config__.distribute_by_config,
            primary_key_config: config.__warehouse_specific_config__.primary_key_config,
            cluster_by: config.__warehouse_specific_config__.cluster_by,
            hours_to_expiration: config.__warehouse_specific_config__.hours_to_expiration,
            job_execution_timeout_seconds: config
                .__warehouse_specific_config__
                .job_execution_timeout_seconds,
            reservation: config.__warehouse_specific_config__.reservation,
            labels: config.__warehouse_specific_config__.labels,
            labels_from_meta: config.__warehouse_specific_config__.labels_from_meta,
            resource_tags: config.__warehouse_specific_config__.resource_tags,
            kms_key_name: config.__warehouse_specific_config__.kms_key_name,
            require_partition_filter: config
                .__warehouse_specific_config__
                .require_partition_filter,
            partition_expiration_days: config
                .__warehouse_specific_config__
                .partition_expiration_days,
            grant_access_to: config.__warehouse_specific_config__.grant_access_to,
            partitions: config.__warehouse_specific_config__.partitions,
            enable_refresh: config.__warehouse_specific_config__.enable_refresh,
            refresh_interval_minutes: config
                .__warehouse_specific_config__
                .refresh_interval_minutes,
            max_staleness: config.__warehouse_specific_config__.max_staleness,
            max_data_extension_time_in_days: config
                .__warehouse_specific_config__
                .max_data_extension_time_in_days,
            file_format: config.__warehouse_specific_config__.file_format,
            location_root: config.__warehouse_specific_config__.location_root,
            use_uniform: config.__warehouse_specific_config__.use_uniform,
            tblproperties: config.__warehouse_specific_config__.tblproperties,
            include_full_name_in_path: config
                .__warehouse_specific_config__
                .include_full_name_in_path,
            liquid_clustered_by: config.__warehouse_specific_config__.liquid_clustered_by,
            auto_liquid_cluster: config.__warehouse_specific_config__.auto_liquid_cluster,
            zorder: config.__warehouse_specific_config__.zorder,
            clustered_by: config.__warehouse_specific_config__.clustered_by,
            buckets: config.__warehouse_specific_config__.buckets,
            catalog: config.__warehouse_specific_config__.catalog,
            databricks_tags: config.__warehouse_specific_config__.databricks_tags,
            compression: config.__warehouse_specific_config__.compression,
            databricks_compute: config.__warehouse_specific_config__.databricks_compute,
            dist: config.__warehouse_specific_config__.dist,
            sort: config.__warehouse_specific_config__.sort,
            sort_type: config.__warehouse_specific_config__.sort_type,
            matched_condition: config.__warehouse_specific_config__.matched_condition,
            merge_with_schema_evolution: config
                .__warehouse_specific_config__
                .merge_with_schema_evolution,
            not_matched_by_source_action: config
                .__warehouse_specific_config__
                .not_matched_by_source_action,
            not_matched_by_source_condition: config
                .__warehouse_specific_config__
                .not_matched_by_source_condition,
            not_matched_condition: config.__warehouse_specific_config__.not_matched_condition,
            source_alias: config.__warehouse_specific_config__.source_alias,
            target_alias: config.__warehouse_specific_config__.target_alias,
            skip_matched_step: config.__warehouse_specific_config__.skip_matched_step,
            skip_not_matched_step: config.__warehouse_specific_config__.skip_not_matched_step,
            skip_optimize: config.__warehouse_specific_config__.skip_optimize,
            storage_serialization_policy: config
                .__warehouse_specific_config__
                .storage_serialization_policy,
            target_file_size: config.__warehouse_specific_config__.target_file_size,
            as_columnstore: config.__warehouse_specific_config__.as_columnstore,
            table_type: config.__warehouse_specific_config__.table_type,
            indexes: config.__warehouse_specific_config__.indexes,
            unlogged: config.__warehouse_specific_config__.unlogged,
            schedule: config.__warehouse_specific_config__.schedule,
            row_filter: config.__warehouse_specific_config__.row_filter,
            incremental_apply_config_changes: config
                .__warehouse_specific_config__
                .incremental_apply_config_changes,
            persist_constraints: config.__warehouse_specific_config__.persist_constraints,
            use_safer_relation_operations: config
                .__warehouse_specific_config__
                .use_safer_relation_operations,
            view_update_via_alter: config.__warehouse_specific_config__.view_update_via_alter,
            unique_tmp_table_suffix: config.__warehouse_specific_config__.unique_tmp_table_suffix,
            primary_key: config.__warehouse_specific_config__.primary_key,
            category: config.__warehouse_specific_config__.category,
            sync: config.sync,
            engine: config.__warehouse_specific_config__.engine,
            order_by: config.__warehouse_specific_config__.order_by,
            ttl: config.__warehouse_specific_config__.ttl,
            settings: config.__warehouse_specific_config__.settings,
            query_settings: config.__warehouse_specific_config__.query_settings,
            projections: config.__warehouse_specific_config__.projections,
            inserts_only: config.__warehouse_specific_config__.inserts_only,
            connection_overrides: config.__warehouse_specific_config__.connection_overrides,
            fields: config.__warehouse_specific_config__.fields,
            source_type: config.__warehouse_specific_config__.source_type,
            url: config.__warehouse_specific_config__.url,
            format: config.__warehouse_specific_config__.format,
            layout: config.__warehouse_specific_config__.layout,
            lifetime: config.__warehouse_specific_config__.lifetime,
            range: config.__warehouse_specific_config__.range,
            table: config.__warehouse_specific_config__.table,
            update_field: config.__warehouse_specific_config__.update_field,
            update_lag: config.__warehouse_specific_config__.update_lag,
            definer: config.__warehouse_specific_config__.definer,
            sql_security: config.__warehouse_specific_config__.sql_security,
            refreshable: config.__warehouse_specific_config__.refreshable,
            catchup: config.__warehouse_specific_config__.catchup,
            mv_on_schema_change: config.__warehouse_specific_config__.mv_on_schema_change,
            repopulate_from_mvs_on_full_refresh: config
                .__warehouse_specific_config__
                .repopulate_from_mvs_on_full_refresh,
            __additional_properties__: BTreeMap::new(),
        }
    }
}

impl ResolvableConfig<ModelConfig> for ModelConfig {
    fn default_to(&mut self, parent: &ModelConfig) {
        self.default_to_fields(parent);
    }

    type Resolved = ResolvedModelConfig;
    type PackageDefaults = DbtQuoting;
    type ResolveDefaults = (StaticAnalysisKind, Option<SyncConfig>, Option<AdapterType>);

    fn get_enabled_with_default(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn get_enabled(&self) -> Option<bool> {
        self.enabled
    }

    fn disable(&mut self) {
        self.enabled = Some(false);
    }

    fn apply_package_defaults(&mut self, quoting: DbtQuoting) {
        if self.quoting.is_none() {
            self.quoting = Some(quoting);
        }
    }

    fn apply_resolve_defaults(
        &mut self,
        (static_analysis, sync, default_adapter): Self::ResolveDefaults,
    ) {
        if self.static_analysis.is_none() {
            self.static_analysis = Some(Spanned::new(static_analysis));
        }
        if self.sync.is_none() {
            self.sync = sync;
        }
        if self.on_configuration_change.is_none() {
            self.on_configuration_change = Some(OnConfigurationChange::default());
        }
        // Lake compute writes open-format tables: a node placed there materializes
        // an Iceberg table unless its author says otherwise. Applied here rather
        // than through `#[resolved(default = ...)]` because the default turns on
        // the node's effective adapter, which is only known once every config layer
        // has merged -- and `finalize` has already collapsed `materialized` to the
        // static `view` default by the time a caller could inspect it.
        if self.adapter.or(default_adapter) == Some(AdapterType::LakeCompute) {
            if self.materialized.is_none() {
                self.materialized = Some(DbtMaterialization::Table);
            }
            if self.table_format.is_none() {
                self.table_format = Some("iceberg".to_string());
            }
        }
    }

    fn finalize(self) -> ResolvedModelConfig {
        self.finalize_resolved()
    }

    fn canonicalize_adapter_aliases(&mut self, default_adapter: AdapterType) {
        if let Some(catalog) = take_databricks_catalog_alias(
            default_adapter,
            &mut self.__warehouse_specific_config__,
            !self.database.is_omitted(),
        ) {
            self.database = Omissible::Present(Some(catalog));
        }
        // BigQuery's `project`/`dataset` aliases are already routed to `database`/`schema` by
        // the pre-existing, ungated serde `alias`es on those fields (D1); nothing to do here.
    }
}

impl ModelConfig {
    pub fn same_database_representation(&self, other: &ModelConfig) -> bool {
        let database_eq = omissible_option_eq(&self.database, &other.database);
        let alias_eq = self.alias == other.alias;

        let result = database_eq && alias_eq;

        if !result {
            log_state_mod_diff(
                "unique_id in next model_config log",
                "model_database_representation",
                [
                    (
                        "database",
                        database_eq,
                        Some((
                            format!("{:?}", &self.database),
                            format!("{:?}", &other.database),
                        )),
                    ),
                    (
                        "alias",
                        alias_eq,
                        Some((format!("{:?}", &self.alias), format!("{:?}", &other.alias))),
                    ),
                ],
            );
        }

        result
    }

    /// Custom comparison that treats Omitted and Present(None) as equivalent for schema/database fields
    pub fn same_config(&self, other: &ModelConfig) -> bool {
        // Compare all fields.
        let enabled_eq = self.enabled == other.enabled;
        let catalog_name_eq = self.catalog_name == other.catalog_name;
        let adapter_eq = self.adapter == other.adapter;
        let propagate_eq = self.propagate == other.propagate;
        let meta_eq_result = meta_eq(&self.meta, &other.meta); // Custom comparison for meta
        let materialized_eq_result = materialized_eq(&self.materialized, &other.materialized);
        let incremental_strategy_eq = self.incremental_strategy == other.incremental_strategy;
        // incremental_predicates can differ because of environment, i.e. dev vs prod
        // so we don't compare them. To compare them we will need a SQL AST whose
        // shape and node types can be compared rather contents of each node.
        // && self.incremental_predicates == other.incremental_predicates
        let batch_size_eq = self.batch_size == other.batch_size;
        let lookback_eq_result = lookback_eq(&self.lookback, &other.lookback); // Custom comparison for lookback
        let begin_eq = self.begin == other.begin;
        let persist_docs_eq_result = persist_docs_eq(&self.persist_docs, &other.persist_docs); // Custom comparison for persist_docs
        let post_hook_eq = hooks_equal(&self.post_hook, &other.post_hook);
        let pre_hook_eq = hooks_equal(&self.pre_hook, &other.pre_hook);
        // let quoting_eq = self.quoting == other.quoting // TODO: re-enable when no longer using mantle/core manifests in IA
        let column_types_eq_result = column_types_eq(&self.column_types, &other.column_types); // Custom comparison for column_types
        let full_refresh_eq = self.full_refresh == other.full_refresh;
        let unique_key_eq = self.unique_key == other.unique_key;
        let on_schema_change_eq_result =
            on_schema_change_eq(&self.on_schema_change, &other.on_schema_change); // Custom comparison for on_schema_change
        let on_configuration_change_eq_result = on_configuration_change_eq(
            &self.on_configuration_change,
            &other.on_configuration_change,
        ); // Custom comparison for on_configuration_change
        let on_error_eq = self.on_error == other.on_error;
        let grants_eq = grants_equal(&self.grants, &other.grants); // Custom comparison for grants
        let packages_eq = packages_and_imports_eq(self.packages.inner(), other.packages.inner()); // Custom comparison for packages
        let imports_eq = packages_and_imports_eq(&self.imports, &other.imports); // Custom comparison for imports (same function as packages)
        let python_version_eq = self.python_version == other.python_version;
        let docs_eq_result = docs_eq(&self.docs, &other.docs); // Custom comparison for docs
        // This is a project level config that can differ between environments,
        // so we don't compare them.
        // && self.event_time == other.event_time
        let concurrent_batches_eq = self.concurrent_batches == other.concurrent_batches;
        let merge_update_columns_eq = self.merge_update_columns == other.merge_update_columns;
        let merge_exclude_columns_eq = self.merge_exclude_columns == other.merge_exclude_columns;
        let access_eq_result = access_eq(&self.access, &other.access); // Custom comparison for access
        let table_format_eq = self.table_format == other.table_format;
        let freshness_eq = self.freshness == other.freshness;
        let state_eq = self.state == other.state;
        // Treat `None` and `Some(LatestVersionPointer::default())` as equivalent so that
        // previous-state manifests written before `latest_version_pointer` was emitted as
        // a default struct (i.e. `null`) do not register as modified after the emit change.
        let default_lvp = LatestVersionPointer::default();
        let latest_version_pointer_eq =
            self.latest_version_pointer.as_ref().unwrap_or(&default_lvp)
                == other
                    .latest_version_pointer
                    .as_ref()
                    .unwrap_or(&default_lvp);
        let sql_header_eq = self.sql_header == other.sql_header;
        let location_eq = self.location == other.location;
        let predicates_eq = self.predicates == other.predicates;
        let warehouse_config_eq = same_warehouse_config(
            &self.__warehouse_specific_config__,
            &other.__warehouse_specific_config__,
        );

        let result = enabled_eq
            && catalog_name_eq
            && adapter_eq
            && propagate_eq
            && meta_eq_result
            && materialized_eq_result
            && incremental_strategy_eq
            && batch_size_eq
            && lookback_eq_result
            && begin_eq
            && persist_docs_eq_result
            && post_hook_eq
            && pre_hook_eq
            // && quoting_eq
            && column_types_eq_result
            && full_refresh_eq
            && unique_key_eq
            && on_schema_change_eq_result
            && on_configuration_change_eq_result
            && on_error_eq
            && grants_eq
            && packages_eq
            && imports_eq
            && python_version_eq
            && docs_eq_result
            // && event_time_eq
            && concurrent_batches_eq
            && merge_update_columns_eq
            && merge_exclude_columns_eq
            && access_eq_result
            && table_format_eq
            && freshness_eq
            && state_eq
            && latest_version_pointer_eq
            && sql_header_eq
            && location_eq
            && predicates_eq
            && warehouse_config_eq;

        if !result {
            log_state_mod_diff(
                "unique_id in next model_config log",
                "model_config",
                [
                    (
                        "enabled",
                        enabled_eq,
                        Some((
                            format!("{:?}", &self.enabled),
                            format!("{:?}", &other.enabled),
                        )),
                    ),
                    (
                        "catalog_name",
                        catalog_name_eq,
                        Some((
                            format!("{:?}", &self.catalog_name),
                            format!("{:?}", &other.catalog_name),
                        )),
                    ),
                    (
                        "adapter",
                        adapter_eq,
                        Some((
                            format!("{:?}", &self.adapter),
                            format!("{:?}", &other.adapter),
                        )),
                    ),
                    (
                        "propagate",
                        propagate_eq,
                        Some((
                            format!("{:?}", &self.propagate),
                            format!("{:?}", &other.propagate),
                        )),
                    ),
                    (
                        "meta",
                        meta_eq_result,
                        Some((format!("{:?}", &self.meta), format!("{:?}", &other.meta))),
                    ),
                    (
                        "materialized",
                        materialized_eq_result,
                        Some((
                            format!("{:?}", &self.materialized),
                            format!("{:?}", &other.materialized),
                        )),
                    ),
                    (
                        "incremental_strategy",
                        incremental_strategy_eq,
                        Some((
                            format!("{:?}", &self.incremental_strategy),
                            format!("{:?}", &other.incremental_strategy),
                        )),
                    ),
                    (
                        "batch_size",
                        batch_size_eq,
                        Some((
                            format!("{:?}", &self.batch_size),
                            format!("{:?}", &other.batch_size),
                        )),
                    ),
                    (
                        "lookback",
                        lookback_eq_result,
                        Some((
                            format!("{:?}", &self.lookback),
                            format!("{:?}", &other.lookback),
                        )),
                    ),
                    (
                        "begin",
                        begin_eq,
                        Some((format!("{:?}", &self.begin), format!("{:?}", &other.begin))),
                    ),
                    ("persist_docs", persist_docs_eq_result, None),
                    (
                        "post_hook",
                        post_hook_eq,
                        Some((
                            format!("{:?}", &self.post_hook),
                            format!("{:?}", &other.post_hook),
                        )),
                    ),
                    (
                        "pre_hook",
                        pre_hook_eq,
                        Some((
                            format!("{:?}", &self.pre_hook),
                            format!("{:?}", &other.pre_hook),
                        )),
                    ),
                    (
                        "column_types",
                        column_types_eq_result,
                        Some((
                            format!("{:?}", &self.column_types),
                            format!("{:?}", &other.column_types),
                        )),
                    ),
                    (
                        "full_refresh",
                        full_refresh_eq,
                        Some((
                            format!("{:?}", &self.full_refresh),
                            format!("{:?}", &other.full_refresh),
                        )),
                    ),
                    (
                        "unique_key",
                        unique_key_eq,
                        Some((
                            format!("{:?}", &self.unique_key),
                            format!("{:?}", &other.unique_key),
                        )),
                    ),
                    (
                        "on_schema_change",
                        on_schema_change_eq_result,
                        Some((
                            format!("{:?}", &self.on_schema_change),
                            format!("{:?}", &other.on_schema_change),
                        )),
                    ),
                    (
                        "on_configuration_change",
                        on_configuration_change_eq_result,
                        Some((
                            format!("{:?}", &self.on_configuration_change),
                            format!("{:?}", &other.on_configuration_change),
                        )),
                    ),
                    (
                        "on_error",
                        on_error_eq,
                        Some((
                            format!("{:?}", &self.on_error),
                            format!("{:?}", &other.on_error),
                        )),
                    ),
                    (
                        "grants",
                        grants_eq,
                        Some((
                            format!("{:?}", &self.grants),
                            format!("{:?}", &other.grants),
                        )),
                    ),
                    (
                        "packages",
                        packages_eq,
                        Some((
                            format!("{:?}", &self.packages),
                            format!("{:?}", &other.packages),
                        )),
                    ),
                    (
                        "imports",
                        imports_eq,
                        Some((
                            format!("{:?}", &self.imports),
                            format!("{:?}", &other.imports),
                        )),
                    ),
                    (
                        "python_version",
                        python_version_eq,
                        Some((
                            format!("{:?}", &self.python_version),
                            format!("{:?}", &other.python_version),
                        )),
                    ),
                    (
                        "docs",
                        docs_eq_result,
                        Some((format!("{:?}", &self.docs), format!("{:?}", &other.docs))),
                    ),
                    (
                        "concurrent_batches",
                        concurrent_batches_eq,
                        Some((
                            format!("{:?}", &self.concurrent_batches),
                            format!("{:?}", &other.concurrent_batches),
                        )),
                    ),
                    (
                        "merge_update_columns",
                        merge_update_columns_eq,
                        Some((
                            format!("{:?}", &self.merge_update_columns),
                            format!("{:?}", &other.merge_update_columns),
                        )),
                    ),
                    (
                        "merge_exclude_columns",
                        merge_exclude_columns_eq,
                        Some((
                            format!("{:?}", &self.merge_exclude_columns),
                            format!("{:?}", &other.merge_exclude_columns),
                        )),
                    ),
                    (
                        "access",
                        access_eq_result,
                        Some((
                            format!("{:?}", &self.access),
                            format!("{:?}", &other.access),
                        )),
                    ),
                    (
                        "table_format",
                        table_format_eq,
                        Some((
                            format!("{:?}", &self.table_format),
                            format!("{:?}", &other.table_format),
                        )),
                    ),
                    (
                        "freshness",
                        freshness_eq,
                        Some((
                            format!("{:?}", &self.freshness),
                            format!("{:?}", &other.freshness),
                        )),
                    ),
                    (
                        "state",
                        state_eq,
                        Some((format!("{:?}", &self.state), format!("{:?}", &other.state))),
                    ),
                    (
                        "latest_version_pointer",
                        latest_version_pointer_eq,
                        Some((
                            format!("{:?}", &self.latest_version_pointer),
                            format!("{:?}", &other.latest_version_pointer),
                        )),
                    ),
                    (
                        "sql_header",
                        sql_header_eq,
                        Some((
                            format!("{:?}", &self.sql_header),
                            format!("{:?}", &other.sql_header),
                        )),
                    ),
                    (
                        "location",
                        location_eq,
                        Some((
                            format!("{:?}", &self.location),
                            format!("{:?}", &other.location),
                        )),
                    ),
                    (
                        "predicates",
                        predicates_eq,
                        Some((
                            format!("{:?}", &self.predicates),
                            format!("{:?}", &other.predicates),
                        )),
                    ),
                    ("warehouse_config", warehouse_config_eq, None),
                ],
            );
        }

        result
    }
}

impl ConfigKeys for ModelConfig {
    fn valid_field_names() -> HashSet<String> {
        let default_instance = Self::default();
        let serialized = dbt_yaml::to_value(&default_instance)
            .expect("Failed to serialize ModelConfig for field extraction");

        let mut field_names = HashSet::new();

        if let YmlValue::Mapping(map, _) = serialized {
            for (key, value) in map {
                if let YmlValue::String(key_str, _) = key {
                    // Extract top-level fields
                    field_names.insert(key_str.clone());

                    // Also include warehouse-specific fields from __warehouse_specific_config__
                    if key_str == "__warehouse_specific_config__" {
                        if let YmlValue::Mapping(warehouse_map, _) = value {
                            for (warehouse_key, _) in warehouse_map {
                                if let YmlValue::String(warehouse_key_str, _) = warehouse_key {
                                    field_names.insert(warehouse_key_str);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Add known aliases that might not show up in serialization
        // The first three are ungated adapter aliases (fs#13424).
        field_names.insert("project".to_string()); // alias for database
        field_names.insert("data_space".to_string()); // alias for database
        field_names.insert("dataset".to_string()); // alias for schema
        field_names.insert("post-hook".to_string()); // might be serialized as post_hook
        field_names.insert("pre-hook".to_string()); // might be serialized as pre_hook

        field_names
    }
}

// Helper function to compare on_schema_change fields, treating None and default OnSchemaChange as equivalent
fn on_schema_change_eq(a: &Option<OnSchemaChange>, b: &Option<OnSchemaChange>) -> bool {
    use crate::schemas::common::OnSchemaChange;
    let default_on_schema_change = OnSchemaChange::default();

    match (a, b) {
        // Both None
        (None, None) => true,
        // Both Some - direct comparison
        (Some(a_val), Some(b_val)) => a_val == b_val,
        // One None, one Some - check if the Some value equals default
        (None, Some(b_val)) => b_val == &default_on_schema_change,
        (Some(a_val), None) => a_val == &default_on_schema_change,
    }
}

// Helper function to compare persist_docs fields, treating None and default PersistDocsConfig as equivalent
fn persist_docs_eq(a: &Option<PersistDocsConfig>, b: &Option<PersistDocsConfig>) -> bool {
    use crate::schemas::common::PersistDocsConfig;
    // Default value in dbt-core is empty dict {}
    // See https://github.com/dbt-labs/dbt-core/blob/main/core/dbt/artifacts/resources/v1/config.py#L86
    let default_persist_docs = PersistDocsConfig {
        columns: None,
        relation: None,
    };

    match (a, b) {
        // Both None
        (None, None) => true,
        // Both Some - direct comparison
        (Some(a_val), Some(b_val)) => a_val == b_val,
        // One None, one Some - check if the Some value equals default
        (None, Some(b_val)) => b_val == &default_persist_docs,
        (Some(a_val), None) => a_val == &default_persist_docs,
    }
}

// Helper function to compare lookback fields, treating None and default lookback as equivalent
fn lookback_eq(a: &Option<i32>, b: &Option<i32>) -> bool {
    // Default value in dbt-core is 1
    // See https://github.com/dbt-labs/dbt-core/blob/main/core/dbt/artifacts/resources/v1/config.py#L84
    let default_lookback = 1;

    match (a, b) {
        // Both None
        (None, None) => true,
        // Both Some - direct comparison
        (Some(a_val), Some(b_val)) => a_val == b_val,
        // One None, one Some - check if the Some value equals default
        (None, Some(b_val)) => b_val == &default_lookback,
        (Some(a_val), None) => a_val == &default_lookback,
    }
}

// Helper function to compare column_types fields, treating None and empty BTreeMap as equivalent
fn column_types_eq(
    a: &Option<BTreeMap<Spanned<String>, String>>,
    b: &Option<BTreeMap<Spanned<String>, String>>,
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

// Helper function to compare packages and imports fields, treating None and empty ArrayOfStrings as equivalent
fn packages_and_imports_eq(
    a: &Option<StringOrArrayOfStrings>,
    b: &Option<StringOrArrayOfStrings>,
) -> bool {
    use crate::schemas::serde::StringOrArrayOfStrings;

    match (a, b) {
        // Both None
        (None, None) => true,
        // Both Some - direct comparison
        (Some(a_val), Some(b_val)) => a_val == b_val,
        // One None, one Some - check if the Some value is an empty ArrayOfStrings
        (None, Some(StringOrArrayOfStrings::ArrayOfStrings(b_vec))) => b_vec.is_empty(),
        (Some(StringOrArrayOfStrings::ArrayOfStrings(a_vec)), None) => a_vec.is_empty(),
        // If one is None and the other is Some(String), they are not equal
        (None, Some(StringOrArrayOfStrings::String(_))) => false,
        (Some(StringOrArrayOfStrings::String(_)), None) => false,
    }
}

// Helper function to compare on_configuration_change fields, treating None and default OnConfigurationChange as equivalent
fn on_configuration_change_eq(
    a: &Option<OnConfigurationChange>,
    b: &Option<OnConfigurationChange>,
) -> bool {
    use crate::schemas::common::OnConfigurationChange;
    // Default value in dbt-core is "apply"
    // See https://github.com/dbt-labs/dbt-core/blob/main/core/dbt/artifacts/resources/v1/config.py#L110-L112
    // and https://github.com/dbt-labs/dbt-common/blob/eb6b6f4a155f94d4863d8f503f8eb997ab6226d3/dbt_common/contracts/config/materialization.py#L4-L11
    let default_on_configuration_change = OnConfigurationChange::Apply;

    match (a, b) {
        // Both None
        (None, None) => true,
        // Both Some - direct comparison
        (Some(a_val), Some(b_val)) => a_val == b_val,
        // One None, one Some - check if the Some value equals default
        (None, Some(b_val)) => b_val == &default_on_configuration_change,
        (Some(a_val), None) => a_val == &default_on_configuration_change,
    }
}

// Helper function to compare materialized fields, treating None as View (the default)
fn materialized_eq(a: &Option<DbtMaterialization>, b: &Option<DbtMaterialization>) -> bool {
    let default_materialized = ModelConfig::default_materialized();

    match (a, b) {
        (None, None) => true,
        (Some(a_val), Some(b_val)) => a_val == b_val,
        (None, Some(b_val)) => b_val == &default_materialized,
        (Some(a_val), None) => a_val == &default_materialized,
    }
}

#[cfg(test)]
mod tests {
    use super::ModelConfig;
    use crate::schemas::common::{
        ConstraintType, FreshnessPeriod, OnConfigurationChange, UpdatesOn,
    };
    use crate::schemas::manifest::ManifestModelConfig;
    use crate::schemas::project::WarehouseSpecificNodeConfig;
    use crate::schemas::project::configs::model_config::ProjectModelConfig;
    use crate::schemas::project::dbt_project::ResolvableConfig;
    use crate::schemas::properties::StatePreClone;
    use crate::schemas::serde::{AdapterTypeOrArray, RefreshableConfig, StringOrArrayOfStrings};
    use dbt_adapter_core::AdapterType;

    /// `+propagate` rides the same project -> node -> project path `+adapter`
    /// does, in both its single-value and list forms.
    #[test]
    fn test_project_model_config_propagate_parses_and_round_trips() {
        let single: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+propagate: snowflake
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(
            single.propagate,
            Some(AdapterTypeOrArray::One(AdapterType::Snowflake))
        );

        let model_config: ModelConfig = single.into();
        let round_tripped: ProjectModelConfig = model_config.into();
        assert_eq!(
            round_tripped.propagate,
            Some(AdapterTypeOrArray::One(AdapterType::Snowflake))
        );

        let list: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+propagate: [snowflake]
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(
            list.propagate,
            Some(AdapterTypeOrArray::Many(vec![AdapterType::Snowflake]))
        );
    }

    #[test]
    fn test_model_clustered_by_accepts_ordered_list() {
        let config: ModelConfig = dbt_yaml::from_str(
            r#"
__warehouse_specific_config__:
  clustered_by: [nation, region]
"#,
        )
        .unwrap();
        assert_eq!(
            config.__warehouse_specific_config__.clustered_by,
            Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "nation".to_string(),
                "region".to_string(),
            ]))
        );
    }

    #[test]
    fn test_project_model_clustered_by_accepts_ordered_list() {
        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+clustered_by: [nation, region]
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(
            config.clustered_by,
            Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "nation".to_string(),
                "region".to_string(),
            ]))
        );
    }

    #[test]
    fn test_tblproperties_order_survives_project_to_model_config() {
        let project_config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+tblproperties:
  zeta: last
  alpha: first
  middle: center
__additional_properties__: {}
"#,
        )
        .unwrap();

        let model_config: ModelConfig = project_config.into();
        let keys = model_config
            .__warehouse_specific_config__
            .tblproperties
            .as_ref()
            .expect("tblproperties should parse")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();

        assert_eq!(keys, ["zeta", "alpha", "middle"]);
    }

    #[test]
    fn test_tblproperties_default_to_merges() {
        let parent: ModelConfig = dbt_yaml::from_str(
            r#"
__warehouse_specific_config__:
  tblproperties:
    parent_only: keep
    shared: parent
"#,
        )
        .unwrap();

        let mut configured_child: ModelConfig = dbt_yaml::from_str(
            r#"
__warehouse_specific_config__:
  tblproperties:
    shared: child
    child_only: new
"#,
        )
        .unwrap();
        configured_child.default_to(&parent);
        let merged = configured_child
            .__warehouse_specific_config__
            .tblproperties
            .as_ref()
            .expect("configured child should retain tblproperties");
        assert_eq!(
            merged.keys().map(String::as_str).collect::<Vec<_>>(),
            ["parent_only", "shared", "child_only"]
        );
        assert_eq!(merged.get("shared").and_then(|v| v.as_str()), Some("child"));

        let mut empty_child: ModelConfig = dbt_yaml::from_str(
            r#"
__warehouse_specific_config__:
  tblproperties: {}
"#,
        )
        .unwrap();
        empty_child.default_to(&parent);
        let empty_keys = empty_child
            .__warehouse_specific_config__
            .tblproperties
            .as_ref()
            .expect("empty tblproperties should remain Some")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(empty_keys, ["parent_only", "shared"]);

        let mut unset_child = ModelConfig::default();
        unset_child.default_to(&parent);
        let inherited_keys = unset_child
            .__warehouse_specific_config__
            .tblproperties
            .as_ref()
            .expect("unset child should inherit tblproperties")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(inherited_keys, ["parent_only", "shared"]);
    }

    #[test]
    fn test_databricks_model_deserializes_and_resolves_query_tags() {
        let project: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+query_tags: '{"team":"model"}'
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(project.query_tags.as_deref(), Some(r#"{"team":"model"}"#));

        let resolved = ModelConfig::from(project);
        assert_eq!(
            resolved.__warehouse_specific_config__.query_tags.as_deref(),
            Some(r#"{"team":"model"}"#)
        );
    }

    #[test]
    fn test_skip_optimize_inherits_from_parent_model_config() {
        let parent = ModelConfig {
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                skip_optimize: Some(true),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut inherited = ModelConfig::default();
        inherited.default_to(&parent);
        assert_eq!(
            inherited.__warehouse_specific_config__.skip_optimize,
            Some(true)
        );

        let mut overridden = ModelConfig {
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                skip_optimize: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        overridden.default_to(&parent);
        assert_eq!(
            overridden.__warehouse_specific_config__.skip_optimize,
            Some(false)
        );
    }

    #[test]
    fn omitted_on_configuration_change_defaults_to_apply_in_runtime_configs() {
        use dbt_common::io_args::StaticAnalysisKind;

        let project: ProjectModelConfig =
            dbt_yaml::from_str("__additional_properties__: {}\n").unwrap();
        let mut project_runtime: ModelConfig = project.into();
        assert_eq!(project_runtime.on_configuration_change, None);
        project_runtime.apply_resolve_defaults((StaticAnalysisKind::default(), None, None));
        assert_eq!(
            project_runtime.on_configuration_change,
            Some(OnConfigurationChange::Apply)
        );

        let manifest: ManifestModelConfig =
            dbt_yaml::from_str("__warehouse_specific_config__: {}\n").unwrap();
        let mut manifest_runtime: ModelConfig = manifest.into();
        assert_eq!(manifest_runtime.on_configuration_change, None);
        manifest_runtime.apply_resolve_defaults((StaticAnalysisKind::default(), None, None));
        assert_eq!(
            manifest_runtime.on_configuration_change,
            Some(OnConfigurationChange::Apply)
        );
    }

    #[test]
    fn test_classifiers_merge_in_default_to() {
        use crate::schemas::project::configs::config_merge::Classifiers;
        use crate::schemas::project::dbt_project::ResolvableConfig;
        use crate::schemas::serde::StringOrArrayOfStrings;

        let parent = ModelConfig {
            classifiers: Classifiers(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "finance".to_string(),
                "pii".to_string(),
            ]))),
            ..Default::default()
        };

        let mut child = ModelConfig {
            classifiers: Classifiers(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "gdpr".to_string(),
                "pii".to_string(),
            ]))),
            ..Default::default()
        };

        child.default_to(&parent);

        assert_eq!(
            child.classifiers.into_inner(),
            Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "finance".to_string(),
                "gdpr".to_string(),
                "pii".to_string(),
            ]))
        );
    }

    #[test]
    fn test_classifiers_none_child_inherits_parent() {
        use crate::schemas::project::configs::config_merge::Classifiers;
        use crate::schemas::project::dbt_project::ResolvableConfig;
        use crate::schemas::serde::StringOrArrayOfStrings;

        let parent = ModelConfig {
            classifiers: Classifiers(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "pii".to_string(),
            ]))),
            ..Default::default()
        };

        let mut child = ModelConfig {
            classifiers: Classifiers(None),
            ..Default::default()
        };

        child.default_to(&parent);

        assert_eq!(
            child.classifiers.into_inner(),
            Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "pii".to_string(),
            ]))
        );
    }

    #[test]
    fn test_model_config_state_parses() {
        let config: ModelConfig = dbt_yaml::from_str(
            r#"
state:
  lag_tolerance:
    count: 2
    period: hour
  require_fresh_data_from: all
  evaluate_volatile_sql: true
  pre_clone: if_missing
  execute_hooks_on_any_reuse: true
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("state config should parse");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(2));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::hour));
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::All));
        assert_eq!(state.evaluate_volatile_sql, Some(true));
        assert_eq!(state.pre_clone, Some(StatePreClone::IfMissing));
        assert_eq!(state.execute_hooks_on_any_reuse, Some(true));
    }

    /// Regression for #16135: `state:` keys merge key by key, so a model that sets
    /// only `require_fresh_data_from` still inherits the project-level `lag_tolerance`.
    #[test]
    fn test_model_config_state_merges_field_by_field() {
        use crate::schemas::common::ModelFreshnessRules;
        use crate::schemas::properties::ModelState;

        let parent = ModelConfig {
            state: Some(ModelState {
                lag_tolerance: Some(ModelFreshnessRules {
                    count: Some(1),
                    period: Some(FreshnessPeriod::minute),
                    updates_on: None,
                }),
                require_fresh_data_from: None,
                evaluate_volatile_sql: None,
                pre_clone: Some(StatePreClone::Always),
                execute_hooks_on_any_reuse: None,
                compare_unrendered_code: None,
            }),
            ..Default::default()
        };
        let mut child = ModelConfig {
            state: Some(ModelState {
                lag_tolerance: None,
                require_fresh_data_from: Some(UpdatesOn::All),
                evaluate_volatile_sql: None,
                pre_clone: None,
                execute_hooks_on_any_reuse: None,
                compare_unrendered_code: None,
            }),
            ..Default::default()
        };
        child.default_to(&parent);

        let state = child.state.expect("state should survive the merge");
        let lag_tolerance = state
            .lag_tolerance
            .expect("project-level lag_tolerance should be inherited");
        assert_eq!(lag_tolerance.count, Some(1));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::minute));
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::All));
        assert_eq!(state.pre_clone, Some(StatePreClone::Always));
    }

    /// Regression for fs#13343: Core accepts a sequence-valued `column_types` entry
    /// (e.g. produced by a templated macro); Fusion must not reject it during YAML load.
    #[test]
    fn test_model_config_column_types_accepts_sequence_value() {
        let config: ModelConfig = dbt_yaml::from_str(
            r#"
column_types:
  id:
    - integer
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();

        let column_types = config.column_types.expect("column_types should parse");
        assert_eq!(
            column_types.get(&dbt_yaml::Spanned::from("id".to_string())),
            Some(&"integer".to_string())
        );
    }

    /// Regression for fs#13343: Core accepts a mapping-valued `event_time`; Fusion
    /// must not reject it during YAML load.
    #[test]
    fn test_model_config_event_time_accepts_mapping_value() {
        let config: ModelConfig = dbt_yaml::from_str(
            r#"
event_time:
  column: event_at
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();

        assert_eq!(
            config.event_time.as_deref(),
            Some(r#"{"column":"event_at"}"#)
        );
    }

    /// dbt-core accepts a mapping-valued `config: constraints:` and yields no constraint from
    /// it, while the sequence shape still parses into typed constraints.
    #[test]
    fn test_model_config_constraints_accepts_mapping_and_sequence() {
        let mapping: ModelConfig = dbt_yaml::from_str(
            r#"
constraints:
  primary_key:
    - id
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();

        assert_eq!(mapping.constraints, None);

        let sequence: ModelConfig = dbt_yaml::from_str(
            r#"
constraints:
  - type: primary_key
    columns: [id]
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();

        let constraints = sequence.constraints.expect("constraints should parse");
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].type_, ConstraintType::PrimaryKey);
        assert_eq!(constraints[0].columns, Some(vec!["id".to_string()]));
    }

    #[test]
    fn test_project_model_config_state_parses_with_plus_prefix() {
        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+state:
  lag_tolerance:
    count: 30
    period: minute
  require_fresh_data_from: any
  evaluate_volatile_sql: false
  pre_clone: always
  execute_hooks_on_any_reuse: false
__additional_properties__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("+state config should parse");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(30));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::minute));
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::Any));
        assert_eq!(state.evaluate_volatile_sql, Some(false));
        assert_eq!(state.pre_clone, Some(StatePreClone::Always));
        assert_eq!(state.execute_hooks_on_any_reuse, Some(false));
    }

    #[test]
    fn test_python_version_deserializes_from_number() {
        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+python_version: 3.10
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(config.python_version.as_deref(), Some("3.1"));

        let config: ModelConfig = dbt_yaml::from_str(
            r#"
python_version: 3.10
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();
        assert_eq!(config.python_version.as_deref(), Some("3.1"));

        let config: ManifestModelConfig = dbt_yaml::from_str(
            r#"
python_version: 3.10
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();
        assert_eq!(config.python_version.as_deref(), Some("3.1"));
    }

    #[test]
    fn test_project_model_config_state_lag_tolerance_parses_duration_string() {
        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+state:
  lag_tolerance: "1 day"
__additional_properties__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("+state config should parse");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(1));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::day));

        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+state:
  lag_tolerance: "1d"
__additional_properties__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("+state config should parse");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(1));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::day));

        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+state:
  lag_tolerance: "1.5h"
__additional_properties__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("+state config should parse");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(90));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::minute));

        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+state:
  lag_tolerance: "0s"
__additional_properties__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("+state config should parse");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(0));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::minute));

        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+state:
  lag_tolerance: "1s"
__additional_properties__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("+state config should parse");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(1));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::second));

        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+state:
  lag_tolerance: 1 second
__additional_properties__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("+state config should parse");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(1));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::second));
    }

    #[test]
    fn test_project_model_config_state_lag_tolerance_parses_structured_seconds() {
        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+state:
  lag_tolerance:
    count: 1
    period: second
__additional_properties__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("+state config should parse");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(1));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::second));
    }

    #[test]
    fn test_packages_append() {
        use crate::schemas::project::configs::config_merge::Packages;
        use crate::schemas::project::dbt_project::ResolvableConfig;
        use crate::schemas::serde::StringOrArrayOfStrings;

        let parent = ModelConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
                "pandas".to_string(),
            ]))),
            ..Default::default()
        };

        let mut child = ModelConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "matplotlib".to_string(),
            ]))),
            ..Default::default()
        };

        child.default_to(&parent);

        // Should have parent packages first, then child packages (no dedup/sort, matches dbt-core)
        assert_eq!(
            child.packages,
            Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
                "pandas".to_string(),
                "matplotlib".to_string(),
            ])))
        );
    }

    #[test]
    fn test_packages_append_with_string_variant() {
        use crate::schemas::project::configs::config_merge::Packages;
        use crate::schemas::project::dbt_project::ResolvableConfig;
        use crate::schemas::serde::StringOrArrayOfStrings;

        let parent = ModelConfig {
            packages: Packages(Some(StringOrArrayOfStrings::String("numpy".to_string()))),
            ..Default::default()
        };

        let mut child = ModelConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "pandas".to_string(),
            ]))),
            ..Default::default()
        };

        child.default_to(&parent);

        // Should convert String to ArrayOfStrings and merge
        assert_eq!(
            child.packages,
            Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
                "pandas".to_string(),
            ])))
        );
    }

    #[test]
    fn test_packages_none_child_inherits_parent() {
        use crate::schemas::project::configs::config_merge::Packages;
        use crate::schemas::project::dbt_project::ResolvableConfig;
        use crate::schemas::serde::StringOrArrayOfStrings;

        let parent = ModelConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
            ]))),
            ..Default::default()
        };

        let mut child = ModelConfig {
            packages: Packages::default(),
            ..Default::default()
        };

        child.default_to(&parent);

        // Child should inherit parent's packages
        assert_eq!(
            child.packages,
            Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
            ])))
        );
    }

    #[test]
    fn test_packages_no_deduplication() {
        use crate::schemas::project::configs::config_merge::Packages;
        use crate::schemas::project::dbt_project::ResolvableConfig;
        use crate::schemas::serde::StringOrArrayOfStrings;

        let parent = ModelConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
                "pandas".to_string(),
            ]))),
            ..Default::default()
        };

        let mut child = ModelConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
                "matplotlib".to_string(),
            ]))),
            ..Default::default()
        };

        child.default_to(&parent);

        // Should preserve duplicates (no dedup/sort, matches dbt-core behavior)
        assert_eq!(
            child.packages,
            Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
                "pandas".to_string(),
                "numpy".to_string(),
                "matplotlib".to_string(),
            ])))
        );
    }

    #[test]
    fn test_clickhouse_project_config_keys_parse_with_plus_prefix() {
        use crate::schemas::serde::StringOrArrayOfStrings;

        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+engine: ReplacingMergeTree()
+order_by:
  - id
  - ts
+ttl: ts + INTERVAL 30 DAY
+settings:
  index_granularity: 4096
+query_settings:
  join_use_nulls: 1
__additional_properties__: {}
"#,
        )
        .unwrap();

        assert_eq!(config.engine.as_deref(), Some("ReplacingMergeTree()"));
        assert_eq!(
            config.order_by,
            Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "id".to_string(),
                "ts".to_string(),
            ]))
        );
        assert_eq!(config.ttl.as_deref(), Some("ts + INTERVAL 30 DAY"));
        let settings = config.settings.as_ref().expect("+settings should parse");
        assert_eq!(
            settings.get("index_granularity").and_then(|v| v.as_i64()),
            Some(4096)
        );
        let query_settings = config
            .query_settings
            .as_ref()
            .expect("+query_settings should parse");
        assert_eq!(
            query_settings
                .get("join_use_nulls")
                .and_then(|v| v.as_i64()),
            Some(1)
        );
    }

    #[test]
    fn test_clickhouse_config_keys_roundtrip_through_model_config() {
        use crate::schemas::serde::StringOrArrayOfStrings;

        let project_config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+engine: MergeTree()
+order_by: id
+ttl: ts + INTERVAL 1 DAY
+settings:
  allow_nullable_key: 1
+query_settings:
  max_threads: 4
__additional_properties__: {}
"#,
        )
        .unwrap();

        // ProjectModelConfig -> ModelConfig lands the keys in the warehouse config
        let model_config: ModelConfig = project_config.clone().into();
        let wh = &model_config.__warehouse_specific_config__;
        assert_eq!(wh.engine, project_config.engine);
        assert_eq!(
            wh.order_by,
            Some(StringOrArrayOfStrings::String("id".to_string()))
        );
        assert_eq!(wh.ttl, project_config.ttl);
        assert_eq!(wh.settings, project_config.settings);
        assert_eq!(wh.query_settings, project_config.query_settings);

        // ModelConfig -> ProjectModelConfig restores them
        let roundtripped: ProjectModelConfig = model_config.into();
        assert_eq!(roundtripped.engine, project_config.engine);
        assert_eq!(roundtripped.order_by, project_config.order_by);
        assert_eq!(roundtripped.ttl, project_config.ttl);
        assert_eq!(roundtripped.settings, project_config.settings);
        assert_eq!(roundtripped.query_settings, project_config.query_settings);
    }

    #[test]
    fn test_clickhouse_mv_project_config_keys_parse_with_plus_prefix() {
        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+refreshable:
  interval: EVERY 1 MINUTE
+catchup: false
+mv_on_schema_change: append_new_columns
+repopulate_from_mvs_on_full_refresh: true
__additional_properties__: {}
"#,
        )
        .unwrap();

        let Some(RefreshableConfig::Config(refreshable)) = config.refreshable.as_ref() else {
            panic!("+refreshable map should parse as RefreshableConfig::Config");
        };
        assert_eq!(
            refreshable.get("interval").and_then(|v| v.as_str()),
            Some("EVERY 1 MINUTE")
        );

        for (yml, expected) in [
            ("+refreshable: false", RefreshableConfig::Bool(false)),
            ("+refreshable: true", RefreshableConfig::Bool(true)),
        ] {
            let config: ProjectModelConfig =
                dbt_yaml::from_str(&format!("{yml}\n__additional_properties__: {{}}\n")).unwrap();
            assert_eq!(config.refreshable, Some(expected));
            let model_config: ModelConfig = config.into();
            let serialized = dbt_yaml::to_string(&model_config).unwrap();
            assert!(
                serialized.contains(&format!("refreshable: {}", yml.rsplit(' ').next().unwrap())),
                "bool refreshable must serialize as a bare bool for Jinja, got:\n{serialized}"
            );
        }
        assert_eq!(config.catchup, Some(false));
        assert_eq!(
            config.mv_on_schema_change.as_deref(),
            Some("append_new_columns")
        );
        assert_eq!(config.repopulate_from_mvs_on_full_refresh, Some(true));

        // bool_or_string_bool accepts string booleans for the bool-typed keys
        let config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+catchup: "true"
+repopulate_from_mvs_on_full_refresh: "false"
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(config.catchup, Some(true));
        assert_eq!(config.repopulate_from_mvs_on_full_refresh, Some(false));
    }

    #[test]
    fn test_clickhouse_mv_config_keys_roundtrip_through_model_config() {
        let project_config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+refreshable:
  interval: EVERY 10 MINUTE
  randomize: 5 MINUTE
+catchup: true
+mv_on_schema_change: fail
+repopulate_from_mvs_on_full_refresh: false
__additional_properties__: {}
"#,
        )
        .unwrap();

        // ProjectModelConfig -> ModelConfig lands the keys in the warehouse config
        let model_config: ModelConfig = project_config.clone().into();
        let wh = &model_config.__warehouse_specific_config__;
        assert_eq!(wh.refreshable, project_config.refreshable);
        assert_eq!(wh.catchup, Some(true));
        assert_eq!(wh.mv_on_schema_change.as_deref(), Some("fail"));
        assert_eq!(wh.repopulate_from_mvs_on_full_refresh, Some(false));

        // ModelConfig -> ProjectModelConfig restores them
        let roundtripped: ProjectModelConfig = model_config.into();
        assert_eq!(roundtripped.refreshable, project_config.refreshable);
        assert_eq!(roundtripped.catchup, project_config.catchup);
        assert_eq!(
            roundtripped.mv_on_schema_change,
            project_config.mv_on_schema_change
        );
        assert_eq!(
            roundtripped.repopulate_from_mvs_on_full_refresh,
            project_config.repopulate_from_mvs_on_full_refresh
        );
    }

    #[test]
    fn test_clickhouse_view_config_keys_roundtrip_through_model_config() {
        let project_config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+definer: admin@localhost
+sql_security: definer
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(project_config.definer.as_deref(), Some("admin@localhost"));
        assert_eq!(project_config.sql_security.as_deref(), Some("definer"));

        // ProjectModelConfig -> ModelConfig lands the keys in the warehouse config
        let model_config: ModelConfig = project_config.clone().into();
        let wh = &model_config.__warehouse_specific_config__;
        assert_eq!(wh.definer, project_config.definer);
        assert_eq!(wh.sql_security, project_config.sql_security);

        // ModelConfig -> ProjectModelConfig restores them
        let roundtripped: ProjectModelConfig = model_config.into();
        assert_eq!(roundtripped.definer, project_config.definer);
        assert_eq!(roundtripped.sql_security, project_config.sql_security);
    }

    #[test]
    fn test_clickhouse_projections_and_inserts_only_roundtrip_through_model_config() {
        let project_config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+projections:
  - name: my_projection
    query: SELECT id, count() GROUP BY id
+inserts_only: "true"
__additional_properties__: {}
"#,
        )
        .unwrap();
        let projections = project_config
            .projections
            .as_ref()
            .expect("+projections should parse");
        assert_eq!(projections.len(), 1);
        assert_eq!(
            projections[0].get("name").and_then(|v| v.as_str()),
            Some("my_projection")
        );
        // bool_or_string_bool accepts the string form
        assert_eq!(project_config.inserts_only, Some(true));

        // ProjectModelConfig -> ModelConfig lands the keys in the warehouse config
        let model_config: ModelConfig = project_config.clone().into();
        let wh = &model_config.__warehouse_specific_config__;
        assert_eq!(wh.projections, project_config.projections);
        assert_eq!(wh.inserts_only, Some(true));

        // ModelConfig -> ProjectModelConfig restores them
        let roundtripped: ProjectModelConfig = model_config.into();
        assert_eq!(roundtripped.projections, project_config.projections);
        assert_eq!(roundtripped.inserts_only, project_config.inserts_only);
    }

    /// `apply_resolve_defaults` is the only seam that can key a default off the
    /// node's effective adapter: it runs after every config layer has merged and
    /// before `finalize` collapses `materialized` to the static `view` default.
    mod lake_compute_defaults {
        use super::ModelConfig;
        use crate::schemas::common::DbtMaterialization;
        use crate::schemas::project::dbt_project::ResolvableConfig;
        use dbt_adapter_core::AdapterType;
        use dbt_common::io_args::StaticAnalysisKind;

        /// Merged config -> the config `finalize` would see.
        fn resolved(
            mut config: ModelConfig,
            default_adapter: Option<AdapterType>,
        ) -> (Option<DbtMaterialization>, Option<String>) {
            config.apply_resolve_defaults((StaticAnalysisKind::default(), None, default_adapter));
            (config.materialized, config.table_format)
        }

        #[test]
        fn an_unconfigured_lake_compute_model_is_an_iceberg_table() {
            let config = ModelConfig {
                adapter: Some(AdapterType::LakeCompute),
                ..Default::default()
            };
            let (materialized, table_format) = resolved(config, Some(AdapterType::Snowflake));
            assert_eq!(materialized, Some(DbtMaterialization::Table));
            assert_eq!(table_format.as_deref(), Some("iceberg"));
        }

        /// These are defaults, not overrides: whatever the merge produced wins.
        #[test]
        fn an_authored_materialization_and_table_format_survive() {
            let config = ModelConfig {
                adapter: Some(AdapterType::LakeCompute),
                materialized: Some(DbtMaterialization::View),
                table_format: Some("default".to_string()),
                ..Default::default()
            };
            let (materialized, table_format) = resolved(config, Some(AdapterType::Snowflake));
            assert_eq!(materialized, Some(DbtMaterialization::View));
            assert_eq!(table_format.as_deref(), Some("default"));
        }

        /// A target whose *default* adapter is lake compute needs no `+adapter:` on
        /// every node to get the same treatment.
        #[test]
        fn a_lake_compute_target_defaults_its_nodes_too() {
            let (materialized, table_format) =
                resolved(ModelConfig::default(), Some(AdapterType::LakeCompute));
            assert_eq!(materialized, Some(DbtMaterialization::Table));
            assert_eq!(table_format.as_deref(), Some("iceberg"));
        }

        #[test]
        fn nodes_off_lake_compute_are_untouched() {
            let config = ModelConfig {
                adapter: Some(AdapterType::Snowflake),
                ..Default::default()
            };
            let (materialized, table_format) = resolved(config, Some(AdapterType::LakeCompute));
            assert_eq!(materialized, None, "left for the static `view` default");
            assert_eq!(table_format, None);
        }

        /// `None` default adapter (the `Default` resolve-defaults value, used by
        /// resolvers that never call `with_resolve_defaults`) means only an
        /// explicit `+adapter` counts.
        #[test]
        fn an_unknown_default_adapter_does_not_opt_a_node_in() {
            let (materialized, table_format) = resolved(ModelConfig::default(), None);
            assert_eq!(materialized, None);
            assert_eq!(table_format, None);
        }
    }
}
