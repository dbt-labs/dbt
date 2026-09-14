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

        let configs: S = dbt_jinja_utils::serde::from_yaml_raw(yaml, None, false, None)
            .expect("yaml deserializes into config type");

        // The data layer only records events emitted within an active span, so run
        // `init_project_config` inside a root span.
        let result = tracing::subscriber::with_default(subscriber, || {
            let _root = create_root_info_span(MockDynSpanEvent {
                name: "root".to_string(),
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            })
            .entered();

            init_project_config::<T, S>(
                &Some(configs),
                Default::default(),
                None,
                disallow_plus_prefix,
                AdapterType::Snowflake,
            )
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
