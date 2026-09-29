//! Test harness for dbt macro unit tests.
//!
//! Provides scaffolding to declare macro tests with minimal boilerplate.
//! Every harness has a [`MockJinjaObject`] injected as the `adapter` global,
//! pre-configured with `dispatch` and `execute` handlers. This is done as
//! a utility as macros tend to very frequently call into adapter APIs.
//!
//! Other Jinja objects that are used in the context can be mocked out
//! by mutating the environment and inserting a customized [`MockJinjaObject`]
//!
//! # Quick Start
//!
//! ```ignore
//! mod macro_test_harness;
//! use macro_test_harness::MacroTestHarness;
//!
//! // Simple render test
//! let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
//!     .with_macro("dbt_databricks", "my_macro",
//!         r#"{% macro my_macro() %}hello{% endmacro %}"#)
//!     .build()
//!     .unwrap();
//!
//! let rendered = harness.render("{{ my_macro() }}", BTreeMap::new()).unwrap();
//! assert_eq!(rendered.trim(), "hello");
//! ```
//!
//! ```ignore
//! // Materialization test
//! let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
//!     .load_all_macros()
//!     .with_stub_functions()
//!     .build()
//!     .unwrap();
//!
//! harness.mock().on("get_relation", |_| Ok(Value::UNDEFINED));
//! let ctx = harness.materialization_context("my_view", "SELECT 1").build();
//! let rendered = harness.render("{{ materialization_view_snowflake() }}", ctx).unwrap();
//! harness.mock().observed_calls().assert_called("execute");
//! ```

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use dbt_adapter::Adapter;
use dbt_adapter::relation::{RelationObject, create_relation};
use dbt_adapter::sql_types::DefaultTypeOps;
use dbt_adapter::stmt_splitter::{DefaultStmtSplitter, StmtSplitter};
use dbt_adapter_core::AdapterType;
use dbt_common::FsResult;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_jinja_utils::{JinjaEnvBuilder, MacroUnitsWrapper};
use dbt_loader::construct_internal_packages;
use dbt_parser::resolve::resolve_macros::resolve_macros;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::macros::build_macro_units;
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::relations::{
    DEFAULT_RESOLVED_QUOTING, SNOWFLAKE_RESOLVED_QUOTING, default_dbt_quoting_for,
};
use dbt_schemas::state::DbtAsset;
use minijinja::Value;
use minijinja::dispatch_object::{DispatchObject, THREAD_LOCAL_DEPENDENCIES};
use minijinja::machinery::Span;
use minijinja::macro_unit::{MacroInfo, MacroUnit};
use serde::Serialize;

static TEST_DEPS_LOCK: Mutex<()> = Mutex::new(());

fn set_thread_local_dependencies(pkgs: impl IntoIterator<Item = impl Into<String>>) {
    let _guard = TEST_DEPS_LOCK.lock().unwrap();
    let deps = THREAD_LOCAL_DEPENDENCIES.get_or_init(|| Mutex::new(BTreeSet::new()));
    let mut deps = deps.lock().unwrap();
    deps.clear();
    deps.extend(pkgs.into_iter().map(Into::into));
}

// ---------------------------------------------------------------------------
// Public helpers
// ---------------------------------------------------------------------------

/// Create a [`MacroUnit`] for inline test macro definitions.
pub fn macro_unit(name: &str, sql: &str, path: &str) -> MacroUnit {
    MacroUnit {
        info: MacroInfo {
            name: name.to_string(),
            path: PathBuf::from(path),
            span: Span::default(),
            funcsign: None,
            args: vec![],
            unique_id: "test".to_string(),
            name_span: Span::default(),
        },
        sql: sql.to_string(),
    }
}

/// Create a [`MockJinjaObject`] pre-configured with structural adapter defaults.
///
/// Pre-registered handlers:
/// - `dispatch` → returns a [`DispatchObject`] for macro resolution
/// - `execute`  → returns `("SUCCESS", [])` tuple
pub fn default_mock_adapter() -> Arc<MockJinjaObject> {
    let mock = Arc::new(MockJinjaObject::new());

    mock.on("dispatch", |args| {
        let macro_name = args
            .first()
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let macro_namespace = args.get(1).and_then(|v| v.as_str()).map(String::from);
        Ok(Value::from_object(DispatchObject {
            macro_name,
            package_name: macro_namespace,
            strict: false,
            auto_execute: false,
            context: None,
        }))
    });

    mock.on("execute", |_args| {
        Ok(Value::from(vec![
            Value::from("SUCCESS"),
            Value::from_serialize(Vec::<Vec<String>>::new()),
        ]))
    });

    mock
}

/// Create a [`MockJinjaObject`] pre-configured as a dbt `config` object.
///
/// Pre-registered handlers:
/// - `get("contract")` → `{"enforced": false}`
/// - `get(<other>)`    → returns the caller-provided default argument
/// - `persist_column_docs` → `false`
/// - `persist_relation_docs` → `false`
pub fn default_mock_config() -> Arc<MockJinjaObject> {
    let mock = Arc::new(MockJinjaObject::new());

    mock.on("get", |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match key {
            Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                "enforced".to_string(),
                Value::from(false),
            )]))),
            _ => Ok(default),
        }
    });

    mock.on("persist_column_docs", |_args| Ok(Value::from(false)));
    mock.on("persist_relation_docs", |_args| Ok(Value::from(false)));
    mock.set_attr("model", Value::UNDEFINED);

    mock
}

/// The default [`ResolvedQuoting`] for a given adapter type.
pub fn resolved_quoting_for(adapter_type: AdapterType) -> ResolvedQuoting {
    match adapter_type {
        AdapterType::Snowflake => SNOWFLAKE_RESOLVED_QUOTING,
        _ => DEFAULT_RESOLVED_QUOTING,
    }
}

/// Collect all SQL strings passed to `adapter.execute()`.
pub fn executed_sql(mock: &MockJinjaObject) -> Vec<String> {
    mock.observed_calls()
        .to("execute")
        .filter_map(|c| c.args.first().and_then(|v| v.as_str().map(String::from)))
        .collect()
}

/// Assert that at least one SQL sent to `adapter.execute()` contains the given
/// substring (case insensitive).
pub fn assert_executed_contains(mock: &MockJinjaObject, substring: &str) {
    let sqls = executed_sql(mock);
    assert!(
        sqls.iter()
            .any(|s| s.to_lowercase().contains(&substring.to_lowercase())),
        "Expected at least one SQL containing '{substring}', got: {sqls:?}",
    );
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Resolve and load all internal macros for a set of adapter types.
///
/// The union over each adapter's inheritance chain, matching what a run with a
/// multi-adapter target loads.
fn load_all_macros_for(adapter_types: &[AdapterType]) -> MacroUnitsWrapper {
    let synthetic_root = Path::new("/tmp/synthetic");
    let packages = construct_internal_packages(adapter_types.iter().copied(), synthetic_root)
        .expect("construct_internal_packages");

    let mut all_macros = BTreeMap::new();
    for pkg in &packages {
        let macro_files: Vec<&DbtAsset> = pkg.macro_files.iter().collect();
        let resolved = resolve_macros(&macro_files, pkg.embedded_file_contents.as_ref())
            .expect("resolve_macros");
        all_macros.extend(resolved);
    }

    MacroUnitsWrapper::new(build_macro_units(&all_macros, synthetic_root))
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A built macro test environment ready for rendering and assertions.
pub struct MacroTestHarness {
    adapter_type: AdapterType,
    root_package: String,
    env: JinjaEnv,
    mock: Arc<MockJinjaObject>,
    store_result_calls: Arc<Mutex<Vec<String>>>,
}

impl MacroTestHarness {
    /// Start building a harness for the given adapter type.
    pub fn for_adapter(adapter_type: AdapterType) -> MacroTestHarnessBuilder {
        MacroTestHarnessBuilder {
            adapter_type,
            extra_adapters: vec![],
            root_package: "test_project".to_string(),
            macros: MacroUnitsWrapper::new(BTreeMap::new()),
            load_all: false,
            extra_deps: vec![],
            stub_functions: false,
            globals: BTreeMap::new(),
            behavior_flags: BTreeMap::new(),
        }
    }

    /// Render a Jinja template string with the given context.
    pub fn render<S: Serialize>(&self, template: &str, ctx: S) -> FsResult<String> {
        self.env.render_str(template, ctx, &[])
    }

    /// Render as a node that selected `adapter_type` with `+adapter`.
    ///
    /// Injects `dialect` into the render context, which is exactly what
    /// `compile_node_context` does for a node with a non-default `+adapter`.
    /// `resolve_dialect` checks the context frames before the environment
    /// global, so this selects that adapter's macro namespace and dispatch
    /// prefixes for this render only.
    pub fn render_for(
        &self,
        adapter_type: AdapterType,
        template: &str,
        ctx: BTreeMap<String, Value>,
    ) -> FsResult<String> {
        let mut ctx = ctx;
        ctx.insert(
            minijinja::constants::DIALECT.to_string(),
            // `as_ref()`, not `to_string()`: the namespace is keyed with `as_ref()`.
            Value::from(adapter_type.as_ref()),
        );
        self.env.render_str(template, ctx, &[])
    }

    /// Access the mock adapter.
    pub fn mock(&self) -> &Arc<MockJinjaObject> {
        &self.mock
    }

    /// Names passed to `store_result(name, ...)`, in call order. Only populated when
    /// [`MacroTestHarnessBuilder::with_stub_functions`] registered the stub that records them.
    pub fn store_result_calls(&self) -> Vec<String> {
        self.store_result_calls.lock().unwrap().clone()
    }

    /// Create a relation appropriate for this adapter's type.
    pub fn relation(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
        relation_type: Option<RelationType>,
    ) -> Arc<dyn BaseRelation> {
        Arc::from(
            create_relation(
                self.adapter_type,
                database.to_string(),
                schema.to_string(),
                Some(identifier.to_string()),
                relation_type,
                resolved_quoting_for(self.adapter_type),
            )
            .expect("create_relation"),
        )
    }

    /// Start building a standard materialization context for this adapter.
    ///
    /// Returns a [`MaterializationContextBuilder`] pre-populated with defaults
    /// that can be overridden before calling `.build()`.
    pub fn materialization_context(
        &self,
        model_alias: &str,
        sql: &str,
    ) -> MaterializationContextBuilder {
        MaterializationContextBuilder {
            adapter_type: self.adapter_type,
            root_package: self.root_package.clone(),
            model_alias: model_alias.to_string(),
            database: "TEST_DB".to_string(),
            schema: "TEST_SCHEMA".to_string(),
            sql: sql.to_string(),
            relation_type: Some(RelationType::View),
            config: Value::from_dyn_object(default_mock_config()),
            extras: BTreeMap::new(),
        }
    }

    /// Set adapter behavior flags (e.g. `use_materialization_v2`).
    ///
    /// These are exposed in Jinja as `adapter.behavior.<flag>`. Replaces
    /// all previously set behavior flags.
    pub fn set_behavior_flags(&self, flags: impl IntoIterator<Item = (&'static str, bool)>) {
        let map: BTreeMap<&str, bool> = flags.into_iter().collect();
        self.mock().set_attr("behavior", Value::from_serialize(map));
    }

    /// Access the underlying Jinja environment for additional inspection.
    pub fn env(&self) -> &JinjaEnv {
        &self.env
    }

    /// Mutable access to the Jinja environment for advanced customization
    /// after build (e.g. adding extra globals or functions).
    pub fn env_mut(&mut self) -> &mut JinjaEnv {
        &mut self.env
    }
}

// ---------------------------------------------------------------------------
// Materialization context builder
// ---------------------------------------------------------------------------

/// Builder for the `BTreeMap<String, Value>` context passed to materialization
/// macro renders.
pub struct MaterializationContextBuilder {
    adapter_type: AdapterType,
    root_package: String,
    model_alias: String,
    database: String,
    schema: String,
    sql: String,
    relation_type: Option<RelationType>,
    config: Value,
    extras: BTreeMap<String, Value>,
}

impl MaterializationContextBuilder {
    pub fn database(mut self, database: &str) -> Self {
        self.database = database.to_string();
        self
    }

    pub fn schema(mut self, schema: &str) -> Self {
        self.schema = schema.to_string();
        self
    }

    /// Override the relation type used for `this` (default: `View`).
    pub fn relation_type(mut self, rt: RelationType) -> Self {
        self.relation_type = Some(rt);
        self
    }

    /// Override the `config` context variable with a custom value.
    pub fn config(mut self, config: Value) -> Self {
        self.config = config;
        self
    }

    /// Insert an arbitrary key-value pair into the context.
    pub fn with(mut self, key: &str, value: Value) -> Self {
        self.extras.insert(key.to_string(), value);
        self
    }

    /// Build the context map.
    pub fn build(self) -> BTreeMap<String, Value> {
        let unique_id = format!("model.{}.{}", self.root_package, self.model_alias);
        let this: Arc<dyn BaseRelation> = Arc::from(
            create_relation(
                self.adapter_type,
                self.database.clone(),
                self.schema.clone(),
                Some(self.model_alias.clone()),
                self.relation_type,
                resolved_quoting_for(self.adapter_type),
            )
            .expect("create_relation"),
        );
        let empty_hooks: Vec<Value> = vec![];

        let mut ctx = BTreeMap::from([
            (
                "model".to_string(),
                Value::from_serialize(BTreeMap::from([
                    ("alias", Value::from(&self.model_alias)),
                    ("unique_id", Value::from(&unique_id)),
                    ("columns", Value::from(BTreeMap::<String, Value>::new())),
                ])),
            ),
            ("database".to_string(), Value::from(&self.database)),
            ("schema".to_string(), Value::from(&self.schema)),
            ("sql".to_string(), Value::from(&self.sql)),
            ("compiled_code".to_string(), Value::from(&self.sql)),
            ("pre_hooks".to_string(), Value::from_serialize(&empty_hooks)),
            (
                "post_hooks".to_string(),
                Value::from_serialize(&empty_hooks),
            ),
            ("execute".to_string(), Value::from(true)),
            ("config".to_string(), self.config),
            ("this".to_string(), RelationObject::new(this).into_value()),
        ]);

        ctx.extend(self.extras);
        ctx
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for [`MacroTestHarness`].
pub struct MacroTestHarnessBuilder {
    adapter_type: AdapterType,
    /// Adapter types this environment serves beyond `adapter_type`, as a
    /// multi-adapter target does. Their internal packages are loaded and each
    /// gets its own macro namespace.
    extra_adapters: Vec<AdapterType>,
    root_package: String,
    macros: MacroUnitsWrapper,
    load_all: bool,
    extra_deps: Vec<String>,
    stub_functions: bool,
    globals: BTreeMap<String, Value>,
    behavior_flags: BTreeMap<&'static str, bool>,
}

impl MacroTestHarnessBuilder {
    /// Override the root package name (default: `"test_project"`).
    pub fn with_root_package(mut self, name: impl Into<String>) -> Self {
        self.root_package = name.into();
        self
    }

    /// Load ALL internal macros for this adapter type via
    /// `construct_internal_packages` + `resolve_macros`.
    ///
    /// This gives the test access to every macro the adapter ships with.
    pub fn load_all_macros(mut self) -> Self {
        self.load_all = true;
        self
    }

    /// Register a single macro under a package namespace.
    ///
    /// The macro path is auto-generated as `inline://{package}/{name}.sql`.
    pub fn with_macro(mut self, package: &str, name: &str, sql: &str) -> Self {
        let path = format!("inline://{package}/{name}.sql");
        self.macros
            .macros
            .entry(package.to_string())
            .or_default()
            .push(macro_unit(name, sql, &path));
        self
    }

    /// Register a macro with an explicit asset path (useful for `include_str!`
    /// macros where the path should reflect the real file location).
    pub fn with_macro_at_path(mut self, package: &str, name: &str, sql: &str, path: &str) -> Self {
        self.macros
            .macros
            .entry(package.to_string())
            .or_default()
            .push(macro_unit(name, sql, path));
        self
    }

    /// Declare additional adapters this environment serves, as a target with an
    /// adapter list does.
    ///
    /// Without this, a harness built with `for_adapter(AdapterType::LakeCompute)` alone
    /// measures lake-compute-as-sole-adapter and proves nothing about a mixed graph.
    /// Combine with [`MacroTestHarness::render_for`] to render a node against
    /// one of the declared adapters.
    pub fn with_extra_adapters(
        mut self,
        adapter_types: impl IntoIterator<Item = AdapterType>,
    ) -> Self {
        self.extra_adapters.extend(adapter_types);
        self
    }

    /// Add extra thread-local dependency package names for dispatch resolution.
    ///
    /// The adapter's internal packages (`dbt`, `dbt_<adapter>`, etc.) are
    /// always included automatically.
    pub fn with_deps(mut self, deps: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.extra_deps.extend(deps.into_iter().map(Into::into));
        self
    }

    /// Register Jinja stub functions (`write`, `log`, `store_result`, `load_result`)
    /// typically needed by materialization macros.
    pub fn with_stub_functions(mut self) -> Self {
        self.stub_functions = true;
        self
    }

    /// Set an adapter behavior flag (e.g. `use_materialization_v2`).
    ///
    /// Flags are applied as the `adapter.behavior` attribute during
    /// [`build`](Self::build).
    pub fn with_behavior_flag(mut self, name: &'static str, value: bool) -> Self {
        self.behavior_flags.insert(name, value);
        self
    }

    /// Add a Jinja global variable.
    pub fn with_global(mut self, name: impl Into<String>, value: Value) -> Self {
        self.globals.insert(name.into(), value);
        self
    }

    /// Build the harness. Fails if macro registration encounters errors.
    pub fn build(self) -> FsResult<MacroTestHarness> {
        // The adapter's own type first, then any extra ones, deduped -- the same
        // order a run derives from `DbtProfile::adapter_types()`.
        let mut adapter_types = vec![self.adapter_type];
        for extra in &self.extra_adapters {
            if !adapter_types.contains(extra) {
                adapter_types.push(*extra);
            }
        }
        let dialects: Vec<String> = adapter_types
            .iter()
            .map(|t| t.as_ref().to_string())
            .collect();

        let internal_deps = minijinja::dispatch_object::get_internal_packages_for(
            dialects.iter().map(String::as_str),
        );
        let all_deps: Vec<String> = internal_deps.into_iter().chain(self.extra_deps).collect();
        set_thread_local_dependencies(all_deps);

        let mut macros = if self.load_all {
            load_all_macros_for(&adapter_types)
        } else {
            MacroUnitsWrapper::new(BTreeMap::new())
        };

        for (pkg, units) in self.macros.macros {
            macros.macros.entry(pkg).or_default().extend(units);
        }

        let quoting = default_dbt_quoting_for(self.adapter_type);
        let adapter: Arc<Adapter> = Arc::new(Adapter::new_parse_phase_adapter(
            self.adapter_type,
            dbt_yaml::Mapping::default(),
            quoting,
            Arc::new(DefaultTypeOps::new(self.adapter_type)),
            None,
        ));

        let builder = JinjaEnvBuilder::new()
            .with_adapter(adapter)
            .with_root_package(self.root_package.clone())
            // Skip the adapter's own dialect: the builder always includes it.
            .with_extra_dialects(dialects.iter().skip(1).cloned().collect())
            .try_with_macros(macros)?;

        let mut env = builder.build();

        let store_result_calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        if self.stub_functions {
            env.env
                .add_function("write", |_val: Value| Ok(Value::UNDEFINED));
            env.env
                .add_function("log", |_msg: Value| Ok(Value::UNDEFINED));
            let recorded_store_results = store_result_calls.clone();
            env.env.add_function(
                "store_result",
                move |name: Value, _kwargs: minijinja::value::Kwargs| {
                    recorded_store_results
                        .lock()
                        .unwrap()
                        .push(name.to_string());
                    Ok(Value::UNDEFINED)
                },
            );
            env.env.add_function("load_result", |_name: Value| {
                Ok(Value::from_serialize(BTreeMap::from([(
                    "table",
                    Vec::<Vec<Value>>::new(),
                )])))
            });
        }

        for (key, value) in self.globals {
            env.env.add_global(key, value);
        }

        let mock = default_mock_adapter();
        let adapter_type_str = self.adapter_type.as_ref().to_string();
        mock.on("type", move |_| Ok(Value::from(adapter_type_str.clone())));
        // Stands in for `Adapter::strip_trailing_statement_terminator`, running
        // the same dialect-aware normalization so macros that wrap a node body
        // see production behavior. The binding itself is covered by
        // `dbt-adapter`'s own tests, not here.
        let adapter_type = self.adapter_type;
        mock.on("strip_trailing_statement_terminator", move |args| {
            let sql = args.first().and_then(|v| v.as_str()).unwrap_or("");
            Ok(Value::from(
                DefaultStmtSplitter.strip_trailing_statement_terminator(sql, adapter_type),
            ))
        });
        if !self.behavior_flags.is_empty() {
            mock.set_attr("behavior", Value::from_serialize(&self.behavior_flags));
        }
        env.env
            .add_global("adapter", Value::from_dyn_object(mock.clone()));

        Ok(MacroTestHarness {
            adapter_type: self.adapter_type,
            root_package: self.root_package,
            env,
            mock,
            store_result_calls,
        })
    }
}
