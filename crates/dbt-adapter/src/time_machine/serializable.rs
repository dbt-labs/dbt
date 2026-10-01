//! Trait and registry for time machine serialization of Object types.
//!
//! # Adding a New Serializable Type
//!
//! 1. Implement `TimeMachineSerializable` for your type
//! 2. Register it in `build_registry()`
//! 3. Add the downcast check in `serialize_object()`

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::relation::{RelationConfig, RelationObject};
use crate::{catalog_relation, column, response};

use super::serde::ReplayCallContext;

/// Trait for types that can be serialized/deserialized for time machine recording.
pub trait TimeMachineSerializable: Send + Sync {
    /// Stable type identifier for cross-version compatibility.
    /// Use simple names like "AgateTable", "AdapterResponse", etc.
    const TYPE_ID: &'static str;

    /// Serialize to JSON. `__type__` field should be added automatically.
    fn to_time_machine_json(&self) -> serde_json::Value;

    /// Reconstruct from JSON. The `json` includes the `__type__` field.
    fn from_time_machine_json(
        json: &serde_json::Value,
        ctx: &ReplayCallContext,
    ) -> Option<minijinja::Value>
    where
        Self: Sized;
}

pub type DeserializeFn = fn(&serde_json::Value, &ReplayCallContext) -> Option<minijinja::Value>;

#[derive(Clone)]
pub struct TypeEntry {
    pub type_id: &'static str,
    pub deserialize: DeserializeFn,
}

static DESERIALIZER_REGISTRY: OnceLock<HashMap<&'static str, TypeEntry>> = OnceLock::new();

pub fn registry() -> &'static HashMap<&'static str, TypeEntry> {
    DESERIALIZER_REGISTRY.get_or_init(build_registry)
}

fn build_registry() -> HashMap<&'static str, TypeEntry> {
    let mut registry = HashMap::new();
    register::<dbt_agate::AgateTable>(&mut registry);
    register::<response::AdapterResponse>(&mut registry);
    register::<RelationObject>(&mut registry);
    register::<catalog_relation::CatalogRelation>(&mut registry);
    register::<column::Column>(&mut registry);
    register::<RelationConfig>(&mut registry);
    registry
}

fn register<T: TimeMachineSerializable>(registry: &mut HashMap<&'static str, TypeEntry>) {
    registry.insert(
        T::TYPE_ID,
        TypeEntry {
            type_id: T::TYPE_ID,
            deserialize: T::from_time_machine_json,
        },
    );
}

/// Serialize a minijinja Value if it's a known TimeMachineSerializable type.
pub fn serialize_object(value: &minijinja::Value) -> Option<serde_json::Value> {
    if let Some(obj) = value.downcast_object_ref::<dbt_agate::AgateTable>() {
        return Some(wrap_with_type::<dbt_agate::AgateTable>(
            obj.to_time_machine_json(),
        ));
    }
    if let Some(obj) = value.downcast_object_ref::<response::AdapterResponse>() {
        return Some(wrap_with_type::<response::AdapterResponse>(
            obj.to_time_machine_json(),
        ));
    }
    if let Some(obj) = value.downcast_object_ref::<RelationObject>() {
        return Some(wrap_with_type::<RelationObject>(obj.to_time_machine_json()));
    }
    if let Some(obj) = value.downcast_object_ref::<catalog_relation::CatalogRelation>() {
        return Some(wrap_with_type::<catalog_relation::CatalogRelation>(
            obj.to_time_machine_json(),
        ));
    }
    if let Some(obj) = value.downcast_object_ref::<column::Column>() {
        return Some(wrap_with_type::<column::Column>(obj.to_time_machine_json()));
    }
    if let Some(obj) = value.downcast_object_ref::<RelationConfig>()
        && obj.adapter_type() == dbt_adapter_core::AdapterType::Databricks
    {
        return Some(wrap_with_type::<RelationConfig>(obj.to_time_machine_json()));
    }
    None
}

fn wrap_with_type<T: TimeMachineSerializable>(mut json: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = json.as_object_mut() {
        let mut result = serde_json::Map::new();
        result.insert(
            "__type__".to_string(),
            serde_json::Value::String(T::TYPE_ID.to_string()),
        );
        result.extend(obj.clone());
        return serde_json::Value::Object(result);
    }
    serde_json::json!({ "__type__": T::TYPE_ID, "__value__": json })
}

/// Deserialize a JSON object to a minijinja Value using the registry.
///
/// Looks up the `__type__` field and dispatches to the appropriate deserializer.
pub fn deserialize_object(
    type_id: &str,
    json: &serde_json::Value,
    ctx: &ReplayCallContext,
) -> Option<minijinja::Value> {
    registry()
        .get(type_id)
        .and_then(|entry| (entry.deserialize)(json, ctx))
}

/// Helper to extract common fields from a JSON object.
pub struct JsonExtractor<'a> {
    obj: &'a serde_json::Map<String, serde_json::Value>,
}

impl<'a> JsonExtractor<'a> {
    /// Create a new extractor from a JSON value.
    pub fn new(json: &'a serde_json::Value) -> Option<Self> {
        json.as_object().map(|obj| Self { obj })
    }

    pub fn opt_str(&self, key: &str) -> Option<String> {
        self.obj
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    pub fn str_or(&self, key: &str, default: &str) -> String {
        self.opt_str(key).unwrap_or_else(|| default.to_string())
    }

    pub fn opt_i64(&self, key: &str) -> Option<i64> {
        self.obj.get(key).and_then(|v| v.as_i64())
    }

    pub fn i64_or(&self, key: &str, default: i64) -> i64 {
        self.opt_i64(key).unwrap_or(default)
    }

    pub fn opt_u64(&self, key: &str) -> Option<u64> {
        self.obj.get(key).and_then(|v| v.as_u64())
    }

    pub fn opt_u32(&self, key: &str) -> Option<u32> {
        self.opt_u64(key).map(|v| v as u32)
    }

    pub fn opt_bool(&self, key: &str) -> Option<bool> {
        self.obj.get(key).and_then(|v| v.as_bool())
    }

    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        self.opt_bool(key).unwrap_or(default)
    }

    pub fn opt_object(&self, key: &str) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.obj.get(key).and_then(|v| v.as_object())
    }

    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.obj.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestType;
    impl TimeMachineSerializable for TestType {
        const TYPE_ID: &'static str = "TestType";
        fn to_time_machine_json(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn from_time_machine_json(
            _: &serde_json::Value,
            _: &ReplayCallContext,
        ) -> Option<minijinja::Value> {
            None
        }
    }

    #[test]
    fn test_wrap_with_type() {
        let json = serde_json::json!({"foo": "bar"});
        let wrapped = wrap_with_type::<TestType>(json);
        let obj = wrapped.as_object().unwrap();
        assert_eq!(obj.get("__type__").unwrap(), "TestType");
        assert_eq!(obj.get("foo").unwrap(), "bar");
    }

    #[test]
    fn test_json_extractor() {
        let json = serde_json::json!({
            "str_field": "hello",
            "int_field": 42,
            "bool_field": true
        });
        let ext = JsonExtractor::new(&json).unwrap();

        assert_eq!(ext.opt_str("str_field"), Some("hello".to_string()));
        assert_eq!(ext.opt_str("missing"), None);
        assert_eq!(ext.str_or("missing", "default"), "default");
        assert_eq!(ext.opt_i64("int_field"), Some(42));
        assert_eq!(ext.i64_or("missing", 0), 0);
        assert_eq!(ext.opt_bool("bool_field"), Some(true));
    }
}
