use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

pub(super) const STATE_EXPLAIN_RECORD_VERSION: u16 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateExplainOptions {
    /// Project directory used to resolve relative log paths.
    pub project_dir: PathBuf,
    /// Optional dbt log root from the global `--log-path` argument.
    pub log_path: Option<PathBuf>,
    /// Optional explicit decision log file to explain.
    pub log_file: Option<PathBuf>,
    /// Optional node selection passed through from the global `--select` argument.
    pub select: Option<Vec<String>>,
    /// Optional node exclusion passed through from the global `--exclude` argument.
    pub exclude: Option<Vec<String>>,
    /// Whether dbt State service explain messages should be requested.
    pub manage_state: bool,
    /// Whether to include lower-level decision details.
    pub verbose: bool,
}

/// One entry in a Fusion-native dbt State explain log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entry_type", rename_all = "snake_case")]
pub enum StateExplainLogRecord {
    /// Run-level context written once at the start of a dbt invocation.
    RunStart(StateExplainRunStart),
    /// Node-level context written after a node's state decision is known.
    Node(StateExplainNode),
}

/// Run-level context for dbt State explain output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateExplainRunStart {
    /// UTC timestamp for when the run started.
    pub start_timestamp_utc: String,
    /// Configuration values needed to render explain output later.
    pub run_config: StateExplainRunConfig,
}

/// Run configuration captured in a dbt State explain log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateExplainRunConfig {
    /// Optional dbt State organization id.
    pub org_id: Option<String>,
    /// Target used for dbt State auto-deferral.
    pub defer_to_target: String,
    /// Freshness tolerance, in seconds, used by dbt State decisions.
    pub freshness_tolerance_seconds: i64,
    /// Whether dbt State tolerated non-deterministic SQL.
    pub tolerate_nondeterminism: bool,
    /// Clone policy configured for incremental models in development.
    pub clone_incremental_in_dev: String,
    /// Metadata cache TTL, in seconds.
    pub metadata_cache_ttl_seconds: i64,
    /// Optional Snowflake view DDL override.
    pub snowflake_get_view_ddl_override: Option<String>,
    /// dbt profile name used by the run.
    pub profile_name: String,
    /// dbt target name used by the run.
    pub target_name: String,
    /// Selectors passed to the run.
    #[serde(default)]
    pub select: Vec<String>,
    /// Exclusions passed to the run.
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Node-level context for dbt State explain output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateExplainNode {
    /// Unique id for the node.
    pub node_unique_id: String,
    /// Display name for the node.
    pub node_name: String,
    /// Context needed to render node-specific explain output.
    pub node_info: StateExplainNodeInfo,
    /// Optional service-side decision id for fetching detailed explain output.
    pub execution_decision_id: Option<String>,
}

/// Node attributes captured in a dbt State explain log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateExplainNodeInfo {
    /// Fully-qualified relation name for the node.
    pub fqn: String,
    /// dbt resource type, such as model, seed, or snapshot.
    pub node_resource_type: String,
    /// Whether the node was materialized as a view.
    pub is_view: bool,
    /// Whether the node was materialized as a table.
    pub is_table: bool,
    /// Whether the node was inlined rather than materialized in the warehouse,
    /// as ephemeral and inline models are.
    #[serde(default)]
    pub is_ephemeral: bool,
    /// Whether the node was an incremental model or snapshot.
    pub is_incremental_or_snapshot: bool,
    /// Whether the node was run in full-refresh mode.
    pub is_full_refresh: bool,
    /// Optional dev clone context.
    pub dev_clone: Option<StateExplainDevClone>,
    /// Deferrals from original relation name to deferred relation FQN.
    #[serde(default)]
    pub deferrals: HashMap<String, String>,
}

impl Default for StateExplainNodeInfo {
    fn default() -> Self {
        Self {
            fqn: String::new(),
            node_resource_type: String::new(),
            is_view: false,
            is_table: true,
            is_ephemeral: false,
            is_incremental_or_snapshot: false,
            is_full_refresh: false,
            dev_clone: None,
            deferrals: HashMap::new(),
        }
    }
}

/// Dev clone context captured for a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateExplainDevClone {
    /// Fully-qualified source relation cloned from.
    pub source_table_fqn: String,
    /// Fully-qualified target relation cloned to.
    pub target_table_fqn: String,
}

/// Parsed Fusion-native dbt State explain log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateExplainLog {
    /// Run-level context, if present in the log.
    pub run_start: Option<StateExplainRunStart>,
    /// Node-level explain entries in log order.
    pub nodes: Vec<StateExplainNode>,
}

/// One cached-state decision recorded for `dbt state explain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateExplainRecord {
    /// Schema version for the JSONL record.
    pub version: u16,
    /// Unique id for the node whose cache decision was recorded.
    pub node_unique_id: String,
    /// Optional service-side decision id for fetching detailed explain output.
    #[serde(default)]
    pub execution_decision_id: Option<String>,
    /// Decision status, such as `hit` or `miss`.
    pub status: StateExplainStatus,
    /// Human-readable reason for the decision.
    pub reason: String,
    /// Optional lower-level details to show in verbose output.
    #[serde(default)]
    pub details: Vec<String>,
}

/// Cache decision status for a state explain record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateExplainStatus {
    /// The cached state was usable.
    Hit,
    /// The cached state was not usable.
    Miss,
    /// Cache lookup was skipped.
    Skipped,
    /// No cache decision was available in the local explain log.
    Unknown,
}

impl StateExplainStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Hit => "HIT",
            Self::Miss => "MISS",
            Self::Skipped => "SKIP",
            Self::Unknown => "UNKNOWN",
        }
    }
}

impl StateExplainRecord {
    /// Build a local fallback record for a structured explain node.
    pub fn fallback_from_node(node: StateExplainNode) -> Self {
        let details = node.local_details();
        Self {
            version: STATE_EXPLAIN_RECORD_VERSION,
            node_unique_id: node.node_unique_id,
            execution_decision_id: node.execution_decision_id,
            status: StateExplainStatus::Unknown,
            reason: "dbt State explain details unavailable".to_string(),
            details,
        }
    }

    pub(super) fn render(&self, verbose: bool) -> String {
        let mut lines = vec![format!(
            "{} {} - {}",
            self.status.label(),
            self.node_unique_id,
            self.reason
        )];
        if verbose {
            lines.extend(self.details.iter().map(|detail| format!("  - {detail}")));
        }
        lines.join("\n")
    }
}

impl StateExplainNode {
    fn local_details(&self) -> Vec<String> {
        let mut details = vec![format!("name: {}", self.node_name)];
        if !self.node_info.fqn.is_empty() {
            details.push(format!("relation: {}", self.node_info.fqn));
        }
        if !self.node_info.node_resource_type.is_empty() {
            details.push(format!(
                "resource type: {}",
                self.node_info.node_resource_type
            ));
        }
        details.push(format!(
            "materialization: {}",
            self.node_info.materialization_label()
        ));
        details.push(format!("full refresh: {}", self.node_info.is_full_refresh));

        if let Some(dev_clone) = &self.node_info.dev_clone {
            details.push(format!(
                "dev clone: {} -> {}",
                dev_clone.source_table_fqn, dev_clone.target_table_fqn
            ));
        }

        let mut deferred_sources: Vec<_> = self.node_info.deferrals.keys().collect();
        deferred_sources.sort();
        details.extend(deferred_sources.into_iter().map(|source| {
            let target = &self.node_info.deferrals[source];
            format!("deferred relation: {source} -> {target}")
        }));
        details
    }
}

impl StateExplainNodeInfo {
    fn materialization_label(&self) -> &'static str {
        if self.node_resource_type == "snapshot" {
            "snapshot"
        } else if self.node_resource_type == "seed" {
            "seed"
        } else if self.is_ephemeral {
            "ephemeral"
        } else if self.is_incremental_or_snapshot {
            "incremental"
        } else if self.is_view {
            "view"
        } else if self.is_table {
            "table"
        } else {
            "other"
        }
    }
}
