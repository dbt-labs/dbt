use crate::{functions::register_base_functions, jinja_environment::JinjaEnv};
use dbt_adapter::{Adapter, relation::is_parse_time_relation};
use dbt_common::{
    ErrorCode, FsError, FsResult, fs_err, io_args::IoArgs, unexpected_fs_err,
    warn_error_options::WarnErrorOptions,
};
use minijinja::{
    AdapterDispatchFunction, Argument, DynTypeObject, Environment, UndefinedFunctionType,
    UserDefinedFunctionType, Value,
    compiler::typecheck::FunctionRegistry,
    constants::{
        DBT_AND_ADAPTERS_NAMESPACE, MACRO_NAMESPACE_REGISTRY, MACRO_TEMPLATE_REGISTRY,
        NON_INTERNAL_PACKAGES, ROOT_PACKAGE_NAME,
    },
    dispatch_object::{get_internal_packages, get_internal_packages_for},
    funcsign_parser, load_builtins_with_namespace,
    macro_unit::MacroUnit,
    value::ValueMap,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{collections::BTreeMap, path::PathBuf};

type PackageName = String;

/// A wrapper struct that contains a map of macros.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MacroUnitsWrapper {
    /// A map of macros.
    pub macros: BTreeMap<PackageName, Vec<MacroUnit>>,
}

impl MacroUnitsWrapper {
    /// Create a new MacrosWrapper.
    pub fn new(macros: BTreeMap<PackageName, Vec<MacroUnit>>) -> Self {
        Self { macros }
    }
}

/// A builder struct that configures and returns a Minijinja Environment.
/// You can add additional fields and methods as needed.
// Default Jinja Env Behaves differently than Envirornment::new()
pub struct JinjaEnvBuilder {
    env: Environment<'static>,
    adapter: Option<Arc<Adapter>>,
    globals: BTreeMap<String, Value>,
    root_package: Option<String>,
    undefined_behavior: minijinja::UndefinedBehavior,
    io_args: IoArgs,
    warn_error_options: WarnErrorOptions,
    function_registry: Arc<FunctionRegistry>,
    /// Dialects whose internal macro packages this environment serves. Empty
    /// means "just the adapter's own", which is the single-adapter case.
    extra_dialects: Vec<String>,
}

impl JinjaEnvBuilder {
    /// Create a new JinjaEnvBuilder with a default Environment.
    pub fn new() -> Self {
        Self {
            env: Environment::new(),
            adapter: None,
            globals: BTreeMap::new(),
            root_package: None,
            undefined_behavior: Default::default(),
            io_args: IoArgs::default(),
            warn_error_options: WarnErrorOptions::default(),
            function_registry: Arc::new(FunctionRegistry::new()),
            extra_dialects: Vec::new(),
        }
    }

    /// Specify the undefined behavior for the environment.
    pub fn with_undefined_behavior(
        mut self,
        undefined_behavior: minijinja::UndefinedBehavior,
    ) -> Self {
        self.undefined_behavior = undefined_behavior;
        self
    }

    /// Specify the root package for the environment.
    pub fn with_root_package(mut self, root_package: String) -> Self {
        self.root_package = Some(root_package);
        self.env.add_global(
            ROOT_PACKAGE_NAME,
            Value::from(self.root_package.clone().unwrap()),
        );
        self
    }

    /// Specify an adapter type (e.g. "parse", "compile") or other distinguishing feature.
    pub fn with_adapter(mut self, adapter: Arc<Adapter>) -> Self {
        self.adapter = Some(adapter);
        self
    }

    /// Add a global to the environment.
    /// TODO: create a typed struct to validate we recieve all the globals we need
    pub fn with_globals(mut self, globals: BTreeMap<String, Value>) -> Self {
        self.globals = globals;
        self
    }

    /// Declare additional dialects this environment serves, beyond the
    /// adapter's own. A multi-adapter target supplies the other adapters' types
    /// so their internal packages are recognised as internal and each dialect
    /// gets its own macro namespace.
    pub fn with_extra_dialects(mut self, dialects: Vec<String>) -> Self {
        self.extra_dialects = dialects;
        self
    }

    /// Add IoArgs
    pub fn with_io_args(mut self, io_args: IoArgs) -> Self {
        self.io_args = io_args;
        self
    }

    /// Add WarnErrorOptions
    pub fn with_warn_error_options(mut self, warn_error_options: WarnErrorOptions) -> Self {
        self.warn_error_options = warn_error_options;
        self
    }

    /// Register macros with the environment.
    pub fn try_with_macros(mut self, mut macros: MacroUnitsWrapper) -> FsResult<Self> {
        let adapter = self.adapter.as_ref().ok_or_else(|| {
            unexpected_fs_err!("try_with_macros requires adapter configuration to be set")
        })?;

        // Replay-mode behavior: suppress Elementary artifact uploads to avoid non-semantic drift
        // (graph-derived metadata hashes, timestamped temp tables, etc.) that can cause replay
        // recordings to diverge despite no underlying semantic differences.
        //
        // We do this by rewriting the macro body for `elementary.upload_artifacts_to_table` to a
        // no-op. This avoids touching downstream macros like delete_and_insert/create_intermediate_relation
        // that create timestamped `__TMP_<...>` relations and trigger adapter calls not present in older
        // recordings.
        // `try_with_macros` is executed during the parse-phase environment build, which uses
        // a `ParseAdapter` even when the overall run is in replay mode. Therefore we cannot
        // rely on `adapter.as_replay()` here.
        //
        // Instead we key off the invocation args dict, which is built from `EvalArgs` and
        // includes whether replay mode is active.
        let is_replay = self
            .globals
            .get("invocation_args_dict")
            .and_then(|v| {
                // Check both lowercase (from invocation_args_dict after filtering) and uppercase
                v.get_item(&Value::from("replay"))
                    .ok()
                    .or_else(|| v.get_item(&Value::from("REPLAY")).ok())
            })
            .map(|v| v.is_true())
            .unwrap_or(false);

        if is_replay {
            const ELEMENTARY_PKG: &str = "elementary";
            const TARGET_MACRO: &str = "upload_artifacts_to_table";
            const NOOP_MACRO_SQL: &str = r#"{% macro upload_artifacts_to_table(table_relation, artifacts, flatten_artifact_callback, append=False, should_commit=False, metadata_hashes=None, on_query_exceed=none) %}
  {# dbt-fusion replay: deliberately no-op to avoid non-semantic drift and temp-table lookups #}
  {{- return('') -}}
{% endmacro %}
"#;

            if let Some(units) = macros.macros.get_mut(ELEMENTARY_PKG) {
                for unit in units.iter_mut() {
                    if unit.info.name == TARGET_MACRO {
                        unit.sql = NOOP_MACRO_SQL.to_string();
                        break;
                    }
                }
            }

            // Replay-mode behavior: suppress dbt_project_evaluator graph materialization post-hooks.
            //
            // dbt_project_evaluator's staging graph models populate tables via post-hook INSERTs
            // generated from `graph.*` (nodes/depends_on, etc.). Fusion and Mantle can have benign
            // graph differences, which results in non-semantic SQL drift and replay mismatches.
            //
            // We rewrite `dbt_project_evaluator.insert_resources_from_graph` to a no-op, similar to
            // the Elementary suppression above.
            const DBT_PROJECT_EVALUATOR_PKG: &str = "dbt_project_evaluator";
            const DBT_PROJECT_EVALUATOR_TARGET_MACRO: &str = "insert_resources_from_graph";
            const DBT_PROJECT_EVALUATOR_NOOP_MACRO_SQL: &str = r#"{% macro insert_resources_from_graph(relation, resource_type='nodes', relationships=False, columns=False, batch_size=var('insert_batch_size') | int) %}
  {# dbt-fusion replay: deliberately no-op to avoid graph-derived SQL drift #}
  {{- return('') -}}
{% endmacro %}
"#;

            if let Some(units) = macros.macros.get_mut(DBT_PROJECT_EVALUATOR_PKG) {
                for unit in units.iter_mut() {
                    if unit.info.name == DBT_PROJECT_EVALUATOR_TARGET_MACRO {
                        unit.sql = DBT_PROJECT_EVALUATOR_NOOP_MACRO_SQL.to_string();
                        break;
                    }
                }
            }
        }

        // Get the root package name
        let root_package = self
            .root_package
            .clone()
            .ok_or_else(|| unexpected_fs_err!("try_with_macros requires root package to be set"))?;

        // The dialects whose internal packages this environment serves. One today;
        // a multi-adapter target will supply several, at which point each gets its
        // own namespace below.
        let mut dialects: Vec<String> = vec![adapter.adapter_type().as_ref().to_string()];
        for dialect in &self.extra_dialects {
            if !dialects.contains(dialect) {
                dialects.push(dialect.clone());
            }
        }

        // Internal packages across every dialect served. Resolution stays
        // per-dialect (see the namespace build below), so widening this set does
        // not let one adapter's unprefixed macros leak into another's lookups.
        #[allow(unused_mut)]
        let mut internal_packages = get_internal_packages_for(dialects.iter().map(String::as_str));
        // Initialize all registries
        let mut macro_namespace_registry = ValueMap::new(); // package_name → {macro_name → true}
        let mut macro_template_registry = ValueMap::new(); // template_name → macro_info

        let mut non_internal_packages: ValueMap = ValueMap::new(); // package_name → [macro_names]
        let mut internal_packages_macros: BTreeMap<String, BTreeMap<String, Value>> =
            BTreeMap::new(); // package_name → {macro_name → info}
        let mut function_registry = FunctionRegistry::new(); // macro_name → DynObject

        // Process all macros
        for (package_name, macro_units) in macros.macros.clone() {
            // Add package to namespace registry.
            //
            // Stored as a map keyed by macro name rather than a sequence: the hot
            // consumer is `DbtNamespace::get_property`, which membership-tests this on
            // every `pkg.macro_name` access. A map makes that a hash lookup instead of a
            // scan over the whole package. Iterating a minijinja map yields its keys, so
            // consumers that walk this for macro names are unaffected.
            let macro_names: ValueMap = macro_units
                .iter()
                .map(|m| (Value::from(m.info.name.clone()), Value::from(true)))
                .collect();
            macro_namespace_registry.insert(
                Value::from(package_name.clone()),
                Value::from_object(macro_names),
            );
        }
        self.env.add_global(
            MACRO_NAMESPACE_REGISTRY,
            Value::from_object(macro_namespace_registry),
        );

        let macro_namespace_registry = self
            .env
            .get_macro_namespace_registry()
            .expect("macro namespace registry set above");
        // Load jinja typecheck builtins
        let builtins = load_builtins_with_namespace(Some(macro_namespace_registry.clone()))
            .map_err(|e| unexpected_fs_err!("Failed to load jinja typecheck builtins: {}", e))?;
        for (package_name, macro_units) in macros.macros {
            // Internal packages (dbt, dbt_postgres, etc.)
            let is_internal = internal_packages.contains(&package_name);

            // For non-internal packages, copy the entry from macro_namespace_registry
            // contains root and non-internal packages
            if !is_internal
                && let Some(macro_names) =
                    macro_namespace_registry.get(&Value::from(package_name.clone()))
            {
                non_internal_packages
                    .insert(Value::from(package_name.clone()), macro_names.clone());
            }
            for macro_unit in macro_units {
                let filename = macro_unit.info.path.to_string_lossy().to_string();
                let offset = dbt_frontend_common::error::CodeLocation::new(
                    macro_unit.info.span.start_line,
                    macro_unit.info.span.start_col,
                    macro_unit.info.span.start_offset,
                );
                let macro_name = macro_unit.info.name.clone();
                let template_name = format!("{package_name}.{macro_name}");

                // Add to environment and template registry
                self.env
                    .add_template_owned(
                        template_name.clone(),
                        macro_unit.sql.clone(),
                        Some(filename.clone()),
                    )
                    .map_err(|e| FsError::from_jinja_err(e, "Failed to add template"))?;

                let funcsign = match macro_unit.info.funcsign.clone() {
                    Some(funcsign) => {
                        let (args, returns) = funcsign_parser::parse(&funcsign, builtins.clone())
                            .map_err(|e| {
                            fs_err!(
                                ErrorCode::JinjaError,
                                "Failed to parse function signature: {}",
                                e
                            )
                        })?;
                        if args.len() != macro_unit.info.args.len() {
                            return Err(fs_err!(
                                ErrorCode::JinjaError,
                                "Function {} signature '{}' has {} args: {}, but macro has {} args: {}",
                                macro_name,
                                funcsign,
                                args.len(),
                                args.iter()
                                    .map(|a| a.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                macro_unit.info.args.len(),
                                macro_unit
                                    .info
                                    .args
                                    .iter()
                                    .map(|a| a.name.clone())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            ));
                        }
                        let args = macro_unit
                            .info
                            .args
                            .iter()
                            .zip(args.iter())
                            .map(|(arg_spec, arg_type)| Argument {
                                name: arg_spec.name.clone(),
                                type_: arg_type.clone(),
                                is_optional: arg_spec.is_optional,
                            })
                            .collect::<Vec<_>>();

                        DynTypeObject::new(Arc::new(UserDefinedFunctionType::new(
                            &macro_name,
                            args,
                            returns,
                            &macro_unit.info.path,
                            &macro_unit.info.name_span,
                            &macro_unit.info.unique_id,
                        )))
                    }
                    None => DynTypeObject::new(Arc::new(UndefinedFunctionType::new(
                        &macro_name,
                        minijinja::CodeLocation::new(
                            offset.line,
                            offset.col,
                            PathBuf::from(filename.clone()),
                        ),
                        &macro_unit.info.path,
                        &macro_unit.info.span,
                        &macro_unit.info.unique_id,
                    ))),
                };
                function_registry.insert(template_name.clone(), funcsign.clone());
                function_registry.insert(macro_name.clone(), funcsign.clone());

                macro_template_registry.insert(
                    Value::from(template_name),
                    Value::from_serialize(macro_unit.info.clone()),
                );

                if is_internal {
                    internal_packages_macros
                        .entry(package_name.clone())
                        .or_default()
                        .insert(
                            macro_name.clone(),
                            Value::from_serialize(macro_unit.info.clone()),
                        );
                }
            }
        }

        self.function_registry = Arc::new(function_registry);
        AdapterDispatchFunction::instance().set_function_registry(self.function_registry.clone());

        // One `macro_name -> package` namespace per dialect, keyed by dialect.
        //
        // Within a dialect the packages are walked in reverse order, as dbt does,
        // so a more specific adapter package overrides `dbt-adapters`. That
        // layering is load-bearing and preserved. Keying by dialect is what lets
        // two adapters that both define an unprefixed macro (`run_hooks`,
        // `py_write_table`) coexist without one silently winning for every node.
        let mut dbt_and_adapters_namespaces = ValueMap::new();
        for dialect in &dialects {
            let mut namespace = ValueMap::new();
            for pkg in get_internal_packages(dialect).iter().rev() {
                if let Some(pkg_macros) = internal_packages_macros.get(pkg) {
                    for macro_name in pkg_macros.keys() {
                        namespace.insert(Value::from(macro_name.clone()), Value::from(pkg.clone()));
                    }
                }
            }
            dbt_and_adapters_namespaces
                .insert(Value::from(dialect.clone()), Value::from_object(namespace));
        }

        // Ensure the root package is ALWAYS in non_internal_packages even if it has no macros
        non_internal_packages
            .entry(Value::from(root_package))
            .or_insert_with(|| Value::from_serialize(Vec::<String>::new()));

        // Add all registries to environment
        self.env.add_global(
            MACRO_TEMPLATE_REGISTRY,
            Value::from_object(macro_template_registry),
        );
        self.env.add_global(
            NON_INTERNAL_PACKAGES,
            Value::from_object(non_internal_packages),
        );
        self.env.add_global(
            DBT_AND_ADAPTERS_NAMESPACE,
            Value::from_object(dbt_and_adapters_namespaces),
        );

        // Register the dbt_classification macro package — embedded SQL
        // shipped with `dbt-classification` for the Snowflake tag
        // round-trip.  These macros are dialect-specific by content but
        // are registered unconditionally; they only execute when the
        // adapter-gated Phase 1 / Phase 3 callers in `dbt-tasks`
        // actually look them up.  See propagation_of_snowflake_tags.md §4.2.
        for (template_name, sql) in dbt_classification_types::propagation_macro_templates() {
            self.env
                .add_template_owned(template_name, sql, None)
                .map_err(|e| {
                    FsError::from_jinja_err(
                        e,
                        "Failed to register dbt_classification propagation macro",
                    )
                })?;
        }

        Ok(self)
    }

    /// Build the Minijinja Environment with all configured settings.
    pub fn build(mut self) -> JinjaEnv {
        // Register filters (as_bool, as_number, as_native, as_text)
        // These are used to convert values to the appropriate type that might be
        // expected by the jinja template.

        self.register_filters();

        // Register tests
        self.register_tests();

        // Register filter types in function_registry
        // This must be done before register_base_functions which consumes io_args
        let function_registry = self.register_filter_types();

        // Register "base" dbt style functions.
        register_base_functions(&mut self.env, self.warn_error_options);

        // Register all configured global values.
        // TODO (Ani) type the globals struct to validate we recieve all the globals we need
        for (key, val) in self.globals {
            self.env.add_global(key, val);
        }

        // Any extra steps (unknown method callback, etc.)
        minijinja_contrib::add_to_environment(&mut self.env);

        self.env.set_undefined_behavior(self.undefined_behavior);

        // Pull in the pycompat methods
        self.env
            .set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);

        let mut jinja_env = JinjaEnv::new(self.env);
        if let Some(adapter) = self.adapter {
            jinja_env.set_adapter(adapter);
        }
        jinja_env.jinja_function_registry = function_registry;

        jinja_env
    }

    /// Registers filter types in the function registry
    fn register_filter_types(&self) -> Arc<FunctionRegistry> {
        let mut registry = (*self.function_registry).clone();
        dbt_jinja_filters::register_filter_types(&mut registry);
        Arc::new(registry)
    }

    fn register_filters(&mut self) {
        dbt_jinja_filters::register_filters(&mut self.env);
    }

    fn register_tests(&mut self) {
        self.env.add_test("none", |value: Value| {
            value.is_none() || is_parse_time_relation(&value)
        });

        // https://github.com/pallets/jinja/blob/5ef70112a1ff19c05324ff889dd30405b1002044/src/jinja2/runtime.py#L878
        // Since `__call__` is technically implemented, {{ undefined is callable }} is true.
        self.env.add_test("callable", |value: Value| {
            value.is_undefined() || value.as_object().is_some()
        });
    }
}

impl Default for JinjaEnvBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, path::PathBuf, sync::Mutex};

    use dbt_adapter::Adapter;
    use dbt_adapter::sql_types::DefaultTypeOps;
    use dbt_adapter_core::AdapterType;
    use dbt_schemas::schemas::relations::DEFAULT_DBT_QUOTING;
    use minijinja::{
        constants::MACRO_DISPATCH_ORDER, context, dispatch_object::THREAD_LOCAL_DEPENDENCIES,
        machinery::Span, macro_unit::MacroInfo, value::ValueMap,
    };

    use super::*;
    use dbt_test_primitives::assert_contains;
    use insta::assert_snapshot;

    // THREAD_LOCAL_DEPENDENCIES is a global OnceCell. Multiple tests mutate it and Rust runs tests
    // in parallel by default, so we serialize those mutations to avoid cross-test races/flakes.
    static TEST_DEPS_LOCK: Mutex<()> = Mutex::new(());

    fn set_thread_local_dependencies(pkgs: impl IntoIterator<Item = String>) {
        let _guard = TEST_DEPS_LOCK.lock().unwrap();
        let deps = THREAD_LOCAL_DEPENDENCIES.get_or_init(|| Mutex::new(BTreeSet::new()));
        let mut deps = deps.lock().unwrap();
        deps.clear();
        deps.extend(pkgs);
    }

    fn create_macro_unit(name: &str, sql: &str) -> MacroUnit {
        MacroUnit {
            info: MacroInfo {
                name: name.to_string(),
                path: PathBuf::from("test"),
                span: Span {
                    start_line: 0,
                    start_col: 0,
                    start_offset: 0,
                    end_line: 0,
                    end_col: 0,
                    end_offset: 0,
                },
                funcsign: None,
                args: vec![],
                unique_id: "test".to_string(),
                name_span: Span::default(),
            },
            sql: sql.to_string(),
        }
    }

    fn invocation_args_dict(replay: bool) -> Value {
        Value::from_object(ValueMap::from([
            (Value::from("REPLAY".to_string()), Value::from(replay)),
            (Value::from("replay".to_string()), Value::from(replay)),
        ]))
    }

    #[test]
    fn test_filter_none() {
        let mut builder = JinjaEnvBuilder::new();
        builder.register_filters();
        let env = builder.build();
        let rv = env
            .render_str(
                r#"
    {%- set x = y | as_bool -%}
    {%- set x = y | as_number -%}
    all okay!
    "#,
                context! {},
                &[],
            )
            .unwrap();
        assert_snapshot!(rv, @r"

all okay!");
    }

    #[test]
    fn parse_get_relation_matches_none_control_flow() {
        let adapter = Arc::new(Adapter::new_parse_phase_adapter(
            AdapterType::DuckDB,
            dbt_yaml::Mapping::new(),
            DEFAULT_DBT_QUOTING,
            Arc::new(DefaultTypeOps::new(AdapterType::DuckDB)),
            None,
        ));
        let globals = BTreeMap::from([(
            "adapter".to_string(),
            Value::from_object(adapter.as_ref().clone()),
        )]);
        let env = JinjaEnvBuilder::new()
            .with_adapter(adapter)
            .with_globals(globals)
            .build();

        let rendered = env
            .render_str(
                r#"{%- set relation = adapter.get_relation(database="", schema="main", identifier="missing") -%}
{{- relation is none }}|{{ relation is not none }}|{{ relation and "truthy" or "falsey" }}|{{ relation.include(database=false, schema=false, identifier=false).render() is none -}}"#,
                context! {},
                &[],
            )
            .unwrap();

        assert_eq!(rendered, "True|False|falsey|True");
    }

    #[test]
    fn test_dispatch_mode() {
        set_thread_local_dependencies(std::iter::empty());
        let mut macro_units = MacroUnitsWrapper::new(BTreeMap::new());
        macro_units.macros.insert(
            "dbt".to_string(),
            vec![
                create_macro_unit(
                    "default__one",
                    "{% macro default__one() %}dbt default one{% endmacro %}",
                ),
                create_macro_unit(
                    "default__two",
                    "{% macro default__two() %}dbt default two{% endmacro %}",
                ),
                create_macro_unit(
                    "default__three",
                    "{% macro default__three() %}dbt default three{% endmacro %}",
                ),
            ],
        );
        macro_units.macros.insert(
            "dbt_postgres".to_string(),
            vec![
                create_macro_unit(
                    "postgres__one",
                    "{% macro postgres__one() %}postgres one{% endmacro %}",
                ),
                create_macro_unit(
                    "postgres__two",
                    "{% macro postgres__two() %}postgres two{% endmacro %}",
                ),
                create_macro_unit(
                    "postgres__some_macro",
                    "{% macro postgres__some_macro() %}some_macro in postgres{% endmacro %}",
                ),
            ],
        );
        macro_units.macros.insert(
            "a_package".to_string(),
            vec![
                create_macro_unit(
                    "postgres__some_macro",
                    "{% macro postgres__some_macro() %}some_macro in a_package{% endmacro %}",
                ),
                create_macro_unit(
                    "default__another_macro",
                    "{% macro default__another_macro() %}default another_macro{% endmacro %}",
                ),
            ],
        );
        macro_units.macros.insert(
            "test_package".to_string(),
            vec![create_macro_unit(
                "default__one",
                "{% macro default__one() %}test_package one{% endmacro %}",
            )],
        );
        let adapter = Adapter::new_parse_phase_adapter(
            AdapterType::Postgres,
            dbt_yaml::Mapping::default(),
            DEFAULT_DBT_QUOTING,
            Arc::new(DefaultTypeOps::new(AdapterType::Postgres)),
            None,
        );
        let builder: JinjaEnvBuilder = JinjaEnvBuilder::new()
            .with_adapter(Arc::new(adapter) as Arc<Adapter>)
            .with_root_package("test_package".to_string())
            .try_with_macros(macro_units)
            .expect("Failed to register macros");
        let env = builder.build();
        // one exists in test_package, dbt_postgres, and dbt
        let rv = env
            .render_str("{{adapter.dispatch('one', 'dbt')()}}", context! {}, &[])
            .unwrap();
        assert_snapshot!(rv, "test_package one");
        // two exists in dbt_postgres, and dbt
        let rv = env
            .render_str("{{adapter.dispatch('two', 'dbt')()}}", context! {}, &[])
            .unwrap();
        assert_snapshot!(rv, "postgres two");
        // three exists in dbt
        let rv = env
            .render_str("{{adapter.dispatch('three', 'dbt')()}}", context! {}, &[])
            .unwrap();
        assert_snapshot!(rv, "dbt default three");

        // one exists in test_package, dbt_postgres, and dbt, but package is not specified, so last one wins
        let rv = env
            .render_str("{{adapter.dispatch('one')()}}", context! {}, &[])
            .unwrap();
        assert_snapshot!(rv, "test_package one");

        // some_macro exists in a_package, and dbt_postgres, but package is not specified, so last one wins
        let rv = env
            .render_str("{{adapter.dispatch('some_macro')()}}", context! {}, &[])
            .unwrap();
        assert_snapshot!(rv, "some_macro in a_package");

        // some_macro exists in a_package, postgres, and dbt_postgres, but package is specified, so a_package wins
        let rv = env
            .render_str(
                "{{adapter.dispatch('some_macro', 'a_package')()}}",
                context! {},
                &[],
            )
            .unwrap();
        assert_snapshot!(rv, "some_macro in a_package");
    }
    #[test]
    fn test_dispatch_config() {
        let mut macro_units = MacroUnitsWrapper::new(BTreeMap::new());
        macro_units.macros.insert(
            "dbt".to_string(),
            vec![create_macro_unit(
                "default__some_macro",
                "{% macro default__some_macro() %}some_macro in dbt{% endmacro %}",
            )],
        );
        macro_units.macros.insert(
            "a_package".to_string(),
            vec![
                create_macro_unit(
                    "postgres__some_macro",
                    "{% macro postgres__some_macro() %}some_macro in a_package{% endmacro %}",
                ),
                create_macro_unit(
                    "default__some_macro",
                    "{% macro default__some_macro() %}default some_macro{% endmacro %}",
                ),
            ],
        );
        macro_units.macros.insert(
            "test_package".to_string(),
            vec![
                create_macro_unit(
                    "postgres__some_macro",
                    "{% macro postgres__some_macro() %}some_macro in test_package{% endmacro %}",
                ),
                create_macro_unit(
                    "default__some_macro",
                    "{% macro default__some_macro() %}default some_macro{% endmacro %}",
                ),
            ],
        );
        let adapter = Adapter::new_parse_phase_adapter(
            AdapterType::Postgres,
            dbt_yaml::Mapping::default(),
            DEFAULT_DBT_QUOTING,
            Arc::new(DefaultTypeOps::new(AdapterType::Postgres)),
            None,
        );
        let builder: JinjaEnvBuilder = JinjaEnvBuilder::new()
            .with_adapter(Arc::new(adapter) as Arc<Adapter>)
            .with_root_package("test_package".to_string())
            .try_with_macros(macro_units)
            .expect("Failed to register macros");
        let env = builder.build();

        // Set non-default dispatch order of a_package and dbt
        let ctx = BTreeMap::from([(
            MACRO_DISPATCH_ORDER,
            Value::from_object(ValueMap::from([
                (
                    Value::from("a_package".to_string()),
                    Value::from(vec!["test_package".to_string(), "a_package".to_string()]),
                ),
                (
                    Value::from("dbt".to_string()),
                    Value::from(vec!["dbt".to_string(), "test_package".to_string()]),
                ),
                (
                    Value::from("test_package".to_string()),
                    Value::from(vec!["b_package".to_string(), "test_package".to_string()]),
                ),
            ])),
        )]);

        // some_macro dispatched with a_package now resolves to test_package
        let rv = env
            .render_str(
                "{{adapter.dispatch('some_macro', 'a_package')()}}",
                &ctx,
                &[],
            )
            .unwrap();
        assert_eq!(rv, "some_macro in test_package");

        // some_macro dispatched with dbt now resolves to dbt
        let rv = env
            .render_str("{{adapter.dispatch('some_macro', 'dbt')()}}", &ctx, &[])
            .unwrap();
        assert_eq!(rv, "some_macro in dbt");

        // some_macro does not exist in b_package, so when dispatched with test_package still resolves to test_package
        let rv = env
            .render_str(
                "{{adapter.dispatch('some_macro', 'test_package')()}}",
                ctx,
                &[],
            )
            .unwrap();
        assert_snapshot!(rv, "some_macro in test_package");
    }

    #[test]
    fn test_macro_assignment() {
        let adapter = Adapter::new_parse_phase_adapter(
            AdapterType::Postgres,
            dbt_yaml::Mapping::default(),
            DEFAULT_DBT_QUOTING,
            Arc::new(DefaultTypeOps::new(AdapterType::Postgres)),
            None,
        );
        let env = JinjaEnvBuilder::new()
            .with_root_package("test_package".to_string())
            .with_adapter(Arc::new(adapter) as Arc<Adapter>)
            .try_with_macros(MacroUnitsWrapper::new(BTreeMap::from([(
                "test_package".to_string(),
                vec![
                    MacroUnit {
                        info: MacroInfo {
                            name: "some_macro".to_string(),
                            path: PathBuf::from("test"),
                            span: Span {
                                start_line: 1,
                                start_col: 1,
                                start_offset: 0,
                                end_line: 1,
                                end_col: 1,
                                end_offset: 0,
                            },
                            funcsign: None,
                            args: vec![],
                            unique_id: "test".to_string(),
                            name_span: Span::default(),
                        },
                        sql: "{% macro some_macro() %}hello{% endmacro %}".to_string(),
                    },
                    MacroUnit {
                        info: MacroInfo {
                            name: "macro_b".to_string(),
                            path: PathBuf::from("test"),
                            span: Span::default(),
                            funcsign: None,
                            args: vec![],
                            unique_id: "test".to_string(),
                            name_span: Span::default(),
                        },
                        sql: "{% macro macro_b() %}{%- set small_macro_name = some_macro -%} {{ small_macro_name() }}{% endmacro %}".to_string(),
                    },
                ],
            )]),
        ))
            .unwrap()
            .build();
        // Test assigning macro to variable and using it
        let rv = env.render_str("{{macro_b()}}", context! {}, &[]).unwrap();

        // The first print should show the macro object, second print shows the macro output
        // assert_contains!(rv, "<macro 'some_macro'>");
        assert_contains!(rv, "hello");
    }

    #[test]
    fn test_date_format() {
        let env = JinjaEnvBuilder::new().build();
        let rv = env
            .render_str(
                "{{modules.pytz.utc}} {{- modules.datetime.datetime.now(modules.pytz.utc).isoformat() -}}",
                context! {},
                &[],
            )
            .unwrap()
            ;
        assert_contains!(rv, "UTC");
        assert_contains!(rv, "+00:00");
    }
    #[test]
    fn test_datetime_strftime_with_timedelta() {
        let env = JinjaEnvBuilder::new().build();
        let rv = env
            .render_str(
                "
                {%- set today = modules.datetime.date.today() -%}
                {%- set now = modules.datetime.datetime.now().astimezone(modules.pytz.timezone('UTC')) -%}
                {{modules.datetime.datetime.strftime(today - modules.datetime.timedelta(days=1), '%Y-%m-%d')}}",
                context! {},
                &[],
            )
            .unwrap()
            ;
        // Extract the date from the rendered string and verify it's in the expected format
        let date_pattern = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap();
        assert!(
            date_pattern.is_match(rv.trim()),
            "Expected date in YYYY-MM-DD format, got: '{}'",
            rv.trim()
        );

        // Verify the date is yesterday by comparing with today's date
        let today = chrono::Local::now().date_naive();
        let yesterday = (today - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        assert_eq!(
            rv.trim(),
            yesterday,
            "Expected yesterday's date ({}), got: {}",
            yesterday,
            rv.trim()
        );
    }

    // from https://github.com/dbt-labs/scratch/tree/main/repros/fusion-789
    #[test]
    fn test_try_or_compiler_error_with_bound_method() {
        let env = JinjaEnvBuilder::new().build();
        let rv = env
            .render_str(
                "{%- set res = try_or_compiler_error('bad date', modules.datetime.datetime.strptime, '20240504', '%Y%m%d') -%}{{ res.strftime('%Y-%m-%d') }}",
                context! {},
                &[],
            )
            .unwrap();
        assert_eq!(rv.trim(), "2024-05-04");
    }

    #[test]
    fn test_try_or_compiler_error_raises_on_failure() {
        let env = JinjaEnvBuilder::new().build();
        let err = env
            .render_str(
                "{%- set res = try_or_compiler_error('partition date mismatch', modules.datetime.datetime.strptime, 'not-a-date', '%Y-%m-%d') -%}{{ res }}",
                context! {},
                &[],
            )
            .unwrap_err();
        assert_contains!(format!("{err}"), "partition date mismatch");
    }

    #[test]
    fn test_root_package_in_non_internal_packages() {
        // Create macro units with macros only in other packages, not in root
        let mut macro_units = MacroUnitsWrapper::new(BTreeMap::new());

        // Add macros to a non-root package
        macro_units.macros.insert(
            "other_package".to_string(),
            vec![create_macro_unit(
                "some_macro",
                "{% macro some_macro() %}some_macro in other_package{% endmacro %}",
            )],
        );

        let adapter = Adapter::new_parse_phase_adapter(
            AdapterType::Postgres,
            dbt_yaml::Mapping::default(),
            DEFAULT_DBT_QUOTING,
            Arc::new(DefaultTypeOps::new(AdapterType::Postgres)),
            None,
        );

        // Root package has no macros

        // Build environment with the empty root package
        let builder: JinjaEnvBuilder = JinjaEnvBuilder::new()
            .with_adapter(Arc::new(adapter) as Arc<Adapter>)
            .with_root_package("empty_root".to_string())
            .try_with_macros(macro_units)
            .expect("Failed to register macros");

        let env = builder.build();

        // Get the non_internal_packages registry
        let non_internal_packages = env.get_global(NON_INTERNAL_PACKAGES).unwrap();
        // Verify that empty_root is in the keys
        let keys: Vec<_> = non_internal_packages
            .as_object()
            .unwrap()
            .try_iter_pairs()
            .unwrap()
            .map(|(k, _)| k.to_string())
            .collect();

        assert!(
            keys.contains(&"empty_root".to_string()),
            "Root package should be in non_internal_packages even with no macros"
        );
    }

    #[test]
    fn test_elementary_upload_artifacts_to_table_is_noop_in_replay() {
        // In replay mode we rewrite `elementary.upload_artifacts_to_table` to a no-op during macro
        // registration to avoid non-semantic package side effects (e.g. timestamped temp tables).
        set_thread_local_dependencies(["test_package".to_string(), "elementary".to_string()]);

        let mut macro_units = MacroUnitsWrapper::new(BTreeMap::new());
        macro_units.macros.insert(
            "elementary".to_string(),
            vec![create_macro_unit(
                "upload_artifacts_to_table",
                "{% macro upload_artifacts_to_table(table_relation, artifacts, flatten_artifact_callback, append=False, should_commit=False, metadata_hashes=None, on_query_exceed=none) %}SHOULD_NOT_HAPPEN{% endmacro %}",
            )],
        );

        let adapter = Adapter::new_parse_phase_adapter(
            AdapterType::Postgres,
            dbt_yaml::Mapping::default(),
            DEFAULT_DBT_QUOTING,
            Arc::new(DefaultTypeOps::new(AdapterType::Postgres)),
            None,
        );

        let globals = BTreeMap::from([(
            "invocation_args_dict".to_string(),
            invocation_args_dict(true),
        )]);

        let env = JinjaEnvBuilder::new()
            .with_adapter(Arc::new(adapter) as Arc<Adapter>)
            .with_root_package("test_package".to_string())
            .with_globals(globals)
            .try_with_macros(macro_units)
            .expect("Failed to register macros")
            .build();

        let template = r#"
{% from 'elementary.upload_artifacts_to_table' import upload_artifacts_to_table %}
{{ upload_artifacts_to_table('rel', [], none) }}
"#;

        let rv = env.render_str(template, context! {}, &[]).unwrap();
        assert_eq!(rv.trim(), "");
    }

    #[test]
    fn test_elementary_upload_artifacts_to_table_unchanged_outside_replay() {
        // Outside replay, we should preserve the package-provided macro body. This is the control
        // case for the replay-only rewrite above.
        set_thread_local_dependencies(["test_package".to_string(), "elementary".to_string()]);

        let mut macro_units = MacroUnitsWrapper::new(BTreeMap::new());
        macro_units.macros.insert(
            "elementary".to_string(),
            vec![create_macro_unit(
                "upload_artifacts_to_table",
                "{% macro upload_artifacts_to_table(table_relation, artifacts, flatten_artifact_callback, append=False, should_commit=False, metadata_hashes=None, on_query_exceed=none) %}SHOULD_NOT_HAPPEN{% endmacro %}",
            )],
        );

        let adapter = Adapter::new_parse_phase_adapter(
            AdapterType::Postgres,
            dbt_yaml::Mapping::default(),
            DEFAULT_DBT_QUOTING,
            Arc::new(DefaultTypeOps::new(AdapterType::Postgres)),
            None,
        );

        let globals = BTreeMap::from([(
            "invocation_args_dict".to_string(),
            invocation_args_dict(false),
        )]);

        let env = JinjaEnvBuilder::new()
            .with_adapter(Arc::new(adapter) as Arc<Adapter>)
            .with_root_package("test_package".to_string())
            .with_globals(globals)
            .try_with_macros(macro_units)
            .expect("Failed to register macros")
            .build();

        let template = r#"
{% from 'elementary.upload_artifacts_to_table' import upload_artifacts_to_table %}
{{ upload_artifacts_to_table('rel', [], none) }}
"#;

        let rv = env.render_str(template, context! {}, &[]).unwrap();
        assert_eq!(rv.trim(), "SHOULD_NOT_HAPPEN");
    }

    /// Regression test for the incremental-parse dispatch bug.
    ///
    /// On the incremental code path `drop_all_unchanged_nodes` removes external packages
    /// (e.g. `dbt_utils`) from `dbt_state.packages` because none of their files changed.
    /// If `THREAD_LOCAL_DEPENDENCIES` is populated from `dbt_state.packages` (the old code),
    /// `dbt_utils` is absent and `adapter.dispatch('generate_surrogate_key', 'dbt_utils')`
    /// returns `vec![None]` → dispatch searches globally without namespace → macro not found.
    ///
    /// The fix: derive the dependency set from `macros.macros.values().map(|m| m.package_name)`
    /// — the macros are always fully populated (from cache) even when `dbt_state.packages` is
    /// filtered, and package names are the correct namespace identifiers (not unique IDs).
    #[test]
    fn test_incremental_parse_dispatch_finds_external_package_macros() {
        let mut macro_units = MacroUnitsWrapper::new(BTreeMap::new());
        // External package macro (e.g. installed via packages.yml, files unchanged this run)
        macro_units.macros.insert(
            "dbt_utils".to_string(),
            vec![create_macro_unit(
                "default__generate_surrogate_key",
                "{% macro default__generate_surrogate_key(field_list) %}surrogate_key_result{% endmacro %}",
            )],
        );
        // Root package macro
        macro_units.macros.insert(
            "my_project".to_string(),
            vec![create_macro_unit(
                "my_macro",
                "{% macro my_macro() %}root{% endmacro %}",
            )],
        );

        let adapter = Adapter::new_parse_phase_adapter(
            AdapterType::Postgres,
            dbt_yaml::Mapping::default(),
            DEFAULT_DBT_QUOTING,
            Arc::new(DefaultTypeOps::new(AdapterType::Postgres)),
            None,
        );

        let build_env = |pkgs: &[&str]| {
            set_thread_local_dependencies(pkgs.iter().map(|s| (*s).to_string()));
            JinjaEnvBuilder::new()
                .with_adapter(Arc::new(adapter.clone()) as Arc<Adapter>)
                .with_root_package("my_project".to_string())
                .try_with_macros(macro_units.clone())
                .expect("Failed to register macros")
                .build()
        };

        // BUG: THREAD_LOCAL_DEPENDENCIES only contains the root package (incremental parse
        // filtered dbt_utils out of dbt_state.packages). Dispatch to dbt_utils namespace
        // falls back to vec![None] and fails to find the macro.
        let env_broken = build_env(&["my_project"]);
        let result = env_broken.render_str(
            "{{ adapter.dispatch('generate_surrogate_key', 'dbt_utils')(['id']) }}",
            context! {},
            &[],
        );
        assert!(
            result.is_err(),
            "Expected dispatch to fail when dbt_utils is absent from THREAD_LOCAL_DEPENDENCIES"
        );

        // FIX: THREAD_LOCAL_DEPENDENCIES is derived from macros.macros.values().map(package_name),
        // which always includes all packages whose macros are registered — regardless of whether
        // their files were in the delta parse. Dispatch now resolves correctly.
        let env_fixed = build_env(&["my_project", "dbt_utils"]);
        let rv = env_fixed
            .render_str(
                "{{ adapter.dispatch('generate_surrogate_key', 'dbt_utils')(['id']) }}",
                context! {},
                &[],
            )
            .expect("Dispatch should succeed when dbt_utils is in THREAD_LOCAL_DEPENDENCIES");
        assert_eq!(rv.trim(), "surrogate_key_result");
    }
}
