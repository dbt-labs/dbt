//! Databricks-specific helpers for the ADBC engine (`super::adbc`).

use dbt_schemas::schemas::AdapterAttr;
use minijinja::State;
use serde::Deserialize;

/// Node-agnostic probe for `__adapter_attr__` (matches v1's
/// `config.get("databricks_compute")`); models/snapshots/data tests all
/// carry this. TODO: seeds don't yet — needs a `databricks_attr` on `DbtSeed`.
#[derive(Deserialize)]
struct AdapterAttrProbe {
    // Must be the literal, non-`Option` `__adapter_attr__` field — dbt_yaml's
    // dunder-key flatten only redelivers into that exact shape (see nodes.rs).
    #[serde(default)]
    __adapter_attr__: AdapterAttr,
}

/// Get the Databricks compute engine configured for this model/snapshot
///
/// https://docs.getdbt.com/reference/resource-configs/databricks-configs#selecting-compute-per-model
pub(crate) fn compute_from_state(state: &State) -> Option<String> {
    let model = state.lookup("model", &[])?;
    // Only genuine malformation reaches here; log instead of silently falling
    // back to the default compute.
    let yaml_node = match dbt_yaml::to_value(&model) {
        Ok(node) => node,
        Err(e) => {
            tracing::debug!("databricks_compute: could not serialize node config: {e}");
            return None;
        }
    };
    let probe = match AdapterAttrProbe::deserialize(&yaml_node) {
        Ok(probe) => probe,
        Err(e) => {
            tracing::debug!("databricks_compute: could not read __adapter_attr__: {e}");
            return None;
        }
    };
    probe.__adapter_attr__.databricks_attr?.databricks_compute
}
