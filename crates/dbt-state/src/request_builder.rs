//! Stable request-builder DTOs and helpers for the dbt State service protocol.
//!
//! This module accepts normalized inputs from higher-level crates, derives
//! service-specific fields such as execution type and semantic extras, and
//! converts those inputs into generated protobuf request messages.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::time::Duration;

use dbt_telemetry::NodeType;
use serde_json::Value;
use thiserror::Error;

use crate::hash;
use crate::materialization::normalize_incremental_strategy;
use crate::proto::query_cache::{
    ClientPrepareEnrichedSqlRequest, ClientTelemetryEvent, CloneRequest, DbtNodeState,
    ExecutionOutcome, ExecutionRecord, ModelExecutionType, QueryDependency, SessionEndRequest,
    SessionStartRequest, StaleUpstreamPolicy, Struct, SubmitEnrichedSqlRequest,
    SubmitTelemetryBatchRequest, SubmitValuesRequest, TableModifiedInfo, TableProperties,
    ValuesExecution, client_telemetry_event, execution_record,
};

pub const SQL_SEMANTIC_EXTRA_KEYS: &[&str] = &[
    "on_schema_change",
    "incremental_predicates",
    "merge_update_columns",
    "merge_exclude_columns",
    "contract",
    "constraints",
    "cluster_by",
    "unique_key",
    "grants",
    "event_time",
    "sql_header",
    "lookback",
    "partition_by",
    "table_format",
    // Test attributes
    "severity",
    "limit",
    "where",
    "fail_calc",
    "warn_if",
    "error_if",
    "store_failures",
    "store_failures_as",
    // Databricks attributes
    "auto_liquid_cluster",
    "databricks_tags",
    "file_format",
    "location_root",
    "include_full_name_in_path",
    "clustered_by",
    "buckets",
    "liquid_clustered_by",
    "tblproperties",
    // Snowflake attributes
    "transient",
    "target_lag",
    "refresh_mode",
    "immutable_where",
    "copy_grants",
    "tmp_relation_type",
    // Postgres attributes
    "unlogged",
    "indexes",
    // Bigquery attributes
    "require_partition_filter",
    "partition_expiration_days",
    "hours_to_expiration",
    // Redshift attributes
    "dist",
    "sort",
    "sort_type",
];

pub const SEED_SEMANTIC_EXTRA_KEYS: &[&str] = &["column_types", "quote_columns", "delimiter"];

#[derive(Debug, Error)]
pub enum RequestBuildError {
    #[error("failed to serialize dbt State semantic extra: {0}")]
    SemanticExtra(#[from] serde_json::Error),
    #[error("failed to build dbt State node hash: {0}")]
    NodeHashError(#[from] hash::NodeHashError),
    #[error("failed to read seed data for dbt State hash: {0}")]
    SeedHash(#[from] io::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeIdentity {
    pub name: String,
    pub fqn: Vec<String>,
    pub unique_id: String,
}

impl NodeIdentity {
    pub fn labels(&self) -> HashMap<String, String> {
        HashMap::from([
            ("dbt_node_name".to_string(), self.name.clone()),
            ("dbt_node_fqn".to_string(), self.fqn.join(".")),
            ("dbt_node_unique_id".to_string(), self.unique_id.clone()),
        ])
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionTypeInput {
    pub resource_type: NodeType,
    pub is_view: bool,
    pub is_custom_materialization: bool,
    pub is_incremental: bool,
    pub full_refresh: bool,
    pub incremental_strategy: Option<String>,
    pub has_unique_key: bool,
}

/// Derives the wire execution type for a node.
///
/// Branch order is significant. `DBT_CUSTOM` is decided *before* the
/// incremental/snapshot branch, so a custom node stays `DBT_CUSTOM` even under
/// `--full-refresh`. The incremental/snapshot branch is gated on full refresh
/// as a whole, so a full-refreshed snapshot rewrites its target wholesale and
/// is `FULL` rather than `SNAPSHOT`.
pub fn execution_type_from_input(
    input: &ExecutionTypeInput,
) -> Result<ModelExecutionType, RequestBuildError> {
    if input.resource_type == NodeType::Test {
        return Ok(ModelExecutionType::DbtDataTest);
    }
    if input.is_view {
        return Ok(ModelExecutionType::View);
    }
    if input.is_custom_materialization {
        return Ok(ModelExecutionType::DbtCustom);
    }
    // An incremental model, or any node whose resource type is snapshot.
    let is_incremental_or_snapshot =
        input.is_incremental || input.resource_type == NodeType::Snapshot;
    if is_incremental_or_snapshot && !input.full_refresh {
        if input.resource_type == NodeType::Snapshot {
            return Ok(ModelExecutionType::Snapshot);
        }

        // Proto enum names are upper-case; the shared normalizer lower-cases,
        // so flip the case after it has stripped any `+` prefix.
        let strategy = normalize_incremental_strategy(
            input.incremental_strategy.as_deref().unwrap_or("append"),
        )
        .to_ascii_uppercase();

        if strategy == "MERGE" && !input.has_unique_key {
            return Ok(ModelExecutionType::Append);
        }

        // A strategy outside dbt's built-ins is already classified as a custom
        // materialization above; this fallback only guards names the proto does
        // not carry.
        return Ok(
            ModelExecutionType::from_str_name(&strategy).unwrap_or(ModelExecutionType::DbtCustom)
        );
    }

    Ok(ModelExecutionType::Full)
}

pub type SemanticExtraConfig = BTreeMap<String, Option<Value>>;
pub type SemanticExtras = HashMap<String, String>;

pub fn sql_semantic_extras(
    config: &SemanticExtraConfig,
) -> Result<SemanticExtras, RequestBuildError> {
    semantic_extras_from_keys(config, SQL_SEMANTIC_EXTRA_KEYS)
}

pub fn seed_semantic_extras(
    config: &SemanticExtraConfig,
) -> Result<SemanticExtras, RequestBuildError> {
    semantic_extras_from_keys(config, SEED_SEMANTIC_EXTRA_KEYS)
}

pub fn semantic_extras_from_keys(
    config: &SemanticExtraConfig,
    keys: &[&str],
) -> Result<SemanticExtras, RequestBuildError> {
    let mut extras = HashMap::new();

    for key in keys {
        if let Some(value) = config.get(*key) {
            let serialized = match value {
                Some(value) => serde_json::to_string(value)?,
                None => String::new(),
            };
            extras.insert((*key).to_string(), serialized);
        }
    }

    Ok(extras)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitEnrichedSqlRequestInput {
    pub target_table: Option<String>,
    pub dialect: String,
    pub default_catalog: String,
    pub default_schema: Option<String>,
    pub execution_type: ModelExecutionType,
    pub sql: String,
    pub tables: Vec<TableModifiedInfo>,
    pub query_dependencies: Vec<QueryDependency>,
    pub semantic_extras: SemanticExtras,
    pub freshness_tolerance_seconds: i64,
    pub lenient_dependencies: Vec<String>,
    pub tolerate_nondeterminism: bool,
    pub labels: HashMap<String, String>,
    pub clone_time_travel_limit: Option<i64>,
    pub clone_table_properties: Option<TableProperties>,
    pub stale_upstream_policy: StaleUpstreamPolicy,
    pub clone_chain_depth_limit: Option<i64>,
    pub dbt_node_state: Option<DbtNodeState>,
    pub compare_unrendered_code: bool,
    pub table_namespace: Option<String>,
}

impl SubmitEnrichedSqlRequestInput {
    pub fn into_proto(self) -> SubmitEnrichedSqlRequest {
        SubmitEnrichedSqlRequest {
            target_table: self.target_table,
            dialect: self.dialect,
            default_catalog: self.default_catalog,
            default_schema: self.default_schema,
            execution_type: self.execution_type as i32,
            sql: self.sql,
            tables: self.tables,
            query_dependencies: self.query_dependencies,
            semantic_extras: self.semantic_extras,
            freshness_tolerance_seconds: self.freshness_tolerance_seconds,
            lenient_dependencies: self.lenient_dependencies,
            tolerate_nondeterminism: self.tolerate_nondeterminism,
            labels: self.labels,
            clone_time_travel_limit: self.clone_time_travel_limit,
            clone_table_properties: self.clone_table_properties,
            stale_upstream_policy: self.stale_upstream_policy as i32,
            clone_chain_depth_limit: self.clone_chain_depth_limit,
            dbt_node_state: self.dbt_node_state,
            compare_unrendered_code: self.compare_unrendered_code,
            table_namespace: self.table_namespace,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitValuesRequestInput {
    pub target_table: String,
    pub dialect: String,
    pub default_catalog: String,
    pub values_hash: String,
    pub semantic_extras: SemanticExtras,
    pub last_modified_epoch: Option<i64>,
    pub labels: HashMap<String, String>,
    pub clone_time_travel_limit: Option<i64>,
    pub clone_table_properties: Option<TableProperties>,
    pub clone_chain_depth_limit: Option<i64>,
    pub dbt_node_state: Option<DbtNodeState>,
    pub table_namespace: Option<String>,
}

impl SubmitValuesRequestInput {
    pub fn into_proto(self) -> SubmitValuesRequest {
        SubmitValuesRequest {
            target_table: self.target_table,
            dialect: self.dialect,
            default_catalog: self.default_catalog,
            values_hash: self.values_hash,
            semantic_extras: self.semantic_extras,
            last_modified_epoch: self.last_modified_epoch,
            labels: self.labels,
            clone_time_travel_limit: self.clone_time_travel_limit,
            clone_table_properties: self.clone_table_properties,
            clone_chain_depth_limit: self.clone_chain_depth_limit,
            dbt_node_state: self.dbt_node_state,
            table_namespace: self.table_namespace,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionOutcomeInput {
    pub last_modified_epoch: Option<i64>,
    pub table_type: Option<String>,
    pub execution_runtime_ms: Option<i64>,
}

impl ExecutionOutcomeInput {
    pub fn into_proto(self) -> ExecutionOutcome {
        ExecutionOutcome {
            last_modified_epoch: self.last_modified_epoch,
            table_type: self.table_type,
            execution_results: None,
            execution_runtime_ms: self.execution_runtime_ms,
        }
    }
}

pub fn sql_execution_record_from_submit_request(
    request: SubmitEnrichedSqlRequest,
    outcome: ExecutionOutcomeInput,
    from_speculative_submit: bool,
) -> ExecutionRecord {
    ExecutionRecord {
        outcome: Some(outcome.into_proto()),
        input: Some(execution_record::Input::EnrichedSql(Box::new(
            crate::proto::query_cache::SqlExecution {
                target_table: request.target_table,
                dialect: request.dialect,
                default_catalog: request.default_catalog,
                execution_type: request.execution_type,
                sql: request.sql,
                tables: request.tables,
                query_dependencies: request.query_dependencies,
                semantic_extras: request.semantic_extras,
                labels: request.labels,
                default_schema: request.default_schema,
                dbt_node_state: request.dbt_node_state,
                from_speculative_submit,
                table_namespace: request.table_namespace,
            },
        ))),
    }
}

pub fn values_execution_record_from_submit_request(
    request: SubmitValuesRequest,
    outcome: ExecutionOutcomeInput,
) -> ExecutionRecord {
    ExecutionRecord {
        outcome: Some(outcome.into_proto()),
        input: Some(execution_record::Input::Values(Box::new(ValuesExecution {
            target_table: request.target_table,
            dialect: request.dialect,
            default_catalog: request.default_catalog,
            values_hash: request.values_hash,
            semantic_extras: request.semantic_extras,
            labels: request.labels,
            dbt_node_state: request.dbt_node_state,
            table_namespace: request.table_namespace,
        }))),
    }
}

/// `dbt_run_cache_version` and `sqlglot_version` are left empty: unlike the
/// Python dbt State client (a package layered on top of dbt-core, using a
/// separately-versioned sqlglot dependency), Fusion has no separate "state
/// client plugin" or SQL-parsing library with its own version to report —
/// the running binary's version (`dbt_version`) is the only one that applies.
pub fn session_start_event(config: Struct, event_order: i64) -> ClientTelemetryEvent {
    ClientTelemetryEvent {
        request: Some(client_telemetry_event::Request::SessionStart(
            SessionStartRequest {
                dbt_run_cache_version: String::new(),
                dbt_version: env!("CARGO_PKG_VERSION").to_string(),
                sqlglot_version: String::new(),
                config: Some(config),
            },
        )),
        event_order: Some(event_order),
    }
}

pub fn session_end_event(
    duration: Duration,
    result: SessionEndResult,
    description: impl Into<String>,
    event_order: i64,
) -> ClientTelemetryEvent {
    ClientTelemetryEvent {
        request: Some(client_telemetry_event::Request::SessionEnd(
            SessionEndRequest {
                session_duration: Some(prost_types::Duration {
                    seconds: i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
                    nanos: i32::try_from(duration.subsec_nanos()).unwrap_or_default(),
                }),
                result: result as i32,
                result_description: description.into(),
                metrics: Some(Struct::default()),
            },
        )),
        event_order: Some(event_order),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionEndResult {
    Success = 0,
    Failure = 1,
    Cancelled = 2,
}

#[allow(clippy::too_many_arguments)]
pub fn enriched_sql_prepared_event(
    request_id: String,
    duration: Duration,
    target_table_fqn: Option<String>,
    num_dependencies: Option<i64>,
    num_view_dependencies: Option<i64>,
    error_type: Option<String>,
    labels: HashMap<String, String>,
    event_order: i64,
) -> ClientTelemetryEvent {
    ClientTelemetryEvent {
        request: Some(client_telemetry_event::Request::EnrichedSqlPrepared(
            ClientPrepareEnrichedSqlRequest {
                request_id,
                duration: duration.as_secs_f64(),
                target_table_fqn,
                num_dependencies,
                num_view_dependencies,
                error_type,
                view_traversal_duration_ms: None,
                last_modified_duration_ms: None,
                labels,
            },
        )),
        event_order: Some(event_order),
    }
}

pub fn telemetry_batch(events: Vec<ClientTelemetryEvent>) -> SubmitTelemetryBatchRequest {
    SubmitTelemetryBatchRequest { events }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloneRequestInput {
    pub target_table: String,
    pub dialect: String,
    pub default_catalog: String,
    pub execution_type: ModelExecutionType,
    pub clone_source_table: String,
    pub clone_source_last_modified_epoch: Option<i64>,
    pub labels: HashMap<String, String>,
    pub clone_source_table_type: Option<String>,
    pub table_properties: Option<TableProperties>,
    pub clone_chain_depth_limit: Option<i64>,
    pub table_namespace: Option<String>,
}

impl CloneRequestInput {
    pub fn into_proto(self) -> CloneRequest {
        CloneRequest {
            target_table: self.target_table,
            dialect: self.dialect,
            default_catalog: self.default_catalog,
            execution_type: self.execution_type as i32,
            clone_source_table: self.clone_source_table,
            clone_source_last_modified_epoch: self.clone_source_last_modified_epoch,
            labels: self.labels,
            clone_source_table_type: self.clone_source_table_type,
            table_properties: self.table_properties,
            clone_chain_depth_limit: self.clone_chain_depth_limit,
            table_namespace: self.table_namespace,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labels_match_python_client_keys() {
        let labels = NodeIdentity {
            name: "orders".to_string(),
            fqn: vec![
                "jaffle_shop".to_string(),
                "marts".to_string(),
                "orders".to_string(),
            ],
            unique_id: "model.jaffle_shop.orders".to_string(),
        }
        .labels();

        assert_eq!(labels.get("dbt_node_name").unwrap(), "orders");
        assert_eq!(
            labels.get("dbt_node_fqn").unwrap(),
            "jaffle_shop.marts.orders"
        );
        assert_eq!(
            labels.get("dbt_node_unique_id").unwrap(),
            "model.jaffle_shop.orders"
        );
        assert_eq!(labels.len(), 3);
    }

    #[test]
    fn telemetry_events_use_expected_payloads_and_order() {
        let start = session_start_event(Struct::default(), 7);
        assert_eq!(start.event_order, Some(7));
        let Some(client_telemetry_event::Request::SessionStart(payload)) = start.request else {
            panic!("expected session start event");
        };
        assert_eq!(payload.dbt_run_cache_version, "");
        assert_eq!(payload.dbt_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(payload.sqlglot_version, "");

        let mut labels = HashMap::new();
        labels.insert(
            "dbt_node_unique_id".to_string(),
            "model.pkg.node".to_string(),
        );
        let prepared = enriched_sql_prepared_event(
            "request-1".to_string(),
            Duration::from_millis(1250),
            Some("db.schema.node".to_string()),
            Some(2),
            Some(1),
            None,
            labels,
            8,
        );
        assert_eq!(prepared.event_order, Some(8));
        let Some(client_telemetry_event::Request::EnrichedSqlPrepared(payload)) = prepared.request
        else {
            panic!("expected prepared SQL event");
        };
        assert_eq!(payload.request_id, "request-1");
        assert_eq!(payload.duration, 1.25);
        assert_eq!(payload.target_table_fqn.as_deref(), Some("db.schema.node"));
        assert_eq!(payload.num_dependencies, Some(2));
        assert_eq!(payload.num_view_dependencies, Some(1));
        assert_eq!(payload.error_type, None);

        let failed = enriched_sql_prepared_event(
            "request-2".to_string(),
            Duration::from_millis(40),
            None,
            None,
            None,
            Some("Rpc".to_string()),
            HashMap::new(),
            9,
        );
        let Some(client_telemetry_event::Request::EnrichedSqlPrepared(failed_payload)) =
            failed.request
        else {
            panic!("expected prepared SQL event");
        };
        assert_eq!(failed_payload.error_type, Some("Rpc".to_string()));
        assert_eq!(failed_payload.num_dependencies, None);

        let end = session_end_event(
            Duration::from_secs(3),
            SessionEndResult::Success,
            "completed",
            9,
        );
        assert_eq!(end.event_order, Some(9));
        let Some(client_telemetry_event::Request::SessionEnd(payload)) = end.request else {
            panic!("expected session end event");
        };
        assert_eq!(payload.result, SessionEndResult::Success as i32);
        assert_eq!(payload.result_description, "completed");
        assert_eq!(payload.session_duration.unwrap().seconds, 3);
    }

    #[test]
    fn sql_semantic_extras_match_python_key_surface_and_json_values() {
        let mut config = SemanticExtraConfig::new();
        config.insert(
            "on_schema_change".to_string(),
            Some(json!("sync_all_columns")),
        );
        config.insert(
            "incremental_predicates".to_string(),
            Some(json!(["updated_at >= current_date", "deleted_at is null"])),
        );
        config.insert("merge_update_columns".to_string(), None);
        config.insert("unique_key".to_string(), Some(json!("id")));

        let extras = sql_semantic_extras(&config).unwrap();

        assert_eq!(
            extras.get("on_schema_change").unwrap(),
            "\"sync_all_columns\""
        );
        assert_eq!(
            extras.get("incremental_predicates").unwrap(),
            "[\"updated_at >= current_date\",\"deleted_at is null\"]"
        );
        assert_eq!(extras.get("merge_update_columns").unwrap(), "");
        assert_eq!(extras.get("unique_key").unwrap(), "\"id\"");
    }

    #[test]
    fn sql_semantic_extras_include_databricks_attributes() {
        let mut config = SemanticExtraConfig::new();
        config.insert("auto_liquid_cluster".to_string(), Some(json!(true)));
        config.insert(
            "databricks_tags".to_string(),
            Some(json!({"team": "analytics"})),
        );

        let extras = sql_semantic_extras(&config).unwrap();

        assert_eq!(extras.get("auto_liquid_cluster").unwrap(), "true");
        assert_eq!(
            extras.get("databricks_tags").unwrap(),
            "{\"team\":\"analytics\"}"
        );
    }

    #[test]
    fn sql_semantic_extras_include_expanded_model_attributes() {
        let mut config = SemanticExtraConfig::new();
        config.insert(
            "contract".to_string(),
            Some(json!({"alias_types": true, "enforced": false})),
        );
        config.insert("constraints".to_string(), Some(json!([])));
        config.insert("cluster_by".to_string(), Some(json!(["id"])));
        config.insert("grants".to_string(), Some(json!({})));
        config.insert("event_time".to_string(), None);
        config.insert("sql_header".to_string(), Some(json!("/* header */")));
        config.insert("lookback".to_string(), Some(json!(1)));
        config.insert(
            "partition_by".to_string(),
            Some(json!({"field": "created_at"})),
        );
        config.insert("table_format".to_string(), Some(json!("iceberg")));

        let extras = sql_semantic_extras(&config).unwrap();

        assert_eq!(
            extras.get("contract").unwrap(),
            "{\"alias_types\":true,\"enforced\":false}"
        );
        assert_eq!(extras.get("constraints").unwrap(), "[]");
        assert_eq!(extras.get("cluster_by").unwrap(), "[\"id\"]");
        assert_eq!(extras.get("grants").unwrap(), "{}");
        assert_eq!(extras.get("event_time").unwrap(), "");
        assert_eq!(extras.get("sql_header").unwrap(), "\"/* header */\"");
        assert_eq!(extras.get("lookback").unwrap(), "1");
        assert_eq!(
            extras.get("partition_by").unwrap(),
            "{\"field\":\"created_at\"}"
        );
        assert_eq!(extras.get("table_format").unwrap(), "\"iceberg\"");
    }

    #[test]
    fn sql_semantic_extras_include_snowflake_and_postgres_attributes() {
        let mut config = SemanticExtraConfig::new();
        config.insert("transient".to_string(), Some(json!(true)));
        config.insert("target_lag".to_string(), Some(json!("1 hour")));
        config.insert("refresh_mode".to_string(), Some(json!("FULL")));
        config.insert("immutable_where".to_string(), Some(json!("id > 0")));
        config.insert("copy_grants".to_string(), Some(json!(true)));
        config.insert("tmp_relation_type".to_string(), Some(json!("TRANSIENT")));
        config.insert("unlogged".to_string(), Some(json!(true)));
        config.insert("indexes".to_string(), Some(json!([{"columns": ["id"]}])));

        let extras = sql_semantic_extras(&config).unwrap();

        assert_eq!(extras.get("transient").unwrap(), "true");
        assert_eq!(extras.get("target_lag").unwrap(), "\"1 hour\"");
        assert_eq!(extras.get("refresh_mode").unwrap(), "\"FULL\"");
        assert_eq!(extras.get("immutable_where").unwrap(), "\"id > 0\"");
        assert_eq!(extras.get("copy_grants").unwrap(), "true");
        assert_eq!(extras.get("tmp_relation_type").unwrap(), "\"TRANSIENT\"");
        assert_eq!(extras.get("unlogged").unwrap(), "true");
        assert_eq!(extras.get("indexes").unwrap(), "[{\"columns\":[\"id\"]}]");
    }

    #[test]
    fn sql_semantic_extras_include_bigquery_and_redshift_attributes() {
        let mut config = SemanticExtraConfig::new();
        config.insert("require_partition_filter".to_string(), Some(json!(true)));
        config.insert("partition_expiration_days".to_string(), Some(json!(30)));
        config.insert("hours_to_expiration".to_string(), Some(json!(24)));
        config.insert("dist".to_string(), Some(json!("ALL")));
        config.insert("sort".to_string(), Some(json!(["id"])));
        config.insert("sort_type".to_string(), Some(json!("COMPOUND")));

        let extras = sql_semantic_extras(&config).unwrap();

        assert_eq!(extras.get("require_partition_filter").unwrap(), "true");
        assert_eq!(extras.get("partition_expiration_days").unwrap(), "30");
        assert_eq!(extras.get("hours_to_expiration").unwrap(), "24");
        assert_eq!(extras.get("dist").unwrap(), "\"ALL\"");
        assert_eq!(extras.get("sort").unwrap(), "[\"id\"]");
        assert_eq!(extras.get("sort_type").unwrap(), "\"COMPOUND\"");
    }

    #[test]
    fn seed_semantic_extras_match_python_key_surface() {
        let mut config = SemanticExtraConfig::new();
        config.insert("column_types".to_string(), Some(json!({"id": "integer"})));
        config.insert("quote_columns".to_string(), Some(json!(true)));
        config.insert("delimiter".to_string(), Some(json!("|")));
        config.insert("on_schema_change".to_string(), Some(json!("ignored")));

        let extras = seed_semantic_extras(&config).unwrap();

        assert_eq!(extras.get("column_types").unwrap(), "{\"id\":\"integer\"}");
        assert_eq!(extras.get("quote_columns").unwrap(), "true");
        assert_eq!(extras.get("delimiter").unwrap(), "\"|\"");
        assert_eq!(extras.len(), 3);
    }

    #[test]
    fn execution_type_mapping_matches_python_client() {
        assert_eq!(
            execution_type_from_input(&ExecutionTypeInput {
                resource_type: NodeType::Model,
                is_view: true,
                is_custom_materialization: false,
                is_incremental: false,
                full_refresh: false,
                incremental_strategy: None,
                has_unique_key: false,
            })
            .unwrap(),
            ModelExecutionType::View
        );
        assert_eq!(
            execution_type_from_input(&ExecutionTypeInput {
                resource_type: NodeType::Snapshot,
                is_view: false,
                is_custom_materialization: false,
                is_incremental: false,
                full_refresh: false,
                incremental_strategy: None,
                has_unique_key: false,
            })
            .unwrap(),
            ModelExecutionType::Snapshot
        );
        assert_eq!(
            execution_type_from_input(&ExecutionTypeInput {
                resource_type: NodeType::Model,
                is_view: false,
                is_custom_materialization: false,
                is_incremental: true,
                full_refresh: false,
                incremental_strategy: Some("delete+insert".to_string()),
                has_unique_key: true,
            })
            .unwrap(),
            ModelExecutionType::DeleteInsert
        );
        assert_eq!(
            execution_type_from_input(&ExecutionTypeInput {
                resource_type: NodeType::Model,
                is_view: false,
                is_custom_materialization: false,
                is_incremental: true,
                full_refresh: false,
                incremental_strategy: Some("merge".to_string()),
                has_unique_key: false,
            })
            .unwrap(),
            ModelExecutionType::Append
        );
        assert_eq!(
            execution_type_from_input(&ExecutionTypeInput {
                resource_type: NodeType::Model,
                is_view: false,
                is_custom_materialization: false,
                is_incremental: true,
                full_refresh: true,
                incremental_strategy: Some("merge".to_string()),
                has_unique_key: true,
            })
            .unwrap(),
            ModelExecutionType::Full
        );
        assert_eq!(
            execution_type_from_input(&ExecutionTypeInput {
                resource_type: NodeType::Test,
                is_view: false,
                is_custom_materialization: false,
                is_incremental: false,
                full_refresh: false,
                incremental_strategy: None,
                has_unique_key: false,
            })
            .unwrap(),
            ModelExecutionType::DbtDataTest
        );
    }

    #[test]
    fn custom_incremental_strategy_maps_to_dbt_custom() {
        // A user-defined `get_incremental_<name>_sql` strategy has no dedicated
        // execution type, so it is submitted opaquely as DBT_CUSTOM rather than
        // failing and forfeiting dbt State.
        assert_eq!(
            execution_type_from_input(&ExecutionTypeInput {
                resource_type: NodeType::Model,
                is_view: false,
                is_custom_materialization: false,
                is_incremental: true,
                full_refresh: false,
                incremental_strategy: Some("insert_only".to_string()),
                has_unique_key: true,
            })
            .unwrap(),
            ModelExecutionType::DbtCustom
        );

        // Adapter-native strategies the service does not model directly take the
        // same opaque path.
        assert_eq!(
            execution_type_from_input(&ExecutionTypeInput {
                resource_type: NodeType::Model,
                is_view: false,
                is_custom_materialization: false,
                is_incremental: true,
                full_refresh: false,
                incremental_strategy: Some("replace_where".to_string()),
                has_unique_key: false,
            })
            .unwrap(),
            ModelExecutionType::DbtCustom
        );
    }

    #[test]
    fn request_inputs_convert_to_proto_messages() {
        let labels = NodeIdentity {
            name: "orders".to_string(),
            fqn: vec!["pkg".to_string(), "orders".to_string()],
            unique_id: "model.pkg.orders".to_string(),
        }
        .labels();
        let request = SubmitEnrichedSqlRequestInput {
            target_table: Some("analytics.orders".to_string()),
            dialect: "snowflake".to_string(),
            default_catalog: "analytics".to_string(),
            default_schema: Some("marts".to_string()),
            execution_type: ModelExecutionType::Merge,
            sql: "select * from raw.orders".to_string(),
            tables: vec![TableModifiedInfo {
                name: "raw.orders".to_string(),
                last_modified_epoch: Some(123),
            }],
            query_dependencies: vec![QueryDependency {
                name: "raw.order_view".to_string(),
                query: "select * from raw.orders".to_string(),
                default_catalog: "analytics".to_string(),
                default_schema: "raw".to_string(),
            }],
            semantic_extras: HashMap::from([(
                "on_schema_change".to_string(),
                "\"sync_all_columns\"".to_string(),
            )]),
            freshness_tolerance_seconds: 2700,
            lenient_dependencies: vec!["raw.orders".to_string()],
            tolerate_nondeterminism: true,
            labels: labels.clone(),
            clone_time_travel_limit: None,
            clone_table_properties: Some(TableProperties {
                hours_to_expiration: Some(12),
                partition_expiration_days: None,
            }),
            stale_upstream_policy: StaleUpstreamPolicy::Any,
            clone_chain_depth_limit: None,
            dbt_node_state: Some(DbtNodeState {
                node_unique_id: "unique_id".to_string(),
                profile_name: "profile_name".to_string(),
                target_name: "target_name".to_string(),
                project_name: "project_name".to_string(),
                project_id: Some("project_id".to_string()),
                resource_type: "model".to_string(),
                node_hash: "node_hash".to_string(),
                node_body_hash: Some("node_body_hash".to_string()),
                node_configs_hash: Some("node_configs_hash".to_string()),
                node_persisted_descriptions_hash: Some(
                    "node_persisted_descriptions_hash".to_string(),
                ),
                node_macros_hash: Some("node_macros_hash".to_string()),
                node_contract_hash: Some("node_contract_hash".to_string()),
                node_database_representation: None,
            }),
            compare_unrendered_code: true,
            table_namespace: None,
        }
        .into_proto();

        assert!(request.compare_unrendered_code);
        assert_eq!(request.target_table.as_deref(), Some("analytics.orders"));
        assert_eq!(request.default_schema.as_deref(), Some("marts"));
        assert_eq!(request.execution_type, ModelExecutionType::Merge as i32);
        assert_eq!(request.labels, labels);
        assert_eq!(request.tables[0].last_modified_epoch, Some(123));
        assert_eq!(
            request.clone_table_properties.unwrap().hours_to_expiration,
            Some(12)
        );
        assert_eq!(
            request.stale_upstream_policy,
            StaleUpstreamPolicy::Any as i32
        );
        assert_eq!(
            request.dbt_node_state,
            Some(DbtNodeState {
                node_unique_id: "unique_id".to_string(),
                profile_name: "profile_name".to_string(),
                target_name: "target_name".to_string(),
                project_name: "project_name".to_string(),
                project_id: Some("project_id".to_string()),
                resource_type: "model".to_string(),
                node_hash: "node_hash".to_string(),
                node_body_hash: Some("node_body_hash".to_string()),
                node_configs_hash: Some("node_configs_hash".to_string()),
                node_persisted_descriptions_hash: Some(
                    "node_persisted_descriptions_hash".to_string(),
                ),
                node_macros_hash: Some("node_macros_hash".to_string()),
                node_contract_hash: Some("node_contract_hash".to_string()),
                node_database_representation: None
            })
        )
    }

    #[test]
    fn values_request_input_converts_to_proto_message() {
        let request = SubmitValuesRequestInput {
            target_table: "analytics.seed_orders".to_string(),
            dialect: "bigquery".to_string(),
            default_catalog: "analytics".to_string(),
            values_hash: "6c1abef29d8c78fb8e696f94546a3918".to_string(),
            semantic_extras: HashMap::from([("delimiter".to_string(), "\",\"".to_string())]),
            last_modified_epoch: Some(456),
            labels: HashMap::new(),
            clone_time_travel_limit: Some(3600),
            clone_table_properties: None,
            clone_chain_depth_limit: None,
            dbt_node_state: Some(DbtNodeState {
                node_unique_id: "unique_id".to_string(),
                profile_name: "profile_name".to_string(),
                target_name: "target_name".to_string(),
                project_name: "project_name".to_string(),
                project_id: Some("project_id".to_string()),
                resource_type: "model".to_string(),
                node_hash: "node_hash".to_string(),
                node_body_hash: Some("node_body_hash".to_string()),
                node_configs_hash: Some("node_configs_hash".to_string()),
                node_persisted_descriptions_hash: Some(
                    "node_persisted_descriptions_hash".to_string(),
                ),
                node_macros_hash: Some("node_macros_hash".to_string()),
                node_contract_hash: Some("node_contract_hash".to_string()),
                node_database_representation: None,
            }),
            table_namespace: Some("foo".to_string()),
        }
        .into_proto();

        assert_eq!(request.target_table, "analytics.seed_orders");
        assert_eq!(request.values_hash, "6c1abef29d8c78fb8e696f94546a3918");
        assert_eq!(request.last_modified_epoch, Some(456));
        assert_eq!(request.clone_time_travel_limit, Some(3600));
        assert!(request.clone_chain_depth_limit.is_none());
        assert_eq!(
            request.dbt_node_state,
            Some(DbtNodeState {
                node_unique_id: "unique_id".to_string(),
                profile_name: "profile_name".to_string(),
                target_name: "target_name".to_string(),
                project_name: "project_name".to_string(),
                project_id: Some("project_id".to_string()),
                resource_type: "model".to_string(),
                node_hash: "node_hash".to_string(),
                node_body_hash: Some("node_body_hash".to_string()),
                node_configs_hash: Some("node_configs_hash".to_string()),
                node_persisted_descriptions_hash: Some(
                    "node_persisted_descriptions_hash".to_string(),
                ),
                node_macros_hash: Some("node_macros_hash".to_string()),
                node_contract_hash: Some("node_contract_hash".to_string()),
                node_database_representation: None
            })
        );
        assert_eq!(request.table_namespace(), "foo")
    }

    #[test]
    fn sql_record_drops_cache_decision_only_fields() {
        let request = SubmitEnrichedSqlRequestInput {
            target_table: Some("analytics.orders".to_string()),
            dialect: "snowflake".to_string(),
            default_catalog: "analytics".to_string(),
            default_schema: Some("marts".to_string()),
            execution_type: ModelExecutionType::Merge,
            sql: "select * from raw.orders".to_string(),
            tables: vec![TableModifiedInfo {
                name: "raw.orders".to_string(),
                last_modified_epoch: Some(123),
            }],
            query_dependencies: Vec::new(),
            semantic_extras: HashMap::new(),
            freshness_tolerance_seconds: 2700,
            lenient_dependencies: vec!["raw.orders".to_string()],
            tolerate_nondeterminism: true,
            labels: HashMap::from([("dbt_node_name".to_string(), "orders".to_string())]),
            clone_time_travel_limit: Some(3600),
            clone_table_properties: None,
            stale_upstream_policy: StaleUpstreamPolicy::Any,
            clone_chain_depth_limit: None,
            dbt_node_state: None,
            compare_unrendered_code: false,
            table_namespace: None,
        }
        .into_proto();

        let record = sql_execution_record_from_submit_request(
            request,
            ExecutionOutcomeInput {
                last_modified_epoch: Some(456),
                table_type: Some("TABLE".to_string()),
                execution_runtime_ms: Some(789),
            },
            false,
        );

        assert_eq!(record.outcome.unwrap().last_modified_epoch, Some(456));
        let Some(execution_record::Input::EnrichedSql(sql)) = record.input else {
            panic!("expected SQL execution input");
        };
        assert_eq!(sql.target_table.as_deref(), Some("analytics.orders"));
        assert_eq!(sql.default_schema.as_deref(), Some("marts"));
        assert_eq!(sql.execution_type, ModelExecutionType::Merge as i32);
        assert_eq!(sql.labels.get("dbt_node_name").unwrap(), "orders");
        assert!(!sql.from_speculative_submit);
        assert!(sql.dbt_node_state.is_none());
    }

    #[test]
    fn values_record_moves_last_modified_to_outcome() {
        let request = SubmitValuesRequestInput {
            target_table: "analytics.seed_orders".to_string(),
            dialect: "bigquery".to_string(),
            default_catalog: "analytics".to_string(),
            values_hash: "abc123".to_string(),
            semantic_extras: HashMap::new(),
            last_modified_epoch: Some(123),
            labels: HashMap::new(),
            clone_time_travel_limit: Some(3600),
            clone_table_properties: None,
            clone_chain_depth_limit: None,
            dbt_node_state: None,
            table_namespace: None,
        }
        .into_proto();

        let record = values_execution_record_from_submit_request(
            request,
            ExecutionOutcomeInput {
                last_modified_epoch: Some(456),
                table_type: None,
                execution_runtime_ms: None,
            },
        );

        assert_eq!(record.outcome.unwrap().last_modified_epoch, Some(456));
        let Some(execution_record::Input::Values(values)) = record.input else {
            panic!("expected values execution input");
        };
        assert_eq!(values.target_table, "analytics.seed_orders");
        assert_eq!(values.values_hash, "abc123");
    }

    #[test]
    fn clone_request_input_converts_to_proto_message() {
        let labels = HashMap::from([(
            "dbt_node_unique_id".to_string(),
            "model.pkg.orders".to_string(),
        )]);
        let properties = TableProperties {
            hours_to_expiration: Some(12),
            partition_expiration_days: None,
        };
        let request = CloneRequestInput {
            target_table: "dev.analytics.orders".to_string(),
            dialect: "snowflake".to_string(),
            default_catalog: "dev".to_string(),
            execution_type: ModelExecutionType::Merge,
            clone_source_table: "prod.analytics.orders".to_string(),
            clone_source_last_modified_epoch: Some(123),
            labels: labels.clone(),
            clone_source_table_type: Some("table".to_string()),
            table_properties: Some(properties),
            clone_chain_depth_limit: Some(3),
            table_namespace: None,
        }
        .into_proto();

        assert_eq!(request.target_table, "dev.analytics.orders");
        assert_eq!(request.execution_type, ModelExecutionType::Merge as i32);
        assert_eq!(request.clone_source_table, "prod.analytics.orders");
        assert_eq!(request.clone_source_last_modified_epoch, Some(123));
        assert_eq!(request.labels, labels);
        assert_eq!(request.clone_source_table_type.as_deref(), Some("table"));
        assert_eq!(request.table_properties, Some(properties));
        assert_eq!(request.clone_chain_depth_limit, Some(3));
    }
}
