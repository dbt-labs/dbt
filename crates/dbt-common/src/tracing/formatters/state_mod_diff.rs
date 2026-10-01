use dbt_telemetry::StateModifiedDiff;

pub fn format_state_modified_diff_lines(diff: &StateModifiedDiff) -> Vec<String> {
    let unique_id = diff.unique_id.as_deref().unwrap_or("unknown");
    let mut lines = vec![format!(
        "[state_mod_diff] unique_id={}, node_type_or_category={}, check={}",
        unique_id, diff.node_type_or_category, diff.check
    )];

    if let Some(self_value) = diff.self_value.as_deref() {
        lines.push(format!("   self:  {:?}", self_value));
    }

    if let Some(other_value) = diff.other_value.as_deref() {
        lines.push(format!("   other:  {:?}", other_value));
    }

    lines
}
