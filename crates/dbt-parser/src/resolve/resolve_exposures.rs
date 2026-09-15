use crate::args::ResolveArgs;
use crate::dbt_project_config::{
    ProjectConfigResolver, RootProjectConfigs, disallow_plus_prefix_from_flags, init_project_config,
};
use crate::resolve::resolve_utils::build_unrendered_config;
use crate::resolve::resolve_utils::extract_config_map;
use crate::utils::{extract_resource_config_from_raw_project, get_node_fqn};
use dbt_adapter_core::AdapterType;
use dbt_common::CodeLocationWithFile;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::path::DbtPath;
use dbt_common::tracing::dbt_emit::emit_error_log_from_fs_error;
use dbt_common::{ErrorCode, FsResult, err, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::phases::parse::build_resolve_model_context;
use dbt_jinja_utils::phases::parse::sql_resource::SqlResource;
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_jinja_utils::utils::{dependency_package_name_from_ctx, render_extract_ref_or_source_expr};
use dbt_schemas::schemas::common::NodeDependsOn;
use dbt_schemas::schemas::nodes::{
    CommonAttributes, DbtExposure, DbtExposureAttr, NodeBaseAttributes,
};
use dbt_schemas::schemas::project::{ExposureConfig, ResolvedExposureConfig};
use dbt_schemas::schemas::properties::ExposureProperties;
use dbt_schemas::schemas::ref_and_source::{DbtRef, DbtSourceWrapper};
use dbt_schemas::schemas::relations::DEFAULT_DBT_QUOTING;
use dbt_schemas::state::{DbtPackage, DbtRuntimeConfig};
use dbt_yaml::Spanned;
use minijinja::value::Value as MinijinjaValue;
use regex::Regex;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::resolve_properties::MinimalPropertiesEntry;

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub async fn resolve_exposures(
    args: &ResolveArgs,
    exposure_properties: &mut BTreeMap<String, MinimalPropertiesEntry>,
    package: &DbtPackage,
    root_package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    database: &str,
    schema: &str,
    adapter_type: AdapterType,
    package_name: &str,
    env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
) -> FsResult<(
    HashMap<String, Arc<DbtExposure>>,
    HashMap<String, Arc<DbtExposure>>,
)> {
    let mut exposures: HashMap<String, Arc<DbtExposure>> = HashMap::new();
    let mut disabled_exposures: HashMap<String, Arc<DbtExposure>> = HashMap::new();
    let dependency_package_name = dependency_package_name_from_ctx(env, base_ctx);
    let is_dependency = dependency_package_name.is_some();
    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.exposures.clone(),
        is_dependency,
        || {
            init_project_config(
                &package.dbt_project.exposures,
                (),
                dependency_package_name,
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref()),
                adapter_type,
            )
        },
        adapter_type,
    )?;

    let raw_local_project_config = extract_resource_config_from_raw_project(
        &package.raw_project_yml,
        "exposures",
        adapter_type,
    )?;
    let raw_root_project_cfg = if is_dependency {
        Some(extract_resource_config_from_raw_project(
            &root_package.raw_project_yml,
            "exposures",
            adapter_type,
        )?)
    } else {
        None
    };

    // Retrieve exposures from yaml
    let exposure_name_re = Regex::new(r"[\w-]+$").unwrap();
    for (exposure_name, mpe) in exposure_properties.iter_mut() {
        if !mpe.schema_value.is_null() {
            // validate dbt_exposure name
            if !exposure_name_re.is_match(exposure_name) {
                let e = fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => mpe.relative_path.clone(),
                    "Exposure name '{}' can only contain letters, numbers, and underscores.",
                    exposure_name
                );
                emit_error_log_from_fs_error(*e);
            }

            let unique_id = format!("exposure.{}.{}", &package_name, exposure_name);

            let fqn = get_node_fqn(
                package_name,
                mpe.relative_path.clone(),
                vec![exposure_name.to_owned()],
                &package.dbt_project.all_source_paths(),
            );

            let schema_value = std::mem::replace(&mut mpe.schema_value, dbt_yaml::Value::null());
            let raw_properties_yml_config = extract_config_map(&schema_value);
            // ExposureProperties is for the yaml schema
            let exposure: ExposureProperties = into_typed_with_jinja(
                schema_value,
                false,
                env,
                base_ctx,
                &[],
                dependency_package_name,
                true,
            )?;

            // Get combined properties
            let exposure_properties_config =
                config_resolver.resolve_with_properties(&fqn, exposure.config.as_ref());

            //      depends_on:
            //        - ref('model')
            //        - source('test_source', 'test_table')
            //        - metric('metric')

            let is_enabled = exposure_properties_config.enabled;

            // Extract refs, sources & metrics from yaml depends_on
            let (refs, sources, metrics) = if let Some(depends_on) = &exposure.depends_on {
                resolve_yaml_depends_on(
                    depends_on,
                    env,
                    base_ctx,
                    &exposure_properties_config,
                    database,
                    schema,
                    adapter_type,
                    package_name,
                    &root_package.dbt_project.name,
                    fqn.clone(),
                    &mpe.relative_path.to_string_lossy(),
                    args.static_analysis,
                )?
            } else {
                (vec![], vec![], vec![])
            };
            let unrendered_config = build_unrendered_config(
                &fqn,
                &raw_local_project_config,
                raw_root_project_cfg.as_ref(),
                raw_properties_yml_config.as_ref(),
                None,
                false,
                adapter_type,
            )?;

            let dbt_exposure = DbtExposure {
                __common_attr__: CommonAttributes {
                    name: exposure_name.to_string(),
                    package_name: package_name.to_string(),
                    path: DbtPath::from(&mpe.relative_path),
                    name_span: dbt_common::Span::from_serde_span(
                        mpe.name_span.clone(),
                        mpe.relative_path.clone(),
                    ),
                    original_file_path: DbtPath::from(&mpe.relative_path),
                    unique_id: unique_id.clone(),
                    fqn,
                    // dbt-core: description is always default ''
                    description: Some(exposure.description.unwrap_or_default()),
                    patch_path: None,
                    checksum: Default::default(),
                    language: None,
                    raw_code: None,
                    tags: exposure_properties_config
                        .tags
                        .inner()
                        .clone()
                        .map(Into::into)
                        .unwrap_or_default(),
                    classifiers: Default::default(),
                    meta: exposure_properties_config.meta.clone().unwrap_or_default(),
                },
                __base_attr__: NodeBaseAttributes {
                    adapter: adapter_type,
                    // This node type has no `+propagate` config; nothing is published.
                    propagate: Vec::new(),
                    database: "".to_string(),
                    schema: "".to_string(),
                    alias: "".to_string(),
                    relation_name: None,
                    quoting: Default::default(),
                    materialized: Default::default(),
                    static_analysis: Default::default(),
                    static_analysis_off_reason: None,
                    compute: None,
                    enabled: true,
                    extended_model: false,
                    persist_docs: None,
                    columns: vec![],
                    refs,
                    sources,
                    functions: vec![],
                    metrics,
                    depends_on: NodeDependsOn::default(),
                    quoting_ignore_case: false,
                    unrendered_config: Default::default(),
                },
                __exposure_attr__: DbtExposureAttr {
                    owner: exposure.owner,
                    label: exposure.label.clone(),
                    maturity: normalize_maturity(exposure.maturity.as_deref()),
                    type_: exposure.type_.clone(),
                    url: exposure.url,
                    unrendered_config,
                    created_at: chrono::Utc::now().timestamp() as f64,
                },
                deprecated_config: exposure_properties_config.into(),
            };

            // Check if exposure is enabled, add to appropriate collection
            if is_enabled {
                exposures.insert(unique_id, Arc::new(dbt_exposure));
            } else {
                disabled_exposures.insert(unique_id, Arc::new(dbt_exposure));
            }
        }
    }

    Ok((exposures, disabled_exposures))
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
pub fn resolve_yaml_depends_on(
    depends_on: &[Spanned<String>],
    env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    exposure_config: &ResolvedExposureConfig,
    database: &str,
    schema: &str,
    adapter_type: AdapterType,
    package_name: &str,
    root_project_name: &str,
    fqn: Vec<String>,
    relative_path: &str,
    global_static_analysis: Option<StaticAnalysisKind>,
) -> FsResult<(Vec<DbtRef>, Vec<DbtSourceWrapper>, Vec<Vec<String>>)> {
    let exposure_config: ExposureConfig = exposure_config.clone().into();
    let mut dependent_refs = vec![];
    let mut dependent_sources = vec![];
    let mut dependent_metrics = vec![];

    let expanded: Vec<Spanned<String>> =
        depends_on.iter().flat_map(split_depends_on_item).collect();

    for dependency in &expanded {
        let sql_resources: Arc<Mutex<Vec<SqlResource<ExposureConfig>>>> =
            Arc::new(Mutex::new(Vec::new()));

        let mut resolve_model_context = base_ctx.clone();
        resolve_model_context.extend(build_resolve_model_context(
            &exposure_config,
            false,
            adapter_type,
            database,
            schema,
            &fqn.join("."),
            fqn.clone(),
            package_name,
            root_project_name,
            DEFAULT_DBT_QUOTING,
            Arc::new(DbtRuntimeConfig::default()),
            Arc::new(DbtRuntimeConfig::default()),
            sql_resources.clone(),
            Arc::new(AtomicBool::new(false)),
            &PathBuf::from(relative_path),
            &PathBuf::new(),
            global_static_analysis,
        ));

        let sql_resource = render_extract_ref_or_source_expr(
            env,
            &resolve_model_context,
            sql_resources.clone(),
            dependency,
        )?;

        let dep_location = if dependency.has_valid_span() {
            Some(CodeLocationWithFile::new(
                dependency.span().start.line as u32,
                dependency.span().start.column as u32,
                dependency.span().start.index as u32,
                relative_path.to_string(),
            ))
        } else {
            None
        };

        match sql_resource {
            SqlResource::Ref(ref_info) => {
                dependent_refs.push(DbtRef {
                    name: ref_info.0,
                    package: ref_info.1,
                    version: ref_info.2,
                    location: dep_location,
                });
            }
            SqlResource::Source(source_info) => {
                dependent_sources.push(DbtSourceWrapper {
                    source: vec![source_info.0, source_info.1],
                    location: dep_location,
                });
            }
            SqlResource::Metric(metric_info) => {
                dependent_metrics.push(vec![metric_info.0]);
            }
            _ => {
                return err!(
                    ErrorCode::Unexpected,
                    "Invalid dependency input: {}",
                    dependency.as_str()
                );
            }
        }
    }

    Ok((dependent_refs, dependent_sources, dependent_metrics))
}

/// Split a single `depends_on` list item on top-level commas, trimming
/// whitespace from each part and dropping empty parts (trailing comma).
///
/// dbt-core is lenient about comma-separated ref()/source() calls within one
/// YAML list item and about trailing commas.  Examples:
///   `ref('a'), ref('b')`  →  [`ref('a')`, `ref('b')`]
///   `ref('a'),`           →  [`ref('a')`]
///   `ref('a')`            →  [`ref('a')`]   (unchanged)
///
/// The split respects parenthesis depth so that commas inside argument lists
/// (e.g. `ref('model', version=2)`) are not treated as separators.
fn split_depends_on_item(dep: &Spanned<String>) -> Vec<Spanned<String>> {
    let s = dep.as_str();
    let mut parts: Vec<Spanned<String>> = Vec::new();
    let mut depth: usize = 0;
    let mut start = 0usize;

    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let part = s[start..i].trim();
                if !part.is_empty() {
                    parts.push(dep.clone().map(|_| part.to_string()));
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        parts.push(dep.clone().map(|_| tail.to_string()));
    }
    if parts.is_empty() {
        parts.push(dep.clone());
    }
    parts
}

/// Keep exposure maturity within the values accepted by the manifest schema.
fn normalize_maturity(maturity: Option<&str>) -> Option<String> {
    match maturity? {
        "low" => Some("low".to_string()),
        "medium" => Some("medium".to_string()),
        "high" => Some("high".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spanned(s: &str) -> Spanned<String> {
        Spanned::from(s.to_string())
    }

    fn split(s: &str) -> Vec<String> {
        split_depends_on_item(&spanned(s))
            .into_iter()
            .map(|sp| sp.into_inner())
            .collect()
    }

    #[test]
    fn test_split_depends_on_no_comma() {
        assert_eq!(split("ref('model_a')"), vec!["ref('model_a')"]);
    }

    #[test]
    fn test_split_depends_on_trailing_comma() {
        assert_eq!(split("ref('model_a'),"), vec!["ref('model_a')"]);
    }

    #[test]
    fn test_split_depends_on_two_inline() {
        assert_eq!(
            split("ref('model_a'), ref('model_b')"),
            vec!["ref('model_a')", "ref('model_b')"]
        );
    }

    #[test]
    fn test_split_depends_on_preserves_inner_commas() {
        assert_eq!(
            split("ref('model_a', version=2)"),
            vec!["ref('model_a', version=2)"]
        );
    }

    #[test]
    fn test_split_depends_on_multiple_with_inner_commas() {
        assert_eq!(
            split("ref('a', version=2), ref('b')"),
            vec!["ref('a', version=2)", "ref('b')"]
        );
    }

    #[test]
    fn test_normalize_maturity_valid_values_preserved() {
        assert_eq!(normalize_maturity(Some("low")), Some("low".to_string()));
        assert_eq!(
            normalize_maturity(Some("medium")),
            Some("medium".to_string())
        );
        assert_eq!(normalize_maturity(Some("high")), Some("high".to_string()));
    }

    #[test]
    fn test_normalize_maturity_invalid_value_dropped_to_none() {
        // e.g. `maturity: production`, which the manifest schema rejects.
        assert_eq!(normalize_maturity(Some("production")), None);
        assert_eq!(normalize_maturity(Some("")), None);
        assert_eq!(normalize_maturity(Some("   ")), None);
    }

    #[test]
    fn test_normalize_maturity_none_stays_none() {
        assert_eq!(normalize_maturity(None), None);
    }

    #[test]
    fn test_normalize_maturity_requires_exact_schema_value() {
        assert_eq!(normalize_maturity(Some("HIGH")), None);
        assert_eq!(normalize_maturity(Some("  medium  ")), None);
    }

    #[test]
    fn test_split_depends_on_whitespace_trimmed() {
        assert_eq!(
            split("  ref('a')  ,  ref('b')  ,  "),
            vec!["ref('a')", "ref('b')"]
        );
    }
}
