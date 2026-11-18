use std::sync::Arc;

use dbt_adapter::Adapter;
use dbt_adapter::cast_util::downcast_value_to_dyn_base_relation;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use minijinja::Value;

/// This function is used to add materializations to the relation cache
/// This assumes that a materialization macro returns a [Value] of { "relations": [...relations] }
///
/// # Arguments
/// * `adapter` - The adapter whose relation cache receives the relations
/// * `jinja_env` - The Jinja environment that provides the state for `cache_added`
/// * `relations_map` - A [Value] map containing a "relations" key with an array of [Arc<dyn BaseRelation>] objects to cache
///
/// # Returns
/// * `Result<(), minijinja::Error>` - Error if caching a provided relation fails
pub fn cache_materialization_return_value(
    adapter: &Adapter,
    jinja_env: Arc<JinjaEnv>,
    relations_map: &Value,
) -> Result<(), minijinja::Error> {
    // XXX: If relations aren't provided, we still return Ok as
    // creating relations that aren't meant to be cached is achieved by not returning a relation
    // in your materialization
    let dummy_state = jinja_env.empty_state();
    let relations = relations_map.get_item(&Value::from("relations"));
    if let Ok(relations) = relations
        && let Ok(relations_iter) = relations.try_iter()
    {
        for relation in relations_iter {
            let relation_arc = downcast_value_to_dyn_base_relation(&relation)?;
            adapter.cache_added(&dummy_state, relation_arc)?;
        }
    }
    Ok(())
}
