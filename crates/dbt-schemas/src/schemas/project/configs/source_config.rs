use dbt_adapter_core::AdapterType;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::serde_utils::Omissible;
use dbt_proc_macros::Resolvable;
use dbt_yaml::{DbtSchema, ShouldBe, Spanned, Verbatim};
use serde::{Deserialize, Serialize};
// Type aliases for clarity
type YmlValue = dbt_yaml::Value;
use indexmap::IndexMap;
use std::collections::BTreeMap;
use std::collections::btree_map::Iter;

use super::config_keys::ConfigKeys;
use crate::schemas::common::PartitionConfig;
use crate::schemas::common::{
    ClusterConfig, FreshnessDefinition, Schedule, SchemaOrigin, SyncConfig,
};
use crate::schemas::manifest::GrantAccessToTarget;
use crate::schemas::project::configs::common::{
    LazyWarehouseSpecificNodeConfig, WarehouseSpecificNodeConfig,
};
use crate::schemas::project::configs::config_merge::{Tags, TblProperties};
use crate::schemas::project::{ResolvableConfig, TypedRecursiveConfig};
use crate::schemas::serde::{
    IndexesConfig, PartitionsConfig, PrimaryKeyConfig, StringOrArrayOfStrings, StringOrInteger,
    bool_or_string_bool, event_time_or_map_to_string, f64_or_string_f64,
    hours_to_expiration_or_string_omissible, u64_or_string_u64,
};
use dbt_proc_macros::DefaultTo;

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ProjectSourceConfig {
    #[serde(default, rename = "+enabled", deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(
        default,
        rename = "+event_time",
        deserialize_with = "event_time_or_map_to_string"
    )]
    pub event_time: Option<String>,
    #[serde(rename = "+meta")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(default, rename = "+freshness")]
    pub freshness: Omissible<Option<FreshnessDefinition>>,
    #[serde(rename = "+tags")]
    pub tags: Option<StringOrArrayOfStrings>,
    #[serde(rename = "+loaded_at_query")]
    pub loaded_at_query: Verbatim<Option<String>>,
    #[serde(rename = "+loaded_at_field")]
    pub loaded_at_field: Option<String>,
    #[serde(rename = "+static_analysis")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,

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
    #[serde(rename = "+external_location")]
    pub external_location: Option<String>,
    #[serde(rename = "+formatter")]
    pub formatter: Option<String>,
    #[serde(rename = "+location_root")]
    pub location_root: Option<String>,
    #[serde(rename = "+tblproperties")]
    pub tblproperties: Option<TblProperties>,
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
    #[serde(rename = "+merge_actions_explicit")]
    pub merge_actions_explicit: Option<String>,
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

    /// Specifies where the schema metadata originates: 'remote' (default) or 'local'
    #[serde(rename = "+schema_origin")]
    pub schema_origin: Option<SchemaOrigin>,
    /// Schema synchronization configuration
    #[serde(rename = "+sync")]
    pub sync: Option<SyncConfig>,

    // Flattened fields
    pub __additional_properties__: BTreeMap<String, ShouldBe<ProjectSourceConfig>>,
}

impl TypedRecursiveConfig for ProjectSourceConfig {
    fn type_name() -> &'static str {
        "source"
    }

    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>> {
        self.__additional_properties__.iter()
    }

    fn has_set_fields(&self) -> bool {
        self.enabled.is_some()
            || self.event_time.is_some()
            || self.meta.is_some()
            || self.freshness.is_present()
            || self.tags.is_some()
            || self.loaded_at_query.is_some()
            || self.loaded_at_field.is_some()
            || self.static_analysis.is_some()
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
            || self.external_location.is_some()
            || self.formatter.is_some()
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
            || self.merge_actions_explicit.is_some()
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
            || self.schema_origin.is_some()
            || self.sync.is_some()
    }
}

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(
    Resolvable, DefaultTo, Deserialize, Serialize, Debug, Clone, Default, PartialEq, DbtSchema,
)]
pub struct SourceConfig {
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "event_time_or_map_to_string")]
    pub event_time: Option<String>,
    #[serde(serialize_with = "crate::schemas::serde::serialize_none_as_empty_map")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(default)]
    pub freshness: Omissible<Option<FreshnessDefinition>>,
    #[serde(default)]
    pub tags: Tags,
    pub loaded_at_field: Option<String>,
    pub loaded_at_query: Verbatim<Option<String>>,
    #[resolved(promote, expect = "static_analysis set by apply_resolve_defaults")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    /// Specifies where the schema metadata originates: 'remote' (default) or 'local'
    #[resolved(promote)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_origin: Option<SchemaOrigin>,
    /// Schema synchronization configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formatter: Option<String>,
    // Adapter specific configs
    #[schemars(with = "WarehouseSpecificNodeConfig")]
    pub __warehouse_specific_config__: LazyWarehouseSpecificNodeConfig,
}

impl From<ProjectSourceConfig> for SourceConfig {
    fn from(config: ProjectSourceConfig) -> Self {
        Self {
            enabled: config.enabled,
            event_time: config.event_time,
            meta: config.meta,
            freshness: config.freshness,
            tags: Tags(config.tags),
            loaded_at_field: config.loaded_at_field,
            loaded_at_query: config.loaded_at_query,
            static_analysis: config.static_analysis,
            schema_origin: config.schema_origin,
            sync: config.sync,
            external_location: config.external_location,
            formatter: config.formatter,
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                description: None, // Only for Bigquery Models
                adapter_properties: None,
                external_volume: None,
                base_location_root: None,
                base_location_subpath: None,
                change_tracking: None,
                data_retention_time_in_days: None,
                max_data_extension_time_in_days: None,
                storage_serialization_policy: None,
                target_file_size: None,
                target_lag: None,
                snowflake_initialization_warehouse: None,
                immutable_where: None,
                snowflake_warehouse: None,
                refresh_warehouse: None,
                refresh_mode: None,
                initialize: None,
                scheduler: None,
                tmp_relation_type: None,
                query_tag: None,
                query_tags: None,
                properties: None,
                on_table_exists: None,
                view_security: None,
                grace_period: None,
                views_enabled: None,
                table_tag: None,
                row_access_policy: None,
                automatic_clustering: None,
                copy_grants: None,
                copy_tags: None,
                secure: None,
                transient: None,
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
                enable_change_history: None,
                incremental_apply_config_changes: None,
                persist_constraints: None,
                use_materialization_v2: None,
                use_safer_relation_operations: None,
                view_update_via_alter: None,

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
                merge_actions_explicit: config.merge_actions_explicit,
                not_matched_by_source_action: config.not_matched_by_source_action,
                merge_with_schema_evolution: config.merge_with_schema_evolution,
                skip_matched_step: config.skip_matched_step,
                skip_not_matched_step: config.skip_not_matched_step,
                unique_tmp_table_suffix: None,
                schedule: config.schedule,
                row_filter: None,

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

                // sources doesn't need this field
                primary_key: PrimaryKeyConfig::default(),
                category: None,

                engine: None,
                order_by: None,
                ttl: None,
                settings: None,
                query_settings: None,
                projections: None,
                sharding_key: None,
                disable_on_cluster: None,
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
            }
            .into(),
        }
    }
}

impl From<SourceConfig> for ProjectSourceConfig {
    fn from(config: SourceConfig) -> Self {
        let warehouse_specific = config.__warehouse_specific_config__.into_inner();
        Self {
            enabled: config.enabled,
            event_time: config.event_time,
            meta: config.meta,
            freshness: config.freshness,
            tags: config.tags.into_inner(),
            loaded_at_field: config.loaded_at_field,
            loaded_at_query: config.loaded_at_query,
            static_analysis: config.static_analysis,
            schema_origin: config.schema_origin,
            sync: config.sync,
            external_location: config.external_location,
            formatter: config.formatter,
            // BigQuery fields
            partition_by: warehouse_specific.partition_by,
            cluster_by: warehouse_specific.cluster_by,
            hours_to_expiration: warehouse_specific.hours_to_expiration,
            job_execution_timeout_seconds: warehouse_specific.job_execution_timeout_seconds,
            reservation: warehouse_specific.reservation,
            labels: warehouse_specific.labels,
            labels_from_meta: warehouse_specific.labels_from_meta,
            kms_key_name: warehouse_specific.kms_key_name,
            require_partition_filter: warehouse_specific.require_partition_filter,
            partition_expiration_days: warehouse_specific.partition_expiration_days,
            grant_access_to: warehouse_specific.grant_access_to,
            partitions: warehouse_specific.partitions,
            enable_refresh: warehouse_specific.enable_refresh,
            refresh_interval_minutes: warehouse_specific.refresh_interval_minutes,
            max_staleness: warehouse_specific.max_staleness,
            // Databricks fields
            file_format: warehouse_specific.file_format,
            catalog_name: warehouse_specific.catalog_name,
            location_root: warehouse_specific.location_root,
            tblproperties: warehouse_specific.tblproperties,
            include_full_name_in_path: warehouse_specific.include_full_name_in_path,
            liquid_clustered_by: warehouse_specific.liquid_clustered_by,
            auto_liquid_cluster: warehouse_specific.auto_liquid_cluster,
            clustered_by: warehouse_specific.clustered_by,
            buckets: warehouse_specific.buckets,
            catalog: warehouse_specific.catalog,
            databricks_tags: warehouse_specific.databricks_tags,
            compression: warehouse_specific.compression,
            databricks_compute: warehouse_specific.databricks_compute,
            target_alias: warehouse_specific.target_alias,
            source_alias: warehouse_specific.source_alias,
            matched_condition: warehouse_specific.matched_condition,
            not_matched_condition: warehouse_specific.not_matched_condition,
            not_matched_by_source_condition: warehouse_specific.not_matched_by_source_condition,
            merge_actions_explicit: warehouse_specific.merge_actions_explicit,
            not_matched_by_source_action: warehouse_specific.not_matched_by_source_action,
            merge_with_schema_evolution: warehouse_specific.merge_with_schema_evolution,
            skip_matched_step: warehouse_specific.skip_matched_step,
            skip_not_matched_step: warehouse_specific.skip_not_matched_step,
            // Redshift fields
            auto_refresh: warehouse_specific.auto_refresh,
            backup: warehouse_specific.backup,
            bind: warehouse_specific.bind,
            dist: warehouse_specific.dist,
            sort: warehouse_specific.sort,
            sort_type: warehouse_specific.sort_type,
            // MSSQL fields
            as_columnstore: warehouse_specific.as_columnstore,
            // Athena Fields
            table_type: warehouse_specific.table_type,
            // Postgres Fields
            indexes: warehouse_specific.indexes,
            unlogged: warehouse_specific.unlogged,
            // Schedule (Databricks streaming tables)
            schedule: warehouse_specific.schedule,
            __additional_properties__: BTreeMap::new(),
        }
    }
}

impl ResolvableConfig<SourceConfig> for SourceConfig {
    type Resolved = ResolvedSourceConfig;
    type PackageDefaults = ();
    type ResolveDefaults = (StaticAnalysisKind, Option<SyncConfig>);

    fn get_enabled_with_default(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn disable(&mut self) {
        self.enabled = Some(false);
    }

    fn apply_package_defaults(&mut self, _: ()) {}

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

    fn finalize(self) -> ResolvedSourceConfig {
        self.finalize_resolved()
    }

    fn default_to(&mut self, parent: &SourceConfig) {
        self.default_to_fields(parent);
    }

    // `SourceConfig` has no `database`/`schema` field of its own -- a source's database/schema
    // are per-table, authored as top-level `SourceProperties`/`Tables` keys in schema.yml
    // (`resolve_sources.rs`), not through this project-config pipeline. Canonicalized there
    // instead of here.
    fn canonicalize_adapter_aliases(&mut self, _alias_adapter: AdapterType) {}
}

impl ConfigKeys for SourceConfig {
    // The default implementation from the trait will handle
    // extracting field names via serialization automatically
}

impl crate::schemas::project::configs::warehouse_scope::WarehouseConfigResource for SourceConfig {
    const NODE_TYPE: dbt_telemetry::NodeType = dbt_telemetry::NodeType::Source;
}

#[cfg(test)]
mod tests {
    use super::SourceConfig;
    use crate::schemas::project::configs::warehouse_config_test_support::{
        assert_warehouse_config_allocates_only_when_set, typed_from_yaml,
    };
    use dbt_common::serde_utils::Omissible;

    #[test]
    fn test_source_config_freshness_loaded_at_field_parses() {
        let config: SourceConfig = dbt_yaml::from_str(
            r#"
freshness:
  loaded_at_field: FRESHNESS_LOADED_AT
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();

        let Omissible::Present(Some(freshness)) = config.freshness else {
            panic!("freshness config should parse");
        };
        assert_eq!(
            freshness.loaded_at_field.as_deref(),
            Some("FRESHNESS_LOADED_AT")
        );
        assert_eq!(freshness.loaded_at_query, None);
    }

    #[test]
    fn test_source_config_freshness_loaded_at_query_parses() {
        let config: SourceConfig = dbt_yaml::from_str(
            r#"
freshness:
  loaded_at_query: select max(loaded_at) from {{ source('raw', 'events') }}
__warehouse_specific_config__: {}
"#,
        )
        .unwrap();

        let Omissible::Present(Some(freshness)) = config.freshness else {
            panic!("freshness config should parse");
        };
        assert_eq!(freshness.loaded_at_field, None);
        assert_eq!(
            freshness.loaded_at_query.as_deref(),
            Some("select max(loaded_at) from {{ source('raw', 'events') }}")
        );
    }

    #[test]
    fn test_source_warehouse_config_allocates_only_when_set() {
        assert_warehouse_config_allocates_only_when_set::<SourceConfig>(
            "enabled: true\n",
            |config| &config.__warehouse_specific_config__,
        );
    }

    #[test]
    fn test_source_allocated_default_warehouse_config_matches_unallocated() {
        let (config, _) = typed_from_yaml::<SourceConfig>("enabled: true\n", false);
        let mut allocated = config.clone();
        allocated.__warehouse_specific_config__.catalog_name = None;
        assert!(allocated.__warehouse_specific_config__.get().is_some());
        assert_eq!(allocated, config);
        assert_eq!(format!("{allocated:?}"), format!("{config:?}"));
        assert_eq!(
            dbt_yaml::to_value(&allocated).unwrap(),
            dbt_yaml::to_value(&config).unwrap()
        );
    }

    #[test]
    fn test_source_warehouse_config_default_to() {
        use super::ProjectSourceConfig;
        use crate::schemas::project::dbt_project::ResolvableConfig;
        use crate::schemas::serde::StringOrInteger;

        let (parent, _) =
            typed_from_yaml::<SourceConfig>("catalog_name: cat\nhours_to_expiration: 5\n", false);

        let mut child = SourceConfig::default();
        child.default_to(&parent);
        assert_eq!(child, parent);

        // An explicit null still clears the inherited value.
        let (mut child, _) = typed_from_yaml::<SourceConfig>("hours_to_expiration: null\n", false);
        child.default_to(&parent);
        let warehouse_specific = &child.__warehouse_specific_config__;
        assert_eq!(warehouse_specific.catalog_name.as_deref(), Some("cat"));
        assert_eq!(
            warehouse_specific.hours_to_expiration,
            Omissible::Present(None)
        );

        let mut child = parent.clone();
        child.default_to(&SourceConfig::default());
        assert_eq!(child, parent);
        assert_eq!(
            child.__warehouse_specific_config__.hours_to_expiration,
            Omissible::Present(Some(StringOrInteger::Integer(5)))
        );

        let mut child = SourceConfig::default();
        child.default_to(&SourceConfig::default());
        assert!(child.__warehouse_specific_config__.get().is_none());

        let round_tripped: SourceConfig = ProjectSourceConfig::from(parent.clone()).into();
        assert_eq!(
            round_tripped.__warehouse_specific_config__,
            parent.__warehouse_specific_config__
        );
    }
}
