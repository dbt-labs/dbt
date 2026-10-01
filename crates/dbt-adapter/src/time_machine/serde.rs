//! Conversion between minijinja::Value and serde_json::Value for time-machine recordings.

use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::ResolvedQuoting;

use super::serializable::{deserialize_object, serialize_object};

// =============================================================================
// Serialization: minijinja::Value -> serde_json::Value
// =============================================================================

/// Serialize adapter call arguments to JSON.
///
/// Uses `serialize_value` for each argument to handle special types like AgateTable.
pub fn serialize_args(args: &[minijinja::Value]) -> serde_json::Value {
    let serialized: Vec<serde_json::Value> = args.iter().map(serialize_value).collect();
    serde_json::Value::Array(serialized)
}

/// Serialize a minijinja::Value result to JSON with special handling for certain adapter types
pub fn serialize_value(value: &minijinja::Value) -> serde_json::Value {
    use minijinja::value::ValueKind;

    // Try to serialize using the TimeMachineSerializable trait system first.
    // This handles AgateTable, AdapterResponse, RelationObject, CatalogRelation, Column, etc.
    if let Some(json) = serialize_object(value) {
        return json;
    }

    // Handle actual sequences - recursively serialize each element
    // This catches tuples like (AdapterResponse, AgateTable)
    if matches!(value.kind(), ValueKind::Seq)
        && let Ok(iter) = value.try_iter()
    {
        let items: Vec<serde_json::Value> = iter.map(|v| serialize_value(&v)).collect();
        return serde_json::Value::Array(items);
    }

    // For other objects not in the registry, inject __type__ for debugging
    if let Some(obj) = value.as_object() {
        let type_name = obj.type_name();
        // Serialize the value first
        if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(value) {
            // Inject __type__ at the beginning
            let mut result = serde_json::Map::new();
            result.insert(
                "__type__".to_string(),
                serde_json::Value::String(type_name.to_string()),
            );
            result.extend(map);
            return serde_json::Value::Object(result);
        }
    }

    // Default JSON serialization
    match serde_json::to_value(value) {
        Ok(v) => v,
        Err(_) => serde_json::Value::String(format!("{:?}", value)),
    }
}

// =============================================================================
// Deserialization: serde_json::Value -> minijinja::Value
// =============================================================================

/// Context for reconstructing typed objects during replay.
#[derive(Clone)]
pub struct ReplayContext {
    pub adapter_type: AdapterType,
    pub quoting: ResolvedQuoting,
}

/// Context that applies only while reconstructing one recorded adapter call.
#[derive(Clone)]
pub struct ReplayCallContext {
    replay: ReplayContext,
    relation_type: Option<RelationType>,
}

impl ReplayCallContext {
    fn new(replay: ReplayContext) -> Self {
        Self {
            replay,
            relation_type: None,
        }
    }

    pub(crate) fn with_relation_type(mut self, relation_type: Option<RelationType>) -> Self {
        self.relation_type = relation_type;
        self
    }

    pub(crate) fn replay_context(&self) -> &ReplayContext {
        &self.replay
    }

    pub(crate) fn relation_type(&self) -> Option<RelationType> {
        self.relation_type
    }
}

impl From<ReplayContext> for ReplayCallContext {
    fn from(replay: ReplayContext) -> Self {
        Self::new(replay)
    }
}

impl Default for ReplayContext {
    fn default() -> Self {
        Self {
            adapter_type: AdapterType::Snowflake,
            quoting: ResolvedQuoting::default(),
        }
    }
}

/// Deserialize a JSON value back to a minijinja::Value with adapter context.
///
/// This allows proper reconstruction of RelationObjects.
pub fn json_to_value_with_context(
    json: &serde_json::Value,
    ctx: &ReplayCallContext,
) -> minijinja::Value {
    match json {
        serde_json::Value::Null => minijinja::Value::from(()),
        serde_json::Value::Bool(b) => minijinja::Value::from(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                minijinja::Value::from(i)
            } else if let Some(f) = n.as_f64() {
                minijinja::Value::from(f)
            } else {
                minijinja::Value::from(n.to_string())
            }
        }
        serde_json::Value::String(s) => minijinja::Value::from(s.clone()),
        serde_json::Value::Array(arr) => {
            minijinja::Value::from_iter(arr.iter().map(|v| json_to_value_with_context(v, ctx)))
        }
        serde_json::Value::Object(obj) => {
            if let Some(type_name) = obj.get("__type__").and_then(|v| v.as_str())
                && let Some(value) = reconstruct_typed_object(type_name, json, ctx)
            {
                return value;
            }

            // Fall back to plain map
            let pairs: Vec<(minijinja::Value, minijinja::Value)> = obj
                .iter()
                .filter(|(k, _)| *k != "__type__") // Don't include __type__ in the map
                .map(|(k, v)| {
                    (
                        minijinja::Value::from(k.clone()),
                        json_to_value_with_context(v, ctx),
                    )
                })
                .collect();
            minijinja::Value::from_iter(pairs)
        }
    }
}

fn reconstruct_typed_object(
    type_name: &str,
    json: &serde_json::Value,
    ctx: &ReplayCallContext,
) -> Option<minijinja::Value> {
    deserialize_object(type_name, json, ctx)
}

/// Check if a JSON value is a "zero value" (false, null, 0, "", [], {}).
///
/// Used by `values_match` to treat missing keys as equivalent to their default.
fn is_json_zero_value(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => true,
        serde_json::Value::Bool(b) => !b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f == 0.0),
        serde_json::Value::String(s) => s.is_empty(),
        serde_json::Value::Array(a) => a.is_empty(),
        serde_json::Value::Object(o) => o.is_empty(),
    }
}

/// Check if two JSON values match semantically, ignoring `__type__` fields.
///
/// For objects, keys present in one side but missing in the other are tolerated
/// if the value is a "zero value" (false, null, 0, "", [], {}). This provides
/// forward compatibility when new fields with default values are added to
/// serialization — old recordings without those fields still match.
pub fn values_match(expected: &serde_json::Value, actual: &serde_json::Value) -> bool {
    values_match_inner(expected, actual, false)
}

fn values_match_inner(
    expected: &serde_json::Value,
    actual: &serde_json::Value,
    ignore_internal_model_fields: bool,
) -> bool {
    match (expected, actual) {
        (serde_json::Value::Null, serde_json::Value::Null) => true,
        (serde_json::Value::Bool(a), serde_json::Value::Bool(b)) => a == b,
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => a == b,
        (serde_json::Value::String(a), serde_json::Value::String(b)) => {
            // Normalize path separators so recordings made on Linux (/) match
            // replay on Windows (\) and vice versa.
            a == b || dbt_common::path::path_separator_eq(a, b)
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(x, y)| values_match_inner(x, y, false))
        }
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            // LazyModelWrapper and graph node maps carry the resolved adapter for internal
            // consumers. It is not stable public model data, so tolerate its presence or absence
            // during replay. Graph nodes are plain serialized maps and have no __type__ marker.
            let is_lazy_model = [a, b].iter().any(|value| {
                value
                    .get("__type__")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|type_name| type_name.ends_with("LazyModelWrapper"))
            });
            let is_graph_node = [a, b].iter().any(|value| {
                value.get("resource_type").is_some() && value.get("unique_id").is_some()
            });
            let ignore_adapter = ignore_internal_model_fields || is_lazy_model || is_graph_node;
            // Collect keys from both sides, ignoring __type__ and internal model fields.
            let a_keys: std::collections::HashSet<_> = a
                .keys()
                .filter(|k| *k != "__type__" && (!ignore_adapter || *k != "adapter"))
                .collect();
            let b_keys: std::collections::HashSet<_> = b
                .keys()
                .filter(|k| *k != "__type__" && (!ignore_adapter || *k != "adapter"))
                .collect();

            // Keys present in both must match
            for k in a_keys.intersection(&b_keys) {
                let ignore_nested_adapter =
                    (is_lazy_model || is_graph_node) && *k == "__base_attr__";
                if !values_match_inner(
                    a.get(*k).unwrap(),
                    b.get(*k).unwrap(),
                    ignore_nested_adapter,
                ) {
                    return false;
                }
            }

            // Keys only in `a` (expected) are OK if their value is a zero value
            for k in a_keys.difference(&b_keys) {
                if !is_json_zero_value(a.get(*k).unwrap()) {
                    return false;
                }
            }

            // Keys only in `b` (actual) are OK if their value is a zero value
            for k in b_keys.difference(&a_keys) {
                if !is_json_zero_value(b.get(*k).unwrap()) {
                    return false;
                }
            }

            true
        }
        _ => false,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn json_to_value(json: &serde_json::Value) -> minijinja::Value {
        json_to_value_with_context(json, &ReplayContext::default().into())
    }

    #[test]
    fn test_serialize_args() {
        let args = vec![
            minijinja::Value::from("hello"),
            minijinja::Value::from(42),
            minijinja::Value::from(true),
        ];

        let serialized = serialize_args(&args);
        assert!(serialized.is_array());

        let arr = serialized.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], serde_json::json!("hello"));
        assert_eq!(arr[1], serde_json::json!(42));
        assert_eq!(arr[2], serde_json::json!(true));
    }

    #[test]
    fn test_json_to_value_primitives() {
        assert!(json_to_value(&serde_json::json!(null)).is_none());
        assert!(json_to_value(&serde_json::json!(true)).is_true());
        assert!(!json_to_value(&serde_json::json!(false)).is_true());
        assert_eq!(json_to_value(&serde_json::json!(42)).as_i64(), Some(42));
        assert_eq!(
            json_to_value(&serde_json::json!("hello")).as_str(),
            Some("hello")
        );
    }

    #[test]
    fn test_json_to_value_array() {
        let json = serde_json::json!([1, 2, 3]);
        let value = json_to_value(&json);
        let items: Vec<_> = value.try_iter().unwrap().collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_i64(), Some(1));
    }

    #[test]
    fn test_json_to_value_object() {
        let json = serde_json::json!({"a": 1, "b": "two"});
        let value = json_to_value(&json);
        assert_eq!(value.get_attr("a").unwrap().as_i64(), Some(1));
        assert_eq!(value.get_attr("b").unwrap().as_str(), Some("two"));
    }

    #[test]
    fn test_values_match_ignores_lazy_model_adapter() {
        let expected = serde_json::json!({
            "__type__": "LazyModelWrapper",
            "adapter": "snowflake",
            "__base_attr__": {"adapter": "snowflake"},
            "config": {"adapter": "snowflake"},
            "name": "model"
        });
        let actual = serde_json::json!({
            "__type__": "LazyModelWrapper",
            "adapter": "bigquery",
            "__base_attr__": {"adapter": "bigquery"},
            "config": {"adapter": "snowflake"},
            "name": "model"
        });

        assert!(values_match(&expected, &actual));

        let mut nested_adapter_change = actual;
        nested_adapter_change["config"]["adapter"] = serde_json::json!("databricks");
        assert!(!values_match(&expected, &nested_adapter_change));
    }

    #[test]
    fn test_values_match_ignores_graph_node_adapter() {
        assert!(values_match(
            &serde_json::json!({
                "resource_type": "model",
                "unique_id": "model.pkg.example",
                "adapter": "snowflake",
                "__base_attr__": {"adapter": "snowflake"}
            }),
            &serde_json::json!({
                "resource_type": "model",
                "unique_id": "model.pkg.example",
                "adapter": "bigquery",
                "__base_attr__": {"adapter": "bigquery"}
            }),
        ));
    }

    #[test]
    fn test_values_match_does_not_ignore_adapter_for_other_objects() {
        assert!(!values_match(
            &serde_json::json!({"adapter": "snowflake"}),
            &serde_json::json!({"adapter": "bigquery"}),
        ));
    }

    #[test]
    fn test_values_match() {
        assert!(values_match(
            &serde_json::json!({"a": 1}),
            &serde_json::json!({"a": 1})
        ));
        assert!(values_match(
            &serde_json::json!({"__type__": "Foo", "a": 1}),
            &serde_json::json!({"__type__": "Bar", "a": 1})
        ));
        assert!(!values_match(
            &serde_json::json!({"a": 1}),
            &serde_json::json!({"a": 2})
        ));
    }

    #[test]
    fn test_values_match_tolerates_missing_zero_value_keys() {
        // Extra key with false value on one side — should match (forward compat)
        assert!(values_match(
            &serde_json::json!({"a": 1}),
            &serde_json::json!({"a": 1, "is_delta": false})
        ));
        assert!(values_match(
            &serde_json::json!({"a": 1, "is_delta": false}),
            &serde_json::json!({"a": 1})
        ));

        // Extra key with null value — should match
        assert!(values_match(
            &serde_json::json!({"a": 1}),
            &serde_json::json!({"a": 1, "extra": null})
        ));

        // Extra key with zero number — should match
        assert!(values_match(
            &serde_json::json!({"a": 1, "count": 0}),
            &serde_json::json!({"a": 1})
        ));

        // Extra key with empty string — should match
        assert!(values_match(
            &serde_json::json!({"a": 1}),
            &serde_json::json!({"a": 1, "name": ""})
        ));

        // Extra key with non-zero value — should NOT match
        assert!(!values_match(
            &serde_json::json!({"a": 1}),
            &serde_json::json!({"a": 1, "is_delta": true})
        ));
        assert!(!values_match(
            &serde_json::json!({"a": 1, "extra": "data"}),
            &serde_json::json!({"a": 1})
        ));
        assert!(!values_match(
            &serde_json::json!({"a": 1}),
            &serde_json::json!({"a": 1, "count": 42})
        ));
    }

    #[test]
    fn test_values_match_nested_zero_value_tolerance() {
        // Nested objects: the tolerance applies recursively
        let old_recording = serde_json::json!({
            "__type__": "RelationObject",
            "adapter_type": "snowflake",
            "database": "raw",
            "is_table": false,
        });
        let new_code = serde_json::json!({
            "__type__": "RelationObject",
            "adapter_type": "snowflake",
            "database": "raw",
            "is_table": false,
            "is_delta": false,
        });
        assert!(values_match(&old_recording, &new_code));
        assert!(values_match(&new_code, &old_recording));
    }
}
