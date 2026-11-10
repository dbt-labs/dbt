use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::{collections::BTreeMap, sync::Arc};

use super::{
    InternalDbtNodeAttributes, TimingInfo,
    common::{FreshnessDefinition, FreshnessStatus},
};

fn serialize_internal_dbt_node<S>(
    node: &Option<Arc<dyn InternalDbtNodeAttributes>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match node {
        Some(node) => node.serialize_keep_none().serialize(serializer),
        None => serializer.serialize_none(),
    }
}

/// Metadata about the dbt run invocation.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct FreshnessResultsMetadata {
    pub dbt_schema_version: String,
    pub dbt_version: String,
    pub generated_at: DateTime<Utc>,
    pub invocation_id: String,
    /// Timestamp when the invocation started, if available.
    pub invocation_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Result for a single source freshness check.
///
/// Used both for the sources.json artifact (where `node` is `None` and omitted) and
/// for the Jinja `on_run_end` context (where `node` is populated, matching dbt-core behavior).
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FreshnessResultsNode {
    pub unique_id: String,
    /// Populated only in `freshness.json`; `sources.json`'s shape must not change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    pub max_loaded_at: DateTime<Utc>,
    pub snapshotted_at: DateTime<Utc>,
    pub max_loaded_at_time_ago_in_s: f64,
    pub status: FreshnessStatus,
    pub criteria: FreshnessDefinition,
    pub adapter_response: BTreeMap<String, String>,
    pub timing: Vec<TimingInfo>,
    pub thread_id: String,
    pub execution_time: f64,
    /// The source node that was checked for freshness.
    /// Populated when passed to `on_run_end` hooks; `None` (and omitted) in the artifact.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        skip_deserializing,
        serialize_with = "serialize_internal_dbt_node"
    )]
    pub node: Option<Arc<dyn InternalDbtNodeAttributes>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_node(resource_type: Option<&str>) -> FreshnessResultsNode {
        FreshnessResultsNode {
            unique_id: "source.pkg.raw.orders".to_string(),
            resource_type: resource_type.map(str::to_string),
            max_loaded_at: DateTime::from_timestamp_nanos(0),
            snapshotted_at: DateTime::from_timestamp_nanos(0),
            max_loaded_at_time_ago_in_s: 0.0,
            status: FreshnessStatus::Pass,
            criteria: FreshnessDefinition::default(),
            adapter_response: BTreeMap::new(),
            timing: vec![],
            thread_id: "Thread-1".to_string(),
            execution_time: 0.0,
            node: None,
        }
    }

    /// Omitted entirely, not `null`: `sources.json` must stay byte-identical.
    #[test]
    fn resource_type_is_omitted_when_unset() {
        let json = serde_json::to_string(&result_node(None)).unwrap();
        assert!(
            !json.contains("resource_type"),
            "resource_type leaked into sources.json shape: {json}"
        );
    }

    #[test]
    fn resource_type_is_serialized_when_set() {
        let json = serde_json::to_string(&result_node(Some("model"))).unwrap();
        assert!(json.contains(r#""resource_type":"model""#), "got {json}");
    }

    #[test]
    fn resource_type_defaults_to_none_when_absent() {
        let json = serde_json::to_string(&result_node(None)).unwrap();
        let parsed: FreshnessResultsNode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.resource_type, None);
    }
}

/// Represents the structure of the sources.json artifact.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FreshnessResultsArtifact {
    /// Metadata about the dbt invocation.
    pub metadata: FreshnessResultsMetadata,
    /// List of results for each executed node.
    pub results: Vec<FreshnessResultsNode>,
    /// Total elapsed time for the entire dbt invocation in seconds.
    pub elapsed_time: f64,
}
