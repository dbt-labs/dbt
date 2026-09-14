//! Render tests for the dbt-parser crate
#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use dbt_adapter::Adapter;
    use dbt_adapter::sql_types::DefaultTypeOps;
    use dbt_adapter_core::AdapterType;
    use dbt_common::io_args::StaticAnalysisKind;
    use dbt_common::{FsResult, io_args::IoArgs};
    use dbt_frontend_common::error::CodeLocation;
    use dbt_jinja_utils::invocation_args::InvocationArgs;
    use dbt_jinja_utils::jinja_environment::JinjaEnv;
    use dbt_jinja_utils::listener::DefaultRenderingEventListenerFactory;
    use dbt_jinja_utils::phases::parse::build_resolve_model_context;
    use dbt_jinja_utils::phases::parse::init::initialize_parse_jinja_environment;
    use dbt_jinja_utils::phases::parse::sql_resource::SqlResource;
    use dbt_jinja_utils::utils::render_sql;
    use dbt_schemas::schemas::profiles::PostgresDbConfig;
    use dbt_schemas::schemas::project::ProjectModelConfig;
    use dbt_schemas::schemas::project::{ModelConfig, ResolvableConfig};
    use dbt_schemas::schemas::relations::DEFAULT_DBT_QUOTING;
    use dbt_schemas::schemas::serde::StringOrInteger;
    use dbt_schemas::state::DbtRuntimeConfig;
    use dbt_test_primitives::assert_contains;
    use minijinja::ArgSpec;
    use minijinja::constants::TARGET_PACKAGE_NAME;
    use minijinja::machinery::Span;
    use minijinja::{AutoEscape, Error};
    use minijinja::{Environment, Value};

    use crate::utils::{get_node_fqn, parse_macro_statements};

    use chrono::{DateTime, Utc};
    use chrono_tz::Tz;
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::{collections::BTreeMap, path::PathBuf};

    fn create_resolve_model_context<T: ResolvableConfig<T> + serde::Serialize + 'static>(
        init_config: &T,
        sql_resources: &Arc<Mutex<Vec<SqlResource<T>>>>,
    ) -> BTreeMap<String, Value> {
        let mut context = build_resolve_model_context(
            init_config,
            false,
            AdapterType::Postgres,
            "db",
            "schema",
            "my_model",
            get_node_fqn(
                "common",
                PathBuf::from("test"),
                vec!["my_model".to_string()],
                &["models".to_string()],
            ),
            "common",
            "test",
            DEFAULT_DBT_QUOTING,
            Arc::new(DbtRuntimeConfig::default()),
            Arc::new(DbtRuntimeConfig::default()),
            sql_resources.clone(),
            Arc::new(AtomicBool::new(false)),
            &PathBuf::from("test"),
            &PathBuf::from("test"),
            Some(StaticAnalysisKind::Strict),
        );
        context.insert(TARGET_PACKAGE_NAME.to_string(), Value::from("common"));
        context
    }

    fn setup_test_env() -> (
        JinjaEnv,
        Arc<Mutex<Vec<SqlResource<ModelConfig>>>>,
        ModelConfig,
    ) {
        let init_config = ModelConfig {
            alias: Some("alias".to_string()),
            ..Default::default()
        };
        let invocation_args = InvocationArgs {
            ..Default::default()
        };
        let tz_now: DateTime<Tz> = Utc::now().with_timezone(&Tz::UTC);

        let env = initialize_parse_jinja_environment(
            "common",
            "profile",
            "target",
            AdapterType::Postgres,
            (PostgresDbConfig {
                port: Some(StringOrInteger::Integer(5432)),
                database: Some("postgres".to_string()),
                host: Some("localhost".to_string()),
                user: Some("postgres".to_string()),
                password: Some("postgres".to_string()),
                schema: Some("schema".to_string()),
                ..Default::default()
            })
            .into(),
            vec![AdapterType::Postgres],
            DEFAULT_DBT_QUOTING,
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            tz_now,
            &invocation_args,
            BTreeSet::from(["common".to_string()]),
            IoArgs::default(),
            None,
        )
        .unwrap();

        let sql_resources = Arc::new(Mutex::new(Vec::new()));

        (env, sql_resources, init_config)
    }

    #[test]
    fn test_meta_field_is_verbatim_at_parse_time() {
        // +description (a plain string field) still renders eagerly as part of the single
        // combined `into_typed` pass over the whole document.
        //
        // +meta does not: it's `Verbatim<...>` (fs#14217) precisely so that a render failure
        // nested inside a customer's `+meta` value can't fail this whole node -- and by
        // extension the whole enclosing directory-path node -- the way a plain field's failure
        // would. `dbt_project_yml_loader::render_meta_tolerantly` re-renders +meta on its own,
        // afterward, tolerating a failure instead of propagating it; see
        // `dbt-loader::dbt_project_yml_loader::tests::test_meta_render_failure_does_not_drop_sibling_config`
        // for that success/failure-tolerance behavior end to end.
        let yaml = r#"
        +meta:
          demo: "{{ 1 + 2 }}"
        +description: "prefix {{ 1 + 2 }}"
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let cfg: ProjectModelConfig = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();

        let meta = cfg.meta.0.as_ref().expect("+meta should be present");
        let demo_val = meta.get("demo").expect("demo key in +meta");
        // Still the raw, unrendered Jinja text -- `into_typed_with_jinja` alone never touches it.
        match demo_val {
            dbt_yaml::Value::String(s, _) => assert_eq!(s, "{{ 1 + 2 }}"),
            other => panic!("expected raw string in +meta.demo, got {other:?}"),
        }

        assert_eq!(cfg.description.as_deref(), Some("prefix 3"));
    }

    #[test]
    fn test_freshness_dict_literal_renders_as_typed() {
        use dbt_schemas::schemas::common::{FreshnessDefinition, FreshnessPeriod};

        // Body contains a dict literal, so inner `{`/`}` are present. The
        // outer `{{ ... }}` is still a single expression and must deserialize
        // into FreshnessRules rather than collapsing to the string
        // "{'count': 38, 'period': 'day'}".
        let yaml = r#"
        error_after: "{{ {'count': 38, 'period': 'day'} }}"
        warn_after:
          count: 10
          period: hour
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let freshness: FreshnessDefinition = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();

        let error = freshness
            .error_after
            .expect("error_after should deserialize as FreshnessRules");
        assert_eq!(error.count, Some(38));
        assert_eq!(error.period, Some(FreshnessPeriod::day));

        let warn = freshness
            .warn_after
            .expect("warn_after should deserialize as FreshnessRules");
        assert_eq!(warn.count, Some(10));
        assert_eq!(warn.period, Some(FreshnessPeriod::hour));
    }

    #[test]
    fn test_freshness_dict_literal_ternary_renders_as_null() {
        use dbt_schemas::schemas::common::FreshnessDefinition;

        // Ternary where the `none` branch is taken. The dict literal on the
        // other branch injects inner `{`/`}` into the expression body, but
        // the rendered result must still be YAML null — not the string "None".
        let yaml = r#"
        error_after: "{{ none if true else {'count': 1, 'period': 'day'} }}"
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let freshness: FreshnessDefinition = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();

        assert!(
            freshness.error_after.is_none(),
            "expected error_after to deserialize as null, got {:?}",
            freshness.error_after
        );
    }

    #[test]
    fn test_freshness_entire_value_nested_dict_renders_as_typed() {
        use dbt_schemas::schemas::common::{FreshnessDefinition, FreshnessPeriod};

        // The whole `freshness` value is a single Jinja expression evaluating to
        // a mapping whose `warn_after` entry is itself a nested dict literal
        // (issue #13214). The adjacent closing braces from the inner and outer
        // dict literals (`...'day'}}`) must not be mistaken for a Jinja `}}`
        // delimiter -- doing so previously routed the whole mapping through
        // string rendering and raised dbt1013 instead of a typed FreshnessDefinition.
        let yaml = r#""{{ {'warn_after': {'count': 1, 'period': 'day'}} }}""#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let freshness: FreshnessDefinition = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();

        let warn = freshness
            .warn_after
            .expect("warn_after should deserialize as FreshnessRules");
        assert_eq!(warn.count, Some(1));
        assert_eq!(warn.period, Some(FreshnessPeriod::day));
    }

    #[dbt_runtime::test]
    async fn test_render_sql_with_ref_macro() {
        let (env, sql_resources, init_config) = setup_test_env();
        // Set the package name for the current context
        {
            let resolve_model_context = create_resolve_model_context(&init_config, &sql_resources);
            let sql = "SELECT * FROM {{ ref('my_table') }};";

            let rendered = render_sql(
                sql,
                &env,
                &resolve_model_context,
                &DefaultRenderingEventListenerFactory::default(),
                &PathBuf::from("test"),
            )
            .unwrap();

            let sql_resources_locked = sql_resources.lock().unwrap().clone();

            assert_eq!(
                rendered.trim(),
                "SELECT * FROM \"db\".\"schema\".\"my_table\";"
            );
            assert_eq!(
                sql_resources_locked,
                vec![SqlResource::Ref((
                    "my_table".to_string(),
                    None,
                    None,
                    CodeLocation::new(1, 15, 14)
                ))]
            );
        }
    }

    #[dbt_runtime::test]
    async fn test_render_sql_with_source_macro() {
        let (env, sql_resources, init_config) = setup_test_env();
        // Set the package name for the current context
        {
            let resolve_model_scope = create_resolve_model_context(&init_config, &sql_resources);
            let sql = "SELECT * FROM {{ source('my_schema', 'my_table') }};";

            let rendered = render_sql(
                sql,
                &env,
                &resolve_model_scope,
                &DefaultRenderingEventListenerFactory::default(),
                &PathBuf::from("test"),
            )
            .unwrap();

            let sql_resources_locked = sql_resources.lock().unwrap().clone();

            assert_eq!(
                rendered.trim(),
                "SELECT * FROM \"db\".\"schema\".\"my_table\";"
            );
            assert_eq!(
                sql_resources_locked,
                vec![SqlResource::Source((
                    "my_schema".to_string(),
                    "my_table".to_string(),
                    CodeLocation::new(1, 15, 14)
                ))]
            );
        }
    }

    #[dbt_runtime::test]
    async fn test_render_sql_with_metric_macro() {
        let (env, sql_resources, init_config) = setup_test_env();
        // Set the package name for the current context
        {
            let resolve_model_scope = create_resolve_model_context(&init_config, &sql_resources);
            let sql = "{{ metric('metric') }} {{ metric('metric_package', 'metric_two') }}";

            let rendered = render_sql(
                sql,
                &env,
                &resolve_model_scope,
                &DefaultRenderingEventListenerFactory::default(),
                &PathBuf::from("test"),
            )
            .unwrap();

            let sql_resources_locked = sql_resources.lock().unwrap().clone();

            assert_eq!(rendered.trim(), "metric metric_two");
            assert_eq!(
                sql_resources_locked,
                vec![
                    SqlResource::Metric(("metric".to_string(), None)),
                    SqlResource::Metric((
                        "metric_two".to_string(),
                        Some("metric_package".to_string())
                    )),
                ]
            );
        }
    }

    #[dbt_runtime::test]
    async fn test_render_sql_with_config_macro() {
        let (env, sql_resources, init_config) = setup_test_env();
        // Set the package name for the current context
        {
            let resolve_model_scope = create_resolve_model_context(&init_config, &sql_resources);
            let sql = r#"
        {{
            config(
                schema = 'my_schema',
                alias = 'my_alias'~'suffix',
                materialized = 'view'
            )
        }}
        "#;
            let rendered = render_sql(
                sql,
                &env,
                &resolve_model_scope,
                &DefaultRenderingEventListenerFactory::default(),
                &PathBuf::from("test"),
            )
            .unwrap();

            assert_eq!(rendered.trim(), "");

            let expected_config = {
                let mut map = BTreeMap::new();
                map.insert("schema".to_string(), Value::from("my_schema"));
                map.insert("alias".to_string(), Value::from("my_aliassuffix"));
                map.insert("materialized".to_string(), Value::from("view"));
                map.insert("enabled".to_string(), Value::from(true)); // this gets inhertied from the global config which is true if not specified (important that this is not overridden)
                let config: ModelConfig =
                    dbt_yaml::from_value(dbt_yaml::to_value(map).unwrap()).unwrap();
                SqlResource::ConfigCall(Box::new(config))
            };

            let sql_resources_locked = sql_resources.lock().unwrap().clone();
            assert_eq!(sql_resources_locked, vec![expected_config]);
        }
    }

    #[test]
    #[ignore = "This test does not work due to dispatch not getting context of macros defined below"]
    fn test_adapter_dispatch() {
        #[allow(unused_imports)] // required to compile code with various feature flags
        use minijinja::compiler::parser::Parser;
        #[allow(unused_imports)] // required to compile code with various feature flags
        use minijinja::machinery::WhitespaceConfig;
        #[allow(unused_imports)] // required to compile code with various feature flags
        use minijinja::machinery::{CodeGenerator, Instructions, Vm};
        #[allow(unused_imports)] // required to compile code with various feature flags
        use minijinja::syntax::SyntaxConfig;
        #[allow(dead_code)]
        fn simple_eval<S: serde::Serialize>(
            instructions: &Instructions<'_>,
            ctx: S,
        ) -> Result<String, Error> {
            let mut env = Environment::new();
            let adapter = Arc::new(Adapter::new_parse_phase_adapter(
                AdapterType::Postgres,
                dbt_yaml::Mapping::default(),
                DEFAULT_DBT_QUOTING,
                Arc::new(DefaultTypeOps::new(AdapterType::Postgres)),
                None,
            ));
            env.add_global("adapter", adapter.as_value());
            let empty_blocks = BTreeMap::new();
            let vm = Vm::new(&env);
            let root = Value::from_serialize(&ctx);

            Ok(vm
                .eval(instructions, root, &empty_blocks, AutoEscape::None, &[])?
                .0
                .as_str()
                .unwrap()
                .to_string())
        }
        panic!("test code disabled below");
    }

    #[dbt_runtime::test]
    async fn test_fromjson() {
        let (env, _, _) = setup_test_env();
        let env = Arc::new(env);
        let sql = r#"
        {% set json_str = '{"abc": 123}' %}
        {% set parsed = fromjson(json_str) %}
        {{ parsed['abc'] }}
        "#;

        let rendered = render_sql(
            sql,
            &env,
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            &PathBuf::from("test"),
        )
        .unwrap();

        assert_eq!(rendered.trim(), "123");
    }

    #[dbt_runtime::test]
    async fn test_tojson() {
        let (env, _, _) = setup_test_env();
        let env = Arc::new(env);
        let sql = r#"
        {% set my_dict = {"abc": 123, "def": 456} %}
        {% set json_str = tojson(my_dict) %}
        {{ json_str }}
        "#;

        let rendered = render_sql(
            sql,
            &env,
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            &PathBuf::from("test"),
        )
        .unwrap();

        let rendered = rendered.trim().replace(" ", "").replace("\n", "");
        assert_eq!(rendered, r#"{"abc":123,"def":456}"#);
    }

    #[dbt_runtime::test]
    async fn test_tojson_with_sort_keys() {
        let (env, _, _) = setup_test_env();
        let env = Arc::new(env);
        let sql = r#"
        {% set my_dict = {"def": 456, "abc": 123} %}
        {% set json_str = tojson(my_dict, sort_keys=true) %}
        {{ json_str }}
        "#;

        let rendered = render_sql(
            sql,
            &env,
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            &PathBuf::from("test"),
        )
        .unwrap();

        let rendered = rendered.trim().replace(" ", "").replace("\n", "");
        assert_eq!(rendered, r#"{"abc":123,"def":456}"#);
    }

    #[dbt_runtime::test]
    async fn test_tojson_with_default() {
        let (env, _, _) = setup_test_env();
        let env = Arc::new(env);
        let sql = r#"
        {% set invalid_json = undefined %}
        {% set json_str = tojson(invalid_json, '{"default": true}') %}
        {{ json_str }}
        "#;

        let rendered = render_sql(
            sql,
            &env,
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            &PathBuf::from("test"),
        )
        .unwrap();

        assert_eq!(rendered.trim(), r#"{"default": true}"#);
    }

    #[dbt_runtime::test]
    async fn test_fromyaml() {
        let (env, _, _) = setup_test_env();
        let env = Arc::new(env);
        let sql = r#"
        {% set my_yml_str -%}
        dogs:
         - good
         - bad
        {%- endset %}
        {% set my_dict = fromyaml(my_yml_str) %}
        {{ my_dict['dogs'] | join(", ") }}
        "#;

        let rendered = render_sql(
            sql,
            &env,
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            &PathBuf::from("test"),
        )
        .unwrap();

        assert_eq!(rendered.trim(), "good, bad");
    }

    #[dbt_runtime::test]
    async fn test_toyaml_basic() {
        let (env, _, _) = setup_test_env();
        let env = Arc::new(env);
        let sql = r#"
        {% set my_dict = {"abc": 123, "def": 456} %}
        {% set yaml_str = toyaml(my_dict) %}
        {{ yaml_str }}
        "#;

        // Render the snippet
        let rendered = render_sql(
            sql,
            &env,
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            &PathBuf::from("test"),
        )
        .unwrap();

        let trimmed = rendered.trim().replace('\n', " ").replace('\r', "");
        assert_contains!(trimmed, "abc: 123");
        assert_contains!(trimmed, "def: 456");
    }

    #[dbt_runtime::test]
    async fn test_set_strict_function() {
        let (env, _, _) = setup_test_env();
        let env = Arc::new(env);
        let sql = r#"
        {% set my_list = [1, 2, 2, 3] %}
        {% set my_set = set_strict(my_list) %}
        {{ my_set | join(", ") }}
        "#;

        let rendered = render_sql(
            sql,
            &env,
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            &PathBuf::from("test"),
        )
        .unwrap();

        let trimmed = rendered.trim();
        assert!(
            trimmed == "1, 2, 3"
                || trimmed == "1, 3, 2"
                || trimmed == "2, 1, 3"
                || trimmed == "2, 3, 1"
                || trimmed == "3, 1, 2"
                || trimmed == "3, 2, 1"
        );

        // Test error case with non-iterable
        let sql_error = r#"
        {% set my_set = set_strict(42) %}
        {{ my_set }}
        "#;

        let result = render_sql(
            sql_error,
            &env,
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            &PathBuf::from("test"),
        );

        assert!(result.is_err());
    }

    #[dbt_runtime::test]
    async fn test_local_md5() {
        let (env, _, _) = setup_test_env();
        let env = Arc::new(env);
        let sql = r#"
        {% set value = "hello world" %}
        {{ local_md5(value) }}
        "#;

        let rendered = render_sql(
            sql,
            &env,
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            &PathBuf::from("test"),
        )
        .unwrap();

        assert_eq!(rendered.trim(), "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }

    #[test]
    fn test_parse_regular_macro() -> FsResult<()> {
        let sql = r#"
            {% macro my_macro() %}
                select 1 as col
            {% endmacro %}
        "#;

        let resources = parse_macro_statements(sql, &PathBuf::from("test.sql"), &["macro"])?;
        assert_eq!(
            resources,
            vec![SqlResource::Macro(
                "my_macro".to_string(),
                Span {
                    start_line: 2,
                    start_col: 13,
                    start_offset: 13,
                    end_line: 4,
                    end_col: 27,
                    end_offset: 94
                },
                None,
                vec![],
                Span {
                    start_line: 2,
                    start_col: 22,
                    start_offset: 22,
                    end_line: 2,
                    end_col: 30,
                    end_offset: 30
                }
            )]
        );
        Ok(())
    }

    #[test]
    fn test_parse_macro_with_non_ascii_name() -> FsResult<()> {
        let sql = r#"
            {% macro trans_añadir_costes(nombre_tabla) %}
                select * from {{ nombre_tabla }}
            {% endmacro %}
        "#;

        let resources = parse_macro_statements(sql, &PathBuf::from("test.sql"), &["macro"])?;
        assert_eq!(resources.len(), 1);
        match &resources[0] {
            SqlResource::Macro(name, _, _, args, _) => {
                assert_eq!(name, "trans_añadir_costes");
                assert_eq!(
                    args,
                    &vec![ArgSpec {
                        name: "nombre_tabla".to_string(),
                        is_optional: false,
                    }]
                );
            }
            other => panic!("expected macro resource, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn test_parse_test_macro() -> FsResult<()> {
        let sql = r#"
            {% test positive_value(model, column_name) %}
                select *
                from {{ model }}
                where {{ column_name }} < 0
            {% endtest %}
        "#;

        let resources = parse_macro_statements(sql, &PathBuf::from("test.sql"), &["test"])?;
        assert_eq!(
            resources,
            vec![SqlResource::Test(
                "test_positive_value".to_string(),
                Span {
                    start_line: 2,
                    start_col: 13,
                    start_offset: 13,
                    end_line: 6,
                    end_col: 26,
                    end_offset: 186
                },
                vec![
                    ArgSpec {
                        name: "model".to_string(),
                        is_optional: false
                    },
                    ArgSpec {
                        name: "column_name".to_string(),
                        is_optional: false
                    },
                ],
                Span {
                    start_line: 2,
                    start_col: 21,
                    start_offset: 21,
                    end_line: 2,
                    end_col: 35,
                    end_offset: 35
                }
            )]
        );
        Ok(())
    }

    #[test]
    fn test_parse_multiple_macros() -> FsResult<()> {
        let sql = r#"
            {% macro first() %}
                select 1
            {% endmacro %}

            {% test second(model) %}
                select * from {{ model }}
            {% endtest %}

            {% macro third() %}
                select 3
            {% endmacro %}
        "#;

        let resources =
            parse_macro_statements(sql, &PathBuf::from("test.sql"), &["macro", "test"])?;
        assert_eq!(
            resources,
            vec![
                SqlResource::Macro(
                    "first".to_string(),
                    Span {
                        start_line: 2,
                        start_col: 13,
                        start_offset: 13,
                        end_line: 4,
                        end_col: 27,
                        end_offset: 84
                    },
                    None,
                    vec![],
                    Span {
                        start_line: 2,
                        start_col: 22,
                        start_offset: 22,
                        end_line: 2,
                        end_col: 27,
                        end_offset: 27
                    }
                ),
                SqlResource::Test(
                    "test_second".to_string(),
                    Span {
                        start_line: 6,
                        start_col: 13,
                        start_offset: 98,
                        end_line: 8,
                        end_col: 26,
                        end_offset: 190
                    },
                    vec![ArgSpec {
                        name: "model".to_string(),
                        is_optional: false
                    }],
                    Span {
                        start_line: 6,
                        start_col: 21,
                        start_offset: 106,
                        end_line: 6,
                        end_col: 27,
                        end_offset: 112
                    }
                ),
                SqlResource::Macro(
                    "third".to_string(),
                    Span {
                        start_line: 10,
                        start_col: 13,
                        start_offset: 204,
                        end_line: 12,
                        end_col: 27,
                        end_offset: 275
                    },
                    None,
                    vec![],
                    Span {
                        start_line: 10,
                        start_col: 22,
                        start_offset: 213,
                        end_line: 10,
                        end_col: 27,
                        end_offset: 218
                    }
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn test_parse_nested_macros() -> FsResult<()> {
        let sql = r#"
            {% macro outer() %}
                {% macro inner() %}
                    select 1
                {% endmacro %}
            {% endmacro %}
        "#;

        let resources = parse_macro_statements(sql, &PathBuf::from("test.sql"), &["macro"])?;
        assert_eq!(
            resources,
            vec![
                SqlResource::Macro(
                    "outer".to_string(),
                    Span {
                        start_line: 2,
                        start_col: 13,
                        start_offset: 13,
                        end_line: 6,
                        end_col: 27,
                        end_offset: 155
                    },
                    None,
                    vec![],
                    Span {
                        start_line: 2,
                        start_col: 22,
                        start_offset: 22,
                        end_line: 2,
                        end_col: 27,
                        end_offset: 27
                    }
                ),
                SqlResource::Macro(
                    "inner".to_string(),
                    Span {
                        start_line: 3,
                        start_col: 17,
                        start_offset: 49,
                        end_line: 5,
                        end_col: 31,
                        end_offset: 128
                    },
                    None,
                    vec![],
                    Span {
                        start_line: 3,
                        start_col: 26,
                        start_offset: 58,
                        end_line: 3,
                        end_col: 31,
                        end_offset: 63
                    }
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn test_parse_invalid_sql() {
        let sql = r#"
            {% macro unclosed() %}
                select 1
            {# Missing endmacro #}
        "#;

        let result = parse_macro_statements(sql, &PathBuf::from("test.sql"), &["macro"]);
        println!("result: {result:?}");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_unclosed_if_inside_macro_gives_rich_error() {
        // Reproducer from https://github.com/dbt-labs/dbt-fusion/issues/130
        let sql = r#"
            {% macro my_macro() %}
              {% if true %}
            {% endmacro %}
        "#;

        let err = parse_macro_statements(sql, &PathBuf::from("macros/my_macro.sql"), &["macro"])
            .unwrap_err();
        let msg = err.to_string();
        println!("error: {msg}");
        assert!(
            msg.contains("Encountered unknown tag 'endmacro'"),
            "expected 'Encountered unknown tag' in: {msg}"
        );
        assert!(
            msg.contains("innermost block that needs to be closed is 'if'"),
            "expected innermost block hint in: {msg}"
        );
        assert!(
            msg.contains("looking for"),
            "expected 'looking for' hint in: {msg}"
        );
        assert!(
            msg.contains("'endif'") || msg.contains("endif"),
            "expected 'endif' in expected tags hint: {msg}"
        );
    }

    #[test]
    fn test_parse_unclosed_for_inside_if_gives_rich_error() {
        let sql = r#"
            {% macro my_macro() %}
              {% if true %}
                {% for x in [] %}
              {% endif %}
            {% endmacro %}
        "#;

        let err = parse_macro_statements(sql, &PathBuf::from("macros/my_macro.sql"), &["macro"])
            .unwrap_err();
        let msg = err.to_string();
        println!("error: {msg}");
        assert!(
            msg.contains("Encountered unknown tag 'endif'"),
            "expected 'Encountered unknown tag' in: {msg}"
        );
        assert!(
            msg.contains("innermost block that needs to be closed is 'for'"),
            "expected innermost block hint in: {msg}"
        );
    }

    #[test]
    fn test_parse_materialization_macro() -> FsResult<()> {
        let sql_default = r#"
            {% materialization name, default %}

            {% endmaterialization %}
        "#;

        let resources = parse_macro_statements(
            sql_default,
            &PathBuf::from("test.sql"),
            &["materialization"],
        )?;
        assert_eq!(
            resources,
            vec![SqlResource::Materialization(
                "materialization_name_default".to_string(),
                "default".to_string(),
                None,
                Span {
                    start_line: 2,
                    start_col: 13,
                    end_line: 4,
                    end_col: 37,
                    start_offset: 13,
                    end_offset: 86
                },
                Span {
                    start_line: 2,
                    start_col: 32,
                    start_offset: 32,
                    end_line: 2,
                    end_col: 36,
                    end_offset: 36
                }
            )]
        );

        let sql_custom = r#"
        {% materialization name, adapter='redshift', supported_languages=['sql', 'python'] %}

        {% endmaterialization %}
    "#;

        let resources =
            parse_macro_statements(sql_custom, &PathBuf::from("test.sql"), &["materialization"])?;
        assert_eq!(
            resources,
            vec![SqlResource::Materialization(
                "materialization_name_redshift".to_string(),
                "redshift".to_string(),
                Some(vec!["sql".to_string(), "python".to_string()]),
                Span {
                    start_line: 2,
                    start_col: 9,
                    end_line: 4,
                    end_col: 33,
                    start_offset: 9,
                    end_offset: 128
                },
                Span {
                    start_line: 2,
                    start_col: 28,
                    start_offset: 28,
                    end_line: 2,
                    end_col: 32,
                    end_offset: 32
                }
            )]
        );
        Ok(())
    }

    #[dbt_runtime::test]
    async fn test_dict_update() {
        let (env, _, _) = setup_test_env();
        let env = Arc::new(env);
        let sql = r#"
        {% set my_dict = dict(
            a=1,
            b=2,
            c=3
        ) %}
        {% do my_dict.update({"d": 4, "c": 5}) %}
        {{ tojson(my_dict, sort_keys=true) }}
        "#;

        let rendered = render_sql(
            sql,
            &env,
            &BTreeMap::new(),
            &DefaultRenderingEventListenerFactory::default(),
            &PathBuf::from("test"),
        )
        .unwrap();

        let rendered = rendered.trim().replace(" ", "").replace("\n", "");
        assert_eq!(rendered, r#"{"a":1,"b":2,"c":5,"d":4}"#);
    }

    #[test]
    fn test_process_markdown_single_doc() {
        let sql = r#"
        {% docs cloud_plan_tier %}
        An identifier to group specific plans by targeted user groups.
        {% enddocs %}
        "#;

        let docs = parse_macro_statements(sql, Path::new("test.sql"), &["docs"]).unwrap();
        let doc_names: Vec<String> = docs
            .iter()
            .filter_map(|x| {
                if let SqlResource::Doc(name, _) = x {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(doc_names, vec!["cloud_plan_tier".to_string()]);
    }

    #[test]
    fn test_process_markdown_multiple_docs() {
        let sql = r#"


        {% docs cloud_plan %}
        The plan name representing the pricing and features for a given Cloud account.
        {% enddocs %}

        {% docs database_source %}
        The source Postgres database the Cloud account information comes from.
        {% enddocs %}
        "#;

        let docs = parse_macro_statements(sql, Path::new("test.sql"), &["docs"]).unwrap();
        let doc_names: Vec<String> = docs
            .iter()
            .filter_map(|x| {
                if let SqlResource::Doc(name, _) = x {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(
            doc_names,
            vec!["cloud_plan".to_string(), "database_source".to_string()]
        );
    }

    #[test]
    fn test_process_markdown_with_md_suffix() {
        let sql = r#"
        {% docs cloud_plan_tier.md %}
        An identifier to group specific plans by targeted user groups.
        {% enddocs %}
        "#;

        let docs = parse_macro_statements(sql, Path::new("test.sql"), &["docs"]).unwrap();
        let doc_names: Vec<String> = docs
            .iter()
            .filter_map(|x| {
                if let SqlResource::Doc(name, _) = x {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(doc_names, vec!["cloud_plan_tier".to_string()]);
    }

    #[test]
    fn test_snapshot_with_sql_suffix() {
        let sql = r#"
        {% snapshot stg_crm_client_role.sql %}
        SELECT 1
        {% endsnapshot %}
        "#;

        let resources = parse_macro_statements(sql, Path::new("test.sql"), &["snapshot"]).unwrap();
        let snapshot_names: Vec<String> = resources
            .iter()
            .filter_map(|x| {
                if let SqlResource::Snapshot(name, _, _) = x {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        // parse_macro_statements returns the macro name with the "snapshot_" prefix;
        // the prefix is stripped later in resolve_snapshots. The important thing
        // here is that the ".sql" suffix is absent, matching dbt-core behavior.
        assert_eq!(
            snapshot_names,
            vec!["snapshot_stg_crm_client_role".to_string()]
        );
    }

    #[test]
    fn test_process_markdown_no_docs() {
        let sql = r#"
        This is a readme.md file with {{ invalid-ish jinja }} in it
        "#;

        let docs = parse_macro_statements(sql, Path::new("test.sql"), &["docs"]).unwrap();
        assert!(docs.is_empty());
    }
    #[test]
    fn test_process_markdown_unclosed_docs() {
        let sql = r#"
    {% docs cloud_plan_tier %}
    An identifier to group specific plans by targeted user groups.
    "#;

        let res = parse_macro_statements(sql, Path::new("test.sql"), &["docs"]);
        println!("res: {res:?}");
        assert!(res.is_err());
    }

    /// Regression test for GitHub issue #998: doc block names starting with a digit
    /// previously caused a parse error: `'_' may not occur at end of number`.
    /// dbt Core allows this; Fusion should too.
    /// https://github.com/dbt-labs/dbt-fusion/issues/998
    #[test]
    fn test_process_markdown_doc_name_starting_with_digit() {
        let sql = r#"
        {% docs 3_months_prior_date %}
        The date 3 months prior to today.
        {% enddocs %}
        "#;

        let docs = parse_macro_statements(sql, Path::new("test.sql"), &["docs"]).unwrap();
        let doc_names: Vec<String> = docs
            .iter()
            .filter_map(|x| {
                if let SqlResource::Doc(name, _) = x {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(doc_names, vec!["3_months_prior_date".to_string()]);
    }

    /// dbt-core#15473: an explicit `null` at a deeper level clears the inherited
    /// value; an omitted key still inherits.
    #[test]
    fn test_null_config_clears_inherited_value_in_project_hierarchy() {
        use crate::dbt_project_config::recur_build_dbt_project_config;
        use dbt_schemas::schemas::project::ProjectModelConfig;

        let yaml = r#"
        +hours_to_expiration: 120
        cleared:
          +hours_to_expiration: null
        inherited:
          +materialized: table
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let pmc: ProjectModelConfig = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();

        let base = ModelConfig::default();
        let tree = recur_build_dbt_project_config(
            &base,
            &pmc,
            "",
            &|_variant: &dbt_yaml::ShouldBe<ProjectModelConfig>, _key: &str, _key_path: &str| {},
            false,
            AdapterType::Snowflake,
        );

        let hours = |cfg: &ModelConfig| {
            cfg.__warehouse_specific_config__
                .hours_to_expiration
                .clone()
                .into_inner()
                .flatten()
        };

        assert_eq!(hours(&tree.config), Some(StringOrInteger::Integer(120)));

        let cleared = tree.get_config_for_fqn(&["cleared".to_string()]);
        assert_eq!(
            hours(cleared),
            None,
            "explicit null must clear the inherited hours_to_expiration"
        );

        let inherited = tree.get_config_for_fqn(&["inherited".to_string()]);
        assert_eq!(
            hours(inherited),
            Some(StringOrInteger::Integer(120)),
            "omitted key must still inherit hours_to_expiration"
        );
    }

    /// Resolve a `models:` hierarchy snippet the same way the parser does.
    fn build_model_config_tree(
        yaml: &str,
    ) -> crate::dbt_project_config::DbtProjectConfig<ModelConfig> {
        use crate::dbt_project_config::recur_build_dbt_project_config;
        use dbt_schemas::schemas::project::ProjectModelConfig;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let pmc: ProjectModelConfig = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();

        let base = ModelConfig::default();
        recur_build_dbt_project_config(
            &base,
            &pmc,
            "",
            &|_variant: &dbt_yaml::ShouldBe<ProjectModelConfig>, _key: &str, _key_path: &str| {},
            false,
            AdapterType::Snowflake,
        )
    }

    fn hours_of(cfg: &ModelConfig) -> Option<StringOrInteger> {
        cfg.__warehouse_specific_config__
            .hours_to_expiration
            .clone()
            .into_inner()
            .flatten()
    }

    /// dbt-core#15473: a concrete value at a more specific level overrides the
    /// inherited value.
    #[test]
    fn test_config_child_value_overrides_inherited_in_project_hierarchy() {
        let tree = build_model_config_tree(
            r#"
        +hours_to_expiration: 120
        overridden:
          +hours_to_expiration: 240
        "#,
        );

        assert_eq!(hours_of(&tree.config), Some(StringOrInteger::Integer(120)));
        let overridden = tree.get_config_for_fqn(&["overridden".to_string()]);
        assert_eq!(
            hours_of(overridden),
            Some(StringOrInteger::Integer(240)),
            "a concrete child value must override the inherited value"
        );
    }

    /// dbt-core#15473: a `null` at one level does not stop a deeper level from
    /// setting a new concrete value.
    #[test]
    fn test_config_null_clear_then_deeper_override_in_project_hierarchy() {
        let tree = build_model_config_tree(
            r#"
        +hours_to_expiration: 120
        a:
          +hours_to_expiration: null
          b:
            +hours_to_expiration: 200
        "#,
        );

        assert_eq!(hours_of(&tree.config), Some(StringOrInteger::Integer(120)));
        let a = tree.get_config_for_fqn(&["a".to_string()]);
        assert_eq!(hours_of(a), None, "explicit null at `a` clears the value");
        let ab = tree.get_config_for_fqn(&["a".to_string(), "b".to_string()]);
        assert_eq!(
            hours_of(ab),
            Some(StringOrInteger::Integer(200)),
            "a deeper concrete value applies even after a parent cleared it"
        );
    }

    /// dbt-core#15473: an omitted key inherits the nearest ancestor's value
    /// across multiple intermediate levels that never mention it.
    #[test]
    fn test_config_inherits_through_omitted_intermediate_levels() {
        let tree = build_model_config_tree(
            r#"
        +hours_to_expiration: 120
        a:
          +materialized: table
          b:
            +materialized: view
        "#,
        );

        let a = tree.get_config_for_fqn(&["a".to_string()]);
        assert_eq!(
            hours_of(a),
            Some(StringOrInteger::Integer(120)),
            "intermediate level that omits the key inherits it"
        );
        let ab = tree.get_config_for_fqn(&["a".to_string(), "b".to_string()]);
        assert_eq!(
            hours_of(ab),
            Some(StringOrInteger::Integer(120)),
            "value inherits through multiple omitted intermediate levels"
        );
    }

    /// dbt-core#15473: `null` at the top level leaves the value unset, and a
    /// child can still set a concrete value below it.
    #[test]
    fn test_config_null_at_root_stays_cleared_child_can_reset() {
        let tree = build_model_config_tree(
            r#"
        +hours_to_expiration: null
        child:
          +hours_to_expiration: 72
        "#,
        );

        assert_eq!(
            hours_of(&tree.config),
            None,
            "explicit null at root leaves the value unset"
        );
        let child = tree.get_config_for_fqn(&["child".to_string()]);
        assert_eq!(
            hours_of(child),
            Some(StringOrInteger::Integer(72)),
            "a child may set a concrete value under a cleared root"
        );
    }

    /// dbt-core#15473: explicit-null-clears must hold for non-model resource
    /// types too. Guards the sibling `Project*Config` fix (fs#12155 review),
    /// where a plain `Option` had collapsed null and omitted to the same `None`.
    #[test]
    fn test_null_config_clears_inherited_value_for_snapshots() {
        use crate::dbt_project_config::recur_build_dbt_project_config;
        use dbt_schemas::schemas::project::{ProjectSnapshotConfig, SnapshotConfig};

        let yaml = r#"
        +hours_to_expiration: 120
        cleared:
          +hours_to_expiration: null
        inherited:
          +enabled: true
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let psc: ProjectSnapshotConfig = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();

        let base = SnapshotConfig::default();
        let tree = recur_build_dbt_project_config(
            &base,
            &psc,
            "",
            &|_variant: &dbt_yaml::ShouldBe<ProjectSnapshotConfig>, _key: &str, _key_path: &str| {},
            false,
            AdapterType::Snowflake,
        );

        let hours = |cfg: &SnapshotConfig| {
            cfg.__warehouse_specific_config__
                .hours_to_expiration
                .clone()
                .into_inner()
                .flatten()
        };

        assert_eq!(hours(&tree.config), Some(StringOrInteger::Integer(120)));

        let cleared = tree.get_config_for_fqn(&["cleared".to_string()]);
        assert_eq!(
            hours(cleared),
            None,
            "explicit null must clear inherited hours_to_expiration for snapshots"
        );

        let inherited = tree.get_config_for_fqn(&["inherited".to_string()]);
        assert_eq!(
            hours(inherited),
            Some(StringOrInteger::Integer(120)),
            "omitted key must still inherit hours_to_expiration for snapshots"
        );
    }

    /// fs#13424: a Databricks `+catalog:` config-key alias canonicalizes into `database`
    /// (dbt-core's `Credentials._ALIASES`, `catalog` -> `database`), and the
    /// warehouse-specific `catalog` field is cleared once canonicalized. Gated on adapter type
    /// (D1): the same project on Snowflake -- which has no such alias -- keeps `catalog` as an
    /// ordinary extra config key and leaves `database` unset.
    #[test]
    fn test_databricks_catalog_alias_canonicalizes_to_database() {
        use crate::dbt_project_config::recur_build_dbt_project_config;
        use dbt_schemas::schemas::project::ProjectModelConfig;

        let yaml = r#"
        +catalog: my_catalog
        +catalog_name: unrelated_catalog_name
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let build_tree = |adapter_type: AdapterType| {
            let pmc: ProjectModelConfig = dbt_jinja_utils::serde::into_typed_with_jinja(
                val.clone(),
                false,
                &env,
                &ctx,
                &listeners,
                None,
                true,
            )
            .unwrap();
            recur_build_dbt_project_config(
                &ModelConfig::default(),
                &pmc,
                "",
                &|_variant: &dbt_yaml::ShouldBe<ProjectModelConfig>,
                  _key: &str,
                  _key_path: &str| {},
                false,
                adapter_type,
            )
        };

        let databricks_tree = build_tree(AdapterType::Databricks);
        assert_eq!(
            databricks_tree
                .config
                .database
                .clone()
                .into_inner()
                .flatten(),
            Some("my_catalog".to_string()),
            "Databricks `+catalog:` must canonicalize into `database`"
        );
        assert_eq!(
            databricks_tree.config.__warehouse_specific_config__.catalog, None,
            "the warehouse-specific `catalog` field must be cleared once canonicalized"
        );
        assert_eq!(
            databricks_tree.config.catalog_name,
            Some("unrelated_catalog_name".to_string()),
            "an unrelated `catalog_name` field must be untouched by canonicalization"
        );

        let snowflake_tree = build_tree(AdapterType::Snowflake);
        assert_eq!(
            snowflake_tree
                .config
                .database
                .clone()
                .into_inner()
                .flatten(),
            None,
            "on Snowflake, `+catalog:` must not canonicalize into `database`"
        );
        assert_eq!(
            snowflake_tree.config.__warehouse_specific_config__.catalog,
            Some("my_catalog".to_string()),
            "on Snowflake, `catalog` stays an inert extra config key"
        );
    }

    /// fs#13424: dbt-core's `credentials.translate_aliases` canonicalizes each config *source* --
    /// here, the project-level `dbt_project.yml` subtree and the model-level properties/inline
    /// layer -- independently, before ordinary `default_to` precedence combines them. So a
    /// project-level `+catalog:` and a model-level `database:` (or the reverse) must resolve to
    /// the model-level value either way, never to a fold applied once after merging.
    #[test]
    fn test_databricks_catalog_alias_mixed_spelling_precedence() {
        use crate::dbt_project_config::{ProjectConfigResolver, recur_build_dbt_project_config};
        use dbt_common::serde_utils::Omissible;
        use dbt_schemas::schemas::project::ProjectModelConfig;

        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let project_tree = |yaml: &str| {
            let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
            let pmc: ProjectModelConfig = dbt_jinja_utils::serde::into_typed_with_jinja(
                val, false, &env, &ctx, &listeners, None, true,
            )
            .unwrap();
            recur_build_dbt_project_config(
                &ModelConfig::default(),
                &pmc,
                "",
                &|_variant: &dbt_yaml::ShouldBe<ProjectModelConfig>,
                  _key: &str,
                  _key_path: &str| {},
                false,
                AdapterType::Databricks,
            )
        };
        let fqn = vec!["my_model".to_string()];

        // Project-level `+catalog:` (canonicalizes to `database`); model-level `database:` set
        // directly. The model-level layer wins by ordinary precedence.
        let root = project_tree("+catalog: project_catalog\n");
        let resolver = ProjectConfigResolver::for_root(root, AdapterType::Databricks);
        let model_level_database = ModelConfig {
            database: Omissible::Present(Some("model_database".to_string())),
            ..Default::default()
        };
        let resolved = resolver.with_configs(&fqn, &[Some(&model_level_database)]);
        assert_eq!(
            resolved.database.into_inner().flatten(),
            Some("model_database".to_string()),
            "model-level `database:` must win over a project-level `+catalog:`"
        );

        // The reverse: project-level `database:` set directly; model-level `+catalog:` --
        // authored via inline/properties config, so already the typed `ModelConfig` shape --
        // canonicalizes to `database` and must still win.
        let root = project_tree("+database: project_database\n");
        let resolver = ProjectConfigResolver::for_root(root, AdapterType::Databricks);
        let mut model_level_catalog = ModelConfig::default();
        model_level_catalog.__warehouse_specific_config__.catalog =
            Some("model_catalog".to_string());
        let resolved = resolver.with_configs(&fqn, &[Some(&model_level_catalog)]);
        assert_eq!(
            resolved.database.into_inner().flatten(),
            Some("model_catalog".to_string()),
            "model-level `catalog` (canonicalized) must win over a project-level `database:`"
        );
    }

    /// Residual (fs#13424 phase 2): `canonicalize_adapter_aliases` runs with the *target's
    /// default* adapter, threaded once per `resolve_*.rs` call, before a node's own `+adapter:`
    /// override (a real, mergeable `ModelConfig.adapter` field -- Fusion's multi-adapter/mesh
    /// dispatch, `resolve_utils.rs::validate_node_adapter`) is known. That override can only be
    /// read *from* the merged config, so canonicalizing per-layer before merging (required for
    /// D3's mixed-spelling precedence above) and keying it to the node's own resolved adapter
    /// are in tension -- resolving both needs a two-pass merge (discover `.adapter`, then
    /// re-merge with the right alias table), which this phase does not attempt. Pre-existing:
    /// the five deleted `RelationComponents.database` special cases and phase 1's
    /// `build_unrendered_config` gate on the exact same target-default `adapter_type`, so a
    /// `+adapter:`-overridden node was never correctly handled either; this residual just
    /// documents it rather than fixing it. `#[ignore]`d because it pins the gap, not the
    /// desired behavior.
    #[test]
    #[ignore = "fs#13424 phase 2 residual: canonicalize_adapter_aliases is keyed to the \
                target's default adapter, not a node's own +adapter: override (see \
                ResolvableConfig::canonicalize_adapter_aliases)"]
    fn test_databricks_catalog_alias_not_canonicalized_for_adapter_overridden_node() {
        use crate::dbt_project_config::{DbtProjectConfig, ProjectConfigResolver};

        let root = DbtProjectConfig::<ModelConfig> {
            config: ModelConfig::default(),
            children: indexmap::IndexMap::new(),
        };
        // The package/target default is Snowflake; this node overrides to Databricks via
        // `+adapter:`, which is only readable *after* this same merge produces it.
        let resolver = ProjectConfigResolver::for_root(root, AdapterType::Snowflake);
        let mut model_level_databricks_override = ModelConfig {
            adapter: Some(AdapterType::Databricks),
            ..Default::default()
        };
        model_level_databricks_override
            .__warehouse_specific_config__
            .catalog = Some("my_catalog".to_string());

        let fqn = vec!["my_model".to_string()];
        let resolved = resolver.with_configs(&fqn, &[Some(&model_level_databricks_override)]);

        // Desired (not delivered): `database == Some("my_catalog")`, since the node actually
        // runs on Databricks. Actual: canonicalization ran keyed to Snowflake (a no-op), so
        // `database` stays unset and `catalog` stays an un-canonicalized extra key.
        assert_eq!(resolved.database.into_inner().flatten(), None);
        assert_eq!(
            resolved.__warehouse_specific_config__.catalog,
            Some("my_catalog".to_string())
        );
    }

    /// Residual (fs#13424): the `+dataset` / `+project` / `+data_space` serde aliases on
    /// `ProjectModelConfig` (and its `Seed`/`Snapshot`/`DataTest`/`Function` siblings) are
    /// **ungated** -- they route a value to `schema`/`database` on every adapter, where dbt-core
    /// only translates an alias the *active* adapter's `Credentials._ALIASES` declares. On
    /// Snowflake, dbt-core keeps `dataset` as an inert extra config key and leaves `schema` unset;
    /// Fusion sets `schema`, which changes the rendered relation. Predates the gated map in
    /// `dbt_adapter_core::config_aliases` (a no-op for these, since serde has already moved the
    /// value by the time it runs) and is left alone deliberately: gating them is behavior-changing
    /// for existing non-BigQuery projects and needs its own conformance evidence. Note the two
    /// paths also disagree with each other -- `build_unrendered_config` canonicalizes *gated*, so
    /// on Snowflake Stage 1 sees a `dataset` key while Stage 2 sees the rendered `schema`.
    /// `#[ignore]`d because it pins the gap, not the desired behavior; the reasoning is in
    /// `.agents/plans/completed/2026-08-20-adapter-config-alias-canonicalization/`.
    #[test]
    #[ignore = "fs#13424 residual: the +dataset/+project/+data_space serde aliases are ungated, \
                unlike dbt-core's per-adapter _ALIASES map"]
    fn test_ungated_serde_dataset_alias_applies_on_every_adapter() {
        use crate::dbt_project_config::recur_build_dbt_project_config;
        use dbt_schemas::schemas::project::ProjectModelConfig;

        let val: dbt_yaml::Value = dbt_yaml::from_str("+dataset: my_dataset\n").unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let pmc: ProjectModelConfig = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();
        let tree = recur_build_dbt_project_config(
            &ModelConfig::default(),
            &pmc,
            "",
            &|_variant: &dbt_yaml::ShouldBe<ProjectModelConfig>, _key: &str, _key_path: &str| {},
            false,
            // Snowflake declares no `_ALIASES` at all, so dbt-core translates nothing here.
            AdapterType::Snowflake,
        );

        // Desired (not delivered): `schema` unset, `dataset` carried as an extra config key.
        assert_eq!(
            tree.config.schema.into_inner().flatten(),
            Some("my_dataset".to_string()),
            "`+dataset:` is routed to `schema` even on an adapter with no such alias"
        );
    }

    /// fs#13424 (surfaced by a phase-3 self-review): dbt-core's `Translator.translate_mapping`
    /// raises a `DuplicateAliasError` when a single config source authors both an alias and its
    /// canonical key (D4) -- `+catalog:` and `+database:` at the same `dbt_project.yml`/schema.yml
    /// layer is that same shape. The raw-dict paths already raise a hard `FsError` for it
    /// (`build_unrendered_config`'s `canonicalize_source_config_keys`, both inline-config layers);
    /// `take_databricks_catalog_alias`'s typed-struct fold can't return a `Result` without
    /// cascading through `ResolvableConfig::canonicalize_adapter_aliases` and every one of
    /// `ProjectConfigResolver::{apply_root_overlay, with_configs}`'s ~25 callers, so it instead
    /// emits a hard parse-time error via `emit_error_log_message` -- same effect (the run fails),
    /// no signature change anywhere. This test proves the emission actually fires, using the
    /// `TestLayer` tracing-capture harness (`dbt_common::tracing::tests::dbt_emit_tests` in
    /// `dbt-common` uses the identical pattern for the same convenience functions).
    #[test]
    fn test_databricks_catalog_alias_duplicate_at_same_layer_errors() {
        use crate::dbt_project_config::{DbtProjectConfig, ProjectConfigResolver};
        use dbt_common::ErrorCode;
        use dbt_common::serde_utils::Omissible;
        use dbt_common::tracing::fs_error_log::get_log_message;
        use dbt_common::tracing::layer::ConsumerLayer;
        use dbt_tracing::emit::create_root_info_span;
        use dbt_tracing::init::create_tracing_subcriber_with_layer;
        use dbt_tracing::test_support::mocks::{MockDynSpanEvent, TestLayer, test_data_layer};
        use dbt_tracing::{SeverityNumber, TelemetryOutputFlags};

        let root = DbtProjectConfig::<ModelConfig> {
            config: ModelConfig::default(),
            children: indexmap::IndexMap::new(),
        };
        let resolver = ProjectConfigResolver::for_root(root, AdapterType::Databricks);

        // Both spellings authored at the same layer -- the same shape dbt-core rejects outright.
        let mut same_layer_duplicate = ModelConfig {
            database: Omissible::Present(Some("explicit_database".to_string())),
            ..Default::default()
        };
        same_layer_duplicate.__warehouse_specific_config__.catalog =
            Some("aliased_catalog".to_string());
        let fqn = vec!["my_model".to_string()];

        let trace_id = 0x13424_u128;
        let (test_layer, _, _, log_records) = TestLayer::new();
        let subscriber = create_tracing_subcriber_with_layer(
            tracing::level_filters::LevelFilter::TRACE,
            test_data_layer(
                trace_id,
                None,
                false,
                std::iter::empty(),
                std::iter::once(Box::new(test_layer) as ConsumerLayer),
            ),
            &[],
        )
        .expect("tracing filter directives must be valid");

        let resolved = tracing::subscriber::with_default(subscriber, || {
            let _rs = create_root_info_span(MockDynSpanEvent {
                name: "root".to_string(),
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            })
            .entered();
            resolver.with_configs(&fqn, &[Some(&same_layer_duplicate)])
        });

        // `database` still wins by ordinary same-layer precedence and `catalog` is left in place
        // (not cleared) so the emitted message and the config both point at it -- but unlike the
        // pre-fix behavior, the run now also fails.
        assert_eq!(
            resolved.database.into_inner().flatten(),
            Some("explicit_database".to_string())
        );
        assert_eq!(
            resolved.__warehouse_specific_config__.catalog,
            Some("aliased_catalog".to_string())
        );

        let log_records = log_records.lock().expect("should have no locks").clone();
        let error_event = log_records
            .iter()
            .find(|r| r.body.contains("both resolve to `database`"))
            .expect("expected a hard error for the same-layer catalog/database duplicate");
        assert_eq!(error_event.severity_number, SeverityNumber::Error);
        let lm = get_log_message(&error_event.attributes)
            .expect("expected LogMessage attributes on the emitted error");
        assert_eq!(lm.code, Some(ErrorCode::InvalidConfig as u32));
    }

    // --- dbt-core#15957: state:modified diagnostic logging must carry compared values ---
    //
    // `log_state_mod_diff` (`dbt-schemas`) emits a `StateModifiedDiff` TRACE event per failed
    // `state:modified` sub-check. Several call sites pass `None` for the compared-values slot
    // even though real values are on hand, so trace logs/OTEL never show *what* differed — only
    // *that* something did. `check_relation_modified` is the working baseline (real `(current,
    // previous)` tuples); `check_body_modified` and `check_contract_modified` are the reported
    // and a sibling gap, respectively. These use the same `TestLayer` tracing-capture harness as
    // `test_databricks_catalog_alias_duplicate_at_same_layer_errors` above, driven through
    // `StateArtifacts::is_modified` — the only public entry point into these checks.

    /// Runs `f` under a subscriber that captures every emitted `StateModifiedDiff` telemetry
    /// event, at TRACE level (matching `--log-level-file trace`, the level `log_state_mod_diff`
    /// itself gates on).
    fn capture_state_mod_diffs<F: FnOnce()>(f: F) -> Vec<dbt_telemetry::StateModifiedDiff> {
        use dbt_tracing::layer::ConsumerLayer;
        use dbt_tracing::test_support::mocks::{MockDynSpanEvent, TestLayer, test_data_layer};
        use dbt_tracing::{emit::create_root_info_span, init::create_tracing_subcriber_with_layer};

        let (test_layer, _, _, log_records) = TestLayer::new();
        let subscriber = create_tracing_subcriber_with_layer(
            tracing::level_filters::LevelFilter::TRACE,
            test_data_layer(
                1,
                None,
                false,
                std::iter::empty(),
                std::iter::once(Box::new(test_layer) as ConsumerLayer),
            ),
            &[],
        )
        .expect("test tracing filter directives must be valid");

        tracing::subscriber::with_default(subscriber, || {
            let _rs = create_root_info_span(MockDynSpanEvent {
                name: "root".to_string(),
                flags: dbt_tracing::TelemetryOutputFlags::ALL,
                ..Default::default()
            })
            .entered();
            f();
        });

        log_records
            .lock()
            .expect("should have no locks")
            .iter()
            .filter_map(|r| {
                r.attributes
                    .downcast_ref::<dbt_telemetry::StateModifiedDiff>()
                    .cloned()
            })
            .collect()
    }

    /// RED: reported bug. A genuine body change must be logged with the actual (current,
    /// previous) checksums so a customer/support engineer can see *why* the node was selected —
    /// today `check_body_modified` always logs `self_value = other_value = None`.
    #[test]
    fn test_state_modified_body_check_logs_checksum_values() {
        use dbt_schemas::schemas::common::DbtChecksum;
        use dbt_schemas::schemas::{DbtModel, ModificationType, Nodes, StateArtifacts};
        use std::sync::Arc;

        let mut previous = DbtModel::default();
        previous.__common_attr__.unique_id = "model.pkg.my_model".to_string();
        previous.__common_attr__.checksum = DbtChecksum::String("aaa".to_string());

        let mut current = previous.clone();
        current.__common_attr__.checksum = DbtChecksum::String("bbb".to_string());

        let mut nodes = Nodes::default();
        nodes.models.insert(
            previous.__common_attr__.unique_id.clone(),
            Arc::new(previous),
        );
        let state = StateArtifacts::from_previous_nodes(nodes);

        let diffs = capture_state_mod_diffs(|| {
            assert!(
                state.is_modified(
                    &current,
                    Some(ModificationType::Body),
                    None,
                    AdapterType::Snowflake
                ),
                "body checksum differs"
            );
        });

        let body_diff = diffs
            .iter()
            .find(|d| d.check == "body")
            .expect("check_body_modified must emit a StateModifiedDiff for the body check");
        assert!(
            body_diff.self_value.is_some() && body_diff.other_value.is_some(),
            "expected the current/previous checksums to be logged, got self_value={:?} \
             other_value={:?} — dbt-core#15957",
            body_diff.self_value,
            body_diff.other_value,
        );
    }

    /// RED: the same gap recurs for the contract check — `check_contract_modified` always logs
    /// `None`, even though `same_contract`'s caller has both contract states in hand. This shows
    /// the reported issue is not specific to `state:modified.body`: any sub-check that only logs
    /// a bare boolean loses the same diagnostic information.
    #[test]
    fn test_state_modified_contract_check_logs_no_values_same_gap_as_body() {
        use dbt_schemas::schemas::common::DbtContract;
        use dbt_schemas::schemas::{DbtModel, ModificationType, Nodes, StateArtifacts};
        use std::sync::Arc;

        let mut previous = DbtModel::default();
        previous.__common_attr__.unique_id = "model.pkg.my_model".to_string();
        previous.__model_attr__.contract = None;

        let mut current = previous.clone();
        current.__model_attr__.contract = Some(DbtContract {
            alias_types: true,
            enforced: true,
            checksum: None,
        });

        let mut nodes = Nodes::default();
        nodes.models.insert(
            previous.__common_attr__.unique_id.clone(),
            Arc::new(previous),
        );
        let state = StateArtifacts::from_previous_nodes(nodes);

        let diffs = capture_state_mod_diffs(|| {
            assert!(
                state.is_modified(
                    &current,
                    Some(ModificationType::Contract),
                    None,
                    AdapterType::Snowflake
                ),
                "contract was newly enforced"
            );
        });

        let contract_diff = diffs
            .iter()
            .find(|d| d.check == "contract")
            .expect("check_contract_modified must emit a StateModifiedDiff for the contract check");
        assert!(
            contract_diff.self_value.is_some() && contract_diff.other_value.is_some(),
            "expected the current/previous contract state to be logged, got self_value={:?} \
             other_value={:?} — same diagnostic gap as dbt-core#15957, just for `contract` \
             instead of `body`",
            contract_diff.self_value,
            contract_diff.other_value,
        );
    }

    /// GREEN control: `check_relation_modified` already does this correctly — it logs real
    /// `(current, previous)` tuples for `database`/`schema`/`alias`. This is the pattern
    /// `check_body_modified`/`check_contract_modified` should be brought in line with.
    #[test]
    fn test_state_modified_relation_check_already_logs_real_values() {
        use dbt_schemas::schemas::{DbtModel, ModificationType, Nodes, StateArtifacts};
        use std::collections::BTreeMap;
        use std::sync::Arc;

        let mut previous = DbtModel::default();
        previous.__common_attr__.unique_id = "model.pkg.my_model".to_string();
        previous.__base_attr__.unrendered_config = BTreeMap::from([(
            "schema".to_string(),
            dbt_yaml::from_str::<dbt_yaml::Value>("\"prod\"").expect("valid yaml value"),
        )]);

        let mut current = previous.clone();
        current.__base_attr__.unrendered_config = BTreeMap::from([(
            "schema".to_string(),
            dbt_yaml::from_str::<dbt_yaml::Value>("\"other\"").expect("valid yaml value"),
        )]);

        let mut nodes = Nodes::default();
        nodes.models.insert(
            previous.__common_attr__.unique_id.clone(),
            Arc::new(previous),
        );
        let state = StateArtifacts::from_previous_nodes(nodes);

        let diffs = capture_state_mod_diffs(|| {
            assert!(
                state.is_modified(
                    &current,
                    Some(ModificationType::Relation),
                    None,
                    AdapterType::Snowflake
                ),
                "schema differs"
            );
        });

        let schema_diff = diffs
            .iter()
            .find(|d| d.check == "schema")
            .expect("check_relation_modified must emit a StateModifiedDiff for the schema check");
        assert_eq!(schema_diff.self_value.as_deref(), Some("Some(\"other\")"));
        assert_eq!(schema_diff.other_value.as_deref(), Some("Some(\"prod\")"));
    }

    /// RED (pre-fix): analyses had *no* content-modified logging at all — unlike models, which at
    /// least logged a bare boolean before the dbt-core#15957 fix, `DbtAnalysis::has_same_content`
    /// never called `log_state_mod_diff`. There is no dedicated `ModificationType` for the content
    /// check, so it's only reached via the default `Any` chain (`is_modified(node, None, ...)`),
    /// which is also how it fires for a real edited analysis SQL file.
    #[test]
    fn test_state_modified_analysis_content_check_logs_checksum_values() {
        use dbt_schemas::schemas::common::DbtChecksum;
        use dbt_schemas::schemas::{DbtAnalysis, Nodes, StateArtifacts};
        use std::sync::Arc;

        let mut previous = DbtAnalysis::default();
        previous.__common_attr__.unique_id = "analysis.pkg.my_analysis".to_string();
        previous.__common_attr__.checksum = DbtChecksum::String("aaa".to_string());

        let mut current = previous.clone();
        current.__common_attr__.checksum = DbtChecksum::String("bbb".to_string());

        let mut nodes = Nodes::default();
        nodes.analyses.insert(
            previous.__common_attr__.unique_id.clone(),
            Arc::new(previous),
        );
        let state = StateArtifacts::from_previous_nodes(nodes);

        let diffs = capture_state_mod_diffs(|| {
            assert!(
                state.is_modified(&current, None, None, AdapterType::Snowflake),
                "content checksum differs"
            );
        });

        let content_diff = diffs.iter().find(|d| d.check == "content").expect(
            "check_modified_content (via DbtAnalysis::has_same_content) must emit a \
             StateModifiedDiff for the content check",
        );
        assert!(
            content_diff.self_value.is_some() && content_diff.other_value.is_some(),
            "expected the current/previous checksums to be logged, got self_value={:?} \
             other_value={:?} — same diagnostic gap as dbt-core#15957, for analyses' content \
             check instead of models' body check",
            content_diff.self_value,
            content_diff.other_value,
        );
    }

    /// A doc block whose name is not an identifier is skipped, but the other
    /// blocks in the same file must still be registered. Dropping them made
    /// every `doc()` reference in the project render as a missing-doc
    /// placeholder, which then reached the warehouse as a column COMMENT.
    #[test]
    fn test_process_markdown_invalid_doc_name_skips_only_that_block() {
        let sql = r#"
        {% docs cloud_plan %}
        The plan name representing the pricing and features for a given Cloud account.
        {% enddocs %}

        {% docs *** end of list, for new entries insert rows above this line *** %}
        *** END OF LIST ***
        {% enddocs %}

        {% docs database_source %}
        The source Postgres database the Cloud account information comes from.
        {% enddocs %}
        "#;

        let docs = parse_macro_statements(sql, Path::new("test.md"), &["docs"]).unwrap();
        let doc_names: Vec<String> = docs
            .iter()
            .filter_map(|x| {
                if let SqlResource::Doc(name, _) = x {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(
            doc_names,
            vec!["cloud_plan".to_string(), "database_source".to_string()]
        );
    }

    /// dbt-core matches end tags on the block type alone, so `{% enddocs name %}`
    /// closes the block. Rejecting the extra name discarded every doc in the file.
    #[test]
    fn test_process_markdown_named_enddocs() {
        let sql = r#"
        {% docs cloud_plan %}
        The plan name representing the pricing and features for a given Cloud account.
        {% enddocs cloud_plan %}

        {% docs database_source %}
        The source Postgres database the Cloud account information comes from.
        {% enddocs %}
        "#;

        let docs = parse_macro_statements(sql, Path::new("test.md"), &["docs"]).unwrap();
        let doc_names: Vec<String> = docs
            .iter()
            .filter_map(|x| {
                if let SqlResource::Doc(name, _) = x {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(
            doc_names,
            vec!["cloud_plan".to_string(), "database_source".to_string()]
        );
    }

    #[test]
    fn test_databricks_python_environment_config_keys_are_recognized() {
        let yaml = r#"
        +environment_key: test_key
        +environment_dependencies:
          - requests
        +submission_method: serverless_cluster
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let cfg: ProjectModelConfig = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();

        assert_eq!(cfg.environment_key.as_deref(), Some("test_key"));
        assert_eq!(
            cfg.environment_dependencies.as_deref(),
            Some(["requests".to_string()].as_slice())
        );
        assert_eq!(cfg.submission_method.as_deref(), Some("serverless_cluster"));
    }

    #[test]
    fn test_databricks_python_environment_config_keys_on_model_config() {
        let yaml = r#"
        environment_key: test_key
        environment_dependencies:
          - requests
        submission_method: serverless_cluster
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        let cfg: ModelConfig = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();

        assert_eq!(cfg.environment_key.as_deref(), Some("test_key"));
        assert_eq!(
            cfg.environment_dependencies.as_deref(),
            Some(["requests".to_string()].as_slice())
        );
        assert_eq!(cfg.submission_method.as_deref(), Some("serverless_cluster"));
    }

    #[test]
    fn test_unknown_python_config_key_is_still_unused() {
        // schema.yml config keys (no `+` prefix). ProjectModelConfig absorbs
        // unknown `+` keys as nested folder configs via __additional_properties__.
        let yaml = r#"
        environment_key: test_key
        not_a_real_python_config: true
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let mut unused_keys = Vec::new();
        let cfg: ModelConfig = val
            .into_typed(
                |path, key, _| {
                    let key_repr =
                        dbt_yaml::to_string(key).unwrap_or_else(|_| "<opaque>".to_string());
                    unused_keys.push((path.to_string(), key_repr));
                },
                |_| Ok(None),
            )
            .unwrap();

        assert_eq!(cfg.environment_key.as_deref(), Some("test_key"));
        assert!(
            unused_keys
                .iter()
                .any(|(_, key)| key.contains("not_a_real_python_config")),
            "expected unused key warning, got {unused_keys:?}"
        );
        assert!(
            unused_keys
                .iter()
                .all(|(_, key)| !key.contains("environment_key")),
            "environment_key should not be reported unused, got {unused_keys:?}"
        );
    }

    #[test]
    fn test_literal_block_scalar_names_drop_trailing_newline() {
        use dbt_schemas::schemas::properties::{
            MinimalSchemaValue, MinimalTableValue, SourceProperties, Tables,
        };
        use dbt_schemas::schemas::serde::strip_one_trailing_newline_at_keys;

        // YAML hands a literal block scalar to serde with its final newline intact. dbt-core
        // loses it because every schema-yaml string goes through `get_rendered(..., native=True)`,
        // which never takes the no-jinja fast path and so always runs the Jinja lexer
        // (`keep_trailing_newline=False`). Fusion's fast path returns the string untouched, so the
        // newline has to be dropped -- otherwise it lands in the source identity and
        // `source('declared_source', 'rates')` cannot resolve it (#13842).
        let yaml = r#"
        name: |
          declared_source
        schema: |
          raw
        database: |
          analytics
        tables:
          - name: |
              rates
            identifier: |
              raw_rates
        "#;

        let mut val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        strip_one_trailing_newline_at_keys(&mut val, &["name", "schema", "database", "catalog"]);
        let source: SourceProperties = dbt_jinja_utils::serde::into_typed_with_jinja(
            val.clone(),
            false,
            &env,
            &ctx,
            &listeners,
            None,
            true,
        )
        .unwrap();
        assert_eq!(source.name, "declared_source");
        assert_eq!(source.schema.as_deref(), Some("raw"));
        assert_eq!(source.database.as_deref(), Some("analytics"));
        // `tables` is nested, so the entries are stripped where they are deserialized instead.
        let table = &source.tables.expect("tables")[0];
        assert_eq!(table.name, "rates\n");

        // The same YAML is also read as a `MinimalSchemaValue` to build the resolver keys, and
        // that name has to agree with the one above or the source is registered under one key
        // and looked up under another.
        let minimal: MinimalSchemaValue = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();
        assert_eq!(minimal.name, "declared_source");

        let mut table_val: dbt_yaml::Value =
            dbt_yaml::from_str("name: |\n  rates\nidentifier: |\n  raw_rates\n").unwrap();
        strip_one_trailing_newline_at_keys(&mut table_val, &["name", "identifier"]);
        let minimal_table: MinimalTableValue = dbt_jinja_utils::serde::into_typed_with_jinja(
            table_val.clone(),
            false,
            &env,
            &ctx,
            &listeners,
            None,
            true,
        )
        .unwrap();
        assert_eq!(minimal_table.name.as_ref(), "rates");
        // The strip rebuilds the scalar with its original span, so diagnostics still point at it.
        assert_ne!(minimal_table.name.span(), &dbt_yaml::Span::default());

        let table: Tables = dbt_jinja_utils::serde::into_typed_with_jinja(
            table_val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();
        assert_eq!(table.name, "rates");
        assert_eq!(table.identifier.as_deref(), Some("raw_rates"));
    }

    #[test]
    fn test_jinja_produced_trailing_newline_survives() {
        use dbt_schemas::schemas::properties::SourceProperties;
        use dbt_schemas::schemas::serde::strip_one_trailing_newline_at_keys;

        // dbt-core strips the newline off the *template source*, then evaluates, so a newline
        // that the expression itself returns is kept. Stripping after rendering would eat it and
        // diverge from Core in the opposite direction -- hence the pre-render pass above.
        // `schema` is a plain scalar, `database` a block scalar wrapping the same expression;
        // Core renders both to a value that still ends in `\n`.
        let yaml = r#"
        name: declared_source
        schema: "{{ 'raw' ~ '\n' }}"
        database: |
          {{ 'analytics' ~ '\n' }}
        "#;

        let mut val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let (env, _sql_resources, _init_cfg) = setup_test_env();
        let ctx: BTreeMap<String, Value> = BTreeMap::new();
        let listeners: Vec<Rc<dyn minijinja::listener::RenderingEventListener>> = Vec::new();

        strip_one_trailing_newline_at_keys(&mut val, &["name", "schema", "database", "catalog"]);
        let source: SourceProperties = dbt_jinja_utils::serde::into_typed_with_jinja(
            val, false, &env, &ctx, &listeners, None, true,
        )
        .unwrap();
        assert_eq!(source.name, "declared_source");
        assert_eq!(source.schema.as_deref(), Some("raw\n"));
        assert_eq!(source.database.as_deref(), Some("analytics\n"));
    }
}
