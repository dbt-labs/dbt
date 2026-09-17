use crate::schemas::common::ClusterConfig;
use crate::schemas::serde::AdapterTypeOrArray;
use crate::schemas::serde::OmissibleGrantConfig;
use crate::schemas::serde::QueryTag;
use dbt_adapter_core::AdapterType;
use dbt_common::io_args::ComputeArg;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_yaml::DbtSchema;
use dbt_yaml::ShouldBe;
use dbt_yaml::Spanned;
use dbt_yaml::Verbatim;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::collections::btree_map::Iter;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use super::config_keys::ConfigKeys;
use crate::schemas::common::DbtMaterialization;
use crate::schemas::common::DbtQuoting;
use crate::schemas::common::DocsConfig;
use crate::schemas::common::HardDeletes;
use crate::schemas::common::Hooks;
use crate::schemas::common::PartitionConfig;
use crate::schemas::common::PersistDocsConfig;
use crate::schemas::common::Schedule;
use crate::schemas::common::SyncConfig;
use crate::schemas::manifest::GrantAccessToTarget;
use crate::schemas::project::ResolvableConfig;
use crate::schemas::project::TypedRecursiveConfig;
use crate::schemas::project::configs::common::{
    WarehouseSpecificNodeConfig, take_databricks_catalog_alias,
};
use crate::schemas::project::configs::config_merge::Tags;
use crate::schemas::properties::ModelState;
use crate::schemas::serde::PartitionsConfig;
use crate::schemas::serde::StringOrArrayOfStrings;
use crate::schemas::serde::bool_or_string_bool;
use crate::schemas::serde::{
    IndexesConfig, PrimaryKeyConfig, StringOrInteger, event_time_or_map_to_string,
    f64_or_string_f64, hours_to_expiration_or_string_omissible, u64_or_string_u64,
};
use dbt_common::serde_utils::Omissible;
use dbt_proc_macros::DefaultTo;
use dbt_proc_macros::Resolvable;

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ProjectSnapshotConfig {
    // Snapshot-specific Configuration
    #[serde(rename = "+database", alias = "+project", alias = "+data_space")]
    pub database: Option<String>,
    #[serde(rename = "+schema", alias = "+dataset")]
    pub schema: Option<String>,
    #[serde(rename = "+alias")]
    pub alias: Option<String>,
    #[serde(rename = "+materialized")]
    pub materialized: Option<DbtMaterialization>,
    #[serde(rename = "+strategy")]
    pub strategy: Option<String>,
    #[serde(rename = "+unique_key")]
    pub unique_key: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+check_cols")]
    pub check_cols: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+updated_at")]
    pub updated_at: Option<String>,
    #[serde(rename = "+dbt_valid_to_current")]
    pub dbt_valid_to_current: Option<String>,
    #[serde(rename = "+snapshot_meta_column_names")]
    pub snapshot_meta_column_names: Option<SnapshotMetaColumnNames>,
    #[serde(rename = "+hard_deletes")]
    pub hard_deletes: Option<HardDeletes>,
    // Legacy snapshot configs (these behave differently than `database` and `schemas`,
    // they're not not just aliases)
    #[serde(rename = "+target_database")]
    pub target_database: Option<String>,
    #[serde(rename = "+target_schema")]
    pub target_schema: Option<String>,
    // General Configuration
    #[serde(default, rename = "+enabled", deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(
        default,
        rename = "+full_refresh",
        deserialize_with = "bool_or_string_bool"
    )]
    pub full_refresh: Option<bool>,
    #[serde(rename = "+tags")]
    pub tags: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+pre-hook", alias = "+pre_hook")]
    pub pre_hook: Verbatim<Option<Hooks>>,
    #[serde(rename = "+post-hook", alias = "+post_hook")]
    pub post_hook: Verbatim<Option<Hooks>>,
    #[serde(rename = "+persist_docs")]
    pub persist_docs: Option<PersistDocsConfig>,
    #[serde(rename = "+grants")]
    pub grants: OmissibleGrantConfig,
    #[serde(
        default,
        rename = "+event_time",
        deserialize_with = "event_time_or_map_to_string"
    )]
    pub event_time: Option<String>,
    #[serde(rename = "+quoting")]
    pub quoting: Option<DbtQuoting>,
    #[serde(rename = "+static_analysis")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    #[serde(rename = "+meta")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+group")]
    pub group: Option<String>,
    #[serde(
        default,
        rename = "+quote_columns",
        deserialize_with = "bool_or_string_bool"
    )]
    pub quote_columns: Option<bool>,
    #[serde(
        default,
        rename = "+invalidate_hard_deletes",
        deserialize_with = "bool_or_string_bool"
    )]
    pub invalidate_hard_deletes: Option<bool>,
    #[serde(rename = "+docs")]
    pub docs: Option<DocsConfig>,
    // Adapter-specific fields (Snowflake)
    #[serde(rename = "+adapter_properties")]
    pub adapter_properties: Option<BTreeMap<String, YmlValue>>,
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
    #[serde(default, rename = "+backup", deserialize_with = "bool_or_string_bool")]
    pub backup: Option<bool>,
    #[serde(rename = "+base_location_root")]
    pub base_location_root: Option<String>,
    #[serde(rename = "+base_location_subpath")]
    pub base_location_subpath: Option<String>,
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
    #[serde(rename = "+external_volume")]
    pub external_volume: Option<String>,
    #[serde(rename = "+initialize")]
    pub initialize: Option<String>,
    #[serde(rename = "+scheduler")]
    pub scheduler: Option<String>,
    #[serde(rename = "+query_tag")]
    pub query_tag: Option<QueryTag>,
    #[serde(rename = "+query_tags")]
    pub query_tags: Option<String>,
    #[serde(rename = "+table_tag")]
    pub table_tag: Option<String>,
    #[serde(rename = "+row_access_policy")]
    pub row_access_policy: Option<String>,
    #[serde(rename = "+refresh_mode")]
    pub refresh_mode: Option<String>,
    #[serde(default, rename = "+secure", deserialize_with = "bool_or_string_bool")]
    pub secure: Option<bool>,
    #[serde(rename = "+snowflake_initialization_warehouse")]
    pub snowflake_initialization_warehouse: Option<String>,
    #[serde(rename = "+immutable_where")]
    pub immutable_where: Option<String>,
    #[serde(rename = "+snowflake_warehouse")]
    pub snowflake_warehouse: Option<String>,
    #[serde(rename = "+refresh_warehouse")]
    pub refresh_warehouse: Option<String>,
    #[serde(rename = "+target_lag")]
    pub target_lag: Option<String>,
    #[serde(rename = "+tmp_relation_type")]
    pub tmp_relation_type: Option<String>,
    #[serde(
        default,
        rename = "+transient",
        deserialize_with = "bool_or_string_bool"
    )]
    pub transient: Option<bool>,
    // Adapter-specific fields (BigQuery)
    #[serde(rename = "+cluster_by")]
    pub cluster_by: Option<ClusterConfig>,
    #[serde(
        default,
        rename = "+enable_refresh",
        deserialize_with = "bool_or_string_bool"
    )]
    pub enable_refresh: Option<bool>,
    #[serde(rename = "+grant_access_to")]
    pub grant_access_to: Option<Vec<GrantAccessToTarget>>,
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
    #[serde(rename = "+max_staleness")]
    pub max_staleness: Option<String>,
    #[serde(rename = "+partition_by")]
    pub partition_by: Option<PartitionConfig>,
    #[serde(
        default,
        rename = "+partition_expiration_days",
        deserialize_with = "u64_or_string_u64"
    )]
    pub partition_expiration_days: Option<u64>,
    #[serde(rename = "+partitions")]
    pub partitions: Option<PartitionsConfig>,
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
    // Adapter-specific fields (Databricks)
    #[serde(
        default,
        rename = "+auto_liquid_cluster",
        deserialize_with = "bool_or_string_bool"
    )]
    pub auto_liquid_cluster: Option<bool>,
    #[serde(rename = "+buckets")]
    pub buckets: Option<i64>,
    #[serde(rename = "+catalog")]
    pub catalog: Option<String>,
    #[serde(rename = "+clustered_by")]
    pub clustered_by: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+compute")]
    pub compute: Option<ComputeArg>,
    #[serde(rename = "+compression")]
    pub compression: Option<String>,
    #[serde(rename = "+databricks_compute")]
    pub databricks_compute: Option<String>,
    #[serde(rename = "+databricks_tags")]
    pub databricks_tags: Option<IndexMap<String, YmlValue>>,
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
    #[serde(
        default,
        rename = "+include_full_name_in_path",
        deserialize_with = "bool_or_string_bool"
    )]
    pub include_full_name_in_path: Option<bool>,
    #[serde(rename = "+liquid_clustered_by")]
    pub liquid_clustered_by: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+location_root")]
    pub location_root: Option<String>,
    #[serde(rename = "+matched_condition")]
    pub matched_condition: Option<String>,
    #[serde(
        default,
        rename = "+merge_with_schema_evolution",
        deserialize_with = "bool_or_string_bool"
    )]
    pub merge_with_schema_evolution: Option<bool>,
    #[serde(rename = "+not_matched_by_source_action")]
    pub not_matched_by_source_action: Option<String>,
    #[serde(rename = "+not_matched_by_source_condition")]
    pub not_matched_by_source_condition: Option<String>,
    #[serde(rename = "+not_matched_condition")]
    pub not_matched_condition: Option<String>,
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
        rename = "+persist_constraints",
        deserialize_with = "bool_or_string_bool"
    )]
    pub persist_constraints: Option<bool>,
    #[serde(
        default,
        rename = "+unique_tmp_table_suffix",
        deserialize_with = "bool_or_string_bool"
    )]
    pub unique_tmp_table_suffix: Option<bool>,
    #[serde(rename = "+source_alias")]
    pub source_alias: Option<String>,
    #[serde(rename = "+target_alias")]
    pub target_alias: Option<String>,
    #[serde(rename = "+tblproperties")]
    pub tblproperties: Option<IndexMap<String, YmlValue>>,
    // Adapter-specific fields (Redshift)
    #[serde(default, rename = "+bind", deserialize_with = "bool_or_string_bool")]
    pub bind: Option<bool>,
    #[serde(rename = "+dist")]
    pub dist: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+sort")]
    pub sort: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+sort_type")]
    pub sort_type: Option<String>,
    // Adapter-specific fields (MSSQL)
    #[serde(
        default,
        rename = "+as_columnstore",
        deserialize_with = "bool_or_string_bool"
    )]
    pub as_columnstore: Option<bool>,
    // Adapter-specific fields (Athena)
    #[serde(default, rename = "+table_type")]
    pub table_type: Option<String>,

    // Adapter-specific fields (Postgres)
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

    /// Schema synchronization configuration
    #[serde(rename = "+sync")]
    pub sync: Option<SyncConfig>,

    // dbt State configs (state-aware run-cache behavior)
    #[serde(rename = "+state")]
    pub state: Option<ModelState>,

    // Flattened field:
    pub __additional_properties__: BTreeMap<String, ShouldBe<ProjectSnapshotConfig>>,
}

impl TypedRecursiveConfig for ProjectSnapshotConfig {
    fn type_name() -> &'static str {
        "snapshot"
    }

    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>> {
        self.__additional_properties__.iter()
    }

    fn has_set_fields(&self) -> bool {
        self.database.is_some()
            || self.schema.is_some()
            || self.alias.is_some()
            || self.materialized.is_some()
            || self.strategy.is_some()
            || self.unique_key.is_some()
            || self.check_cols.is_some()
            || self.updated_at.is_some()
            || self.dbt_valid_to_current.is_some()
            || self.snapshot_meta_column_names.is_some()
            || self.hard_deletes.is_some()
            || self.target_database.is_some()
            || self.target_schema.is_some()
            || self.enabled.is_some()
            || self.full_refresh.is_some()
            || self.tags.is_some()
            || self.pre_hook.is_some()
            || self.post_hook.is_some()
            || self.persist_docs.is_some()
            || self.grants.0.is_present()
            || self.event_time.is_some()
            || self.quoting.is_some()
            || self.static_analysis.is_some()
            || self.meta.is_some()
            || self.group.is_some()
            || self.quote_columns.is_some()
            || self.invalidate_hard_deletes.is_some()
            || self.docs.is_some()
            || self.adapter_properties.is_some()
            || self.automatic_clustering.is_some()
            || self.auto_refresh.is_some()
            || self.backup.is_some()
            || self.base_location_root.is_some()
            || self.base_location_subpath.is_some()
            || self.copy_grants.is_some()
            || self.copy_tags.is_some()
            || self.external_volume.is_some()
            || self.initialize.is_some()
            || self.scheduler.is_some()
            || self.query_tag.is_some()
            || self.table_tag.is_some()
            || self.row_access_policy.is_some()
            || self.refresh_mode.is_some()
            || self.secure.is_some()
            || self.snowflake_initialization_warehouse.is_some()
            || self.immutable_where.is_some()
            || self.snowflake_warehouse.is_some()
            || self.refresh_warehouse.is_some()
            || self.target_lag.is_some()
            || self.tmp_relation_type.is_some()
            || self.transient.is_some()
            || self.cluster_by.is_some()
            || self.enable_refresh.is_some()
            || self.grant_access_to.is_some()
            || self.hours_to_expiration.is_present()
            || self.job_execution_timeout_seconds.is_some()
            || self.reservation.is_some()
            || self.kms_key_name.is_some()
            || self.labels.is_some()
            || self.labels_from_meta.is_some()
            || self.max_staleness.is_some()
            || self.partition_by.is_some()
            || self.partition_expiration_days.is_some()
            || self.partitions.is_some()
            || self.refresh_interval_minutes.is_some()
            || self.resource_tags.is_some()
            || self.require_partition_filter.is_some()
            || self.auto_liquid_cluster.is_some()
            || self.buckets.is_some()
            || self.catalog.is_some()
            || self.clustered_by.is_some()
            || self.compute.is_some()
            || self.compression.is_some()
            || self.databricks_compute.is_some()
            || self.databricks_tags.is_some()
            || self.file_format.is_some()
            || self.catalog_name.is_some()
            || self.adapter.is_some()
            || self.propagate.is_some()
            || self.include_full_name_in_path.is_some()
            || self.liquid_clustered_by.is_some()
            || self.location_root.is_some()
            || self.matched_condition.is_some()
            || self.merge_with_schema_evolution.is_some()
            || self.not_matched_by_source_action.is_some()
            || self.not_matched_by_source_condition.is_some()
            || self.not_matched_condition.is_some()
            || self.skip_matched_step.is_some()
            || self.skip_not_matched_step.is_some()
            || self.persist_constraints.is_some()
            || self.unique_tmp_table_suffix.is_some()
            || self.source_alias.is_some()
            || self.target_alias.is_some()
            || self.tblproperties.is_some()
            || self.bind.is_some()
            || self.dist.is_some()
            || self.sort.is_some()
            || self.sort_type.is_some()
            || self.as_columnstore.is_some()
            || self.table_type.is_some()
            || self.indexes.is_some()
            || self.unlogged.is_some()
            || self.schedule.is_some()
            || self.sync.is_some()
    }
}

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(
    Resolvable, DefaultTo, Deserialize, Serialize, Debug, Clone, DbtSchema, Default, PartialEq,
)]
pub struct SnapshotConfig {
    // Snapshot-specific Configuration
    #[serde(alias = "project", alias = "data_space")]
    pub database: Option<String>,
    #[serde(alias = "dataset")]
    pub schema: Option<String>,
    pub alias: Option<String>,
    #[resolved(promote, default = DbtMaterialization::Snapshot)]
    pub materialized: Option<DbtMaterialization>,
    pub strategy: Option<String>,
    pub unique_key: Option<StringOrArrayOfStrings>,
    pub check_cols: Option<StringOrArrayOfStrings>,
    pub updated_at: Option<String>,
    pub dbt_valid_to_current: Option<String>,
    pub snapshot_meta_column_names: Option<SnapshotMetaColumnNames>,
    pub hard_deletes: Option<HardDeletes>,
    // Legacy snapshot configs (these behave differently than `database` and `schemas`,
    // they're not not just aliases)
    pub target_database: Option<String>,
    pub target_schema: Option<String>,
    pub compute: Option<ComputeArg>,
    // Internal placement hint; kept out of serialized config/telemetry output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    // Internal placement hint; kept out of serialized config/telemetry output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<StringOrArrayOfStrings>")]
    pub propagate: Option<AdapterTypeOrArray>,
    // General Configuration
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub full_refresh: Option<bool>,
    #[serde(default)]
    pub tags: Tags,
    #[serde(alias = "pre-hook")]
    pub pre_hook: Verbatim<Option<Hooks>>,
    #[serde(alias = "post-hook")]
    pub post_hook: Verbatim<Option<Hooks>>,
    pub persist_docs: Option<PersistDocsConfig>,
    #[serde(default)]
    pub grants: OmissibleGrantConfig,
    #[serde(default, deserialize_with = "event_time_or_map_to_string")]
    pub event_time: Option<String>,
    #[resolved(promote, expect = "quoting set by apply_package_defaults")]
    pub quoting: Option<DbtQuoting>,
    #[resolved(promote, expect = "static_analysis set by apply_resolve_defaults")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    pub meta: Option<IndexMap<String, YmlValue>>,
    pub group: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub quote_columns: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub invalidate_hard_deletes: Option<bool>,
    pub docs: Option<DocsConfig>,
    /// Schema synchronization configuration
    pub sync: Option<SyncConfig>,
    // dbt State configs (state-aware run-cache behavior)
    pub state: Option<ModelState>,
    // Adapter specific configs
    pub __warehouse_specific_config__: WarehouseSpecificNodeConfig,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq, Default)]
pub struct SnapshotMetaColumnNames {
    pub dbt_scd_id: Option<String>,
    pub dbt_updated_at: Option<String>,
    pub dbt_valid_from: Option<String>,
    pub dbt_valid_to: Option<String>,
    pub dbt_is_deleted: Option<String>,
}

impl SnapshotMetaColumnNames {
    pub fn new(
        dbt_scd_id: Option<String>,
        dbt_updated_at: Option<String>,
        dbt_valid_from: Option<String>,
        dbt_valid_to: Option<String>,
        dbt_is_deleted: Option<String>,
    ) -> Self {
        Self {
            dbt_scd_id,
            dbt_updated_at,
            dbt_valid_from,
            dbt_valid_to,
            dbt_is_deleted,
        }
    }

    pub fn get_dbt_scd_id(&self, adapter_type: &str) -> String {
        if adapter_type == "snowflake" {
            self.dbt_scd_id
                .clone()
                .unwrap_or_else(|| "DBT_SCD_ID".to_string())
                .to_uppercase()
        } else {
            self.dbt_scd_id
                .clone()
                .unwrap_or_else(|| "dbt_scd_id".to_string())
                .to_lowercase()
        }
    }

    pub fn get_dbt_updated_at(&self, adapter_type: &str) -> String {
        if adapter_type == "snowflake" {
            self.dbt_updated_at
                .clone()
                .unwrap_or_else(|| "DBT_UPDATED_AT".to_string())
                .to_uppercase()
        } else {
            self.dbt_updated_at
                .clone()
                .unwrap_or_else(|| "dbt_updated_at".to_string())
                .to_lowercase()
        }
    }

    pub fn get_dbt_valid_from(&self, adapter_type: &str) -> String {
        if adapter_type == "snowflake" {
            self.dbt_valid_from
                .clone()
                .unwrap_or_else(|| "DBT_VALID_FROM".to_string())
                .to_uppercase()
        } else {
            self.dbt_valid_from
                .clone()
                .unwrap_or_else(|| "dbt_valid_from".to_string())
                .to_lowercase()
        }
    }

    pub fn get_dbt_valid_to(&self, adapter_type: &str) -> String {
        if adapter_type == "snowflake" {
            self.dbt_valid_to
                .clone()
                .unwrap_or_else(|| "DBT_VALID_TO".to_string())
                .to_uppercase()
        } else {
            self.dbt_valid_to
                .clone()
                .unwrap_or_else(|| "dbt_valid_to".to_string())
                .to_lowercase()
        }
    }

    pub fn get_dbt_is_deleted(&self, adapter_type: &str) -> String {
        if adapter_type == "snowflake" {
            self.dbt_is_deleted
                .clone()
                .unwrap_or_else(|| "DBT_IS_DELETED".to_string())
                .to_uppercase()
        } else {
            self.dbt_is_deleted
                .clone()
                .unwrap_or_else(|| "dbt_is_deleted".to_string())
                .to_lowercase()
        }
    }

    pub fn to_defaulted_column_names(&self) -> YmlValue {
        fn insert_name(map: &mut dbt_yaml::Mapping, key: &str, value: &Option<String>) {
            let column_name = match value {
                Some(value) => value.as_str(),
                None => key,
            };
            map.insert(key.into(), YmlValue::string(column_name.to_string()));
        }

        let mut names = dbt_yaml::Mapping::new();
        insert_name(&mut names, "dbt_scd_id", &self.dbt_scd_id);
        insert_name(&mut names, "dbt_updated_at", &self.dbt_updated_at);
        insert_name(&mut names, "dbt_valid_from", &self.dbt_valid_from);
        insert_name(&mut names, "dbt_valid_to", &self.dbt_valid_to);
        insert_name(&mut names, "dbt_is_deleted", &self.dbt_is_deleted);
        YmlValue::mapping(names)
    }
}

impl From<ProjectSnapshotConfig> for SnapshotConfig {
    fn from(config: ProjectSnapshotConfig) -> Self {
        Self {
            database: config.database,
            schema: config.schema,
            alias: config.alias,
            materialized: Some(DbtMaterialization::Snapshot),
            strategy: config.strategy,
            unique_key: config.unique_key,
            check_cols: config.check_cols,
            updated_at: config.updated_at,
            dbt_valid_to_current: config.dbt_valid_to_current,
            snapshot_meta_column_names: config.snapshot_meta_column_names,
            hard_deletes: config.hard_deletes,
            target_database: config.target_database,
            target_schema: config.target_schema,
            compute: config.compute,
            adapter: config.adapter,
            propagate: config.propagate,
            enabled: config.enabled,
            full_refresh: config.full_refresh,
            tags: Tags(config.tags),
            pre_hook: config.pre_hook,
            post_hook: config.post_hook,
            persist_docs: config.persist_docs,
            grants: config.grants,
            event_time: config.event_time,
            quoting: config.quoting,
            static_analysis: config.static_analysis,
            meta: config.meta,
            group: config.group,
            quote_columns: config.quote_columns,
            invalidate_hard_deletes: config.invalidate_hard_deletes,
            docs: config.docs,
            sync: config.sync,
            state: config.state,
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                description: None, // Only for Bigquery models
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
                persist_constraints: config.persist_constraints,
                unique_tmp_table_suffix: config.unique_tmp_table_suffix,
                schedule: config.schedule,
                row_filter: None,
                incremental_apply_config_changes: None,
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

                // snapshot is unsupported for Salesforce yet
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

impl From<SnapshotConfig> for ProjectSnapshotConfig {
    fn from(config: SnapshotConfig) -> Self {
        Self {
            database: config.database,
            schema: config.schema,
            alias: config.alias,
            materialized: config.materialized,
            strategy: config.strategy,
            unique_key: config.unique_key,
            check_cols: config.check_cols,
            updated_at: config.updated_at,
            dbt_valid_to_current: config.dbt_valid_to_current,
            snapshot_meta_column_names: config.snapshot_meta_column_names,
            hard_deletes: config.hard_deletes,
            target_database: config.target_database,
            target_schema: config.target_schema,
            compute: config.compute,
            adapter: config.adapter,
            propagate: config.propagate,
            enabled: config.enabled,
            full_refresh: config.full_refresh,
            tags: config.tags.into_inner(),
            pre_hook: config.pre_hook,
            post_hook: config.post_hook,
            persist_docs: config.persist_docs,
            grants: config.grants,
            event_time: config.event_time,
            quoting: config.quoting,
            static_analysis: config.static_analysis,
            meta: config.meta,
            group: config.group,
            quote_columns: config.quote_columns,
            invalidate_hard_deletes: config.invalidate_hard_deletes,
            docs: config.docs,
            // Snowflake fields
            adapter_properties: config.__warehouse_specific_config__.adapter_properties,
            external_volume: config.__warehouse_specific_config__.external_volume,
            base_location_root: config.__warehouse_specific_config__.base_location_root,
            base_location_subpath: config.__warehouse_specific_config__.base_location_subpath,
            target_lag: config.__warehouse_specific_config__.target_lag,
            snowflake_initialization_warehouse: config
                .__warehouse_specific_config__
                .snowflake_initialization_warehouse,
            immutable_where: config.__warehouse_specific_config__.immutable_where,
            snowflake_warehouse: config.__warehouse_specific_config__.snowflake_warehouse,
            refresh_warehouse: config.__warehouse_specific_config__.refresh_warehouse,
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
            persist_constraints: config.__warehouse_specific_config__.persist_constraints,
            unique_tmp_table_suffix: config.__warehouse_specific_config__.unique_tmp_table_suffix,
            // Redshift fields
            auto_refresh: config.__warehouse_specific_config__.auto_refresh,
            backup: config.__warehouse_specific_config__.backup,
            bind: config.__warehouse_specific_config__.bind,
            dist: config.__warehouse_specific_config__.dist,
            sort: config.__warehouse_specific_config__.sort,
            sort_type: config.__warehouse_specific_config__.sort_type,
            transient: config.__warehouse_specific_config__.transient,
            // MSSQL fields
            as_columnstore: config.__warehouse_specific_config__.as_columnstore,
            // Athena Fields
            table_type: config.__warehouse_specific_config__.table_type,
            // Postgres Fields
            indexes: config.__warehouse_specific_config__.indexes,
            unlogged: config.__warehouse_specific_config__.unlogged,
            // Schedule (Databricks streaming tables)
            schedule: config.__warehouse_specific_config__.schedule,
            sync: config.sync,
            state: config.state,
            __additional_properties__: BTreeMap::new(),
        }
    }
}

impl ResolvableConfig<SnapshotConfig> for SnapshotConfig {
    type Resolved = ResolvedSnapshotConfig;
    type PackageDefaults = DbtQuoting;
    type ResolveDefaults = (StaticAnalysisKind, Option<SyncConfig>);

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
        (static_analysis, sync): (StaticAnalysisKind, Option<SyncConfig>),
    ) {
        if self.static_analysis.is_none() {
            self.static_analysis = Some(Spanned::new(static_analysis));
        }
        if self.sync.is_none() {
            self.sync = sync;
        }
    }

    fn finalize(self) -> ResolvedSnapshotConfig {
        self.finalize_resolved()
    }

    fn default_to(&mut self, parent: &SnapshotConfig) {
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
        //
        // Databricks' `target_catalog` -> `target_database` has no dedicated alias field the
        // way `catalog`/`database` do, so it cannot be canonicalized here; it is handled only
        // where a raw config-key rename is possible, at the inline `{{ config(...) }}` layer
        // (`ParseConfig::apply_config`). A `+target_catalog:` in `dbt_project.yml` or a
        // schema.yml `config:` block remains an unrecognized key -- see the marker test
        // `test_snapshot_target_catalog_in_project_yml_is_unrecognized_key` in
        // `dbt-parser/src/dbt_project_config.rs`.
    }
}

impl ConfigKeys for SnapshotConfig {
    fn valid_field_names() -> HashSet<String> {
        let default_instance = Self::default();
        let serialized = dbt_yaml::to_value(&default_instance)
            .expect("Failed to serialize SnapshotConfig for field extraction");

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
    use super::{AdapterType, ProjectSnapshotConfig, SnapshotConfig};
    use crate::schemas::common::{FreshnessPeriod, UpdatesOn};
    use crate::schemas::properties::{ModelState, StatePreClone};

    #[test]
    fn test_snapshot_query_tags_propagate_through_resolved_config() {
        let project: ProjectSnapshotConfig = dbt_yaml::from_str(
            r#"
+query_tags: '{"team":"snapshot"}'
__additional_properties__: {}
"#,
        )
        .unwrap();

        let resolved: SnapshotConfig = project.into();
        assert_eq!(
            resolved.__warehouse_specific_config__.query_tags.as_deref(),
            Some(r#"{"team":"snapshot"}"#)
        );
    }

    #[test]
    fn test_project_snapshot_config_resource_tags_parses() {
        let config: ProjectSnapshotConfig = dbt_yaml::from_str(
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
            .expect("+resource_tags should parse on ProjectSnapshotConfig");
        assert_eq!(resource_tags.len(), 2);
        assert_eq!(resource_tags["123456789012/dbt-access"], "managed");
        assert_eq!(resource_tags["123456789012/cost-center"], "analytics");
    }

    #[test]
    fn test_project_snapshot_config_resource_tags_propagates_to_snapshot_config() {
        let project_config: ProjectSnapshotConfig = dbt_yaml::from_str(
            r#"
+resource_tags:
  "123456789012/dbt-access": "managed"
__additional_properties__: {}
"#,
        )
        .unwrap();

        let snapshot_config: SnapshotConfig = project_config.into();
        let resource_tags = snapshot_config
            .__warehouse_specific_config__
            .resource_tags
            .expect("resource_tags should propagate from ProjectSnapshotConfig to SnapshotConfig");
        assert_eq!(resource_tags["123456789012/dbt-access"], "managed");
    }

    #[test]
    fn test_snapshot_config_resource_tags_propagates_to_project_snapshot_config() {
        let project_config: ProjectSnapshotConfig = dbt_yaml::from_str(
            r#"
+resource_tags:
  "123456789012/dbt-access": "managed"
__additional_properties__: {}
"#,
        )
        .unwrap();

        let snapshot_config: SnapshotConfig = project_config.into();
        let round_tripped: ProjectSnapshotConfig = snapshot_config.into();
        let resource_tags = round_tripped.resource_tags.expect(
            "resource_tags should propagate from SnapshotConfig back to ProjectSnapshotConfig",
        );
        assert_eq!(resource_tags["123456789012/dbt-access"], "managed");
    }

    #[test]
    fn test_project_snapshot_config_state_parses_with_plus_prefix() {
        let config: ProjectSnapshotConfig = dbt_yaml::from_str(
            r#"
+state:
  lag_tolerance:
    count: 2
    period: hour
  require_fresh_data_from: all
  evaluate_volatile_sql: true
  pre_clone: if_missing
  execute_hooks_on_any_reuse: true
__additional_properties__: {}
"#,
        )
        .unwrap();

        let snapshot_config: SnapshotConfig = config.into();
        let state = snapshot_config
            .state
            .expect("+state should propagate to SnapshotConfig");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(2));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::hour));
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::All));
        assert_eq!(state.evaluate_volatile_sql, Some(true));
        assert_eq!(state.pre_clone, Some(StatePreClone::IfMissing));
        assert_eq!(state.execute_hooks_on_any_reuse, Some(true));
    }

    #[test]
    fn test_snapshot_config_state_parses() {
        let config: SnapshotConfig = dbt_yaml::from_str(
            r#"
state:
  lag_tolerance:
    count: 30
    period: minute
  require_fresh_data_from: any
  evaluate_volatile_sql: false
  pre_clone: always
  execute_hooks_on_any_reuse: false
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();

        let state = config.state.expect("state config should parse");
        let lag_tolerance = state.lag_tolerance.expect("lag_tolerance should parse");
        assert_eq!(lag_tolerance.count, Some(30));
        assert_eq!(lag_tolerance.period, Some(FreshnessPeriod::minute));
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::Any));
        assert_eq!(state.evaluate_volatile_sql, Some(false));
        assert_eq!(state.pre_clone, Some(StatePreClone::Always));
        assert_eq!(state.execute_hooks_on_any_reuse, Some(false));
    }

    #[test]
    fn test_snapshot_config_state_propagates_via_default_to() {
        use crate::schemas::project::dbt_project::ResolvableConfig;

        let parent = SnapshotConfig {
            state: Some(ModelState {
                lag_tolerance: None,
                require_fresh_data_from: Some(UpdatesOn::All),
                evaluate_volatile_sql: Some(true),
                pre_clone: Some(StatePreClone::IfMissing),
                execute_hooks_on_any_reuse: None,
                compare_unrendered_code: None,
            }),
            ..Default::default()
        };
        let mut child = SnapshotConfig::default();
        child.default_to(&parent);

        let state = child
            .state
            .expect("state should propagate from parent to child via default_to");
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::All));
        assert_eq!(state.evaluate_volatile_sql, Some(true));
        assert_eq!(state.pre_clone, Some(StatePreClone::IfMissing));
    }

    /// Regression for #16135: a snapshot that sets one `state:` key keeps the keys
    /// the project layer set.
    #[test]
    fn test_snapshot_config_state_merges_field_by_field() {
        use crate::schemas::project::dbt_project::ResolvableConfig;

        let parent = SnapshotConfig {
            state: Some(ModelState {
                lag_tolerance: None,
                require_fresh_data_from: None,
                evaluate_volatile_sql: Some(true),
                pre_clone: Some(StatePreClone::IfMissing),
                execute_hooks_on_any_reuse: None,
                compare_unrendered_code: None,
            }),
            ..Default::default()
        };
        let mut child = SnapshotConfig {
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
        assert_eq!(state.require_fresh_data_from, Some(UpdatesOn::All));
        assert_eq!(state.evaluate_volatile_sql, Some(true));
        assert_eq!(state.pre_clone, Some(StatePreClone::IfMissing));
    }

    /// `+adapter` names an adapter *type*, so the value is typed rather than a
    /// free string -- anything that is not a supported adapter fails here, at
    /// deserialization. Mirrors the seed and model cases.
    #[test]
    fn test_project_snapshot_config_adapter_parses_and_round_trips() {
        let project_config: ProjectSnapshotConfig = dbt_yaml::from_str(
            r#"
+adapter: bigquery
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(project_config.adapter, Some(AdapterType::Bigquery));

        let config: SnapshotConfig = project_config.into();
        assert_eq!(config.adapter, Some(AdapterType::Bigquery));

        let round_tripped: ProjectSnapshotConfig = config.into();
        assert_eq!(round_tripped.adapter, Some(AdapterType::Bigquery));
    }

    #[test]
    fn test_project_snapshot_config_rejects_a_value_that_is_not_an_adapter() {
        let err = dbt_yaml::from_str::<ProjectSnapshotConfig>(
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
