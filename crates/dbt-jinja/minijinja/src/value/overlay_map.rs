use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Serialize, Serializer};

use crate::value::{Enumerator, Object, ObjectRepr, Value, ValueMap};

/// A map that layers its own entries over a shared base map.
///
/// An entry shadows the base entry with the same key, as if the base were
/// copied and the entries inserted into the copy, but the base is not copied.
/// The base can be any map-like value, including another [`OverlayMap`].
///
/// Build the entries with [`insert`](Self::insert) and [`Extend`], then wrap
/// the map with [`Value::from_object`]. Serializing an `OverlayMap` into a
/// [`Value`] keeps it an object, so it can be passed to `render` as is.
///
/// Iteration yields the keys of the entries and the base in sorted order, not
/// in insertion order. Do not use it for values where insertion order is
/// expected, such as dicts that templates iterate.
#[derive(Debug, Clone, Default)]
pub struct OverlayMap {
    base: Value,
    entries: ValueMap,
}

impl OverlayMap {
    /// Creates an overlay with no entries over `base`.
    pub fn new(base: Value) -> Self {
        OverlayMap {
            base,
            entries: ValueMap::new(),
        }
    }

    /// Returns the entry for `key`, else the base value for `key`.
    pub fn get(&self, key: &str) -> Option<Value> {
        self.get_value(&Value::from(key))
    }

    /// Sets `key` in the entries. The base does not change.
    pub fn insert(&mut self, key: impl Into<Value>, value: Value) {
        self.entries.insert(key.into(), value);
    }

    /// Returns whether the entries or the base have `key`.
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Removes `key` from the entries. A base value for `key` shows again.
    pub fn remove_entry(&mut self, key: &str) -> Option<Value> {
        let key = Value::from(key);
        #[cfg(feature = "preserve_order")]
        {
            self.entries.shift_remove(&key)
        }
        #[cfg(not(feature = "preserve_order"))]
        {
            self.entries.remove(&key)
        }
    }

    /// Sets `key` to `f` applied to its current value. The current value moves
    /// out of the entries, or is the base value when the entries do not have
    /// `key`.
    pub fn update(&mut self, key: &str, f: impl FnOnce(Option<Value>) -> Value) {
        let key = Value::from(key);
        let current = match self.entries.get_mut(&key) {
            Some(entry) => Some(std::mem::replace(entry, Value::UNDEFINED)),
            None => self.base_value(&key),
        };
        self.entries.insert(key, f(current));
    }

    /// Creates an overlay to add entries over `base`.
    ///
    /// When `base` is an `OverlayMap`, the result starts with a copy of its
    /// entries over its base, so layering stays one level deep however often
    /// a context is extended.
    pub fn extending(base: &Value) -> OverlayMap {
        match base.downcast_object_ref::<OverlayMap>() {
            Some(overlay) => overlay.clone(),
            None => OverlayMap::new(base.clone()),
        }
    }

    fn get_value(&self, key: &Value) -> Option<Value> {
        match self.entries.get(key) {
            Some(value) => Some(value.clone()),
            None => self.base_value(key),
        }
    }

    fn base_value(&self, key: &Value) -> Option<Value> {
        self.base.as_object()?.get_value(key)
    }
}

impl<K: Into<Value>> Extend<(K, Value)> for OverlayMap {
    fn extend<I: IntoIterator<Item = (K, Value)>>(&mut self, iter: I) {
        self.entries
            .extend(iter.into_iter().map(|(key, value)| (key.into(), value)));
    }
}

impl From<BTreeMap<String, Value>> for OverlayMap {
    /// Creates an overlay with no entries over `map`, without copying it.
    fn from(map: BTreeMap<String, Value>) -> Self {
        OverlayMap::new(Value::from_object(map))
    }
}

impl From<OverlayMap> for Value {
    fn from(value: OverlayMap) -> Self {
        Value::from_object(value)
    }
}

impl Object for OverlayMap {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Map
    }

    fn is_introspective_stub(self: &Arc<Self>) -> bool {
        self.base.is_introspective_stub()
            || self
                .entries
                .iter()
                .any(|(key, value)| key.is_introspective_stub() || value.is_introspective_stub())
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        OverlayMap::get_value(self, key)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        let mut keys: BTreeSet<Value> = self.entries.keys().cloned().collect();
        if let Ok(base_keys) = self.base.try_iter() {
            keys.extend(base_keys);
        }
        Enumerator::Iter(Box::new(keys.into_iter()))
    }
}

impl Serialize for OverlayMap {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Value::from_object(self.clone()).serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn base() -> Value {
        Value::from_object(BTreeMap::from([
            ("a".to_string(), Value::from(1)),
            ("c".to_string(), Value::from(3)),
            ("u".to_string(), Value::UNDEFINED),
        ]))
    }

    #[test]
    fn entries_shadow_the_base() {
        let mut map = OverlayMap::new(base());
        map.extend([("b", Value::from(20)), ("c", Value::from(30))]);

        assert_eq!(map.get("a"), Some(Value::from(1)));
        assert_eq!(map.get("b"), Some(Value::from(20)));
        assert_eq!(map.get("c"), Some(Value::from(30)));
        assert_eq!(map.get("missing"), None);
        // A base key holding undefined is present, as in a plain map.
        assert_eq!(map.get("u"), Some(Value::UNDEFINED));

        let value = Value::from_object(map);
        let keys: Vec<Value> = value.try_iter().unwrap().collect();
        assert_eq!(keys, ["a", "b", "c", "u"].map(Value::from));
        assert_eq!(Value::from_serialize(&value), value);
    }

    #[test]
    fn update_moves_the_current_value() {
        let mut map = OverlayMap::new(base());
        map.update("a", |current| {
            Value::from(current.unwrap().as_i64().unwrap() + 10)
        });
        map.update("a", |current| {
            Value::from(current.unwrap().as_i64().unwrap() + 100)
        });
        map.update("new", |current| {
            assert!(current.is_none());
            Value::from(5)
        });
        assert_eq!(map.get("a"), Some(Value::from(111)));
        assert_eq!(map.get("new"), Some(Value::from(5)));
    }

    #[test]
    fn macros_see_an_overlay_root() {
        let mut env = crate::Environment::new();
        env.add_template(
            "t",
            "{% macro m() %}[{{ a }}|{{ b }}]{% endmacro %}{{ m() }}",
        )
        .unwrap();
        let mut root = OverlayMap::new(base());
        root.insert("b", Value::from(2));

        let rendered = env.get_template("t").unwrap().render(&root, &[]).unwrap();
        assert_eq!(rendered, "[1|2]");
    }

    #[test]
    fn an_overlay_state_root_is_the_base_context() {
        let env = crate::Environment::new();
        let mut root = OverlayMap::new(base());
        root.insert("b", Value::from(2));
        let state = env.new_state_with_context(root);

        let context = state.get_base_context();
        assert_eq!(context.get_attr("a").unwrap(), Value::from(1));
        assert_eq!(context.get_attr("b").unwrap(), Value::from(2));
    }

    #[test]
    fn extending_keeps_one_base() {
        let mut first = OverlayMap::new(base());
        first.insert("b", Value::from(2));
        let first = Value::from_object(first);
        let mut second = OverlayMap::extending(&first);
        second.insert("c", Value::from(30));
        let second = Value::from_object(second);

        let second_map = second.downcast_object_ref::<OverlayMap>().unwrap();
        assert!(second_map
            .base
            .downcast_object_ref::<OverlayMap>()
            .is_none());
        assert_eq!(second.get_attr("b").unwrap(), Value::from(2));
        assert_eq!(second.get_attr("c").unwrap(), Value::from(30));
        // The first map does not change.
        assert_eq!(first.get_attr("c").unwrap(), Value::from(3));
    }
}
