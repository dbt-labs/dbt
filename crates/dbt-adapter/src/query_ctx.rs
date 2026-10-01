//! Util methods for creating query context.

use crate::errors::AdapterResult;

use dbt_adapter_core::DBT_EXECUTION_PHASES;
use dbt_adbc::QueryCtx;
use dbt_schemas::schemas::{
    CommonAttributes, DbtModel, DbtSeed, DbtSnapshot, DbtTest, DbtUnitTest, manifest::DbtOperation,
};
use minijinja::{
    State,
    constants::{CURRENT_EXECUTION_PHASE, TARGET_UNIQUE_ID},
};
use serde::Deserialize;

pub fn query_ctx_from_state(state: &State) -> AdapterResult<QueryCtx> {
    // TODO: The following should really be an error, but
    // our tests (functional tests in particular) do not
    // set anything about model in the state.
    //
    // TODO: The following should be an error but there
    // are tests that do not include model.
    //return Err(AdapterError::new(
    //AdapterErrorKind::Configuration,
    //"Missing model in the state",
    //));
    let mut query = QueryCtx::default();
    // TODO: use node_metadata_from_state
    let common_attrs = common_attrs_from_state(state);
    if let Some(attrs) = &common_attrs {
        query = query.with_node_id(attrs.unique_id.clone());
        query = query.with_project_name(attrs.package_name.clone());
        query = query.with_model_name(attrs.name.clone());
    }
    if let Some(target_unique_id) = target_unique_id_from_state(state) {
        query = query.with_target_unique_id(target_unique_id);
    }
    if let Some(phase) = execution_phase_from_state(state) {
        query = query.with_phase(phase);
    }
    if let Some(run_id) = invocation_id_from_state(state) {
        query = query.with_run_id(run_id);
    }
    Ok(query)
}

fn common_attrs_from_state(state: &State) -> Option<CommonAttributes> {
    let node = state.lookup("model", &[]).as_ref()?.clone();
    // all deserialization must go through yaml value
    // should this be a .ok?
    let yaml_node = dbt_yaml::to_value(&node)
        .map_err(|e| {
            minijinja::Error::new(minijinja::ErrorKind::SerdeDeserializeError, e.to_string())
        })
        .ok()?;

    if let Ok(model) = DbtModel::deserialize(&yaml_node) {
        Some(model.__common_attr__)
    } else if let Ok(test) = DbtTest::deserialize(&yaml_node) {
        Some(test.__common_attr__)
    } else if let Ok(snapshot) = DbtSnapshot::deserialize(&yaml_node) {
        Some(snapshot.__common_attr__)
    } else if let Ok(seed) = DbtSeed::deserialize(&yaml_node) {
        Some(seed.__common_attr__)
    } else if let Ok(unit_test) = DbtUnitTest::deserialize(&yaml_node) {
        Some(unit_test.__common_attr__)
    } else if let Ok(unit_test) = DbtOperation::deserialize(&yaml_node) {
        Some(unit_test.__common_attr__)
    } else {
        None
    }
}

pub fn node_id_from_state(state: &State) -> Option<String> {
    common_attrs_from_state(state).map(|attrs| attrs.unique_id)
}

/// Read the dbt invocation id, the same key the BigQuery job-label path uses.
fn invocation_id_from_state(state: &State) -> Option<String> {
    state
        .lookup("invocation_id", &[])
        .and_then(|value| value.as_str().map(|s| s.to_string()))
}

/// Read the `TARGET_UNIQUE_ID` Jinja key, the unique id the current adapter calls target.
///
/// This is the node's own unique id everywhere except where a node materializes an additional
/// relation on its own behalf and sets the key to a distinct id for it. Deliberately kept
/// separate from [`node_id_from_state`] so that identity never overwrites the node's own — see
/// [`QueryCtx::target_unique_id`].
pub fn target_unique_id_from_state(state: &State) -> Option<String> {
    state
        .lookup(TARGET_UNIQUE_ID, &[])?
        .as_str()
        .map(|s| s.to_string())
}

pub fn execution_phase_from_state(state: &State) -> Option<&'static str> {
    let value = state.lookup(CURRENT_EXECUTION_PHASE, &[])?;
    let s = value.as_str()?;
    DBT_EXECUTION_PHASES
        .iter()
        .position(|&p| p == s)
        .map(|idx| DBT_EXECUTION_PHASES[idx])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use minijinja::{Environment, Value};

    use super::*;
    use crate::LATEST_VERSION_POINTER_SUFFIX;

    /// Build a `State` carrying `ctx` and hand it to `f`, mirroring how the Jinja `execute()`
    /// builtin reaches `query_ctx_from_state`.
    fn with_state(ctx: BTreeMap<String, Value>, f: impl FnOnce(&State)) {
        let env = Environment::new();
        let template = env.template_from_str("").expect("state template");
        let state = template
            .eval_to_state(ctx, &[])
            .expect("state should build");
        f(&state);
    }

    /// Regression guard: a node's identity must survive `TARGET_UNIQUE_ID` diverging from it.
    ///
    /// `materialize_latest_version_pointer` sets `TARGET_UNIQUE_ID` to a synthetic pointer id so
    /// the pointer view's adapter calls can be told apart from the base model's, and it once did
    /// so by overwriting the cloned model's `unique_id` as well. That leaked the synthetic id
    /// into everything keyed on node identity — including cross-version record/replay recording
    /// keys, which then could not find the events earlier versions had recorded under the real
    /// node id. The two ids must stay on separate channels.
    #[test]
    fn target_unique_id_does_not_displace_the_node_id() {
        let node_id = "model.pkg.my_model.v1";
        let pointer_id = format!("{node_id}{LATEST_VERSION_POINTER_SUFFIX}");

        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = node_id.to_string();
        let yaml_model = dbt_yaml::to_value(&model).expect("model yaml");

        let mut ctx: BTreeMap<String, Value> = BTreeMap::new();
        ctx.insert("model".to_string(), Value::from_serialize(&yaml_model));
        ctx.insert(
            TARGET_UNIQUE_ID.to_string(),
            Value::from(pointer_id.as_str()),
        );
        with_state(ctx, |state| {
            let query_ctx = query_ctx_from_state(state).expect("query ctx");
            assert_eq!(query_ctx.node_id().map(String::as_str), Some(node_id));
            assert_eq!(
                query_ctx.target_unique_id().map(String::as_str),
                Some(pointer_id.as_str())
            );
            assert_eq!(
                query_ctx.target_or_node_id().map(String::as_str),
                Some(pointer_id.as_str())
            );
        });
    }

    /// For ordinary work the two ids agree, and `target_or_node_id` still resolves when only the
    /// `model` key is present (no `TARGET_UNIQUE_ID` seeded).
    #[test]
    fn target_unique_id_defaults_to_the_node_id() {
        let node_id = "model.pkg.my_model";

        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = node_id.to_string();
        let yaml_model = dbt_yaml::to_value(&model).expect("model yaml");

        let mut ctx: BTreeMap<String, Value> = BTreeMap::new();
        ctx.insert("model".to_string(), Value::from_serialize(&yaml_model));
        with_state(ctx, |state| {
            let query_ctx = query_ctx_from_state(state).expect("query ctx");
            assert_eq!(query_ctx.node_id().map(String::as_str), Some(node_id));
            assert_eq!(query_ctx.target_unique_id(), None);
            assert_eq!(
                query_ctx.target_or_node_id().map(String::as_str),
                Some(node_id)
            );
        });
    }

    /// dbt-compute usage attribution reads project/model name off the node and the
    /// invocation id off the Jinja `invocation_id` global, the same key the BigQuery
    /// job-label path already reads.
    #[test]
    fn project_model_and_run_id_are_read_for_usage_attribution() {
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.analytics.customer_orders_summary".to_string();
        model.__common_attr__.name = "customer_orders_summary".to_string();
        model.__common_attr__.package_name = "analytics".to_string();
        let yaml_model = dbt_yaml::to_value(&model).expect("model yaml");

        let mut ctx: BTreeMap<String, Value> = BTreeMap::new();
        ctx.insert("model".to_string(), Value::from_serialize(&yaml_model));
        ctx.insert("invocation_id".to_string(), Value::from("run-42"));
        with_state(ctx, |state| {
            let query_ctx = query_ctx_from_state(state).expect("query ctx");
            assert_eq!(
                query_ctx.project_name().map(String::as_str),
                Some("analytics")
            );
            assert_eq!(
                query_ctx.model_name().map(String::as_str),
                Some("customer_orders_summary")
            );
            assert_eq!(query_ctx.run_id().map(String::as_str), Some("run-42"));
        });
    }
}
