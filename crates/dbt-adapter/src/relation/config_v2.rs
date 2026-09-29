//! NOTE: This module will replace config.rs, but for backwards compat reasons I kept both
//! until everything is migrated.
//!
//! This module contains the primitives for describing and diffing relation configurations.
//!
//! 1. There is a single `RelationConfig` which contains a set of `RelationConfigComponent`. All
//!    components are type erased to avoid lots of specialization for every component type of every
//!    warehouse platform.
//!
//! 2. Each component can be read either from a remote dataset (the current applied state) or from
//!    local configuration (the desired state).
//!
//! 3. Each component implements its own diffing logic to generate the diff between applied and
//!    desired states. This can be used by macros to generate ALTER statements.
//!
//! 4. A `RelationComponentConfigChangeSet` aggregates diffs from many components, and represents the
//!    changeset needed to take the current state to the desired state.
//!
//! 5. Each warehouse implements its own `<Warehouse>RelationConfigObject` that wraps a
//!    `RelationConfig` but captures historical differences in Jinja implementations across
//!    adapters.

use crate::errors::AdapterResult;
use crate::value::none_value;

use dbt_adapter_core::AdapterType;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use indexmap::{IndexMap, map::Iter as IndexMapIter};
use minijinja::{
    arg_utils::ArgParser,
    listener::RenderingEventListener,
    value::{Enumerator, Object, Value, ValueMap},
};
use minijinja_contrib::dyn_object::DynJinjaObject;
use std::{any::Any, fmt, rc::Rc, sync::Arc};

pub trait ComponentConfig: fmt::Debug + Send + Sync + Any {
    /// Assuming self is the desired state, get the diff that takes the current state to the desired state.
    ///
    /// Returns None if no change was detected.
    fn diff_from(
        &self,
        current_state: Option<&dyn ComponentConfig>,
    ) -> Option<Box<dyn ComponentConfig>>;

    /// The unique name that identifies this component's type
    fn type_name(&self) -> &'static str;

    fn as_any(&self) -> &dyn Any;

    fn to_jinja(&self) -> Value;
}

/// Contains custom diffing functions that can be used by component implementations
pub(crate) mod diff {
    use super::*;
    use std::hash::Hash;

    /// The signature of a diff function
    pub(crate) type DiffFn<T> = fn(&T, &T) -> Option<T>;

    /// The state is immutable; the diff is always None
    pub(crate) fn immutable<T>(_desired_state: &T, _current_state: &T) -> Option<T> {
        None
    }

    /// The resulting diff is simply a clone of the desired state
    pub(crate) fn desired_state<T: Sized + PartialEq + Clone>(
        desired_state: &T,
        current_state: &T,
    ) -> Option<T> {
        if desired_state != current_state {
            Some(desired_state.clone())
        } else {
            None
        }
    }

    /// Compare an optional string against its readback under a caller-supplied equality, for
    /// components whose value survives Snowflake's normalization only up to that equality.
    pub(crate) fn optional_by(
        desired: &Option<String>,
        current: &Option<String>,
        eq: impl Fn(&str, &str) -> bool,
    ) -> Option<Option<String>> {
        let equal = match (desired, current) {
            (Some(desired), Some(current)) => eq(desired, current),
            (None, None) => true,
            _ => false,
        };
        if equal { None } else { Some(desired.clone()) }
    }

    /// The resulting diff contains only the keys:
    /// 1. that are present in the desired state but not the current state; or
    /// 2. whose value in the desired state does not match the current state.
    /// 3. whose value is not present in the desired state but it is in current state,
    ///    (new state assumes Default).
    #[allow(dead_code)]
    pub(crate) fn changed_keys<K, V>(
        desired_state: &IndexMap<K, V>,
        current_state: &IndexMap<K, V>,
    ) -> Option<IndexMap<K, V>>
    where
        K: Clone + Eq + Hash,
        V: Clone + Eq + Default,
    {
        let mut diff: IndexMap<K, V> = desired_state
            .iter()
            .filter_map(|(k, v)| {
                if Some(v) != current_state.get(k) {
                    Some((k.clone(), v.clone()))
                } else {
                    None
                }
            })
            .collect();

        for current_k in current_state.keys() {
            if !desired_state.contains_key(current_k) {
                diff.insert(current_k.clone(), V::default());
            }
        }

        if diff.is_empty() { None } else { Some(diff) }
    }
}

/// Contains shared function implementations for Jinja objects
mod jinja {
    use super::ComponentConfig;
    use crate::value::none_value;
    use minijinja::value::{Value, ValueMap};

    pub fn bigquery_as_ddl_dict<'a>(
        iter: impl Iterator<Item = (&'a str, &'a dyn ComponentConfig)>,
    ) -> Result<Value, minijinja::Error> {
        use crate::relation::bigquery::config::components;

        let mut vm = ValueMap::new();
        let none = none_value();
        for (name, component) in iter {
            if matches!(
                name,
                components::partition_by::TYPE_NAME | components::cluster_by::TYPE_NAME
            ) {
                continue;
            }

            // Python BigQuery adapter flattens refresh keys
            if name == components::refresh::TYPE_NAME {
                let inner_vm = component.to_jinja().downcast_object::<ValueMap>().unwrap();
                for (k, v) in inner_vm.iter() {
                    vm.insert(k.clone(), v.clone());
                }
                continue;
            }

            let jinja = component.to_jinja();
            if jinja != none {
                vm.insert(Value::from(name), component.to_jinja());
            }
        }

        Ok(Value::from(vm))
    }
}

/// A function that takes a config component value and turns it into a Jinja `Value`
pub type ToJinjaFn<T> = fn(&T) -> Value;

#[derive(Clone, Debug)]
pub(crate) struct SimpleComponentConfigImpl<T: fmt::Debug + Send + Sync + Any + Clone> {
    // TODO(serramatutu): maybe dynamic dispatch here with dyn T
    pub type_name: &'static str,
    pub diff_fn: diff::DiffFn<T>,
    pub to_jinja_fn: ToJinjaFn<T>,
    pub value: T,
}

impl<T: fmt::Debug + Send + Sync + Any + Clone> ComponentConfig for SimpleComponentConfigImpl<T> {
    fn diff_from(
        &self,
        current_state: Option<&dyn ComponentConfig>,
    ) -> Option<Box<dyn ComponentConfig>> {
        let current_state = match current_state {
            Some(current_state) => current_state.as_any().downcast_ref::<Self>(),
            None => return Some(Box::new(self.clone())),
        };

        let value = match current_state {
            Some(current_state) => &current_state.value,
            None => {
                debug_assert!(
                    false,
                    "type of value passed to SimpleComponentConfigImpl::diff() is incorrect"
                );
                return None;
            }
        };

        if let Some(diff) = (self.diff_fn)(&self.value, value) {
            let self_clone = Self {
                type_name: self.type_name,
                diff_fn: self.diff_fn,
                to_jinja_fn: self.to_jinja_fn,
                value: diff,
            };

            Some(Box::new(self_clone))
        } else {
            None
        }
    }

    fn type_name(&self) -> &'static str {
        self.type_name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn to_jinja(&self) -> Value {
        (self.to_jinja_fn)(&self.value)
    }
}

/// Represents a change in a certain configuration by comparing the applied state with the desired state
#[derive(Debug)]
pub enum ComponentConfigChange {
    /// The config has changed
    Some(Box<dyn ComponentConfig>),
    /// The config used to exist but has been dropped
    Drop,
    /// There were no detected changes
    None,
}

impl ComponentConfigChange {
    fn to_jinja(&self) -> Value {
        match self {
            Self::Some(v) => v.to_jinja(),
            _ => none_value(),
        }
    }
}

/// A function that evaluates a set of components that have been changed and returns whether or not
/// those will require a full refresh
pub(crate) type RequiresFullRefreshFn = fn(&IndexMap<&'static str, ComponentConfigChange>) -> bool;
pub(crate) type CompleteChangesetKeyFn = fn(&'static str) -> Option<&'static str>;

#[derive(Debug)]
pub struct RelationConfig {
    adapter_type: AdapterType,
    components: IndexMap<&'static str, Box<dyn ComponentConfig>>,
    requires_full_refresh_fn: RequiresFullRefreshFn,
    complete_changeset_key_fn: Option<CompleteChangesetKeyFn>,
}

impl RelationConfig {
    pub fn new(
        adapter_type: AdapterType,
        configs: impl IntoIterator<Item = Box<dyn ComponentConfig>>,
        requires_full_refresh_fn: RequiresFullRefreshFn,
    ) -> Self {
        Self {
            adapter_type,
            components: configs
                .into_iter()
                .map(|cfg| (cfg.type_name(), cfg))
                .collect(),
            requires_full_refresh_fn,
            complete_changeset_key_fn: None,
        }
    }

    fn with_complete_changeset(
        mut self,
        complete_changeset_key_fn: Option<CompleteChangesetKeyFn>,
    ) -> Self {
        self.complete_changeset_key_fn = complete_changeset_key_fn;
        self
    }

    pub(crate) fn adapter_type(&self) -> AdapterType {
        self.adapter_type
    }

    pub(crate) fn components(&self) -> IndexMapIter<'_, &'static str, Box<dyn ComponentConfig>> {
        self.components.iter()
    }
}

impl RelationConfig {
    pub(crate) fn get<'a>(
        &'a self,
        component_type_name: &'static str,
    ) -> Option<&'a dyn ComponentConfig> {
        self.components
            .get(&component_type_name)
            .map(|inner| inner.as_ref())
    }

    fn complete_changeset(
        &self,
        mut changes: IndexMap<&'static str, ComponentConfigChange>,
    ) -> IndexMap<&'static str, ComponentConfigChange> {
        let Some(changeset_key) = self.complete_changeset_key_fn else {
            return changes;
        };
        if changes.is_empty() {
            return changes;
        }

        let mut complete = IndexMap::new();
        for (type_name, desired_component) in &self.components {
            let Some(key) = changeset_key(type_name) else {
                continue;
            };
            let change = changes.shift_remove(type_name).unwrap_or_else(|| {
                ComponentConfigChange::Some(
                    desired_component
                        .diff_from(None)
                        .expect("a desired component with no current state must produce a change"),
                )
            });
            complete.insert(key, change);
        }
        complete.extend(changes);
        complete
    }

    fn sparse_diff(
        &self,
        current_state: &RelationConfig,
    ) -> IndexMap<&'static str, ComponentConfigChange> {
        let changed = self.components.iter().filter_map(|(type_name, desired)| {
            desired
                .diff_from(current_state.get(type_name))
                .map(|diff| (*type_name, ComponentConfigChange::Some(diff)))
        });
        let dropped = current_state
            .components
            .keys()
            .filter(|type_name| !self.components.contains_key(*type_name))
            .map(|type_name| (*type_name, ComponentConfigChange::Drop));

        changed.chain(dropped).collect()
    }

    /// Get the diff that takes the current state to the desired state
    pub fn diff(
        desired_state: &RelationConfig,
        current_state: &RelationConfig,
    ) -> RelationComponentConfigChangeSet {
        debug_assert!(desired_state.adapter_type == current_state.adapter_type);

        let diffs = desired_state.sparse_diff(current_state);

        let requires_full_refresh = (desired_state.requires_full_refresh_fn)(&diffs);

        let diffs = desired_state.complete_changeset(diffs);

        RelationComponentConfigChangeSet::from_changes(
            desired_state.adapter_type,
            diffs,
            requires_full_refresh,
        )
    }
}

impl Object for RelationConfig {
    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State,
        name: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        use AdapterType::Databricks;

        let mut parser = ArgParser::new(args, None);
        match (&self.adapter_type, name) {
            (Databricks, "get_changeset") => {
                let val = if let Some(existing) = parser
                    .get::<Value>("existing_relation")?
                    .downcast_object::<RelationConfig>()
                {
                    let change_set = RelationConfig::diff(self.as_ref(), existing.as_ref());
                    if !change_set.is_empty() {
                        let intermediate_map = Value::from(ValueMap::from([
                            (
                                Value::from("requires_full_refresh"),
                                Value::from(change_set.requires_full_refresh()),
                            ),
                            (Value::from("changes"), Value::from_object(change_set)),
                        ]));
                        Value::from_serialize(intermediate_map)
                    } else {
                        none_value()
                    }
                } else {
                    none_value()
                };

                Ok(val)
            }
            (_, _) => unimplemented!("RelationConfigBaseObject does not support method: {}", name),
        }
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        use AdapterType::Bigquery;

        match (self.adapter_type, key.as_str()?) {
            (Bigquery, "options") => {
                let obj = DynJinjaObject::<(), Self>::new_arc(
                    "BigqueryMaterializedViewOptions",
                    (),
                    self.clone(),
                )
                .with_method("as_ddl_dict", |obj, _state, _args, _listeners| {
                    jinja::bigquery_as_ddl_dict(
                        obj.repr_ref()
                            .components
                            .iter()
                            .map(|(k, v)| (*k, v.as_ref())),
                    )
                });

                Some(Value::from_object(obj))
            }
            (_, _) => {
                let key = key.as_str()?;
                self.components.get(key).map(|v| v.to_jinja())
            }
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Values(self.components.keys().map(|v| Value::from(*v)).collect())
    }
}

/// Loads a `ComponentConfig` from the remote data platform state (current state)
/// or from the local configs (desired state).
pub(crate) trait ComponentConfigLoader<R> {
    /// Load the current applied state for the component given the remote state
    #[expect(clippy::wrong_self_convention)]
    fn from_remote_state(&self, remote_state: &R) -> AdapterResult<Box<dyn ComponentConfig>>;

    /// Load the desired component state from local dbt configs
    #[expect(clippy::wrong_self_convention)]
    fn from_local_config(
        &self,
        relation_config: &dyn InternalDbtNodeAttributes,
    ) -> AdapterResult<Box<dyn ComponentConfig>>;

    #[cfg(test)]
    /// The unique type name of the component loaded by this loader
    fn type_name(&self) -> &'static str;
}

/// Generate the impl block for a ComponentConfigLoader
///
/// It requires `TYPE_NAME`, `from_remote_state()`, `from_local_config()` to
/// be defined in the current scope.
macro_rules! impl_loader {
    ($component_name:ident, $remote_type:ident) => (
        paste::paste! {
            pub(crate) struct [<$component_name Loader>];
            impl ComponentConfigLoader<$remote_type> for [<$component_name Loader>] {
                #[cfg(test)]
                fn type_name(&self) -> &'static str {
                    TYPE_NAME
                }

                fn from_remote_state(&self, remote_state: &$remote_type) -> AdapterResult<Box<dyn ComponentConfig>> {
                    Ok(Box::new(from_remote_state(remote_state)?))
                }

                fn from_local_config(
                    &self,
                    relation_config: &dyn InternalDbtNodeAttributes,
                ) -> AdapterResult<Box<dyn ComponentConfig>> {
                    Ok(Box::new(from_local_config(relation_config)?))
                }
            }
        }
    )
}

pub(crate) use impl_loader;

/// Holds a collection of `ComponentConfigLoader` to populate a `RelationConfig`
/// by loading each of its components one by one
pub(crate) struct RelationConfigLoader<'a, R> {
    adapter_type: AdapterType,
    component_loaders: Vec<Box<dyn ComponentConfigLoader<R> + 'a>>,
    requires_full_refresh_fn: RequiresFullRefreshFn,
    complete_changeset_key_fn: Option<CompleteChangesetKeyFn>,
}

impl<'a, R> RelationConfigLoader<'a, R> {
    pub(crate) fn new(
        adapter_type: AdapterType,
        component_loaders: impl IntoIterator<Item = Box<dyn ComponentConfigLoader<R> + 'a>>,
        requires_full_refresh_fn: RequiresFullRefreshFn,
    ) -> Self {
        Self {
            adapter_type,
            component_loaders: component_loaders.into_iter().collect(),
            requires_full_refresh_fn,
            complete_changeset_key_fn: None,
        }
    }

    /// Include selected desired components when any component changed.
    ///
    /// Some adapter renderers rebuild a relation from a changeset and therefore
    /// need complete desired state rather than only the components that differ.
    pub(crate) fn with_complete_changeset(
        mut self,
        complete_changeset_key_fn: CompleteChangesetKeyFn,
    ) -> Self {
        self.complete_changeset_key_fn = Some(complete_changeset_key_fn);
        self
    }

    fn relation_config(&self, components: Vec<Box<dyn ComponentConfig>>) -> RelationConfig {
        RelationConfig::new(self.adapter_type, components, self.requires_full_refresh_fn)
            .with_complete_changeset(self.complete_changeset_key_fn)
    }

    /// Load the current applied state for the relation and all its components given the remote state
    #[expect(clippy::wrong_self_convention)]
    pub(crate) fn from_remote_state(&self, remote_state: &R) -> AdapterResult<RelationConfig> {
        let components = self
            .component_loaders
            .iter()
            .map(|l| l.from_remote_state(remote_state))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.relation_config(components))
    }

    /// Load the desired relation state from local dbt configs
    #[expect(clippy::wrong_self_convention)]
    pub(crate) fn from_local_config(
        &self,
        relation_config: &dyn InternalDbtNodeAttributes,
    ) -> AdapterResult<RelationConfig> {
        let components = self
            .component_loaders
            .iter()
            .map(|l| l.from_local_config(relation_config))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.relation_config(components))
    }
}

#[derive(Debug)]
pub struct RelationComponentConfigChangeSet {
    adapter_type: AdapterType,
    changes: IndexMap<&'static str, ComponentConfigChange>,
    requires_full_refresh: bool,
    forced_full_refresh: bool,
}

impl RelationComponentConfigChangeSet {
    pub fn new(
        adapter_type: AdapterType,
        changes: impl Into<IndexMap<&'static str, ComponentConfigChange>>,
        requires_full_refresh_fn: RequiresFullRefreshFn,
    ) -> Self {
        let changes = changes.into();
        let requires_full_refresh = requires_full_refresh_fn(&changes);
        Self::from_changes(adapter_type, changes, requires_full_refresh)
    }

    #[cfg(test)]
    pub(crate) fn new_with_requires_full_refresh(
        adapter_type: AdapterType,
        changes: impl Into<IndexMap<&'static str, ComponentConfigChange>>,
        requires_full_refresh: bool,
    ) -> Self {
        Self::from_changes(adapter_type, changes.into(), requires_full_refresh)
    }

    fn from_changes(
        adapter_type: AdapterType,
        changes: IndexMap<&'static str, ComponentConfigChange>,
        requires_full_refresh: bool,
    ) -> Self {
        Self {
            adapter_type,
            changes,
            requires_full_refresh,
            forced_full_refresh: false,
        }
    }

    /// Require a full refresh regardless of what `requires_full_refresh_fn` concludes.
    ///
    /// `RequiresFullRefreshFn` sees only the changeset, and a component diff carries just the
    /// desired value, so a transition that is only recognizable by comparing both
    /// configuration states cannot be detected from inside it. The caller holding both
    /// states decides those cases and marks the changeset here.
    pub(crate) fn forcing_full_refresh(mut self) -> Self {
        self.forced_full_refresh = true;
        self
    }

    pub fn len(&self) -> usize {
        self.changes.len()
    }

    pub fn adapter_type(&self) -> AdapterType {
        self.adapter_type
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> IndexMapIter<'_, &'static str, ComponentConfigChange> {
        self.changes.iter()
    }

    pub fn get<'a>(&'a self, component_type_name: &'static str) -> &'a ComponentConfigChange {
        self.changes
            .get(&component_type_name)
            .unwrap_or(&ComponentConfigChange::None)
    }

    /// Whether applying this config to an existing table requires a full refresh
    pub fn requires_full_refresh(&self) -> bool {
        self.forced_full_refresh || self.requires_full_refresh
    }
}

impl Object for RelationComponentConfigChangeSet {
    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State,
        name: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        // TODO: ArgsIter
        let mut parser = ArgParser::new(args, None);
        match name {
            // support example `_configuration_changes.changes.get("tags", None)`
            "get" => {
                let key = parser.get::<Value>("key")?;
                let key = key.as_str().ok_or_else(|| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::InvalidArgument,
                        "key must be a string",
                    )
                })?;

                Ok(self
                    .changes
                    .get(key)
                    .map(|v| v.to_jinja())
                    .unwrap_or_else(none_value))
            }
            "has_changes" => Ok(Value::from(!self.changes.is_empty())),
            "requires_full_refresh" => Ok(Value::from(self.requires_full_refresh())),
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("RelationComponentConfigChangeSet has no method named '{name}'"),
            )),
        }
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        use AdapterType::{Bigquery, Snowflake};
        match (self.adapter_type, key.as_str()?) {
            // Reference: https://github.com/dbt-labs/dbt-adapters/blob/bd80e5a9d4b7b3b0200872892ff41994586c72ef/dbt-bigquery/src/dbt/adapters/bigquery/relation_configs/_options.py#L190
            (Bigquery, "options") => {
                if self.changes.is_empty() {
                    None
                } else {
                    let obj = DynJinjaObject::<(), ValueMap>::empty("BigqueryOptionsConfigChange")
                        .with_repr(ValueMap::from([(
                            "context".into(),
                            Value::from_object(
                                DynJinjaObject::<(), Self>::new_arc(
                                    "BigqueryMaterializedViewOptions",
                                    (),
                                    self.clone(),
                                )
                                .with_method(
                                    "as_ddl_dict",
                                    |obj, _state, _args, _listeners| {
                                        jinja::bigquery_as_ddl_dict(
                                            obj.repr_ref().changes.iter().filter_map(
                                                |(name, change)| match change {
                                                    ComponentConfigChange::Some(c) => {
                                                        Some((*name, c.as_ref()))
                                                    }
                                                    ComponentConfigChange::None => None,
                                                    ComponentConfigChange::Drop => None,
                                                },
                                            ),
                                        )
                                    },
                                ),
                            ),
                        )]));
                    Some(Value::from_object(obj))
                }
            }
            (_, "requires_full_refresh") => Some(Value::from(self.requires_full_refresh())),
            // Reference: https://github.com/dbt-labs/dbt-adapters/blob/cb1b4a0b0758fd307dc21583bb3acfc78397a077/dbt-snowflake/src/dbt/adapters/snowflake/relation_configs/dynamic_table.py#L250
            // All Snowflake config changesets are like this, so we inject the `context` after calling to_jinja
            (Snowflake, key) => {
                if self.changes.is_empty() {
                    None
                } else {
                    let context_value = self.changes.get(key).map(|v| v.to_jinja())?;
                    Some(Value::from(ValueMap::from([(
                        "context".into(),
                        context_value,
                    )])))
                }
            }
            (_, key) => self.changes.get(key).map(|v| v.to_jinja()),
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Iter(Box::new(
            self.changes
                .keys()
                .map(|v| Value::from(*v))
                .collect::<Vec<_>>()
                .into_iter(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl<T: PartialEq + fmt::Debug + Send + Sync + Any + Clone> PartialEq
        for SimpleComponentConfigImpl<T>
    {
        fn eq(&self, other: &Self) -> bool {
            self.value == other.value
        }
    }

    impl<T: Eq + fmt::Debug + Send + Sync + Any + Clone> Eq for SimpleComponentConfigImpl<T> {}

    fn custom_diff(desired_state: &u8, current_state: &u8) -> Option<u8> {
        let diff = desired_state - current_state;
        if diff != 0 { Some(diff) } else { None }
    }

    fn assert_dyn_eq<T: Clone + Eq + fmt::Debug + 'static>(a: T, b: Box<dyn ComponentConfig>) {
        assert_eq!(
            a,
            b.as_ref()
                .as_any()
                .downcast_ref::<T>()
                .expect("Downcast failed")
                .clone()
        )
    }

    fn assert_component_config_change_eq<T: fmt::Debug + PartialEq + 'static>(
        a: &ComponentConfigChange,
        b: &ComponentConfigChange,
    ) {
        assert_eq!(std::mem::discriminant(a), std::mem::discriminant(b));
        if let (ComponentConfigChange::Some(a_diff), ComponentConfigChange::Some(b_diff)) = (a, b) {
            assert_eq!(
                a_diff.as_any().downcast_ref::<T>(),
                b_diff.as_any().downcast_ref::<T>()
            );
        }
    }

    fn return_true(_: &IndexMap<&'static str, ComponentConfigChange>) -> bool {
        true
    }

    const TYPE_NAME: &str = "mock";

    type MockComponent = SimpleComponentConfigImpl<u8>;

    fn to_jinja(v: &u8) -> Value {
        Value::from(*v)
    }

    #[test]
    fn simple_diff_config_created() {
        let next = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: diff::desired_state,
            to_jinja_fn: to_jinja,
            value: 1,
        };
        let diff = ComponentConfig::diff_from(&next, None).unwrap();
        assert_dyn_eq(next, diff);
    }

    #[test]
    fn simple_diff_no_change() {
        let prev = MockComponent {
            diff_fn: diff::desired_state,
            to_jinja_fn: to_jinja,
            type_name: TYPE_NAME,
            value: 1,
        };
        let next = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: diff::desired_state,
            to_jinja_fn: to_jinja,
            value: 1,
        };
        let diff = ComponentConfig::diff_from(&next, Some(&prev));
        assert!(diff.is_none());
    }

    #[test]
    fn simple_diff_with_change() {
        let prev = MockComponent {
            diff_fn: diff::desired_state,
            to_jinja_fn: to_jinja,
            type_name: TYPE_NAME,
            value: 1,
        };
        let next = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: diff::desired_state,
            to_jinja_fn: to_jinja,
            value: 10,
        };
        let diff = ComponentConfig::diff_from(&next, Some(&prev)).unwrap();
        assert_dyn_eq(next, diff);
    }

    #[test]
    fn custom_diff_with_change() {
        let prev = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: custom_diff,
            to_jinja_fn: to_jinja,
            value: 1,
        };
        let next = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: custom_diff,
            to_jinja_fn: to_jinja,
            value: 10,
        };
        let diff = ComponentConfig::diff_from(&next, Some(&prev)).unwrap();

        let expected = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: custom_diff,
            to_jinja_fn: to_jinja,
            // 10 - 1, per our custom diff
            value: 9,
        };

        assert_dyn_eq(expected, diff);
    }

    #[test]
    fn relation_config_diff_created() {
        let next_component = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: diff::desired_state,
            to_jinja_fn: to_jinja,
            value: 10,
        };
        let prev = RelationConfig::new(AdapterType::Bigquery, [], return_true);
        let next = RelationConfig::new(
            AdapterType::Bigquery,
            [Box::new(next_component.clone()) as Box<dyn ComponentConfig>],
            return_true,
        );
        let changeset = RelationConfig::diff(&next, &prev);
        assert!(changeset.requires_full_refresh());
        assert_eq!(changeset.changes.len(), 1);
        let change = changeset.get(TYPE_NAME);
        assert_component_config_change_eq::<MockComponent>(
            change,
            &ComponentConfigChange::Some(Box::new(next_component) as Box<dyn ComponentConfig>),
        );
    }

    #[test]
    fn relation_config_diff_no_changes() {
        let component = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: diff::desired_state,
            to_jinja_fn: to_jinja,
            value: 10,
        };
        let relation_config = RelationConfig::new(
            AdapterType::Bigquery,
            [Box::new(component) as Box<dyn ComponentConfig>],
            return_true,
        );
        let changeset = RelationConfig::diff(&relation_config, &relation_config);
        assert!(changeset.requires_full_refresh());
        assert_eq!(changeset.len(), 0);
        let change = changeset.get(TYPE_NAME);
        assert_component_config_change_eq::<MockComponent>(change, &ComponentConfigChange::None);
    }

    #[test]
    fn relation_config_diff_with_changes() {
        let prev_component = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: diff::desired_state,
            to_jinja_fn: to_jinja,
            value: 1,
        };
        let next_component = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: diff::desired_state,
            to_jinja_fn: to_jinja,
            value: 10,
        };
        let prev = RelationConfig::new(
            AdapterType::Bigquery,
            [Box::new(prev_component) as Box<dyn ComponentConfig>],
            return_true,
        );
        let next = RelationConfig::new(
            AdapterType::Bigquery,
            [Box::new(next_component.clone()) as Box<dyn ComponentConfig>],
            return_true,
        );
        let changeset = RelationConfig::diff(&next, &prev);
        assert!(changeset.requires_full_refresh());
        assert_eq!(changeset.len(), 1);
        let change = changeset.get(TYPE_NAME);
        assert_component_config_change_eq::<MockComponent>(
            change,
            &ComponentConfigChange::Some(Box::new(next_component) as Box<dyn ComponentConfig>),
        );
    }

    #[test]
    fn relation_config_diff_drop() {
        let prev_component = MockComponent {
            type_name: TYPE_NAME,
            diff_fn: diff::desired_state,
            to_jinja_fn: to_jinja,
            value: 1,
        };
        let prev = RelationConfig::new(
            AdapterType::Bigquery,
            [Box::new(prev_component) as Box<dyn ComponentConfig>],
            return_true,
        );
        let next = RelationConfig::new(AdapterType::Bigquery, [], return_true);
        let changeset = RelationConfig::diff(&next, &prev);
        assert!(changeset.requires_full_refresh());
        assert_eq!(changeset.len(), 1);
        let change = changeset.get(TYPE_NAME);
        assert_component_config_change_eq::<MockComponent>(change, &ComponentConfigChange::Drop);
    }

    #[test]
    fn diff_changed_keys_no_changes() {
        let hashmap = IndexMap::from([("a", 1), ("b", 2)]);
        let diff = diff::changed_keys(&hashmap, &hashmap);
        assert!(diff.is_none());
    }

    #[test]
    fn diff_changed_keys_with_changes() {
        let prev = IndexMap::from([("a", 1), ("b", 2)]);
        let next = IndexMap::from([("a", 1), ("b", 3)]);
        let diff = diff::changed_keys(&next, &prev);
        let expected = Some(IndexMap::from([("b", 3)]));
        assert_eq!(diff, expected);
    }

    #[test]
    fn diff_changed_keys_dropped_key() {
        let prev = IndexMap::from([("a", 1), ("b", 2)]);
        let next = IndexMap::from([("a", 1)]);
        let diff = diff::changed_keys(&next, &prev);
        // Dropping key resets the value to the default
        let expected = Some(IndexMap::from([("b", 0)]));
        assert_eq!(diff, expected);
    }

    /// Tests related to the jinja Object implementation
    mod jinja {
        use super::*;
        use minijinja::value::{Enumerator, Object, Value};
        use minijinja_contrib::testing::jinja_assert;
        use std::sync::Arc;

        #[derive(Debug)]
        struct RelationConfigTestWrapper {
            desired: Arc<RelationConfig>,
            existing: Arc<RelationConfig>,
        }

        impl RelationConfigTestWrapper {
            fn new(desired: RelationConfig, existing: RelationConfig) -> Self {
                RelationConfigTestWrapper {
                    desired: Arc::new(desired),
                    existing: Arc::new(existing),
                }
            }
        }

        impl Object for RelationConfigTestWrapper {
            fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
                match key.as_str()? {
                    "desired" => Some(Value::from_dyn_object(self.desired.clone())),
                    "existing" => Some(Value::from_dyn_object(self.existing.clone())),
                    _ => None,
                }
            }

            fn enumerate(self: &Arc<Self>) -> Enumerator {
                Enumerator::Values(vec![Value::from("desired"), Value::from("existing")])
            }
        }

        mod bigquery {
            use super::*;
            use dbt_schemas::schemas::manifest::{
                BigqueryPartitionConfig, BigqueryPartitionConfigInner, TimeConfig,
            };

            use std::collections::HashMap;

            use crate::relation::bigquery::config::relation_types::materialized_view;
            use crate::relation::bigquery::config::test_helpers::{
                TestTableConfig, make_driver_data,
            };

            fn make_mat_view(cfg: TestTableConfig) -> RelationConfig {
                materialized_view::new_loader()
                    .from_remote_state(&make_driver_data(cfg))
                    .unwrap()
            }

            #[test]
            fn mat_view_config() {
                let cfg = make_mat_view(TestTableConfig {
                    partition_by: Some(BigqueryPartitionConfig {
                        field: "my_field".to_string(),
                        data_type: "DATETIME".to_string(),
                        __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                            granularity: "DAY".to_string(),
                            time_ingestion_partitioning: false,
                        }),
                        copy_partitions: false,
                    }),
                    kms_key: "kms_key",
                    description: "description\nother_line",
                    cluster_by: &["a", "b"],
                    tags: HashMap::from([("tag_key", "tag_value")]),
                    labels: HashMap::from([("label_key", "label_value")]),
                    enable_refresh: Some(true),
                    expiration_ns: 1111,
                    max_staleness: "1 day",
                    refresh_interval_minutes: 1234.0,
                });

                let template = "
                obj.partition
                {{ obj.partition|tojson(indent=2) }}
                ---
                obj.cluster
                {{ obj.cluster|tojson(indent=2) }}
                ---
                obj.options.as_ddl_dict()
                {{ obj.options.as_ddl_dict()|tojson(indent=2) }}
                ";
                let expect = r#"
                obj.partition
                {
                    "copy_partitions": false,
                    "data_type": "DATETIME",
                    "field": "my_field",
                    "granularity": "DAY",
                    "range": null,
                    "time_ingestion_partitioning": false
                }
                ---
                obj.cluster
                {
                    "fields": [
                        "a",
                        "b"
                    ]
                }
                ---
                obj.options.as_ddl_dict()
                {
                    "description": "\"\"\"description\\nother_line\"\"\"",
                    "enable_refresh": true,
                    "expiration_timestamp": "TIMESTAMP \u00271970-01-01T00:00:00.000001Z\u0027",
                    "kms_key_name": "\u0027kms_key\u0027",
                    "labels": [
                        [
                            "label_key", 
                            "label_value"
                        ]
                    ],
                    "max_staleness": "1 day",
                    "refresh_interval_minutes": 1234.0,
                    "tags": [
                        [
                            "tag_key", 
                            "tag_value"
                        ]
                    ]
                }
                "#;
                jinja_assert(cfg, template, expect);
            }

            #[test]
            fn mat_view_changeset() {
                let current_state = make_mat_view(TestTableConfig {
                    partition_by: Some(BigqueryPartitionConfig {
                        field: "my_field".to_string(),
                        data_type: "DATETIME".to_string(),
                        __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                            granularity: "DAY".to_string(),
                            time_ingestion_partitioning: false,
                        }),
                        copy_partitions: false,
                    }),
                    kms_key: "kms_key",
                    description: "description\nother_line",
                    cluster_by: &["a", "b"],
                    tags: HashMap::from([("tag_key", "tag_value")]),
                    labels: HashMap::from([("label_key", "label_value")]),
                    enable_refresh: Some(true),
                    expiration_ns: 1111,
                    max_staleness: "1 day",
                    refresh_interval_minutes: 1234.0,
                });

                let desired_state = make_mat_view(TestTableConfig {
                    partition_by: Some(BigqueryPartitionConfig {
                        field: "my_new_field".to_string(),
                        data_type: "DATETIME".to_string(),
                        __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                            granularity: "DAY".to_string(),
                            time_ingestion_partitioning: false,
                        }),
                        copy_partitions: false,
                    }),
                    kms_key: "new_kms_key",
                    description: "new description\nother_line",
                    cluster_by: &["a", "b", "c"],
                    tags: HashMap::from([("new_tag_key", "new_tag_value")]),
                    labels: HashMap::from([("new_label_key", "new_label_value")]),
                    enable_refresh: Some(false),
                    expiration_ns: 2222,
                    max_staleness: "2 day",
                    refresh_interval_minutes: 4321.0,
                });

                let changeset = RelationConfig::diff(&desired_state, &current_state);

                let template = "
                obj.requires_full_refresh
                {{ obj.requires_full_refresh|tojson(indent=2) }}
                ---
                obj.options.context.as_ddl_dict()
                {{ obj.options.context.as_ddl_dict()|tojson(indent=2) }}
                ";
                let expect = r#"
                obj.requires_full_refresh
                true
                ---
                obj.options.context.as_ddl_dict()
                {
                    "description": "\"\"\"new description\\nother_line\"\"\"",
                    "enable_refresh": false,
                    "expiration_timestamp": "TIMESTAMP \u00271970-01-01T00:00:00.000002Z\u0027",
                    "kms_key_name": "\u0027new_kms_key\u0027",
                    "labels": [
                        [
                            "new_label_key", 
                            "new_label_value"
                        ]
                    ],
                    "max_staleness": "2 day",
                    "refresh_interval_minutes": 4321.0,
                    "tags": [
                        [
                            "new_tag_key", 
                            "new_tag_value"
                        ]
                    ]
                }
                "#;
                jinja_assert(changeset, template, expect);
            }
        }

        mod databricks {
            use super::*;

            fn to_jinja_plus_one(v: &u8) -> Value {
                Value::from(*v + 1)
            }

            #[test]
            fn relation_config_empty() {
                let cfg = RelationConfig::new(AdapterType::Databricks, [], return_true);
                let template = "
                {% for key in obj %}
                    {{ key }}
                {% endfor %}
                ";
                let expect = "";
                jinja_assert(cfg, template, expect);
            }

            #[test]
            fn relation_config_iter_keys_and_get_values() {
                let cfg = RelationConfig::new(
                    AdapterType::Databricks,
                    [
                        Box::new(MockComponent {
                            type_name: "mock1",
                            diff_fn: diff::desired_state,
                            to_jinja_fn: to_jinja,
                            value: 111,
                        }) as Box<dyn ComponentConfig>,
                        Box::new(MockComponent {
                            type_name: "mock2",
                            diff_fn: diff::desired_state,
                            to_jinja_fn: to_jinja_plus_one,
                            value: 222,
                        }) as Box<dyn ComponentConfig>,
                    ],
                    return_true,
                );
                let template = "
                {% for key in obj %}
                    key       : {{ key }}
                    value[key]: {{ obj[key] }}
                {% endfor %}
                ";
                let expect = "
                key       : mock1
                value[key]: 111
                key       : mock2
                value[key]: 223
                ";
                jinja_assert(cfg, template, expect);
            }

            #[test]
            fn relation_config_get_changeset_with_changes() {
                let existing = RelationConfig::new(
                    AdapterType::Databricks,
                    [Box::new(MockComponent {
                        type_name: "mock1",
                        diff_fn: diff::desired_state,
                        to_jinja_fn: to_jinja,
                        value: 100,
                    }) as Box<dyn ComponentConfig>],
                    return_true,
                );
                let desired = RelationConfig::new(
                    AdapterType::Databricks,
                    [Box::new(MockComponent {
                        type_name: "mock1",
                        diff_fn: diff::desired_state,
                        to_jinja_fn: to_jinja,
                        value: 200,
                    }) as Box<dyn ComponentConfig>],
                    return_true,
                );

                let wrapper = RelationConfigTestWrapper::new(desired, existing);
                let template = "
                {% set changeset = obj.desired.get_changeset(obj.existing) %}
                has_changeset: {{ changeset is not none }}
                requires_full_refresh: {{ changeset.requires_full_refresh }}
                desired_state: {{ changeset.changes['mock1'] }}
                ";
                let expect = "
                has_changeset: True
                requires_full_refresh: True
                desired_state: 200
                ";
                jinja_assert(wrapper, template, expect);
            }

            #[test]
            fn relation_config_get_changeset_no_changes() {
                let existing = RelationConfig::new(
                    AdapterType::Databricks,
                    [Box::new(MockComponent {
                        type_name: "mock1",
                        diff_fn: diff::desired_state,
                        to_jinja_fn: to_jinja,
                        value: 100,
                    }) as Box<dyn ComponentConfig>],
                    return_true,
                );
                let desired = RelationConfig::new(
                    AdapterType::Databricks,
                    [Box::new(MockComponent {
                        type_name: "mock1",
                        diff_fn: diff::desired_state,
                        to_jinja_fn: to_jinja,
                        value: 100,
                    }) as Box<dyn ComponentConfig>],
                    return_true,
                );

                let wrapper = RelationConfigTestWrapper::new(desired, existing);
                let template = "
                {% set changeset = obj.desired.get_changeset(obj.existing) %}
                has_changeset: {{ changeset is not none }}
                ";
                let expect = "
                has_changeset: False
                ";
                jinja_assert(wrapper, template, expect);
            }

            #[test]
            fn changeset_empty() {
                let cfg =
                    RelationComponentConfigChangeSet::new(AdapterType::Databricks, [], return_true);
                let template = "
                {% for key in obj %}
                    {{ key }}
                {% endfor %}
                ";
                let expect = "";
                jinja_assert(cfg, template, expect);
            }

            #[test]
            fn changeset_iter_keys_and_get_values() {
                let cfg = RelationComponentConfigChangeSet::new(
                    AdapterType::Databricks,
                    [
                        (
                            "mock1",
                            ComponentConfigChange::Some(Box::new(MockComponent {
                                type_name: TYPE_NAME,
                                diff_fn: diff::desired_state,
                                to_jinja_fn: to_jinja,
                                value: 111,
                            })
                                as Box<dyn ComponentConfig>),
                        ),
                        (
                            "mock2",
                            ComponentConfigChange::Some(Box::new(MockComponent {
                                type_name: TYPE_NAME,
                                diff_fn: diff::desired_state,
                                to_jinja_fn: to_jinja_plus_one,
                                value: 222,
                            })
                                as Box<dyn ComponentConfig>),
                        ),
                    ],
                    return_true,
                );
                let template = "
                {% for key in obj %}
                    key           : {{ key }}
                    value.get(key): {{ obj.get(key) }}
                    value[key]    : {{ obj[key] }}
                {% endfor %}
                ";
                let expect = "
                key           : mock1
                value.get(key): 111
                value[key]    : 111
                key           : mock2
                value.get(key): 223
                value[key]    : 223
                ";
                jinja_assert(cfg, template, expect);
            }
        }
    }
}
