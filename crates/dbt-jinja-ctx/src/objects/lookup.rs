//! Macro / namespace lookup `Object` impls — std-only types moved here from
//! `dbt-jinja-utils` so the typed parse/compile/run-phase ctx structs can
//! hold them via `JinjaObject<T>` rather than opaque `MinijinjaValue`.
//!
//! Naming note: this module's [`DbtNamespace`] is the *dispatch-side*
//! namespace — it intercepts `{{ dbt.macro_name }}` lookups and produces
//! `DispatchObject`s for macro execution. There is a separate
//! `dbt_parser::dbt_namespace::DbtNamespace` (in the `dbt-parser` crate)
//! which is parse-phase adapter-call tracking (intercepts
//! `get_columns_in_relation` / `get_relation` to record introspection
//! calls). The two never collide at the type level (different crates,
//! different fully-qualified paths), but the name is reused. Renaming
//! either is a cross-cutting cleanup; leaving for now and flagging here.

use std::collections::BTreeSet;
use std::fmt::Debug;
use std::rc::Rc;
use std::sync::Arc;

use minijinja::arg_utils::{ArgParser, ArgsIter};
use minijinja::dispatch_object::{DispatchObject, macro_namespace_template_resolver};
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Object, ValueKind, ValueMap, mutable_map::MutableMap};
use minijinja::{Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, State, Value};

/// A namespace object for `{{ dbt.macro_name }}` (or
/// `{{ snowflake.macro_name }}` etc.) lookups during render. On
/// `get_property("name")` it consults the env's macro namespace registry
/// and produces a `DispatchObject` that, when called, executes the matching
/// macro.
#[derive(Debug, Clone)]
pub struct DbtNamespace {
    /// The namespace key (e.g. `"dbt"`, `"snowflake"`).
    pub name: String,
}

impl DbtNamespace {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

/// Does package `macro_names` define a macro called `name`?
///
/// The macro namespace registry stores each package's macro names as a map keyed by
/// name, so the common path is a single hash lookup. It used to store a sequence, which
/// made this a linear scan over every macro in the package — for a package the size of
/// elementary (~1000 macros) that dominated the cost of every `pkg.macro_name` access.
///
/// The sequence branch is kept because the registry is a plain global that callers
/// outside `environment_builder` can populate themselves; an unexpected shape should
/// degrade in speed, not correctness.
fn namespace_defines_macro(macro_names: &Value, name: &str) -> bool {
    if macro_names.kind() == ValueKind::Map {
        return macro_names
            .get_item(&Value::from(name))
            .is_ok_and(|found| !found.is_undefined());
    }
    macro_names
        .try_iter()
        .map(|mut iter| iter.any(|v| v.as_str() == Some(name)))
        .unwrap_or(false)
}

impl Object for DbtNamespace {
    fn get_property(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        name: &str,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, MinijinjaError> {
        let ns_name = Value::from(self.name.clone());
        let namespace_registry = state
            .env()
            .get_macro_namespace_registry()
            .unwrap_or_default();
        let template_registry = state.env().get_macro_template_registry();
        // Could be a package name; check whether there's a macro in the namespace.
        if namespace_registry
            .get(&ns_name)
            .is_some_and(|val| namespace_defines_macro(val, name))
        {
            let template_registry_entry = template_registry.get(&ns_name);
            let path = template_registry_entry
                .and_then(|entry| entry.get_attr("path").ok())
                .unwrap_or(ns_name);
            let span = template_registry_entry
                .and_then(|entry| entry.get_attr("span").ok())
                .unwrap_or_else(|| Value::from_serialize(minijinja::machinery::Span::default()));

            let template_name = format!("{}.{}", self.name, name);
            listeners
                .iter()
                .for_each(|l| l.on_macro_dependency(&template_name));
            let context = state.get_base_context_with_path_and_span(&path, &span);
            Ok(Value::from_object(DispatchObject {
                macro_name: (*name).to_string(),
                package_name: Some(self.name.clone()),
                strict: true,
                auto_execute: false,
                context: Some(context),
            }))
        } else if self.name == "dbt" {
            // `dbt.<name>` is the unprefixed internal-macro path, so this is
            // exactly where two adapters defining the same macro would collide.
            // Resolve within the node's dialect.
            let dialect = minijinja::dispatch_object::resolve_dialect(state);
            let dbt_and_adapters = state.env().get_dbt_and_adapters_namespace(&dialect);
            if let Some(package) = dbt_and_adapters.get(&Value::from(name)) {
                let package_name = package.as_str().map(|s| s.to_string());
                let template_registry_entry = template_registry.get(&ns_name);
                let path = template_registry_entry
                    .and_then(|entry| entry.get_attr("path").ok())
                    .unwrap_or(ns_name);
                let span = template_registry_entry
                    .and_then(|entry| entry.get_attr("span").ok())
                    .unwrap_or_else(
                        || Value::from_serialize(minijinja::machinery::Span::default()),
                    );

                let template_name = format!("{}.{name}", package_name.as_deref().unwrap_or("dbt"));
                listeners
                    .iter()
                    .for_each(|l| l.on_macro_dependency(&template_name));
                let context = state.get_base_context_with_path_and_span(&path, &span);
                Ok(Value::from_object(DispatchObject {
                    macro_name: (*name).to_string(),
                    package_name,
                    strict: true,
                    auto_execute: false,
                    context: Some(context),
                }))
            } else {
                Ok(Value::UNDEFINED)
            }
        } else {
            Ok(Value::UNDEFINED)
        }
    }
}

/// Special context object available during the compile or run phase.
/// Allows users to look up macros by string and returns a `DispatchObject`,
/// which when called executes the macro. Users can also look up macro
/// namespaces by string, returning a nested `MacroLookupContext` that
/// itself, when called with a macro name, returns a `DispatchObject`.
#[derive(Debug, Clone)]
pub struct MacroLookupContext {
    /// The root project name.
    pub root_project_name: String,
    /// The current project name; when `None`, search begins from the root
    /// project.
    pub current_project_name: Option<String>,
    /// The packages available in the project.
    pub packages: BTreeSet<String>,
    /// Values written by user Jinja via `context.update(...)`.
    local_values: Arc<MutableMap>,
}

impl MacroLookupContext {
    /// Create a new macro lookup context.
    pub fn new(
        root_project_name: String,
        current_project_name: Option<String>,
        packages: BTreeSet<String>,
    ) -> Self {
        Self {
            root_project_name,
            current_project_name,
            packages,
            local_values: Arc::new(MutableMap::new()),
        }
    }

    fn with_current_project(&self, current_project_name: Option<String>) -> Self {
        Self {
            root_project_name: self.root_project_name.clone(),
            current_project_name,
            packages: BTreeSet::new(),
            local_values: self.local_values.clone(),
        }
    }

    fn get_context_value(&self, key: &Value) -> Option<Value> {
        self.local_values.get(key).or_else(|| match key.as_str()? {
            // NOTE(serramatutu): In Core, the following non-macro keys are
            // all members of `MacroLookupContext`. They can technically be
            // used (the usage is undocumented and not encouraged):
            // dbt_version, project_name, schema, run_started_at.
            //
            // We added `project_name` because some naughty famous macro
            // uses it and was breaking lots of projects, but I prefer to
            // avoid polluting this scope and sticking as faithfully as
            // possible to the "intended" behavior (only looking up macros).
            "project_name" => Some(Value::from(self.root_project_name.clone())),
            _ => None,
        })
    }

    fn get_package_context(&self, key: &Value) -> Option<Value> {
        let package_name = key.as_str()?;
        self.packages
            .contains(package_name)
            .then(|| Value::from_object(self.with_current_project(Some(package_name.to_string()))))
    }

    fn get_macro_from_state(
        &self,
        state: &State<'_, '_>,
        key: &Value,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Option<Value> {
        if self.current_project_name.is_some() {
            return None;
        }
        let macro_name = key.as_str()?;
        let template_name = macro_namespace_template_resolver(state, macro_name, &mut Vec::new())?;
        self.dispatch_object_for_template(&template_name, state, listeners)
    }

    fn get_macro_from_current_project(
        &self,
        state: &State<'_, '_>,
        key: &Value,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Option<Value> {
        let package_name = self.current_project_name.as_ref()?;
        let macro_name = key.as_str()?;
        let template_name = format!("{package_name}.{macro_name}");
        state.env().get_template(&template_name).ok()?;
        self.dispatch_object_for_template(&template_name, state, listeners)
    }

    fn dispatch_object_for_template(
        &self,
        template_name: &str,
        state: &State<'_, '_>,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Option<Value> {
        listeners
            .iter()
            .for_each(|listener| listener.on_macro_dependency(template_name));
        let (package_name, macro_name) = template_name.split_once('.')?;

        Some(Value::from_object(DispatchObject {
            macro_name: macro_name.to_string(),
            package_name: Some(package_name.to_string()),
            strict: true,
            auto_execute: false,
            context: Some(state.get_base_context()),
        }))
    }

    fn update_local_values(&self, args: &[Value]) -> Result<Value, MinijinjaError> {
        let parser = ArgParser::new(args, None);
        let num_pos_args = parser.positional_len();
        if num_pos_args > 1 {
            return Err(MinijinjaError::new(
                MinijinjaErrorKind::TooManyArguments,
                format!(
                    "update() takes at most one positional argument, but {} were given",
                    num_pos_args
                ),
            ));
        }

        let mut entries = ValueMap::new();
        let pos_args = parser.get_args_as_vec_of_values();
        if let Some(other) = pos_args.first() {
            entries.extend(dict_update_entries(other)?);
        }

        for (key, value) in parser.get_kwargs_as_value_map() {
            entries.insert(key, value);
        }

        self.local_values.update(&entries);
        Ok(Value::NONE)
    }
}

fn dict_update_entries(other: &Value) -> Result<ValueMap, MinijinjaError> {
    if other.kind() == ValueKind::Map
        && let Some(entries) = other.as_object().and_then(|obj| obj.try_iter_pairs())
    {
        return Ok(entries.collect());
    }

    let mut entries = ValueMap::new();
    let iter = other.try_iter().map_err(|_| {
        MinijinjaError::new(
            MinijinjaErrorKind::CannotUnpack,
            "update() expects a mapping or iterable of pairs",
        )
    })?;
    for entry in iter {
        let mut pair = entry.try_iter().map_err(|_| {
            MinijinjaError::new(
                MinijinjaErrorKind::CannotUnpack,
                "update() sequence element is not iterable",
            )
        })?;
        let key = pair.next().ok_or_else(|| {
            MinijinjaError::new(
                MinijinjaErrorKind::CannotUnpack,
                "update() sequence element has length 0; 2 is required",
            )
        })?;
        let value = pair.next().ok_or_else(|| {
            MinijinjaError::new(
                MinijinjaErrorKind::CannotUnpack,
                "update() sequence element has length 1; 2 is required",
            )
        })?;
        if pair.next().is_some() {
            return Err(MinijinjaError::new(
                MinijinjaErrorKind::CannotUnpack,
                "update() sequence element has length greater than 2; 2 is required",
            ));
        }
        entries.insert(key, value);
    }
    Ok(entries)
}

impl Object for MacroLookupContext {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        self.get_context_value(key).or_else(|| {
            let lookup_macro = key.as_str()?;
            if self.packages.contains(lookup_macro) {
                Some(Value::from_object(
                    self.with_current_project(Some(lookup_macro.to_string())),
                ))
            } else {
                Some(Value::from_object(DispatchObject {
                    macro_name: lookup_macro.to_string(),
                    package_name: self.current_project_name.clone(),
                    strict: self.current_project_name.is_some(),
                    auto_execute: false,
                    // TODO: If the macro uses a recursive context (i.e.
                    // context['self']) we will stack overflow but there
                    // is no way to conjure up a context object here
                    // without access to State.
                    context: None,
                }))
            }
        })
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        Self: Sized + 'static,
    {
        self.fmt(f)
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, MinijinjaError> {
        // TODO(serramatutu): should this behave fully like a dict, with
        // values, keys, items, enumerate etc?
        match method {
            "get" => {
                let iter = ArgsIter::new("MacroLookupContext.get", &["key"], args);
                let key = iter.next_arg::<&Value>()?;
                let default = iter.next_kwarg::<Option<&Value>>("default")?;
                iter.finish()?;

                Ok(self
                    .get_context_value(key)
                    .or_else(|| self.get_package_context(key))
                    .or_else(|| self.get_macro_from_current_project(state, key, listeners))
                    .or_else(|| self.get_macro_from_state(state, key, listeners))
                    .or_else(|| default.cloned())
                    .unwrap_or(Value::from(None::<Value>)))
            }
            "update" => self.update_local_values(args),
            _ => {
                let key = Value::from(method);
                if let Some(value) = self
                    .get_context_value(&key)
                    .or_else(|| self.get_package_context(&key))
                    .or_else(|| self.get_macro_from_current_project(state, &key, listeners))
                    .or_else(|| self.get_macro_from_state(state, &key, listeners))
                {
                    return value.call(state, args, listeners);
                }
                Err(MinijinjaError::new(
                    MinijinjaErrorKind::UnknownMethod,
                    format!("MacroLookupContext has no method named {method}"),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::value::mutable_vec::MutableVec;

    fn map_of(names: &[&str]) -> Value {
        let entries: ValueMap = names
            .iter()
            .map(|n| (Value::from(*n), Value::from(true)))
            .collect();
        Value::from_object(entries)
    }

    fn seq_of(names: &[&str]) -> Value {
        Value::from_object(MutableVec::from(
            names.iter().map(|n| Value::from(*n)).collect::<Vec<_>>(),
        ))
    }

    #[test]
    fn map_shaped_namespace_resolves_membership() {
        let names = map_of(&["flatten_column", "union_lists", "safe_get_with_default"]);
        assert!(namespace_defines_macro(&names, "flatten_column"));
        assert!(namespace_defines_macro(&names, "safe_get_with_default"));
        assert!(!namespace_defines_macro(&names, "not_a_macro"));
    }

    #[test]
    fn sequence_shaped_namespace_still_resolves_membership() {
        // The registry is a plain global; callers outside environment_builder may still
        // populate it with a sequence. That must keep working, just without the O(1) path.
        let names = seq_of(&["flatten_column", "union_lists"]);
        assert!(namespace_defines_macro(&names, "flatten_column"));
        assert!(namespace_defines_macro(&names, "union_lists"));
        assert!(!namespace_defines_macro(&names, "not_a_macro"));
    }

    #[test]
    fn both_shapes_agree() {
        let present = ["a", "b", "c"];
        for probe in ["a", "b", "c", "d", ""] {
            assert_eq!(
                namespace_defines_macro(&map_of(&present), probe),
                namespace_defines_macro(&seq_of(&present), probe),
                "shapes disagreed for {probe:?}"
            );
        }
    }

    #[test]
    fn empty_and_unexpected_shapes_report_no_macro() {
        assert!(!namespace_defines_macro(&map_of(&[]), "anything"));
        assert!(!namespace_defines_macro(&seq_of(&[]), "anything"));
        // A scalar is neither a map nor iterable; must not panic.
        assert!(!namespace_defines_macro(&Value::from(42), "anything"));
        assert!(!namespace_defines_macro(&Value::UNDEFINED, "anything"));
    }
}
