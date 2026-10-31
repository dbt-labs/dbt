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
