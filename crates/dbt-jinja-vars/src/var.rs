use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use minijinja::arg_utils::ArgsIter;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Object, ValueKind};
use minijinja::{Error, ErrorKind, State, Value};
use serde::Serialize;

use crate::utils::yml_value_to_minijinja;

/// Shared behavior for dbt var-like functions.
pub trait VarFunction: Object {
    /// Type-specific presence lookup.
    fn contains_var(&self, state: &State<'_, '_>, var_name: &str) -> Result<bool, Error>;

    /// Type-specific call as function.
    fn call_as_function(
        &self,
        state: &State<'_, '_>,
        var_name: String,
        default_value: Option<Value>,
    ) -> Result<Value, Error>;

    /// Parse var() arguments into (name, default).
    fn parse_args(args: &[Value]) -> Result<(String, Option<Value>), Error> {
        // NOTE: Minijinja encodes keyword arguments into `args` as a final map value.
        // Using `ArgsIter` ensures we correctly support both:
        // - var("name", "default")
        // - var("name", default="default")
        // and we don't accidentally treat the kwargs map itself as the default value.
        let iter = ArgsIter::new("var", &["name", "default"], args);
        // Jinja will happily pass "undefined" into functions if the caller writes `var(x)`
        // and `x` is not defined. Jinja2's Undefined is string-coercible, so dbt-core will
        // end up treating it like an empty string key and still honor the provided default.
        //
        // For compatibility (and to keep tests/fixtures stable), we explicitly coerce
        // undefined/none to an empty string here instead of raising a type error.
        let var_name_value = iter.next_arg::<Value>()?;
        let var_name = if let Some(s) = var_name_value.as_str() {
            s.to_string()
        } else if var_name_value.is_undefined() || var_name_value.is_none() {
            "".to_string()
        } else {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "argument 'name' to var() has incompatible type; value is not a string",
            ));
        };

        // IMPORTANT: ArgsIter's `next_kwarg::<Option<&Value>>` cannot distinguish between:
        // - kwarg missing
        // - kwarg present with value `none`
        //
        // dbt projects commonly do `var("x", default=None)`, so we need to preserve an
        // explicit `none` default.
        //
        // We therefore parse the kwargs map ourselves (if present), and fall back to
        // a true positional default only when the 2nd argument is not that kwargs map.
        let mut default_value: Option<Value> = None;

        // there are two ways for the second argument to be a map:
        // 1. var("x", {"a": 1})
        // 2. var("x", {"default": 1})
        // The first use provides a default map-typed value for x
        // The second use provides a default int-typed value for x

        // Handling the default value
        if args.len() == 2 {
            let second = args.get(1).expect("second must be present");
            let default_key = Value::from("default");
            let keyword_default_value = if second.kind() == ValueKind::Map {
                // NOTE: `get_item` can succeed and still return `undefined` when the key
                // does not exist. Treat that as "not present".
                match second.get_item(&default_key) {
                    Ok(v) if !v.is_undefined() => Some(v),
                    _ => None,
                }
            } else {
                None
            };

            if let Some(v) = keyword_default_value {
                // Keyword default: var("x", default=...)
                default_value = Some(v);
            } else {
                // Positional default: var("x", "abc")
                default_value = Some(second.clone());
            }
        }

        Ok((var_name, default_value))
    }

    /// Consistent missing-var error shape.
    fn missing_var_error<M>(package_name: &str, var_name: &str, vars_lookup: &M) -> Error
    where
        M: Serialize + ?Sized,
    {
        Error::new(
            ErrorKind::InvalidOperation,
            format!(
                "Required var '{}' not found in config:\nVars supplied to {} = {}",
                var_name,
                package_name,
                serde_json::to_string_pretty(vars_lookup).unwrap()
            ),
        )
    }

    /// Common handler for `has_var`.
    fn call_has_var(&self, state: &State<'_, '_>, args: &[Value]) -> Result<Value, Error> {
        if args.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "has_var requires 1 argument",
            ));
        }
        let var_name = args[0].as_str().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidOperation,
                "argument 'name' to has_var() has incompatible type; value is not a string",
            )
        })?;
        let present = self.contains_var(state, var_name)?;
        Ok(Value::from(present))
    }

    /// Common implementation for `Object::call`.
    fn call_impl(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        let (var_name, default_value) = Self::parse_args(args)?;
        self.call_as_function(state, var_name, default_value)
    }

    /// Common implementation for `Object::call_method`.
    fn call_method_impl(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        if method == "has_var" {
            self.call_has_var(state, args)
        } else {
            Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("Method {method} not found"),
            ))
        }
    }
}

/// A struct that returns a variable from a map of variables (with optional overrides)
/// https://github.com/dbt-labs/dbt-core/blob/31881d2a3bea030e700e9df126a3445298385698/core/dbt/context/base.py#L139
#[derive(Debug)]
pub struct Var {
    vars: BTreeMap<String, dbt_yaml::Value>,
    overrides: Option<BTreeMap<String, dbt_yaml::Value>>,
}

impl Var {
    /// Make a new Var struct
    pub fn new(vars: BTreeMap<String, dbt_yaml::Value>) -> Self {
        Self {
            vars,
            overrides: None,
        }
    }

    /// Make a new Var struct with override values
    pub fn with_overrides(
        vars: BTreeMap<String, dbt_yaml::Value>,
        overrides: Option<BTreeMap<String, dbt_yaml::Value>>,
    ) -> Self {
        Self { vars, overrides }
    }
}

impl std::fmt::Display for Var {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "var(...)")
    }
}

impl VarFunction for Var {
    fn contains_var(&self, _state: &State<'_, '_>, var_name: &str) -> Result<bool, Error> {
        // Preserve existing behavior: check only self.vars
        Ok(self.vars.contains_key(var_name))
    }

    fn call_as_function(
        &self,
        _state: &State<'_, '_>,
        var_name: String,
        default_value: Option<Value>,
    ) -> Result<Value, Error> {
        if let Some(overrides) = &self.overrides
            && let Some(value) = overrides.get(&var_name)
        {
            Ok(yml_value_to_minijinja(value))
        } else if let Some(value) = self.vars.get(&var_name) {
            Ok(yml_value_to_minijinja(value))
        } else if let Some(default) = default_value {
            Ok(default)
        } else {
            Err(Self::missing_var_error(
                "<Configuration>",
                &var_name,
                &self.vars,
            ))
        }
    }
}

impl Object for Var {
    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        self.call_impl(state, args, listeners)
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        self.call_method_impl(state, method, args, listeners)
    }
}
