use crate::arg_utils::ArgParser;
use crate::constants::DIALECT;
use crate::constants::MACRO_DISPATCH_ORDER;
use crate::constants::TARGET_PACKAGE_NAME;
use crate::listener::RenderingEventListener;
use crate::machinery::Span;
use crate::value::Enumerator;
use crate::value::Object;
use crate::value::{value_map_with_capacity, Kwargs, Value, ValueMap};
use crate::vm::Macro;
use crate::vm::MACRO_RECURSION_COST;
use crate::{Error, ErrorKind, State};

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

/// all the package names in the project and their dependencies
/// - Contains a map of package_name -> Vec<dependencies>
/// - Also contains a special entry "__all__" -> Vec<all_package_names>
pub static THREAD_LOCAL_DEPENDENCIES: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();

/// Dispatch object for Jinja templates
#[derive(Debug)]
pub struct DispatchObject {
    /// The name of the macro to dispatch
    pub macro_name: String,
    /// The user-specified package name to disptch to
    pub package_name: Option<String>,
    /// When true, only look up in the specified package, when false
    /// fallback to default lookup behavior
    pub strict: bool,
    /// Indicates if the object should be automatically invoked by the
    /// interpreter in case it is result of a method call
    pub auto_execute: bool,
    /// The context of the macro
    pub context: Option<Value>,
}

impl Object for DispatchObject {
    fn is_true(self: &Arc<Self>) -> bool {
        true
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&["macro_name", "package_name", "strict", "auto_execute"])
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "macro_name" => Some(Value::from(&self.macro_name)),
            "package_name" => self.package_name.as_ref().map(|s| Value::from(s.as_str())),
            "strict" => Some(Value::from(self.strict)),
            "auto_execute" => Some(Value::from(self.auto_execute)),
            _ => None,
        }
    }

    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        // Check for "." in macro_name
        // TODO: delete this, this is already checked in the adapter.dispatch method
        if self.macro_name.contains('.') {
            let parts: Vec<&str> = self.macro_name.split('.').collect();
            if parts.len() == 2 {
                let (suggest_macro_namespace, suggest_macro_name) = (parts[0], parts[1]);
                let msg = format!(
                    "In adapter.dispatch, got a macro name of \"{}\", \
                    but \".\" is not a valid macro name component. Did you mean \
                    `adapter.dispatch(\"{}\", macro_namespace=\"{}\")`?",
                    self.macro_name, suggest_macro_name, suggest_macro_namespace
                );
                return Err(Error::new(ErrorKind::UnknownFunction, msg));
            }
        }

        let mut attempts = Vec::new();

        // Handle strict mode (used when we want to dispatch to a specific macro in a package with no prefixes)
        if self.strict {
            if let Some(pkg) = &self.package_name {
                let template_name = format!("{}.{}", pkg, self.macro_name);
                attempts.push(template_name.clone());

                if let Some(rv) = self.execute_template(state, &template_name, args, listeners)? {
                    return Ok(rv);
                }
                return Err(self.macro_not_found_error(&attempts));
            }
        }

        // Get search packages according to dbt's logic
        let search_packages = self.get_search_packages(state);

        // Context-first so a node rendered for a non-default adapter dispatches
        // against that adapter; falls back to the environment default otherwise.
        let dialect = resolve_dialect(state);
        let dialect = dialect.as_str();

        // Get adapter specific prefixes
        let adapter_prefixes = get_adapter_prefixes(dialect);
        // get not internal packages
        let non_internal_namespace = state.env().get_non_internal_packages();
        // get dbt and adapters namespace for this node's dialect
        let dbt_and_adapters_namespace = state.env().get_dbt_and_adapters_namespace(dialect);
        // First try with specific packages if specified
        // The logic below comes from https://github.com/dbt-labs/dbt-core/blob/4aa5169212d8256002095d44dc5f2505dca1b07c/core/dbt/context/providers.py#L158
        for package_name_opt in &search_packages {
            if let Some(package_name) = package_name_opt {
                for prefix in &adapter_prefixes {
                    let search_name = format!("{}__{}", prefix, self.macro_name);
                    if package_name == "dbt" {
                        // For dbt package, check dbt_and_adapters namespace
                        let search_name_value = Value::from(&search_name);
                        if let Some(pkg) = dbt_and_adapters_namespace.get(&search_name_value) {
                            let template_name = format!("{pkg}.{search_name}");
                            attempts.push(template_name.clone());
                            if let Some(rv) =
                                self.execute_template(state, &template_name, args, listeners)?
                            {
                                return Ok(rv);
                            }
                        }
                    } else if non_internal_namespace.contains_key(&Value::from(package_name)) {
                        // For non-internal packages
                        let template_name = format!("{package_name}.{search_name}");
                        attempts.push(template_name.clone());
                        if template_exists(state, &template_name) {
                            if let Some(rv) =
                                self.execute_template(state, &template_name, args, listeners)?
                            {
                                return Ok(rv);
                            }
                        }
                    }
                }
            } else {
                // Iterate through adapter prefixes and try to find a template
                for prefix in &adapter_prefixes {
                    let search_name = format!("{}__{}", prefix, self.macro_name);

                    for template_name in
                        macro_namespace_template_candidates(state, &search_name, &mut attempts)
                    {
                        if let Some(rv) =
                            self.execute_template(state, &template_name, args, listeners)?
                        {
                            return Ok(rv);
                        }
                    }
                }
                // find the macro without prefix
                for template_name in
                    macro_namespace_template_candidates(state, &self.macro_name, &mut attempts)
                {
                    if let Some(rv) =
                        self.execute_template(state, &template_name, args, listeners)?
                    {
                        return Ok(rv);
                    }
                }
            }
        }

        Err(self.macro_not_found_error(&attempts))
    }
}

impl DispatchObject {
    fn macro_not_found_error(&self, attempts: &[String]) -> Error {
        let searched = attempts
            .iter()
            .map(|a| format!("'{a}'"))
            .collect::<Vec<_>>()
            .join(", ");

        Error::new(
            ErrorKind::UnknownFunction,
            format!(
                "In dispatch: No macro named '{}' found within namespace: '{}'\n    Searched for: {}",
                self.macro_name,
                self.package_name.clone().unwrap_or_else(|| "None".to_string()),
                searched
            ),
        )
    }

    // Update the get_search_packages method to better match Python's logic
    fn get_search_packages(&self, state: &State<'_, '_>) -> Vec<Option<String>> {
        let root_package = state.env().get_root_package_name();

        match &self.package_name {
            None => {
                // When no namespace is specified, return [None]
                vec![None]
            }
            Some(namespace) => {
                // First check macro_dispatch_order (custom search order)
                // TODO @venkaa28: I (@akbog) moved the lookup here from the environment
                // since there was only a single call site for it. However, I don't like that
                // this would default to empty in the previous implementation.
                let macro_dispatch_order = {
                    let value = state.lookup(MACRO_DISPATCH_ORDER, &[]).unwrap_or_default();
                    let mut value_map = ValueMap::new();

                    if let Ok(keys) = value.try_iter() {
                        for key in keys {
                            if let Ok(val) = value.get_item(&key) {
                                // Convert key to string if needed
                                value_map.insert(key, val);
                            }
                        }
                    }

                    Arc::new(value_map)
                };
                if let Some(order) = macro_dispatch_order.get(&Value::from(namespace.as_str())) {
                    // Use configured order
                    order
                        .downcast_object::<Vec<String>>()
                        .unwrap_or_else(|| Arc::new(vec![]))
                        .as_ref()
                        .iter()
                        .map(|s| Some(s.clone()))
                        .collect()
                } else {
                    // namespaced dispatch looks if the namespace is a dependency of the root project (all packages)
                    #[allow(clippy::incompatible_msrv)]
                    let is_dependency = THREAD_LOCAL_DEPENDENCIES
                        .get()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .contains(namespace);
                    if is_dependency {
                        // For dependencies without explicit order check the root package first, then the namespace
                        vec![Some(root_package), Some(namespace.clone())]
                    } else {
                        // Default case when namespace is specified, no search order, and the namespace
                        // is not a dependency of the current package
                        // Match dbt's behavior of returning [None]
                        vec![None]
                    }
                }
            }
        }
    }

    fn execute_template(
        &self,
        state: &State<'_, '_>,
        template_name: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Option<Value>, Error> {
        let template = match state.env().get_template(template_name) {
            Ok(template) => template,
            Err(err) => {
                // If the template name was found in a namespace but the template itself doesn't exist,
                // this should be a hard error rather than a silent continuation
                return Err(Error::new(
                    ErrorKind::TemplateNotFound,
                    format!(
                        "Template '{template_name}' was found in namespace but cannot be loaded: {err}"
                    ),
                ));
            }
        };
        let template_registry = state.env.get_macro_template_registry();
        let template_registry_entry = template_registry.get(&Value::from(template_name));
        let path = template_registry_entry
            .and_then(|entry| entry.get_attr_fast("path"))
            .unwrap_or_else(|| Value::from(template_name));
        let span = template_registry_entry
            .and_then(|entry| entry.get_attr_fast("span"))
            .unwrap_or_else(|| Value::from_serialize(Span::default()));

        let context = state.get_base_context_with_path_and_span(&path, &span);
        let mut template_state = template.eval_to_state_with_outer_stack_depth(
            context,
            listeners,
            state.ctx.depth() + MACRO_RECURSION_COST,
        )?;

        // When a {% call %} block is used (e.g. {% call statement(...) %}),
        // a caller macro is passed as a kwarg. That macro carries the state_id
        // of the *calling* state. Since we just created a fresh template_state,
        // the caller macro's state_id won't match and calling it would fail with
        // "template state went away". Re-create the caller with the new state's
        // id and copy its instructions over.
        let mut args = args.to_vec();
        let mut parser = ArgParser::new(&args, None);
        if parser.has_kwarg("caller") {
            let last_idx = args.len() - 1;
            let caller = parser.get::<Value>("caller").unwrap();
            if let Some(caller_macro) = caller.downcast_object_ref::<Macro>() {
                let mut new_kwargs = value_map_with_capacity(parser.kwargs_len());
                for (key, value) in parser.kwargs_iter() {
                    new_kwargs.insert(Value::from(key), value.clone());
                }
                new_kwargs.insert(
                    Value::from("caller"),
                    Value::from_object(Macro {
                        name: Value::from("caller"),
                        package_name: caller_macro.package_name.clone(),
                        arg_spec: caller_macro.arg_spec.clone(),
                        macro_ref_id: template_state.macros.len(),
                        state_id: template_state.id,
                        closure: caller_macro.closure.clone(),
                        caller_reference: true,
                        path: caller_macro.path.clone(),
                        span: caller_macro.span,
                    }),
                );
                args[last_idx] = Kwargs::wrap(new_kwargs);

                Arc::make_mut(&mut template_state.macros)
                    .push(state.macros[caller_macro.macro_ref_id]);
            }
        }

        let macro_name = template_name
            .split('.')
            .next_back()
            .expect("template_name should have a dot");
        let Some(func) = template_state.ctx.exports().get(macro_name).cloned() else {
            return Ok(None);
        };
        if func.downcast_object_ref::<Macro>().is_none() {
            return Ok(None);
        }

        // Forward the caller's pending call site (recorded by the vm before the
        // dispatch) onto the macro's template state, since `func.call` runs with
        // `template_state` rather than the original caller state.
        template_state.pending_call_site = state.pending_call_site.clone();

        func.call(&template_state, &args, listeners).map(Some)
    }
}

/// Helper method to get adapter prefixes including parents
pub fn get_adapter_prefixes(dialect: &str) -> Vec<String> {
    let mut prefixes = Vec::new();

    // Current adapter
    prefixes.push(dialect.to_string());

    // Add parent adapters
    match dialect {
        "redshift" => prefixes.push("postgres".to_string()),
        "databricks" => prefixes.push("spark".to_string()),
        // `AdapterType::LakeCompute` renders as `lakecompute`; lake compute defines no
        // macros of its own and inherits DuckDB's.
        "lakecompute" => prefixes.push("duckdb".to_string()),
        // Add other adapter hierarchies as needed
        _ => {}
    }

    // Always add default as last fallback
    prefixes.push("default".to_string());

    prefixes
}

/// The dialect macro resolution should use for the template being rendered.
///
/// Looked up through the render context, which checks the context frames first
/// and only then falls back to the environment global
/// (`Context::load` -> `env.get_global`). That ordering is the whole mechanism
/// for per-node dialects: a node rendered for a non-default adapter has
/// `dialect` injected into its own context, while every other node transparently
/// gets the environment-wide default.
/// Deliberately `ctx.load` rather than `State::lookup`. `lookup` falls through to
/// `macro_namespace_template_resolver` when a name is absent, and that calls back
/// here — so a missing `dialect` would recurse until the stack overflowed.
/// `ctx.load` gives the same context-then-globals ordering with no such
/// fallthrough.
pub fn resolve_dialect(state: &State<'_, '_>) -> String {
    try_resolve_dialect(state).unwrap_or_else(|| "postgres".to_string())
}

/// As [`resolve_dialect`], but distinguishes "no dialect set" from a dialect that
/// happens to be `postgres`.
///
/// Callers that have their own fallback — an adapter falling back to its *own*
/// type, say — must use this. Using `resolve_dialect` there would silently treat
/// an unset dialect as Postgres and the fallback would never fire.
pub fn try_resolve_dialect(state: &State<'_, '_>) -> Option<String> {
    state
        .ctx
        .load(state.env(), DIALECT)
        .and_then(|v| v.as_str().map(str::to_string))
}

/// Select one dialect's `macro_name -> package` namespace out of the
/// `dialect -> namespace` map published as [`crate::constants::DBT_AND_ADAPTERS_NAMESPACE`].
///
/// Shared by dispatch and the typechecker so both resolve internal macros the
/// same way. An unknown dialect yields an empty namespace rather than falling
/// back to another dialect's answer, which would resolve an unprefixed macro to
/// the wrong adapter's implementation.
pub fn dbt_and_adapters_namespace_for(namespaces: &ValueMap, dialect: &str) -> Arc<ValueMap> {
    namespaces
        .get(&Value::from(dialect))
        .and_then(|v| v.clone().downcast_object::<ValueMap>())
        .unwrap_or_else(|| Arc::new(ValueMap::new()))
}

/// Helper method to get internal packages
pub fn get_internal_packages(dialect: &str) -> Vec<String> {
    let mut internal_packages = Vec::new();

    internal_packages.push(format!("dbt_{dialect}"));

    // Add parent packages
    match dialect {
        "redshift" => internal_packages.push("dbt_postgres".to_string()),
        "databricks" => internal_packages.push("dbt_spark".to_string()),
        // See `get_adapter_prefixes`: lake compute has no macro package of its own.
        "lakecompute" => internal_packages.push("dbt_duckdb".to_string()),
        // Add other adapter hierarchies as needed
        _ => {}
    }
    internal_packages.push("dbt".to_string());

    internal_packages
}

/// The union of [`get_internal_packages`] over several dialects, deduped,
/// preserving first-seen order.
///
/// Used to decide which loaded macro packages count as *internal* when a run
/// serves more than one adapter. Resolution itself stays per-dialect — see
/// [`dbt_and_adapters_namespace_for`] — so widening this set does not let one
/// adapter's unprefixed macros leak into another's lookups.
pub fn get_internal_packages_for<'a>(dialects: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut packages: Vec<String> = Vec::new();
    for dialect in dialects {
        for name in get_internal_packages(dialect) {
            if !packages.contains(&name) {
                packages.push(name);
            }
        }
    }
    packages
}

/// Helper function to check if a template exists in the environment
fn template_exists(state: &State<'_, '_>, template_name: &str) -> bool {
    state.env().get_template(template_name).is_ok()
}

/// Finds a template in the namespace according to dbt's resolution rules
///
/// Follows dbt's search order:
/// 1. Local namespace (current package)
/// 2. Root package namespace
/// 3. Internal packages (dbt and adapters)
///
/// # Arguments
/// * `state` - The current state object containing environment info
///     - state must have the following attributes:
///         - root package name from env().get_root_package_name() -- Name of the root package
///         - non-internal packages from env().get_non_internal_packages() -- Map of non-internal packages
///         - dbt and adapters namespace from env().get_dbt_and_adapters_namespace() -- Map of internal packages (dbt and adapters)
/// * `search_name` - Name of the macro to resolve, including prefix (e.g., "postgres__get_test_value")
///
/// Thread local variables:
///     * `current_package_name` - Name of the current package
/// * `attempts` - A vector to track attempted template paths (for error reporting)
///
/// # Returns
/// * `Option<String>` - The template path if found, None otherwise
///
/// Logic comes from https://github.com/dbt-labs/dbt-core/blob/4aa5169212d8256002095d44dc5f2505dca1b07c/core/dbt/context/macros.py#L83
/// and https://github.com/dbt-labs/dbt-core/blob/4aa5169212d8256002095d44dc5f2505dca1b07c/core/dbt/context/macros.py#L34
///
pub fn macro_namespace_template_resolver(
    state: &State<'_, '_>,
    search_name: &str,
    attempts: &mut Vec<String>,
) -> Option<String> {
    macro_namespace_template_candidates(state, search_name, attempts)
        .into_iter()
        .next()
}

fn macro_namespace_template_candidates(
    state: &State<'_, '_>,
    search_name: &str,
    attempts: &mut Vec<String>,
) -> Vec<String> {
    // Get necessary values from state
    let current_package_name = state
        .ctx
        .load(state.env(), TARGET_PACKAGE_NAME)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "dbt".to_string());
    let root_package = state.env().get_root_package_name();
    // Internal-macro resolution is per-dialect: several adapters' packages may be
    // loaded at once and an unprefixed macro can be defined by more than one.
    let dialect = resolve_dialect(state);
    let dbt_and_adapters = state.env().get_dbt_and_adapters_namespace(&dialect);
    let mut candidates = Vec::new();

    // 1. Local namespace (current package)
    let template_name = format!("{current_package_name}.{search_name}");
    attempts.push(template_name.clone());
    if template_exists(state, &template_name) {
        candidates.push(template_name);
    }

    // 2. Root package namespace
    let template_name = format!("{root_package}.{search_name}");
    attempts.push(template_name.clone());
    if template_exists(state, &template_name) && !candidates.contains(&template_name) {
        candidates.push(template_name);
    }

    // 3. Internal packages
    let search_name_value = Value::from(search_name);
    if let Some(pkg) = dbt_and_adapters.get(&search_name_value) {
        let template_name = format!("{pkg}.{search_name}");
        attempts.push(template_name.clone());
        if template_exists(state, &template_name) && !candidates.contains(&template_name) {
            candidates.push(template_name);
        }
    }

    candidates
}

#[cfg(test)]
mod namespace_selection_tests {
    use super::*;

    /// Build a `dialect -> (macro_name -> package)` map like the environment
    /// builder produces.
    fn namespaces(entries: &[(&str, &[(&str, &str)])]) -> ValueMap {
        let mut outer = ValueMap::new();
        for (dialect, macros) in entries {
            let mut inner = ValueMap::new();
            for (macro_name, pkg) in *macros {
                inner.insert(Value::from(*macro_name), Value::from(*pkg));
            }
            outer.insert(Value::from(*dialect), Value::from_object(inner));
        }
        outer
    }

    fn lookup(ns: &ValueMap, dialect: &str, macro_name: &str) -> Option<String> {
        dbt_and_adapters_namespace_for(ns, dialect)
            .get(&Value::from(macro_name))
            .and_then(|v| v.as_str().map(str::to_string))
    }

    /// The whole point of the reshape: an unprefixed macro defined by two loaded
    /// adapters resolves per dialect instead of one silently winning.
    #[test]
    fn unprefixed_collisions_resolve_per_dialect() {
        let ns = namespaces(&[
            (
                "snowflake",
                &[("run_hooks", "dbt"), ("py_write_table", "dbt_snowflake")],
            ),
            (
                "duckdb",
                &[
                    ("run_hooks", "dbt_duckdb"),
                    ("py_write_table", "dbt_duckdb"),
                ],
            ),
        ]);

        assert_eq!(
            lookup(&ns, "snowflake", "run_hooks").as_deref(),
            Some("dbt")
        );
        assert_eq!(
            lookup(&ns, "duckdb", "run_hooks").as_deref(),
            Some("dbt_duckdb")
        );
        assert_eq!(
            lookup(&ns, "snowflake", "py_write_table").as_deref(),
            Some("dbt_snowflake")
        );
        assert_eq!(
            lookup(&ns, "duckdb", "py_write_table").as_deref(),
            Some("dbt_duckdb")
        );
    }

    /// An unknown dialect must not borrow another dialect's answer — that would
    /// reintroduce exactly the wrong-adapter resolution this keying prevents.
    #[test]
    fn unknown_dialect_yields_an_empty_namespace() {
        let ns = namespaces(&[("duckdb", &[("run_hooks", "dbt_duckdb")])]);

        assert!(dbt_and_adapters_namespace_for(&ns, "snowflake").is_empty());
        assert_eq!(lookup(&ns, "snowflake", "run_hooks"), None);
    }

    #[test]
    fn missing_namespace_map_is_tolerated() {
        let ns = ValueMap::new();
        assert!(dbt_and_adapters_namespace_for(&ns, "duckdb").is_empty());
    }
}

#[cfg(test)]
mod resolve_dialect_tests {
    use crate::constants::DIALECT;
    use crate::dispatch_object::resolve_dialect;
    use crate::{Environment, Value};

    /// With nothing in the render context, the environment global is used — the
    /// path every single-adapter node takes.
    #[test]
    fn falls_back_to_the_environment_global() {
        let mut env = Environment::new();
        env.add_global(DIALECT, Value::from("snowflake"));
        env.add_template("t", "").unwrap();

        let tmpl = env.get_template("t").unwrap();
        let state = tmpl.eval_to_state(Value::UNDEFINED, &[]).unwrap();

        assert_eq!(resolve_dialect(&state), "snowflake");
    }

    /// A per-node context value wins over the global. This is the mechanism that
    /// makes per-node dispatch work at all.
    #[test]
    fn context_value_overrides_the_global() {
        let mut env = Environment::new();
        env.add_global(DIALECT, Value::from("snowflake"));
        env.add_template("t", "").unwrap();

        let ctx = std::collections::BTreeMap::from([(DIALECT.to_string(), Value::from("duckdb"))]);
        let tmpl = env.get_template("t").unwrap();
        let state = tmpl
            .eval_to_state(Value::from_serialize(&ctx), &[])
            .unwrap();

        assert_eq!(
            resolve_dialect(&state),
            "duckdb",
            "the node's own dialect must win over the environment default"
        );
    }

    #[test]
    fn defaults_when_neither_is_set() {
        let mut env = Environment::new();
        env.add_template("t", "").unwrap();

        let tmpl = env.get_template("t").unwrap();
        let state = tmpl.eval_to_state(Value::UNDEFINED, &[]).unwrap();

        assert_eq!(resolve_dialect(&state), "postgres");
    }
}
