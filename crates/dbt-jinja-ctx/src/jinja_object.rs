//! `JinjaObject<T>` — a serializable wrapper that preserves minijinja `Object`
//! identity through serde.
//!
//! Plain `serde::Serialize` on a struct field of type `T: Object` would emit
//! `T`'s data shape, losing the dispatch needed for callables like `ref(...)`.
//! `JinjaObject<T>` instead serializes by wrapping `self` as
//! `minijinja::Value::from_dyn_object(...)` and serializing that — minijinja's
//! `VALUE_HANDLES` thread-local registry smuggles the original `Value` through
//! the outer `Value::from_serialize(parent_ctx)` call so the Object dispatch
//! survives intact. See `minijinja/src/value/mod.rs` (the `Value: Serialize`
//! impl) and `minijinja/src/value/serialize.rs` (`serialize_unit_variant`'s
//! `VALUE_HANDLE_MARKER` interception) for the underlying mechanism.

use std::sync::Arc;

use minijinja::Value;
use minijinja::value::Object;
use schemars::JsonSchema;
use schemars::r#gen::SchemaGenerator;
use schemars::schema::{InstanceType, Schema, SchemaObject};
use serde::{Serialize, Serializer};

/// A serializable wrapper around any `Object` impl that preserves Jinja
/// dispatch identity through `serde::Serialize`.
#[derive(Debug)]
pub struct JinjaObject<T: Object + Send + Sync + 'static>(pub Arc<T>);

// Manual `Clone` so we don't require `T: Clone` — `Arc<T>` is unconditionally
// cloneable via reference counting.
impl<T: Object + Send + Sync + 'static> Clone for JinjaObject<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: Object + Send + Sync + 'static> JinjaObject<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(value))
    }

    pub fn from_arc(value: Arc<T>) -> Self {
        Self(value)
    }

    pub fn as_value(&self) -> Value {
        Value::from_dyn_object(self.0.clone())
    }
}

impl<T: Object + Send + Sync + 'static> From<T> for JinjaObject<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: Object + Send + Sync + 'static> From<Arc<T>> for JinjaObject<T> {
    fn from(value: Arc<T>) -> Self {
        Self::from_arc(value)
    }
}

impl<T: Object + Send + Sync + 'static> Serialize for JinjaObject<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Triggers minijinja's VALUE_HANDLES round-trip when the outer
        // serializer is its `ValueSerializer`; falls back to the underlying
        // Value's data-shaped serialize for foreign serializers (e.g. JSON
        // schema snapshots).
        self.as_value().serialize(serializer)
    }
}

impl<T: Object + Send + Sync + 'static> JsonSchema for JinjaObject<T> {
    fn schema_name() -> String {
        format!("JinjaObject<{}>", core::any::type_name::<T>())
    }

    fn json_schema(_gen: &mut SchemaGenerator) -> Schema {
        // Opaque to JsonSchema — the Object's surface is dynamic by design.
        let mut schema = SchemaObject {
            instance_type: Some(InstanceType::Object.into()),
            ..Default::default()
        };
        schema.metadata().description = Some(format!(
            "Opaque Jinja-callable object backed by `{}`.",
            core::any::type_name::<T>()
        ));
        schema
            .extensions
            .insert("x-jinja-object".to_string(), serde_json::json!(true));
        schema.extensions.insert(
            "x-jinja-object-type".to_string(),
            serde_json::json!(core::any::type_name::<T>()),
        );
        Schema::Object(schema)
    }
}
