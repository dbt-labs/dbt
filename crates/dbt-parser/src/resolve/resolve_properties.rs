use crate::args::ResolveArgs;
use crate::dbt_project_config::{
    ProjectConfigResolver, RootProjectConfigs, disallow_plus_prefix_from_flags, init_project_config,
};
use crate::resolve::resolve_utils::deep_merge_yaml;
use dbt_adapter_core::AdapterType;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_utils::try_read_yml_to_str;
use dbt_common::tracing::dbt_emit::{emit_strict_parse_error, emit_warn_log_message};
use dbt_common::tracing::span_info::SpanStatusRecorder as _;
use dbt_common::{ErrorCode, FsResult, create_debug_span, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::{from_yaml_raw, into_typed_with_jinja};
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::schemas::properties::{
    AnalysesProperties, CheckProperties, DbtPropertiesFileValues, MinimalSchemaValue,
    MinimalTableValue, MinimalUnitTestValue,
};
use dbt_schemas::schemas::serde::{FloatOrString, strip_one_trailing_newline_at_keys};
use dbt_schemas::state::DbtPackage;
use dbt_telemetry::AssetParsed;
use dbt_yaml::{Span, Spanned, Verbatim};
use itertools::Itertools;
use minijinja::Value as MinijinjaValue;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct MinimalPropertiesEntry {
    pub name: String,
    pub name_span: Span,
    pub relative_path: PathBuf,
    pub schema_value: dbt_yaml::Value,
    pub table_value: Option<dbt_yaml::Value>,
    pub version_info: Option<VersionInfo>,
    pub duplicate_paths: Vec<PathBuf>,
}

#[derive(Debug, serde::Deserialize)]
struct MinimalSnapshotValue {
    name: Spanned<String>,
    __additional_properties__: Verbatim<BTreeMap<String, dbt_yaml::Value>>,
}

#[derive(Default, Debug)]
pub struct MinimalProperties {
    pub source_tables: BTreeMap<(String, String), MinimalPropertiesEntry>,
    pub models: BTreeMap<String, MinimalPropertiesEntry>,
    pub analyses: BTreeMap<String, MinimalPropertiesEntry>,
    pub checks: BTreeMap<String, MinimalPropertiesEntry>,
    pub seeds: BTreeMap<String, MinimalPropertiesEntry>,
    pub snapshots: BTreeMap<String, MinimalPropertiesEntry>,
    pub functions: BTreeMap<String, MinimalPropertiesEntry>,
    pub unit_tests: BTreeMap<String, MinimalPropertiesEntry>,
    pub tests: BTreeMap<String, MinimalPropertiesEntry>,
    pub exposures: BTreeMap<String, MinimalPropertiesEntry>,
    pub metrics: BTreeMap<String, MinimalPropertiesEntry>,
    pub saved_queries: BTreeMap<String, MinimalPropertiesEntry>,
    pub groups: BTreeMap<String, MinimalPropertiesEntry>,
    pub macros: BTreeMap<String, MinimalPropertiesEntry>,
    pub semantic_layer_spec_is_legacy: bool,
}

// impl try extend from MinimalResolvedProperties
#[allow(clippy::cognitive_complexity)]
impl MinimalProperties {
    pub fn extend_from_minimal_properties_file(
        &mut self,
        other: DbtPropertiesFileValues,
        jinja_env: &JinjaEnv,
        properties_path: &Path,
        base_ctx: &BTreeMap<String, MinijinjaValue>,
    ) -> FsResult<()> {
        // TODO: This is a bit repetetive. Can be shortened!
        if let Some(models) = other.models {
            // Extend but error on duplicate keys
            for model_value in models {
                let model = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&model_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                let name_span = match model_value.get("name") {
                    Some(name) => name.span(),
                    None => &Span::default(),
                };
                for (key, maybe_version_info) in collect_model_version_info(&model).into_iter() {
                    if let Some(existing_model) = self.models.get_mut(&key) {
                        existing_model
                            .duplicate_paths
                            .push(properties_path.to_path_buf());
                    } else {
                        // Each version gets its own clone, so merging per version affects no sibling.
                        let mut schema_value = model_value.clone();
                        if let Some(version_info) = maybe_version_info.as_ref() {
                            merge_version_config_into_schema_value(
                                &mut schema_value,
                                &version_info.version_config,
                            );
                        }
                        self.models.insert(
                            key,
                            MinimalPropertiesEntry {
                                name: validate_resource_name(&model.name)?,
                                name_span: name_span.clone(),
                                relative_path: properties_path.to_path_buf(),
                                version_info: maybe_version_info,
                                schema_value,
                                table_value: None,
                                duplicate_paths: vec![],
                            },
                        );
                    }
                }
            }
        }
        if let Some(analyses) = other.analyses {
            for analysis_value in analyses {
                let analysis = into_typed_with_jinja::<AnalysesProperties, _>(
                    name_stripped(&analysis_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_analysis) = self.analyses.get_mut(&analysis.name) {
                    existing_analysis
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.analyses.insert(
                        analysis.name.clone(),
                        MinimalPropertiesEntry {
                            name: validate_resource_name(&analysis.name)?,
                            name_span: Span::default(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: analysis_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(checks) = other.checks {
            for check_value in checks {
                let check = into_typed_with_jinja::<CheckProperties, _>(
                    name_stripped(&check_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_check) = self.checks.get_mut(&check.name) {
                    existing_check
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.checks.insert(
                        check.name.clone(),
                        MinimalPropertiesEntry {
                            name: validate_resource_name(&check.name)?,
                            name_span: Span::default(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: check_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(sources) = other.sources {
            for mut source_value in sources {
                // Pre-render the `tables` field if it is a single Jinja expression
                // (e.g. `{{ var('source_tables') }}`). MinimalSchemaValue wraps
                // `tables` in `Verbatim` so that table *contents* are not rendered
                // prematurely, but this also blocks expanding the field value itself
                // from a string into a sequence.
                // See: https://github.com/dbt-labs/dbt-fusion/issues/982
                pre_render_tables_field(&mut source_value, jinja_env, base_ctx);

                let source = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&source_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;

                if let Some(tables) = &*source.tables {
                    // Clone the original source_value to preserve all field spans
                    // and only modify the tables field to null
                    let mut schema_value = source_value.clone();

                    // Set only the tables field to null while preserving all other fields and their spans
                    if let Some(mapping) = schema_value.as_mapping_mut() {
                        mapping.insert(
                            dbt_yaml::Value::string("tables".to_string()),
                            dbt_yaml::Value::null(),
                        );
                    }

                    validate_resource_name(&source.name)?;
                    for table in tables.iter() {
                        let minimum_table_value = into_typed_with_jinja::<MinimalTableValue, _>(
                            name_stripped(table),
                            false,
                            jinja_env,
                            base_ctx,
                            &[],
                            dependency_package_name_from_ctx(jinja_env, base_ctx),
                            true,
                        )?;
                        let key = (
                            source.name.clone(),
                            minimum_table_value.name.clone().into_inner(),
                        );

                        if let Some(existing_entry) = self.source_tables.get_mut(&key) {
                            existing_entry
                                .duplicate_paths
                                .push(properties_path.to_path_buf());

                            emit_warn_log_message(
                                ErrorCode::DuplicateSourceTable,
                                format!(
                                    "Duplicate definition for table '{}' in source '{}' found in file '{}'. Using definition from '{}'.",
                                    minimum_table_value.name.clone().into_inner(),
                                    source.name,
                                    properties_path.display(),
                                    existing_entry.relative_path.display()
                                ),
                            );
                        } else {
                            self.source_tables.insert(
                                key,
                                MinimalPropertiesEntry {
                                    name: minimum_table_value.name.clone().into_inner(),
                                    name_span: minimum_table_value.name.span().clone(),
                                    relative_path: properties_path.to_path_buf(),
                                    schema_value: schema_value.clone(),
                                    table_value: Some(table.clone()), // Store table separately
                                    version_info: None,
                                    duplicate_paths: vec![],
                                },
                            );
                        }
                    }
                } else {
                    emit_warn_log_message(
                        ErrorCode::NoTablesInSource,
                        format!(
                            "No tables defined for source '{}' in file '{}'.",
                            source.name,
                            properties_path.display()
                        ),
                    );
                }
            }
        }
        if let Some(seeds) = other.seeds {
            for seed_value in seeds {
                let seed = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&seed_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_seed) = self.seeds.get_mut(&seed.name) {
                    existing_seed
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.seeds.insert(
                        seed.name.clone(),
                        MinimalPropertiesEntry {
                            name: validate_resource_name(&seed.name)?,
                            name_span: Span::default(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: seed_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(snapshots) = other.snapshots {
            for snapshot_value in snapshots {
                let snapshot = into_typed_with_jinja::<MinimalSnapshotValue, _>(
                    name_stripped(&snapshot_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_snapshot) = self.snapshots.get_mut(snapshot.name.as_ref()) {
                    existing_snapshot
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.snapshots.insert(
                        snapshot.name.clone().into_inner(),
                        MinimalPropertiesEntry {
                            name: validate_resource_name(snapshot.name.as_ref())?,
                            name_span: snapshot.name.span().clone(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: snapshot_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(functions) = other.functions {
            for function_value in functions {
                let function = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&function_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                // Function node names are always derived as lowercase from the SQL
                // filename. Normalize the YAML `name:` to lowercase so the lookup
                // succeeds even when the author used a different case (e.g.
                // `RENAME_DATA_CLOUD_COLUMN` vs `rename_data_cloud_column.sql`).
                let normalized_name = function.name.to_ascii_lowercase();
                if normalized_name != function.name {
                    emit_warn_log_message(
                        ErrorCode::FunctionNameCaseMismatch,
                        format!(
                            "Function name '{}' in '{}' does not match the expected lowercase \
                             form '{}'. dbt derives the function node name from the SQL filename \
                             (always lowercase), so the YAML entry will be matched \
                             case-insensitively. Consider renaming it to '{}' to avoid ambiguity.",
                            function.name,
                            properties_path.display(),
                            normalized_name,
                            normalized_name,
                        ),
                        io_args.status_reporter.as_ref(),
                    );
                }
                if let Some(existing_function) = self.functions.get_mut(&normalized_name) {
                    existing_function
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.functions.insert(
                        normalized_name,
                        MinimalPropertiesEntry {
                            name: validate_resource_name(&function.name)?,
                            name_span: Span::default(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: function_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(exposures) = other.exposures {
            for exposure_value in exposures {
                let exposure = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&exposure_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                self.exposures.insert(
                    exposure.name.clone(),
                    MinimalPropertiesEntry {
                        name: validate_resource_name(&exposure.name)?,
                        name_span: Span::default(),
                        relative_path: properties_path.to_path_buf(),
                        schema_value: exposure_value,
                        table_value: None,
                        version_info: None,
                        duplicate_paths: vec![],
                    },
                );
            }
        }
        if let Some(metrics) = other.metrics {
            for metric_value in metrics {
                let metric = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&metric_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_metric) = self.metrics.get_mut(&metric.name) {
                    existing_metric
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.metrics.insert(
                        metric.name.clone(),
                        MinimalPropertiesEntry {
                            name: validate_resource_name(&metric.name)?,
                            name_span: Span::default(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: metric_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(saved_queries) = other.saved_queries {
            for saved_query_value in saved_queries {
                let saved_query = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&saved_query_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_saved_query) = self.saved_queries.get_mut(&saved_query.name) {
                    existing_saved_query
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.saved_queries.insert(
                        saved_query.name.clone(),
                        MinimalPropertiesEntry {
                            name: validate_resource_name(&saved_query.name)?,
                            name_span: Span::default(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: saved_query_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(unit_tests) = other.unit_tests {
            for unit_test_value in unit_tests {
                let unit_test = into_typed_with_jinja::<MinimalUnitTestValue, _>(
                    name_stripped(&unit_test_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_unit_test) = self.unit_tests.get_mut(unit_test.name.as_ref()) {
                    existing_unit_test
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.unit_tests.insert(
                        unit_test.name.clone().into_inner(),
                        MinimalPropertiesEntry {
                            name: validate_resource_name(unit_test.name.as_ref())?,
                            name_span: unit_test.name.span().clone(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: unit_test_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(tests) = other.tests {
            for test_value in tests {
                let test = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&test_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_test) = self.tests.get_mut(&test.name) {
                    existing_test
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.tests.insert(
                        test.name.clone(),
                        MinimalPropertiesEntry {
                            name: validate_resource_name(&test.name)?,
                            name_span: Span::default(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: test_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(data_tests) = other.data_tests {
            for test_value in data_tests {
                let test = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&test_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_test) = self.tests.get_mut(&test.name) {
                    existing_test
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.tests.insert(
                        test.name.clone(),
                        MinimalPropertiesEntry {
                            name: validate_resource_name(&test.name)?,
                            name_span: Span::default(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: test_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(groups) = other.groups {
            for group_value in groups {
                let group = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&group_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_group) = self.groups.get_mut(&group.name) {
                    existing_group
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.groups.insert(
                        group.name.clone(),
                        MinimalPropertiesEntry {
                            name: group.name.clone(),
                            name_span: Span::default(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: group_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }
        if let Some(macros) = other.macros {
            for macro_value in macros {
                // Only `name` is needed here. The full `MacrosProperties` renders every string
                // field and discards it, double-rendering what `apply_macro_patches` renders.
                let macro_props = into_typed_with_jinja::<MinimalSchemaValue, _>(
                    name_stripped(&macro_value),
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name_from_ctx(jinja_env, base_ctx),
                    true,
                )?;
                if let Some(existing_macro) = self.macros.get_mut(&macro_props.name) {
                    existing_macro
                        .duplicate_paths
                        .push(properties_path.to_path_buf());
                } else {
                    self.macros.insert(
                        macro_props.name.clone(),
                        MinimalPropertiesEntry {
                            name: validate_resource_name(&macro_props.name)?,
                            name_span: Span::default(),
                            relative_path: properties_path.to_path_buf(),
                            schema_value: macro_value,
                            table_value: None,
                            version_info: None,
                            duplicate_paths: vec![],
                        },
                    );
                }
            }
        }

        Ok(())
    }
}

fn validate_resource_name(name: &str) -> FsResult<String> {
    // Check for the space character for now. This can be extended anytime we deprecate
    // more of special characters like !@#%$":'
    if name.chars().any(|c| matches!(c, ' ')) {
        let err = fs_err!(
            ErrorCode::DbtYamlValidationError,
            "Resource name '{}' contains forbidden characters",
            name
        );
        Err(err)
    } else {
        Ok(name.to_string())
    }
}

#[allow(clippy::cognitive_complexity)]
pub fn resolve_minimal_properties(
    arg: &ResolveArgs,
    package: &DbtPackage,
    root_package: &DbtPackage,
    root_package_name: &str,
    root_project_configs: &RootProjectConfigs,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    token: &CancellationToken,
    adapter_type: AdapterType,
) -> FsResult<MinimalProperties> {
    let mut minimal_resolved_properties = MinimalProperties {
        semantic_layer_spec_is_legacy: false,
        ..Default::default()
    };

    let is_dependency = package.dbt_project.name != root_package_name;
    let semantic_model_config_resolver = ProjectConfigResolver::build(
        root_project_configs.semantic_models.clone(),
        is_dependency,
        || {
            init_project_config(
                &package.dbt_project.semantic_models,
                (),
                Some(package.dbt_project.name.as_str()),
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref()),
                adapter_type,
            )
        },
        adapter_type,
    )?;

    for dbt_asset in package.dbt_properties.iter().dedup() {
        token.check_cancellation()?;
        let absolute_path = dbt_asset.base_path.join(&dbt_asset.path);
        let display_path = dbt_asset.to_display_path(&arg.io.in_dir);
        let asset_name = dbt_asset
            .path
            .file_stem()
            .expect("File name can't be empty")
            .to_string_lossy();
        let span = create_debug_span(AssetParsed::new_with_phase_from_context(
            package.dbt_project.name.clone(),
            asset_name.to_string(),
            dbt_asset.path.display().to_string(),
            display_path.display().to_string(),
            None,
        ));

        let dependency_package_name = if package.dbt_project.name != root_package_name {
            Some(package.dbt_project.name.as_str())
        } else {
            None
        };

        {
            let _guard = span.enter();
            let input = try_read_yml_to_str(&absolute_path)?;

            let result = match from_yaml_raw::<DbtPropertiesFileValues>(
                &input,
                Some(&absolute_path),
                true,
                dependency_package_name,
            ) {
                Ok(properties_file_values) => {
                    let properties_path = &dbt_asset.path;
                    minimal_resolved_properties.extend_from_minimal_properties_file(
                        properties_file_values.clone(),
                        jinja_env,
                        properties_path,
                        base_ctx,
                    )?;

                    if !minimal_resolved_properties.semantic_layer_spec_is_legacy
                        && let Some(_semantic_models) = properties_file_values.semantic_models
                    {
                        // Check whether the root project has explicitly disabled this package's
                        // semantic models. If so, suppress the legacy warning and skip them.
                        let has_enabled_package_semantic_models = !semantic_model_config_resolver
                            .is_disabled_by_root_overlay(std::slice::from_ref(
                                &package.dbt_project.name,
                            ));

                        if has_enabled_package_semantic_models {
                            // Top level semantic models are not allowed anymore
                            // TODO: edit copy to encourage user to use auto-fix.
                            emit_warn_log_message(
                                ErrorCode::SemanticModelDeprecated,
                                format!(
                                    "The package '{}' defines semantic models and metrics using the legacy YAML. Please migrate to the new YAML to use the semantic layer with dbt.",
                                    &package.dbt_project.name,
                                ),
                            );

                            minimal_resolved_properties.semantic_layer_spec_is_legacy = true;
                        }
                    }

                    Ok(())
                }
                Err(e) => Err(e),
            };

            // Record both success and failure statuses to the span, but continue processing
            // regardless of outcome.
            let _ = result.as_ref().record_status(&span);
            if let Err(e) = result {
                emit_strict_parse_error(*e, dependency_package_name);
            }
        }
    }
    Ok(minimal_resolved_properties)
}

#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub version: String,
    pub latest_version: String,
    pub versioned_name: String,
    /// This version's raw `config:` mapping. Consumed only by
    /// [`merge_version_config_into_schema_value`]; a second merge would apply it twice.
    pub version_config: Verbatim<Option<dbt_yaml::Value>>,
    // TODO: Remove this and figure out more efficient way to handle this
    pub all_versions: BTreeMap<String, String>,
}

/// Deep-merges a version's `config:` onto the model-level `config:` in that version's own clone of
/// the schema.yml entry, per `deep_merge(target.config, unparsed_version.config)` in dbt-mantle
/// `core/dbt/parser/schemas.py:1093`. Both the rendered and unrendered pipelines read
/// `mpe.schema_value`, so merging here keeps them from disagreeing.
fn merge_version_config_into_schema_value(
    schema_value: &mut dbt_yaml::Value,
    version_config: &Verbatim<Option<dbt_yaml::Value>>,
) {
    let Some(version_config) = (*version_config).as_ref() else {
        return;
    };
    if !version_config.is_mapping() {
        return;
    }
    let Some(schema_mapping) = schema_value.as_mapping_mut() else {
        return;
    };
    match schema_mapping.get_mut("config") {
        Some(model_config) => deep_merge_yaml(model_config, version_config),
        None => {
            schema_mapping.insert(
                dbt_yaml::Value::string("config".to_string()),
                version_config.clone(),
            );
        }
    }
}

// Collect and build a properites config for all versions of a model
pub fn collect_model_version_info(
    model: &MinimalSchemaValue,
) -> Vec<(String, Option<VersionInfo>)> {
    if let Some(versions) = &model.versions {
        let mut version_entries = versions
            .iter()
            .map(|v| {
                let version = match &v.v {
                    dbt_yaml::Value::String(s, _) => Some(s.to_string()),
                    dbt_yaml::Value::Number(n, _) => Some(n.to_string()),
                    _ => None,
                }
                .unwrap_or_else(|| {
                    panic!("Version '{:?}' does not meet the required format", v.v);
                });

                let versioned_name = format!("{}_v{}", model.name, version);

                let defined_in = v
                    .defined_in
                    .as_deref()
                    .map(|s| s.strip_suffix(".sql").unwrap_or(s).to_string());

                let version_config = v.config.clone();

                (
                    version,
                    defined_in.unwrap_or(versioned_name),
                    version_config,
                )
            })
            .collect::<Vec<_>>();
        let latest_version = model
            .latest_version
            .clone()
            .map(|v| match v {
                FloatOrString::String(s) => s,
                FloatOrString::Number(n) => n.to_string(),
            })
            .unwrap_or_else(|| {
                // Try parsing as numbers first
                let numeric_versions: Vec<_> = version_entries
                    .iter()
                    .filter_map(|(v, _, _)| v.parse::<f32>().ok())
                    .collect();

                if numeric_versions.len() == version_entries.len() {
                    // If all versions are numeric, use highest number
                    numeric_versions
                        .iter()
                        .reduce(|a, b| if a > b { a } else { b })
                        .map(|n| n.to_string())
                        .expect("Versions should not be empty")
                } else {
                    // Otherwise use lexicographically last
                    version_entries
                        .iter()
                        .map(|(v, _, _)| v)
                        .max()
                        .unwrap()
                        .clone()
                }
            });

        // Find the config for the latest version from existing version entries
        let latest_version_config = version_entries
            .iter()
            .find(|(v, _, _)| v == &latest_version)
            .map(|(_, _, config)| config.clone())
            .unwrap_or_else(|| Verbatim::from(None));

        // Only add the latest version by model.name if it's not already in the list (as in, defined by a defined_in)
        if !version_entries.iter().any(|(_, d, _)| d == &model.name) {
            // how do I get the config for the latest version?
            version_entries.push((
                latest_version.clone(),
                model.name.clone(),
                latest_version_config,
            ));
        }
        version_entries
            .iter()
            .map(|(v, d, config)| {
                (
                    d.clone(),
                    Some(VersionInfo {
                        version: v.clone(),
                        latest_version: latest_version.clone(),
                        versioned_name: d.clone(),
                        all_versions: version_entries
                            .iter()
                            .map(|(v, d, _)| (v.clone(), d.clone()))
                            .collect(),
                        version_config: config.clone(),
                    }),
                )
            })
            .collect()
    } else {
        vec![(model.name.clone(), None)]
    }
}

/// If the `tables` field of a source YAML mapping is a single Jinja expression
/// (e.g. `"{{ var('source_tables') }}"`), evaluate it and replace the string
/// with the rendered sequence in-place. This is necessary because
/// `MinimalSchemaValue.tables` is wrapped in `Verbatim` (to protect table
/// *contents* from premature rendering), which also blocks the field-level
/// Jinja transform that would normally expand the string into a sequence.
/// Clone of a schema-yaml entry with `name`'s trailing newline dropped, before Jinja runs.
fn name_stripped(value: &dbt_yaml::Value) -> dbt_yaml::Value {
    let mut value = value.clone();
    strip_one_trailing_newline_at_keys(&mut value, &["name"]);
    value
}

fn pre_render_tables_field(
    source_value: &mut dbt_yaml::Value,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
) {
    let Some(mapping) = source_value.as_mapping_mut() else {
        return;
    };
    let tables_key = dbt_yaml::Value::string("tables".to_string());
    let jinja_expr = mapping
        .get(&tables_key)
        .and_then(|v| v.as_str())
        .filter(|s| dbt_jinja_utils::serde::check_single_expression_without_whitepsace_control(s))
        .map(|s| s[2..s.len() - 2].trim().to_string());

    if let Some(expr) = jinja_expr {
        if let Ok(compiled) = jinja_env.compile_expression(&expr) {
            if let Ok(result) = compiled.eval(base_ctx, &[]) {
                if let Ok(val) = dbt_yaml::to_value(&result) {
                    if val.is_sequence() {
                        mapping.insert(tables_key, val);
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn make_function_yaml_value(name: &str) -> dbt_yaml::Value {
        let yaml = format!("name: {name}\n");
        dbt_yaml::from_str(&yaml).expect("valid yaml")
    }

    /// Inserting a function whose YAML `name:` is uppercase must store the
    /// entry under the lowercase key so the renderer (which derives the node
    /// name from the lowercase SQL filename) can find it.
    #[test]
    fn test_function_name_normalized_to_lowercase_on_insert() {
        let mut props = MinimalProperties::default();

        let uppercase_name = "RENAME_DATA_CLOUD_COLUMN";
        let lowercase_name = "rename_data_cloud_column";

        let normalized = uppercase_name.to_ascii_lowercase();
        props.functions.insert(
            normalized.clone(),
            MinimalPropertiesEntry {
                name: uppercase_name.to_string(),
                name_span: Default::default(),
                relative_path: std::path::PathBuf::from("schema.yml"),
                schema_value: make_function_yaml_value(uppercase_name),
                table_value: None,
                version_info: None,
                duplicate_paths: vec![],
            },
        );

        // Must be reachable via lowercase key (how renderer looks it up)
        assert!(
            props.functions.contains_key(lowercase_name),
            "expected key '{lowercase_name}' after inserting uppercase '{uppercase_name}'"
        );

        // Original casing preserved in the entry itself
        let entry = props.functions.get(lowercase_name).unwrap();
        assert_eq!(entry.name, uppercase_name);

        // Uppercase key must NOT exist (that was the silent no-op bug)
        assert!(!props.functions.contains_key(uppercase_name));
    }

    /// A function whose YAML `name:` is already lowercase is stored as-is.
    #[test]
    fn test_function_name_already_lowercase_unchanged() {
        let mut props = MinimalProperties::default();
        let name = "my_udf";

        props.functions.insert(
            name.to_string(),
            MinimalPropertiesEntry {
                name: name.to_string(),
                name_span: Default::default(),
                relative_path: std::path::PathBuf::from("schema.yml"),
                schema_value: make_function_yaml_value(name),
                table_value: None,
                version_info: None,
                duplicate_paths: vec![],
            },
        );

        assert!(props.functions.contains_key(name));
    }
}