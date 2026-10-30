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
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_error_log_message};
use dbt_common::{ErrorCode, FsResult};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::schemas::common::{DbtChecksum, NodeDependsOn};
use dbt_schemas::schemas::project::MetricConfig;
use dbt_schemas::schemas::properties::metrics_properties::{MetricType, PercentileType};
use dbt_schemas::schemas::properties::{MetricsProperties, ModelProperties};
use dbt_schemas::schemas::{CommonAttributes, NodeBaseAttributes};
use dbt_schemas::state::DbtPackage;
use minijinja::value::Value as MinijinjaValue;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use super::resolve_properties::MinimalPropertiesEntry;
use super::validate_metrics::validate_metric;

use dbt_schemas::schemas::manifest::metric::{
    DbtMetric, DbtMetricAttr, MeasureAggregationParameters, MetricAggregationParameters,
    MetricTypeParams, NonAdditiveDimension,
};
use minijinja::constants::CURRENT_PATH;

type ResolveMetricsResult = FsResult<(
    HashMap<String, Arc<DbtMetric>>,
    HashMap<String, Arc<DbtMetric>>,
)>;

#[allow(clippy::too_many_arguments)]
pub async fn resolve_metrics(
    adapter_type: AdapterType,
    arg: &ResolveArgs,
    package: &DbtPackage,
    root_package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    minimal_model_properties: &BTreeMap<String, MinimalPropertiesEntry>,
    minimal_metric_properties: &BTreeMap<String, MinimalPropertiesEntry>,
    typed_models_properties: &BTreeMap<String, ModelProperties>,
    package_name: &str,
    env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
) -> ResolveMetricsResult {
    let mut metrics: HashMap<String, Arc<DbtMetric>> = HashMap::new();
    let mut disabled_metrics: HashMap<String, Arc<DbtMetric>> = HashMap::new();
    let mut seen_metric_names = HashSet::new();

    let dependency_package_name = dependency_package_name_from_ctx(env, base_ctx);
    let raw_local_project_config = extract_resource_config_from_raw_project(
        &package.raw_project_yml,
        "metrics",
        adapter_type,
    )?;
    let raw_root_project_cfg = if dependency_package_name.is_some() {
        Some(extract_resource_config_from_raw_project(
            &root_package.raw_project_yml,
            "metrics",
            adapter_type,
        )?)
    } else {
        None
    };
    let disallow_plus_prefix =
        disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref());

    let (nested_metrics, nested_disabled_metrics) = resolve_nested_model_metrics(
        adapter_type,
        arg,
        package,
        root_project_configs,
        minimal_model_properties,
        typed_models_properties,
        package_name,
        env,
        base_ctx,
        &mut seen_metric_names,
        &raw_local_project_config,
        raw_root_project_cfg.as_ref(),
        disallow_plus_prefix,
    )?;
    metrics.extend(nested_metrics);
    disabled_metrics.extend(nested_disabled_metrics);

    let (top_level_metrics, top_level_disabled_metrics) = resolve_top_level_metrics(
        adapter_type,
        arg,
        package,
        root_project_configs,
        minimal_metric_properties,
        package_name,
        env,
        base_ctx,
        &mut seen_metric_names,
        &raw_local_project_config,
        raw_root_project_cfg.as_ref(),
        disallow_plus_prefix,
    )?;
    metrics.extend(top_level_metrics);
    disabled_metrics.extend(top_level_disabled_metrics);

    Ok((metrics, disabled_metrics))
}

#[allow(clippy::too_many_arguments)]
pub fn resolve_nested_model_metrics(
    adapter_type: AdapterType,
    arg: &ResolveArgs,
    package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    minimal_model_properties: &BTreeMap<String, MinimalPropertiesEntry>,
    typed_models_properties: &BTreeMap<String, ModelProperties>,
    package_name: &str,
    env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    seen_metric_names: &mut HashSet<String>,
    raw_local_project_config: &crate::utils::RawProjectConfig,
    raw_root_project_cfg: Option<&crate::utils::RawProjectConfig>,
    disallow_plus_prefix: bool,
) -> ResolveMetricsResult {
    let mut metrics = HashMap::new();
    let mut disabled_metrics = HashMap::new();

    if minimal_model_properties.is_empty() {
        return Ok((metrics, disabled_metrics));
    }

    // TODO: what is the difference between 'package_name' and 'dependency_package_name'?
    let dependency_package_name = dependency_package_name_from_ctx(env, base_ctx);
    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.metrics.clone(),
        dependency_package_name.is_some(),
        || {
            init_project_config(
                &package.dbt_project.metrics,
                (),
                dependency_package_name,
                disallow_plus_prefix,
                adapter_type,
            )
        },
        adapter_type,
    )?;

    for (model_name, model_props) in typed_models_properties.iter() {
        if model_props.metrics.is_none() {
            continue;
        }

        let mpe = minimal_model_properties
            .get(model_name)
            .expect("ModelPropertiesEntry guaranteed to exist for model");

        // For versioned models, `typed_models_properties` contains one entry per
        // version plus a canonical entry keyed by `mpe.name` pointing at the
        // latest version. Process only the canonical entry to avoid emitting
        // duplicate-metric-name errors for a single YAML declaration.
        if mpe.version_info.is_some() && model_name != &mpe.name {
            continue;
        }

        let mut semantic_model_name = model_props.name.clone();
        if let Some(semantic_model) = &model_props.semantic_model
            && let Some(name) = &semantic_model.name
        {
            semantic_model_name = name.clone();
        }
        let semantic_model_unique_id =
            get_unique_id(&semantic_model_name, package_name, None, "semantic_model");

        if let Some(model_metrics_props) = model_props.metrics.clone().as_ref() {
            for metric_props in model_metrics_props {
                let metric_name = metric_props.name.clone();

                // Validate metric (name and window)
                if let Err(e) = validate_metric(metric_props) {
                    emit_error_log_from_fs_error(*e);

                    continue;
                }

                // Check for duplicate metric names
                if !seen_metric_names.insert(metric_name.clone()) {
                    emit_error_log_message(
                        ErrorCode::DbtYamlValidationError,
                        format!(
                            "Duplicate metric name '{}' found in package '{}'",
                            metric_name, package_name
                        ),
                    );
                    continue;
                }

                let metric_unique_id = get_unique_id(&metric_name, package_name, None, "metric");
                let metric_fqn = get_node_fqn(
                    package_name,
                    mpe.relative_path.clone(),
                    vec![metric_name.to_owned()],
                    &package.dbt_project.all_source_paths(),
                );

                // Get combined config from project config and metric config
                let metric_config = config_resolver
                    .resolve_with_properties(&metric_fqn, metric_props.config.as_ref());

                let mut type_params: MetricTypeParams = metric_props.clone().into();
                type_params.metric_aggregation_params =
                    model_metrics_props_to_metric_aggregation_params(
                        &semantic_model_name,
                        model_props,
                        metric_props,
                    );

                // Nested metrics are defined inside model YAML, so we find this
                // metric's entry by name in the model's raw `metrics:` list.
                let raw_properties_yml_config = mpe
                    .schema_value
                    .get("metrics")
                    .and_then(|v| v.as_sequence())
                    .and_then(|seq| {
                        seq.iter().find(|m| {
                            m.get("name")
                                .and_then(|n| n.as_str())
                                .is_some_and(|n| n == metric_name)
                        })
                    })
                    .and_then(extract_config_map);

                let unrendered_config = build_unrendered_config(
                    &metric_fqn,
                    raw_local_project_config,
                    raw_root_project_cfg,
                    raw_properties_yml_config.as_ref(),
                    None,
                    false,
                    adapter_type,
                )?;

                let dbt_metric = DbtMetric {
                    __common_attr__: CommonAttributes {
                        name: metric_name.clone(),
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
                        unique_id: metric_unique_id.clone(),
                        fqn: metric_fqn.clone(),
                        description: metric_props.description.clone(),
                        checksum: DbtChecksum::default(),
                        raw_code: None,
                        language: None,
                        tags: metric_config
                            .tags
                            .inner()
                            .clone()
                            .map(Into::into)
                            .unwrap_or_default(),
                        classifiers: Default::default(),
                        meta: metric_config.meta.clone().unwrap_or_default(),
                    },
                    __base_attr__: NodeBaseAttributes {
                        // Not executed against a warehouse; records the target it parsed under.
                        adapter: adapter_type,
                        // This node type has no `+propagate` config; nothing is published.
                        propagate: Vec::new(),
                        effective_propagation_target: None,
                        database: "".to_string(),
                        schema: "".to_string(),
                        alias: "".to_string(),
                        relation_name: None,
                        quoting: Default::default(),
                        materialized: Default::default(),
                        static_analysis: StaticAnalysisKind::Off.into(),
                        static_analysis_off_reason: Some(
                            StaticAnalysisOffReason::UnableToFetchSchema,
                        ),
                        compute: None,
                        enabled: true,
                        extended_model: false,
                        persist_docs: None,
                        columns: Default::default(),
                        refs: vec![],
                        sources: vec![],
                        functions: vec![],
                        metrics: vec![],
                        depends_on: NodeDependsOn {
                            macros: vec![],
                            nodes: vec![semantic_model_unique_id.clone()],
                            nodes_with_ref_location: vec![],
                        },
                        quoting_ignore_case: false,
                        unrendered_config: Default::default(),
                    },
                    __metric_attr__: DbtMetricAttr {
                        unrendered_config,
                        group: metric_config.group.clone(),
                        created_at: chrono::Utc::now().timestamp() as f64,
                        metadata: None,
                        // fall back to metric_name matches core
                        label: Some(
                            metric_props
                                .label
                                .clone()
                                .unwrap_or_else(|| metric_name.clone()),
                        ),
                        metric_type: metric_props.type_.clone().unwrap_or_default(),
                        type_params,
                        filter: metric_props.filter.0.clone().map(|f| vec![f].into()),
                        time_granularity: metric_props.time_granularity.clone(),
                        metrics: vec![], // always empty, hydrated in type_params.metrics
                    },
                    deprecated_config: MetricConfig {
                        enabled: Some(metric_config.enabled),
                        tags: metric_config.tags.clone(),
                        meta: metric_config.meta.clone(),
                        group: metric_config.group.clone(),
                    },
                    __other__: BTreeMap::new(),
                };

                // Check if metric is enabled (following exposures pattern)
                if metric_config.enabled {
                    metrics.insert(metric_unique_id, Arc::new(dbt_metric));
                } else {
                    disabled_metrics.insert(metric_unique_id, Arc::new(dbt_metric));
                }
            }
        }
    }

    Ok((metrics, disabled_metrics))
}

#[allow(clippy::too_many_arguments)]
pub fn resolve_top_level_metrics(
    adapter_type: AdapterType,
    arg: &ResolveArgs,
    package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    minimal_metric_properties: &BTreeMap<String, MinimalPropertiesEntry>,
    package_name: &str,
    env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    seen_metric_names: &mut HashSet<String>,
    raw_local_project_config: &crate::utils::RawProjectConfig,
    raw_root_project_cfg: Option<&crate::utils::RawProjectConfig>,
    disallow_plus_prefix: bool,
) -> ResolveMetricsResult {
    let mut metrics = HashMap::new();
    let mut disabled_metrics = HashMap::new();

    if minimal_metric_properties.is_empty() {
        return Ok((metrics, disabled_metrics));
    }

    let dependency_package_name = dependency_package_name_from_ctx(env, base_ctx);
    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.metrics.clone(),
        dependency_package_name.is_some(),
        || {
            init_project_config(
                &package.dbt_project.metrics,
                (),
                dependency_package_name,
                disallow_plus_prefix,
                adapter_type,
            )
        },
        adapter_type,
    )?;

    for (metric_name, mpe) in minimal_metric_properties.iter() {
        if mpe.schema_value.is_null() {
            continue;
        }

        let raw_properties_yml_config = extract_config_map(&mpe.schema_value);

        let mut metric_ctx = base_ctx.clone();
        metric_ctx.insert(
            CURRENT_PATH.to_string(),
            MinijinjaValue::from(mpe.relative_path.to_string_lossy().to_string()),
        );

        // Parse the metric properties from YAML
        let metric_props: MetricsProperties = into_typed_with_jinja(
            mpe.schema_value.clone(),
            false,
            env,
            &metric_ctx,
            &[],
            dependency_package_name,
            // Set show_errors_or_warnings to false for legacy top-level metrics to avoid strict validation errors, since these metrics use a different specification format than the current semantic layer spec.
            false,
        )?;

        let metric_fqn = get_node_fqn(
            package_name,
            mpe.relative_path.clone(),
            vec![metric_name.to_owned()],
            &package.dbt_project.all_source_paths(),
        );

        // Get combined config from project config and metric config
        let metric_metric_config =
            config_resolver.resolve_with_properties(&metric_fqn, metric_props.config.as_ref());

        let metric_name = metric_props.name.clone();

        // Validate metric (name and window)
        if let Err(e) = validate_metric(&metric_props) {
            emit_error_log_from_fs_error(*e);

            continue;
        }

        // Check for duplicate metric names
        if !seen_metric_names.insert(metric_name.clone()) {
            emit_error_log_message(
                ErrorCode::DbtYamlValidationError,
                format!(
                    "Duplicate metric name '{}' found in package '{}'",
                    metric_name, package_name
                ),
            );
            continue;
        }

        let metric_unique_id = get_unique_id(&metric_name, package_name, None, "metric");
        let metric_fqn = get_node_fqn(
            package_name,
            mpe.relative_path.clone(),
            vec![metric_name.to_owned()],
            &package.dbt_project.all_source_paths(),
        );

        let type_params: MetricTypeParams = metric_props.clone().into();
        let metric_type: MetricType = metric_props.type_.clone().unwrap_or_default();

        // in contrast to model nested metrics depending on the underlying semantic model
        // top-level metrics depend on other metrics from other semantic models
        let names_of_nodes_depends_on: Vec<String> = match &metric_type {
            MetricType::Ratio => {
                let maybe_numerator = type_params.numerator.clone();
                let maybe_denominator = type_params.denominator.clone();
                if let Some(numerator) = maybe_numerator
                    && let Some(denominator) = maybe_denominator
                {
                    vec![numerator.name, denominator.name]
                } else {
                    vec![]
                }
            }
            MetricType::Derived => {
                let mut metric_names: Vec<String> = type_params
                    .metrics
                    .clone()
                    .unwrap_or_default()
                    .iter()
                    .map(|metric| metric.name.clone())
                    .collect();
                metric_names.dedup();
                metric_names
            }
            MetricType::Cumulative => {
                let maybe_cumulative_metric = type_params
                    .cumulative_type_params
                    .clone()
                    .unwrap_or_default()
                    .metric;

                if let Some(metric) = maybe_cumulative_metric {
                    vec![metric.name]
                } else {
                    vec![]
                }
            }
            MetricType::Conversion => {
                if type_params.conversion_type_params.is_none() {
                    vec![]
                } else {
                    let conversion_type_params = type_params
                        .conversion_type_params
                        .clone()
                        .expect("conversion_type_params must exist for conversion metric");
                    let base_metric_name =
                        conversion_type_params.base_metric.unwrap_or_default().name;
                    let conversion_metric_name = conversion_type_params
                        .conversion_metric
                        .unwrap_or_default()
                        .name;

                    if base_metric_name.is_empty() || conversion_metric_name.is_empty() {
                        vec![]
                    } else {
                        vec![base_metric_name, conversion_metric_name]
                    }
                }
            }
            _ => vec![],
        };

        let unique_ids_of_nodes_depends_on: Vec<String> = names_of_nodes_depends_on
            .iter()
            .map(|name| get_unique_id(name, package_name, None, "metric"))
            .collect();

        let depends_on = NodeDependsOn {
            macros: vec![],
            nodes: unique_ids_of_nodes_depends_on,
            nodes_with_ref_location: vec![],
        };

        let unrendered_config = build_unrendered_config(
            &metric_fqn,
            raw_local_project_config,
            raw_root_project_cfg,
            raw_properties_yml_config.as_ref(),
            None,
            false,
            adapter_type,
        )?;

        let dbt_metric = DbtMetric {
            __common_attr__: CommonAttributes {
                name: metric_name.clone(),
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
                unique_id: metric_unique_id.clone(),
                fqn: metric_fqn.clone(),
                description: metric_props.description.clone(),
                checksum: DbtChecksum::default(),
                raw_code: None,
                language: None,
                tags: metric_metric_config
                    .tags
                    .inner()
                    .clone()
                    .map(Into::into)
                    .unwrap_or_default(),
                classifiers: Default::default(),
                meta: metric_metric_config.meta.clone().unwrap_or_default(),
            },
            __base_attr__: NodeBaseAttributes {
                // Not executed against a warehouse; records the target it parsed under.
                adapter: adapter_type,
                // This node type has no `+propagate` config; nothing is published.
                propagate: Vec::new(),
                effective_propagation_target: None,
                database: "".to_string(),
                schema: "".to_string(),
                alias: "".to_string(),
                relation_name: None,
                quoting: Default::default(),
                materialized: Default::default(),
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
            __metric_attr__: DbtMetricAttr {
                unrendered_config,
                group: metric_metric_config.group.clone(),
                created_at: chrono::Utc::now().timestamp() as f64,
                metadata: None,
                // fall back to metric_name matches core
                label: Some(
                    metric_props
                        .label
                        .clone()
                        .unwrap_or_else(|| metric_name.clone()),
                ),
                metric_type,
                type_params,
                filter: metric_props.filter.0.clone().map(|f| vec![f].into()),
                time_granularity: metric_props.time_granularity.clone(),
                metrics: vec![],
            },
            deprecated_config: MetricConfig {
                enabled: Some(metric_metric_config.enabled),
                tags: metric_metric_config.tags.clone(),
                meta: metric_metric_config.meta.clone(),
                group: metric_metric_config.group.clone(),
            },
            __other__: BTreeMap::new(),
        };

        // Check if metric is enabled (following exposures pattern)
        if metric_metric_config.enabled {
            metrics.insert(metric_unique_id, Arc::new(dbt_metric));
        } else {
            disabled_metrics.insert(metric_unique_id, Arc::new(dbt_metric));
        }
    }

    Ok((metrics, disabled_metrics))
}

pub fn model_metrics_props_to_metric_aggregation_params(
    semantic_model: &str,
    model: &ModelProperties,
    metric: &MetricsProperties,
) -> Option<MetricAggregationParameters> {
    let agg_params = if let Some(percentile) = metric.percentile {
        let use_discrete_percentile =
            matches!(metric.percentile_type, Some(PercentileType::Discrete));
        let use_approximate_percentile =
            matches!(metric.percentile_type, Some(PercentileType::Continuous));
        Some(MeasureAggregationParameters {
            percentile: Some(percentile),
            use_discrete_percentile: Some(use_discrete_percentile),
            use_approximate_percentile: Some(use_approximate_percentile),
        })
    } else {
        Some(MeasureAggregationParameters {
            percentile: None,
            use_discrete_percentile: Some(false),
            use_approximate_percentile: Some(false),
        })
    };

    let mut agg_time_dimension = metric.agg_time_dimension.clone();
    if agg_time_dimension.is_none() {
        agg_time_dimension = model.agg_time_dimension.clone();
    }

    metric.agg.clone().map(|agg| MetricAggregationParameters {
        semantic_model: semantic_model.to_string(),
        agg: Some(agg),
        agg_params,
        agg_time_dimension,
        non_additive_dimension: metric
            .non_additive_dimension
            .clone()
            .map(NonAdditiveDimension::from),
        expr: metric.expr.clone().map(String::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::properties::metrics_properties::{AggregationType, PercentileType};

    fn base_metric(agg: Option<AggregationType>) -> MetricsProperties {
        MetricsProperties {
            name: "test_metric".to_string(),
            agg,
            ..Default::default()
        }
    }

    fn base_model() -> ModelProperties {
        ModelProperties {
            name: "test_model".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_agg_params_defaults_when_no_percentile() {
        let metric = base_metric(Some(AggregationType::Sum));
        let result = model_metrics_props_to_metric_aggregation_params("sm", &base_model(), &metric);
        let agg_params = result.unwrap().agg_params.unwrap();
        assert_eq!(agg_params.percentile, None);
        assert_eq!(agg_params.use_discrete_percentile, Some(false));
        assert_eq!(agg_params.use_approximate_percentile, Some(false));
    }

    #[test]
    fn test_agg_params_with_unspecified_percentile_type() {
        let metric = MetricsProperties {
            percentile: Some(0.95),
            ..base_metric(Some(AggregationType::Percentile))
        };
        let result = model_metrics_props_to_metric_aggregation_params("sm", &base_model(), &metric);
        let agg_params = result.unwrap().agg_params.unwrap();
        assert_eq!(agg_params.percentile, Some(0.95));
        assert_eq!(agg_params.use_discrete_percentile, Some(false));
        assert_eq!(agg_params.use_approximate_percentile, Some(false));
    }

    #[test]
    fn test_agg_params_with_discrete_percentile() {
        let metric = MetricsProperties {
            percentile: Some(0.5),
            percentile_type: Some(PercentileType::Discrete),
            ..base_metric(Some(AggregationType::Percentile))
        };
        let result = model_metrics_props_to_metric_aggregation_params("sm", &base_model(), &metric);
        let agg_params = result.unwrap().agg_params.unwrap();
        assert_eq!(agg_params.use_discrete_percentile, Some(true));
        assert_eq!(agg_params.use_approximate_percentile, Some(false));
    }

    #[test]
    fn test_agg_params_with_continuous_percentile() {
        let metric = MetricsProperties {
            percentile: Some(0.5),
            percentile_type: Some(PercentileType::Continuous),
            ..base_metric(Some(AggregationType::Percentile))
        };
        let result = model_metrics_props_to_metric_aggregation_params("sm", &base_model(), &metric);
        let agg_params = result.unwrap().agg_params.unwrap();
        assert_eq!(agg_params.use_discrete_percentile, Some(false));
        assert_eq!(agg_params.use_approximate_percentile, Some(true));
    }

    #[test]
    fn test_returns_none_when_no_agg() {
        let metric = base_metric(None);
        let result = model_metrics_props_to_metric_aggregation_params("sm", &base_model(), &metric);
        assert!(result.is_none());
    }
}
