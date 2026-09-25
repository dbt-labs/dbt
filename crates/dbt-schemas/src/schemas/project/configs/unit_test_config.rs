use dbt_adapter_core::AdapterType;
use dbt_common::io_args::ComputeArg;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::serde_utils::Omissible;
use dbt_proc_macros::Resolvable;
use dbt_yaml::{DbtSchema, ShouldBe, Spanned};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
// Type aliases for clarity
type YmlValue = dbt_yaml::Value;
use serde_with::skip_serializing_none;
use std::collections::{BTreeMap, btree_map::Iter};

use crate::schemas::{
    common::{ClusterConfig, PartitionConfig, Schedule},
    manifest::GrantAccessToTarget,
    project::{
        ResolvableConfig, TypedRecursiveConfig,
        configs::{
            common::WarehouseSpecificNodeConfig, config_keys::ConfigKeys, config_merge::Tags,
        },
    },
    serde::{
        IndexesConfig, PartitionsConfig, PrimaryKeyConfig, QueryTag, StringOrArrayOfStrings,
        StringOrInteger, bool_or_string_bool, f64_or_string_f64,
        hours_to_expiration_or_string_omissible, u64_or_string_u64,
    },
};
use dbt_proc_macros::DefaultTo;

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ProjectUnitTestConfig {
    #[serde(default, rename = "+enabled", deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(rename = "+compute")]
    pub compute: Option<ComputeArg>,
    #[serde(rename = "+meta")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+tags")]
    pub tags: Option<StringOrArrayOfStrings>,
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
    #[serde(rename = "+immutable_where")]
    pub immutable_where: Option<String>,
    #[serde(rename = "+snowflake_warehouse")]
    pub snowflake_warehouse: Option<String>,
    #[serde(rename = "+refresh_warehouse")]
    pub refresh_warehouse: Option<String>,
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
    #[serde(
        default,
        rename = "+unique_tmp_table_suffix",
        deserialize_with = "bool_or_string_bool"
    )]
    pub unique_tmp_table_suffix: Option<bool>,
    // Schedule (Databricks streaming tables)
    #[serde(rename = "+schedule")]
    pub schedule: Option<Schedule>,

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

    // Flattened fields
    pub __additional_properties__: BTreeMap<String, ShouldBe<ProjectUnitTestConfig>>,
}

impl TypedRecursiveConfig for ProjectUnitTestConfig {
    fn type_name() -> &'static str {
        "unit_test"
    }

    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>> {
        self.__additional_properties__.iter()
    }

    fn has_set_fields(&self) -> bool {
        self.enabled.is_some()
            || self.compute.is_some()
            || self.meta.is_some()
            || self.tags.is_some()
            || self.static_analysis.is_some()
            || self.adapter_properties.is_some()
            || self.external_volume.is_some()
            || self.base_location_root.is_some()
            || self.base_location_subpath.is_some()
            || self.target_lag.is_some()
            || self.snowflake_initialization_warehouse.is_some()
            || self.immutable_where.is_some()
            || self.snowflake_warehouse.is_some()
            || self.refresh_warehouse.is_some()
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
            || self.unique_tmp_table_suffix.is_some()
            || self.schedule.is_some()
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
    }
}

#[skip_serializing_none]
#[derive(
    Resolvable, DefaultTo, Deserialize, Serialize, Debug, Clone, Default, PartialEq, DbtSchema,
)]
pub struct UnitTestConfig {
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    pub compute: Option<ComputeArg>,
    #[resolved(promote, expect = "static_analysis set by apply_resolve_defaults")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(default)]
    pub tags: Tags,
    // Adapter specific configs
    pub __warehouse_specific_config__: WarehouseSpecificNodeConfig,
}

impl From<ProjectUnitTestConfig> for UnitTestConfig {
    fn from(config: ProjectUnitTestConfig) -> Self {
        Self {
            enabled: config.enabled,
            compute: config.compute,
            static_analysis: config.static_analysis,
            meta: config.meta,
            tags: Tags(config.tags),
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                description: None, // Only for Bigquery Models
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
                unique_tmp_table_suffix: config.unique_tmp_table_suffix,
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

                // unit test is unsupported for Salesforce yet
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

impl From<UnitTestConfig> for ProjectUnitTestConfig {
    fn from(config: UnitTestConfig) -> Self {
        Self {
            enabled: config.enabled,
            compute: config.compute,
            static_analysis: config.static_analysis,
            meta: config.meta,
            tags: config.tags.into_inner(),
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
            transient: config.__warehouse_specific_config__.transient,
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
            unique_tmp_table_suffix: config.__warehouse_specific_config__.unique_tmp_table_suffix,
            schedule: config.__warehouse_specific_config__.schedule,
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
            __additional_properties__: BTreeMap::new(),
        }
    }
}

impl ResolvableConfig<UnitTestConfig> for UnitTestConfig {
    type Resolved = ResolvedUnitTestConfig;
    type PackageDefaults = ();
    type ResolveDefaults = StaticAnalysisKind;

    fn get_enabled_with_default(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn disable(&mut self) {
        self.enabled = Some(false);
    }

    fn apply_package_defaults(&mut self, _: ()) {}

    fn apply_resolve_defaults(&mut self, static_analysis: StaticAnalysisKind) {
        if self.static_analysis.is_none() {
            self.static_analysis = Some(Spanned::new(static_analysis));
        }
    }

    fn finalize(self) -> ResolvedUnitTestConfig {
        self.finalize_resolved()
    }

    fn default_to(&mut self, parent: &UnitTestConfig) {
        self.default_to_fields(parent);
    }

    // Unit tests have no `database`/`schema` field of their own -- they run against the model
    // under test's relation, so there is nothing to canonicalize a `catalog`-style alias into.
    fn canonicalize_adapter_aliases(&mut self, _default_adapter: AdapterType) {}
}

impl ConfigKeys for UnitTestConfig {
    // The default implementation from the trait will handle
    // extracting field names via serialization automatically
}

#[cfg(test)]
mod tests {
    use super::{ComputeArg, ProjectUnitTestConfig, UnitTestConfig};

    #[test]
    fn test_unit_test_query_tags_propagate_through_resolved_config() {
        let project: ProjectUnitTestConfig = dbt_yaml::from_str(
            r#"
+query_tags: '{"team":"unit-test"}'
__additional_properties__: {}
"#,
        )
        .unwrap();

        let resolved: UnitTestConfig = project.into();
        assert_eq!(
            resolved.__warehouse_specific_config__.query_tags.as_deref(),
            Some(r#"{"team":"unit-test"}"#)
        );
    }

    #[test]
    fn test_compute_local_is_an_alias_for_sidecar() {
        // Project-level, in dbt_project.yml.
        let project_config: ProjectUnitTestConfig = dbt_yaml::from_str(
            r#"
+compute: local
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(project_config.compute, Some(ComputeArg::Sidecar));

        // Per-test, in a properties file or `config()`.
        let config: UnitTestConfig = dbt_yaml::from_str(
            r#"
compute: local
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();
        assert_eq!(config.compute, Some(ComputeArg::Sidecar));
    }
}
