use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use dbt_adapter::response::AdapterResponse;
use dbt_common::stats::Stat;
use dbt_schemas::schemas::{ContextRunResult, TimingInfo};
use dbt_schemas::stats::Stats;

type YmlValue = dbt_yaml::Value;

/// Flatten a captured `store_result('main')` response into the flat
/// `adapter_response` map `run_results.json` expects, falling back to the
/// rows-affected-only map for nodes that never stored a main result.
fn adapter_response_map(
    stat: &Stat,
    adapter_responses: &HashMap<String, AdapterResponse>,
) -> BTreeMap<String, YmlValue> {
    if let Some(response) = adapter_responses.get(&stat.unique_id)
        && let Ok(value) = dbt_yaml::to_value(response)
        && let Some(mapping) = value.as_mapping()
    {
        return mapping
            .iter()
            .filter_map(|(key, value)| key.as_str().map(|key| (key.to_string(), value.clone())))
            .collect();
    }

    let mut map = BTreeMap::new();
    if let Some(rows_affected) = stat.rows_affected
        && let Ok(value) = dbt_yaml::to_value(rows_affected)
    {
        map.insert("rows_affected".to_string(), value);
    }
    map
}

pub fn generate_run_results(
    stat: &Stat,
    stats: &Stats,
    adapter_responses: &HashMap<String, AdapterResponse>,
) -> ContextRunResult {
    let status = stat.result_status_string();
    let execution_time = stat.get_duration().as_secs_f64();
    let started_at: DateTime<Utc> = DateTime::from(stat.start_time);
    let completed_at: DateTime<Utc> = DateTime::from(stat.end_time);

    // TODO: Differentiate between compile and execute timing
    let timing = vec![
        TimingInfo {
            name: "compile".to_string(),
            started_at: Some(started_at),
            completed_at: Some(completed_at),
        },
        TimingInfo {
            name: "execute".to_string(),
            started_at: Some(started_at),
            completed_at: Some(completed_at),
        },
    ];

    let nodes = stats
        .nodes
        .as_ref()
        .expect("stats should have nodes for results generation");
    let node_arc = nodes.get_node_owned(&stat.unique_id);

    // Determine failures for tests
    let failures =
        if stat.unique_id.starts_with("test.") || stat.unique_id.starts_with("unit_test.") {
            stat.num_rows.map(|n| n as i64)
        } else {
            None
        };

    // Get static_analysis_off_reason from the node if available
    let static_analysis_off_reason = node_arc
        .as_ref()
        .and_then(|node| node.static_analysis_off_reason());

    let batch_results = stats.batch_results.get(&stat.unique_id).cloned();

    let compiled_code = stats.compiled_code.get(&stat.unique_id).cloned();

    ContextRunResult {
        status,
        timing,
        thread_id: stat.thread_id.clone(),
        execution_time,
        adapter_response: adapter_response_map(stat, adapter_responses),
        message: stat.message.clone(),
        failures,
        node: node_arc,
        unique_id: stat.unique_id.clone(),
        batch_results,
        compiled_code,
        static_analysis_off_reason,
    }
}
