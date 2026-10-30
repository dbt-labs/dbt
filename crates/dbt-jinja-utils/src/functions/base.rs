//! Core functions that are shared across all contexts

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    rc::Rc,
    sync::Arc,
};

use indexmap::IndexMap;

use dbt_agate::AgateTable;
use dbt_common::{
    CodeLocationWithFile, ErrorCode, FsError, fs_err,
    tracing::dbt_emit::{emit_warn_log_from_fs_error, emit_warn_log_message},
    tracing::emit::{emit_debug_event, emit_info_event},
    warn_error_options::{WarnErrorDecision, WarnErrorOptions},
};
use dbt_schemas::schemas::Nodes;
use dbt_schemas::schemas::manifest::{
    defer_relation_for_model, defer_relation_for_seed, defer_relation_for_snapshot,
};
use dbt_telemetry::UserLogMessage;
use minijinja::{
    arg_utils::ArgsIter,
    constants::TARGET_PACKAGE_NAME,
    value::{ValueMap, mutable_map::MutableMap, mutable_set::MutableSet},
};

use minijinja::{
    Environment, Error, ErrorKind, State, Value,
    arg_utils::ArgParser,
    listener::RenderingEventListener,
    value::{Kwargs, Object},
};
type YmlValue = dbt_yaml::Value;
use crate::utils::{ENV_VARS, node_metadata_from_state};

use crate::functions::contract_error::get_contract_mismatches;
use crate::functions::python_json::to_json_string_python_style;

pub use dbt_jinja_vars::{LookupFn, SECRET_PLACEHOLDER, Var};

/// Registers all the functions shared across all contexts
pub fn register_base_functions(env: &mut Environment, warn_error_options: WarnErrorOptions) {
    env.add_global("dbt_version", Value::from(crate::utils::DBT_VERSION));
    env.add_global(
        "exceptions".to_owned(),
        Value::from_object(Exceptions { warn_error_options }),
    );
    // dbt-core templates commonly use Python-ish constants (capitalized).
    // In Jinja2 the canonical values are `none/true/false`, but many dbt projects
    // (and dbt-core's Python context) also make `None/True/False` available.
    // Missing these can cause `default=None` to be treated as an undefined variable,
    // which in turn can make `var(..., default=None)` behave like a required var.
    env.add_global("None", Value::from(()));
    env.add_global("True", Value::from(true));
    env.add_global("False", Value::from(false));

    env.add_function("otel_trace_id", otel_trace_id_fn());
    env.add_func_func("otel_span_id", otel_span_id);
    env.add_func_func("fromjson", fromjson);
    env.add_func_func("tojson", tojson);
    env.add_func_func("fromyaml", fromyaml);
    env.add_func_func("toyaml", toyaml);
    env.add_function("set", set_fn());
    env.add_function("render", render_fn());
    env.add_function("set_strict", set_strict_fn());
    env.add_function("zip", zip_fn());
    env.add_function("zip_strict", zip_strict_fn());
    // TODO: log
    // env.add_global("invocation_id", panic!("TODO_INVOCATION_ID"));
    // TODO: modules
    // TODO: flags
    env.add_function("print", print_fn());
    env.add_function("log", log_fn());
    env.add_function("diff_of_two_dicts", diff_of_two_dicts_fn());
    env.add_function("local_md5", local_md5_fn());
    env.add_func_func("env_var", |state, args| env_var(false, None, state, args));
    env.add_function("try_or_compiler_error", try_or_compiler_error_fn());
    // var and env_Var are slightly different depending on the context
}

/// Silences the base context by overriding the print and log functions
pub fn silence_base_context(base_ctx: &mut BTreeMap<String, Value>) {
    base_ctx.insert(
        "print".to_string(),
        Value::from_function(|_args: &[Value], _kwargs: Kwargs| Ok(())),
    );
    base_ctx.insert(
        "log".to_string(),
        Value::from_function(|_args: &[Value], _kwargs: Kwargs| Ok(())),
    );
}

/// A struct that represents a reusable doc object to be used in configuration contexts
#[derive(Debug)]
pub struct DocMacro {
    /// Package precedence for unqualified doc references.
    package_search_order: Vec<String>,
    /// Core-compatible argument and missing-target checking.
    strict: bool,
    /// The actual doc strings stored once to avoid duplication
    docs_content: Vec<String>,
    /// Maps (package_name, doc_name) to index in docs_content
    package_doc_map: HashMap<(String, String), usize>,
    /// Maps doc_name to a list of (package_name, content_index) pairs
    doc_name_map: HashMap<String, Vec<(String, usize)>>,
}

impl DocMacro {
    /// Initializes the doc macro
    pub fn new(package_name: String, docs: BTreeMap<(String, String), String>) -> Self {
        Self::new_internal(vec![package_name], docs, false)
    }

    /// Core's `DocsRuntimeContext` `doc()`: one or two positional strings, and a
    /// missing target aborts rendering.
    pub fn new_strict_with_search_order(
        package_search_order: Vec<String>,
        docs: BTreeMap<(String, String), String>,
    ) -> Self {
        Self::new_internal(package_search_order, docs, true)
    }

    fn new_internal(
        package_search_order: Vec<String>,
        docs: BTreeMap<(String, String), String>,
        strict: bool,
    ) -> Self {
        let mut docs_content = Vec::new();
        let mut package_doc_map = HashMap::new();
        let mut doc_name_map: HashMap<String, Vec<(String, usize)>> = HashMap::new();

        // Convert the BTreeMap into our optimized structure
        for ((package, doc_name), content) in docs {
            let content_idx = docs_content.len();
            docs_content.push(content);

            // Update both lookup maps
            package_doc_map.insert((package.clone(), doc_name.clone()), content_idx);
            doc_name_map
                .entry(doc_name)
                .or_default()
                .push((package, content_idx));
        }

        Self {
            package_search_order,
            strict,
            docs_content,
            package_doc_map,
            doc_name_map,
        }
    }

    /// Lookup a doc by package name and doc name
    fn lookup_doc(&self, package_name: &str, doc_name: &str) -> Option<&str> {
        self.package_doc_map
            .get(&(package_name.to_string(), doc_name.to_string()))
            .map(|&idx| self.docs_content[idx].as_str())
    }

    /// Lookup a doc by doc name
    fn lookup_doc_in_packages(&self, doc_name: &str) -> Option<&str> {
        self.doc_name_map.get(doc_name).and_then(|package_indices| {
            // Return the first doc found in the list of packages
            package_indices
                .first()
                .map(|(_, idx)| self.docs_content[*idx].as_str())
        })
    }
}

impl Object for DocMacro {
    /// Implements the call method on the var object
    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        let mut args = ArgParser::new(args, None);
        // Core's `doc(self, *args: str)`. Lenient mode keeps the historical tolerance,
        // because model/source/column descriptions still render through it.
        if self.strict && (args.kwargs_len() != 0 || !(1..=2).contains(&args.positional_len())) {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "doc() takes one or two positional string arguments",
            ));
        }
        let (arg1, arg2) = if self.strict {
            // Both args are annotated `str`, so no coercion either.
            let package_or_name = args.get::<Arc<str>>("")?.to_string();
            let name = if args.positional_len() == 0 {
                None
            } else {
                Some(args.get::<Arc<str>>("")?.to_string())
            };
            (package_or_name, name)
        } else {
            let arg1 = args.get::<String>("").map_err(|_| {
                Error::new(
                    ErrorKind::InvalidOperation,
                    "Invalid arguments to doc macro",
                )
            })?;
            (arg1, args.get_optional::<String>(""))
        };

        let (doc, target_package, doc_name) = match &arg2 {
            // Two arguments: explicit package and doc name
            Some(doc_name) => (
                self.lookup_doc(&arg1, doc_name),
                arg1.clone(),
                doc_name.clone(),
            ),
            // One argument: search the configured package precedence, then any package
            None => {
                let doc = self
                    .package_search_order
                    .iter()
                    .find_map(|package_name| self.lookup_doc(package_name, &arg1))
                    .or_else(|| self.lookup_doc_in_packages(&arg1));
                (
                    doc,
                    self.package_search_order
                        .first()
                        .cloned()
                        .unwrap_or_default(),
                    arg1,
                )
            }
        };

        match doc {
            Some(content) => Ok(Value::from_serialize(content)),
            // `DocTargetNotFoundError`
            None if self.strict => Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("Documentation depends on doc '{doc_name}' which was not found"),
            )),
            None => {
                let current_span = state.current_span_of_context();
                let current_file_path = state.current_path().clone();
                if !current_file_path.as_os_str().is_empty() {
                    let location = CodeLocationWithFile::new(
                        current_span.start_line,
                        current_span.start_col,
                        current_span.start_offset,
                        current_file_path,
                    );
                    Self::warn_missing_doc(&target_package, &doc_name, location);
                }
                Ok(Value::from(Self::missing_doc_placeholder(
                    &target_package,
                    &doc_name,
                )))
            }
        }
    }
}

impl DocMacro {
    fn warn_missing_doc(package_name: &str, doc_name: &str, location: CodeLocationWithFile) {
        let code = ErrorCode::InvalidConfig;
        let message = format!(
            "doc macro reference '{}' not found for package '{}'",
            doc_name, package_name
        );
        let warning = fs_err!(code, "{}", message).with_location(location);
        emit_warn_log_from_fs_error(warning)
    }

    fn missing_doc_placeholder(package_name: &str, doc_name: &str) -> String {
        format!("<missing doc('{}', package='{}')>", doc_name, package_name)
    }
}

/// A function that returns an environment variable from the environment,
/// delegating to `dbt_jinja_vars::env_var` with ENV_VARS tracking.
pub fn env_var(
    placeholder_on_secret_access: bool,
    overrides_fn: Option<&LookupFn>,
    state: &State,
    args: &[Value],
) -> Result<Value, Error> {
    let tracker = |name: &str, value: &str| {
        ENV_VARS
            .lock()
            .unwrap()
            .insert(name.to_string(), value.to_string());
    };
    dbt_jinja_vars::env_var(
        placeholder_on_secret_access,
        overrides_fn,
        Some(&tracker),
        state,
        args,
    )
}

/// Builds the `otel_trace_id()` Jinja function. Must be a callable, not a
/// plain global, to match how dbt Core v1 exposes it. Captures the trace id
/// once (unlike `otel_span_id()`, which re-reads live since span id does
/// change within an invocation).
pub fn otel_trace_id_fn() -> impl Fn(&[Value], Kwargs) -> Result<Value, Error> {
    let trace_id = dbt_common::tracing::span_info::read_current_span_start_info(|info| {
        format!("{:032x}", info.trace_id)
    });
    let trace_id = Value::from(trace_id.unwrap_or_else(|| "0".repeat(32)));

    move |args: &[Value], _kwargs: Kwargs| -> Result<Value, Error> {
        if !args.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "otel_trace_id() takes no arguments",
            ));
        }
        Ok(trace_id.clone())
    }
}

/// Returns the current span's OTEL id (16-char hex).
///
/// Real code always runs inside a tracing dispatcher, so a missing span is a
/// bug, not a supported state: debug builds assert, release builds fall back
/// to the OTEL-standard invalid span id (all zeros).
pub fn otel_span_id(_state: &State, args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::nullary("otel_span_id", args);
    iter.finish()?;

    let span_id = dbt_common::tracing::span_info::read_current_span_start_info(|info| {
        format!("{:016x}", info.span_id)
    });
    debug_assert!(
        span_id.is_some(),
        "otel_span_id() called with no active span"
    );
    Ok(Value::from(span_id.unwrap_or_else(|| "0".repeat(16))))
}

/// Deserialize a JSON string into a Python object primitive (e.g., a dict or list).
///
/// ```python
/// def fromjson(string: str, default: Any = None) -> Any:
///     """The `fromjson` context method can be used to deserialize a json
///     string into a Python object primitive, eg. a `dict` or `list`.
///
///     :param value: The json string to deserialize
///     :param default: A default value to return if the `string` argument
///         cannot be deserialized (optional)
///
///     Usage:
///
///         {% set my_json_str = '{"abc": 123}' %}
///         {% set my_dict = fromjson(my_json_str) %}
///         {% do log(my_dict['abc']) %}
///     """
/// ```
pub fn fromjson(_state: &State, args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("fromjson", &["string"], args);
    let string = iter.next_arg::<&str>()?;
    let default = iter.next_kwarg::<Option<Value>>("default")?;
    iter.finish()?;

    match serde_json::from_str::<serde_json::Value>(string) {
        Ok(value) => Ok(Value::from_serialize(value)),
        Err(_) => Ok(default.unwrap_or_else(|| Value::from(()))),
    }
}

/// Serialize a Python object primitive (e.g., a dict or list) to a JSON string.
///
/// NOTE: Not all dbt kwargs like separators/indent are fully implemented here.
///
/// ```python
/// def tojson(value: Any, default: Any = None, sort_keys: bool = False) -> Any:
///     """The `tojson` context method can be used to serialize a Python
///     object primitive, eg. a `dict` or `list` to a json string.
///
///     :param value: The value serialize to json
///     :param default: A default value to return if the `value` argument
///         cannot be serialized
///     :param sort_keys: If True, sort the keys.
///
///
///     Usage:
///
///         {% set my_dict = {"abc": 123} %}
///         {% set my_json_string = tojson(my_dict) %}
///         {% do log(my_json_string) %}
///     """
/// ```
pub fn tojson(_state: &State, args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("tojson", &["value"], args);
    let value = iter.next_arg::<&Value>()?;
    let default = iter.next_kwarg::<Option<&Value>>("default")?;
    let sort_keys = iter
        .next_kwarg::<Option<bool>>("sort_keys")?
        .unwrap_or(false);
    iter.finish()?;

    // Return default if value is undefined
    if value.is_undefined() {
        return match default {
            Some(default_value) => Ok(default_value.clone()),
            None => Ok(Value::from(())),
        };
    }

    // First convert to serde_json::Value for consistent serialization
    match serde_json::to_value(value) {
        Ok(mut json_value) => {
            if sort_keys && let Some(obj) = json_value.as_object_mut() {
                // Sort the keys using BTreeMap
                let sorted: serde_json::Map<String, serde_json::Value> = obj
                    .iter()
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                json_value = serde_json::Value::Object(sorted);
            }
            // Use Python-style formatting (space after colon)
            let json_str =
                to_json_string_python_style(&json_value).unwrap_or_else(|_| "{}".to_string());
            Ok(Value::from_safe_string(json_str))
        }
        Err(err) => match default {
            Some(default_value) => Ok(default_value.clone()),
            None => Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("Failed to convert value to JSON: {err}"),
            )),
        },
    }
}

/// Deserialize a YAML string into a Python object primitive.
///
/// ```python
/// def fromyaml(value: str, default: Any = None) -> Any:
///     """The fromyaml context method can be used to deserialize a yaml string
///     into a Python object primitive, eg. a `dict` or `list`.
///
///     :param value: The yaml string to deserialize
///     :param default: A default value to return if the `string` argument
///         cannot be deserialized (optional)
///
///     Usage:
///
///         {% set my_yml_str -%}
///         dogs:
///          - good
///          - bad
///         {%- endset %}
///         {% set my_dict = fromyaml(my_yml_str) %}
///         {% do log(my_dict['dogs'], info=true) %}
///         -- ["good", "bad"]
///         {% do my_dict['dogs'].pop() }
///         {% do log(my_dict['dogs'], info=true) %}
///         -- ["good"]
///     """
/// ```
pub fn fromyaml(_state: &State, args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("fromyaml", &["value"], args);
    let value = iter.next_arg::<&str>()?;
    let default = iter.next_kwarg::<Option<Value>>("default")?;
    iter.finish()?;

    match parse_fromyaml_value(value) {
        Ok(serde_value) => Ok(Value::from_serialize(serde_value)),
        Err(_) => match default {
            Some(default_value) => Ok(default_value),
            None => Ok(Value::from(())),
        },
    }
}

fn parse_fromyaml_value(value: &str) -> Result<dbt_yaml::Value, dbt_yaml::Error> {
    let mut value =
        dbt_yaml::Value::from_str(value, |_, _, _| dbt_yaml::mapping::DuplicateKey::Overwrite)?;
    value.apply_merge()?;
    Ok(value)
}

/// Serialize a Python object primitive to a YAML string.
///
/// ```python
/// def toyaml(
///     value: Any, default: Optional[str] = None, sort_keys: bool = False
/// ) -> Optional[str]:
///     """The `tojson` context method can be used to serialize a Python
///     object primitive, eg. a `dict` or `list` to a yaml string.
///
///     :param value: The value serialize to yaml
///     :param default: A default value to return if the `value` argument
///         cannot be serialized
///     :param sort_keys: If True, sort the keys.
///
///
///     Usage:
///
///         {% set my_dict = {"abc": 123} %}
///         {% set my_yaml_string = toyaml(my_dict) %}
///         {% do log(my_yaml_string) %}
///     """
/// ```
pub fn toyaml(_state: &State, args: &[Value]) -> Result<Value, Error> {
    let iter = ArgsIter::new("toyaml", &["value"], args);
    let value = iter.next_arg::<&Value>()?;
    let default = iter.next_kwarg::<Option<&Value>>("default")?;
    let sort_keys = iter
        .next_kwarg::<Option<bool>>("sort_keys")?
        .unwrap_or(false);

    // If the value is undefined or none and there's a default, return it
    if value.is_undefined() || value.is_none() {
        return match default {
            Some(def) => Ok(def.clone()),
            // Return none for undefined/none values when no default is provided
            None => Ok(Value::from(())),
        };
    }

    // Convert the Minijinja Value to a serde_json::Value
    // Should this say YAML or JSON cause this is toyaml function, not sure
    let mut json_value = match serde_json::to_value(value) {
        Ok(val) => val,
        Err(err) => {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("Failed to convert value to YAML: {err}"),
            ));
        }
    };

    // Sort keys if requested
    if sort_keys && let Some(obj) = json_value.as_object_mut() {
        let sorted_map: BTreeMap<_, _> = obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        *obj = sorted_map.into_iter().collect();
    }

    match dbt_yaml::to_string(&json_value) {
        Ok(yaml_str) => Ok(Value::from(yaml_str)),
        Err(err) => Err(Error::new(
            ErrorKind::InvalidOperation,
            format!("Failed to convert value to YAML: {err}"),
        )),
    }
}

/// A function that returns the difference between two dictionaries
/// Not documented in dbt Jinja docs, but included in base.py
pub fn diff_of_two_dicts_fn() -> impl Fn(&[Value], Kwargs) -> Result<Value, Error> {
    move |args: &[Value], kwargs: Kwargs| -> Result<Value, Error> {
        if args.len() != 2 {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "diff_of_two_dicts requires exactly 2 arguments",
            ));
        }

        let dict_a_arg = match (kwargs.get("dict_a"), args.first()) {
            (Ok(value), _) => value,
            (_, Some(value)) => value,
            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "diff_of_two_dicts requires a dict_a argument",
                ));
            }
        }
        .clone();

        let dict_b_arg = match (kwargs.get("dict_b"), args.get(1)) {
            (Ok(value), _) => value,
            (_, Some(value)) => value,
            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "diff_of_two_dicts requires a dict_b argument",
                ));
            }
        }
        .clone();

        let dict_a = parse_dict_of_lists(&dict_a_arg)?;
        let dict_b = parse_dict_of_lists(&dict_b_arg)?;

        // Convert dict_b to lowercase for case-insensitive comparison.
        // IndexMap preserves insertion order so `diff_of_two_dicts` matches
        // Python's dict semantics (iteration follows dict_a's insertion order),
        // which apply_grants macros rely on for deterministic REVOKE/GRANT order.
        let mut dict_b_lowered: IndexMap<String, Vec<String>> = IndexMap::new();
        for (key, value_list) in dict_b {
            dict_b_lowered.insert(
                key.to_lowercase(),
                value_list.into_iter().map(|v| v.to_lowercase()).collect(),
            );
        }

        // Perform the difference
        let mut dict_diff: IndexMap<String, Vec<String>> = IndexMap::new();
        for (key, value_list) in dict_a {
            if let Some(lowered_b_vals) = dict_b_lowered.get(&key.to_lowercase()) {
                // Filter out values that appear in dict_b, ignoring case
                let diff: Vec<String> = value_list
                    .into_iter()
                    .filter(|v| !lowered_b_vals.contains(&v.to_lowercase()))
                    .collect();
                if !diff.is_empty() {
                    dict_diff.insert(key, diff);
                }
            } else {
                // Key doesn't exist in dict_b ignoring case, so keep all
                dict_diff.insert(key, value_list);
            }
        }
        Ok(Value::from_serialize(&dict_diff))
    }
}

/// Convert any iterable to a set with unique elements.
///
/// Args:
///     value: An iterable value to convert to a set (required)
///     default: (optional) Value to return if conversion fails (can be passed
///              as second positional argument or kwarg: default="...")
///
/// Example:
/// ```jinja
/// {% set my_list = [1, 2, 2, 3] %}
/// {% set unique_values = set(my_list) %}
/// -- Returns set with {1, 2, 3}
/// {% set empty = set([]) %}
/// -- Returns empty set
/// ```
pub fn set_fn() -> impl Fn(&[Value], Kwargs) -> Result<Value, Error> {
    move |args: &[Value], kwargs: Kwargs| -> Result<Value, Error> {
        if args.is_empty() || args.len() > 2 {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "set() requires 1 argument",
            ));
        }

        let mut arg_parser = ArgParser::new(args, Some(kwargs));
        let value = arg_parser.get::<Value>("value")?;
        let default = arg_parser.get_optional::<Value>("default");

        match value.try_iter() {
            Ok(iter) => Ok(Value::from_object(iter.collect::<MutableSet>())),
            Err(_) => match default {
                Some(def) => Ok(def),
                None => Ok(Value::from(())),
            },
        }
    }
}
/// Renders a string as a Jinja template using the current context.
///
/// Args:
///     sql: The string to render as a template.
///
/// Example:
/// ```jinja
/// {% set rendered = render("Hello {{ this.name }}") %}
/// ```
///
/// Returns:
///     The rendered string with all template expressions evaluated in the current context.
///
/// Errors:
///     Raises an error if the argument is not a string or if rendering fails.
pub fn render_fn() -> impl Fn(&State, &[Value], Kwargs) -> Result<Value, Error> {
    move |state: &State, args: &[Value], _kwargs: Kwargs| -> Result<Value, Error> {
        if args.len() != 1 {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "render requires exactly one argument (the string to render)",
            ));
        }
        // dbt-core (Jinja2/Python) effectively accepts any value here and stringifies it.
        // In practice, many dbt projects call `render(...)` on values that can legitimately be
        // `none` (e.g. optional metadata-driven SQL snippets from `run_query`), expecting
        // `"None"` and handling that downstream.
        //
        // Fusion uses minijinja which is stricter by default; align behavior by accepting
        // `none` and treating it like Python's `str(None)` => `"None"`.
        let sql = if args[0].is_none() {
            "None"
        } else {
            args[0].as_str().ok_or_else(|| {
                Error::new(ErrorKind::InvalidOperation, "Argument must be a string")
            })?
        };

        let env = state.env();

        let template = env.template_from_str(sql)?;
        let rendered = template.render(state.get_base_context(), &[])?;
        Ok(Value::from(rendered))
    }
}

/// Strict version of set() that fails if the input is not iterable.
///
/// Args:
///     value: An iterable value to convert to a set
///
/// Example:
/// ```jinja
/// {% set my_list = [1, 2, 2, 3] %}
/// {% set unique_values = set_strict(my_list) %}
/// -- Returns [1, 2, 3] or fails if my_list is not iterable
/// ```
pub fn set_strict_fn() -> impl Fn(&[Value], Kwargs) -> Result<Value, Error> {
    move |args: &[Value], _kwargs: Kwargs| -> Result<Value, Error> {
        if args.len() != 1 {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "set_strict requires exactly 1 argument",
            ));
        }

        let value = &args[0];
        match value.try_iter() {
            Ok(iter) => {
                let set: BTreeSet<_> = iter.map(|v| v.to_string()).collect();
                Ok(Value::from_iter(set))
            }
            Err(_) => Err(Error::new(
                ErrorKind::InvalidOperation,
                "set_strict requires an iterable value",
            )),
        }
    }
}

/// Try to call a function and raise a CompilationError if it raises an exception.
///
/// Args:
///     message_if_exception: The message to raise if the function raises an exception
///     func: The function to call
///     *args: The arguments to pass to the function
///     **kwargs: The keyword arguments to pass to the function
///
/// Example:
/// ```jinja
/// {% set result = try_or_compiler_error("Error", my_function, arg1, arg2, kwarg1="value1", kwarg2="value2") %}
/// ```
pub fn try_or_compiler_error_fn()
-> impl Fn(&State<'_, '_>, &[Value], Kwargs) -> Result<Value, Error> {
    move |state: &State<'_, '_>, args: &[Value], kwargs: Kwargs| -> Result<Value, Error> {
        let mut args = ArgParser::new(args, Some(kwargs));
        let message_if_exception = args.get::<String>("message_if_exception")?;
        let func = args.get::<Value>("func")?;
        let mut remaining_args = args.get_args_as_vec_of_values();
        let drained_kwargs = args.drain_kwargs();
        let remaining_kwargs = Kwargs::from_iter(drained_kwargs);
        remaining_args.push(remaining_kwargs.into());

        match func.call(state, &remaining_args, &[]) {
            Ok(result) => Ok(result),
            // TODO: we need to raise CompilationError(message_if_exception, self.model)
            Err(_) => Err(Error::new(
                ErrorKind::InvalidOperation,
                message_if_exception,
            )),
        }
    }
}

/// Return an iterator of tuples where each tuple contains the i-th element from each of the input iterables.
/// dbt's zip also supports a custom kwarg `fillvalue` (default=None) to match the longest iterable.
///
/// Args:
///     *iterables: Two or more iterables
///     fillvalue: (optional) Value to fill in shorter iterables, default=None
///
/// Example:
/// ```jinja
/// {% set list1 = [1, 2] %}
/// {% set list2 = ['a', 'b', 'c'] %}
/// {% set pairs = zip(list1, list2, fillvalue='N/A') %}
/// -- Returns [(1, 'a'), (2, 'b'), ('N/A', 'c')]
/// ```
pub fn zip_fn() -> impl Fn(&[Value], Kwargs) -> Result<Value, Error> {
    move |args: &[Value], kwargs: Kwargs| -> Result<Value, Error> {
        if args.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "zip requires at least 1 argument",
            ));
        }

        let default = match (kwargs.get::<Value>("default"), args.get(1)) {
            (Ok(value), _) => Some(value),
            (_, Some(value)) => Some(value.clone()),
            _ => None,
        };

        // Try to convert each argument to an internal Vec<Value>
        let mut iterators: Vec<Vec<Value>> = Vec::new();
        for arg in args {
            match arg.try_iter() {
                Ok(iter) => iterators.push(iter.collect()),
                Err(_) => return Ok(default.unwrap_or_else(|| Value::from(()))),
            }
        }

        // Find shortest length (Python's zip stops at shortest iterator)
        let min_len = iterators.iter().map(|v| v.len()).min().unwrap_or(0);

        let mut zipped = Vec::new();
        for i in 0..min_len {
            let tuple: Vec<Value> = iterators.iter().map(|iter| iter[i].clone()).collect();
            zipped.push(Value::from(tuple));
        }

        Ok(Value::from_iter(zipped))
    }
}

/// Strict version of zip() that fails if any input is not iterable or the iterables differ in length.
///
/// Args:
///     *iterables: Two or more iterables
///
/// Example:
/// ```jinja
/// {% set list1 = [1, 2, 3] %}
/// {% set list2 = ['a', 'b', 'c'] %}
/// {% set pairs = zip_strict(list1, list2) %}
/// -- Returns [(1, 'a'), (2, 'b'), (3, 'c')] or fails if inputs aren't iterable or lengths differ
/// ```
pub fn zip_strict_fn() -> impl Fn(&[Value], Kwargs) -> Result<Value, Error> {
    move |args: &[Value], _kwargs: Kwargs| -> Result<Value, Error> {
        if args.len() < 2 {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "zip_strict requires two or more iterable arguments",
            ));
        }

        let mut iterators: Vec<Vec<Value>> = Vec::new();
        for arg in args {
            match arg.try_iter() {
                Ok(iter) => iterators.push(iter.collect()),
                Err(_) => {
                    return Err(Error::new(
                        ErrorKind::InvalidOperation,
                        "zip_strict requires all arguments to be iterable",
                    ));
                }
            }
        }

        // Find shortest length (Python's zip behavior)
        let min_len = iterators.iter().map(|v| v.len()).min().unwrap_or(0);

        let mut zipped = Vec::new();
        for i in 0..min_len {
            let tuple: Vec<Value> = iterators.iter().map(|iter| iter[i].clone()).collect();
            zipped.push(Value::from(tuple));
        }

        Ok(Value::from_iter(zipped))
    }
}

/// Print a message to the log file and stdout.
///
/// Args:
///     msg: Message to print
///
/// Example:
/// ```jinja
/// {{ print("Hello world!") }}
/// ```
pub fn print_fn() -> impl Fn(&State<'_, '_>, &[Value], Kwargs) -> Result<Value, Error> {
    move |state: &State<'_, '_>, args: &[Value], _kwargs: Kwargs| -> Result<Value, Error> {
        if args.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "print requires at least one argument (a message to print)",
            ));
        }
        if args.len() > 1 {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "print accepts only one argument",
            ));
        }

        // Format the message using Display formatting (not Debug) to match dbt's behavior
        // This ensures strings aren't wrapped in quotes (e.g., "string" instead of "'string'")
        let msg = format!("{}", args[0]);

        // Get metadata for the event
        let current_package_name = state
            .lookup(TARGET_PACKAGE_NAME, &[])
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let line = state.current_span().start_line;
        let column = state.current_span().start_col;
        let cur_file_path = state.current_path().to_str().map(str::to_string);

        // Emit UserLogMessage event for print
        emit_info_event(
            UserLogMessage::print(
                current_package_name,
                Some(line),
                Some(column),
                cur_file_path,
            ),
            Some(&msg),
        );

        Ok(Value::from(""))
    }
}

/// Print a message to the log file and stdout.
///
/// Args:
///     msg: Message to print
///     info: If true, log at info level. If False emit at debug level.
///
/// Example:
/// ```jinja
/// {{ log("Hello world!", info=true) }}
/// ```
pub fn log_fn() -> impl Fn(&State<'_, '_>, &[Value], Kwargs) -> Result<Value, Error> {
    move |state: &State<'_, '_>, args: &[Value], kwargs: Kwargs| -> Result<Value, Error> {
        let mut args = ArgParser::new(args, Some(kwargs));
        let msg = args.get::<Value>("msg")?.to_string();
        let info = args.get::<Value>("info").ok();

        // Get metadata for the event
        let current_package_name = state
            .lookup(TARGET_PACKAGE_NAME, &[])
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let line = state.current_span().start_line;
        let column = state.current_span().start_col;
        let cur_file_path = state.current_path().to_str().map(str::to_string);

        if info.is_some() && info.unwrap().is_true() {
            // Emit UserLogMessage event for log
            emit_info_event(
                UserLogMessage::log_info(
                    current_package_name,
                    Some(line),
                    Some(column),
                    cur_file_path,
                ),
                Some(&msg),
            );
        } else {
            // Emit UserLogMessage event for debug log
            emit_debug_event(
                UserLogMessage::log_debug(
                    current_package_name,
                    Some(line),
                    Some(column),
                    cur_file_path,
                ),
                Some(&msg),
            );
        }

        Ok(Value::from(""))
    }
}

/// Calculate an MD5 hash of the given string.
///
/// Args:
///     value: String to hash
///
/// Example:
/// ```jinja
/// {% set hash = local_md5("hello") %}
/// -- Returns "5d41402abc4b2a76b9719d911017c592"
/// ```
pub fn local_md5_fn() -> impl Fn(&[Value], Kwargs) -> Result<Value, Error> {
    move |args: &[Value], _kwargs: Kwargs| -> Result<Value, Error> {
        if args.len() != 1 {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "local_md5 requires exactly 1 argument",
            ));
        }

        let value = args[0].as_str().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidOperation,
                "local_md5's argument must be a string",
            )
        })?;
        // Create MD5 hasher
        let result = format!("{:x}", md5::compute(value.as_bytes()));
        Ok(Value::from(result))
    }
}

/// Parse a dictionary of lists into a BTreeMap<String, Vec<String>>
fn parse_dict_of_lists(dict: &Value) -> Result<IndexMap<String, Vec<String>>, Error> {
    let mut result = IndexMap::new();

    // Iterate over the keys in the dictionary
    for key in dict.try_iter()? {
        // Get the value associated with the key
        let value = dict.get_item(&key)?;

        // Try to iterate over the value as a list
        let mut value_list = Vec::new();
        for item in value.try_iter()? {
            value_list.push(item.to_string());
        }
        // Insert the key-value pair into the result
        result.insert(key.to_string(), value_list);
    }

    Ok(result)
}

/// A struct that represents the 'exceptions' object, which makes exceptions.warn() and...
#[derive(Debug)]
pub struct Exceptions {
    warn_error_options: WarnErrorOptions,
}

impl Object for Exceptions {
    // todo: create a shared enum for call_method and get_value to work off
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        // exceptions evaluate to 'true' in Python for truthiness checks
        // usage: https://github.com/dbt-labs/dbt-adapters/blob/be0ab62ae2ad3504a37287f7a4ac12d30e7e94d9/dbt-adapters/src/dbt/include/global_project/macros/materializations/snapshots/helpers.sql#L271
        match key.as_str()? {
            "warn_snapshot_timestamp_data_types" => Some(Value::from(true)),
            _ => None,
        }
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        // reference: core/dbt/context/exceptions_jinja.py
        // We are only implementing methods actually used in the supported adapters.
        match method {
            "warn" => {
                let mut args = ArgParser::new(args, None);
                let warn_string = args.get::<String>("").unwrap_or_else(|_| "".to_string());
                let current_span = state.current_span_of_context();
                let current_file_path = state.current_path().clone();
                let warning = Box::new(
                    FsError::new_no_backtrace(ErrorCode::JinjaWarn, warn_string.clone())
                        .with_location(CodeLocationWithFile::new(
                            current_span.start_line,
                            current_span.start_col,
                            current_span.start_offset,
                            current_file_path,
                        )),
                );

                // Emit through the warn path even when warn-error upgrades it because tracing
                // handles the event level upgrade for dbt-facing outputs.
                let warn_error_decision = self
                    .warn_error_options
                    .decision_for_error_code(warning.code)
                    == WarnErrorDecision::UpgradeToError;
                emit_warn_log_from_fs_error(*warning);

                if warn_error_decision {
                    return Err(Error::new(ErrorKind::ExitWithStatus, warn_string));
                }

                Ok(Value::UNDEFINED)
            }
            // (msg, node=None)
            "raise_compiler_error" => {
                let mut args = ArgParser::new(args, None);
                let message = args.get::<String>("msg")?;
                if let Some((node_id, file_path)) = node_metadata_from_state(state) {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!(
                            "Compilation Error for {} from {}: {}",
                            node_id,
                            file_path.display(),
                            message
                        ),
                    ))
                } else {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!("Compilation Error: {message}"),
                    ))
                }
            }
            // (msg) String
            "raise_not_implemented" => {
                let mut args = ArgParser::new(args, None);
                let message = args.get::<String>("msg")?;
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!("Not implemented: {message}"),
                ))
            }
            // (relation, expected_type, model=None)
            //   Relation,  String
            "relation_wrong_type" => {
                let mut args = ArgParser::new(args, None);
                let relation = args.get::<Value>("relation").unwrap_or(Value::UNDEFINED);
                let expected_type = args.get::<String>("expected_type").unwrap_or_default();

                // Get the relation type from the relation value
                let relation_type = if relation.is_undefined() {
                    "unknown".to_string()
                } else {
                    match relation.get_item(&Value::from("type")) {
                        Ok(type_value) => type_value.to_string(),
                        Err(_) => "unknown".to_string(),
                    }
                };

                let message = format!(
                    "Trying to create {expected_type} {relation}, but it currently exists as a {relation_type}. Either drop {relation} manually, or run dbt with `--full-refresh` and dbt will drop it for you."
                );
                if let Some((node_id, file_path)) = node_metadata_from_state(state) {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!(
                            "Compilation Error for {} from {}: {}",
                            node_id,
                            file_path.display(),
                            message
                        ),
                    ))
                } else {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!("Compilation Error: {message}"),
                    ))
                }
            }
            // (yaml_columns, sql_columns)
            // [{"name": ..., "data_type": ..., "formatted": ...},...]
            "raise_contract_error" => {
                let mut args = ArgParser::new(args, None);
                let yaml_columns = args
                    .get::<Value>("yaml_columns")
                    .unwrap_or(Value::UNDEFINED);
                let sql_columns = args.get::<Value>("sql_columns").unwrap_or(Value::UNDEFINED);
                let column_diff_table: &Arc<AgateTable> =
                    get_contract_mismatches(yaml_columns, sql_columns)?;
                let column_diff_display = column_diff_table
                    .display()
                    .with_max_rows(50)
                    .with_max_columns(50)
                    .with_max_column_width(50);
                let message = format_args!(
                    "This model has an enforced contract that failed.\n Please ensure the name, data_type, and number of columns in your contract match the columns in your model's definition.\n\n{column_diff_display}"
                );
                if let Some((node_id, file_path)) = node_metadata_from_state(state) {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!(
                            "Compilation Error for {} from {}: {}",
                            node_id,
                            file_path.display(),
                            message
                        ),
                    ))
                } else {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!("Compilation Error: {message}"),
                    ))
                }
            }
            // (column_names)
            // ["column1", "column2"]
            "column_type_missing" => {
                let mut args = ArgParser::new(args, None);
                // column_names should be a list of strings
                let column_names = args
                    .get::<Value>("column_names")
                    .unwrap_or(Value::UNDEFINED);

                // Convert column_names to a vector of strings
                let column_names_string = if column_names.is_undefined() {
                    "".to_string()
                } else {
                    match column_names.try_iter() {
                        Ok(iter) => {
                            let strings: Vec<String> = iter.map(|v| v.to_string()).collect();
                            strings.join(", ")
                        }
                        Err(_) => "".to_string(),
                    }
                };

                let message = format!(
                    "Contracted models require data_type to be defined for each column.  Please ensure that the column name and data_type are defined within the YAML configuration for the {column_names_string} column(s)."
                );
                if let Some((node_id, file_path)) = node_metadata_from_state(state) {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!(
                            "Contract Error for {} from {}: {}",
                            node_id,
                            file_path.display(),
                            message
                        ),
                    ))
                } else {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!("Contract Error: {message}"),
                    ))
                }
            }
            // (msg, node=None)
            "raise_fail_fast_error" => {
                let mut args = ArgParser::new(args, None);
                let message = args.get::<String>("msg")?;
                if let Some((node_id, file_path)) = node_metadata_from_state(state) {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!(
                            "FailFast Error for {} from {}: {}",
                            node_id,
                            file_path.display(),
                            message
                        ),
                    ))
                } else {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!("FailFast Error: {message}"),
                    ))
                }
            }
            // (snapshot_time_data_type: str, updated_at_data_type: str)
            "warn_snapshot_timestamp_data_types" => {
                let mut args = ArgParser::new(args, None);
                let snapshot_time_data_type = args
                    .get::<String>("snapshot_time_data_type")
                    .unwrap_or_else(|_| "".to_string());
                let updated_at_data_type = args
                    .get::<String>("updated_at_data_type")
                    .unwrap_or_else(|_| "".to_string());

                let metadata = node_metadata_from_state(state);
                let snapshot_name = state.lookup("model", &[]).and_then(|m| {
                    m.get_attr("name")
                        .ok()
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                });

                let name_part = snapshot_name
                    .as_deref()
                    .map(|n| format!("snapshot '{n}'"))
                    .unwrap_or_else(|| "snapshot table".to_string());
                let location_hint = metadata
                    .map(|(_, path)| format!("\n  --> {}", path.display()))
                    .unwrap_or_default();

                let warning = format!(
                    "Data type of {name_part} hard-delete timestamps ({snapshot_time_data_type}) does not match its 'updated_at'-derived timestamp columns ({updated_at_data_type}). Values written when closing out deleted rows will be implicitly converted. Override the 'snapshot_get_time' macro in your project to emit a matching type.{location_hint}"
                );

                emit_warn_log_message(ErrorCode::SnapshotTimestampMismatch, warning);

                Ok(Value::UNDEFINED)
            }
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("Unknown method on Exceptions: {method}"),
            )),
        }
    }
}

/// Build the graph with node-backed entries; field values are serialized on access.
/// Graph entries keyed by unique id, each a lazy context over its node.
fn lazy_graph_entries<T: dbt_schemas::schemas::InternalDbtNode>(
    nodes: &BTreeMap<String, Arc<T>>,
    mode: dbt_schemas::schemas::serialization_utils::SerializationMode,
) -> Value {
    Value::from_serialize(
        nodes
            .iter()
            .map(|(uid, node)| {
                let context =
                    dbt_schemas::jinja_node::LazyNodeContext::from_node(node.clone(), mode);
                (uid.clone(), Value::from_object(context))
            })
            .collect::<BTreeMap<String, Value>>(),
    )
}

pub fn build_flat_graph(nodes: &Nodes, defer_nodes: Option<&Nodes>) -> MutableMap {
    use dbt_schemas::jinja_node::LazyNodeContext;
    use dbt_schemas::schemas::serialization_utils::SerializationMode::{KeepNone, OmitNone};

    let mut graph = ValueMap::new();
    let mut graph_nodes = BTreeMap::new();
    let strip_path = |context: &mut LazyNodeContext, prefix: &'static str| {
        context.transform("path", move |value| {
            if let Some(path) = value.as_str() {
                let normalized = path.replace('\\', "/");
                let stripped = normalized.strip_prefix(prefix).unwrap_or(&normalized);
                YmlValue::string(stripped.to_string())
            } else {
                value
            }
        });
    };
    for (uid, model) in &nodes.models {
        let mut context = LazyNodeContext::from_node(model.clone(), KeepNone);
        context.transform("description", |value| {
            if value.as_str().is_some() {
                value
            } else {
                YmlValue::string(String::new())
            }
        });
        strip_path(&mut context, "models/");
        let relation = defer_nodes
            .and_then(|nodes| nodes.models.get(uid))
            .and_then(|node| dbt_yaml::to_value(defer_relation_for_model(node.as_ref())).ok())
            .unwrap_or_else(YmlValue::null);
        context.insert(
            Value::from("defer_relation"),
            Value::from_serialize(relation),
        );
        graph_nodes.insert(uid.clone(), Value::from_object(context));
    }
    for (uid, snapshot) in &nodes.snapshots {
        let mut context = LazyNodeContext::from_node(snapshot.clone(), KeepNone);
        strip_path(&mut context, "snapshots/");
        let relation = defer_nodes
            .and_then(|nodes| nodes.snapshots.get(uid))
            .and_then(|node| dbt_yaml::to_value(defer_relation_for_snapshot(node.as_ref())).ok())
            .unwrap_or_else(YmlValue::null);
        context.insert(
            Value::from("defer_relation"),
            Value::from_serialize(relation),
        );
        graph_nodes.insert(uid.clone(), Value::from_object(context));
    }
    for (uid, test) in &nodes.tests {
        let mut context = LazyNodeContext::from_node(test.clone(), KeepNone);
        context.insert(
            Value::from("original_file_path"),
            Value::from(test.manifest_original_file_path.display().to_string()),
        );
        context.transform("path", |value| {
            if let Some(path) = value.as_str() {
                let name = std::path::Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(path);
                YmlValue::string(name.to_string())
            } else {
                value
            }
        });
        context.transform("config", |mut config| {
            if let YmlValue::Mapping(ref mut map, _) = config {
                let key = YmlValue::string("severity".to_string());
                if map.get(&key).and_then(YmlValue::as_str).is_none() {
                    map.insert(key, YmlValue::string("ERROR".to_string()));
                }
            }
            config
        });
        graph_nodes.insert(uid.clone(), Value::from_object(context));
    }
    for (uid, seed) in &nodes.seeds {
        let mut context = LazyNodeContext::from_node(seed.clone(), KeepNone);
        strip_path(&mut context, "seeds/");
        let relation = defer_nodes
            .and_then(|nodes| nodes.seeds.get(uid))
            .and_then(|node| dbt_yaml::to_value(defer_relation_for_seed(node.as_ref())).ok())
            .unwrap_or_else(YmlValue::null);
        context.insert(
            Value::from("defer_relation"),
            Value::from_serialize(relation),
        );
        graph_nodes.insert(uid.clone(), Value::from_object(context));
    }
    graph.insert(Value::from("nodes"), Value::from_serialize(graph_nodes));
    for (key, entries) in [
        ("sources", lazy_graph_entries(&nodes.sources, KeepNone)),
        (
            "unit_tests",
            lazy_graph_entries(&nodes.unit_tests, KeepNone),
        ),
        ("exposures", lazy_graph_entries(&nodes.exposures, OmitNone)),
        ("metrics", lazy_graph_entries(&nodes.metrics, OmitNone)),
        (
            "semantic_models",
            lazy_graph_entries(&nodes.semantic_models, OmitNone),
        ),
        (
            "saved_queries",
            lazy_graph_entries(&nodes.saved_queries, OmitNone),
        ),
        ("functions", lazy_graph_entries(&nodes.functions, OmitNone)),
    ] {
        graph.insert(Value::from(key), entries);
    }
    // TODO(LazyNodeContext): This serializes the struct directly, so a group entry has the keys
    // `__common_attr__`, `__base_attr__` and `__group_attr__`, which are read-only maps. In
    // dbt-core it is a flat dict (`name`, `owner`, `unique_id`, ...), like other graph entries.
    let groups_insert: BTreeMap<String, Value> = nodes
        .groups
        .iter()
        .map(|(unique_id, group)| (unique_id.clone(), Value::from_serialize(Arc::as_ref(group))))
        .collect();
    graph.insert(Value::from("groups"), Value::from_serialize(groups_insert));
    MutableMap::from(graph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::path::DbtPath;
    use minijinja::{Environment, Value};
    use minijinja_contrib::pycompat::unknown_method_callback;

    #[test]
    fn otel_trace_id_and_span_id_are_callable_with_nothing_bound_in_scope() {
        // Must work even with nothing bound in the render context at all
        // (e.g. connection-level queries with no `node`).
        use dbt_tracing::emit::create_debug_span;
        use dbt_tracing::test_support::mocks::{MockDynSpanEvent, test_data_layer};
        use tracing::level_filters::LevelFilter;

        let trace_id: u128 = 0xabc;
        let subscriber = dbt_tracing::init::create_tracing_subcriber_with_layer(
            LevelFilter::TRACE,
            test_data_layer(
                trace_id,
                None,
                false,
                std::iter::empty(),
                std::iter::empty(),
            ),
            &[],
        )
        .expect("subscriber should build");

        tracing::subscriber::with_default(subscriber, || {
            let span = create_debug_span(MockDynSpanEvent::default());
            let _guard = span.enter();

            let mut env = Environment::new();
            register_base_functions(&mut env, WarnErrorOptions::default());

            let tmpl = env
                .template_from_str("{{ otel_trace_id() }}|{{ otel_span_id()|length }}")
                .unwrap();
            let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
            assert_eq!(output, format!("{trace_id:032x}|16"));
        });
    }

    #[test]
    fn otel_trace_id_is_captured_once_and_does_not_track_later_spans() {
        use dbt_tracing::emit::create_debug_span;
        use dbt_tracing::test_support::mocks::{MockDynSpanEvent, test_data_layer};
        use tracing::level_filters::LevelFilter;

        let registered_trace_id: u128 = 0x1111;
        let subscriber = dbt_tracing::init::create_tracing_subcriber_with_layer(
            LevelFilter::TRACE,
            test_data_layer(
                registered_trace_id,
                None,
                false,
                std::iter::empty(),
                std::iter::empty(),
            ),
            &[],
        )
        .expect("subscriber should build");

        let mut env = Environment::new();
        tracing::subscriber::with_default(subscriber, || {
            let span = create_debug_span(MockDynSpanEvent::default());
            let _guard = span.enter();
            register_base_functions(&mut env, WarnErrorOptions::default());
        });

        // A later, unrelated trace is active now; otel_trace_id() must still
        // report the one captured above, not this one.
        let other_subscriber = dbt_tracing::init::create_tracing_subcriber_with_layer(
            LevelFilter::TRACE,
            test_data_layer(0x2222, None, false, std::iter::empty(), std::iter::empty()),
            &[],
        )
        .expect("subscriber should build");

        tracing::subscriber::with_default(other_subscriber, || {
            let span = create_debug_span(MockDynSpanEvent::default());
            let _guard = span.enter();

            let tmpl = env.template_from_str("{{ otel_trace_id() }}").unwrap();
            let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
            assert_eq!(output, format!("{registered_trace_id:032x}"));
        });
    }

    #[test]
    fn test_set_union_integration() {
        let mut env = Environment::new();

        // Register the set function from base.rs
        env.add_function("set", set_fn());

        // Enable pycompat for union() method
        env.set_unknown_method_callback(unknown_method_callback);

        // Test the exact DBT use case: {% set res = set([1, 2]).union(set([3, 4])) %}
        let template_source = r#"
        {%- set set1 = set([1, 2, 2]) -%}
        {%- set set2 = set([3, 4, 4]) -%}
        {%- set result = set1.union(set2) -%}
        {{ result | sort | join(',') }}
        "#;

        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();

        // Should contain all unique elements from both sets: 1,2,3,4
        let result = output.trim();
        assert_eq!(result, "1,2,3,4");
    }

    #[test]
    fn test_set_union_multiple_args() {
        let mut env = Environment::new();
        env.add_function("set", set_fn());
        env.set_unknown_method_callback(unknown_method_callback);

        let template_source = r#"
        {%- set set1 = set([1, 2]) -%}
        {%- set set2 = set([3, 4]) -%}
        {%- set set3 = set([5, 6]) -%}
        {%- set result = set1.union(set2, set3) -%}
        {{ result | length }}
        "#;

        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();

        // Should have 6 unique elements
        assert_eq!(output.trim(), "6");
    }

    #[test]
    fn test_set_union_with_duplicates() {
        let mut env = Environment::new();
        env.add_function("set", set_fn());
        env.set_unknown_method_callback(unknown_method_callback);

        let template_source = r#"
        {%- set original = [1, 1, 2, 2, 3] -%}
        {%- set other = [3, 4, 4, 5] -%}
        {%- set result = set(original).union(set(other)) -%}
        {{ result | sort | join(',') }}
        "#;

        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();

        // Should remove duplicates: 1,2,3,4,5
        assert_eq!(output.trim(), "1,2,3,4,5");
    }

    #[test]
    fn test_render_accepts_none() {
        let mut env = Environment::new();
        env.add_function("render", render_fn());

        // dbt projects frequently pass `none` into `render(...)` (often after `run_query`)
        // and expect it to stringify to "None" (Python/Jinja2 behavior).
        let template_source = r#"{{ render(None) }}"#;
        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();

        assert_eq!(output.trim(), "None");
    }

    #[test]
    fn test_tojson_simple_dict() {
        let mut env = Environment::new();
        env.add_function("tojson", tojson);

        let template_source = r#"{{ tojson({'a': 1, 'b': 'hello'}) }}"#;
        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), r#"{"a": 1, "b": "hello"}"#);
    }

    #[test]
    fn test_tojson_nested_dict() {
        let mut env = Environment::new();
        env.add_function("tojson", tojson);

        let template_source = r#"{{ tojson({'a': 1, 'b': {'c': 3}}) }}"#;
        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), r#"{"a": 1, "b": {"c": 3}}"#);
    }

    #[test]
    fn test_tojson_with_sort_keys() {
        let mut env = Environment::new();
        env.add_function("tojson", tojson);

        let template_source = r#"{{ tojson({'b': 2, 'a': 1}, sort_keys=True) }}"#;
        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), r#"{"a": 1, "b": 2}"#);
    }

    #[test]
    fn test_tojson_escapes_non_ascii() {
        let mut env = Environment::new();
        env.add_function("tojson", tojson);
        env.add_function("local_md5", local_md5_fn());

        let cases = [
            ("{{ tojson({'x': '\u{3042}'}) }}", r#"{"x": "\u3042"}"#),
            (
                "{{ tojson({'x': '\u{e9}\u{1f600}\u{2028}'}) }}",
                r#"{"x": "\u00e9\ud83d\ude00\u2028"}"#,
            ),
            (
                "{{ tojson({'k\u{e9}': 'a\"b\\n\u{7f}'}) }}",
                r#"{"k\u00e9": "a\"b\n\u007f"}"#,
            ),
        ];
        for (src, expected) in cases {
            let output = env.render_str(src, (), &[]).unwrap();
            assert_eq!(output, expected, "{src}");
        }

        // Same digest as dbt 1.x: md5(json.dumps({"x": "\u3042"}))
        let output = env
            .render_str("{{ local_md5(tojson({'x': '\u{3042}'})) }}", (), &[])
            .unwrap();
        assert_eq!(output, "f3dd52a0cdda6cf470609dcfc4759191");
    }

    #[test]
    fn test_toyaml_simple_dict() {
        let mut env = Environment::new();
        env.add_function("toyaml", toyaml);

        let template_source = r#"{{ toyaml({'a': 1, 'b': 'hello'}) }}"#;
        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), "a: 1\nb: hello");
    }

    #[test]
    fn test_toyaml_nested_dict() {
        let mut env = Environment::new();
        env.add_function("toyaml", toyaml);

        let template_source = r#"{{ toyaml({'a': 1, 'b': {'c': 3}}) }}"#;
        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), "a: 1\nb:\n  c: 3");
    }

    #[test]
    fn test_toyaml_with_sort_keys() {
        let mut env = Environment::new();
        env.add_function("toyaml", toyaml);

        let template_source = r#"{{ toyaml({'b': 2, 'a': 1}, sort_keys=True) }}"#;
        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), "a: 1\nb: 2");
    }

    #[test]
    fn test_fromyaml_allows_duplicate_keys() {
        let mut env = Environment::new();
        env.add_func_func("fromyaml", fromyaml);

        let template_source = r#"
            {% set parsed = fromyaml("HASHED_COLUMNS:\n  COMPANY_X_CONTACT_HASH_ID: old\n  COMPANY_X_CONTACT_HASH_ID: new") %}
            {{ parsed.HASHED_COLUMNS.COMPANY_X_CONTACT_HASH_ID }}
        "#;
        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), "new");
    }

    #[test]
    fn test_fromyaml_returns_default_on_parse_error() {
        let mut env = Environment::new();
        env.add_func_func("fromyaml", fromyaml);

        let template_source = r#"{{ fromyaml("[", default="fallback") }}"#;
        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), "fallback");
    }

    #[test]
    fn test_fromyaml_returns_none_on_parse_error_without_default() {
        let mut env = Environment::new();
        env.add_func_func("fromyaml", fromyaml);

        let tmpl = env
            .template_from_str(r#"{{ fromyaml("[") is none }}"#)
            .unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), "True");
    }

    #[test]
    fn doc_macro_missing_doc_returns_placeholder() {
        let mut env = Environment::new();
        let docs = BTreeMap::<(String, String), String>::new();
        env.add_global(
            "doc",
            Value::from_object(DocMacro::new("pkg".to_string(), docs)),
        );

        let template_source = "{{ doc('unknown') }}";
        let tmpl = env.template_from_str(template_source).unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert!(
            output.contains("<missing doc('unknown', package='pkg')>"),
            "expected placeholder, got {output}"
        );

        // Only the documentation context is strict; model descriptions still tolerate these.
        for (source, expected) in [
            ("{{ doc(1) }}", "<missing doc('1', package='pkg')>"),
            (
                "{{ doc('a', 'b', 'extra') }}",
                "<missing doc('b', package='a')>",
            ),
            (
                "{{ doc('unknown', ignored='x') }}",
                "<missing doc('unknown', package='pkg')>",
            ),
        ] {
            let output = env
                .template_from_str(source)
                .unwrap()
                .render(Value::UNDEFINED, &[])
                .unwrap();
            assert!(output.contains(expected), "{source} rendered {output}");
        }
    }

    #[test]
    fn test_fromjson_returns_none_for_invalid_json() {
        let mut env = Environment::new();
        env.add_func_func("fromjson", fromjson);

        let tmpl = env
            .template_from_str("{{ fromjson('plain: [tag') is none }}")
            .unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), "True");
    }

    #[test]
    fn test_fromjson_returns_default_for_invalid_json() {
        let mut env = Environment::new();
        env.add_func_func("fromjson", fromjson);

        let tmpl = env
            .template_from_str("{{ fromjson('plain: [tag', default={'fallback': 1}) }}")
            .unwrap();
        let output = tmpl.render(Value::UNDEFINED, &[]).unwrap();
        assert_eq!(output.trim(), "{'fallback': 1}");
    }

    fn make_env_with_var() -> minijinja::Environment<'static> {
        let mut env = minijinja::Environment::new();

        // Mirror Fusion's global Jinja context: dbt projects frequently use
        // Python-ish constants (capitalized) like `None`.
        env.add_global("None", Value::from(()));
        env.add_global("True", Value::from(true));
        env.add_global("False", Value::from(false));

        env.add_global("var", Value::from_object(Var::new(BTreeMap::new())));
        env
    }

    #[test]
    fn var_default_keyword_none_is_treated_as_none() {
        let env = make_env_with_var();
        let template = env
            .template_from_str(
                "{% set x = var('x', default=None) %}{% if x is not none %}NOT{% else %}NONE{% endif %}",
            )
            .unwrap();

        let rendered = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(rendered, "NONE");
    }

    #[test]
    fn var_default_keyword_string_is_used_when_var_missing() {
        let env = make_env_with_var();
        let template = env
            .template_from_str("{{ var('x', default='abc') }}")
            .unwrap();

        let rendered = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(rendered, "abc");
    }

    #[test]
    fn var_default_positional_string_is_used_when_var_missing() {
        let env = make_env_with_var();
        let template = env.template_from_str("{{ var('x', 'abc') }}").unwrap();

        let rendered = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(rendered, "abc");
    }

    #[test]
    fn var_default_positional_map_is_used_when_var_missing() {
        let env = make_env_with_var();
        let template = env
            .template_from_str("{{ var('config', {'materialized': 'table'}).materialized }}")
            .unwrap();

        let rendered = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(rendered, "table");
    }

    #[test]
    fn var_has_var() {
        let env = make_env_with_var();
        let template = env
            .template_from_str("{{ var.has_var('somevar') }}")
            .unwrap();

        let rendered = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(rendered, "False");
    }

    #[test]
    fn build_flat_graph_populates_semantic_models_metrics_saved_queries_groups_and_unit_tests() {
        use dbt_schemas::schemas::manifest::{DbtMetric, DbtSavedQuery, DbtSemanticModel};
        use dbt_schemas::schemas::nodes::{DbtGroup, DbtUnitTest};

        let mut nodes = Nodes::default();

        // Insert one entry per collection
        nodes.semantic_models.insert(
            "semantic_model.pkg.sm1".to_string(),
            Arc::new(DbtSemanticModel::default()),
        );
        nodes.unit_tests.insert(
            "unit_test.pkg.ut1".to_string(),
            Arc::new(DbtUnitTest::default()),
        );
        nodes.metrics.insert(
            "metric.pkg.m1".to_string(),
            Arc::new(DbtMetric {
                __common_attr__: Default::default(),
                __base_attr__: Default::default(),
                __metric_attr__: Default::default(),
                deprecated_config: Default::default(),
                __other__: Default::default(),
            }),
        );
        nodes.saved_queries.insert(
            "saved_query.pkg.sq1".to_string(),
            Arc::new(DbtSavedQuery {
                __common_attr__: Default::default(),
                __base_attr__: Default::default(),
                __saved_query_attr__: Default::default(),
                deprecated_config: Default::default(),
                __other__: Default::default(),
            }),
        );
        nodes
            .groups
            .insert("group.pkg.g1".to_string(), Arc::new(DbtGroup::default()));

        let graph = build_flat_graph(&nodes, None);
        let graph_val = Value::from_object(graph);

        // Each key should contain exactly one entry
        for key in &[
            "semantic_models",
            "metrics",
            "saved_queries",
            "groups",
            "unit_tests",
        ] {
            let collection = graph_val.get_attr(key).unwrap();
            assert_ne!(
                collection.len(),
                Some(0),
                "expected graph.{key} to be non-empty"
            );
        }
    }

    /// Regression test for https://github.com/dbt-labs/dbt-fusion/issues/1366:
    /// Each model/snapshot/seed in `graph.nodes` must carry a `defer_relation`
    /// key — populated when `defer_nodes` provides a match, otherwise null.
    #[test]
    fn build_flat_graph_populates_defer_relation_for_deferrable_nodes() {
        use dbt_schemas::schemas::nodes::{CommonAttributes, NodeBaseAttributes};
        use dbt_schemas::schemas::{DbtModel, DbtSeed, DbtSnapshot};

        fn make_model(unique_id: &str, name: &str, alias: &str, schema: &str) -> Arc<DbtModel> {
            Arc::new(DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: unique_id.to_string(),
                    name: name.to_string(),
                    package_name: "pkg".to_string(),
                    fqn: vec!["pkg".to_string(), name.to_string()],
                    path: DbtPath::from(format!("{name}.sql")),
                    original_file_path: DbtPath::from(format!("models/{name}.sql")),
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    database: "prod_db".to_string(),
                    schema: schema.to_string(),
                    alias: alias.to_string(),
                    relation_name: Some(format!("\"prod_db\".\"{schema}\".\"{alias}\"")),
                    ..Default::default()
                },
                ..Default::default()
            })
        }

        // Current state: model.pkg.foo, model.pkg.bar
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.pkg.foo".to_string(),
            make_model("model.pkg.foo", "foo", "foo", "dev"),
        );
        nodes.models.insert(
            "model.pkg.bar".to_string(),
            make_model("model.pkg.bar", "bar", "bar", "dev"),
        );
        nodes.snapshots.insert(
            "snapshot.pkg.snap".to_string(),
            Arc::new(DbtSnapshot::default()),
        );
        nodes
            .seeds
            .insert("seed.pkg.seed".to_string(), Arc::new(DbtSeed::default()));

        // Defer state has only model.pkg.foo, with prod schema/alias
        let mut defer_nodes = Nodes::default();
        defer_nodes.models.insert(
            "model.pkg.foo".to_string(),
            make_model("model.pkg.foo", "foo", "foo", "prod"),
        );

        let graph = build_flat_graph(&nodes, Some(&defer_nodes));
        let graph_val = Value::from_object(graph);
        let nodes_val = graph_val.get_attr("nodes").unwrap();

        // model.pkg.foo: defer_relation populated from defer_nodes.
        // Wire shape mirrors dbt-core's `DeferRelation` dataclass (#1366).
        let foo = nodes_val.get_attr("model.pkg.foo").unwrap();
        let foo_defer = foo.get_attr("defer_relation").unwrap();
        assert!(
            !foo_defer.is_none(),
            "model.pkg.foo defer_relation must not be null when present in defer_nodes"
        );
        assert_eq!(
            foo_defer.get_attr("database").unwrap().to_string(),
            "prod_db"
        );
        assert_eq!(
            foo_defer.get_attr("schema").unwrap().to_string(),
            "prod",
            "defer_relation.schema should reflect the deferred (prior) schema"
        );
        assert_eq!(foo_defer.get_attr("alias").unwrap().to_string(), "foo");
        assert_eq!(
            foo_defer.get_attr("relation_name").unwrap().to_string(),
            "\"prod_db\".\"prod\".\"foo\"",
        );
        assert_eq!(
            foo_defer.get_attr("resource_type").unwrap().to_string(),
            "model",
        );
        assert_eq!(foo_defer.get_attr("name").unwrap().to_string(), "foo");
        // dbt-core emits description as a non-optional string (default "").
        assert_eq!(foo_defer.get_attr("description").unwrap().to_string(), "");
        // compiled_code is None for models that didn't round-trip through compile state.
        assert!(foo_defer.get_attr("compiled_code").unwrap().is_none());
        // config is populated even when the source config is mostly defaults.
        assert!(!foo_defer.get_attr("config").unwrap().is_none());
        // unique_id was emitted in #9865 but is NOT in dbt-core's DeferRelation
        // — ensure the corrected shape no longer carries it.
        assert!(
            foo_defer.get_attr("unique_id").is_err()
                || foo_defer.get_attr("unique_id").unwrap().is_undefined(),
            "defer_relation must not include `unique_id` (not in dbt-core's wire shape)"
        );

        // model.pkg.bar: not in defer_nodes, key must still exist as null
        let bar = nodes_val.get_attr("model.pkg.bar").unwrap();
        let bar_defer = bar.get_attr("defer_relation").unwrap();
        assert!(
            bar_defer.is_none(),
            "defer_relation must be null (not missing) for nodes absent from defer_nodes"
        );

        // Snapshots and seeds also get the key (always null when not in defer_nodes)
        let snap = nodes_val.get_attr("snapshot.pkg.snap").unwrap();
        assert!(snap.get_attr("defer_relation").unwrap().is_none());
        let seed = nodes_val.get_attr("seed.pkg.seed").unwrap();
        assert!(seed.get_attr("defer_relation").unwrap().is_none());
    }

    /// When called without defer_nodes (parse phase, or non-defer runs),
    /// defer_relation is still emitted as null on every deferrable node so
    /// `node.defer_relation is not none` doesn't error.
    #[test]
    fn build_flat_graph_emits_null_defer_relation_when_no_defer_nodes() {
        use dbt_schemas::schemas::DbtModel;
        use dbt_schemas::schemas::nodes::CommonAttributes;

        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.pkg.foo".to_string(),
            Arc::new(DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: "model.pkg.foo".to_string(),
                    name: "foo".to_string(),
                    package_name: "pkg".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        );

        let graph = build_flat_graph(&nodes, None);
        let graph_val = Value::from_object(graph);
        let foo = graph_val
            .get_attr("nodes")
            .unwrap()
            .get_attr("model.pkg.foo")
            .unwrap();
        let defer_rel = foo.get_attr("defer_relation").unwrap();
        assert!(
            defer_rel.is_none(),
            "defer_relation key must be present and null when defer_nodes is None"
        );
    }

    #[test]
    fn diff_of_two_dicts_preserves_insertion_order() {
        // Regression test: Fusion used to build the diff through a `HashMap`
        // (and `standardize_grants_dict` through a `BTreeMap`), which scrambled
        // or sorted iteration order and caused non-deterministic REVOKE/GRANT
        // statement order in `apply_grants`, producing SQL mismatches against
        // Python dbt (which uses an insertion-ordered dict). The ten keys and
        // deliberately non-alphabetical insertion order here mirror the real
        // `show grants` shape from the fct_directmail SQL-mismatch report:
        // `SELECT` is inserted first (multiple grantees) and must stay first,
        // while the remaining privileges follow in their insertion positions.
        //
        // dict_a is constructed via `Value::from_serialize(&IndexMap)` to mirror
        // the production path: `standardize_grants_dict` returns an `IndexMap`
        // that is handed to Jinja as a `Value` before being passed here.
        let mut env = Environment::new();
        env.add_function("diff_of_two_dicts", diff_of_two_dicts_fn());
        env.add_function("tojson", tojson);
        env.set_unknown_method_callback(unknown_method_callback);

        let mut dict_a: IndexMap<String, Vec<String>> = IndexMap::new();
        dict_a.insert(
            "SELECT".to_string(),
            vec![
                "ALATION".to_string(),
                "DM_APPDBA".to_string(),
                "MONTECARLO".to_string(),
            ],
        );
        for privilege in [
            "APPLYBUDGET",
            "DELETE",
            "EVOLVE SCHEMA",
            "INSERT",
            "REBUILD",
            "REFERENCES",
            "SELECT ERROR TABLE",
            "TRUNCATE",
            "UPDATE",
        ] {
            dict_a.insert(privilege.to_string(), vec!["MONTECARLO".to_string()]);
        }

        // Iterate like the real `apply_grants` macro does — via Jinja
        // `.items()` — since `tojson` would reroute through serde_json and sort
        // keys. This mirrors the production iteration that produces the REVOKE
        // statement sequence.
        let template_source = r#"
{%- set diff = diff_of_two_dicts(dict_a, {}) -%}
{%- for privilege, grantees in diff.items() -%}
{{ privilege }}={{ grantees | join(',') }}
{% endfor -%}
"#;
        let tmpl = env.template_from_str(template_source).unwrap();
        let rendered = tmpl
            .render(
                minijinja::context!(dict_a => Value::from_serialize(&dict_a)),
                &[],
            )
            .unwrap();

        let expected = "\
SELECT=ALATION,DM_APPDBA,MONTECARLO
APPLYBUDGET=MONTECARLO
DELETE=MONTECARLO
EVOLVE SCHEMA=MONTECARLO
INSERT=MONTECARLO
REBUILD=MONTECARLO
REFERENCES=MONTECARLO
SELECT ERROR TABLE=MONTECARLO
TRUNCATE=MONTECARLO
UPDATE=MONTECARLO
";

        assert_eq!(rendered, expected);
    }
}
