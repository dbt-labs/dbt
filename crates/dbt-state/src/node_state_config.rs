use dbt_common::tracing::dbt_emit::emit_trace_log_message;
use dbt_schemas::schemas::{DbtModel, DbtSnapshot, DbtTest, InternalDbtNodeAttributes};

/// `compare_unrendered_code`, honored by models and snapshots (via `ModelState`) and data tests
/// (via `DataTestState`).
fn compare_unrendered_code_for_node(node: &dyn InternalDbtNodeAttributes) -> Option<bool> {
    let any = node.as_any();
    if let Some(model) = any.downcast_ref::<DbtModel>() {
        return model
            .__model_attr__
            .state
            .as_ref()
            .and_then(|s| s.compare_unrendered_code);
    }
    if let Some(snapshot) = any.downcast_ref::<DbtSnapshot>() {
        return snapshot
            .__snapshot_attr__
            .state
            .as_ref()
            .and_then(|s| s.compare_unrendered_code);
    }
    any.downcast_ref::<DbtTest>()
        .and_then(|test| test.__test_attr__.state.as_ref())
        .and_then(|state| state.compare_unrendered_code)
}

/// Per-node override for the service's `compare_unrendered_code` wire flag. The node config
/// wins over the service default, otherwise the default stands.
pub fn resolve_compare_unrendered_code(
    node: &dyn InternalDbtNodeAttributes,
    service_default: bool,
) -> bool {
    let node_override = compare_unrendered_code_for_node(node);
    let resolved = node_override.unwrap_or(service_default);

    // Logs the override separately from the result so a surprising `false` distinguishes
    // "the node never set it" from "the service default lost to an explicit node value".
    emit_trace_log_message(|| {
        format!(
            "dbt State compare_unrendered_code={resolved} for node {} (node config {}, service default {service_default})",
            node.unique_id(),
            match node_override {
                Some(v) => v.to_string(),
                None => "unset".to_owned(),
            },
        )
    });

    resolved
}

/// `compare_selected_columns`, honored by models and snapshots (via `ModelState`) and data tests
/// (via `DataTestState`).
fn compare_selected_columns_for_node(node: &dyn InternalDbtNodeAttributes) -> Option<bool> {
    let any = node.as_any();
    if let Some(model) = any.downcast_ref::<DbtModel>() {
        return model
            .__model_attr__
            .state
            .as_ref()
            .and_then(|s| s.compare_selected_columns);
    }
    if let Some(snapshot) = any.downcast_ref::<DbtSnapshot>() {
        return snapshot
            .__snapshot_attr__
            .state
            .as_ref()
            .and_then(|s| s.compare_selected_columns);
    }
    any.downcast_ref::<DbtTest>()
        .and_then(|test| test.__test_attr__.state.as_ref())
        .and_then(|state| state.compare_selected_columns)
}

/// Per-node override for the service's `compare_selected_columns` default. The node config
/// wins over the service default, otherwise the default stands.
pub fn resolve_compare_selected_columns(
    node: &dyn InternalDbtNodeAttributes,
    service_default: bool,
) -> bool {
    let node_override = compare_selected_columns_for_node(node);
    let resolved = node_override.unwrap_or(service_default);

    emit_trace_log_message(|| {
        format!(
            "dbt State compare_selected_columns={resolved} for node {} (node config {}, service default {service_default})",
            node.unique_id(),
            match node_override {
                Some(v) => v.to_string(),
                None => "unset".to_owned(),
            },
        )
    });

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::properties::{DataTestState, ModelState};

    fn model_state(compare_selected_columns: Option<bool>) -> ModelState {
        ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
            ignore_external_modifications: None,
            compare_selected_columns,
        }
    }

    #[test]
    fn resolve_compare_selected_columns_prefers_node_over_default() {
        let mut model = DbtModel::default();
        model.__model_attr__.state = Some(model_state(Some(false)));
        assert!(!resolve_compare_selected_columns(&model, true));

        let mut test = DbtTest::default();
        test.__test_attr__.state = Some(DataTestState {
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            compare_unrendered_code: None,
            compare_selected_columns: Some(true),
        });
        assert!(resolve_compare_selected_columns(&test, false));

        let mut snapshot = DbtSnapshot::default();
        snapshot.__snapshot_attr__.state = Some(model_state(None));
        assert!(resolve_compare_selected_columns(&snapshot, true));
    }
}
