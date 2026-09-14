use crate::args::ResolveArgs;
use crate::dbt_project_config::{
    ProjectConfigResolver, RootProjectConfigs, disallow_plus_prefix_from_flags, init_project_config,
};
use crate::resolve::resolve_utils::{
    build_unrendered_config, err_resource_name_has_spaces, extract_config_map,
    validate_node_adapter,
};
use crate::utils::{
    RelationComponents, extract_resource_config_from_raw_project, get_node_fqn,
    register_duplicate_resource, trigger_duplicate_errors, update_node_relation_components,
};
use crate::validation::check_node_static_analysis;
use dbt_adapter_core::AdapterType;
use dbt_common::io_args::{StaticAnalysisKind, StaticAnalysisOffReason};
use dbt_common::path::DbtPath;
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_warn_log_from_fs_error};
use dbt_common::{ErrorCode, FsResult, fs_err, stdfs};
use dbt_frontend_common::Dialect;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::node_resolver::NodeResolver;
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::dbt_utils::resolve_package_quoting;
use dbt_schemas::dbt_utils::validate_delimiter;
use dbt_schemas::schemas::common::{DbtChecksum, DbtMaterialization, DbtQuoting, NodeDependsOn};
use dbt_schemas::schemas::dbt_column::process_columns;
use dbt_schemas::schemas::properties::SeedProperties;
use dbt_schemas::schemas::{CommonAttributes, DbtSeed, DbtSeedAttr, NodeBaseAttributes};
use dbt_schemas::state::{DbtPackage, GenericTestAsset};
use dbt_schemas::state::{ModelStatus, NodeResolverTracker};
use indexmap::IndexMap;
use minijinja::value::Value as MinijinjaValue;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use super::resolve_properties::MinimalPropertiesEntry;
use super::resolve_tests::persist_generic_data_tests::TestableNodeTrait;
use super::resolve_tests::persist_generic_data_tests::{
    TestUnrenderedConfigs, extract_test_unrendered_configs,
};

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub async fn resolve_seeds(
    arg: &ResolveArgs,
    mut seed_properties: BTreeMap<String, MinimalPropertiesEntry>,
    package: &DbtPackage,
    // Authored quoting per declared adapter name. See `resolve_models`.
    adapter_quoting: &IndexMap<AdapterType, DbtQuoting>,
    root_package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    database: &str,
    schema: &str,
    default_adapter: AdapterType,
    package_name: &str,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    collected_generic_tests: &mut Vec<GenericTestAsset>,
    test_name_truncations: &mut HashMap<String, String>,
    seen_generic_test_paths: &mut HashMap<PathBuf, String>,
    node_resolver: &mut NodeResolver,
) -> FsResult<(HashMap<String, Arc<DbtSeed>>, HashMap<String, Arc<DbtSeed>>)> {
    let mut seeds: HashMap<String, Arc<DbtSeed>> = HashMap::new();
    let mut disabled_seeds: HashMap<String, Arc<DbtSeed>> = HashMap::new();
    let io_args = &arg.io;
    let dependency_package_name = dependency_package_name_from_ctx(jinja_env, base_ctx);

    let is_dependency = dependency_package_name.is_some();
    let raw_local_project_config = extract_resource_config_from_raw_project(
        &package.raw_project_yml,
        "seeds",
        default_adapter,
    )?;
    let raw_root_project_cfg = if is_dependency {
        Some(extract_resource_config_from_raw_project(
            &root_package.raw_project_yml,
            "seeds",
            default_adapter,
        )?)
    } else {
        None
    };

    let raw_schema_yml_configs: BTreeMap<String, BTreeMap<String, dbt_yaml::Value>> =
        seed_properties
            .iter()
            .filter_map(|(name, mpe)| {
                let config_map = extract_config_map(&mpe.schema_value)?;
                Some((name.clone(), config_map))
            })
            .collect();

    // Raw (unrendered) schema.yml test config blocks, consumed by `persist` to populate
    // generic tests' unrendered_config with their schema.yml config.
    let raw_test_configs: BTreeMap<String, TestUnrenderedConfigs> = seed_properties
        .iter()
        .map(|(name, mpe)| {
            (
                name.clone(),
                extract_test_unrendered_configs(&mpe.schema_value),
            )
        })
        .collect();

    let mut seed_root_dirs: Vec<String> = package
        .dbt_project
        .seed_paths
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|path| {
            Path::new(&path)
                .components()
                .next()
                .map(|comp| comp.as_os_str().to_string_lossy().to_string())
        })
        .collect();
    if seed_root_dirs.is_empty() {
        seed_root_dirs.push("seeds".to_string());
    }

    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.seeds.clone(),
        is_dependency,
        || {
            init_project_config(
                &package.dbt_project.seeds,
                DbtQuoting::default(),
                dependency_package_name,
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref()),
                default_adapter,
            )
        },
        default_adapter,
    )?
    .with_resolve_defaults(arg.static_analysis.unwrap_or_default());

    // TODO: update this to be relative of the root project
    let mut duplicate_errors = Vec::new();
    // Track seed names seen so far (name → relative path) to detect duplicates across subdirs
    let mut seen_seed_names: HashMap<String, PathBuf> = HashMap::new();
    for seed_file in package.seed_files.iter() {
        // Validate that path extension is one of csv, parquet, json, jsonl, or ndjson
        let path = seed_file.path.clone();
        let path_extension = path.extension().unwrap_or_default().to_ascii_lowercase();
        if path_extension != "csv"
            && path_extension != "parquet"
            && path_extension != "json"
            && path_extension != "jsonl"
            && path_extension != "ndjson"
        {
            continue;
        }

        let seed_name_owned = if path_extension == "parquet" {
            let components: Vec<String> = path
                .iter()
                .map(|part| part.to_string_lossy().to_string())
                .collect();
            if components.len() >= 2 {
                let parent_component = &components[components.len() - 2];
                let parent_is_seed_root =
                    seed_root_dirs.iter().any(|root| root == parent_component);
                if parent_is_seed_root && components.len() == 2 {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| parent_component.clone())
                } else {
                    parent_component.clone()
                }
            } else {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string())
            }
        } else {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string())
        };
        let seed_name = seed_name_owned.as_str();
        if seed_name.contains(' ') {
            return Err(err_resource_name_has_spaces(seed_name, &path));
        }

        // Detect two seeds with the same name in different subdirectories
        let original_file_path_for_name_check =
            stdfs::diff_paths(seed_file.base_path.join(&path), &io_args.in_dir)?;
        if let Some(existing_path) = seen_seed_names.get(seed_name) {
            let err_msg = format!(
                "dbt found two seeds with the name \"{}\".\n  Since these resources have the same name, dbt will be unable to find the correct resource when ref(\"{}\") is used.\n  To fix this, change the name of one of these resources:\n  - seed.{}.{} ({})\n  - seed.{}.{} ({})",
                seed_name,
                seed_name,
                package_name,
                seed_name,
                existing_path.display(),
                package_name,
                seed_name,
                original_file_path_for_name_check.display(),
            );
            duplicate_errors.push(
                *fs_err!(code => ErrorCode::InvalidConfig, loc => original_file_path_for_name_check.clone(), "{}", err_msg),
            );
            continue;
        }
        seen_seed_names.insert(seed_name.to_string(), original_file_path_for_name_check);

        let unique_id = format!("seed.{package_name}.{seed_name}");

        let fqn = get_node_fqn(
            package_name,
            path.to_owned(),
            vec![seed_name.to_owned()],
            package.dbt_project.seed_paths.as_ref().unwrap_or(&vec![]),
        );

        // TODO (deferred, not a bug here): dbt-core deep_merges the rendered and unrendered
        // schema.yml configs, which doubles list fields (e.g. tags) — the same class of issue as
        // the versioned-model `unrendered_config` divergence documented at
        // `.agents/state-modified-conformance.md` § "Versioned models" (GT4). Seeds have no
        // `versions:`, so there is nothing version-level to double-apply here today, but if seeds
        // ever grow one, do not replicate the duplication for the same reason models don't:
        // https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/parser/base.py#L424-L426
        let unrendered_config = build_unrendered_config(
            &fqn,
            &raw_local_project_config,
            raw_root_project_cfg.as_ref(),
            raw_schema_yml_configs.get(seed_name),
            None,
            true,
            default_adapter,
        )?;

        // Merge schema_file_info
        let (seed, patch_path) = if let Some(mpe) = seed_properties.remove(seed_name) {
            if !mpe.duplicate_paths.is_empty() {
                register_duplicate_resource(&mpe, seed_name, "seed", &mut duplicate_errors);
            }
            (
                into_typed_with_jinja::<SeedProperties, _>(
                    mpe.schema_value,
                    false,
                    jinja_env,
                    base_ctx,
                    &[],
                    dependency_package_name,
                    true,
                )?,
                Some(mpe.relative_path.clone()),
            )
        } else {
            (SeedProperties::empty(seed_name.to_owned()), None)
        };

        let mut properties_config =
            config_resolver.resolve_with_properties(&fqn, seed.config.as_ref());
        let static_analysis = properties_config.static_analysis.clone();
        check_node_static_analysis(
            &properties_config,
            arg.static_analysis,
            seed_name,
            dependency_package_name,
        );

        // XXX: normalize column_types to uppercase if it is snowflake
        if matches!(default_adapter, AdapterType::Snowflake)
            && let Some(column_types) = &properties_config.column_types
        {
            let column_types = column_types
                .iter()
                .map(|(k, v)| {
                    // Normalize column names for Snowflake case folding.
                    // If the key is not a valid unquoted identifier (e.g. contains
                    // spaces), auto-wrap it in double-quotes before parsing so it
                    // is treated as a quoted (case-preserving) identifier instead
                    // of being rejected. This matches Mantle behavior.
                    // Normalize column names for Snowflake case folding.
                    // If the key is not a valid unquoted identifier (e.g. it
                    // contains spaces), auto-wrap it in SQL double-quotes so it
                    // is treated as a case-preserving quoted identifier instead
                    // of being rejected. This matches Mantle behavior.
                    let key = k.as_str();
                    let sql;
                    let sql_str = if Dialect::Snowflake.parse_identifier(key).is_ok() {
                        key
                    } else {
                        sql = format!("\"{}\"", key.replace('"', "\"\""));
                        sql.as_str()
                    };
                    Ok((
                        Dialect::Snowflake
                            .parse_identifier(sql_str)
                            .map_err(|e| {
                                fs_err!(
                                    code => ErrorCode::InvalidColumnReference,
                                    loc => k.span().clone(),
                                    "Invalid identifier: {e}",
                                )
                            })?
                            .to_value()
                            .into(),
                        v.to_owned(),
                    ))
                })
                .collect::<FsResult<_>>()?;

            properties_config.column_types = Some(column_types);
        }
        let is_enabled = properties_config.enabled;

        let columns = process_columns(
            seed.columns.as_ref(),
            properties_config
                .tags
                .inner()
                .clone()
                .map(|tags| tags.into()),
        )?;

        validate_delimiter(&properties_config.delimiter)?;

        // See `resolve_models`: the flag overrides the config.
        validate_node_adapter(properties_config.adapter, &path)?;
        let resolved_node_adapter = arg.adapter_override.or(properties_config.adapter);

        // Calculate original file path first so we can use it for the checksum
        // if necessary for large seeds
        let original_file_path =
            stdfs::diff_paths(seed_file.base_path.join(&path), &io_args.in_dir)?;

        // See `resolve_models`: both remaining layers depend on the node's
        // `+adapter`, which the config merge cannot know. Written back so
        // `deprecated_config` carries the resolved value too.
        // `propagate` comes straight off the node's own config. Unlike
        // `adapter` there is no target default to fall back to and nothing to
        // inherit: an unset `+propagate` means "publish nowhere".
        let selected_propagate: Vec<AdapterType> = properties_config
            .propagate
            .clone()
            .map(Into::into)
            .unwrap_or_default();
        let selected_adapter = resolved_node_adapter.unwrap_or(default_adapter);
        properties_config.quoting = resolve_package_quoting(
            Some(match adapter_quoting.get(&selected_adapter) {
                Some(authored) => properties_config.quoting.filled_from(authored),
                None => properties_config.quoting,
            }),
            resolved_node_adapter.unwrap_or(default_adapter),
        );

        // Create initial seed with default values
        let mut dbt_seed = DbtSeed {
            __common_attr__: CommonAttributes {
                name: seed_name.to_owned(),
                package_name: package_name.to_owned(),
                path: DbtPath::from(path.to_owned()),
                name_span: dbt_common::Span::default(),
                original_file_path: DbtPath::from(&original_file_path),
                checksum: DbtChecksum::seed_file_checksum(
                    &seed_file.base_path.join(&path),
                    &original_file_path.to_string_lossy(),
                    arg.maximum_seed_size_mib,
                )?,
                patch_path: patch_path.as_ref().map(DbtPath::from),
                unique_id: unique_id.clone(),
                fqn,
                // dbt-core: description is always default ''
                description: Some(seed.description.clone().unwrap_or_default()),
                // dbt-core writes raw_code as `block.contents or ""` for seeds —
                // there's no Jinja source, so it's always empty string, not null.
                raw_code: Some(String::new()),
                language: None,
                tags: properties_config
                    .tags
                    .inner()
                    .clone()
                    .map(Into::into)
                    .unwrap_or_default(),
                classifiers: Default::default(),
                meta: properties_config.meta.clone().unwrap_or_default(),
            },
            __base_attr__: NodeBaseAttributes {
                adapter: selected_adapter,
                propagate: selected_propagate,
                database: database.to_string(), // will be updated below
                schema: schema.to_string(),     // will be updated below
                alias: "".to_owned(),           // will be updated below
                relation_name: None,            // will be updated below
                columns,
                depends_on: NodeDependsOn::default(),
                quoting: properties_config
                    .quoting
                    .try_into()
                    .expect("DbtQuoting -> ResolvedQuoting conversion"),
                materialized: DbtMaterialization::Table,
                static_analysis_off_reason: (*static_analysis == StaticAnalysisKind::Off)
                    .then_some(StaticAnalysisOffReason::ConfiguredOff),
                static_analysis,
                unrendered_config,
                persist_docs: properties_config.persist_docs.clone(),
                ..Default::default()
            },
            __seed_attr__: DbtSeedAttr {
                quote_columns: properties_config.quote_columns.unwrap_or(false),
                column_types: properties_config.column_types.clone(),
                delimiter: properties_config.delimiter.clone().map(|d| d.into_inner()),
                root_path: Some(seed_file.base_path.clone()),
                catalog_name: properties_config.catalog_name.clone(),
            },
            __other__: BTreeMap::new(),
            deprecated_config: properties_config.clone().into(),
        };

        let components = RelationComponents {
            database: properties_config.database.clone(),
            schema: properties_config.schema.clone(),
            alias: properties_config.alias.clone(),
            store_failures: None,
        };

        update_node_relation_components(
            &mut dbt_seed,
            jinja_env,
            &root_package.dbt_project.name,
            package_name,
            base_ctx,
            &components,
            default_adapter,
        )?;

        let status = if is_enabled {
            ModelStatus::Enabled
        } else {
            ModelStatus::Disabled
        };

        match node_resolver.insert_ref(&dbt_seed, default_adapter, status, false) {
            Ok(_) => (),
            Err(e) => {
                let err_with_loc = e.with_location(path.clone());
                emit_error_log_from_fs_error(err_with_loc);
            }
        }

        match status {
            ModelStatus::Enabled => {
                seeds.insert(unique_id, Arc::new(dbt_seed));
                if !arg.skip_creating_generic_tests {
                    seed.as_testable().persist(
                        package_name,
                        &root_package.dbt_project.name,
                        collected_generic_tests,
                        test_name_truncations,
                        seen_generic_test_paths,
                        default_adapter,
                        io_args,
                        patch_path.as_ref().unwrap_or(&path),
                        false,
                        &raw_test_configs.get(seed_name).cloned().unwrap_or_default(),
                    )?;
                }
            }
            ModelStatus::Disabled => {
                disabled_seeds.insert(unique_id, Arc::new(dbt_seed));
            }
            _ => {}
        }
    }

    for (seed_name, mpe) in seed_properties.iter() {
        if !mpe.schema_value.is_null() {
            let err = fs_err!(
                code => ErrorCode::NoNodeForYamlKey,
                loc => mpe.relative_path.clone(),
                "Unused schema.yml entry for seed '{}'",
                seed_name,
            );
            emit_warn_log_from_fs_error(*err);
        }
    }

    trigger_duplicate_errors(&mut duplicate_errors)?;
    Ok((seeds, disabled_seeds))
}
