use crate::args::ResolveArgs;
use crate::dbt_project_config::{
    ProjectConfigResolver, RootProjectConfigs, disallow_plus_prefix_from_flags, init_project_config,
};
use crate::resolve::resolve_utils::build_unrendered_config;
use crate::resolve::resolve_utils::extract_config_map;
use crate::utils::{
    extract_resource_config_from_raw_project, get_node_fqn, get_original_file_path, get_unique_id,
};
use dbt_adapter_core::AdapterType;

use dbt_common::io_args::{StaticAnalysisKind, StaticAnalysisOffReason};
use dbt_common::path::DbtPath;
use dbt_common::tracing::dbt_emit::emit_error_log_from_fs_error;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::schemas::common::{DbtChecksum, DbtMaterialization, NodeDependsOn};
use dbt_schemas::schemas::manifest::saved_query::{
    self, DbtSavedQuery, DbtSavedQueryAttr, SavedQueryExportConfig, SavedQueryParams,
};
use dbt_schemas::schemas::properties::SavedQueriesProperties;
use dbt_schemas::schemas::{CommonAttributes, NodeBaseAttributes};
use dbt_schemas::state::DbtPackage;
use minijinja::value::Value as MinijinjaValue;
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use super::resolve_properties::MinimalPropertiesEntry;

fn extract_raw_export_configs(
    exports: &dbt_yaml::Value,
) -> HashMap<String, BTreeMap<String, dbt_yaml::Value>> {
    let mut export_configs = HashMap::new();
    if let Some(exports) = exports.as_sequence() {
        for export in exports.iter() {
            if let Some(name) = export.get("name").and_then(|n| n.as_str()) {
                let config = extract_config_map(export).unwrap_or_default();
                export_configs.insert(name.into(), config);
            }
        }
    }
    export_configs
}

#[allow(clippy::too_many_arguments)]
pub async fn resolve_saved_queries(
    adapter_type: AdapterType,
    arg: &ResolveArgs,
    package: &DbtPackage,
    root_package: &DbtPackage,
    _root_package_name: &str,
    root_project_configs: &RootProjectConfigs,
    saved_query_properties: &mut BTreeMap<String, MinimalPropertiesEntry>,
    database: &str,
    schema: &str,
    package_name: &str,
    env: Arc<JinjaEnv>,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
) -> FsResult<(
    HashMap<String, Arc<DbtSavedQuery>>,
    HashMap<String, Arc<DbtSavedQuery>>,
)> {
    let mut saved_queries: HashMap<String, Arc<DbtSavedQuery>> = HashMap::new();
    let mut disabled_saved_queries: HashMap<String, Arc<DbtSavedQuery>> = HashMap::new();

    // Return early if no saved queries to process
    if saved_query_properties.is_empty() {
        return Ok((saved_queries, disabled_saved_queries));
    }

    let dependency_package_name = dependency_package_name_from_ctx(&env, base_ctx);
    let is_dependency = dependency_package_name.is_some();
    let _raw_local_project_config = extract_resource_config_from_raw_project(
        &package.raw_project_yml,
        "saved-queries",
        adapter_type,
    )?;
    let _raw_root_project_cfg = if is_dependency {
        Some(extract_resource_config_from_raw_project(
            &root_package.raw_project_yml,
            "saved-queries",
            adapter_type,
        )?)
    } else {
        None
    };

    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.saved_queries.clone(),
        is_dependency,
        || {
            init_project_config(
                &package.dbt_project.saved_queries,
                (),
                dependency_package_name,
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref()),
                adapter_type,
            )
        },
        adapter_type,
    )?;

    // Validate saved query names with regex (similar to exposures)
    let saved_query_name_re = Regex::new(r"[\w-]+$").unwrap();

    for (saved_query_name, mpe) in saved_query_properties.iter_mut() {
        if !mpe.schema_value.is_null() {
            // Validate saved query name
            if !saved_query_name_re.is_match(saved_query_name) {
                let e = fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => mpe.relative_path.clone(),
                    "Saved query name '{}' can only contain letters, numbers, and underscores.",
                    saved_query_name
                );
                emit_error_log_from_fs_error(*e);

                continue;
            }

            let unique_id = get_unique_id(saved_query_name, package_name, None, "saved_query");
            let fqn = get_node_fqn(
                package_name,
                mpe.relative_path.clone(),
                vec![saved_query_name.to_owned()],
                &package.dbt_project.all_source_paths(),
            );

            let schema_value = std::mem::replace(&mut mpe.schema_value, dbt_yaml::Value::null());

            let raw_properties_yml_config = extract_config_map(&schema_value);

            // Keyed by export name for lookup when building each export's unrendered_config.
            let raw_export_configs = schema_value
                .get("exports")
                .map(extract_raw_export_configs)
                .unwrap_or_default();

            // Parse the saved query properties from YAML
            let saved_query_props: SavedQueriesProperties = into_typed_with_jinja(
                schema_value,
                false,
                &env,
                base_ctx,
                &[],
                dependency_package_name,
                true,
            )?;

            // Get combined config from project config and saved query config
            let saved_query_config =
                config_resolver.resolve_with_properties(&fqn, saved_query_props.config.as_ref());
            let is_enabled = saved_query_config.enabled;

            let props_query_params = &saved_query_props.query_params;

            // Create default query params and exports since we're doing minimal implementation
            let query_params = SavedQueryParams {
                metrics: props_query_params.metrics.clone().unwrap_or_default(),
                group_by: props_query_params.group_by.clone().unwrap_or_default(),
                where_: props_query_params
                    .where_
                    .0
                    .clone()
                    .map(|where_clause| where_clause.into()),
                order_by: vec![],
                limit: None, // TODO? not sure where to source from
            };

            let unique_ids_of_nodes_depends_on: Vec<String> = query_params
                .metrics
                .iter()
                .map(|name| get_unique_id(name, package_name, None, "metric"))
                .collect();

            // TODO: we should probably also try to resolve semantic_models for dimensions,
            let depends_on = NodeDependsOn {
                macros: vec![],
                nodes: unique_ids_of_nodes_depends_on,
                nodes_with_ref_location: vec![],
            };

            // schema can be overriden from either Export config or Saved Query config
            // TODO: we should also allow overriding the database in the same manner
            let saved_query_schema = saved_query_config
                .schema
                .clone()
                .unwrap_or_else(|| schema.to_string());

            let exports = saved_query_props
                .exports
                .unwrap_or_default()
                .iter()
                .map(|export| {
                    let config = export.config.clone().unwrap_or_default();

                    saved_query::SavedQueryExport {
                        name: export.name.clone(),
                        config: SavedQueryExportConfig {
                            export_as: config.export_as.unwrap_or_default(),
                            schema_name: Some(
                                config
                                    .schema
                                    .unwrap_or_else(|| saved_query_schema.to_string()),
                            ),
                            alias: Some(config.alias.unwrap_or_else(|| export.name.clone())),
                            database: Some(database.to_string()),
                        },
                        unrendered_config: raw_export_configs
                            .get(&export.name)
                            .cloned()
                            .unwrap_or_default(),
                    }
                })
                .collect::<Vec<saved_query::SavedQueryExport>>();

            // FIXME: this is likely not completely correct, we should figure out the "right" solution
            // use the first export destination for the saved query values,
            // if there's no exports then the saved query doesn't technially materialize
            let schema = exports
                .first()
                .map(|export| export.config.schema_name.clone())
                .unwrap_or_default();
            let database = exports
                .first()
                .map(|export| export.config.database.clone())
                .unwrap_or_default();
            let alias = exports
                .first()
                .map(|export| export.config.alias.clone())
                .unwrap_or_default();

            // TODO: Core has a bug where saved query unrendered_config only captures YAML-level
            // config and excludes dbt_project.yml config entirely. In _generate_saved_query_config(),
            // `patch_config_dict` is built only from `target.config` (the YAML file), so project-level
            // config never reaches UnrenderedConfigGenerator:
            // https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/parser/schema_yaml_readers.py#L1236-L1246
            // Passing `local` and `root` as None to match Core's behavior. Once Core fixes this,
            // pass `&raw_local_project_config` and `raw_root_project_cfg.as_ref()` instead.
            let unrendered_config = build_unrendered_config(
                &fqn,
                &crate::utils::RawProjectConfig::empty(),
                None,
                raw_properties_yml_config.as_ref(),
                None,
                false,
                adapter_type,
            )?;

            let dbt_saved_query = DbtSavedQuery {
                __common_attr__: CommonAttributes {
                    name: saved_query_name.clone(),
                    package_name: package_name.to_string(),
                    path: DbtPath::from(&mpe.relative_path),
                    original_file_path: get_original_file_path(
                        &package.package_root_path,
                        &arg.io.in_dir,
                        &mpe.relative_path,
                    ),
                    name_span: dbt_common::Span::from_serde_span(
                        mpe.name_span.clone(),
                        mpe.relative_path.clone(),
                    ),
                    patch_path: Some(DbtPath::from(&mpe.relative_path)),
                    unique_id: unique_id.clone(),
                    fqn,
                    description: saved_query_props.description,
                    checksum: DbtChecksum::default(),
                    raw_code: None,
                    language: None,
                    tags: saved_query_config
                        .tags
                        .inner()
                        .clone()
                        .map(Into::into)
                        .unwrap_or_default(),
                    classifiers: Default::default(),
                    meta: saved_query_config.meta.clone().unwrap_or_default(),
                },
                __base_attr__: NodeBaseAttributes {
                    // Not executed against a warehouse; records the target it parsed under.
                    adapter: adapter_type,
                    // This node type has no `+propagate` config; nothing is published.
                    propagate: Vec::new(),
                    effective_propagation_target: None,
                    database: database.unwrap_or_default(),
                    schema: schema.unwrap_or_default(),
                    alias: alias.unwrap_or_default(),
                    relation_name: None,         // TODO: what should this be?
                    quoting: Default::default(), // TODO: what should this be?
                    materialized: DbtMaterialization::Unknown("export".to_string()),
                    static_analysis: StaticAnalysisKind::Off.into(),
                    static_analysis_off_reason: Some(StaticAnalysisOffReason::UnableToFetchSchema),
                    compute: None,
                    enabled: true,
                    extended_model: false,
                    persist_docs: None,
                    columns: Default::default(),
                    refs: vec![],
                    sources: vec![],
                    functions: vec![],
                    metrics: vec![],
                    depends_on,
                    quoting_ignore_case: false,
                    unrendered_config: Default::default(),
                },
                __saved_query_attr__: DbtSavedQueryAttr {
                    query_params,
                    exports,
                    label: saved_query_props.label,
                    metadata: None,
                    unrendered_config,
                    group: saved_query_config.group.clone(),
                    created_at: chrono::Utc::now().timestamp() as f64,
                    cache: saved_query_config.cache.clone(),
                },
                deprecated_config: saved_query_config.into(),
                __other__: BTreeMap::new(),
            };

            // Check if saved query is enabled (following exposures pattern)
            if is_enabled {
                saved_queries.insert(unique_id, Arc::new(dbt_saved_query));
            } else {
                disabled_saved_queries.insert(unique_id, Arc::new(dbt_saved_query));
            }
        }
    }

    Ok((saved_queries, disabled_saved_queries))
}
