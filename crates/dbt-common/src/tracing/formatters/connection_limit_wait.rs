use dbt_telemetry::ConnectionLimitWait;

use super::duration::format_duration_fixed_width;

fn format_connection_limit_wait_detail(wait: &ConnectionLimitWait) -> String {
    match (wait.active_nodes, wait.active_connections) {
        (Some(active_nodes), Some(active_connections)) => {
            format!(" ({active_nodes} active nodes, {active_connections} active connections)")
        }
        (Some(active_nodes), None) => format!(" ({active_nodes} active nodes)"),
        (None, Some(active_connections)) => {
            format!(" ({active_connections} active connections)")
        }
        (None, None) => String::new(),
    }
}

pub fn format_connection_limit_wait_start(wait: &ConnectionLimitWait) -> String {
    format!(
        "Started connection limit wait{}",
        format_connection_limit_wait_detail(wait)
    )
}

pub fn format_connection_limit_wait_end(
    wait: &ConnectionLimitWait,
    duration: std::time::Duration,
) -> String {
    format!(
        "Finished connection limit wait [{}]{}",
        format_duration_fixed_width(duration),
        format_connection_limit_wait_detail(wait)
    )
}
