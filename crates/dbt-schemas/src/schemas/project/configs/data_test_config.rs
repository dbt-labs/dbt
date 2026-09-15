use dbt_adapter_core::AdapterType;
use dbt_common::io_args::ComputeArg;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::serde_utils::Omissible;
use dbt_yaml::{DbtSchema, ShouldBe, Spanned};
use serde::{Deserialize, Serialize};
// Type aliases for clarity
type YmlValue = dbt_yaml::Value;
use indexmap::IndexMap;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::collections::btree_map::Iter;

use super::config_keys::ConfigKeys;
use crate::schemas::common::PartitionConfig;
use crate::schemas::common::{
    ClusterConfig, DbtMaterialization, DbtQuoting, Schedule, Severity, StoreFailuresAs,
};
use crate::schemas::manifest::GrantAccessToTarget;
use crate::schemas::project::configs::common::{
    WarehouseSpecificNodeConfig, take_databricks_catalog_alias,
};
use crate::schemas::project::configs::config_merge::Tags;
use crate::schemas::properties::DataTestState;
use dbt_proc_macros::DefaultTo;
use dbt_proc_macros::Resolvable;

use crate::schemas::project::{ResolvableConfig, TypedRecursiveConfig};
use crate::schemas::serde::{
    IndexesConfig, PartitionsConfig, PrimaryKeyConfig, QueryTag, StringOrArrayOfStrings,
    StringOrInteger, bool_or_string_bool, f64_or_string_f64,
    hours_to_expiration_or_string_omissible, u64_or_string_u64,
};

pub const DEFAULT_DATA_TEST_ERROR_IF: &str = "!= 0";
pub const DEFAULT_DATA_TEST_FAIL_CALC: &str = "count(*)";
pub const DEFAULT_DATA_TEST_SEVERITY: Severity = Severity::Error;
pub const DEFAULT_DATA_TEST_WARN_IF: &str = "!= 0";

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ProjectDataTestConfig {
    #[serde(rename = "+adapter")]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    #[serde(rename = "+alias")]
    pub alias: Option<String>,
    #[serde(rename = "+compute")]
    pub compute: Option<ComputeArg>,
    #[serde(rename = "+database", alias = "+project", alias = "+data_space")]
    pub database: Option<String>,
    #[serde(default, rename = "+enabled", deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(rename = "+error_if")]
    pub error_if: Option<String>,
    #[serde(rename = "+fail_calc")]
    pub fail_calc: Option<String>,
    #[serde(
        default,
        rename = "+full_refresh",
        deserialize_with = "bool_or_string_bool"
    )]
    pub full_refresh: Option<bool>,
    #[serde(rename = "+group")]
    pub group: Option<String>,
    #[serde(rename = "+limit")]
    pub limit: Option<i32>,
    #[serde(rename = "+meta")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+schema", alias = "+dataset")]
    pub schema: Omissible<Option<String>>,
    #[serde(rename = "+severity")]
    pub severity: Option<Severity>,
    #[serde(
        default,
        rename = "+store_failures",
        deserialize_with = "bool_or_string_bool"
    )]
    pub store_failures: Option<bool>,
    #[serde(rename = "+store_failures_as")]
    pub store_failures_as: Option<StoreFailuresAs>,
    #[serde(rename = "+sql_header")]
    pub sql_header: Option<String>,
    #[serde(rename = "+tags")]
    pub tags: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+warn_if")]
    pub warn_if: Option<String>,
    #[serde(rename = "+where")]
    pub where_: Option<String>,
    #[serde(rename = "+quoting")]
    pub quoting: Option<DbtQuoting>,
    #[serde(rename = "+static_analysis")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,

    // Snowflake specific fields
    #[serde(rename = "+adapter_properties")]
    pub adapter_properties: Option<BTreeMap<String, YmlValue>>,
    #[serde(rename = "+external_volume")]
    pub external_volume: Option<String>,
    #[serde(rename = "+base_location_root")]
    pub base_location_root: Option<String>,
    #[serde(rename = "+base_location_subpath")]
    pub base_location_subpath: Option<String>,
    #[serde(rename = "+target_lag")]
    pub target_lag: Option<String>,
    #[serde(rename = "+snowflake_initialization_warehouse")]
    pub snowflake_initialization_warehouse: Option<String>,
    #[serde(rename = "+snowflake_warehouse")]
    pub snowflake_warehouse: Option<String>,
    #[serde(rename = "+refresh_warehouse")]
    pub refresh_warehouse: Option<String>,
    #[serde(rename = "+immutable_where")]
    pub immutable_where: Option<String>,
    #[serde(rename = "+refresh_mode")]
    pub refresh_mode: Option<String>,
    #[serde(rename = "+initialize")]
    pub initialize: Option<String>,
    #[serde(rename = "+scheduler")]
    pub scheduler: Option<String>,
    #[serde(rename = "+tmp_relation_type")]
    pub tmp_relation_type: Option<String>,
    #[serde(rename = "+query_tag")]
    pub query_tag: Option<QueryTag>,
    #[serde(rename = "+query_tags")]
    pub query_tags: Option<String>,
    #[serde(rename = "+table_tag")]
    pub table_tag: Option<String>,
    #[serde(rename = "+row_access_policy")]
    pub row_access_policy: Option<String>,
    #[serde(
        default,
        rename = "+automatic_clustering",
        deserialize_with = "bool_or_string_bool"
    )]
    pub automatic_clustering: Option<bool>,
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
    #[serde(default, rename = "+secure", deserialize_with = "bool_or_string_bool")]
    pub secure: Option<bool>,
    #[serde(
        default,
        rename = "+transient",
        deserialize_with = "bool_or_string_bool"
    )]
    pub transient: Option<bool>,

    // BigQuery specific fields
    #[serde(rename = "+partition_by")]
    pub partition_by: Option<PartitionConfig>,
    #[serde(rename = "+cluster_by")]
    pub cluster_by: Option<ClusterConfig>,
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
    #[serde(rename = "+labels")]
    pub labels: Option<IndexMap<String, String>>,
    #[serde(
        default,
        rename = "+labels_from_meta",
        deserialize_with = "bool_or_string_bool"
    )]
    pub labels_from_meta: Option<bool>,
    #[serde(rename = "+kms_key_name")]
    pub kms_key_name: Option<String>,
    #[serde(
        default,
        rename = "+require_partition_filter",
        deserialize_with = "bool_or_string_bool"
    )]
    pub require_partition_filter: Option<bool>,
    #[serde(
        default,
        rename = "+partition_expiration_days",
        deserialize_with = "u64_or_string_u64"
    )]
    pub partition_expiration_days: Option<u64>,
    #[serde(rename = "+grant_access_to")]
    pub grant_access_to: Option<Vec<GrantAccessToTarget>>,
    #[serde(rename = "+partitions")]
    pub partitions: Option<PartitionsConfig>,
    #[serde(
        default,
        rename = "+enable_refresh",
        deserialize_with = "bool_or_string_bool"
    )]
    pub enable_refresh: Option<bool>,
    #[serde(
        default,
        rename = "+refresh_interval_minutes",
        deserialize_with = "f64_or_string_f64"
    )]
    pub refresh_interval_minutes: Option<f64>,
    #[serde(rename = "+max_staleness")]
    pub max_staleness: Option<String>,

    // Databricks specific fields
    #[serde(rename = "+file_format")]
    pub file_format: Option<String>,
    #[serde(rename = "+catalog_name")]
    pub catalog_name: Option<String>,
    #[serde(rename = "+location_root")]
    pub location_root: Option<String>,
    #[serde(rename = "+tblproperties")]
    pub tblproperties: Option<IndexMap<String, YmlValue>>,
    #[serde(
        default,
        rename = "+include_full_name_in_path",
        deserialize_with = "bool_or_string_bool"
    )]
    pub include_full_name_in_path: Option<bool>,
    #[serde(rename = "+liquid_clustered_by")]
    pub liquid_clustered_by: Option<StringOrArrayOfStrings>,
    #[serde(
        default,
        rename = "+auto_liquid_cluster",
        deserialize_with = "bool_or_string_bool"
    )]
    pub auto_liquid_cluster: Option<bool>,
    #[serde(rename = "+clustered_by")]
    pub clustered_by: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+buckets")]
    pub buckets: Option<i64>,
    #[serde(rename = "+catalog")]
    pub catalog: Option<String>,
    #[serde(rename = "+databricks_tags")]
    pub databricks_tags: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+compression")]
    pub compression: Option<String>,
    #[serde(rename = "+databricks_compute")]
    pub databricks_compute: Option<String>,
    #[serde(rename = "+target_alias")]
    pub target_alias: Option<String>,
    #[serde(rename = "+source_alias")]
    pub source_alias: Option<String>,
    #[serde(rename = "+matched_condition")]
    pub matched_condition: Option<String>,
    #[serde(rename = "+not_matched_condition")]
    pub not_matched_condition: Option<String>,
    #[serde(rename = "+not_matched_by_source_condition")]
    pub not_matched_by_source_condition: Option<String>,
    #[serde(rename = "+not_matched_by_source_action")]
    pub not_matched_by_source_action: Option<String>,
    #[serde(
        default,
        rename = "+merge_with_schema_evolution",
        deserialize_with = "bool_or_string_bool"
    )]
    pub merge_with_schema_evolution: Option<bool>,
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

    // Redshift specific fields
    #[serde(
        default,
        rename = "+auto_refresh",
        deserialize_with = "bool_or_string_bool"
    )]
    pub auto_refresh: Option<bool>,
    #[serde(default, rename = "+backup", deserialize_with = "bool_or_string_bool")]
    pub backup: Option<bool>,
    #[serde(default, rename = "+bind", deserialize_with = "bool_or_string_bool")]
    pub bind: Option<bool>,
    #[serde(rename = "+dist")]
    pub dist: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+sort")]
    pub sort: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+sort_type")]
    pub sort_type: Option<String>,

    // MSSQL specific fields
    #[serde(
        default,
        rename = "+as_columnstore",
        deserialize_with = "bool_or_string_bool"
    )]
    pub as_columnstore: Option<bool>,

    // Athena specific fields
    #[serde(default, rename = "+table_type")]
    pub table_type: Option<String>,

    // Postgres specific fields
    #[serde(default, rename = "+indexes")]
    pub indexes: IndexesConfig,
    #[serde(
        default,
        rename = "+unlogged",
        deserialize_with = "bool_or_string_bool"
    )]
    pub unlogged: Option<bool>,

    // Schedule (Databricks streaming tables)
    #[serde(rename = "+schedule")]
    pub schedule: Option<Schedule>,

    // dbt State configs (restricted subset: only require_fresh_data_from + evaluate_volatile_sql)
    #[serde(rename = "+state")]
    pub state: Option<DataTestState>,

    pub __additional_properties__: BTreeMap<String, ShouldBe<ProjectDataTestConfig>>,
}

impl TypedRecursiveConfig for ProjectDataTestConfig {
    fn type_name() -> &'static str {
        "data_test"
    }

    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>> {
        self.__additional_properties__.iter()
    }

    fn has_set_fields(&self) -> bool {
        self.adapter.is_some()
            || self.alias.is_some()
            || self.compute.is_some()
            || self.database.is_some()
            || self.enabled.is_some()
            || self.error_if.is_some()
            || self.fail_calc.is_some()
            || self.full_refresh.is_some()
            || self.group.is_some()
            || self.limit.is_some()
            || self.meta.is_some()
            || self.schema.is_present()
            || self.severity.is_some()
            || self.store_failures.is_some()
            || self.store_failures_as.is_some()
            || self.sql_header.is_some()
            || self.tags.is_some()
            || self.warn_if.is_some()
            || self.where_.is_some()
            || self.quoting.is_some()
            || self.static_analysis.is_some()
            || self.adapter_properties.is_some()
            || self.external_volume.is_some()
            || self.base_location_root.is_some()
            || self.base_location_subpath.is_some()
            || self.target_lag.is_some()
            || self.snowflake_initialization_warehouse.is_some()
            || self.snowflake_warehouse.is_some()
            || self.refresh_warehouse.is_some()
            || self.immutable_where.is_some()
            || self.refresh_mode.is_some()
            || self.initialize.is_some()
            || self.scheduler.is_some()
            || self.tmp_relation_type.is_some()
            || self.query_tag.is_some()
            || self.table_tag.is_some()
            || self.row_access_policy.is_some()
            || self.automatic_clustering.is_some()
            || self.copy_grants.is_some()
            || self.copy_tags.is_some()
            || self.secure.is_some()
            || self.transient.is_some()
            || self.partition_by.is_some()
            || self.cluster_by.is_some()
            || self.hours_to_expiration.is_present()
            || self.job_execution_timeout_seconds.is_some()
            || self.reservation.is_some()
            || self.labels.is_some()
            || self.labels_from_meta.is_some()
            || self.kms_key_name.is_some()
            || self.require_partition_filter.is_some()
            || self.partition_expiration_days.is_some()
            || self.grant_access_to.is_some()
            || self.partitions.is_some()
            || self.enable_refresh.is_some()
            || self.refresh_interval_minutes.is_some()
            || self.max_staleness.is_some()
            || self.file_format.is_some()
            || self.catalog_name.is_some()
            || self.location_root.is_some()
            || self.tblproperties.is_some()
            || self.include_full_name_in_path.is_some()
            || self.liquid_clustered_by.is_some()
            || self.auto_liquid_cluster.is_some()
            || self.clustered_by.is_some()
            || self.buckets.is_some()
            || self.catalog.is_some()
            || self.databricks_tags.is_some()
            || self.compression.is_some()
            || self.databricks_compute.is_some()
            || self.target_alias.is_some()
            || self.source_alias.is_some()
            || self.matched_condition.is_some()
            || self.not_matched_condition.is_some()
            || self.not_matched_by_source_condition.is_some()
            || self.not_matched_by_source_action.is_some()
            || self.merge_with_schema_evolution.is_some()
            || self.skip_matched_step.is_some()
            || self.skip_not_matched_step.is_some()
            || self.auto_refresh.is_some()
            || self.backup.is_some()
            || self.bind.is_some()
            || self.dist.is_some()
            || self.sort.is_some()
            || self.sort_type.is_some()
            || self.as_columnstore.is_some()
            || self.table_type.is_some()
            || self.indexes.is_some()
            || self.unlogged.is_some()
            || self.schedule.is_some()
    }
}

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(
    Resolvable, DefaultTo, Deserialize, Serialize, Debug, Clone, Default, DbtSchema, PartialEq,
)]
pub struct DataTestConfig {
    // Internal placement hint; kept out of serialized config/telemetry output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    pub alias: Option<String>,
    pub compute: Option<ComputeArg>,
    #[serde(alias = "project", alias = "data_space")]
    pub database: Option<String>,
    #[resolved(or_else = Some(minijinja::constants::DEFAULT_TEST_SCHEMA.to_string()))]
    #[serde(alias = "dataset")]
    pub schema: Omissible<Option<String>>,
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[resolved(promote, default = DEFAULT_DATA_TEST_ERROR_IF.to_string())]
    pub error_if: Option<String>,
    #[resolved(promote, default = DEFAULT_DATA_TEST_FAIL_CALC.to_string())]
    pub fail_calc: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub full_refresh: Option<bool>,
    pub group: Option<String>,
    pub limit: Option<i32>,
    #[serde(serialize_with = "crate::schemas::serde::serialize_none_as_empty_map")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[resolved(promote, default = DEFAULT_DATA_TEST_SEVERITY.clone())]
    pub severity: Option<Severity>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub store_failures: Option<bool>,
    pub store_failures_as: Option<StoreFailuresAs>,
    pub sql_header: Option<String>,
    #[serde(default)]
    pub tags: Tags,
    #[resolved(promote, default = DEFAULT_DATA_TEST_WARN_IF.to_string())]
    pub warn_if: Option<String>,
    #[resolved(promote, expect = "quoting set by apply_package_defaults")]
    pub quoting: Option<DbtQuoting>,
    #[resolved(promote, expect = "static_analysis set by apply_resolve_defaults")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    #[serde(rename = "where")]
    pub where_: Option<String>,
    #[resolved(promote, default = DbtMaterialization::Test)]
    pub materialized: Option<DbtMaterialization>,
    // dbt State configs (restricted subset: only require_fresh_data_from + evaluate_volatile_sql).
    // `skip_serializing_if` avoids emitting `state: null` into data-test manifest nodes (this config
    // is serialized directly into the manifest).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<DataTestState>,
    // Adapter specific configs
    pub __warehouse_specific_config__: WarehouseSpecificNodeConfig,
}

impl From<ProjectDataTestConfig> for DataTestConfig {
    fn from(config: ProjectDataTestConfig) -> Self {
        Self {
            adapter: config.adapter,
            alias: config.alias,
            compute: config.compute,
            database: config.database,
            enabled: config.enabled,
            error_if: config.error_if,
            fail_calc: config.fail_calc,
            full_refresh: config.full_refresh,
            group: config.group,
            limit: config.limit,
            meta: config.meta,
            schema: config.schema,
            severity: config.severity,
            store_failures: config.store_failures,
            store_failures_as: config.store_failures_as,
            sql_header: config.sql_header,
            tags: Tags(config.tags),
            warn_if: config.warn_if,
            quoting: config.quoting,
            where_: config.where_,
            static_analysis: config.static_analysis,
            materialized: Some(DataTestConfig::default_materialized()), // TODO: config.materialized?
            state: config.state,
            // Initialize adapter specific configs with values from flattened fields
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                description: None, // Not applicable for data tests
                adapter_properties: config.adapter_properties,
                external_volume: config.external_volume,
                base_location_root: config.base_location_root,
                base_location_subpath: config.base_location_subpath,
                change_tracking: None,
                data_retention_time_in_days: None,
                max_data_extension_time_in_days: None,
                storage_serialization_policy: None,
                target_file_size: None,
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
                iceberg_version: None,

                partition_by: config.partition_by,

                partition_by_config: None,

                distribute_by_config: None,

                primary_key_config: None,
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
                refresh_interval_minutes: config.refresh_interval_minutes,
                resource_tags: None,
                max_staleness: config.max_staleness,
                jar_file_uri: None,
                timeout: None,
                batch_id: None,
                dataproc_cluster_name: None,
                notebook_template_id: None,
                enable_list_inference: None,
                intermediate_format: None,
                storage_uri: None,

                file_format: config.file_format,
                catalog_name: config.catalog_name,
                location_root: config.location_root,
                use_uniform: None,
                tblproperties: config.tblproperties,
                include_full_name_in_path: config.include_full_name_in_path,
                liquid_clustered_by: config.liquid_clustered_by,
                auto_liquid_cluster: config.auto_liquid_cluster,
                zorder: None,
                skip_optimize: None,
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
                unique_tmp_table_suffix: None,
                schedule: config.schedule,
                row_filter: None,
                incremental_apply_config_changes: None,
                persist_constraints: None,
                use_safer_relation_operations: None,
                view_update_via_alter: None,

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

                // data test is unsupported for Salesforce yet
                primary_key: PrimaryKeyConfig::default(),
                category: None,

                engine: None,
                order_by: None,
                ttl: None,
                settings: None,
                query_settings: None,
                projections: None,
                inserts_only: None,
                connection_overrides: None,
                fields: None,
                source_type: None,
                url: None,
                format: None,
                layout: None,
                lifetime: None,
                range: None,
                table: None,
                update_field: None,
                update_lag: None,
                definer: None,
                sql_security: None,
                refreshable: None,
                catchup: None,
                mv_on_schema_change: None,
                repopulate_from_mvs_on_full_refresh: None,
            },
        }
    }
}

impl From<DataTestConfig> for ProjectDataTestConfig {
    fn from(config: DataTestConfig) -> Self {
        Self {
            adapter: config.adapter,
            alias: config.alias,
            compute: config.compute,
            database: config.database,
            enabled: config.enabled,
            error_if: config.error_if,
            fail_calc: config.fail_calc,
            full_refresh: config.full_refresh,
            group: config.group,
            limit: config.limit,
            meta: config.meta,
            schema: config.schema,
            severity: config.severity,
            store_failures: config.store_failures,
            store_failures_as: config.store_failures_as,
            sql_header: config.sql_header,
            tags: config.tags.into_inner(),
            warn_if: config.warn_if,
            quoting: config.quoting,
            where_: config.where_,
            static_analysis: config.static_analysis,
            state: config.state,
            partition_by: config.__warehouse_specific_config__.partition_by,
            // Snowflake fields
            adapter_properties: config.__warehouse_specific_config__.adapter_properties,
            external_volume: config.__warehouse_specific_config__.external_volume,
            base_location_root: config.__warehouse_specific_config__.base_location_root,
            base_location_subpath: config.__warehouse_specific_config__.base_location_subpath,
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
            copy_grants: config.__warehouse_specific_config__.copy_grants,
            copy_tags: config.__warehouse_specific_config__.copy_tags,
            secure: config.__warehouse_specific_config__.secure,
            transient: config.__warehouse_specific_config__.transient,
            // BigQuery fields
            cluster_by: config.__warehouse_specific_config__.cluster_by,
            hours_to_expiration: config.__warehouse_specific_config__.hours_to_expiration,
            job_execution_timeout_seconds: config
                .__warehouse_specific_config__
                .job_execution_timeout_seconds,
            reservation: config.__warehouse_specific_config__.reservation,
            labels: config.__warehouse_specific_config__.labels,
            labels_from_meta: config.__warehouse_specific_config__.labels_from_meta,
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
            // Databricks fields
            file_format: config.__warehouse_specific_config__.file_format,
            catalog_name: config.__warehouse_specific_config__.catalog_name,
            location_root: config.__warehouse_specific_config__.location_root,
            tblproperties: config.__warehouse_specific_config__.tblproperties,
            include_full_name_in_path: config
                .__warehouse_specific_config__
                .include_full_name_in_path,
            liquid_clustered_by: config.__warehouse_specific_config__.liquid_clustered_by,
            auto_liquid_cluster: config.__warehouse_specific_config__.auto_liquid_cluster,
            clustered_by: config.__warehouse_specific_config__.clustered_by,
            buckets: config.__warehouse_specific_config__.buckets,
            catalog: config.__warehouse_specific_config__.catalog,
            databricks_tags: config.__warehouse_specific_config__.databricks_tags,
            compression: config.__warehouse_specific_config__.compression,
            databricks_compute: config.__warehouse_specific_config__.databricks_compute,
            target_alias: config.__warehouse_specific_config__.target_alias,
            source_alias: config.__warehouse_specific_config__.source_alias,
            matched_condition: config.__warehouse_specific_config__.matched_condition,
            not_matched_condition: config.__warehouse_specific_config__.not_matched_condition,
            not_matched_by_source_condition: config
                .__warehouse_specific_config__
                .not_matched_by_source_condition,
            not_matched_by_source_action: config
                .__warehouse_specific_config__
                .not_matched_by_source_action,
            merge_with_schema_evolution: config
                .__warehouse_specific_config__
                .merge_with_schema_evolution,
            skip_matched_step: config.__warehouse_specific_config__.skip_matched_step,
            skip_not_matched_step: config.__warehouse_specific_config__.skip_not_matched_step,
            // Redshift fields
            auto_refresh: config.__warehouse_specific_config__.auto_refresh,
            backup: config.__warehouse_specific_config__.backup,
            bind: config.__warehouse_specific_config__.bind,
            dist: config.__warehouse_specific_config__.dist,
            sort: config.__warehouse_specific_config__.sort,
            sort_type: config.__warehouse_specific_config__.sort_type,
            // MSSQL fields
            as_columnstore: config.__warehouse_specific_config__.as_columnstore,
            // Athena Fields
            table_type: config.__warehouse_specific_config__.table_type,
            // Postgres Fields
            indexes: config.__warehouse_specific_config__.indexes,
            unlogged: config.__warehouse_specific_config__.unlogged,
            // Schedule (Databricks streaming tables)
            schedule: config.__warehouse_specific_config__.schedule,
            __additional_properties__: BTreeMap::new(),
        }
    }
}

impl ResolvableConfig<DataTestConfig> for DataTestConfig {
    type Resolved = ResolvedDataTestConfig;
    type PackageDefaults = DbtQuoting;
    type ResolveDefaults = (StaticAnalysisKind, bool);

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
        (static_analysis, store_failures): (StaticAnalysisKind, bool),
    ) {
        if self.static_analysis.is_none() {
            self.static_analysis = Some(Spanned::new(static_analysis));
        }
        if store_failures && self.store_failures.is_none() {
            self.store_failures = Some(store_failures);
        }
        // Mirror dbt-core's TestConfig.finalize_and_validate: cross-fill store_failures_as
        // from store_failures so the manifest carries a concrete value matching core.
        // See core/dbt/artifacts/resources/v1/config.py:195-241.
        if self.store_failures_as.is_none() {
            self.store_failures_as = match self.store_failures {
                Some(true) => Some(StoreFailuresAs::Table),
                Some(false) => Some(StoreFailuresAs::Ephemeral),
                None => None,
            };
        } else if self.store_failures.is_none() {
            self.store_failures = Some(matches!(
                self.store_failures_as,
                Some(StoreFailuresAs::Table) | Some(StoreFailuresAs::View)
            ));
        }
    }

    fn finalize(self) -> ResolvedDataTestConfig {
        self.finalize_resolved()
    }

    fn default_to(&mut self, parent: &DataTestConfig) {
        self.default_to_fields(parent);
    }

    fn canonicalize_adapter_aliases(&mut self, default_adapter: AdapterType) {
        if let Some(catalog) = take_databricks_catalog_alias(
            default_adapter,
            &mut self.__warehouse_specific_config__,
            self.database.is_some(),
        ) {
            self.database = Some(catalog);
        }
        // BigQuery's `project`/`dataset` aliases are already routed to `database`/`schema` by
        // the pre-existing, ungated serde `alias`es on those fields (D1); nothing to do here.
    }
}

impl ConfigKeys for DataTestConfig {
    fn valid_field_names() -> HashSet<String> {
        let default_instance = Self::default();
        let serialized = dbt_yaml::to_value(&default_instance)
            .expect("Failed to serialize DataTestConfig for field extraction");

        let mut field_names = HashSet::new();

        if let YmlValue::Mapping(map, _) = serialized {
            for (key, _) in map {
                if let YmlValue::String(key_str, _) = key {
                    field_names.insert(key_str);
                }
            }
        }

        // Add known aliases that might not show up in serialization
        field_names.insert("project".to_string()); // alias for database
        field_names.insert("data_space".to_string()); // alias for database
        field_names.insert("dataset".to_string()); // alias for schema

        field_names
    }
}

#[cfg(test)]
mod tests {
    use super::{AdapterType, DataTestConfig, ProjectDataTestConfig};
    use crate::schemas::common::UpdatesOn;

    #[test]
    fn test_data_test_query_tags_propagate_through_resolved_config() {
        let project: ProjectDataTestConfig = dbt_yaml::from_str(
            r#"
+query_tags: '{"team":"data-test"}'
__additional_properties__: {}
"#,
        )
        .unwrap();

        let resolved: DataTestConfig = project.into();
        assert_eq!(
            resolved.__warehouse_specific_config__.query_tags.as_deref(),
            Some(r#"{"team":"data-test"}"#)
        );
    }

    #[test]
    fn test_project_data_test_config_state_parses_with_plus_prefix() {
        let config: ProjectDataTestConfig = dbt_yaml::from_str(
            r#"
+state:
  require_fresh_data_from: all
  evaluate_volatile_sql: true
__additional_properties__: {}
"#,
        )
        .unwrap();

        let data_test_config: DataTestConfig = config.into();
        let state = data_test_config
            .state
            .expect("+state should propagate to DataTestConfig");
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::All));
        assert_eq!(state.evaluate_volatile_sql, Some(true));
    }

    #[test]
    fn test_data_test_config_state_parses() {
        let config: DataTestConfig = dbt_yaml::from_str(
            r#"
state:
  require_fresh_data_from: any
  evaluate_volatile_sql: false
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("state config should parse");
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::Any));
        assert_eq!(state.evaluate_volatile_sql, Some(false));
    }

    #[test]
    fn test_data_test_config_state_propagates_via_default_to() {
        use crate::schemas::project::dbt_project::ResolvableConfig;
        use crate::schemas::properties::DataTestState;

        let parent = DataTestConfig {
            state: Some(DataTestState {
                require_fresh_data_from: Some(UpdatesOn::All),
                evaluate_volatile_sql: Some(true),
                compare_unrendered_code: None,
            }),
            ..Default::default()
        };
        let mut child = DataTestConfig::default();
        child.default_to(&parent);

        let state = child
            .state
            .expect("state should propagate from parent to child via default_to");
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::All));
        assert_eq!(state.evaluate_volatile_sql, Some(true));
    }

    /// Regression for #16135: a data test that sets one `state:` key keeps the keys
    /// the project layer set.
    #[test]
    fn test_data_test_config_state_merges_field_by_field() {
        use crate::schemas::project::dbt_project::ResolvableConfig;
        use crate::schemas::properties::DataTestState;

        let parent = DataTestConfig {
            state: Some(DataTestState {
                require_fresh_data_from: None,
                evaluate_volatile_sql: Some(true),
                compare_unrendered_code: Some(true),
            }),
            ..Default::default()
        };
        let mut child = DataTestConfig {
            state: Some(DataTestState {
                require_fresh_data_from: Some(UpdatesOn::All),
                evaluate_volatile_sql: None,
                compare_unrendered_code: None,
            }),
            ..Default::default()
        };
        child.default_to(&parent);

        let state = child.state.expect("state should survive the merge");
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::All));
        assert_eq!(state.evaluate_volatile_sql, Some(true));
        assert_eq!(state.compare_unrendered_code, Some(true));
    }

    #[test]
    fn test_data_test_state_type_models_only_two_keys() {
        use crate::schemas::properties::DataTestState;

        // Verifies the `DataTestState` type shape: only `require_fresh_data_from` and
        // `evaluate_volatile_sql` bind; any other key is silently ignored by serde. This does not
        // test user-facing rejection of unsupported keys.
        let state: DataTestState = dbt_yaml::from_str(
            r#"
require_fresh_data_from: all
evaluate_volatile_sql: true
lag_tolerance:
  count: 1
  period: day
pre_clone: if_missing
execute_hooks_on_any_reuse: true
"#,
        )
        .unwrap();

        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::All));
        assert_eq!(state.evaluate_volatile_sql, Some(true));
    }

    /// `+adapter` names an adapter *type*, so the value is typed rather than a
    /// free string -- anything that is not a supported adapter fails here, at
    /// deserialization. Mirrors the seed and model cases.
    #[test]
    fn test_project_data_test_config_adapter_parses_and_round_trips() {
        let project_config: ProjectDataTestConfig = dbt_yaml::from_str(
            r#"
+adapter: bigquery
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(project_config.adapter, Some(AdapterType::Bigquery));

        let config: DataTestConfig = project_config.into();
        assert_eq!(config.adapter, Some(AdapterType::Bigquery));

        let round_tripped: ProjectDataTestConfig = config.into();
        assert_eq!(round_tripped.adapter, Some(AdapterType::Bigquery));
    }

    #[test]
    fn test_project_data_test_config_rejects_a_value_that_is_not_an_adapter() {
        let err = dbt_yaml::from_str::<ProjectDataTestConfig>(
            r#"
+adapter: compute
__additional_properties__: {}
"#,
        )
        .expect_err("`compute` is not an adapter type");
        assert!(
            format!("{err}").contains("compute"),
            "error should name the offending value: {err}"
        );
    }
}
