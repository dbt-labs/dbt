#[cfg(test)]
mod tests {
    use crate::args::ResolveArgs;
    use crate::dbt_project_config::{DbtProjectConfig, ProjectConfigResolver};
    use crate::renderer::{RenderCtx, RenderCtxInner, render_unresolved_sql_files};
    use dbt_adapter_core::AdapterType;
    use dbt_common::io_args::{FsCommand, IoArgs};
    use dbt_common::serde_utils::Omissible;
    use dbt_jinja_utils::jinja_environment::JinjaEnv;
    use dbt_jinja_utils::listener::DefaultJinjaTypeCheckEventListenerFactory;
    use dbt_schemas::filter::RunFilter;
    use dbt_schemas::schemas::common::DbtQuoting;
    use dbt_schemas::schemas::project::ModelConfig;
    use dbt_schemas::schemas::properties::ModelProperties;
    use dbt_schemas::state::{DbtAsset, DbtRuntimeConfig};
    use indexmap::IndexMap;
    use minijinja::Environment;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Test that verifies root project config overrides work correctly in both
    /// sequential and parallel rendering modes by actually calling render_unresolved_sql_files.
    ///
    /// Parser parallelism is gated solely by `no_parallel`; `num_threads`
    /// carries the connection-pool size and must not affect render concurrency.
    #[dbt_runtime::test]
    async fn test_render_unresolved_sql_files_config_override() {
        // Set up a temporary directory and create a test SQL file
        let temp_dir = tempfile::TempDir::new().unwrap();
        let base_path = temp_dir.path().to_path_buf();
        let models_dir = base_path.join("models");
        std::fs::create_dir_all(&models_dir).unwrap();

        let sql_file = models_dir.join("test_model.sql");
        std::fs::write(&sql_file, "SELECT 1 as test").unwrap();

        let path = PathBuf::from("models/test_model.sql");
        // Create the DbtAsset
        let test_asset = DbtAsset {
            base_path: base_path.clone(),
            original_path: path.clone(),
            path: path.clone(),
            package_name: "test_package".to_string(),
        };

        // Create configs - simulating a package with its own config and root project override
        let package_cfg = ModelConfig {
            enabled: Some(true),
            schema: Omissible::Present(Some("package_schema".to_string())),
            quoting: Some(DbtQuoting::default()),
            ..Default::default()
        };
        let package_config = DbtProjectConfig::<ModelConfig> {
            config: package_cfg,
            children: IndexMap::new(),
        };

        let root_cfg = ModelConfig {
            enabled: Some(true),
            schema: Omissible::Present(Some("root_override_schema".to_string())),
            quoting: Some(DbtQuoting::default()),
            ..Default::default()
        };
        let root_config = DbtProjectConfig::<ModelConfig> {
            config: root_cfg,
            children: IndexMap::new(),
        };

        // Set up Jinja environment
        let env = Environment::new();
        let jinja_env = Arc::new(JinjaEnv::new(env));

        // Create the arguments
        let args = ResolveArgs {
            io: IoArgs {
                in_dir: base_path.clone(),
                out_dir: base_path.clone(),
                ..Default::default()
            },
            num_threads: Some(1), // Connection-pool size; parser parallelism is unaffected.
            no_parallel: true,
            command: FsCommand::Test,
            vars: BTreeMap::new(),
            from_main: false,
            selector: None,
            select: None,
            indirect_selection: None,
            exclude: None,
            replay: None,
            sample_config: RunFilter::default(),
            sample_renaming: BTreeMap::new(),
            static_analysis: Some(dbt_common::io_args::StaticAnalysisKind::Strict),
            store_failures: false,
            skip_creating_generic_tests: false,
            adapter_override: None,
            profile_adapter_types: None,
            maximum_seed_size_mib: 1,
        };

        // Create base context with minimal required values
        let mut base_ctx = BTreeMap::new();
        base_ctx.insert(
            "project_name".to_string(),
            minijinja::Value::from("test_package"),
        );

        // Create the render context
        let render_ctx = RenderCtx {
            inner: Arc::new(RenderCtxInner {
                args: args.clone(),
                base_ctx,
                root_project_name: "root_project".to_string(),
                package_name: "test_package".to_string(), // Different from root - this triggers the override logic
                adapter_type: AdapterType::Postgres,
                database: "test_db".to_string(),
                schema: "default_schema".to_string(),
                config_resolver: ProjectConfigResolver::for_dependency(
                    package_config,
                    root_config,
                    AdapterType::Postgres,
                ),
                resource_paths: vec!["models".to_string()],
                package_quoting: DbtQuoting {
                    database: Some(true),
                    schema: Some(true),
                    identifier: Some(true),
                    snowflake_ignore_case: Some(false),
                },
                uses_snapshot_fqn: false,
                defer_render_errors_to_compile: false,
            }),
            jinja_env: jinja_env.clone(),
            runtime_config: Arc::new(DbtRuntimeConfig::default()),
            root_runtime_config: Arc::new(DbtRuntimeConfig::default()),
        };

        // Create a cancellation token
        use dbt_common::cancellation::CancellationToken;
        let token = CancellationToken::never_cancels();

        // Test 1: Sequential rendering (no_parallel = true)
        let mut node_properties = BTreeMap::new();
        let seq_results = render_unresolved_sql_files::<ModelConfig, ModelProperties>(
            &render_ctx,
            &[test_asset],
            &mut node_properties,
            &token,
            Arc::new(DefaultJinjaTypeCheckEventListenerFactory::default()),
        )
        .await
        .unwrap();

        assert_eq!(seq_results.len(), 1, "Should have one result");
        let seq_schema = match &seq_results[0].config.schema {
            Omissible::Present(Some(s)) => s.clone(),
            _ => panic!("Expected schema to be present in sequential result"),
        };

        // Test 2: Parallel rendering (no_parallel = false, and enough files to trigger parallel)
        let mut parallel_ctx = render_ctx.clone();
        Arc::make_mut(&mut parallel_ctx.inner).args.no_parallel = false;

        // Create 60 files to ensure we exceed the 50-file threshold for parallel processing
        let mut many_assets = vec![];
        for i in 0..60 {
            let sql_file = models_dir.join(format!("model_{i}.sql"));
            std::fs::write(&sql_file, format!("SELECT {i} as id")).unwrap();
            let path = PathBuf::from(format!("models/model_{i}.sql"));
            many_assets.push(DbtAsset {
                base_path: base_path.clone(),
                original_path: path.clone(),
                path: path.clone(),
                package_name: "test_package".to_string(),
            });
        }

        node_properties.clear();
        let par_results = render_unresolved_sql_files::<ModelConfig, ModelProperties>(
            &parallel_ctx,
            &many_assets,
            &mut node_properties,
            &token,
            Arc::new(DefaultJinjaTypeCheckEventListenerFactory::default()),
        )
        .await
        .unwrap();

        assert!(par_results.len() >= 60, "Should have results for all files");

        // Check the first result's schema
        let par_schema = match &par_results[0].config.schema {
            Omissible::Present(Some(s)) => s.clone(),
            _ => panic!("Expected schema to be present in parallel result"),
        };

        // The key assertion: both sequential and parallel should resolve to root override
        assert_eq!(
            seq_schema, "root_override_schema",
            "Sequential rendering should use root project override"
        );
        assert_eq!(
            par_schema, "root_override_schema",
            "Parallel rendering should use root project override"
        );
        assert_eq!(
            seq_schema, par_schema,
            "Sequential and parallel should produce the same schema"
        );
    }

    /// Regression test for dbt-fusion #1660: a `source()` inside a Jinja
    /// branch that evaluates to false during the `execute=false` parse render
    /// must still be discovered (via static AST analysis) so that its schema
    /// is fetched into `sourced_remote` and the model can compile.
    #[dbt_runtime::test]
    async fn test_source_in_false_branch_is_statically_discovered() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let base_path = temp_dir.path().to_path_buf();
        let models_dir = base_path.join("models");
        std::fs::create_dir_all(&models_dir).unwrap();

        // `conditional_only` sits inside `{% if execute %}`, which is false
        // during the parse render, so render-driven collection never sees it.
        let sql = "select 1 as id\n\
            from {{ source('my_source', 'always_used') }}\n\
            {% if execute %}\n\
            where id in (select id from {{ source('my_source', 'conditional_only') }})\n\
            {% endif %}\n";
        let sql_file = models_dir.join("conditional_source_model.sql");
        std::fs::write(&sql_file, sql).unwrap();

        let path = PathBuf::from("models/conditional_source_model.sql");
        let test_asset = DbtAsset {
            base_path: base_path.clone(),
            original_path: path.clone(),
            path: path.clone(),
            package_name: "test_package".to_string(),
        };

        let cfg = ModelConfig {
            enabled: Some(true),
            quoting: Some(DbtQuoting::default()),
            ..Default::default()
        };
        let root_config = DbtProjectConfig::<ModelConfig> {
            config: cfg,
            children: IndexMap::new(),
        };

        let env = Environment::new();
        let jinja_env = Arc::new(JinjaEnv::new(env));

        let args = ResolveArgs {
            io: IoArgs {
                in_dir: base_path.clone(),
                out_dir: base_path.clone(),
                ..Default::default()
            },
            num_threads: Some(1),
            no_parallel: true,
            command: FsCommand::Test,
            vars: BTreeMap::new(),
            from_main: false,
            selector: None,
            select: None,
            indirect_selection: None,
            exclude: None,
            replay: None,
            sample_config: RunFilter::default(),
            sample_renaming: BTreeMap::new(),
            static_analysis: Some(dbt_common::io_args::StaticAnalysisKind::Strict),
            store_failures: false,
            skip_creating_generic_tests: false,
            adapter_override: None,
            profile_adapter_types: None,
            maximum_seed_size_mib: 1,
        };

        let mut base_ctx = BTreeMap::new();
        base_ctx.insert(
            "project_name".to_string(),
            minijinja::Value::from("test_package"),
        );

        let render_ctx = RenderCtx {
            inner: Arc::new(RenderCtxInner {
                args: args.clone(),
                base_ctx,
                root_project_name: "test_package".to_string(),
                package_name: "test_package".to_string(),
                adapter_type: AdapterType::Postgres,
                database: "test_db".to_string(),
                schema: "default_schema".to_string(),
                config_resolver: ProjectConfigResolver::for_root(
                    root_config,
                    AdapterType::Postgres,
                ),
                resource_paths: vec!["models".to_string()],
                package_quoting: DbtQuoting {
                    database: Some(true),
                    schema: Some(true),
                    identifier: Some(true),
                    snowflake_ignore_case: Some(false),
                },
                uses_snapshot_fqn: false,
                defer_render_errors_to_compile: false,
            }),
            jinja_env: jinja_env.clone(),
            runtime_config: Arc::new(DbtRuntimeConfig::default()),
            root_runtime_config: Arc::new(DbtRuntimeConfig::default()),
        };

        use dbt_common::cancellation::CancellationToken;
        let token = CancellationToken::never_cancels();

        let mut node_properties = BTreeMap::new();
        let results = render_unresolved_sql_files::<ModelConfig, ModelProperties>(
            &render_ctx,
            &[test_asset],
            &mut node_properties,
            &token,
            Arc::new(DefaultJinjaTypeCheckEventListenerFactory::default()),
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1, "Should have one result");
        let info = &results[0].sql_file_info;

        let live_sources: Vec<(String, String)> = info
            .sources
            .iter()
            .map(|(name, table, _)| (name.clone(), table.clone()))
            .collect();
        let static_sources: Vec<(String, String)> = info
            .static_sources
            .iter()
            .map(|(name, table, _)| (name.clone(), table.clone()))
            .collect();

        assert!(
            live_sources.contains(&("my_source".to_string(), "always_used".to_string())),
            "expected the unconditional source in live sources, got {live_sources:?}"
        );
        assert!(
            static_sources.contains(&("my_source".to_string(), "conditional_only".to_string())),
            "expected the dead-branch source in static_sources (dbt-fusion #1660), \
             got {static_sources:?}"
        );
        assert!(
            !live_sources.contains(&("my_source".to_string(), "conditional_only".to_string())),
            "dead-branch source must NOT appear in live sources, got {live_sources:?}"
        );
    }

    /// Verify that `resolve_with_configs` applies layers in the correct precedence order:
    /// project base → properties → inline → root overlay.
    #[test]
    fn test_resolve_with_configs_override_order() {
        use crate::dbt_project_config::{DbtProjectConfig, ProjectConfigResolver};
        use dbt_common::serde_utils::Omissible;
        use dbt_schemas::schemas::project::ModelConfig;
        use indexmap::IndexMap;

        let quoting = Some(DbtQuoting::default());
        let project_config = ModelConfig {
            schema: Omissible::Present(Some("project_schema".to_string())),
            quoting,
            ..Default::default()
        };
        let properties_config = ModelConfig {
            schema: Omissible::Present(Some("properties_schema".to_string())),
            quoting,
            ..Default::default()
        };
        let inline_config = ModelConfig {
            schema: Omissible::Present(Some("inline_schema".to_string())),
            quoting,
            ..Default::default()
        };
        let root_config = ModelConfig {
            schema: Omissible::Present(Some("root_schema".to_string())),
            quoting,
            ..Default::default()
        };

        let local = DbtProjectConfig::<ModelConfig> {
            config: project_config,
            children: IndexMap::new(),
        };
        let root = DbtProjectConfig::<ModelConfig> {
            config: root_config,
            children: IndexMap::new(),
        };
        let resolver = ProjectConfigResolver::for_dependency(local, root, AdapterType::Snowflake);
        let fqn = vec!["pkg".to_string(), "my_model".to_string()];

        // Root overlay has highest precedence
        let resolved = resolver.resolve_with_configs(
            &fqn,
            &fqn,
            &[Some(&properties_config), Some(&inline_config)],
        );
        match &resolved.schema {
            Omissible::Present(Some(s)) => {
                assert_eq!(s, "root_schema", "Root overlay should win");
            }
            _ => panic!("Expected schema to be present"),
        }

        // Without root overlay, inline wins over properties
        let root_resolver = ProjectConfigResolver::for_root(
            DbtProjectConfig::<ModelConfig> {
                config: ModelConfig {
                    schema: Omissible::Present(Some("project_schema".to_string())),
                    quoting,
                    ..Default::default()
                },
                children: IndexMap::new(),
            },
            AdapterType::Snowflake,
        );
        let resolved_no_root = root_resolver.resolve_with_configs(
            &fqn,
            &fqn,
            &[Some(&properties_config), Some(&inline_config)],
        );
        match &resolved_no_root.schema {
            Omissible::Present(Some(s)) => {
                assert_eq!(
                    s, "inline_schema",
                    "Inline config should win over properties"
                );
            }
            _ => panic!("Expected schema to be present"),
        }
    }
}
