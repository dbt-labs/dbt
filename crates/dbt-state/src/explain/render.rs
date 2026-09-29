use std::collections::HashMap;

use crate::proto::query_cache::{
    ExplainBadge, ExplainLine, ExplainMarker, ExplainMessageEntry, GetExplainMessagesResponse,
    SubmitSqlResultType,
};

use super::types::{StateExplainOptions, StateExplainRecord, StateExplainRunStart};

pub fn render_explain_records(
    records: &[StateExplainRecord],
    options: &StateExplainOptions,
) -> String {
    if records.is_empty() {
        if let Some(selectors) = options
            .select
            .as_deref()
            .filter(|selectors| !selectors.is_empty())
        {
            return format!("No nodes found matching '{}'", selectors.join(" "));
        }
        return "No dbt State explain records found.".to_string();
    }

    records
        .iter()
        .map(|record| record.render(options.verbose))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render state explain records with matching service-side explain messages.
pub fn render_merged_explain_records(
    records: &[StateExplainRecord],
    service_response: Option<&GetExplainMessagesResponse>,
    options: &StateExplainOptions,
) -> String {
    if records.is_empty() {
        return render_explain_records(records, options);
    }

    let messages_by_id: HashMap<_, _> = service_response
        .into_iter()
        .flat_map(|response| response.messages.iter())
        .map(|message| (message.execution_decision_id.as_str(), message))
        .collect();

    records
        .iter()
        .map(|record| {
            record
                .execution_decision_id
                .as_deref()
                .and_then(|id| messages_by_id.get(id).copied())
                .map(|message| {
                    render_service_explain_message(&record.node_unique_id, message, options.verbose)
                })
                .unwrap_or_else(|| record.render(options.verbose))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn render_explain_output(
    records: &[StateExplainRecord],
    service_response: Option<&GetExplainMessagesResponse>,
    run_start: Option<&StateExplainRunStart>,
    options: &StateExplainOptions,
) -> String {
    let output = render_merged_explain_records(records, service_response, options);
    if !options.verbose {
        return output;
    }
    let Some(run_start) = run_start else {
        return output;
    };

    let config = &run_start.run_config;
    let mut lines = vec![
        "Run configuration:".to_string(),
        format!("  - started at: {}", run_start.start_timestamp_utc),
        format!("  - profile: {}", config.profile_name),
        format!("  - target: {}", config.target_name),
        format!("  - defer to target: {}", config.defer_to_target),
        format!(
            "  - freshness tolerance: {} seconds",
            config.freshness_tolerance_seconds
        ),
        format!(
            "  - tolerate nondeterminism: {}",
            config.tolerate_nondeterminism
        ),
        format!(
            "  - clone incremental in dev: {}",
            config.clone_incremental_in_dev
        ),
        format!(
            "  - metadata cache TTL: {} seconds",
            config.metadata_cache_ttl_seconds
        ),
    ];
    if let Some(org_id) = &config.org_id {
        lines.push(format!("  - organization: {org_id}"));
    }
    if let Some(override_value) = &config.snowflake_get_view_ddl_override {
        lines.push(format!(
            "  - Snowflake get-view-DDL override: {override_value}"
        ));
    }
    if !config.select.is_empty() {
        lines.push(format!("  - select: {}", config.select.join(" ")));
    }
    if !config.exclude.is_empty() {
        lines.push(format!("  - exclude: {}", config.exclude.join(" ")));
    }
    lines.push(output);
    lines.join("\n")
}

/// Render a dbt State service explain response as plain text.
pub fn render_service_explain_response(response: &GetExplainMessagesResponse) -> String {
    render_service_explain_messages(&response.messages)
}

/// Render dbt State service explain messages as plain text.
pub fn render_service_explain_messages(messages: &[ExplainMessageEntry]) -> String {
    if messages.is_empty() {
        return "No dbt State service explain messages found.".to_string();
    }

    messages
        .iter()
        .map(|message| {
            render_service_explain_message(&message.execution_decision_id, message, true)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_service_explain_message(
    label: &str,
    message: &ExplainMessageEntry,
    verbose: bool,
) -> String {
    let mut lines = vec![format!(
        "{} {} - {}",
        submit_sql_result_type_name(message.decision),
        label,
        message.decision_description
    )];
    if verbose {
        for line in &message.explain_lines {
            render_service_explain_line(line, 1, &mut lines);
        }
    }
    lines.join("\n")
}

fn render_service_explain_line(line: &ExplainLine, depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    let suffix = match (
        line.marker.and_then(explain_marker_name),
        line.badge.and_then(explain_badge_name),
    ) {
        (Some(marker), Some(badge)) => format!(" [{marker}, {badge}]"),
        (Some(marker), None) => format!(" [{marker}]"),
        (None, Some(badge)) => format!(" [{badge}]"),
        (None, None) => String::new(),
    };
    lines.push(format!("{indent}- {}{suffix}", line.text));
    for child in &line.children {
        render_service_explain_line(child, depth + 1, lines);
    }
}

fn submit_sql_result_type_name(value: i32) -> String {
    SubmitSqlResultType::try_from(value)
        .map(|decision| decision.as_str_name().to_string())
        .unwrap_or_else(|_| format!("UNKNOWN_DECISION({value})"))
}

fn explain_marker_name(value: i32) -> Option<String> {
    match ExplainMarker::try_from(value) {
        Ok(ExplainMarker::EmUnspecified) => None,
        Ok(marker) => Some(marker.as_str_name().to_string()),
        Err(_) => Some(format!("UNKNOWN_MARKER({value})")),
    }
}

fn explain_badge_name(value: i32) -> Option<String> {
    match ExplainBadge::try_from(value) {
        Ok(ExplainBadge::EbUnspecified) => None,
        Ok(badge) => Some(badge.as_str_name().to_string()),
        Err(_) => Some(format!("UNKNOWN_BADGE({value})")),
    }
}
