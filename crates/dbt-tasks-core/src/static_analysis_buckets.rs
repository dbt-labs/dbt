use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use dbt_common::io_args::StaticAnalysisKind;
use dbt_schema_store::CanonicalFqn;
use dbt_schemas::schemas::{IntrospectionKind, Nodes};

use crate::RunTasksArgs;

pub trait StaticAnalysisBuckets: Send + Sync {
    fn global_static_analysis(&self) -> Option<StaticAnalysisKind>;
    fn deferred_unique_ids(&self) -> &HashMap<CanonicalFqn, String>;

    fn in_off_closure(&self, node_id: &str) -> bool;
    fn in_baseline_closure(&self, node_id: &str) -> bool;
    fn in_dynamic_closure(&self, node_id: &str) -> bool;
    fn in_baseline_or_off_closure(&self, node_id: &str) -> bool {
        self.in_baseline_closure(node_id) || self.in_off_closure(node_id)
    }

    fn dynamic_node(&self, node_id: &str) -> Option<IntrospectionKind>;
    fn has_dynamic_closure(&self) -> bool;

    fn will_build_phased_task_graph(&self, arg: &RunTasksArgs, task_nodes: &Nodes);
    fn did_build_phased_task_graph(
        &self,
        arg: &RunTasksArgs,
        nodes_with_no_tasks: &BTreeSet<String>,
    );
}

/// Builds per-source refresh intervals from node configurations.
pub fn build_refresh_intervals(
    unique_ids: &BTreeSet<String>,
    nodes: &Nodes,
) -> HashMap<String, Option<Duration>> {
    unique_ids
        .iter()
        .filter_map(|unique_id| {
            let node = nodes.get_node(unique_id)?;
            let interval = node.schema_refresh_interval().and_then(|i| i.as_duration());
            Some((unique_id.clone(), interval))
        })
        .collect()
}
