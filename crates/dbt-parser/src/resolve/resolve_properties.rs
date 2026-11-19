use crate::args::ResolveArgs;
use crate::dbt_project_config::{
    ProjectConfigResolver, RootProjectConfigs, disallow_plus_prefix_from_flags, init_project_config,
};
use crate::experimental::ActiveExperimentals;
use crate::resolve::properties_index::{
    IndexCx, PackageProperties, PropertiesFile, name_stripped, validate_resource_name,
};
use crate::resolve::resolvable_node::ResolvableNode;
use crate::resolve::resolve_utils::deep_merge_yaml;
use dbt_adapter_core::AdapterType;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_utils::try_read_yml_to_str;
use dbt_common::tracing::dbt_emit::{emit_strict_parse_error, emit_warn_log_message};
use dbt_common::tracing::span_info::SpanStatusRecorder as _;
use dbt_common::{ErrorCode, FsResult, create_debug_span};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::schemas::properties::{MinimalSchemaValue, MinimalTableValue};
use dbt_schemas::schemas::serde::FloatOrString;
use dbt_schemas::state::DbtPackage;
use dbt_telemetry::AssetParsed;
use dbt_yaml::{Span, Verbatim};
use itertools::Itertools;
use minijinja::Value as MinijinjaValue;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::Instrument;

/// The top-level keys of properties files that this pass reads a list of items from, besides the
/// keys of the kinds of node.
const LIST_KEYS: [&str; 3] = ["models", "sources", "semantic_models"];

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

#[derive(Default, Debug)]
pub struct MinimalProperties {
    pub source_tables: BTreeMap<(String, String), MinimalPropertiesEntry>,
    pub models: BTreeMap<String, MinimalPropertiesEntry>,
    pub semantic_layer_spec_is_legacy: bool,
}

// impl try extend from MinimalResolvedProperties
#[allow(clippy::cognitive_complexity)]
impl MinimalProperties {
    pub async fn extend_from_minimal_properties_file(
        &mut self,
        file: &mut PropertiesFile,
        jinja_env: &Arc<JinjaEnv>,
        properties_path: &Path,
        base_ctx: &BTreeMap<String, MinijinjaValue>,
    ) -> FsResult<()> {
        // TODO: This is a bit repetetive. Can be shortened!
        if let Some(models) = file.take_items("models") {
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
        if let Some(sources) = file.take_items("sources") {
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
        Ok(())
    }
}

#[allow(clippy::cognitive_complexity)]
pub async fn resolve_minimal_properties(
    kinds: &[&dyn ResolvableNode],
    arg: &ResolveArgs,
    package: &DbtPackage,
    root_package: &DbtPackage,
    root_package_name: &str,
    root_project_configs: &RootProjectConfigs,
    jinja_env: &Arc<JinjaEnv>,
    base_ctx: &Arc<BTreeMap<String, MinijinjaValue>>,
    token: &CancellationToken,
    adapter_type: AdapterType,
    experimentals: &ActiveExperimentals,
) -> FsResult<(MinimalProperties, PackageProperties)> {
    let mut minimal_resolved_properties = MinimalProperties {
        semantic_layer_spec_is_legacy: false,
        ..Default::default()
    };
    let mut properties = PackageProperties::default();
    let list_keys: Vec<&str> = LIST_KEYS
        .into_iter()
        .chain(
            kinds
                .iter()
                .flat_map(|kind| kind.properties_keys())
                .copied(),
        )
        .collect();
    let index_cx = IndexCx {
        jinja_env,
        base_ctx,
        dependency_package_name: dependency_package_name_from_ctx(jinja_env, base_ctx),
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

    // Entries under `experimental:` for an experimental that is on are collected; any others are
    // ignored.
    let mut ignored_experimental_keys: BTreeSet<String> = BTreeSet::new();

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

        async {
            let input = try_read_yml_to_str(&absolute_path)?;

            let result = match PropertiesFile::read(
                &input,
                &dbt_asset.path,
                &absolute_path,
                dependency_package_name,
                &list_keys,
            ) {
                Ok(mut file) => {
                    let properties_path = &dbt_asset.path;
                    minimal_resolved_properties
                        .extend_from_minimal_properties_file(
                            &mut file,
                            jinja_env,
                            properties_path,
                            base_ctx,
                        )
                        .await?;
                    let experimental = file.experimental.take().unwrap_or_default();
                    let has_semantic_models = file.has_items("semantic_models");
                    let file_id = properties.index_file(kinds, file, &index_cx).await?;

                    for (key, value) in experimental {
                        match experimentals.key_read(&key) {
                            Some(key) => properties.index_experimental_items(
                                file_id, key, value, &index_cx,
                            )?,
                            None => {
                                ignored_experimental_keys.insert(key);
                            }
                        }
                    }

                    if !minimal_resolved_properties.semantic_layer_spec_is_legacy
                        && has_semantic_models
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
            Ok::<(), Box<dbt_common::FsError>>(())
        }
        .instrument(span.clone())
        .await?;
    }
    for key in ignored_experimental_keys {
        let package_name = &package.dbt_project.name;
        if experimentals.is_reserved(&key) {
            emit_warn_log_message(
                ErrorCode::ExperimentalNotEnabled,
                format!(
                    "Ignoring `experimental: {key}` in package '{package_name}': {}",
                    experimentals.why_ignored(&key)
                ),
            );
        } else {
            emit_warn_log_message(
                ErrorCode::UnknownExperimental,
                format!(
                    "Ignoring `experimental: {key}` in package '{package_name}': there is no \
                     `{key}` experimental"
                ),
            );
        }
    }
    Ok((minimal_resolved_properties, properties))
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
