//! Field-level node serialization and mutable Jinja node views.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use minijinja::listener::RenderingEventListener;
use minijinja::value::{Enumerator, Object, ObjectExt, ValueMap, mutable_map::MutableMap};
use minijinja::{Error, ErrorKind, State, Value};
use serde::Serialize;

use crate::schemas::InternalDbtNode;
use crate::schemas::serialization_utils::{SerializationMode, apply_mode, serialize_with_mode};

/// The serialized node surface, accessible without serializing the entire node.
pub trait JinjaNodeFields: Send + Sync + std::fmt::Debug {
    fn get(&self, key: &str, mode: SerializationMode) -> Option<dbt_yaml::Value>;
    fn keys(&self, mode: SerializationMode) -> Vec<&str>;
    fn contains_key(&self, key: &str, mode: SerializationMode) -> bool;
}

/// Serialize a single field with the same null handling as a whole node.
pub fn serialize_field<T: Serialize>(
    value: &T,
    mode: SerializationMode,
) -> Option<dbt_yaml::Value> {
    omit_null_field(serialize_with_mode(value, mode), mode)
}

/// [`serialize_field`] for a field with `#[serde(serialize_with = "...")]`.
pub fn serialize_field_with<T: ?Sized>(
    value: &T,
    mode: SerializationMode,
    serialize: impl FnOnce(&T, dbt_yaml::value::Serializer) -> Result<dbt_yaml::Value, dbt_yaml::Error>,
) -> Option<dbt_yaml::Value> {
    let value = serialize(value, dbt_yaml::value::Serializer).expect("Failed to serialize to YAML");
    omit_null_field(apply_mode(value, mode), mode)
}

fn omit_null_field(value: dbt_yaml::Value, mode: SerializationMode) -> Option<dbt_yaml::Value> {
    if mode == SerializationMode::OmitNone && matches!(value, dbt_yaml::Value::Null(_)) {
        None
    } else {
        Some(value)
    }
}

type FieldTransform = Arc<dyn Fn(dbt_yaml::Value) -> dbt_yaml::Value + Send + Sync>;

/// What a [`LazyNodeContext`] changes over its immutable node: written keys, and masks that hide
/// node keys.
#[derive(Debug, Default, Clone)]
struct Overlay {
    // Keys written from Jinja or Rust. They win over node fields.
    values: ValueMap,
    // Node keys removed by `pop`, `popitem` or `clear`. The node can't change, so removing hides
    // the key.
    deleted: HashSet<Value>,
    // The key order once it can no longer be derived; see `LazyNodeContext`. `None` until then.
    order: Option<Vec<Value>>,
    // Compiled SQL keys last written from Jinja. Attribute reads keep serving these from disk.
    // TODO(LazyNodeContext): Remove; this exists only to match current Fusion, which ignores Jinja writes to
    // compiled SQL. dbt-core honors `model.update({'compiled_code': ...})`.
    jinja_compiled: HashSet<Value>,
}

impl Overlay {
    fn insert(&mut self, key: Value, value: Value) {
        if let Some(order) = &mut self.order
            && !order.contains(&key)
        {
            order.push(key.clone());
        }
        self.deleted.remove(&key);
        self.jinja_compiled.remove(&key);
        self.values.insert(key, value);
    }

    fn insert_from_jinja(&mut self, key: Value, value: Value) {
        let compiled = matches!(key.as_str(), Some("compiled_code" | "compiled_sql"));
        self.insert(key.clone(), value);
        if compiled {
            self.jinja_compiled.insert(key);
        }
    }
}

/// A Jinja dictionary backed by a node, whose fields are serialized only when read.
///
/// The node is immutable and shared, so each access computes the dictionary: written keys
/// (`Overlay::values`) first, then node fields that are not deleted. A node
/// field is serialized on its first read and kept in `cache`. Caching is needed for behavior,
/// not only speed: a field's nested maps and lists are mutable Jinja objects, so
/// `model.config.update(...)` and `model.tags.append(...)` persist only if later reads return the
/// same objects.
///
/// Written keys and served fields are kept apart because views of one dictionary treat them
/// differently:
/// * [`alias`](Self::alias) shares both, so it is the same dictionary under another name.
/// * [`original_alias`](Self::original_alias), for run-time `config.model`, shares written keys
///   with `model` but reads served fields from `original`: a copy of their nested maps, taken
///   when they were first served. The lists inside stay the same objects, so list appends
///   through `model` are still visible.
/// * [`fork`](Self::fork), one per microbatch batch, copies the written keys when it is created.
///   It copies a served field's nested maps when it first reads that field, from the
///   serialization in `forked_from`. Batches thus keep their map writes local but share lists,
///   and the node is serialized once for all of them.
///
/// Serialization runs outside the overlay lock. A write or deletion made meanwhile wins over the
/// value just served.
///
/// While nothing has reordered keys, their order is derived: node keys in serde order, then
/// newly written keys. A written node key keeps its node position. Three operations can't be
/// expressed that way: `pop` and `popitem`, which move the last key into the removed slot as
/// `MutableMap` does, `clear`, and [`sort_keys`](Self::sort_keys). The first of them stores the
/// current order in `Overlay::order`, and later inserts append to it.
///
/// Run-phase compiled SQL is not served from the node: it is read from `compiled_path` on every
/// access and never cached, unless a written key overrides it.
///
/// Several fields and code paths exist only to keep the behavior of the eager contexts this type
/// replaced. Each is marked `TODO(LazyNodeContext)` with the dbt-core behavior it differs from;
/// read those for context before changing them.
pub struct LazyNodeContext {
    // The immutable source. Fields are serialized from it only when read.
    node: Arc<dyn JinjaNodeFields>,
    // Null handling for served fields: `KeepNone` keeps null fields, `OmitNone` drops them.
    mode: SerializationMode,
    // Writes and masks over node keys. Shared with aliases, so each alias is another name for the
    // same dictionary.
    overlay: Arc<Mutex<Overlay>>,
    // Each node field as served by this dictionary, so nested mutations persist. Shared with
    // aliases.
    cache: Arc<Mutex<ValueMap>>,
    // For a `fork`: the shared cache that keeps each field as first serialized. `cache` then
    // holds this fork's copies of those fields' nested maps.
    forked_from: Option<Arc<Mutex<ValueMap>>>,
    // With `with_original_view`: each cached field as first serialized, with its nested maps
    // copied. `original_alias` reads these instead of `cache`.
    original: Option<Arc<Mutex<ValueMap>>>,
    // Set by `original_alias`: served fields come from `original` instead of `cache`.
    reads_original: bool,
    // Applied to a field when it is first served, since the node itself can't change, for
    // example stripping the `path` prefix.
    transforms: HashMap<String, FieldTransform>,
    // Run phase: the file that serves `compiled_code` and `compiled_sql` when the node has none.
    compiled_path: Option<PathBuf>,
    // See `with_eager_nested_maps`.
    eager_nested_maps: bool,
    // Set once an introspective stub is written in; the dictionary then reports itself as one.
    // Shared with aliases.
    tainted: Arc<AtomicBool>,
}

impl std::fmt::Debug for LazyNodeContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyNodeContext")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl LazyNodeContext {
    fn new(node: Arc<dyn JinjaNodeFields>, mode: SerializationMode) -> Self {
        Self {
            node,
            mode,
            overlay: Arc::default(),
            cache: Arc::default(),
            forked_from: None,
            original: None,
            reads_original: false,
            transforms: HashMap::new(),
            compiled_path: None,
            eager_nested_maps: false,
            tainted: Arc::default(),
        }
    }

    pub fn from_node<T: InternalDbtNode + ?Sized>(node: Arc<T>, mode: SerializationMode) -> Self {
        let resource_type = node.resource_type().as_static_ref();
        let context = Self::new(node.into_jinja_fields(), mode);
        context.insert(Value::from("resource_type"), Value::from(resource_type));
        context
    }

    pub fn with_compiled_path(mut self, path: PathBuf) -> Self {
        self.compiled_path = Some(path);
        self
    }

    /// Serve nested maps the way eager node contexts did, apart from mutability. Eager parse,
    /// compile and run contexts converted node YAML with `convert_yml_to_value_map`, which made
    /// every nested map read-only and turned every mapping key into a string. Here nested maps
    /// stay mutable in place, so `model.config.update(...)` works as it does in dbt-core, but
    /// two other effects of that conversion are kept:
    ///
    /// * Mapping keys are strings, so `meta: {1: one}` is read as `meta['1']`.
    /// * `{% set x = model.config %}` binds a copy, as it does for read-only maps (see
    ///   `MutableMap::set_copy_on_store`), so writes through `x` do not reach `model`.
    ///
    /// TODO(LazyNodeContext): Both differ from dbt-core. In dbt-core, YAML keys keep their types (`meta[1]`), and
    /// `{% set %}` binds the same dict, so writes through it reach `model`. Remove this method,
    /// and `MutableMap::set_copy_on_store`, if those changes are accepted.
    pub fn with_eager_nested_maps(mut self) -> Self {
        self.eager_nested_maps = true;
        self
    }

    /// Keep a copy of each field's nested maps as first served, for `original_alias`.
    ///
    /// Why a copy at first serialization rather than a separate lazy cache for the original view:
    /// before node fields were served on demand, `model` and `config.model` were one serialized
    /// map whose lists were mutable and whose nested maps were read-only. So list appends through
    /// `model` (`model.tags.append(...)`, `model.config.tags.append(...)`) were visible through
    /// `config.model`, and templates may rely on that. Keeping it means both views must hold the
    /// same list objects, so they must come from one serialization. Each field is therefore
    /// serialized once, and the original view gets a copy of its maps, made before either view can
    /// mutate them, that still holds the same lists. This costs one structural copy of the nested
    /// maps of every field read at run time (lists, strings and other values are shared).
    ///
    /// TODO(LazyNodeContext): dbt-core gives `config.model` an independent copy of the node, so it sees no list
    /// appends made through `model`. If that change is accepted, drop this, `copy_nested_maps` and
    /// the second cache, and give `original_alias` its own lazily filled cache instead.
    pub fn with_original_view(mut self) -> Self {
        self.original = Some(Arc::default());
        self
    }

    /// Transform one field on first access, before caching its mutable Jinja containers.
    pub fn transform(
        &mut self,
        key: &str,
        transform: impl Fn(dbt_yaml::Value) -> dbt_yaml::Value + Send + Sync + 'static,
    ) {
        self.transforms.insert(key.to_string(), Arc::new(transform));
    }

    pub fn insert(&self, key: Value, value: Value) {
        self.insert_with(key, value, Overlay::insert);
    }

    fn insert_from_jinja(&self, key: Value, value: Value) {
        self.insert_with(key, value, Overlay::insert_from_jinja);
    }

    fn insert_with(&self, key: Value, value: Value, insert: fn(&mut Overlay, Value, Value)) {
        if key.is_introspective_stub() || value.is_introspective_stub() {
            self.tainted.store(true, Ordering::Relaxed);
        }
        let mut overlay = self.overlay.lock().expect("poisoned node overlay");
        insert(&mut overlay, key, value);
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        self.get_key(&Value::from(key))
    }

    fn get_key(&self, key: &Value) -> Option<Value> {
        self.get_key_with_fallback(key, true)
    }

    fn compiled_fallback(&self, key: &Value) -> Option<Value> {
        if matches!(key.as_str(), Some("compiled_code" | "compiled_sql")) {
            std::fs::read_to_string(self.compiled_path.as_ref()?)
                .ok()
                .map(Value::from)
        } else {
            None
        }
    }

    fn get_key_with_fallback(&self, key: &Value, compiled_fallback: bool) -> Option<Value> {
        let masked = {
            let overlay = self.overlay.lock().expect("poisoned node overlay");
            if let Some(value) = overlay.values.get(key) {
                return Some(value.clone());
            }
            overlay.deleted.contains(key)
        };
        if masked {
            return compiled_fallback
                .then(|| self.compiled_fallback(key))
                .flatten();
        }
        let name = key.as_str()?;
        let disk_backed = self.compiled_path.is_some()
            && matches!(name, "compiled_code" | "compiled_sql")
            && !self.node.contains_key(name, self.mode);
        let read_cache = match &self.original {
            Some(original) if self.reads_original => original,
            _ => &self.cache,
        };
        if !disk_backed
            && let Some(value) = read_cache.lock().expect("poisoned node cache").get(key)
        {
            return Some(value.clone());
        }
        let value = if disk_backed {
            compiled_fallback
                .then(|| self.compiled_fallback(key))
                .flatten()
        } else {
            self.node.get(name, self.mode).map(|value| {
                let value = match self.transforms.get(name) {
                    Some(transform) => transform(value),
                    None => value,
                };
                if self.eager_nested_maps {
                    let value = Value::from_serialize(stringify_mapping_keys(value));
                    set_copy_on_store(&value);
                    value
                } else {
                    Value::from_serialize(value)
                }
            })
        };
        // Serialization runs outside the lock. A concurrent write or deletion wins.
        let overlay = self.overlay.lock().expect("poisoned node overlay");
        if let Some(value) = overlay.values.get(key) {
            return Some(value.clone());
        }
        if overlay.deleted.contains(key) {
            return compiled_fallback
                .then(|| self.compiled_fallback(key))
                .flatten();
        }
        if !disk_backed && let Some(value) = &value {
            let first = self.forked_from.as_ref().unwrap_or(&self.cache);
            let live = first
                .lock()
                .expect("poisoned node cache")
                .entry(key.clone())
                .or_insert_with(|| value.clone())
                .clone();
            // Both views cache a field together, before either can mutate it; see
            // `with_original_view` for why the original view is a copy of this serialization.
            if let Some(original) = &self.original {
                let original = original
                    .lock()
                    .expect("poisoned node cache")
                    .entry(key.clone())
                    .or_insert_with(|| copy_nested_maps(&live))
                    .clone();
                if self.reads_original {
                    return Some(original);
                }
            }
            if self.forked_from.is_some() {
                return Some(
                    self.cache
                        .lock()
                        .expect("poisoned node cache")
                        .entry(key.clone())
                        .or_insert_with(|| copy_nested_maps(&live))
                        .clone(),
                );
            }
            return Some(live);
        }
        value
    }

    pub fn keys(&self) -> Vec<Value> {
        let overlay = self.overlay.lock().expect("poisoned node overlay");
        self.keys_from_overlay(&overlay)
    }

    fn keys_from_overlay(&self, overlay: &Overlay) -> Vec<Value> {
        if let Some(order) = &overlay.order {
            return order.clone();
        }
        let mut keys: Vec<Value> = self
            .node
            .keys(self.mode)
            .into_iter()
            .map(Value::from)
            .collect();
        keys.extend(overlay.values.keys().cloned());
        let mut seen = HashSet::new();
        keys.retain(|key| !overlay.deleted.contains(key) && seen.insert(key.clone()));
        keys
    }

    /// Preserve the initial ordering of contexts previously copied through a BTreeMap.
    pub fn sort_keys(&self) {
        let mut overlay = self.overlay.lock().expect("poisoned node overlay");
        let mut keys = self.keys_from_overlay(&overlay);
        keys.sort();
        overlay.order = Some(keys);
    }

    fn remove(&self, key: &Value) -> Option<Value> {
        let value = self.get_key_with_fallback(key, false);
        let mut overlay = self.overlay.lock().expect("poisoned node overlay");
        let keys = self.keys_from_overlay(&overlay);
        let order = overlay.order.get_or_insert(keys);
        if let Some(index) = order.iter().position(|candidate| candidate == key) {
            order.swap_remove(index);
        }
        let value = overlay.values.swap_remove(key).or(value);
        overlay.jinja_compiled.remove(key);
        overlay.deleted.insert(key.clone());
        value
    }

    /// A copy whose writes stay local, for one microbatch batch of a shared run context.
    /// Top-level keys and nested maps are its own, so writes such as `model.meta.update(...)`
    /// in one batch do not reach the original or other forks. Each field's maps are copied
    /// when the fork first reads the field. Lists stay shared, as they were when each batch
    /// copied the top-level entries of one serialized map.
    /// Disk-only SQL fallback is excluded because it is not a stored dictionary entry.
    ///
    /// TODO(LazyNodeContext): In dbt-core each batch gets its own copy of the node, so list appends do not
    /// reach other batches either.
    pub fn fork(&self) -> Self {
        let mut overlay = self.overlay.lock().expect("poisoned node overlay").clone();
        for value in overlay.values.values_mut() {
            *value = copy_nested_maps(value);
        }
        Self {
            node: self.node.clone(),
            mode: self.mode,
            overlay: Arc::new(Mutex::new(overlay)),
            cache: Arc::default(),
            forked_from: Some(
                self.forked_from
                    .clone()
                    .unwrap_or_else(|| self.cache.clone()),
            ),
            original: self.original.clone(),
            reads_original: self.reads_original,
            transforms: self.transforms.clone(),
            compiled_path: None,
            eager_nested_maps: self.eager_nested_maps,
            tainted: Arc::new(AtomicBool::new(self.tainted.load(Ordering::Relaxed))),
        }
    }

    /// Returns a second object for the same dictionary. Reads and writes go to the same data, so
    /// `alias.update(...)` is visible through the original, but Jinja treats them as different
    /// objects: `alias is sameas original` is false.
    ///
    /// Run contexts use this for `node` and `config.model`. Before node fields were serialized
    /// on demand, `model`, `node` and `config.model` were separate wrapper objects around one
    /// shared map, and `config.model` created a new wrapper on every access. This keeps
    /// `is sameas` results unchanged for templates that compare those names.
    ///
    /// TODO(LazyNodeContext): Revisit when run-phase node names are aligned with dbt-core. In dbt-core,
    /// `config.model` is the original node, a separate object that does not see `model.update(...)`,
    /// and `node` is undefined at run time. Then either expose one object under every name and
    /// remove this method, or give `config.model` a real copy.
    pub fn alias(&self) -> Self {
        Self {
            node: self.node.clone(),
            mode: self.mode,
            overlay: self.overlay.clone(),
            cache: self.cache.clone(),
            forked_from: self.forked_from.clone(),
            original: self.original.clone(),
            reads_original: self.reads_original,
            transforms: self.transforms.clone(),
            compiled_path: self.compiled_path.clone(),
            eager_nested_maps: self.eager_nested_maps,
            tainted: self.tainted.clone(),
        }
    }

    /// An alias whose nested maps keep the values they had when each field was first served,
    /// for run-time `config.model` (needs `with_original_view`; otherwise a plain `alias`).
    ///
    /// In dbt-core, `config.model` is the original node: writes to `model`'s nested maps, such as
    /// `model.config.update(...)`, do not reach it, and its own nested writes persist on it. Before
    /// node fields were served on demand, those nested maps were read-only in Fusion, so such
    /// writes failed; this matches dbt-core for them. Everything that already worked is kept:
    /// top-level keys (`model.update(...)`) and lists (`model.tags.append(...)`, including lists
    /// nested in maps) are still shared with `model`.
    ///
    /// TODO(LazyNodeContext): In dbt-core, `config.model` sees neither top-level writes nor list appends made
    /// through `model`. Give it a fully separate copy if those changes are accepted (see
    /// `with_original_view`).
    pub fn original_alias(&self) -> Self {
        let mut alias = self.alias();
        alias.reads_original = self.original.is_some();
        alias
    }
}

/// Turn scalar mapping keys into strings at every depth, as `convert_yml_to_value_map` does.
fn stringify_mapping_keys(value: dbt_yaml::Value) -> dbt_yaml::Value {
    match value {
        dbt_yaml::Value::Mapping(map, span) => dbt_yaml::Value::Mapping(
            map.into_iter()
                .map(|(key, value)| {
                    let key = match key.as_scalar_string() {
                        Some(key) => dbt_yaml::Value::string(key.into_owned()),
                        None => key,
                    };
                    (key, stringify_mapping_keys(value))
                })
                .collect(),
            span,
        ),
        dbt_yaml::Value::Sequence(items, span) => dbt_yaml::Value::Sequence(
            items.into_iter().map(stringify_mapping_keys).collect(),
            span,
        ),
        value => value,
    }
}

/// Copy maps at every depth; lists and other values stay shared.
fn copy_nested_maps(value: &Value) -> Value {
    let Some(map) = value.downcast_object_ref::<MutableMap>() else {
        return value.clone();
    };
    let copy = MutableMap::new();
    for key in map.keys() {
        if let Some(value) = map.get(&key) {
            copy.insert(key, copy_nested_maps(&value));
        }
    }
    if map.copies_on_store() {
        copy.set_copy_on_store();
    }
    Value::from_object(copy)
}

fn set_copy_on_store(value: &Value) {
    if let Some(map) = value.downcast_object_ref::<MutableMap>() {
        map.set_copy_on_store();
        map.keys()
            .iter()
            .filter_map(|key| map.get(key))
            .for_each(|value| set_copy_on_store(&value));
    } else if value.kind() == minijinja::value::ValueKind::Seq
        && let Ok(items) = value.try_iter()
    {
        items.for_each(|value| set_copy_on_store(&value));
    }
}

impl Object for LazyNodeContext {
    fn is_mutable(self: &Arc<Self>) -> bool {
        true
    }

    fn is_introspective_stub(self: &Arc<Self>) -> bool {
        self.tainted.load(Ordering::Relaxed)
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        // As with eager run contexts, Jinja writes to compiled SQL reach only `copy()`;
        // attribute, subscript and pycompat reads use the file. Rust overrides still win.
        // TODO(LazyNodeContext): Remove with `Overlay::jinja_compiled` to honor such writes as dbt-core does.
        if self.compiled_path.is_some()
            && self
                .overlay
                .lock()
                .expect("poisoned node overlay")
                .jinja_compiled
                .contains(key)
        {
            return self.compiled_fallback(key);
        }
        self.get_key(key)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        self.mapped_rev_enumerator(|this| Box::new(this.keys().into_iter()))
    }

    fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
        Some(self.keys().len())
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            // TODO(LazyNodeContext): pycompat `get` returns the default for a key whose value is none, as it does
            // for every map. dbt-core's `dict.get` returns none, so `graph.nodes[id].get('patch_path',
            // 'x')` is none there and `'x'` here.
            "get" | "keys" | "items" | "values" => {
                minijinja_contrib::pycompat::unknown_method_callback(
                    state,
                    &Value::from_dyn_object(self.clone()),
                    method,
                    args,
                )
            }
            "update" => {
                // Reuse the existing dict argument parser; only incoming fields are materialized.
                let incoming = Arc::new(MutableMap::new());
                Object::call_method(&incoming, state, method, args, listeners)?;
                if Object::is_introspective_stub(&incoming) {
                    self.tainted.store(true, Ordering::Relaxed);
                }
                let mut overlay = self.overlay.lock().expect("poisoned node overlay");
                for key in incoming.keys() {
                    overlay.insert_from_jinja(key.clone(), incoming.get(&key).unwrap());
                }
                Ok(Value::from_dyn_object(self.clone()))
            }
            "setdefault" | "pop" | "popitem" => {
                // Validate arguments and preserve return/error behavior through the map
                // implementation, materializing only the affected entry.
                let incoming = Arc::new(MutableMap::new());
                let key = if method == "popitem" {
                    self.keys().pop()
                } else {
                    args.first().cloned()
                };
                if let Some(key) = &key
                    && let Some(value) = self.get_key_with_fallback(key, false)
                {
                    incoming.insert(key.clone(), value);
                }
                let result = Object::call_method(&incoming, state, method, args, listeners)?;
                if let Some(key) = key {
                    if method == "setdefault" {
                        if self.get_key_with_fallback(&key, false).is_none() {
                            self.insert_from_jinja(key, result.clone());
                        }
                    } else {
                        self.remove(&key);
                    }
                }
                Ok(result)
            }
            "clear" => {
                Object::call_method(&Arc::new(MutableMap::new()), state, method, args, listeners)?;
                let mut overlay = self.overlay.lock().expect("poisoned node overlay");
                overlay.values.clear();
                overlay.deleted = self
                    .node
                    .keys(self.mode)
                    .into_iter()
                    .map(Value::from)
                    .collect();
                overlay.jinja_compiled.clear();
                overlay.order = Some(Vec::new());
                Ok(Value::default())
            }
            "copy" => {
                Object::call_method(&Arc::new(MutableMap::new()), state, method, args, listeners)?;
                let entries: ValueMap = self
                    .keys()
                    .into_iter()
                    .filter_map(|key| self.get_key(&key).map(|value| (key, value)))
                    .collect();
                Ok(Value::from_object(MutableMap::from(entries)))
            }
            _ => {
                if let Some(value) = self.get(method) {
                    value.call(state, args, listeners)
                } else {
                    Err(Error::new(
                        ErrorKind::UnknownMethod,
                        format!("MutableMap has no method named {method}"),
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::*;
    use indexmap::IndexMap;
    use minijinja::Environment;

    #[derive(Debug, Default, Serialize, dbt_proc_macros::JinjaNodeFields)]
    struct Fields {
        name: String,
        group: Option<String>,
        nested: IndexMap<String, Option<String>>,
        #[serde(rename = "renamed")]
        original: String,
        #[serde(skip)]
        hidden: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        skipped: Option<String>,
        #[serde(serialize_with = "crate::schemas::serde::serialize_none_as_empty_map")]
        meta: Option<IndexMap<String, dbt_yaml::Value>>,
        null_value: dbt_yaml::Value,
        #[serde(serialize_with = "serialize_as_null")]
        custom_null: Option<String>,
    }

    fn serialize_as_null<S: ::serde::Serializer>(
        _: &Option<String>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_none()
    }

    #[derive(Debug, Default, Serialize, dbt_proc_macros::JinjaNodeFields)]
    struct Flattened {
        name: String,
        #[serde(flatten)]
        fields: Fields,
        #[serde(flatten)]
        __other__: std::collections::BTreeMap<String, dbt_yaml::Value>,
    }

    fn assert_serde_parity<T: Serialize + JinjaNodeFields>(node: &T) {
        for mode in [SerializationMode::KeepNone, SerializationMode::OmitNone] {
            let serialized = serialize_with_mode(node, mode);
            let mapping = serialized.as_mapping().unwrap();
            let expected_keys: Vec<_> = mapping.keys().map(|key| key.as_str().unwrap()).collect();
            assert_eq!(
                node.keys(mode),
                expected_keys,
                "{} {mode:?}",
                std::any::type_name::<T>()
            );
            for &key in &expected_keys {
                assert!(node.contains_key(key, mode), "{key} {mode:?}");
                assert_eq!(
                    node.get(key, mode).as_ref(),
                    serialized.get(key),
                    "{key} {mode:?}"
                );
            }
            assert!(!node.contains_key("missing_field", mode));
            assert!(node.get("missing_field", mode).is_none());
        }
    }

    #[test]
    fn jinja_node_fields_match_serde_attributes_and_flatten_precedence() {
        for populated in [false, true] {
            let mut fields = Fields {
                name: "inner".into(),
                original: "renamed value".into(),
                hidden: "hidden value".into(),
                nested: IndexMap::from([
                    ("empty".into(), None),
                    ("present".into(), Some("value".into())),
                ]),
                ..Default::default()
            };
            if populated {
                fields.group = Some("group".into());
                fields.custom_null = Some("serialized as null".into());
                fields.skipped = Some("included".into());
                fields.meta = Some(IndexMap::from([(
                    "owner".into(),
                    dbt_yaml::Value::string("analytics".into()),
                )]));
            }
            assert_eq!(fields.hidden, "hidden value");
            assert_serde_parity(&fields);
            let mut flattened = Flattened {
                name: "outer".into(),
                fields,
                ..Default::default()
            };
            assert_serde_parity(&flattened);
            flattened
                .__other__
                .insert("name".into(), dbt_yaml::Value::string("override".into()));
            flattened
                .__other__
                .insert("group".into(), dbt_yaml::Value::default());
            flattened
                .__other__
                .insert("extra".into(), dbt_yaml::Value::string("extra".into()));
            assert_serde_parity(&flattened);
        }
    }

    #[test]
    fn jinja_node_fields_match_serde_node_types() {
        assert_serde_parity(&DbtModel::default());
        assert_serde_parity(&DbtSeed::default());
        assert_serde_parity(&DbtSnapshot::default());
        assert_serde_parity(&DbtTest::default());
        assert_serde_parity(&DbtSource::default());
        assert_serde_parity(&DbtUnitTest::default());
        assert_serde_parity(&DbtExposure::default());
        assert_serde_parity(&DbtCheck::default());
        assert_serde_parity(&DbtFunction::default());
        assert_serde_parity(&macros::DbtMacro::default());
        assert_serde_parity(&manifest::metric::DbtMetric::default());
        assert_serde_parity(&manifest::semantic_model::DbtSemanticModel::default());
        assert_serde_parity(&manifest::saved_query::DbtSavedQuery::default());
        assert_serde_parity(&manifest::DbtOperation::default());
        let mut model = DbtModel::default();
        model.__common_attr__.name = "populated".into();
        model.__common_attr__.description = Some("description".into());
        model.__common_attr__.raw_code = Some("select 1".into());
        model
            .__other__
            .insert("name".into(), dbt_yaml::Value::string("override".into()));
        model
            .__other__
            .insert("description".into(), dbt_yaml::Value::default());
        assert_serde_parity(&model);
    }

    fn compare_dicts(template: &str, mode: SerializationMode) {
        let fields = Arc::new(Fields {
            name: "original".into(),
            ..Default::default()
        });
        let eager = Value::from_serialize(serialize_with_mode(&fields, mode));
        let lazy = Value::from_object(LazyNodeContext::new(fields, mode));
        let mut env = Environment::new();
        env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
        let render = |value| {
            env.render_str(
                template,
                std::collections::BTreeMap::from([("node", value)]),
                &[],
            )
        };
        match (render(eager), render(lazy)) {
            (Ok(expected), Ok(actual)) => assert_eq!(actual, expected, "{template} {mode:?}"),
            (Err(expected), Err(actual)) => {
                assert_eq!(actual.kind(), expected.kind(), "{template}");
                assert_eq!(actual.detail(), expected.detail(), "{template}");
            }
            (expected, actual) => {
                panic!("{template} {mode:?}: expected {expected:?}, got {actual:?}")
            }
        }
    }

    #[test]
    fn lazy_node_dict_methods_match_eager_maps() {
        let templates = [
            "{{ node.get('group', 'fallback') }}|{{ node.get('missing', 'fallback') }}|{{ node.get('group') }}|{{ node.get('name', 'fallback') }}",
            "{% set keys = node.keys() %}{% set items = node.items() %}{% set values = node.values() %}{% do node.update({'extra': 'new', 'name': 'updated'}) %}{{ keys }}|{{ items }}|{{ values }}|{{ keys | list }}|{{ items | list }}|{{ values | list }}|{{ node | length }}",
            "{{ node.setdefault('name', 'ignored') }}|{{ node.setdefault('group', 'default') }}|{{ node.setdefault('added', 'new') }}|{{ node }}",
            "{% do node.update([['first', 1], ['second', 2]], third=3) %}{{ node }}|{{ node.pop('name') }}|{{ node }}|{{ node.popitem() }}|{{ node.pop('missing') }}|{{ node.pop('missing', 'default') }}",
            "{% set copy = node.copy() %}{% do node.update({'name': 'changed'}) %}{{ copy.name }}|{{ node.name }}|{{ node.to_dict() }}|{{ node.meta_get('missing', default='fallback') }}",
            "{% do node.clear() %}{{ node }}|{{ node | length }}|{{ node.get('name', 'empty') }}|{{ node.setdefault('name', 'new') }}",
            "{% do node.update({'get': 'shadowed', 'items': 'shadowed'}) %}{{ node.get('name') }}|{{ node.items() | list }}",
            "{{ node.get() }}",
            "{{ node.get('a', 'b', 'c') }}",
            "{{ node.setdefault('missing') }}",
            "{{ node.setdefault() }}",
            "{{ node.pop() }}",
            "{{ node.pop('a', 'b', 'c') }}",
            "{{ node.popitem(1) }}",
            "{{ node.clear(1) }}",
            "{{ node.copy(1) }}",
            "{{ node.keys(1) }}",
            "{{ node.items(1) }}",
            "{{ node.values(1) }}",
            "{% do node.clear() %}{{ node.popitem() }}",
            "{{ node.update(none) }}",
            "{{ node.update(1, 2) }}",
        ];
        for mode in [SerializationMode::KeepNone, SerializationMode::OmitNone] {
            for template in templates {
                compare_dicts(template, mode);
            }
        }
    }

    #[test]
    fn lazy_node_serializes_only_requested_fields_and_shares_fork_cache() {
        #[derive(Debug)]
        struct CountingFields {
            fields: Fields,
            reads: std::sync::atomic::AtomicUsize,
        }
        impl JinjaNodeFields for CountingFields {
            fn get(&self, key: &str, mode: SerializationMode) -> Option<dbt_yaml::Value> {
                self.reads.fetch_add(1, Ordering::Relaxed);
                self.fields.get(key, mode)
            }
            fn keys(&self, mode: SerializationMode) -> Vec<&str> {
                self.fields.keys(mode)
            }
            fn contains_key(&self, key: &str, mode: SerializationMode) -> bool {
                self.fields.contains_key(key, mode)
            }
        }
        let fields = Arc::new(CountingFields {
            fields: Fields {
                name: "model".into(),
                ..Default::default()
            },
            reads: std::sync::atomic::AtomicUsize::new(0),
        });
        let node = Arc::new(LazyNodeContext::new(
            fields.clone(),
            SerializationMode::KeepNone,
        ));
        let env = Environment::new();
        let template = env.template_from_str("").unwrap();
        let state = template.eval_to_state((), &[]).unwrap();
        for method in ["keys", "items", "values"] {
            Object::call_method(&node, &state, method, &[], &[]).unwrap();
        }
        assert_eq!(fields.reads.load(Ordering::Relaxed), 0);
        node.insert(Value::from("new_field"), Value::from("new value"));
        assert_eq!(fields.reads.load(Ordering::Relaxed), 0);
        assert_eq!(node.get("name"), Some(Value::from("model")));
        assert_eq!(node.get("name"), Some(Value::from("model")));
        assert_eq!(fields.reads.load(Ordering::Relaxed), 1);
        let fork = node.fork();
        fork.get("nested").unwrap();
        node.get("nested").unwrap();
        assert_eq!(fields.reads.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn forks_keep_nested_map_writes_local_but_share_lists() {
        let node = LazyNodeContext::new(
            Arc::new(Fields {
                nested: IndexMap::from([("key".into(), Some("value".into()))]),
                ..Default::default()
            }),
            SerializationMode::KeepNone,
        );
        let tags = minijinja::value::mutable_vec::MutableVec::new();
        tags.push(Value::from("a"));
        node.insert(Value::from("tags"), Value::from_object(tags));
        node.insert(Value::from("extra"), Value::from_object(MutableMap::new()));
        let ctx = std::collections::BTreeMap::from([
            ("first", Value::from_object(node.fork())),
            ("second", Value::from_object(node.fork())),
            ("node", Value::from_object(node)),
        ]);
        let env = Environment::new();
        assert_eq!(
            env.render_str(
                "{% do first.nested.update({'key': 'first'}) %}{% do first.extra.update({'x': 1}) %}\
                 {% do first.tags.append('b') %}\
                 {{ second.nested.key }}|{{ node.nested.key }}|{{ second.extra }}|{{ node.extra }}|\
                 {{ second.tags }}|{{ first.nested.key }}",
                ctx,
                &[]
            )
            .unwrap(),
            "value|value|{}|{}|['a', 'b']|first"
        );
    }
}
