//! Microbatch model execution runner.
//!
//! This module provides the execution logic for models using the microbatch
//! incremental strategy. It processes data in time-based windows (batches).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use crate::microbatch::BatchContext;
use chrono_tz::Tz;
use dbt_adapter::cast_util::downcast_value_to_dyn_base_relation;
use dbt_adapter::formatter::SqlLiteralFormatter;
use dbt_adapter::relation::create_relation_from_node;
use dbt_adapter_core::AdapterType;
use dbt_common::FsResult;
use dbt_common::constants::DBT_EPHEMERAL_DIR_NAME;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::phases::compile::DependencyValidationConfig;
use dbt_jinja_utils::phases::run::RunConfig;
use dbt_jinja_utils::phases::{MicrobatchRefContext, RefFunction, SourceFunction};
use dbt_jinja_utils::utils::inject_and_persist_ephemeral_models;
use dbt_schemas::schemas::{DbtModel, InternalDbtNode};
use dbt_schemas::state::{DbtRuntimeConfig, NodeResolverTracker};
use minijinja::Value;
use minijinja::value::ValueMap;
use minijinja::value::mutable_map::MutableMap;
use minijinja_contrib::modules::py_datetime::datetime::PyDateTime;
use minijinja_contrib::modules::pytz::PytzTimezone;
use tracing::warn;

/// Extend the Jinja context with microbatch-specific overrides.
///
/// This replaces `ref` and `source` with microbatch-aware versions that inject
/// event-time filters, adds batch metadata variables, patches the `model` value
/// with batch info and internal timestamps, and controls pre/post hook lists.
pub fn extend_microbatch_node_context(
    batch_ctx: &BatchContext,
    adapter_type: AdapterType,
    model: &DbtModel,
    node_resolver: Arc<dyn NodeResolverTracker>,
    runtime_config: &DbtRuntimeConfig,
    jinja_context: &mut BTreeMap<String, Value>,
    event_time_mapping: Arc<BTreeMap<String, String>>,
) {
    // Create microbatch ref context for this batch
    let microbatch_ctx = MicrobatchRefContext::new(
        batch_ctx.event_time_start,
        batch_ctx.event_time_end,
        event_time_mapping,
    );
    // TODO(chasewalden): maybe we make a "proxy" type that impls `Object`
    //  could be cleaner/more performant than re-building the entire BTreeMap

    // TODO(chasewalden): we likely need to create a proxy for `{{ this }}` to detect if it is used.

    // Replace the ref function with one that has microbatch context
    // Note: The base_context should already have a ref function, but we need
    // to create a new one with the microbatch context
    let allowed_deps = model
        .__base_attr__
        .depends_on
        .nodes
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();

    let microbatch_ref = Value::from_object(RefFunction::new_with_microbatch_context(
        node_resolver.clone(),
        model.__common_attr__.package_name.clone(),
        runtime_config.clone().into(),
        DependencyValidationConfig::new_for_node(model)
            .validate()
            .allow_dependencies(allowed_deps.iter()),
        microbatch_ctx.clone(),
        model.common().unique_id.clone(),
    ));

    // Insert the microbatch-aware ref into context
    jinja_context.insert("ref".to_string(), microbatch_ref.clone());

    // Replace the source function with one that has microbatch context
    let microbatch_source = Value::from_object(SourceFunction::new_with_microbatch_context(
        node_resolver,
        model.__common_attr__.package_name.clone(),
        runtime_config.clone().into(),
        microbatch_ctx,
    ));

    // Insert the microbatch-aware source into context
    jinja_context.insert("source".to_string(), microbatch_source.clone());

    // User overrides call through builtins.ref and builtins.source, so keep
    // those callables aligned with the batch-aware top-level context.
    if let Some(mut builtins) = jinja_context.get("builtins").and_then(|builtins| {
        builtins
            .as_object()?
            .downcast_ref::<BTreeMap<String, Value>>()
            .cloned()
    }) {
        builtins.insert("ref".to_string(), microbatch_ref);
        builtins.insert("source".to_string(), microbatch_source);
        jinja_context.insert("builtins".to_string(), Value::from_object(builtins));
    }

    // Add batch-specific context variables
    jinja_context.insert(
        "__dbt_microbatch_batch_id__".to_string(),
        Value::from(batch_ctx.id.clone()),
    );
    let formatter = SqlLiteralFormatter::new(adapter_type);
    jinja_context.insert(
        "__dbt_microbatch_event_time_start__".to_string(),
        Value::from(formatter.format_timestamp(batch_ctx.event_time_start)),
    );
    jinja_context.insert(
        "__dbt_microbatch_event_time_end__".to_string(),
        Value::from(formatter.format_timestamp(batch_ctx.event_time_end)),
    );
    jinja_context.insert(
        "__dbt_microbatch_is_first_batch__".to_string(),
        Value::from(batch_ctx.is_first()),
    );
    jinja_context.insert(
        "__dbt_microbatch_is_last_batch__".to_string(),
        Value::from(batch_ctx.is_last()),
    );

    // Add model.batch for the Snowflake incremental macro to use
    // The macro checks model.batch.event_time_start and model.batch.event_time_end
    // These must be PyDateTime objects (not strings) so that datetime methods
    // like .strftime() work in Jinja, matching dbt-core's Python datetime objects.
    let utc = PytzTimezone::new(Tz::UTC);
    let event_time_start = Value::from_object(PyDateTime::new_aware(
        batch_ctx.event_time_start.with_timezone(&Tz::UTC),
        Some(utc.clone()),
    ));
    let event_time_end = Value::from_object(PyDateTime::new_aware(
        batch_ctx.event_time_end.with_timezone(&Tz::UTC),
        Some(utc),
    ));
    let config_updates = BTreeMap::from([
        (
            "__dbt_internal_microbatch_event_time_start".to_string(),
            event_time_start.clone(),
        ),
        (
            "__dbt_internal_microbatch_event_time_end".to_string(),
            event_time_end.clone(),
        ),
    ]);
    let mut batch_info = BTreeMap::new();
    batch_info.insert("event_time_start".to_string(), event_time_start);
    batch_info.insert("event_time_end".to_string(), event_time_end);
    batch_info.insert("id".to_string(), Value::from(batch_ctx.id.clone()));

    // Update the model in context to include batch info
    if let Some(model_value) = jinja_context.get("model").cloned() {
        if let Some(model_obj) = model_value.as_object() {
            // Create a mutable map from the model and add batch
            let mut model_map: BTreeMap<String, Value> = BTreeMap::new();

            // Copy existing model properties
            if let Some(iter) = model_obj.try_iter() {
                for key in iter {
                    if let Some(val) = model_obj.get_value(&key) {
                        if let Some(key_str) = key.as_str() {
                            model_map.insert(key_str.to_string(), val);
                        }
                    }
                }
            }

            // Add batch info — use iterator collect so PyDateTime objects retain their
            // object identity (from_serialize would lose it by calling Serialize on Value).
            model_map.insert(
                "batch".to_string(),
                batch_info
                    .iter()
                    .map(|(k, v)| (Value::from(k.as_str()), v.clone()))
                    .collect::<Value>(),
            );

            // Update config with microbatch timestamps
            if let Some(config_val) = model_map.get("config").cloned() {
                if let Some(config_obj) = config_val.as_object() {
                    let mut config_map: BTreeMap<String, Value> = BTreeMap::new();

                    // Copy existing config properties
                    if let Some(iter) = config_obj.try_iter() {
                        for key in iter {
                            if let Some(val) = config_obj.get_value(&key) {
                                if let Some(key_str) = key.as_str() {
                                    config_map.insert(key_str.to_string(), val);
                                }
                            }
                        }
                    }

                    // Add microbatch timestamps to config as PyDateTime objects so that
                    // datetime methods like .strftime() work in adapter macros.
                    config_map.extend(config_updates.clone());

                    model_map.insert(
                        "config".to_string(),
                        config_map
                            .iter()
                            .map(|(k, v)| (Value::from(k.as_str()), v.clone()))
                            .collect::<Value>(),
                    );
                }
            }

            // Mutable because the batch SQL only exists after rendering, so the caller
            // writes it into this map later.
            jinja_context.insert(
                "model".to_string(),
                Value::from_object(MutableMap::from(
                    model_map
                        .into_iter()
                        .map(|(k, v)| (Value::from(k.as_str()), v))
                        .collect::<ValueMap>(),
                )),
            );

            let update_run_config = |config: &Value| {
                // Values are Arc-backed and the base run context may be shared across
                // batches, so create a batch-local config instead of mutating shared state.
                let mut config = config.as_object()?.downcast_ref::<RunConfig>()?.clone();
                config.model_config.extend(config_updates.clone());
                Some(Value::from_object(config))
            };

            if let Some(config) = jinja_context.get("config").and_then(&update_run_config) {
                jinja_context.insert("config".to_string(), config);
            }

            if let Some(mut builtins) = jinja_context.get("builtins").and_then(|builtins| {
                builtins
                    .as_object()?
                    .downcast_ref::<BTreeMap<String, Value>>()
                    .cloned()
            }) {
                if let Some(config) = builtins.get("config").and_then(update_run_config) {
                    builtins.insert("config".to_string(), config);
                    jinja_context.insert("builtins".to_string(), Value::from_object(builtins));
                }
            }
        }
    }

    // Control pre/post hooks based on batch position
    // Note: dbt-core runs pre-hooks only on first batch, post-hooks only on last batch
    if !batch_ctx.is_first() {
        jinja_context.insert("pre_hooks".to_string(), Value::from(Vec::<Value>::new()));
    }
    if !batch_ctx.is_last() {
        jinja_context.insert("post_hooks".to_string(), Value::from(Vec::<Value>::new()));
    }
}

/// Re-render the raw SQL template with the batch context.
///
/// This re-renders the original model SQL (with Jinja tags like {{ ref(...) }})
/// using a context that includes a microbatch-aware ref function. The ref()
/// function will wrap upstream models with event_time filtering to only include
/// rows within the current batch window.
pub fn render_batch_sql(
    raw_sql: &str,
    jinja_env: Arc<JinjaEnv>,
    jinja_context: &BTreeMap<String, Value>,
    out_dir: &Path,
) -> FsResult<String> {
    // Render the raw SQL through Jinja
    // The context already contains config from build_run_node_context
    // This will cause ref() calls to use the microbatch-aware ref function
    // which wraps refs in filtered subqueries based on event_time
    //
    // TODO(chasewalden): {{ source(...) }} calls also need to be made microbatch-aware
    let rendered = jinja_env.render_str(raw_sql, jinja_context, &[])?;
    tracing::debug!("Rendered batch SQL: {rendered}...",);

    // BUG(chasewalden): the microbatch-aware {{ ref(...) }} does not correctly render upstream `ephemeral` models
    let mut macro_spans = Default::default(); // create dummy MacroSpans
    let rendered = inject_and_persist_ephemeral_models(
        rendered,
        &mut macro_spans,
        "microbatch_not_persisted", // can be anything; only used to persist an ephemeral model
        false,                      // a microbatch model cannot be ephemeral
        &out_dir.join(DBT_EPHEMERAL_DIR_NAME),
    )?;

    tracing::debug!("Rendered batch SQL w/ injected CTE (first 200 chars): {rendered:.200}...",);

    Ok(rendered)
}

/// Build the event_time mapping for dependencies of a microbatch model.
///
/// This mapping is used to determine which refs should have event_time filters
/// applied during batch execution.
pub fn build_event_time_mapping(
    model: &DbtModel,
    nodes: &dbt_schemas::schemas::Nodes,
) -> BTreeMap<String, String> {
    let mut mapping = BTreeMap::new();

    // Resolve each dependency's `event_time` through the `InternalDbtNode` trait
    for dep_id in &model.__base_attr__.depends_on.nodes {
        if let Some(event_time) = nodes.get_node(dep_id).and_then(|node| node.event_time()) {
            mapping.insert(dep_id.clone(), event_time);
        }
    }

    mapping
}

pub async fn is_incremental(
    model: &DbtModel,
    full_refresh: bool,
    adapter_type: AdapterType,
    jinja_env: Arc<JinjaEnv>,
) -> bool {
    if full_refresh {
        return false;
    }

    let relation = match create_relation_from_node(adapter_type, model, None) {
        Ok(rel) => rel,
        Err(e) => {
            warn!(
                "Failed to create relation for checkpoint query: {}. Starting fresh.",
                e
            );
            return false;
        }
    };
    // Owned, so the coordinates can cross to the worker without the relation.
    let database = relation.database().unwrap_or_default().to_string();
    let schema = relation.schema().unwrap_or_default().to_string();
    let identifier = relation.identifier().unwrap_or_default().to_string();

    // `get_relation` asks the warehouse whether the target already exists, so
    // it opens a connection and has to run on a `dbt-runtime` worker. The
    // `State` is built there too: it borrows the environment and is not `Send`.
    let existing_table = dbt_runtime::spawn_blocking(move || {
        let Some(adapter) = jinja_env.get_base_adapter() else {
            return false;
        };
        let state = jinja_env.new_state_with_context(BTreeMap::new());
        adapter
            .get_relation(&state, &database, &schema, &identifier, false)
            .ok()
            .filter(|v| !v.is_none())
            .and_then(|v| downcast_value_to_dyn_base_relation(&v).ok())
            .is_some_and(|rel| rel.is_table())
    })
    .await;

    match existing_table {
        Ok(is_table) => is_table,
        Err(e) => {
            warn!("Failed to check for an existing target relation: {e}. Starting fresh.");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use dbt_common::path::DbtPath;

    use super::*;

    #[test]
    fn test_build_event_time_mapping_empty() {
        // Create a minimal model without depends_on
        let model = create_test_model();
        let nodes = dbt_schemas::schemas::Nodes::default();

        let mapping = build_event_time_mapping(&model, &nodes);
        assert!(mapping.is_empty());
    }

    #[test]
    fn test_build_event_time_mapping_resolves_seed_event_time() {
        // A microbatch model whose upstream is a seed (not a model/source) should
        // still pick up the seed's `event_time`, matching dbt-core. Seeds store
        // event_time in `deprecated_config`, like sources.
        use dbt_schemas::schemas::project::SeedConfig;
        use dbt_schemas::schemas::{CommonAttributes, DbtSeed};

        let seed_uid = "seed.test.raw_events".to_string();
        let seed = DbtSeed {
            __common_attr__: CommonAttributes {
                unique_id: seed_uid.clone(),
                name: "raw_events".to_string(),
                package_name: "test".to_string(),
                ..Default::default()
            },
            deprecated_config: SeedConfig {
                event_time: Some("event_time".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut model = create_test_model();
        model.__base_attr__.depends_on.nodes = vec![seed_uid.clone()];

        let mut nodes = dbt_schemas::schemas::Nodes::default();
        nodes.seeds.insert(seed_uid.clone(), Arc::new(seed));

        let mapping = build_event_time_mapping(&model, &nodes);
        assert_eq!(
            mapping.get(&seed_uid).map(String::as_str),
            Some("event_time")
        );
    }

    #[test]
    fn test_build_event_time_mapping_resolves_snapshot_event_time() {
        // A microbatch model whose upstream is a snapshot should also pick up the
        // snapshot's `event_time`. Like seeds and sources, snapshots store it in
        // `deprecated_config`, and resolution now goes through the node trait.
        use dbt_schemas::schemas::CommonAttributes;
        use dbt_schemas::schemas::DbtSnapshot;
        use dbt_schemas::schemas::project::SnapshotConfig;

        let snapshot_uid = "snapshot.test.raw_events".to_string();
        let snapshot = DbtSnapshot {
            __common_attr__: CommonAttributes {
                unique_id: snapshot_uid.clone(),
                name: "raw_events".to_string(),
                package_name: "test".to_string(),
                ..Default::default()
            },
            deprecated_config: SnapshotConfig {
                event_time: Some("event_time".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut model = create_test_model();
        model.__base_attr__.depends_on.nodes = vec![snapshot_uid.clone()];

        let mut nodes = dbt_schemas::schemas::Nodes::default();
        nodes
            .snapshots
            .insert(snapshot_uid.clone(), Arc::new(snapshot));

        let mapping = build_event_time_mapping(&model, &nodes);
        assert_eq!(
            mapping.get(&snapshot_uid).map(String::as_str),
            Some("event_time")
        );
    }

    fn create_test_model() -> DbtModel {
        use dbt_schemas::schemas::project::ModelConfig;
        use dbt_schemas::schemas::{CommonAttributes, DbtModelAttr, NodeBaseAttributes};

        DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: "model.test.my_model".to_string(),
                name: "my_model".to_string(),
                package_name: "test".to_string(),
                path: DbtPath::from("models/my_model.sql"),
                original_file_path: DbtPath::from("models/my_model.sql"),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes::default(),
            __model_attr__: DbtModelAttr::default(),
            __adapter_attr__: Default::default(),
            deprecated_config: ModelConfig::default(),
            __other__: Default::default(),
        }
    }
}
