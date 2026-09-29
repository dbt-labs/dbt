//! Testing utilities for Snowflake relation configs
#![cfg(test)]

use arrow_array::record_batch;
use dbt_schemas::schemas::{
    AdapterAttr, CommonAttributes, DbtModel, NodeBaseAttributes,
    common::{ClusterConfig, DbtMaterialization},
    project::{ModelConfig, WarehouseSpecificNodeConfig},
};
use dbt_yaml::Spanned;

use crate::{AdapterType, relation::snowflake::config::SnowflakeDescribeResults};

/// Model-side config, i.e. what the user writes.
///
/// `default()` is NOT a valid dynamic-table config — `snowflake_warehouse` is required and
/// defaults to `None`. For anything diff-related use `TestRemoteState`, not
/// `make_remote_config`.
#[derive(Debug, Default)]
pub(crate) struct TestDynamicTableConfig {
    pub cluster_by: Option<ClusterConfig>,
    pub immutable_where: Option<&'static str>,
    pub initialize: &'static str,
    pub refresh_mode: Option<&'static str>,
    pub row_access_policy: Option<&'static str>,
    pub scheduler: Option<&'static str>,
    pub snowflake_initialization_warehouse: Option<&'static str>,
    pub snowflake_warehouse: Option<&'static str>,
    pub refresh_warehouse: Option<&'static str>,
    pub table_tag: Option<&'static str>,
    pub target_lag: Option<&'static str>,
    pub transient: Option<bool>,
    pub copy_grants: Option<bool>,
}

/// Raw `SHOW ...` readback values, spelled exactly as Snowflake reports them.
///
/// Deliberately a separate struct from `TestDynamicTableConfig` (the model-side config),
/// because Snowflake does not echo config back verbatim: e.g. `'60 seconds'` reads back as
/// `'1 minute'`, `["id","val"]` as `(id, val)`.
#[derive(Debug, Default)]
pub(crate) struct TestRemoteState {
    pub cluster_by: Option<String>,
    pub immutable_where: Option<String>,
    pub initialization_warehouse: Option<String>,
    pub refresh_mode: Option<String>,
    pub scheduler: Option<String>,
    pub target_lag: Option<String>,
    pub transient: Option<bool>,
    /// `SHOW DYNAMIC TABLES` names this column `warehouse`; interactive names it
    /// `refresh_warehouse`.
    pub refresh_warehouse: Option<String>,
}

/// Readback shaped like `SHOW DYNAMIC TABLES`.
pub(crate) fn make_remote_state(state: TestRemoteState) -> SnowflakeDescribeResults {
    let batch = record_batch!(
        ("name", Utf8, ["test_table"]),
        ("schema_name", Utf8, ["test_schema"]),
        ("database_name", Utf8, ["test_db"]),
        ("text", Utf8, [""]),
        ("target_lag", Utf8, [state.target_lag]),
        ("scheduler", Utf8, [state.scheduler]),
        ("warehouse", Utf8, [state.refresh_warehouse]),
        ("refresh_mode", Utf8, [state.refresh_mode]),
        (
            "initialization_warehouse",
            Utf8,
            [state.initialization_warehouse]
        ),
        ("immutable_where", Utf8, [state.immutable_where]),
        ("cluster_by", Utf8, [state.cluster_by]),
        ("transient", Boolean, [state.transient])
    )
    .unwrap();
    SnowflakeDescribeResults {
        record_batch: std::sync::Arc::new(batch),
    }
}

/// Readback shaped like `SHOW INTERACTIVE TABLES` — a strictly reduced column set. There is
/// no `warehouse`, `scheduler`, `refresh_mode`, `immutable_where` or `transient` column, so
/// a component that reads one of those names must tolerate its absence (or be reading the
/// wrong name).
pub(crate) fn make_remote_interactive_state(state: TestRemoteState) -> SnowflakeDescribeResults {
    let batch = record_batch!(
        ("name", Utf8, ["test_table"]),
        ("schema_name", Utf8, ["test_schema"]),
        ("database_name", Utf8, ["test_db"]),
        ("text", Utf8, [""]),
        ("target_lag", Utf8, [state.target_lag]),
        ("refresh_warehouse", Utf8, [state.refresh_warehouse]),
        (
            "initialization_warehouse",
            Utf8,
            [state.initialization_warehouse]
        ),
        ("cluster_by", Utf8, [state.cluster_by])
    )
    .unwrap();
    SnowflakeDescribeResults {
        record_batch: std::sync::Arc::new(batch),
    }
}

/// Convenience wrapper for tests that only need the readback to echo the model config back
/// verbatim.
pub(crate) fn make_remote_config(cfg: TestDynamicTableConfig) -> SnowflakeDescribeResults {
    make_remote_state(TestRemoteState {
        cluster_by: cfg.cluster_by.map(|c| c.fields().join(", ")),
        immutable_where: cfg.immutable_where.map(|s| s.to_owned()),
        initialization_warehouse: cfg.snowflake_initialization_warehouse.map(|s| s.to_owned()),
        refresh_mode: cfg.refresh_mode.map(|s| s.to_owned()),
        scheduler: cfg.scheduler.map(|s| s.to_owned()),
        target_lag: cfg.target_lag.map(|s| s.to_owned()),
        transient: cfg.transient,
        refresh_warehouse: cfg.snowflake_warehouse.map(|s| s.to_owned()),
    })
}

pub(crate) fn make_local_config(cfg: TestDynamicTableConfig) -> DbtModel {
    let base_attrs = NodeBaseAttributes {
        adapter: AdapterType::Snowflake,
        propagate: Vec::new(),
        effective_propagation_target: None,
        unrendered_config: Default::default(),
        database: "test_db".to_string(),
        schema: "test_schema".to_string(),
        alias: "test_table".to_string(),
        relation_name: None,
        quoting: dbt_schemas::schemas::relations::SNOWFLAKE_RESOLVED_QUOTING,
        quoting_ignore_case: false,
        materialized: DbtMaterialization::DynamicTable,
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
    };

    let wh_config = WarehouseSpecificNodeConfig {
        cluster_by: cfg.cluster_by,
        immutable_where: cfg.immutable_where.map(|s| s.to_owned()),
        initialize: Some(cfg.initialize.to_owned()),
        refresh_mode: cfg.refresh_mode.map(|s| s.to_owned()),
        row_access_policy: cfg.row_access_policy.map(|s| s.to_owned()),
        scheduler: cfg.scheduler.map(|s| s.to_owned()),
        snowflake_initialization_warehouse: cfg
            .snowflake_initialization_warehouse
            .map(|s| s.to_owned()),
        snowflake_warehouse: cfg.snowflake_warehouse.map(|s| s.to_owned()),
        refresh_warehouse: cfg.refresh_warehouse.map(|s| s.to_owned()),
        table_tag: cfg.table_tag.map(|s| s.to_owned()),
        target_lag: cfg.target_lag.map(|s| s.to_owned()),
        transient: cfg.transient,
        copy_grants: cfg.copy_grants,
        ..Default::default()
    };

    let adapter_attr = AdapterAttr::from_config_and_dialect(&wh_config, AdapterType::Snowflake);

    DbtModel {
        deprecated_config: ModelConfig {
            __warehouse_specific_config__: wh_config,
            ..Default::default()
        },
        __common_attr__: CommonAttributes {
            name: "test_dt".to_string(),
            fqn: vec!["test".to_string(), "test_dt".to_string()],
            ..Default::default()
        },
        __adapter_attr__: adapter_attr,
        __base_attr__: base_attrs,
        ..Default::default()
    }
}
