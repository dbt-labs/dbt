use crate::args::ResolveArgs;
use crate::dbt_project_config::{
    ProjectConfigResolver, RootProjectConfigs, disallow_plus_prefix_from_flags, init_project_config,
};
use crate::resolve::resolve_utils::build_unrendered_config;
use crate::utils::{
    extract_resource_config_from_raw_project, get_node_fqn, get_original_file_path, get_unique_id,
};
use dbt_adapter_core::AdapterType;

use dbt_common::io_args::{StaticAnalysisKind, StaticAnalysisOffReason};
use dbt_common::path::DbtPath;
use dbt_common::tracing::dbt_emit::emit_error_log_message;
use dbt_common::{ErrorCode, FsResult};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::schemas::common::{DbtChecksum, Dimension, DimensionTypeParams, NodeDependsOn};
use dbt_schemas::schemas::dbt_column::{
    ColumnPropertiesDimension, ColumnPropertiesDimensionConfig, Entity, EntityConfig,
};
use dbt_schemas::schemas::manifest::metric::{MeasureAggregationParameters, NonAdditiveDimension};
use dbt_schemas::schemas::manifest::semantic_model::{
    DbtSemanticModel, DbtSemanticModelAttr, NodeRelation, SemanticEntity, SemanticMeasure,
    SemanticModelDefaults,
};
use dbt_schemas::schemas::project::SemanticModelConfig;
use dbt_schemas::schemas::project::Tags;
use dbt_schemas::schemas::properties::ModelProperties;
use dbt_schemas::schemas::properties::metrics_properties::{AggregationType, PercentileType};
use dbt_schemas::schemas::ref_and_source::DbtRef;
use dbt_schemas::schemas::semantic_layer::semantic_manifest::SemanticLayerElementConfig;
use dbt_schemas::schemas::{
    CommonAttributes, DbtModel, InternalDbtNodeAttributes, NodeBaseAttributes,
};
use dbt_schemas::state::DbtPackage;
use minijinja::value::Value as MinijinjaValue;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use super::resolve_properties::MinimalPropertiesEntry;

/// Convert `ModelPropertiesSemanticModelConfig` to `SemanticModelConfig` for use with
/// `resolve_with_properties`. The original type stores `meta` nested inside `config` and
/// `enabled` as a plain `bool`, so a manual conversion is needed.
fn semantic_model_properties_config(model_props: &ModelProperties) -> Option<SemanticModelConfig> {
    model_props
        .semantic_model
        .as_ref()
        .map(|smc| SemanticModelConfig {
            enabled: Some(smc.enabled),
            group: smc.group.clone(),
            meta: smc.config.as_ref().and_then(|c| c.meta.clone()),
            tags: Tags::default(),
        })
}

#[allow(clippy::too_many_arguments, clippy::expect_fun_call)]
pub async fn resolve_semantic_models(
    adapter_type: AdapterType,
    args: &ResolveArgs,
    package: &DbtPackage,
    root_package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    minimal_model_properties: &BTreeMap<String, MinimalPropertiesEntry>,
    typed_models_properties: &BTreeMap<String, ModelProperties>,
    resolved_models: &BTreeMap<String, Arc<DbtModel>>,
    package_name: &str,
    env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
) -> FsResult<(
    HashMap<String, Arc<DbtSemanticModel>>,
    HashMap<String, Arc<DbtSemanticModel>>,
)> {
    let mut semantic_models: HashMap<String, Arc<DbtSemanticModel>> = HashMap::new();
    let mut disabled_semantic_models: HashMap<String, Arc<DbtSemanticModel>> = HashMap::new();

    if minimal_model_properties.is_empty() {
        return Ok((semantic_models, disabled_semantic_models));
    }

    let dependency_package_name = dependency_package_name_from_ctx(env, base_ctx);
    let is_dependency = dependency_package_name.is_some();
    let raw_local_project_config = extract_resource_config_from_raw_project(
        &package.raw_project_yml,
        "semantic-models",
        adapter_type,
    )?;
    let raw_root_project_cfg = if is_dependency {
        Some(extract_resource_config_from_raw_project(
            &root_package.raw_project_yml,
            "semantic-models",
            adapter_type,
        )?)
    } else {
        None
    };

    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.semantic_models.clone(),
        is_dependency,
        || {
            init_project_config(
                &package.dbt_project.semantic_models,
                (),
                dependency_package_name,
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref()),
                adapter_type,
            )
        },
        adapter_type,
    )?;

    for (model_name, model_props) in typed_models_properties.iter() {
        // TODO: Do we need to validate semantic_model like how we validate
        // exposure names to only contain letters, numbers, and underscores?

        if model_props.semantic_model.is_none() {
            continue;
        }
        if !model_props.semantic_model.clone().unwrap().enabled {
            continue;
        }

        let mpe = minimal_model_properties
            .get(model_name)
            .expect("ModelPropertiesEntry guaranteed to exist for model");

        // For versioned models, `typed_models_properties` contains one entry per
        // version plus a canonical entry keyed by `mpe.name` pointing at the
        // latest version. Process only the canonical entry to avoid creating
        // duplicate semantic models per version.
        if mpe.version_info.is_some() && model_name != &mpe.name {
            continue;
        }

        // TODO: These are reused from resolve_models, can probably refactor to implement methods in MinimalPropertiesEntry
        let model_maybe_version = mpe.version_info.as_ref().map(|v| v.version.clone());
        let model_unique_id = get_unique_id(&mpe.name, package_name, model_maybe_version, "model");

        // If a semantic_model is declared in YAML but the underlying model SQL
        // is missing (or the unique_id was built incorrectly), skip with an
        // error rather than panicking.
        let Some(resolved_model) = resolved_models.get(&model_unique_id) else {
            emit_error_log_message(
                ErrorCode::NodeNotFoundOrDisabled,
                format!(
                    "Cannot find resolved model '{model_unique_id}' referenced by semantic_model in package '{package_name}'"
                ),
            );
            continue;
        };

        // TODO: semantic_model_name may not always be equal to model_name in the future
        // TODO: if the underlying model has versions, which version is the semantic_model tied to?
        let semantic_model_name = model_props
            .semantic_model
            .clone()
            .unwrap()
            .name
            .unwrap_or_else(|| mpe.name.clone());
        let semantic_model_unique_id =
            get_unique_id(&semantic_model_name, package_name, None, "semantic_model");
        let semantic_model_fqn = get_node_fqn(
            package_name,
            mpe.relative_path.clone(),
            vec![semantic_model_name.to_owned()],
            &package.dbt_project.all_source_paths(),
        );

        // Get combined config from project config and semantic_model config
        let properties_config = semantic_model_properties_config(model_props);
        let mut semantic_model_config = config_resolver
            .resolve_with_properties(&semantic_model_fqn, properties_config.as_ref());
        // Same round-trip canonicalization as `canonicalize_semantic_entity`
        if semantic_model_config
            .meta
            .as_ref()
            .is_some_and(|m| m.is_empty())
        {
            semantic_model_config.meta = None;
        }
        let is_enabled = semantic_model_config.enabled;

        let measures: Vec<SemanticMeasure> = model_props
            .metrics
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|metric| {
                SemanticMeasure {
                    name: metric.name.clone(),
                    expr: metric.expr.clone().map(String::from),
                    description: metric.description.clone(),
                    label: metric.label.clone(),
                    config: metric.config.as_ref().map(|c| SemanticLayerElementConfig {
                        meta: c.meta.clone(),
                    }),
                    agg: metric.agg.clone().unwrap_or(AggregationType::Sum), // FIXME: if metric.agg is optional what should it default to?
                    create_metric: Some(true), // TODO: confirm this: "old spec can declare measure without creating metric, but since this is a metric, always true"
                    agg_params: if metric.percentile.is_some() {
                        Some(MeasureAggregationParameters {
                            percentile: metric.percentile,
                            use_discrete_percentile: metric
                                .percentile_type
                                .as_ref()
                                .map(|pt| matches!(pt, PercentileType::Discrete)),
                            // TODO: confirm approximate == continuous percentile
                            use_approximate_percentile: metric
                                .percentile_type
                                .as_ref()
                                .map(|pt| matches!(pt, PercentileType::Continuous)),
                        })
                    } else {
                        None
                    },
                    non_additive_dimension: metric
                        .non_additive_dimension
                        .clone()
                        .map(NonAdditiveDimension::from),
                    agg_time_dimension: Some(
                        metric
                            .agg_time_dimension
                            .clone()
                            .or_else(|| model_props.agg_time_dimension.clone())
                            .unwrap_or_default(),
                    ),
                }
            })
            .collect();

        let dimensions = model_props_to_dimensions(model_props.clone());
        let node_relation = NodeRelation {
            database: Some(resolved_model.database()),
            schema_name: resolved_model.schema(),
            alias: resolved_model.alias(),
            relation_name: resolved_model.__base_attr__.relation_name.clone(),
        };

        // `enabled` and `group` sit directly on `semantic_model:`, while `meta`
        // (and other config fields) are nested under `semantic_model.config:`.
        let raw_properties_yml_config = {
            let sm = mpe.schema_value.get("semantic_model");
            let mut config: BTreeMap<String, dbt_yaml::Value> = BTreeMap::new();
            for field in ["enabled", "group"] {
                if let Some(v) = sm.and_then(|s| s.get(field)) {
                    config.insert(field.to_string(), v.clone());
                }
            }
            if let Some(mapping) = sm
                .and_then(|s| s.get("config"))
                .and_then(|v| v.as_mapping())
            {
                for (k, v) in mapping.iter() {
                    if let Some(k) = k.as_str() {
                        config.insert(k.to_string(), v.clone());
                    }
                }
            }
            if config.is_empty() {
                None
            } else {
                Some(config)
            }
        };

        let unrendered_config = build_unrendered_config(
            &semantic_model_fqn,
            &raw_local_project_config,
            raw_root_project_cfg.as_ref(),
            raw_properties_yml_config.as_ref(),
            None,
            false,
            adapter_type,
        )?;

        let dbt_semantic_model = DbtSemanticModel {
            __common_attr__: CommonAttributes {
                name: semantic_model_name.clone(),
                package_name: package_name.to_string(),
                path: DbtPath::from(&mpe.relative_path),
                original_file_path: get_original_file_path(
                    &package.package_root_path,
                    &args.io.in_dir,
                    &mpe.relative_path,
                ),
                name_span: dbt_common::Span::from_serde_span(
                    mpe.name_span.clone(),
                    mpe.relative_path.clone(),
                ),
                patch_path: Some(DbtPath::from(&mpe.relative_path)),
                unique_id: semantic_model_unique_id.clone(),
                fqn: semantic_model_fqn.clone(),
                description: Some(model_props.description.clone().unwrap_or_default()),
                checksum: DbtChecksum::default(),
                raw_code: None,
                language: None,
                tags: semantic_model_config
                    .tags
                    .inner()
                    .clone()
                    .map(Into::into)
                    .unwrap_or_default(),
                classifiers: Default::default(),
                meta: semantic_model_config.meta.clone().unwrap_or_default(),
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
                refs: vec![DbtRef {
                    // only name is hydrated for parity with Mantle
                    name: model_name.clone(),
                    package: None,
                    version: None,
                    location: None,
                }],
                sources: vec![],
                functions: vec![],
                metrics: vec![],
                depends_on: NodeDependsOn {
                    nodes: vec![model_unique_id.clone()],
                    macros: vec![],
                    nodes_with_ref_location: vec![],
                },
                quoting_ignore_case: false,
                unrendered_config: Default::default(),
            },
            __semantic_model_attr__: DbtSemanticModelAttr {
                unrendered_config,
                group: semantic_model_config.group.clone(),
                created_at: chrono::Utc::now().timestamp() as f64,
                metadata: None, // deprioritized feature. always null for now.
                label: None,    // no semantic model level label (could maybe inherit from model?)
                model: format!("ref('{model_name}')"),
                node_relation: Some(node_relation),
                defaults: Some(SemanticModelDefaults {
                    agg_time_dimension: model_props.agg_time_dimension.clone(),
                }),
                entities: model_props_to_semantic_entities(model_props.clone()),
                measures,
                dimensions,
                primary_entity: model_props.primary_entity.clone(),
            },
            deprecated_config: semantic_model_config.into(),
            __other__: BTreeMap::new(),
        };

        // Check if semantic_model is enabled (following exposures pattern)
        if is_enabled {
            semantic_models.insert(semantic_model_unique_id, Arc::new(dbt_semantic_model));
        } else {
            disabled_semantic_models.insert(semantic_model_unique_id, Arc::new(dbt_semantic_model));
        }
    }

    Ok((semantic_models, disabled_semantic_models))
}

/// Canonicalize authored-empty entity fields to `None`.
fn canonicalize_semantic_entity(mut entity: SemanticEntity) -> SemanticEntity {
    entity.description = entity.description.filter(|d| !d.is_empty());
    entity.config = entity
        .config
        .filter(|c| *c != SemanticLayerElementConfig::default());
    entity
}

pub fn model_props_to_semantic_entities(model_props: ModelProperties) -> Vec<SemanticEntity> {
    let mut entities: Vec<SemanticEntity> = vec![];

    for column in model_props.columns.unwrap_or_default() {
        if let Some(column_entity) = column.entity {
            let mut column_entity_config = match column_entity {
                Entity::EntityType(ref entity_type) => EntityConfig {
                    name: Some(column.name.clone()), // defaults to column.name if there is no column.entity.name
                    type_: entity_type.clone(),
                    description: column.description.clone(), // defaults to column.description if there is no column.entity.description
                    label: None,
                    config: None,
                },
                Entity::EntityConfig(ref config) => config.clone(),
            };
            if column_entity_config.name.is_none() {
                column_entity_config.name = Some(column.name.clone());
            }
            if column_entity_config.description.is_none() {
                column_entity_config.description = column.description.clone();
            }

            let semantic_entity = SemanticEntity {
                name: column_entity_config.name.unwrap_or_default(),
                entity_type: column_entity_config.type_,
                description: column_entity_config.description,
                expr: Some(column.name.clone()),
                label: column_entity_config.label,
                config: column_entity_config.config,
                // fields below are always null (for now)
                role: None,
                metadata: None,
            };
            entities.push(canonicalize_semantic_entity(semantic_entity));
        }
    }

    let derived_entities = model_props
        .derived_semantics
        .unwrap_or_default()
        .entities
        .unwrap_or_default();
    for derived_entity in derived_entities {
        let semantic_entity = SemanticEntity {
            name: derived_entity.name.clone(),
            expr: Some(derived_entity.expr.clone()),
            entity_type: derived_entity.type_.clone(),
            description: derived_entity.description.clone(),
            label: derived_entity.label.clone(),
            config: derived_entity.config.clone(),
            // fields below are always null (for now)
            role: None,
            metadata: None,
        };
        entities.push(canonicalize_semantic_entity(semantic_entity));
    }

    entities
}

pub fn model_props_to_dimensions(model_props: ModelProperties) -> Vec<Dimension> {
    let mut dimensions: Vec<Dimension> = vec![];

    for column in model_props.columns.unwrap_or_default() {
        if let Some(column_dimension) = column.dimension {
            let mut column_dimension_config = match column_dimension {
                ColumnPropertiesDimension::DimensionType(ref dimension_type) => {
                    ColumnPropertiesDimensionConfig {
                        type_: dimension_type.clone(),
                        is_partition: Some(false),
                        name: Some(column.name.clone()), // defaults to column.name if there is no column.dimension.name
                        description: column.description.clone(), // defaults to column.description if there is no column.dimension.description
                        label: None,
                        config: None,
                        validity_params: None,
                    }
                }
                ColumnPropertiesDimension::DimensionConfig(ref config) => config.clone(),
            };
            if column_dimension_config.name.is_none() {
                column_dimension_config.name = Some(column.name.clone());
            }
            if column_dimension_config.description.is_none() {
                column_dimension_config.description = column.description.clone();
            }

            let mut type_params = None;
            if column.granularity.is_some() || column_dimension_config.validity_params.is_some() {
                type_params = Some(DimensionTypeParams {
                    time_granularity: column.granularity,
                    validity_params: column_dimension_config.validity_params,
                });
            }

            let dimension = Dimension {
                name: column_dimension_config.name.unwrap_or_default(),
                dimension_type: column_dimension_config.type_.clone(),
                description: column_dimension_config.description,
                expr: Some(column.name.clone()),
                label: column_dimension_config.label,
                is_partition: column_dimension_config.is_partition.unwrap_or(false),
                type_params,
                config: column_dimension_config.config.clone(),
                // fields below are always null (for now)
                metadata: None,
            };
            dimensions.push(dimension);
        }
    }

    let derived_dimensions = model_props
        .derived_semantics
        .unwrap_or_default()
        .dimensions
        .unwrap_or_default();
    for derived_dimension in derived_dimensions {
        let mut type_params = None;
        if derived_dimension.granularity.is_some() || derived_dimension.validity_params.is_some() {
            type_params = Some(DimensionTypeParams {
                time_granularity: derived_dimension.granularity,
                validity_params: derived_dimension.validity_params,
            });
        }

        let dimension = Dimension {
            name: derived_dimension.name.clone(),
            expr: Some(derived_dimension.expr.clone()),
            dimension_type: derived_dimension.type_.clone(),
            is_partition: derived_dimension.is_partition.unwrap_or(false),
            description: derived_dimension.description.clone(),
            type_params,
            label: derived_dimension.label.clone(),
            config: derived_dimension.config.clone(),
            // fields below are always null (for now)
            metadata: None,
        };
        dimensions.push(dimension);
    }

    dimensions
}
