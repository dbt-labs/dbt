use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use dbt_adapter::load_store::ResultStore;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_ctx::{
    DbtNamespace, DocsContext, JinjaObject, MacroLookupContext, ResolveBaseCtx, to_jinja_btreemap,
};
use dbt_jinja_vars::ConfiguredVar;
use dbt_schemas::schemas::macros::DbtDocsMacro;
use dbt_schemas::state::DbtRuntimeConfig;
use minijinja::value::{Enumerator, Object, Value as MinijinjaValue};

use crate::{functions::DocMacro, jinja_environment::JinjaEnv};

/// Backs the `context` key, which dbt Core sets to the context dict itself.
/// A `BTreeMap` cannot hold itself, so `context.context` is elided when printed.
#[derive(Debug)]
struct RecursiveDocsContext {
    values: BTreeMap<String, MinijinjaValue>,
}

impl Object for RecursiveDocsContext {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        let key = key.as_str()?;
        if key == "context" {
            Some(MinijinjaValue::from_dyn_object(self.clone()))
        } else {
            self.values.get(key).cloned()
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Values(
            self.values
                .keys()
                .map(|key| MinijinjaValue::from(key.clone()))
                .collect(),
        )
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut map = f.debug_map();
        for (key, value) in &self.values {
            if key == "context" {
                map.entry(key, &"<recursive>");
            } else {
                map.entry(key, value);
            }
        }
        map.finish()
    }
}

/// Jinja2 environment globals, so Core has them even though they are not context keys.
const JINJA_ENGINE_GLOBALS: &[&str] = &["cycler", "joiner", "lipsum"];

/// dbt-core's `BaseResolver` subclasses bound by `GenerateNameProvider`
/// (`core/dbt/context/providers.py`). Each one carries `.config`.
const RESOLVER_NAMES: &[&str] = &["ref", "source", "metric", "function"];

/// Builds the Jinja environment used to render documentation fields.
///
/// Keeps the parse environment's filters, tests and function registry. Drops everything
/// that can reach a project macro: the macro templates and every added global.
pub fn build_docs_jinja_environment(parse_env: &JinjaEnv) -> JinjaEnv {
    let engine_globals: Vec<(&str, MinijinjaValue)> = JINJA_ENGINE_GLOBALS
        .iter()
        .filter_map(|name| parse_env.get_global(name).map(|value| (*name, value)))
        .collect();

    let mut docs_env = parse_env.clone();
    docs_env.env.clear_templates();
    docs_env.env.reset_globals_to_defaults();
    // `jinja2.ext.debug` is not enabled in dbt Core.
    docs_env.env.remove_global("debug");
    for (name, value) in engine_globals {
        docs_env.env.add_global(name, value);
    }
    // `jinja2.Undefined`: printing an unknown name is empty, calling it is an error.
    docs_env
        .env
        .set_undefined_behavior(minijinja::UndefinedBehavior::Lenient);

    docs_env
}

/// Builds dbt Core's `DocsRuntimeContext` (`core/dbt/context/docs.py`).
pub fn build_docs_resolve_context(
    root_project_name: &str,
    node_package_name: &str,
    docs_macros: &BTreeMap<String, DbtDocsMacro>,
    parse_env: &JinjaEnv,
) -> FsResult<BTreeMap<String, MinijinjaValue>> {
    let global = |name: &str| {
        parse_env.get_global(name).ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "parse environment is missing the `{name}` global, \
                 cannot render documentation fields"
            )
        })
    };

    let docs_map: BTreeMap<(String, String), String> = docs_macros
        .values()
        .map(|doc| {
            (
                (doc.package_name.clone(), doc.name.clone()),
                doc.block_contents.clone(),
            )
        })
        .collect();

    // `Manifest._packages_to_search` returns `[current_project, node_package, None]`, so the
    // root project wins over a dependency defining the same doc name.
    let mut doc_package_search_order = vec![root_project_name.to_string()];
    if node_package_name != root_project_name {
        doc_package_search_order.push(node_package_name.to_string());
    }

    // Core binds `var` to the root project's config, and there is no `TARGET_PACKAGE_NAME` key.
    let configured_var = global("var")?
        .downcast_object::<ConfiguredVar>()
        .ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "parse environment `var` global is not a ConfiguredVar"
            )
        })?;

    let docs_ctx = DocsContext {
        context: MinijinjaValue::UNDEFINED,
        builtins: MinijinjaValue::UNDEFINED,
        dbt_version: global("dbt_version")?,
        var: MinijinjaValue::from_object(configured_var.bind_package(root_project_name)),
        env_var: global("env_var")?,
        // Key-surface parity; `return(...)` compiles to the VM instruction, never read here.
        return_fn: MinijinjaValue::UNDEFINED,
        fromjson: global("fromjson")?,
        tojson: global("tojson")?,
        fromyaml: global("fromyaml")?,
        toyaml: global("toyaml")?,
        set: global("set")?,
        set_strict: global("set_strict")?,
        zip: global("zip")?,
        zip_strict: global("zip_strict")?,
        log: global("log")?,
        run_started_at: global("run_started_at")?,
        invocation_id: global("invocation_id")?,
        thread_id: MinijinjaValue::from(
            std::thread::current().name().unwrap_or("main").to_string(),
        ),
        modules: global("modules")?,
        flags: global("flags")?,
        print: global("print")?,
        diff_of_two_dicts: global("diff_of_two_dicts")?,
        local_md5: global("local_md5")?,
        target: global("target")?,
        project_name: global("project_name")?,
        doc: MinijinjaValue::from_object(DocMacro::new_strict_with_search_order(
            doc_package_search_order,
            docs_map,
        )),
    };
    let mut ctx = to_jinja_btreemap(&docs_ctx);

    let mut builtins = ctx.clone();
    builtins.remove("context");
    builtins.remove("builtins");
    ctx.insert(
        "builtins".to_string(),
        MinijinjaValue::from_object(builtins),
    );
    ctx.insert(
        "context".to_string(),
        MinijinjaValue::from_object(RecursiveDocsContext {
            values: ctx.clone(),
        }),
    );

    Ok(ctx)
}

/// Builds a context for resolving models.
///
/// Internally constructs a typed [`ResolveBaseCtx`]; the
/// `BTreeMap<String, MinijinjaValue>` return type is preserved for now so
/// today's `dbt-parser` callers (which `.extend(...)` per-model overlays onto
/// this base) continue to work. A follow-up PR migrates them to consume the
/// typed struct directly via `render_named_str<S: Serialize>(...)`.
pub fn build_resolve_context(
    root_project_name: &str,
    local_project_name: &str,
    docs_macros: &BTreeMap<String, DbtDocsMacro>,
    macro_dispatch_order: BTreeMap<String, Vec<String>>,
    namespace_keys: Vec<String>,
    runtime_config: Option<Arc<DbtRuntimeConfig>>,
) -> BTreeMap<String, MinijinjaValue> {
    let docs_map: BTreeMap<(String, String), String> = docs_macros
        .values()
        .map(|v| {
            (
                (v.package_name.clone(), v.name.clone()),
                v.block_contents.clone(),
            )
        })
        .collect();

    let mut packages: BTreeSet<String> = namespace_keys.iter().cloned().collect();
    packages.insert(root_project_name.to_string());

    let dbt_namespaces: BTreeMap<String, JinjaObject<DbtNamespace>> = namespace_keys
        .into_iter()
        .map(|key| {
            let value = JinjaObject::new(DbtNamespace::new(&key));
            (key, value)
        })
        .collect();

    // Wrap each per-namespace search order as `Value::from(Vec<String>)` —
    // dispatch lookup downcasts to `Vec<String>` so the underlying Object
    // type must be exactly that, not the `MutableVec<Value>` that
    // serde-serializing a `Vec<String>` produces.
    let macro_dispatch_order: BTreeMap<String, MinijinjaValue> = macro_dispatch_order
        .into_iter()
        .map(|(k, v)| (k, MinijinjaValue::from(v)))
        .collect();

    // One store for the whole package, mirroring `build_compile_base_ctx`.
    // Safe to share here (unlike the per-batch case `reset_result_store`
    // guards against): `execute` is false at parse, so `statement` short-
    // circuits before reaching `store_result` and nothing is ever written.
    // Every `load_result` therefore returns none, which is what makes an
    // unguarded `run_query(...)` a no-op at parse — same as dbt-core.
    let result_store = ResultStore::default();

    // dbt-core's `BaseContext.to_dict` does `_ctx["builtins"] = builtins` then
    // `_ctx.update(builtins)`, so every resolver is reachable both ways. Nothing
    // at base scope can resolve a node, so these carry `.config` only.
    let resolvers: BTreeMap<String, MinijinjaValue> = runtime_config
        .map(|runtime_config| {
            let config = MinijinjaValue::from_dyn_object(runtime_config);
            RESOLVER_NAMES
                .iter()
                .map(|name| {
                    let attrs = BTreeMap::from([("config".to_string(), config.clone())]);
                    ((*name).to_string(), MinijinjaValue::from_object(attrs))
                })
                .collect()
        })
        .unwrap_or_default();
    let resolver = |name: &str| {
        resolvers
            .get(name)
            .cloned()
            .unwrap_or(MinijinjaValue::UNDEFINED)
    };
    let (ref_value, source_value, metric_value, function_value) = (
        resolver("ref"),
        resolver("source"),
        resolver("metric"),
        resolver("function"),
    );
    let builtins = if resolvers.is_empty() {
        MinijinjaValue::UNDEFINED
    } else {
        MinijinjaValue::from_object(resolvers)
    };

    let ctx = ResolveBaseCtx {
        doc: MinijinjaValue::from_object(DocMacro::new(root_project_name.to_string(), docs_map)),
        macro_dispatch_order,
        target_package_name: local_project_name.to_string(),
        execute: false,
        context: JinjaObject::new(MacroLookupContext::new(
            root_project_name.to_string(),
            None,
            packages,
        )),
        node: MinijinjaValue::NONE,
        connection_name: String::new(),
        store_result: MinijinjaValue::from_function(result_store.store_result()),
        load_result: MinijinjaValue::from_function(result_store.load_result()),
        store_raw_result: MinijinjaValue::from_function(result_store.store_raw_result()),
        builtins,
        ref_fn: ref_value,
        source: source_value,
        metric: metric_value,
        function: function_value,
        dbt_namespaces,
    };

    to_jinja_btreemap(&ctx)
}

#[cfg(test)]
mod docs_context_tests {
    use std::collections::{BTreeMap, BTreeSet};

    use dbt_common::path::DbtPath;
    use dbt_jinja_vars::{ConfiguredVar, DbtVars};
    use dbt_schemas::schemas::macros::DbtDocsMacro;
    use indexmap::IndexMap;
    use minijinja::{Value, constants::ROOT_PACKAGE_NAME};

    use super::*;
    use crate::environment_builder::JinjaEnvBuilder;

    /// `REQUIRED_DOCS_KEYS` from dbt-core `tests/unit/context/test_context.py`.
    const CORE_DOCS_KEYS: &[&str] = &[
        "context",
        "builtins",
        "dbt_version",
        "var",
        "env_var",
        "return",
        "fromjson",
        "tojson",
        "fromyaml",
        "toyaml",
        "set",
        "set_strict",
        "zip",
        "zip_strict",
        "log",
        "run_started_at",
        "invocation_id",
        "thread_id",
        "modules",
        "flags",
        "print",
        "diff_of_two_dicts",
        "local_md5",
        "target",
        "project_name",
        "doc",
    ];

    /// Stand-in for the parse environment: project macros as templates and as
    /// package-namespace globals, plus the runtime-only callables.
    fn parse_env() -> JinjaEnv {
        let mut root_vars = IndexMap::new();
        root_vars.insert(
            "configured".to_string(),
            DbtVars::Value(dbt_yaml::from_str("configured").unwrap()),
        );
        let mut dependency_vars = IndexMap::new();
        dependency_vars.insert(
            "configured".to_string(),
            DbtVars::Value(dbt_yaml::from_str("dependency").unwrap()),
        );
        let package_vars = BTreeMap::from([
            ("dependency_pkg".to_string(), dependency_vars),
            ("root_pkg".to_string(), root_vars),
        ]);
        let globals = BTreeMap::from([
            ("run_started_at".to_string(), Value::from("2026-07-22")),
            ("invocation_id".to_string(), Value::from("invocation")),
            (
                "flags".to_string(),
                Value::from_object(BTreeMap::<String, Value>::new()),
            ),
            (
                "target".to_string(),
                Value::from_object(BTreeMap::<String, Value>::new()),
            ),
            ("project_name".to_string(), Value::from("root_pkg")),
            (
                "var".to_string(),
                Value::from_object(ConfiguredVar::new(package_vars, BTreeMap::new())),
            ),
        ]);
        let mut env = JinjaEnvBuilder::new().with_globals(globals).build();

        env.env
            .add_global(ROOT_PACKAGE_NAME, Value::from("root_pkg"));
        env.env
            .add_template_owned(
                "root_pkg.project_macro".to_string(),
                "{% macro project_macro() %}project{% endmacro %}".to_string(),
                None,
            )
            .unwrap();
        env.env
            .add_template_owned(
                "dependency_pkg.dependency_macro".to_string(),
                "{% macro dependency_macro() %}dependency{% endmacro %}".to_string(),
                None,
            )
            .unwrap();
        env.env.add_global(
            "root_pkg",
            Value::from_object(BTreeMap::from([(
                "project_macro".to_string(),
                Value::from_function(|_: &[Value]| Ok("project")),
            )])),
        );
        env.env.add_global(
            "dependency_pkg",
            Value::from_object(BTreeMap::from([(
                "dependency_macro".to_string(),
                Value::from_function(|_: &[Value]| Ok("dependency")),
            )])),
        );
        env.env.add_global(
            "adapter",
            Value::from_object(BTreeMap::<String, Value>::new()),
        );
        for name in [
            "ref",
            "source",
            "config",
            "run_query",
            "statement",
            "load_result",
            "store_result",
        ] {
            env.env
                .add_global(name, Value::from_function(|_: &[Value]| Ok("runtime")));
        }
        env
    }

    fn docs_macro(package: &str, name: &str, contents: &str) -> DbtDocsMacro {
        DbtDocsMacro {
            name: name.to_string(),
            package_name: package.to_string(),
            path: DbtPath::from("models/docs.md"),
            original_file_path: DbtPath::from("models/docs.md"),
            unique_id: format!("doc.{package}.{name}"),
            block_contents: contents.to_string(),
        }
    }

    fn docs_macros() -> BTreeMap<String, DbtDocsMacro> {
        BTreeMap::from([(
            "doc.root_pkg.description".to_string(),
            docs_macro("root_pkg", "description", "documented"),
        )])
    }

    fn docs_env_and_ctx() -> (JinjaEnv, BTreeMap<String, Value>) {
        let parse_env = parse_env();
        let ctx =
            build_docs_resolve_context("root_pkg", "root_pkg", &docs_macros(), &parse_env).unwrap();
        (build_docs_jinja_environment(&parse_env), ctx)
    }

    fn value_keys(value: &Value) -> BTreeSet<String> {
        value
            .try_iter()
            .unwrap()
            .filter_map(|key| key.as_str().map(str::to_string))
            .collect()
    }

    #[test]
    fn docs_context_matches_core_key_surface() {
        let (_, ctx) = docs_env_and_ctx();
        let expected: BTreeSet<String> = CORE_DOCS_KEYS
            .iter()
            .map(|key| (*key).to_string())
            .collect();

        assert_eq!(ctx.keys().cloned().collect::<BTreeSet<_>>(), expected);
        assert_eq!(value_keys(ctx.get("context").unwrap()), expected);

        let mut expected_builtins = expected;
        expected_builtins.remove("context");
        expected_builtins.remove("builtins");
        assert_eq!(value_keys(ctx.get("builtins").unwrap()), expected_builtins);
    }

    #[test]
    fn docs_context_supports_core_helpers_and_ordinary_jinja() {
        let (env, ctx) = docs_env_and_ctx();
        let cases = [
            ("doc", "{{ doc('description') }}", "documented"),
            ("var", "{{ var('configured') }}", "configured"),
            (
                "env_var",
                "{{ env_var('DBT_ISSUE_14494_UNSET', 'fallback') }}",
                "fallback",
            ),
            ("fromjson", "{{ fromjson('{\"value\": 1}').value }}", "1"),
            ("fromyaml", "{{ fromyaml('value: 1').value }}", "1"),
            ("tojson", "{{ tojson({'value': 1}) }}", "{\"value\": 1}"),
            ("toyaml", "{{ toyaml({'value': 1}) }}", "value: 1\n"),
            ("set", "{{ set([1, 1, 2]) | length }}", "2"),
            ("set_strict", "{{ set_strict([1, 1, 2]) | length }}", "2"),
            ("zip", "{{ zip([1], [2]) | list | length }}", "1"),
            (
                "zip_strict",
                "{{ zip_strict([1], [2]) | list | length }}",
                "1",
            ),
            (
                "other base helpers",
                "{{ local_md5('hello') | length }}|{{ log is defined }}|{{ print is defined }}|{{ diff_of_two_dicts is defined }}",
                "32|True|True|True",
            ),
            (
                "ordinary Jinja, and undefined names print empty",
                "{% if 2 is even %}{{ 'ORDINARY' | lower }}{% endif %}|{{ missing_name }}",
                "ordinary|",
            ),
            (
                "jinja2 engine globals",
                "{{ range(3) | list | length }}|{{ dict(answer=42).answer }}|{{ namespace(value='ok').value }}|{{ cycler is defined }}|{{ joiner is defined }}|{{ lipsum is defined }}|{{ debug is defined }}",
                "3|42|ok|True|True|True|False",
            ),
            (
                "context and builtins",
                "{{ builtins.doc('description') }}|{{ context.doc('description') }}|{{ context.context.doc('description') }}",
                "documented|documented|documented",
            ),
        ];

        for (label, template, expected) in cases {
            assert_eq!(
                env.render_str(template, &ctx, &[]).unwrap(),
                expected,
                "{label}"
            );
        }

        // Core's `context` is self-referential, so printing it must terminate.
        assert!(
            env.render_str("{{ context }}", &ctx, &[])
                .unwrap()
                .contains("<recursive>")
        );
    }

    #[test]
    fn docs_context_rejects_project_macros_and_runtime_callables() {
        let (env, ctx) = docs_env_and_ctx();
        for (label, template) in [
            ("direct project macro", "{{ project_macro() }}"),
            ("package-qualified macro", "{{ root_pkg.project_macro() }}"),
            (
                "dependency macro",
                "{{ dependency_pkg.dependency_macro() }}",
            ),
            ("adapter.dispatch", "{{ adapter.dispatch('macro') }}"),
            ("ref", "{{ ref('model') }}"),
            ("source", "{{ source('source', 'table') }}"),
            ("config", "{{ config(materialized='table') }}"),
            ("run_query", "{{ run_query('select 1') }}"),
            ("statement", "{{ statement('name') }}"),
            ("load_result", "{{ load_result('name') }}"),
            ("store_result", "{{ store_result('name', {}) }}"),
            ("exceptions", "{{ exceptions.warn('unavailable') }}"),
        ] {
            assert!(
                env.render_str(template, &ctx, &[]).is_err(),
                "{label} unexpectedly rendered"
            );
        }

        for name in [
            "root_pkg",
            "dependency_pkg",
            "adapter",
            "ref",
            "source",
            "config",
            "run_query",
            "statement",
            "load_result",
            "store_result",
            "exceptions",
        ] {
            assert_eq!(
                env.render_str(&format!("{{{{ {name} is defined }}}}"), &ctx, &[])
                    .unwrap(),
                "False",
                "{name} unexpectedly present"
            );
        }
    }

    /// `_packages_to_search` returns `[current_project, node_package, None]` by default.
    #[test]
    fn docs_context_searches_the_root_project_before_the_node_package() {
        let parse_env = parse_env();
        let docs_env = build_docs_jinja_environment(&parse_env);
        let mut docs = docs_macros();
        docs.insert(
            "doc.dependency_pkg.description".to_string(),
            docs_macro("dependency_pkg", "description", "dependency documentation"),
        );

        let ctx =
            build_docs_resolve_context("root_pkg", "dependency_pkg", &docs, &parse_env).unwrap();
        assert_eq!(
            docs_env
                .render_str("{{ doc('description') }}", &ctx, &[])
                .unwrap(),
            "documented"
        );
        // Explicitly qualifying the package still reaches the dependency's block.
        assert_eq!(
            docs_env
                .render_str("{{ doc('dependency_pkg', 'description') }}", &ctx, &[])
                .unwrap(),
            "dependency documentation"
        );
        // `var` resolves against the root project, not the node package.
        assert_eq!(
            docs_env
                .render_str("{{ var('configured') }}", &ctx, &[])
                .unwrap(),
            "configured"
        );

        // Falls back to the node package when the root project has no such block.
        docs.remove("doc.root_pkg.description");
        let ctx =
            build_docs_resolve_context("root_pkg", "dependency_pkg", &docs, &parse_env).unwrap();
        assert_eq!(
            docs_env
                .render_str("{{ doc('description') }}", &ctx, &[])
                .unwrap(),
            "dependency documentation"
        );
    }

    #[test]
    fn docs_context_rejects_missing_doc_targets() {
        let (env, ctx) = docs_env_and_ctx();
        for template in ["{{ doc('missing') }}", "{{ doc('root_pkg', 'missing') }}"] {
            let error = env.render_str(template, &ctx, &[]).unwrap_err();
            assert!(error.to_string().contains("which was not found"), "{error}");
        }
    }

    #[test]
    fn docs_context_rejects_invalid_doc_arguments() {
        let parse_env = parse_env();
        let env = build_docs_jinja_environment(&parse_env);
        let mut docs = docs_macros();
        docs.insert(
            "doc.root_pkg.true".to_string(),
            docs_macro("root_pkg", "true", "boolean-like doc name"),
        );
        docs.insert(
            "doc.root_pkg.1".to_string(),
            docs_macro("root_pkg", "1", "integer-like doc name"),
        );
        let ctx = build_docs_resolve_context("root_pkg", "root_pkg", &docs, &parse_env).unwrap();

        for template in [
            "{{ doc() }}",
            "{{ doc('root_pkg', 'description', 'extra') }}",
            "{{ doc('description', ignored='extra') }}",
            "{{ doc('root_pkg', {'name': 'description'}) }}",
            "{{ doc(true) }}",
            "{{ doc(1) }}",
        ] {
            assert!(env.render_str(template, &ctx, &[]).is_err(), "{template}");
        }
        // Names that merely look boolean or numeric are still valid strings.
        assert_eq!(
            env.render_str("{{ doc('true') }}", &ctx, &[]).unwrap(),
            "boolean-like doc name"
        );
        assert_eq!(
            env.render_str("{{ doc('1') }}", &ctx, &[]).unwrap(),
            "integer-like doc name"
        );
    }

    #[test]
    fn build_docs_resolve_context_errors_instead_of_panicking_on_a_bare_environment() {
        let error = build_docs_resolve_context(
            "root_pkg",
            "root_pkg",
            &docs_macros(),
            &JinjaEnvBuilder::new().build(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("parse environment is missing"),
            "{error}"
        );
    }
}
