#![cfg(test)]

use crate::AdapterType;
use crate::sql_types::{DefaultTypeOps, TypeOps};
use arrow_schema::{Field, Fields, Schema};
use chrono::{DateTime, TimeDelta};
use dbt_schemas::schemas::{common::*, manifest::*, nodes::*, project::*};
use dbt_yaml::Spanned;
use serde_json;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct TestTableConfig {
    pub cluster_by: &'static [&'static str],
    pub partition_by: Option<BigqueryPartitionConfig>,
    pub kms_key: &'static str,
    pub description: &'static str,
    pub labels: HashMap<&'static str, &'static str>,
    pub tags: HashMap<&'static str, &'static str>,
    pub enable_refresh: Option<bool>,
    pub refresh_interval_minutes: f64,
    pub max_staleness: &'static str,
    pub expiration_ns: u64,
}

pub(crate) fn make_driver_data(cfg: TestTableConfig) -> Schema {
    let ty = DefaultTypeOps::new(AdapterType::Bigquery);
    let mut fields = Vec::new();
    let mut metadata = HashMap::from_iter(
        [
            ("Name", "my_view"),
            ("Location", "US"),
            ("FullID", "my_project:my_dataset.my_view"),
            ("Type", "MATERIALIZED_VIEW"),
            ("CreationTime", "2025-01-01T00:00:00+00:00"),
            ("LastModifiedTime", "2025-01-01T00:00:00+00:00"),
            ("NumBytes", "1234"),
            ("NumLongTermBytes", "12345"),
            ("NumRows", "123"),
            ("ETag", "sometag"),
            ("DefaultCollation", ""),
            (
                "SnapshotDefinition.BaseTableReference",
                "my_project:my-dataset.my_snapshot_base_table",
            ),
            (
                "SnapshotDefinition.SnapshotTime",
                "2025-01-01T00:00:00+00:00",
            ),
            (
                "CloneDefinition.BaseTableReference",
                "my_project:my-dataset.my_clone_base_table",
            ),
            ("CloneDefinition.CloneTime", "2025-01-01T00:00:00+00:00"),
            (
                "MaterializedView.LastRefreshTime",
                "2025-01-01T00:00:00+00:00",
            ),
            ("MaterializedView.Query", "SELECT 1"),
            ("MaterializedView.AllowNonIncrementalDefinition", "true"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string())),
    );

    metadata.insert(
        "Labels".to_string(),
        serde_json::to_string(&cfg.labels).unwrap(),
    );

    metadata.insert(
        "ResourceTags".to_string(),
        serde_json::to_string(&cfg.tags).unwrap(),
    );

    if !cfg.kms_key.is_empty() {
        metadata.insert(
            "EncryptionConfig.KMSKeyName".to_string(),
            cfg.kms_key.to_string(),
        );
    }

    if !cfg.description.is_empty() {
        metadata.insert("Description".to_string(), cfg.description.to_string());
    }

    if !cfg.cluster_by.is_empty() {
        metadata.insert(
            "Clustering.Fields".to_string(),
            serde_json::to_string(&cfg.cluster_by).unwrap(),
        );
    }

    if let Some(partition_by) = cfg.partition_by {
        let field_key = match partition_by.__inner__ {
            BigqueryPartitionConfigInner::Time(cfg) => {
                assert!(matches!(
                    cfg.granularity.as_str(),
                    "DAY" | "WEEK" | "MONTH" | "YEAR"
                ));
                metadata.insert("TimePartitioning.Type".to_string(), cfg.granularity);

                "TimePartitioning.Field"
            }
            BigqueryPartitionConfigInner::Range(cfg) => {
                metadata.insert(
                    "RangePartitioning.Range.Start".to_string(),
                    cfg.range.start.to_string(),
                );
                metadata.insert(
                    "RangePartitioning.Range.End".to_string(),
                    cfg.range.end.to_string(),
                );
                metadata.insert(
                    "RangePartitioning.Range.Interval".to_string(),
                    cfg.range.interval.to_string(),
                );

                "RangePartitioning.Field"
            }
        };

        metadata.insert(field_key.to_string(), partition_by.field.to_string());
        fields.push(
            Field::new(
                partition_by.field,
                ty.parse_into_arrow_type(&partition_by.data_type).unwrap(),
                false,
            )
            .with_metadata(HashMap::from([(
                "BIGQUERY:type".to_string(),
                partition_by.data_type,
            )])),
        );
    }

    if let Some(enable_refresh) = cfg.enable_refresh {
        metadata.insert(
            "MaterializedView.EnableRefresh".to_string(),
            serde_json::to_string(&enable_refresh).unwrap(),
        );

        if cfg.refresh_interval_minutes != 0.0 {
            let secs =
                ((cfg.refresh_interval_minutes - cfg.refresh_interval_minutes.abs()) * 60.0) as u64;
            let mins = cfg.refresh_interval_minutes.abs() as u64;
            metadata.insert(
                "MaterializedView.RefreshInterval".to_string(),
                format!("{mins}m{secs}s"),
            );
        }

        if !cfg.max_staleness.is_empty() {
            metadata.insert(
                "MaterializedView.MaxStaleness".to_string(),
                cfg.max_staleness.to_string(),
            );
        }

        if cfg.expiration_ns != 0 {
            metadata.insert(
                "ExpirationTime".to_string(),
                DateTime::from_timestamp_nanos(cfg.expiration_ns as i64).to_rfc3339(),
            );
        }
    }

    Schema::new(Fields::from(fields)).with_metadata(metadata)
}

pub(crate) fn make_local_config(cfg: TestTableConfig) -> DbtModel {
    let base_attrs = NodeBaseAttributes {
        adapter: AdapterType::Snowflake,
        propagate: Vec::new(),
        effective_propagation_target: None,
        database: "test_db".to_string(),
        schema: "test_schema".to_string(),
        alias: "test_table".to_string(),
        relation_name: None,
        quoting: dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING,
        quoting_ignore_case: false,
        materialized: DbtMaterialization::Table,
        compute: None,
        static_analysis: Spanned::new(dbt_common::io_args::StaticAnalysisKind::On),
        static_analysis_off_reason: None,
        enabled: true,
        extended_model: false,
        persist_docs: None,
        columns: vec![],
        refs: vec![],
        sources: vec![],
        functions: vec![],
        metrics: vec![],
        depends_on: Default::default(),
        unrendered_config: Default::default(),
    };

    let wh_config = WarehouseSpecificNodeConfig {
        partition_by: cfg
            .partition_by
            .map(PartitionConfig::BigqueryPartitionConfig),
        cluster_by: Some(ClusterConfig::List(
            cfg.cluster_by.iter().map(|v| (*v).to_string()).collect(),
        )),
        labels: Some(
            cfg.labels
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ),
        resource_tags: Some(
            cfg.tags
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ),
        kms_key_name: if cfg.kms_key.is_empty() {
            None
        } else {
            Some(cfg.kms_key.to_string())
        },
        description: if cfg.description.is_empty() {
            None
        } else {
            Some(cfg.description.to_string())
        },
        enable_refresh: cfg.enable_refresh,
        refresh_interval_minutes: if cfg.refresh_interval_minutes != 0.0 {
            Some(cfg.refresh_interval_minutes)
        } else {
            None
        },
        max_staleness: if !cfg.max_staleness.is_empty() {
            Some(cfg.max_staleness.to_string())
        } else {
            None
        },
        // NOTE: For simplicity, we are assuming that "now" is the unix epoch. Such that
        // expiration_ns = ns_to_expiration
        hours_to_expiration: dbt_common::serde_utils::Omissible::Present(
            if cfg.expiration_ns != 0 {
                Some(dbt_schemas::schemas::serde::StringOrInteger::Integer(
                    TimeDelta::nanoseconds(cfg.expiration_ns as i64).num_hours(),
                ))
            } else {
                None
            },
        ),
        ..Default::default()
    };

    let adapter_attr = AdapterAttr::from_config_and_dialect(&wh_config, AdapterType::Bigquery);

    DbtModel {
        deprecated_config: ModelConfig {
            __warehouse_specific_config__: wh_config,
            ..Default::default()
        },
        __common_attr__: CommonAttributes {
            name: "test_model".to_string(),
            fqn: vec!["test".to_string(), "test_model".to_string()],
            ..Default::default()
        },
        __adapter_attr__: adapter_attr,
        __base_attr__: base_attrs,
        ..Default::default()
    }
}
