use crate::schemas::common::ClusterConfig;
use crate::schemas::serde::AdapterTypeOrArray;
use crate::schemas::serde::OmissibleGrantConfig;
use crate::schemas::serde::QueryTag;
use dbt_adapter_core::AdapterType;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_proc_macros::Resolvable;
use dbt_yaml::DbtSchema;
use dbt_yaml::ShouldBe;
use dbt_yaml::Spanned;
use dbt_yaml::Verbatim;
use serde::{Deserialize, Serialize};
// Type aliases for clarity
type YmlValue = dbt_yaml::Value;
use indexmap::IndexMap;
use serde_with::skip_serializing_none;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::collections::btree_map::Iter;

use super::config_keys::ConfigKeys;
use crate::schemas::common::DbtMaterialization;
use crate::schemas::common::DbtQuoting;
use crate::schemas::common::DocsConfig;
use crate::schemas::common::Hooks;
use crate::schemas::common::PartitionConfig;
use crate::schemas::common::PersistDocsConfig;
use crate::schemas::common::Schedule;
use crate::schemas::manifest::GrantAccessToTarget;
use crate::schemas::project::ResolvableConfig;
use crate::schemas::project::TypedRecursiveConfig;
use crate::schemas::project::configs::common::{
    WarehouseSpecificNodeConfig, take_databricks_catalog_alias,
};
use crate::schemas::project::configs::config_merge::Tags;
use crate::schemas::serde::PartitionsConfig;
use crate::schemas::serde::StringOrArrayOfStrings;
use crate::schemas::serde::bool_or_string_bool;
use crate::schemas::serde::{
    IndexesConfig, PrimaryKeyConfig, StringOrInteger, column_types_map,
    event_time_or_map_to_string, f64_or_string_f64, hours_to_expiration_or_string_omissible,
    u64_or_string_u64,
};
use dbt_common::serde_utils::Omissible;
use dbt_proc_macros::DefaultTo;

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ProjectSeedConfig {
    #[serde(
        default,
        rename = "+column_types",
        deserialize_with = "column_types_map"
    )]
    pub column_types: Option<BTreeMap<Spanned<String>, String>>,
    #[serde(rename = "+copy_grants")]
    pub copy_grants: Option<bool>,
    #[serde(rename = "+copy_tags")]
    pub copy_tags: Option<bool>,
    #[serde(rename = "+database", alias = "+project", alias = "+data_space")]
    pub database: Option<String>,
    #[serde(rename = "+alias")]
    pub alias: Option<String>,
    #[serde(rename = "+docs")]
    pub docs: Option<DocsConfig>,
    #[serde(default, rename = "+enabled", deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(
        default,
        rename = "+event_time",
        deserialize_with = "event_time_or_map_to_string"
    )]
    pub event_time: Option<String>,
    #[serde(rename = "+full_refresh")]
    pub full_refresh: Option<bool>,
    #[serde(rename = "+grants")]
    pub grants: OmissibleGrantConfig,
    #[serde(rename = "+group")]
    pub group: Option<String>,
    #[serde(rename = "+meta")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+persist_docs")]
    pub persist_docs: Option<PersistDocsConfig>,
    #[serde(rename = "+post-hook", alias = "+post_hook")]
    pub post_hook: Verbatim<Option<Hooks>>,
    #[serde(rename = "+pre-hook", alias = "+pre_hook")]
    pub pre_hook: Verbatim<Option<Hooks>>,
    #[serde(
        default,
        rename = "+quote_columns",
        deserialize_with = "bool_or_string_bool"
    )]
    pub quote_columns: Option<bool>,
    #[serde(rename = "+schema", alias = "+dataset")]
    pub schema: Option<String>,
    #[serde(rename = "+snowflake_initialization_warehouse")]
    pub snowflake_initialization_warehouse: Option<String>,
    #[serde(rename = "+immutable_where")]
    pub immutable_where: Option<String>,
    #[serde(rename = "+snowflake_warehouse")]
    pub snowflake_warehouse: Option<String>,
    #[serde(rename = "+refresh_warehouse")]
    pub refresh_warehouse: Option<String>,
    #[serde(rename = "+static_analysis")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    #[serde(rename = "+tags")]
    pub tags: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+transient")]
    pub transient: Option<bool>,
    #[serde(rename = "+quoting")]
    pub quoting: Option<DbtQuoting>,
    #[serde(rename = "+delimiter")]
    pub delimiter: Option<Spanned<String>>,
    #[serde(rename = "+external_volume")]
    pub external_volume: Option<String>,
    #[serde(rename = "+adapter_properties")]
    pub adapter_properties: Option<BTreeMap<String, YmlValue>>,
    #[serde(rename = "+base_location_root")]
    pub base_location_root: Option<String>,
    #[serde(rename = "+base_location_subpath")]
    pub base_location_subpath: Option<String>,
    #[serde(rename = "+target_lag")]
    pub target_lag: Option<String>,
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
    #[serde(default, rename = "+secure", deserialize_with = "bool_or_string_bool")]
    pub secure: Option<bool>,
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
    #[serde(rename = "+resource_tags")]
    pub resource_tags: Option<IndexMap<String, String>>,
    #[serde(rename = "+max_staleness")]
    pub max_staleness: Option<String>,
    #[serde(rename = "+file_format")]
    pub file_format: Option<String>,
    #[serde(rename = "+catalog_name")]
    pub catalog_name: Option<String>,
    #[serde(rename = "+adapter")]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    #[serde(rename = "+propagate")]
    #[schemars(with = "Option<StringOrArrayOfStrings>")]
    pub propagate: Option<AdapterTypeOrArray>,
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

    pub __additional_properties__: BTreeMap<String, ShouldBe<ProjectSeedConfig>>,
}

impl TypedRecursiveConfig for ProjectSeedConfig {
    fn type_name() -> &'static str {
        "seed"
    }

    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>> {
        self.__additional_properties__.iter()
    }

    fn has_set_fields(&self) -> bool {
        self.column_types.is_some()
            || self.copy_grants.is_some()
            || self.copy_tags.is_some()
            || self.database.is_some()
            || self.alias.is_some()
            || self.docs.is_some()
            || self.enabled.is_some()
            || self.event_time.is_some()
            || self.full_refresh.is_some()
            || self.grants.0.is_present()
            || self.group.is_some()
            || self.meta.is_some()
            || self.persist_docs.is_some()
            || self.post_hook.is_some()
            || self.pre_hook.is_some()
            || self.quote_columns.is_some()
            || self.schema.is_some()
            || self.snowflake_initialization_warehouse.is_some()
            || self.immutable_where.is_some()
            || self.snowflake_warehouse.is_some()
            || self.refresh_warehouse.is_some()
            || self.static_analysis.is_some()
            || self.tags.is_some()
            || self.transient.is_some()
            || self.quoting.is_some()
            || self.delimiter.is_some()
            || self.external_volume.is_some()
            || self.adapter_properties.is_some()
            || self.base_location_root.is_some()
            || self.base_location_subpath.is_some()
            || self.target_lag.is_some()
            || self.refresh_mode.is_some()
            || self.initialize.is_some()
            || self.scheduler.is_some()
            || self.tmp_relation_type.is_some()
            || self.query_tag.is_some()
            || self.table_tag.is_some()
            || self.row_access_policy.is_some()
            || self.automatic_clustering.is_some()
            || self.secure.is_some()
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
            || self.resource_tags.is_some()
            || self.max_staleness.is_some()
            || self.file_format.is_some()
            || self.catalog_name.is_some()
            || self.adapter.is_some()
            || self.propagate.is_some()
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
    Resolvable, DefaultTo, Deserialize, Serialize, Debug, Default, PartialEq, Clone, DbtSchema,
)]
pub struct SeedConfig {
    #[serde(default, deserialize_with = "column_types_map")]
    pub column_types: Option<BTreeMap<Spanned<String>, String>>,
    #[serde(alias = "project", alias = "data_space")]
    pub database: Option<String>,
    #[serde(alias = "dataset")]
    pub schema: Option<String>,
    pub alias: Option<String>,
    pub catalog_name: Option<String>,
    // Internal placement hint; kept out of serialized config/telemetry output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    // Internal placement hint; kept out of serialized config/telemetry output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<StringOrArrayOfStrings>")]
    pub propagate: Option<AdapterTypeOrArray>,
    pub docs: Option<DocsConfig>,
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub grants: OmissibleGrantConfig,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub quote_columns: Option<bool>,
    pub delimiter: Option<Spanned<String>>,
    #[serde(default, deserialize_with = "event_time_or_map_to_string")]
    pub event_time: Option<String>,
    pub full_refresh: Option<bool>,
    pub group: Option<String>,
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[resolved(promote, expect = "static_analysis set by apply_resolve_defaults")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    pub persist_docs: Option<PersistDocsConfig>,
    #[serde(alias = "post-hook")]
    pub post_hook: Verbatim<Option<Hooks>>,
    #[serde(alias = "pre-hook")]
    pub pre_hook: Verbatim<Option<Hooks>>,
    #[serde(default)]
    pub tags: Tags,
    #[resolved(promote, expect = "quoting set by apply_package_defaults")]
    pub quoting: Option<DbtQuoting>,
    pub materialized: Option<DbtMaterialization>,
    // Adapter specific configs
    pub __warehouse_specific_config__: WarehouseSpecificNodeConfig,
}

impl From<ProjectSeedConfig> for SeedConfig {
    fn from(config: ProjectSeedConfig) -> Self {
        Self {
            column_types: config.column_types,
            database: config.database,
            schema: config.schema,
            alias: config.alias,
            catalog_name: config.catalog_name.clone(),
            adapter: config.adapter,
            propagate: config.propagate,
            docs: config.docs,
            enabled: config.enabled,
            grants: config.grants,
            quote_columns: config.quote_columns,
            delimiter: config.delimiter,
            event_time: config.event_time,
            full_refresh: config.full_refresh,
            group: config.group,
            meta: config.meta,
            static_analysis: config.static_analysis,
            persist_docs: config.persist_docs,
            post_hook: config.post_hook,
            pre_hook: config.pre_hook,
            tags: Tags(config.tags),
            quoting: config.quoting,
            materialized: Some(DbtMaterialization::Seed),
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                description: None, // Only for BigQuery models
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
                immutable_where: config.immutable_where,
                snowflake_warehouse: config.snowflake_warehouse,
                refresh_warehouse: config.refresh_warehouse,
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
                resource_tags: config.resource_tags,
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

                // seed is unsupported for Salesforce yet
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

impl From<SeedConfig> for ProjectSeedConfig {
    fn from(config: SeedConfig) -> Self {
        Self {
            column_types: config.column_types,
            database: config.database,
            schema: config.schema,
            alias: config.alias,
            adapter: config.adapter,
            propagate: config.propagate,
            docs: config.docs,
            enabled: config.enabled,
            grants: config.grants,
            quote_columns: config.quote_columns,
            delimiter: config.delimiter,
            event_time: config.event_time,
            full_refresh: config.full_refresh,
            group: config.group,
            meta: config.meta,
            persist_docs: config.persist_docs,
            post_hook: config.post_hook,
            pre_hook: config.pre_hook,
            static_analysis: config.static_analysis,
            tags: config.tags.into_inner(),
            quoting: config.quoting,
            // Snowflake fields
            adapter_properties: config.__warehouse_specific_config__.adapter_properties,
            snowflake_initialization_warehouse: config
                .__warehouse_specific_config__
                .snowflake_initialization_warehouse,
            immutable_where: config.__warehouse_specific_config__.immutable_where,
            snowflake_warehouse: config.__warehouse_specific_config__.snowflake_warehouse,
            refresh_warehouse: config.__warehouse_specific_config__.refresh_warehouse,
            transient: config.__warehouse_specific_config__.transient,
            copy_grants: config.__warehouse_specific_config__.copy_grants,
            copy_tags: config.__warehouse_specific_config__.copy_tags,
            external_volume: config.__warehouse_specific_config__.external_volume,
            base_location_root: config.__warehouse_specific_config__.base_location_root,
            base_location_subpath: config.__warehouse_specific_config__.base_location_subpath,
            target_lag: config.__warehouse_specific_config__.target_lag,
            refresh_mode: config.__warehouse_specific_config__.refresh_mode,
            initialize: config.__warehouse_specific_config__.initialize,
            scheduler: config.__warehouse_specific_config__.scheduler,
            tmp_relation_type: config.__warehouse_specific_config__.tmp_relation_type,
            query_tag: config.__warehouse_specific_config__.query_tag,
            query_tags: config.__warehouse_specific_config__.query_tags,
            table_tag: config.__warehouse_specific_config__.table_tag,
            row_access_policy: config.__warehouse_specific_config__.row_access_policy,
            automatic_clustering: config.__warehouse_specific_config__.automatic_clustering,
            secure: config.__warehouse_specific_config__.secure,
            // BigQuery fields
            partition_by: config.__warehouse_specific_config__.partition_by,
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
            resource_tags: config.__warehouse_specific_config__.resource_tags,
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
            as_columnstore: config.__warehouse_specific_config__.as_columnstore,

            table_type: config.__warehouse_specific_config__.table_type,
            indexes: config.__warehouse_specific_config__.indexes,
            unlogged: config.__warehouse_specific_config__.unlogged,
            schedule: config.__warehouse_specific_config__.schedule,
            __additional_properties__: BTreeMap::new(),
        }
    }
}

impl ResolvableConfig<SeedConfig> for SeedConfig {
    type Resolved = ResolvedSeedConfig;
    type PackageDefaults = DbtQuoting;
    type ResolveDefaults = StaticAnalysisKind;

    fn get_enabled_with_default(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn disable(&mut self) {
        self.enabled = Some(false);
    }

    fn apply_package_defaults(&mut self, quoting: DbtQuoting) {
        if self.quoting.is_none() {
            self.quoting = Some(quoting);
        }
    }

    fn apply_resolve_defaults(&mut self, static_analysis: StaticAnalysisKind) {
        if self.static_analysis.is_none() {
            self.static_analysis = Some(Spanned::new(static_analysis));
        }
    }

    fn finalize(self) -> ResolvedSeedConfig {
        self.finalize_resolved()
    }

    fn default_to(&mut self, parent: &SeedConfig) {
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

impl ConfigKeys for SeedConfig {
    fn valid_field_names() -> HashSet<String> {
        let default_instance = Self::default();
        let serialized = dbt_yaml::to_value(&default_instance)
            .expect("Failed to serialize SeedConfig for field extraction");

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
        field_names.insert("post-hook".to_string()); // might be serialized as post_hook
        field_names.insert("pre-hook".to_string()); // might be serialized as pre_hook

        field_names
    }
}

#[cfg(test)]
mod tests {
    use super::{ProjectSeedConfig, SeedConfig};
    use dbt_adapter_core::AdapterType;

    #[test]
    fn test_seed_query_tags_propagate_through_resolved_config() {
        let project: ProjectSeedConfig = dbt_yaml::from_str(
            r#"
+query_tags: '{"team":"seed"}'
__additional_properties__: {}
"#,
        )
        .unwrap();

        let resolved: SeedConfig = project.into();
        assert_eq!(
            resolved.__warehouse_specific_config__.query_tags.as_deref(),
            Some(r#"{"team":"seed"}"#)
        );
    }

    #[test]
    fn test_project_seed_config_resource_tags_parses() {
        let config: ProjectSeedConfig = dbt_yaml::from_str(
            r#"
+resource_tags:
  "123456789012/dbt-access": "managed"
  "123456789012/cost-center": "analytics"
__additional_properties__: {}
"#,
        )
        .unwrap();

        let resource_tags = config
            .resource_tags
            .expect("+resource_tags should parse on ProjectSeedConfig");
        assert_eq!(resource_tags.len(), 2);
        assert_eq!(resource_tags["123456789012/dbt-access"], "managed");
        assert_eq!(resource_tags["123456789012/cost-center"], "analytics");
    }

    #[test]
    fn test_project_seed_config_resource_tags_propagates_to_seed_config() {
        let project_config: ProjectSeedConfig = dbt_yaml::from_str(
            r#"
+resource_tags:
  "123456789012/dbt-access": "managed"
__additional_properties__: {}
"#,
        )
        .unwrap();

        let seed_config: SeedConfig = project_config.into();
        let resource_tags = seed_config
            .__warehouse_specific_config__
            .resource_tags
            .expect("resource_tags should propagate from ProjectSeedConfig to SeedConfig");
        assert_eq!(resource_tags["123456789012/dbt-access"], "managed");
    }

    #[test]
    fn test_seed_config_resource_tags_propagates_to_project_seed_config() {
        let project_config: ProjectSeedConfig = dbt_yaml::from_str(
            r#"
+resource_tags:
  "123456789012/dbt-access": "managed"
__additional_properties__: {}
"#,
        )
        .unwrap();

        let seed_config: SeedConfig = project_config.into();
        let round_tripped: ProjectSeedConfig = seed_config.into();
        let resource_tags = round_tripped
            .resource_tags
            .expect("resource_tags should propagate from SeedConfig back to ProjectSeedConfig");
        assert_eq!(resource_tags["123456789012/dbt-access"], "managed");
    }

    /// `+adapter` names an adapter *type*, so the value is typed rather than a
    /// free string -- anything that is not a supported adapter fails here, at
    /// deserialization.
    #[test]
    fn test_project_seed_config_adapter_parses_and_round_trips() {
        let project_config: ProjectSeedConfig = dbt_yaml::from_str(
            r#"
+adapter: snowflake
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(project_config.adapter, Some(AdapterType::Snowflake));

        let seed_config: SeedConfig = project_config.into();
        assert_eq!(seed_config.adapter, Some(AdapterType::Snowflake));

        let round_tripped: ProjectSeedConfig = seed_config.into();
        assert_eq!(round_tripped.adapter, Some(AdapterType::Snowflake));
    }

    #[test]
    fn test_project_seed_config_rejects_a_value_that_is_not_an_adapter() {
        let err = dbt_yaml::from_str::<ProjectSeedConfig>(
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
