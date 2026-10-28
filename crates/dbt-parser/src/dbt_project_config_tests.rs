//! Tests for FQN project-config construction.
//!
//! The code under test lives in `dbt-schemas` (re-exported as
//! `crate::dbt_project_config`), but these tests need `dbt-jinja-utils` to
//! deserialize the YAML fixtures, and that crate sits above `dbt-schemas` — a
//! dev-dependency back would be a cycle. So they live here, in the crate that
//! already has everything they need.

#[cfg(test)]
mod init_project_config_tests {
    use dbt_adapter_core::AdapterType;
    use dbt_common::tracing::fs_error_log::get_log_message;
    use dbt_common::{ErrorCode, FsResult};
    use dbt_schemas::schemas::project::{
        DbtProjectConfig, ModelConfig, ProjectConfigResolver, ProjectModelConfig,
        ProjectSnapshotConfig, ResolvableConfig, SnapshotConfig, TypedRecursiveConfig,
        init_project_config,
    };
    use dbt_tracing::{
        SeverityNumber, TelemetryOutputFlags,
        emit::create_root_info_span,
        init::create_tracing_subcriber_with_layer,
        layer::ConsumerLayer,
        test_support::mocks::{MockDynSpanEvent, TestLayer, test_data_layer},
    };
    use indexmap::IndexMap;

    /// Runs `f` in a root span; returns its result and the errors and warnings it emitted.
    #[allow(clippy::type_complexity)]
    pub(super) fn capture_diagnostics<R>(
        f: impl FnOnce() -> R,
    ) -> (R, Vec<(ErrorCode, String)>, Vec<(ErrorCode, String)>) {
        // Diagnostics are emitted through the tracing layer, so capture them with a
        // test consumer rather than a status reporter.
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
        .expect("test tracing subscriber should be valid");

        // The data layer only records events emitted within an active span, so run
        // `f` inside a root span.
        let result = tracing::subscriber::with_default(subscriber, || {
            let _root = create_root_info_span(MockDynSpanEvent {
                name: "root".to_string(),
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            })
            .entered();

            f()
        });

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        for record in log_records.lock().unwrap().iter() {
            // Diagnostics may arrive as either an `FsErrorLog` or a plain `LogMessage`
            // attribute; `get_log_message` handles both. The tests only inspect the
            // message string, so a placeholder `ErrorCode` is acceptable.
            let code = get_log_message(&record.attributes)
                .and_then(|lm| lm.code)
                .and_then(|c| u16::try_from(c).ok())
                .and_then(|c| ErrorCode::try_from(c).ok())
                .unwrap_or(ErrorCode::Generic);
            let entry = (code, record.body.clone());
            match record.severity_number {
                SeverityNumber::Error => errors.push(entry),
                SeverityNumber::Warn => warnings.push(entry),
                _ => {}
            }
        }

        (result, errors, warnings)
    }

    #[allow(clippy::type_complexity)]
    fn init_project_config_from_yaml<T, S>(
        yaml: &str,
        disallow_plus_prefix: bool,
    ) -> (
        FsResult<DbtProjectConfig<T>>,
        Vec<(ErrorCode, String)>,
        Vec<(ErrorCode, String)>,
    )
    where
        T: ResolvableConfig<T> + PartialEq,
        S: TypedRecursiveConfig + Into<T> + for<'de> serde::Deserialize<'de>,
        T::PackageDefaults: Default,
    {
        let configs: S = dbt_jinja_utils::serde::from_yaml_raw(yaml, None, false, None)
            .expect("yaml deserializes into config type");

        capture_diagnostics(|| {
            init_project_config::<T, S>(
                &Some(configs),
                Default::default(),
                None,
                disallow_plus_prefix,
                AdapterType::Snowflake,
            )
        })
    }

    #[test]
    fn valid_config_emits_no_diagnostics() {
        let yml = r#"
path_1:
  +enabled: true
path_2:
  +enabled: true
  path_3:
    +enabled: false
"#;
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, false);
        assert!(result.is_ok());
        assert!(errors.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn plus_prefixed_resource_path_emits_diagnostics() {
        let yml = r#"
+path_1:
  +enabled: false
  +quoting:
    database: true
"#;
        // Behavior flag off
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, false);
        assert!(result.is_ok());
        assert!(errors.is_empty());
        assert_eq!(warnings.len(), 1);
        let (_, msg) = &warnings[0];
        assert!(msg.contains("Resource path `+path_1` in dbt_project.yml starts with `+`"));

        // Behavior flag on
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, true);
        assert!(result.is_ok());
        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 1);
        let (_, msg) = &errors[0];
        assert!(msg.contains("Unrecognized key `+path_1`"));
    }

    #[test]
    fn nested_plus_resource_path_emits_diagnostics() {
        let yml = r#"
path_1:
  +path_2:
    path_3:
      path_4:
        +enabled: true
"#;
        // Behavior flag off
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, false);
        assert!(result.is_ok());
        assert!(errors.is_empty());
        assert_eq!(warnings.len(), 1);
        let (_, msg) = &warnings[0];
        assert!(msg.contains("Resource path `path_1.+path_2` in dbt_project.yml starts with `+`"));

        // Behavior flag on
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, true);
        assert!(result.is_ok());
        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 1);
        let (_, msg) = &errors[0];
        assert!(msg.contains("Unrecognized key `path_1.+path_2`"));
    }

    #[test]
    fn ambiguous_resource_path_only_errors() {
        let yml = r#"
path_1:
  path_2:
    +enabled: true
    path_3:
      +not_provably_a_resource_path: {}
"#;
        // Behavior flag off
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, false);
        assert!(result.is_ok());
        assert!(errors.is_empty());
        assert!(warnings.is_empty());

        // Behavior flag on
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, true);
        assert!(result.is_ok());
        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 1);
        let (_, msg) = &errors[0];
        assert!(
            msg.contains("Unrecognized key `path_1.path_2.path_3.+not_provably_a_resource_path`")
        );
    }

    #[test]
    fn meta_accepts_plus_prefixes() {
        let yml = r#"
path_1:
  path_2:
    +enabled: true
    +meta:
      +key_1: true
      +key_2: 15451
"#;
        // Behavior flag off
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, false);
        assert!(result.is_ok());
        assert!(errors.is_empty());
        assert!(warnings.is_empty());

        // Behavior flag on
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, true);
        assert!(result.is_ok());
        assert!(errors.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn exactly_one_warning_per_plus_prefixed_path() {
        let yml = r#"
path_1:
  +path_2:
    +meta:
      +cool_key: 15451
    path_3:
      +quoting:
        database: true
        schema: false
      path_4:
        +enabled: true
  +path_3:
    +alias: cool_alias
    path_5:
      +description: cool_description
  +not_provably_a_resource_path: {}
"#;
        // Behavior flag off
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, false);
        assert!(result.is_ok());
        assert!(errors.is_empty());
        assert_eq!(warnings.len(), 2);
        let (_, msg) = &warnings[0];
        assert!(msg.contains("Resource path `path_1.+path_2` in dbt_project.yml starts with `+`"));
        let (_, msg) = &warnings[1];
        assert!(msg.contains("Resource path `path_1.+path_3` in dbt_project.yml starts with `+`"));

        // Behavior flag on
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, true);
        assert!(result.is_ok());
        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 3);
        let (_, msg) = &errors[0];
        assert!(msg.contains("Unrecognized key `path_1.+not_provably_a_resource_path`"));
        let (_, msg) = &errors[1];
        assert!(msg.contains("Unrecognized key `path_1.+path_2`"));
        let (_, msg) = &errors[2];
        assert!(msg.contains("Unrecognized key `path_1.+path_3`"));
    }

    #[test]
    fn unrecognized_key_always_emits_error() {
        let yml = r#"
path_1:
  +bogus_key: scalar_value
"#;
        // Behavior flag off
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, false);
        assert!(result.is_ok());
        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 1);
        let (_, msg) = &errors[0];
        assert!(msg.contains("Unrecognized key `path_1.+bogus_key`"));

        // Behavior flag on
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, true);
        assert!(result.is_ok());
        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 1);
        let (_, msg) = &errors[0];
        assert!(msg.contains("Unrecognized key `path_1.+bogus_key`"));
    }

    #[test]
    fn unquoted_boolean_resource_path_suggests_quoting() {
        // YAML 1.1 reads an unquoted `off` as `false`, so the folder name is lost.
        let yml = r#"
my_project:
  +materialized: view
  off:
    +static_analysis: off
"#;
        for disallow_plus_prefix in [false, true] {
            let (result, errors, warnings) = init_project_config_from_yaml::<
                ModelConfig,
                ProjectModelConfig,
            >(yml, disallow_plus_prefix);
            assert!(result.is_ok());
            assert!(warnings.is_empty());
            assert_eq!(errors.len(), 1);
            let (_, msg) = &errors[0];
            assert!(
                msg.contains(
                    "Invalid model definition `my_project`: A key under `my_project` reads as \
                     the boolean `false`"
                ),
                "{msg}"
            );
            assert!(msg.contains("e.g. `'off':`"), "{msg}");
        }
    }

    #[test]
    fn repro_emits_error_at_key() {
        // https://github.com/dbt-labs/dbt-core/issues/14433
        let yml = r#"
my_project:
  staging:
    +contract:
      enforced: true
"#;
        // Behavior flag off
        // With the behavior flag off, the behavior is the same as in the issue.
        // We don't recognize that +contract is the source of the issue.

        // Behavior flag on
        // With the behavior flag on, we correctly see the unrecognized key
        // error surface at +contract.
        let (result, errors, warnings) =
            init_project_config_from_yaml::<SnapshotConfig, ProjectSnapshotConfig>(yml, true);
        assert!(result.is_ok());
        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 1);
        let (_, msg) = &errors[0];
        assert!(msg.contains("Unrecognized key `my_project.staging.+contract`"));
    }

    #[test]
    fn persist_constraints_is_valid_and_inherits_with_nested_override() {
        let yml = r#"
+persist_constraints: true
my_project:
  inherited:
    +enabled: true
  disabled:
    +persist_constraints: false
"#;

        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, false);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let config = result.unwrap();
        let inherited = config.get_config_for_fqn(&[
            "my_project".to_string(),
            "inherited".to_string(),
            "model".to_string(),
        ]);
        assert_eq!(
            inherited.__warehouse_specific_config__.persist_constraints,
            Some(true)
        );

        let disabled = config.get_config_for_fqn(&[
            "my_project".to_string(),
            "disabled".to_string(),
            "model".to_string(),
        ]);
        assert_eq!(
            disabled.__warehouse_specific_config__.persist_constraints,
            Some(false)
        );

        let (result, errors, warnings) =
            init_project_config_from_yaml::<SnapshotConfig, ProjectSnapshotConfig>(yml, false);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let config = result.unwrap();
        let inherited = config.get_config_for_fqn(&[
            "my_project".to_string(),
            "inherited".to_string(),
            "snapshot".to_string(),
        ]);
        assert_eq!(
            inherited.__warehouse_specific_config__.persist_constraints,
            Some(true)
        );

        let disabled = config.get_config_for_fqn(&[
            "my_project".to_string(),
            "disabled".to_string(),
            "snapshot".to_string(),
        ]);
        assert_eq!(
            disabled.__warehouse_specific_config__.persist_constraints,
            Some(false)
        );
    }

    #[test]
    fn use_materialization_v2_is_valid_and_inherits_with_nested_override() {
        let yml = r#"
my_project:
  governed:
    +use_materialization_v2: true
    atomic:
      +use_materialization_v2: "false"
  unset:
    +enabled: true
"#;

        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, false);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let config = result.unwrap();
        let use_materialization_v2 = |path: &[&str]| {
            let fqn: Vec<String> = path.iter().map(|s| s.to_string()).collect();
            config
                .get_config_for_fqn(&fqn)
                .__warehouse_specific_config__
                .use_materialization_v2
        };
        assert_eq!(
            use_materialization_v2(&["my_project", "governed", "model"]),
            Some(true)
        );
        assert_eq!(
            use_materialization_v2(&["my_project", "governed", "atomic", "model"]),
            Some(false)
        );
        // Unset must stay `None` so the adapter falls back to the project behavior flag.
        assert_eq!(
            use_materialization_v2(&["my_project", "unset", "model"]),
            None
        );
    }

    /// Residual (fs#13424): Databricks' `target_catalog` -> `target_database`
    /// alias has no dedicated field the way `catalog` -> `database` does (there is no
    /// `target_catalog` field on `SnapshotConfig`/`ProjectSnapshotConfig` to canonicalize),
    /// and unlike the inline `{{ config(...) }}` layer, `dbt_project.yml`'s subtree levels are
    /// already-typed structs by the time `recur_build_dbt_project_config` runs -- there is no
    /// raw YAML map left in which to rename the key. So `+target_catalog:` in `dbt_project.yml`
    /// remains an unrecognized key, unlike the same alias authored via inline config (see
    /// `test_parse_config_inline_target_catalog_alias_resolves_on_databricks_snapshot` in
    /// `dbt-jinja-utils`). `#[ignore]`d because this pins the *gap*, not the desired behavior;
    /// delete it and add real coverage instead if the gap is ever closed (see the comment on
    /// `SnapshotConfig::canonicalize_adapter_aliases`).
    #[test]
    #[ignore = "fs#13424 residual: +target_catalog: in dbt_project.yml has no typed \
                field to canonicalize into (see SnapshotConfig::canonicalize_adapter_aliases)"]
    fn test_snapshot_target_catalog_in_project_yml_is_unrecognized_key() {
        let yml = "+target_catalog: some_cat\n";
        let (result, errors, warnings) =
            init_project_config_from_yaml::<SnapshotConfig, ProjectSnapshotConfig>(yml, true);
        assert!(result.is_ok());
        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 1);
        let (_, msg) = &errors[0];
        assert!(msg.contains("Unrecognized key `+target_catalog`"));
    }

    /// Residual (fs#13424): postgres/redshift's `dbname` -> `database` alias has
    /// the same shape of gap as `target_catalog` above, on a different config type -- `dbname`
    /// has no dedicated field at all (unlike `catalog`, which has one to move out of), so it can
    /// only resolve via a raw config-key rename before typing, which is only available at the
    /// inline `{{ config(...) }}` layer (see
    /// `test_parse_config_inline_dbname_alias_resolves_on_postgres` in `dbt-jinja-utils`).
    /// `#[ignore]`d for the same reason as the test above: this pins the gap, not the desired
    /// behavior.
    #[test]
    #[ignore = "fs#13424 residual: +dbname: in dbt_project.yml has no typed field to \
                canonicalize into (see ResolvableConfig::canonicalize_adapter_aliases)"]
    fn test_dbname_in_project_yml_is_unrecognized_key() {
        let yml = "+dbname: some_db\n";
        let (result, errors, warnings) =
            init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, true);
        assert!(result.is_ok());
        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 1);
        let (_, msg) = &errors[0];
        assert!(msg.contains("Unrecognized key `+dbname`"));
    }

    /// The predicate must fire only on an explicit `+enabled: true`; an absent value must not be
    /// read as `true`, or every dependency package node would be treated as force-enabled.
    #[test]
    fn test_is_enabled_by_root_overlay() {
        let fqn = vec!["pkg".to_string(), "my_model".to_string()];
        let cases = [
            (
                "exact model",
                "pkg:\n  my_model:\n    +enabled: true\n",
                true,
            ),
            ("inherited from package", "pkg:\n  +enabled: true\n", true),
            ("global", "+enabled: true\n", true),
            (
                "absent",
                "pkg:\n  my_model:\n    +materialized: view\n",
                false,
            ),
            (
                "explicitly disabled",
                "pkg:\n  my_model:\n    +enabled: false\n",
                false,
            ),
        ];

        for (label, yml, expected) in cases {
            let (root, errors, _) =
                init_project_config_from_yaml::<ModelConfig, ProjectModelConfig>(yml, true);
            assert!(errors.is_empty(), "{label}: {errors:?}");
            let root = root.expect("root overlay config builds");
            let local = DbtProjectConfig::<ModelConfig> {
                config: ModelConfig::default(),
                children: IndexMap::new(),
            };
            let resolver =
                ProjectConfigResolver::for_dependency(local, root.clone(), AdapterType::Snowflake);
            assert_eq!(
                resolver.is_enabled_by_root_overlay(&fqn),
                expected,
                "{label}"
            );

            // Root packages have no overlay, so their own inline disable keeps winning.
            let root_resolver = ProjectConfigResolver::for_root(root, AdapterType::Snowflake);
            assert!(
                !root_resolver.is_enabled_by_root_overlay(&fqn),
                "{label}: root package must never be force-enabled"
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod per_adapter_config_resolver_tests {
    use super::init_project_config_tests::capture_diagnostics;
    use crate::dbt_project_config::{
        DbtProjectConfig, NodeLayers, ProjectConfigResolver, RootProjectConfigs,
        build_root_project_configs, init_project_config,
    };
    use dbt_adapter_core::AdapterType;
    use dbt_schemas::schemas::common::DbtQuoting;
    use dbt_schemas::schemas::profiles::DbConfig;
    use dbt_schemas::schemas::project::{DbtProject, ModelConfig, ProjectModelConfig};
    use dbt_schemas::state::ProfileAdapter;
    use indexmap::IndexMap;

    fn models(yaml: &str) -> ProjectModelConfig {
        dbt_jinja_utils::serde::from_yaml_raw(yaml, None, false, None).unwrap()
    }

    fn tree(yaml: &str) -> DbtProjectConfig<ModelConfig> {
        init_project_config(
            &Some(models(yaml)),
            DbtQuoting::default(),
            None,
            false,
            AdapterType::Snowflake,
        )
        .unwrap()
    }

    /// A Snowflake default that also declares `other`, over `models_yaml`.
    pub(crate) fn root_configs_declaring(other: DbConfig, models_yaml: &str) -> RootProjectConfigs {
        let root_project = DbtProject {
            name: "my_project".to_string(),
            models: Some(models(models_yaml)),
            ..Default::default()
        };
        let target_adapters: IndexMap<_, _> = [DbConfig::Snowflake(Box::default()), other]
            .map(|config| (config.adapter_type(), ProfileAdapter::single(config)))
            .into();
        build_root_project_configs(
            &root_project,
            &target_adapters,
            AdapterType::Snowflake,
            true,
        )
        .unwrap()
    }

    /// A Snowflake default that also declares Databricks, over `models_yaml`.
    fn root_configs(models_yaml: &str) -> RootProjectConfigs {
        root_configs_declaring(DbConfig::Databricks(Box::default()), models_yaml)
    }

    /// The root package's model resolver, as `resolve_models` builds it.
    pub(crate) fn root_package_resolver(
        root: &RootProjectConfigs,
    ) -> ProjectConfigResolver<ModelConfig> {
        root_package_resolver_with_override(root, None)
    }

    /// Like [`root_package_resolver`], under `--adapter <adapter_override>`.
    pub(crate) fn root_package_resolver_with_override(
        root: &RootProjectConfigs,
        adapter_override: Option<AdapterType>,
    ) -> ProjectConfigResolver<ModelConfig> {
        ProjectConfigResolver::per_adapter(
            root.models.clone(),
            |root| &root.models,
            root,
            &None::<ProjectModelConfig>,
            None,
            Default::default(),
            AdapterType::Snowflake,
            adapter_override,
        )
        .unwrap()
    }

    /// A dependency package's model resolver over its own `local_yaml`.
    fn dependency_resolver(
        root: &RootProjectConfigs,
        local_yaml: &str,
    ) -> ProjectConfigResolver<ModelConfig> {
        ProjectConfigResolver::per_adapter(
            root.models.clone(),
            |root| &root.models,
            root,
            &Some(models(local_yaml)),
            Some("my_dep"),
            Default::default(),
            AdapterType::Snowflake,
            None,
        )
        .unwrap()
    }

    fn fqn(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    fn on(adapter: Option<AdapterType>) -> ModelConfig {
        ModelConfig {
            adapter,
            ..Default::default()
        }
    }

    /// Every layer is translated for the node's adapter, so a node-level alias still wins over a
    /// project-level canonical key, and a folder-level alias reaches `database`.
    #[test]
    fn catalog_alias_keeps_precedence_under_the_node_adapter() {
        let root = root_configs(
            "+database: project_database\n\
             my_project:\n  dbx:\n    +adapter: databricks\n    +catalog: project_catalog\n",
        );
        let resolver = root_package_resolver(&root);

        let mut node_layer = on(Some(AdapterType::Databricks));
        node_layer.__warehouse_specific_config__.catalog = Some("node_catalog".to_string());
        let node_fqn = fqn(&["my_project", "m"]);
        let merged = resolver.with_configs(&node_fqn, &[Some(&node_layer)]);
        assert_eq!(
            merged.database.into_inner().flatten(),
            Some("node_catalog".to_string())
        );
        assert_eq!(merged.__warehouse_specific_config__.catalog, None);

        let folder_fqn = fqn(&["my_project", "dbx", "m"]);
        let merged = resolver.with_configs(&folder_fqn, &[]);
        assert_eq!(
            merged.database.into_inner().flatten(),
            Some("project_catalog".to_string())
        );
    }

    /// The picked adapter is the one the full merge produces, for every combination of layers,
    /// with the local tree read at `original_fqn` and the root overlay at `fqn`.
    #[test]
    fn picked_adapter_matches_the_merged_adapter() {
        let original_fqn = fqn(&["pkg", "orig", "m"]);
        let moved_fqn = fqn(&["pkg", "moved", "m"]);
        // Each tree sets a different adapter at the path it must not be read at.
        let locals = [
            tree("pkg:\n  orig:\n    +adapter: redshift\n  moved:\n    +adapter: duckdb\n"),
            tree("pkg:\n  moved:\n    +adapter: duckdb\n"),
        ];
        let roots = [
            Some(tree(
                "pkg:\n  moved:\n    +adapter: bigquery\n  orig:\n    +adapter: trino\n",
            )),
            Some(tree("pkg:\n  orig:\n    +adapter: trino\n")),
            None,
        ];
        let properties = [None, Some(on(Some(AdapterType::Snowflake)))];
        let inline = [None, Some(on(Some(AdapterType::Postgres)))];

        let mut seen = std::collections::BTreeSet::new();
        for local in &locals {
            for root in &roots {
                let resolver = match root {
                    Some(root) => ProjectConfigResolver::for_dependency(
                        local.clone(),
                        root.clone(),
                        AdapterType::Snowflake,
                    ),
                    None => ProjectConfigResolver::for_root(local.clone(), AdapterType::Snowflake),
                };
                for properties in &properties {
                    for inline in &inline {
                        let configs = [properties.as_ref(), inline.as_ref()];
                        // `merge_node`'s debug_assert checks the peeked pick against the merge.
                        let resolved = resolver.resolve_node(&NodeLayers {
                            original_fqn: &original_fqn,
                            fqn: &moved_fqn,
                            configs: &configs,
                            inherited_adapter: None,
                        });
                        let merged = resolved.config.adapter;
                        assert_eq!(resolved.adapter, merged, "{local:?} {root:?} {configs:?}");
                        seen.insert(format!("{merged:?}"));
                    }
                }
            }
        }
        // Every layer decides the result somewhere in the matrix.
        assert_eq!(seen.len(), 5, "{seen:?}");
    }

    /// A default or undeclared node is merged over the default trees, so it resolves as before;
    /// only a declared non-default adapter gets trees of its own.
    #[test]
    fn default_and_undeclared_adapters_resolve_as_before() {
        let root_yaml = "+materialized: table\n\
                         my_dep:\n  +catalog: root_catalog\n  staging:\n    +schema: stg\n";
        let root = root_configs(root_yaml);
        let local_yaml = "my_dep:\n  +tags: [a]\n  staging:\n    +alias: x\n";
        let today = ProjectConfigResolver::for_dependency(
            tree(local_yaml),
            root.models.clone(),
            AdapterType::Snowflake,
        );
        let resolver = dependency_resolver(&root, local_yaml);

        let node_fqn = fqn(&["my_dep", "staging", "m"]);
        for adapter in [
            None,
            Some(AdapterType::Snowflake),
            Some(AdapterType::Postgres),
        ] {
            let node_layer = ModelConfig {
                enabled: Some(true),
                ..on(adapter)
            };
            let layers = [Some(&node_layer)];
            let expected = format!(
                "{:?}",
                today.resolve_with_configs(&node_fqn, &node_fqn, &layers)
            );
            let resolved = resolver.resolve_node(&NodeLayers::new(&node_fqn, &layers));
            assert_eq!(format!("{:?}", resolved.config), expected, "{adapter:?}");
            assert_eq!(
                resolved.alias_adapter,
                AdapterType::Snowflake,
                "{adapter:?}"
            );
        }
        // Not vacuous: a Databricks node reads the same tree differently.
        let databricks = resolver
            .with_configs_and_root_overlay(&node_fqn, &[Some(&on(Some(AdapterType::Databricks)))]);
        assert_eq!(
            databricks.database.into_inner().flatten(),
            Some("root_catalog".to_string())
        );
    }

    /// A node's config is read with its own adapter's alias table only when the target declares
    /// that adapter; otherwise, and for a resolver without per-adapter trees, with the default's.
    #[test]
    fn alias_adapter_is_the_node_adapter_only_when_declared() {
        let root = root_configs("+materialized: view\n");
        let resolver = root_package_resolver(&root);
        let node_fqn = fqn(&["my_project", "m"]);
        let resolve = |resolver: &ProjectConfigResolver<ModelConfig>, adapter| {
            let layer = on(adapter);
            let resolved = resolver.resolve_node(&NodeLayers::new(&node_fqn, &[Some(&layer)]));
            (resolved.config.adapter, resolved.alias_adapter)
        };
        for (adapter, alias_adapter) in [
            (AdapterType::Databricks, AdapterType::Databricks),
            (AdapterType::Snowflake, AdapterType::Snowflake),
            (AdapterType::Postgres, AdapterType::Snowflake),
        ] {
            assert_eq!(
                resolve(&resolver, Some(adapter)),
                (Some(adapter), alias_adapter)
            );
        }

        // The analyses/checks opt-out: the config still names the node's adapter.
        let opt_out = ProjectConfigResolver::for_root(root.models, AdapterType::Snowflake);
        assert_eq!(
            resolve(&opt_out, Some(AdapterType::Databricks)),
            (Some(AdapterType::Databricks), AdapterType::Snowflake)
        );
    }

    /// A target that declares a second adapter builds its trees only with multi-adapter support.
    #[test]
    fn per_adapter_trees_need_multi_adapter_support() {
        let root_project = DbtProject {
            name: "my_project".to_string(),
            ..Default::default()
        };
        let target_adapters: IndexMap<_, _> = [
            DbConfig::Snowflake(Box::default()),
            DbConfig::Databricks(Box::default()),
        ]
        .map(|config| (config.adapter_type(), ProfileAdapter::single(config)))
        .into();
        for multi_adapter in [false, true] {
            let root = build_root_project_configs(
                &root_project,
                &target_adapters,
                AdapterType::Snowflake,
                multi_adapter,
            )
            .unwrap();
            assert_eq!(
                root_package_resolver(&root).has_per_adapter_trees(),
                multi_adapter
            );
        }
    }

    /// A dependency's own tree is translated for the node's adapter too, so its folder-level
    /// alias reaches `database`; the root tree here sets no catalog.
    #[test]
    fn a_dependency_tree_is_translated_for_the_node_adapter() {
        let root = root_configs("+materialized: view\n");
        let resolver = dependency_resolver(&root, "my_dep:\n  +catalog: dep_catalog\n");
        let node_fqn = fqn(&["my_dep", "m"]);
        let database = |adapter| {
            resolver
                .with_configs_and_root_overlay(&node_fqn, &[Some(&on(adapter))])
                .database
                .into_inner()
                .flatten()
        };
        assert_eq!(
            database(Some(AdapterType::Databricks)),
            Some("dep_catalog".to_string())
        );
        assert_eq!(database(None), None);
    }

    /// A data test's inherited adapter picks the alias table when no layer sets one, and a
    /// layer's `+adapter` beats it.
    #[test]
    fn inherited_adapter_picks_the_alias_table() {
        let root = root_configs("+catalog: root_catalog\n");
        let resolver = root_package_resolver(&root);
        let node_fqn = fqn(&["my_project", "m"]);

        let inherited = resolver.with_node_configs(&NodeLayers {
            original_fqn: &node_fqn,
            fqn: &node_fqn,
            configs: &[None],
            inherited_adapter: Some(AdapterType::Databricks),
        });
        assert_eq!(inherited.adapter, Some(AdapterType::Databricks));
        assert_eq!(inherited.alias_adapter, AdapterType::Databricks);
        assert_eq!(
            inherited.config.database.into_inner().flatten(),
            Some("root_catalog".to_string())
        );

        let snowflake = on(Some(AdapterType::Snowflake));
        let overridden = resolver.with_node_configs(&NodeLayers {
            original_fqn: &node_fqn,
            fqn: &node_fqn,
            configs: &[Some(&snowflake)],
            inherited_adapter: Some(AdapterType::Databricks),
        });
        assert_eq!(overridden.adapter, Some(AdapterType::Snowflake));
        assert_eq!(overridden.alias_adapter, AdapterType::Snowflake);
        assert_eq!(overridden.config.database.into_inner().flatten(), None);
    }

    /// `--adapter` beats every layer's `+adapter`, including for the alias table.
    #[test]
    fn adapter_override_beats_every_layer() {
        let root = root_configs("+catalog: root_catalog\n");
        let resolver = root_package_resolver_with_override(&root, Some(AdapterType::Snowflake));
        let node_fqn = fqn(&["my_project", "m"]);
        let layer = on(Some(AdapterType::Databricks));
        let resolved = resolver.with_node_configs(&NodeLayers::new(&node_fqn, &[Some(&layer)]));
        assert_eq!(resolved.adapter, Some(AdapterType::Snowflake));
        assert_eq!(resolved.alias_adapter, AdapterType::Snowflake);
        assert_eq!(resolved.config.database.into_inner().flatten(), None);
        assert_eq!(
            resolved.config.__warehouse_specific_config__.catalog,
            Some("root_catalog".to_string())
        );
    }

    /// Building a tree consumes each invalid child's one-shot error, so a non-default tree must
    /// be built quietly: no panic, and no diagnostic emitted a second time.
    #[test]
    fn a_non_default_tree_neither_panics_nor_repeats_diagnostics() {
        let (database, errors, warnings) = capture_diagnostics(|| {
            // An unquoted `off` folder is an invalid child; `+path_1` warns.
            let root = root_configs(
                "+catalog: root_catalog\n\
                 +path_1:\n  +enabled: false\n\
                 my_project:\n  +materialized: view\n  off:\n    +static_analysis: off\n",
            );
            let resolver = root_package_resolver(&root);
            let node_fqn = fqn(&["my_project", "m"]);
            resolver
                .with_configs(&node_fqn, &[Some(&on(Some(AdapterType::Databricks)))])
                .database
                .into_inner()
                .flatten()
        });
        assert_eq!(database, Some("root_catalog".to_string()));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].1.contains("reads as the boolean"), "{errors:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].1.contains("`+path_1`"), "{warnings:?}");
    }
}
