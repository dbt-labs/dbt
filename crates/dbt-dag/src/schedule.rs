use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use dbt_common::{io_args::ListOutputFormat, node_selector::SelectExpression};
use dbt_schemas::schemas::Nodes;
#[cfg(debug_assertions)]
use dbt_schemas::schemas::telemetry::NodeType;
use serde_json::Map;

type JsonValue = serde_json::Value;

/// Represents a single node in the list command output
#[derive(Debug, Clone)]
pub struct ListItem {
    /// The unique_id of the node (e.g., "model.my_project.my_model")
    pub unique_id: String,
    /// The formatted content to display
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct Schedule<T> {
    // This is the dependency DAG including selected nodes and frontier nodes
    // - note: all values are also defined as keys
    // - note: T is always a String, namely the unique id of a node
    pub deps: BTreeMap<T, BTreeSet<T>>,
    // Topological sort of selected nodes, followed by frontier nodes (for introspection detection)
    pub sorted_nodes: Vec<T>,
    // Selected nodes before filtering
    pub all_selected_nodes: BTreeSet<T>,
    // The nodes explicitly selected by the user's selection criteria
    pub selected_nodes: BTreeSet<T>,
    // Frontier nodes: dependencies of selected nodes that weren't selected (for schema hydration only)
    pub frontier_nodes: BTreeSet<T>,
    // Sources that share CFQNs with seeds (overlapping sources)
    pub overlapping_sources: BTreeSet<T>,
    // normalized select expressions
    pub select: Option<SelectExpression>,
    // normalized exclude expressions
    pub exclude: Option<SelectExpression>,
}

impl Schedule<String> {
    #[inline]
    #[allow(unused_variables)]
    pub fn debug_assert_invariants(&self, nodes: &Nodes) {
        #[cfg(debug_assertions)]
        {
            if self.selected_nodes.is_empty() {
                return;
            }

            // Ensure no frontier nodes are in selected_nodes
            assert!(
                self.frontier_nodes.is_disjoint(&self.selected_nodes),
                "Schedule invariant violated: frontier_nodes and selected_nodes intersect {:?}",
                self.frontier_nodes
                    .intersection(&self.selected_nodes)
                    .collect::<Vec<_>>()
            );

            for uid in &self.selected_nodes {
                if let Some(node) = nodes.get_node(uid) {
                    // Sources should never be selected; they must be frontier-only
                    assert!(
                        !matches!(node.resource_type(), NodeType::Source),
                        "Schedule invariant violated: source node '{}' is in selected_nodes; sources must be frontier nodes",
                        uid
                    );

                    // Cross-project (external) nodes should never be selected; they must be frontier-only
                    assert!(
                        !node.is_extended_model(),
                        "Schedule invariant violated: external node '{}' is in selected_nodes; external nodes must be frontier nodes",
                        uid
                    );
                }
            }
        }
    }

    /// Show the selected nodes as the type.package.name
    pub fn show_nodes(&self) -> String {
        let mut res = "".to_string();
        if let Some(select) = &self.select {
            res.push_str(&format!("    [--select: {select}]\n"));
        }
        if let Some(exclude) = &self.exclude {
            res.push_str(&format!("    [--exclude: {exclude}]\n"));
        }
        res.push_str(
            &self
                .selected_nodes
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        res.push('\n');
        res
    }

    /// Generate JSON output for a single node based on the specified keys
    fn generate_json_output(node_value: &JsonValue, output_keys: &[String]) -> String {
        let mut json_map = Map::new();

        // If output_keys is empty, use a default set
        // https://github.com/dbt-labs/dbt-core/blob/65d428004a76071d58d6234841bf8b18f9cd3100/core/dbt/task/list.py#L42
        let keys_to_use = if output_keys.is_empty() {
            vec![
                "alias",
                "name",
                "package_name",
                "depends_on",
                "tags",
                "config",
                "resource_type",
                "source_name",
                "original_file_path",
                "unique_id",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
        } else {
            output_keys.to_vec()
        };

        // Convert the serialized node to a map for easier manipulation
        let node_map = node_value
            .as_object()
            .expect("Failed to convert node to map, should not happen");

        // Handle depends_on field specially (remove nodes_with_ref_location)
        if keys_to_use.contains(&"depends_on".to_string())
            && let Some(depends_on_value) = node_map.get("depends_on")
        {
            if let Some(depends_on_obj) = depends_on_value.as_object() {
                let mut cleaned_depends_on = depends_on_obj.clone();
                cleaned_depends_on.remove("nodes_with_ref_location");
                json_map.insert(
                    "depends_on".to_string(),
                    JsonValue::Object(cleaned_depends_on),
                );
            } else {
                json_map.insert("depends_on".to_string(), depends_on_value.clone());
            }
        }

        // Add all other requested keys that exist in the node
        for key in &keys_to_use {
            if key != "depends_on" && node_map.contains_key(key) {
                json_map.insert(key.clone(), node_map[key].clone());
            }
        }

        serde_json::to_string(&json_map).unwrap()
    }

    /// Show the selected nodes in the specified format.
    /// For JSON output, each node is a separate JSON object on a new line,
    /// containing keys specified in `output_keys`.
    pub fn show_dbt_nodes(
        &self,
        nodes: &Nodes,
        output_format: ListOutputFormat,
        output_keys: &[String],
    ) -> Vec<ListItem> {
        let mut res = Vec::new();
        for selected_id in &self.all_selected_nodes.iter().collect::<BTreeSet<_>>() {
            let node = nodes
                .get_node(selected_id)
                .expect("selected node not in manifest");
            let content = match output_format {
                ListOutputFormat::Json => {
                    // Use serialize_keep_none to keep null fields for dbt-core compatibility (omit_none=False)
                    let node_yaml_value = dbt_yaml::to_value(node.serialize_keep_none()).unwrap();
                    let node_value = serde_json::to_value(node_yaml_value).unwrap();
                    Self::generate_json_output(&node_value, output_keys)
                }
                ListOutputFormat::Selector => node.selector_string(),
                ListOutputFormat::Name => node.search_name(),
                ListOutputFormat::Path => node.file_path(),
            };
            res.push(ListItem {
                unique_id: (*selected_id).to_string(),
                content,
            });
        }
        res
    }

    pub fn modify_for_local_execution(&mut self) {
        // 1. Move all frontier nodes that are seeds to the selected nodes
        for unique_id in self.frontier_nodes.clone().iter() {
            if unique_id.starts_with("seed.") {
                self.selected_nodes.insert(unique_id.clone());
                self.frontier_nodes.remove(unique_id);
            }
        }
    }
}

impl fmt::Display for Schedule<String> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let show_frontier = !self.frontier_nodes.is_empty();
        let max_key_len = self.deps.keys().map(|key| key.len()).max().unwrap_or(0);

        if show_frontier {
            writeln!(
                f,
                "{:<width$} | {:<8} | Depends-on",
                "Unique Id",
                "Frontier",
                width = max_key_len
            )?;
        } else {
            writeln!(
                f,
                "{:<width$} |  Depends-on",
                "Unique Id",
                width = max_key_len
            )?;
        }
        let total_width = max_key_len + 3 + 8 + 3 + max_key_len; // 3 is for the spaces and separators

        writeln!(f, "{}", "-".repeat(total_width))?;

        for key in &self.sorted_nodes {
            if let Some(value_set) = self.deps.get(key) {
                let values_str = value_set
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                if show_frontier {
                    let frontier_marker = if self.frontier_nodes.contains(key) {
                        "*"
                    } else {
                        ""
                    };
                    writeln!(
                        f,
                        "{key:<max_key_len$} | {frontier_marker:<8} | {values_str}"
                    )?;
                } else {
                    writeln!(f, "{key:<max_key_len$} | {values_str}")?;
                }
            }
        }
        Ok(())
    }
}
