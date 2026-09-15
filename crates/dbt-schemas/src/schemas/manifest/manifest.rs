use crate::schemas::project::ResolvableConfig;
use chrono::{DateTime, Utc};
use dbt_adapter_core::AdapterType;
use dbt_common::{Span, path::DbtPath};
use dbt_yaml::{Spanned, UntaggedEnumDeserialize};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
};
// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use crate::schemas::project::DataTestConfig;
use crate::schemas::project::configs::model_config::ModelConfig;
use crate::schemas::project::configs::snapshot_config::SnapshotConfig;
use crate::{
    dbt_utils::get_dbt_schema_version,
    schemas::{
        CommonAttributes, DbtFunction, DbtFunctionAttr, DbtModel, DbtModelAttr, DbtSeed,
        DbtSnapshot, DbtSource, DbtTest, DbtUnitTest, DbtUnitTestAttr, IntrospectionKind,
        NodeBaseAttributes, Nodes, TimeSpine, TimeSpinePrimaryColumn,
        common::{
            Access, DbtChecksum, DbtMaterialization, DbtQuoting, NodeDependsOn,
            conform_normalized_snapshot_raw_code_to_mantle_format, normalize_sql,
        },
        macros::DbtDocsMacro,
        manifest::{
            ManifestExposure, ManifestGroup, ManifestSavedQuery, ManifestUnitTest,
            manifest_nodes::{
                ManifestAnalysis, ManifestCheck, ManifestCommonAttributes, ManifestDataTest,
                ManifestFunction, ManifestMaterializableCommonAttributes, ManifestMetric,
                ManifestModel, ManifestOperation, ManifestSeed, ManifestSemanticModel,
                ManifestSnapshot, ManifestSource,
            },
            saved_query::DbtSavedQueryAttr,
            semantic_model::NodeRelation,
        },
        nodes::{
            AdapterAttr, DbtAnalysis, DbtAnalysisAttr, DbtCheck, DbtCheckAttr, DbtGroup,
            DbtGroupAttr, DbtSeedAttr, DbtSnapshotAttr, DbtSourceAttr, DbtTestAttr,
        },
        relations::default_dbt_quoting_for,
    },
    state::{ManifestPathConfig, Operations, ResolverState},
};

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize)]
#[serde(tag = "resource_type")]
#[serde(rename_all = "snake_case")]
pub enum DbtNode {
    Model(ManifestModel),
    Test(ManifestDataTest),
    Snapshot(ManifestSnapshot),
    Seed(ManifestSeed),
    Operation(ManifestOperation),
    Analysis(ManifestAnalysis),
    Function(ManifestFunction),
    Check(ManifestCheck),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestMetadata {
    // NOTE: this flatten should be removed once we completely decouple DbtManifest from Yaml
    #[serde(flatten)]
    pub __base__: BaseMetadata,
    #[serde(default)]
    pub project_name: String,
    /// The MD5 hash of the project name.
    pub project_id: Option<String>,
    pub user_id: Option<String>,
    pub send_anonymous_usage_stats: Option<bool>,
    #[serde(default)]
    pub adapter_type: String,
    pub quoting: Option<DbtQuoting>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BaseMetadata {
    pub dbt_schema_version: String,
    pub dbt_version: String,
    pub generated_at: DateTime<Utc>,
    pub invocation_id: Option<String>,
    pub invocation_started_at: Option<DateTime<Utc>>,
    pub env: BTreeMap<String, String>,
}

impl PartialEq for ManifestMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.__base__.env == other.__base__.env
            && self.project_name == other.project_name
            && self.send_anonymous_usage_stats == other.send_anonymous_usage_stats
            && self.adapter_type == other.adapter_type
        // Note: We intentionally skip comparing the following right now:
        // - generated_at (timestamp)
        // - invocation_id (changes each run)
        // - user_id (may change between environments)
        // - dbt_schema_version (changes between versions)
        // - dbt_version (changes between versions)
        // - project_id (changes between environments)
    }
}

impl Eq for ManifestMetadata {}

// Re-export the current version (V12) as the default
pub use super::v12::DbtManifestV12;

// Type aliases for backwards compatibility
pub type DbtManifest = DbtManifestV12;

pub fn serialize_with_resource_type(mut value: YmlValue, resource_type: &str) -> YmlValue {
    if let YmlValue::Mapping(ref mut map, _) = value {
        map.insert(
            YmlValue::string("resource_type".to_string()),
            YmlValue::string(resource_type.to_string()),
        );
    }
    value
}

pub fn build_manifest(invocation_id: &str, resolver_state: &ResolverState) -> DbtManifest {
    let (parent_map, child_map) =
        build_parent_and_child_maps(&resolver_state.nodes, &resolver_state.operations);
    let group_map = build_group_map(&resolver_state.nodes);

    let disabled = build_disabled_map(resolver_state);
    DbtManifest {
        metadata: ManifestMetadata {
            __base__: BaseMetadata {
                dbt_schema_version: get_dbt_schema_version("manifest", 12),
                dbt_version: env!("CARGO_PKG_VERSION").to_string(),
                generated_at: Utc::now(),
                invocation_id: Some(invocation_id.to_string()),
                invocation_started_at: Some(resolver_state.run_started_at.with_timezone(&Utc)),
                env: dbt_common::constants::collect_dbt_custom_envs(),
            },
            project_name: resolver_state.root_project_name.clone(),
            adapter_type: resolver_state
                .dbt_profile
                .default_db_config()
                .adapter_type()
                .to_string(),
            project_id: Some(format!(
                "{:x}",
                md5::compute(resolver_state.root_project_name.as_bytes())
            )),
            quoting: Some(DbtQuoting {
                database: Some(resolver_state.root_project_quoting.database),
                schema: Some(resolver_state.root_project_quoting.schema),
                identifier: Some(resolver_state.root_project_quoting.identifier),
                ..Default::default()
            }),
            ..Default::default()
        },
        nodes: resolver_state
            .nodes
            .models
            .iter()
            .map(|(id, node)| {
                (id.clone(), {
                    let mut model_node: ManifestModel = (**node).clone().into();

                    if is_public_model_from_publication(resolver_state, &model_node) {
                        model_node.__common_attr__.path = DbtPath::new();
                        model_node.__common_attr__.original_file_path = DbtPath::new();
                    } else {
                        let path_config = path_config_for_package(
                            resolver_state,
                            &model_node.__common_attr__.package_name,
                        );
                        normalize_manifest_common_path(
                            &mut model_node.__common_attr__,
                            path_config,
                            &path_config.model_paths,
                        );
                        normalize_manifest_patch_path(&mut model_node.__common_attr__, path_config);
                    }
                    DbtNode::Model(model_node)
                })
            })
            .chain(resolver_state.nodes.tests.iter().map(|(id, node)| {
                let mut test_node: ManifestDataTest = (**node).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &test_node.__common_attr__.package_name,
                );
                normalize_manifest_test_path(&mut test_node, path_config);
                (id.clone(), DbtNode::Test(test_node))
            }))
            .chain(resolver_state.nodes.snapshots.iter().map(|(id, node)| {
                let mut snapshot_node: ManifestSnapshot = (**node).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &snapshot_node.__common_attr__.package_name,
                );
                normalize_manifest_common_path(
                    &mut snapshot_node.__common_attr__,
                    path_config,
                    &path_config.snapshot_paths,
                );
                normalize_manifest_patch_path(&mut snapshot_node.__common_attr__, path_config);
                (id.clone(), DbtNode::Snapshot(snapshot_node))
            }))
            .chain(resolver_state.nodes.seeds.iter().map(|(id, node)| {
                let mut seed_node: ManifestSeed = (**node).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &seed_node.__common_attr__.package_name,
                );
                normalize_manifest_common_path(
                    &mut seed_node.__common_attr__,
                    path_config,
                    &path_config.seed_paths,
                );
                normalize_manifest_patch_path(&mut seed_node.__common_attr__, path_config);
                (id.clone(), DbtNode::Seed(seed_node))
            }))
            .chain(resolver_state.nodes.analyses.iter().map(|(id, node)| {
                let mut analysis_node: ManifestAnalysis = (**node).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &analysis_node.__common_attr__.package_name,
                );
                normalize_manifest_analysis_path(&mut analysis_node.__common_attr__, path_config);
                normalize_manifest_patch_path(&mut analysis_node.__common_attr__, path_config);
                (id.clone(), DbtNode::Analysis(analysis_node))
            }))
            .chain(resolver_state.nodes.checks.iter().map(|(id, node)| {
                let mut check_node: ManifestCheck = (**node).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &check_node.__common_attr__.package_name,
                );
                normalize_manifest_patch_path(&mut check_node.__common_attr__, path_config);
                (id.clone(), DbtNode::Check(check_node))
            }))
            // Note: Functions are now handled separately in the functions field, not in nodes
            .chain(resolver_state.operations.on_run_start.iter().map(|node| {
                (
                    node.__common_attr__.unique_id.clone(),
                    DbtNode::Operation((*node).clone().into_inner().into()),
                )
            }))
            .chain(resolver_state.operations.on_run_end.iter().map(|node| {
                (
                    node.__common_attr__.unique_id.clone(),
                    DbtNode::Operation((*node).clone().into_inner().into()),
                )
            }))
            .collect(),
        sources: resolver_state
            .nodes
            .sources
            .iter()
            .map(|(id, source)| {
                let mut manifest_source: ManifestSource = (**source).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &manifest_source.__common_attr__.package_name,
                );
                normalize_manifest_source_path(&mut manifest_source, path_config);
                (id.clone(), manifest_source)
            })
            .collect(),
        exposures: resolver_state
            .nodes
            .exposures
            .iter()
            .map(|(id, exposure)| {
                let mut manifest_exposure: ManifestExposure = (**exposure).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &manifest_exposure.__common_attr__.package_name,
                );
                normalize_manifest_common_attrs_property_path(
                    &mut manifest_exposure.__common_attr__,
                    path_config,
                );
                (id.clone(), manifest_exposure)
            })
            .collect(),
        semantic_models: resolver_state
            .nodes
            .semantic_models
            .iter()
            .map(|(id, semantic_model)| {
                let mut manifest_semantic_model: ManifestSemanticModel =
                    (**semantic_model).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &manifest_semantic_model.__common_attr__.package_name,
                );
                normalize_manifest_common_attrs_property_path(
                    &mut manifest_semantic_model.__common_attr__,
                    path_config,
                );
                (id.clone(), manifest_semantic_model)
            })
            .collect(),
        metrics: resolver_state
            .nodes
            .metrics
            .iter()
            .map(|(id, metric)| {
                let mut manifest_metric: ManifestMetric = (**metric).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &manifest_metric.__common_attr__.package_name,
                );
                normalize_manifest_common_attrs_property_path(
                    &mut manifest_metric.__common_attr__,
                    path_config,
                );
                (id.clone(), manifest_metric)
            })
            .collect(),
        saved_queries: resolver_state
            .nodes
            .saved_queries
            .iter()
            .map(|(id, saved_query)| {
                let mut manifest_saved_query: ManifestSavedQuery = (**saved_query).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &manifest_saved_query.__common_attr__.package_name,
                );
                normalize_manifest_common_attrs_property_path(
                    &mut manifest_saved_query.__common_attr__,
                    path_config,
                );
                (id.clone(), manifest_saved_query)
            })
            .collect(),
        unit_tests: resolver_state
            .nodes
            .unit_tests
            .iter()
            .map(|(id, unit_test)| {
                let mut manifest_unit_test: ManifestUnitTest = (**unit_test).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &manifest_unit_test.__common_attr__.package_name,
                );
                normalize_manifest_materializable_property_path(
                    &mut manifest_unit_test.__common_attr__,
                    path_config,
                );
                (id.clone(), manifest_unit_test)
            })
            .collect(),
        macros: resolver_state
            .macros
            .macros
            .iter()
            .filter(|(id, _)| id.starts_with("macro."))
            .map(|(id, macro_)| (id.clone(), macro_.clone().into()))
            .collect(),
        functions: resolver_state
            .nodes
            .functions
            .iter()
            .map(|(id, function)| {
                let mut manifest_function: ManifestFunction = (**function).clone().into();
                let path_config = path_config_for_package(
                    resolver_state,
                    &manifest_function.__common_attr__.package_name,
                );
                normalize_manifest_common_path(
                    &mut manifest_function.__common_attr__,
                    path_config,
                    &path_config.function_paths,
                );
                normalize_manifest_patch_path(&mut manifest_function.__common_attr__, path_config);
                (id.clone(), manifest_function)
            })
            .collect(),
        groups: resolver_state
            .nodes
            .groups
            .iter()
            .map(|(id, group)| {
                let mut manifest_group: ManifestGroup = (**group).clone().into();
                let path_config =
                    path_config_for_package(resolver_state, &manifest_group.package_name);
                manifest_group.path = DbtPath::from(strip_resource_path(
                    &manifest_group.path,
                    &path_config.model_paths,
                ));
                (id.clone(), manifest_group)
            })
            .collect(),
        selectors: resolver_state.manifest_selectors.clone(),
        docs: resolver_state
            .macros
            .docs_macros
            .iter()
            .map(|(id, docs_macro)| {
                let mut docs_macro = docs_macro.clone();
                let path_config = path_config_for_package(resolver_state, &docs_macro.package_name);
                normalize_docs_macro_path(&mut docs_macro, path_config);
                (id.clone(), docs_macro)
            })
            .collect(),
        parent_map,
        child_map,
        group_map,
        disabled,
    }
}

/// Returns the path config that owns a manifest node's resource paths.
///
/// Manifest `path` conformance is package-relative: dependency resources must be normalized
/// using that dependency package's `model-paths`, `test-paths`, etc., not the root project's.
fn path_config_for_package<'a>(
    resolver_state: &'a ResolverState,
    package_name: &str,
) -> &'a ManifestPathConfig {
    let root_config = resolver_state
        .manifest_path_configs
        .get(&resolver_state.root_project_name);
    if package_name == resolver_state.root_project_name {
        return root_config.expect("root manifest path config missing");
    }

    resolver_state
        .manifest_path_configs
        .get(package_name)
        .or(root_config)
        .expect("manifest path config missing for manifest node package")
}

/// True only for public models imported via a publication artifact (cross-project
/// mesh), whose source files don't exist locally. Public models from a
/// locally-parsed package (`local:` / `git:` / `registry:`) always have a
/// `ManifestPathConfig` registered and must keep their real paths so dbt-core
/// can locate the compiled output — matches mantle's behaviour.
fn is_public_model_from_publication(resolver_state: &ResolverState, model: &ManifestModel) -> bool {
    let package = &model.__common_attr__.package_name;
    model.access == Some(Access::Public)
        && &resolver_state.root_project_name != package
        && !resolver_state.manifest_path_configs.contains_key(package)
}

/// dbt-core manifest conformance: `original_file_path` stays project-relative,
/// while `path` is serialized relative to the configured resource root.
///
/// Used for resources with one obvious root list: models, snapshots, seeds,
/// functions, unit tests, and singular tests.
fn normalize_manifest_common_path(
    common: &mut ManifestMaterializableCommonAttributes,
    path_config: &ManifestPathConfig,
    resource_paths: &[String],
) {
    let package_relative_path = strip_package_root_path(&common.path, path_config);
    common.path = DbtPath::from(strip_resource_path(&package_relative_path, resource_paths));
    common.original_file_path = DbtPath::from(strip_package_root_path(
        &common.original_file_path,
        path_config,
    ));
}

/// dbt-core property resources may live under several configured roots.
/// Strip the first matching root from the serialized manifest `path`.
fn normalize_manifest_common_attrs_property_path(
    common: &mut ManifestCommonAttributes,
    path_config: &ManifestPathConfig,
) {
    let package_relative_path = strip_package_root_path(&common.path, path_config);
    common.path = DbtPath::from(strip_property_resource_path(
        &package_relative_path,
        path_config,
    ));
    common.original_file_path = DbtPath::from(strip_package_root_path(
        &common.original_file_path,
        path_config,
    ));
}

/// Same path normalization as property resources, for manifest nodes that use
/// materializable common attrs, such as unit tests.
fn normalize_manifest_materializable_property_path(
    common: &mut ManifestMaterializableCommonAttributes,
    path_config: &ManifestPathConfig,
) {
    let package_relative_path = strip_package_root_path(&common.path, path_config);
    common.path = DbtPath::from(strip_property_resource_path(
        &package_relative_path,
        path_config,
    ));
    common.original_file_path = DbtPath::from(strip_package_root_path(
        &common.original_file_path,
        path_config,
    ));
}

/// Analyses are special in dbt-core manifests: strip the configured analysis
/// root, then keep an `analysis/` prefix in the serialized `path`.
fn normalize_manifest_analysis_path(
    common: &mut ManifestMaterializableCommonAttributes,
    path_config: &ManifestPathConfig,
) {
    let package_relative_path = strip_package_root_path(&common.path, path_config);
    let stripped = strip_resource_path(&package_relative_path, &path_config.analysis_paths);
    common.path = if stripped == package_relative_path {
        DbtPath::from(stripped)
    } else {
        DbtPath::from("analysis").join(stripped)
    };
    common.original_file_path = DbtPath::from(strip_package_root_path(
        &common.original_file_path,
        path_config,
    ));
}

/// dbt-core emits patch paths as package URIs (`package://path`).
fn normalize_manifest_patch_path(
    common: &mut ManifestMaterializableCommonAttributes,
    path_config: &ManifestPathConfig,
) {
    let Some(patch_path) = common.patch_path.as_ref() else {
        return;
    };
    let package_relative_patch_path = strip_package_root_path(patch_path, path_config);
    common.patch_path = Some(DbtPath::from(package_uri_path(
        &common.package_name,
        &package_relative_patch_path,
    )));
}

/// Prefix bare paths with a package URI; leave existing URI-like paths alone.
fn package_uri_path(package_name: &str, path: &Path) -> PathBuf {
    if path
        .as_os_str()
        .as_encoded_bytes()
        .windows(3)
        .any(|bytes| bytes == b"://")
    {
        path.to_path_buf()
    } else {
        let mut package_uri_path = OsString::from(format!("{package_name}://"));
        package_uri_path.push(path.as_os_str());
        PathBuf::from(package_uri_path)
    }
}

/// Docs default to all resource roots unless `docs-paths` is explicitly set.
fn normalize_docs_macro_path(docs_macro: &mut DbtDocsMacro, path_config: &ManifestPathConfig) {
    let package_relative_path = strip_package_root_path(&docs_macro.path, path_config);
    docs_macro.path = if path_config.docs_paths.is_empty() {
        DbtPath::from(strip_default_docs_resource_path(
            &package_relative_path,
            path_config,
        ))
    } else {
        DbtPath::from(strip_resource_path(
            &package_relative_path,
            &path_config.docs_paths,
        ))
    };
    docs_macro.original_file_path = DbtPath::from(strip_package_root_path(
        &docs_macro.original_file_path,
        path_config,
    ));
}

/// Sources keep property-file paths project-relative in dbt-core manifests.
fn normalize_manifest_source_path(source: &mut ManifestSource, path_config: &ManifestPathConfig) {
    source.__common_attr__.path = DbtPath::from(strip_package_root_path(
        &source.__common_attr__.path,
        path_config,
    ));
    source.__common_attr__.original_file_path = DbtPath::from(strip_package_root_path(
        &source.__common_attr__.original_file_path,
        path_config,
    ));
}

/// Apply dbt-core's implicit docs search roots when `docs-paths` is omitted.
fn strip_default_docs_resource_path(path: &Path, path_config: &ManifestPathConfig) -> PathBuf {
    strip_resource_path_from_slices(
        path,
        &[
            path_config.analysis_paths.as_slice(),
            path_config.function_paths.as_slice(),
            path_config.macro_paths.as_slice(),
            path_config.model_paths.as_slice(),
            path_config.seed_paths.as_slice(),
            path_config.snapshot_paths.as_slice(),
            path_config.test_paths.as_slice(),
        ],
    )
}

/// Property files can define many resource types, so try each relevant root.
fn strip_property_resource_path(path: &Path, path_config: &ManifestPathConfig) -> PathBuf {
    strip_resource_path_from_slices(
        path,
        &[
            path_config.model_paths.as_slice(),
            path_config.seed_paths.as_slice(),
            path_config.snapshot_paths.as_slice(),
            path_config.analysis_paths.as_slice(),
            path_config.test_paths.as_slice(),
            path_config.function_paths.as_slice(),
            path_config.macro_paths.as_slice(),
        ],
    )
}

/// Return the first path with any configured resource root stripped.
fn strip_resource_path_from_slices(path: &Path, resource_path_slices: &[&[String]]) -> PathBuf {
    for resource_paths in resource_path_slices {
        let stripped = strip_resource_path(path, resource_paths);
        if stripped != path {
            return stripped;
        }
    }
    path.to_path_buf()
}

/// Normalize test `path` with dbt-core's generic-test special case.
///
/// Singular tests strip `test-paths`; generic tests are YAML-defined but backed
/// by generated SQL, and dbt-core serializes only that generated file name.
fn normalize_manifest_test_path(test: &mut ManifestDataTest, path_config: &ManifestPathConfig) {
    if is_generic_manifest_test(test) {
        if let Some(file_name) = test.__common_attr__.path.file_name() {
            test.__common_attr__.path = DbtPath::from(file_name.to_os_string());
        }
    } else {
        normalize_manifest_common_path(
            &mut test.__common_attr__,
            path_config,
            &path_config.test_paths,
        );
    }
    test.__common_attr__.original_file_path = DbtPath::from(strip_package_root_path(
        &test.__common_attr__.original_file_path,
        path_config,
    ));
}

/// Detect generic data tests after conversion into the manifest shape.
///
/// Generic tests have generated SQL distinct from the YAML declaration.
/// Singular SQL tests use the same path for both.
fn is_generic_manifest_test(test: &ManifestDataTest) -> bool {
    test.generated_sql_file
        .as_deref()
        .map(|generated_sql_file| {
            Path::new(generated_sql_file) != test.__common_attr__.original_file_path.as_path()
        })
        .unwrap_or(false)
}

/// Strip the dependency package root from paths that are stored relative to the root project.
fn strip_package_root_path(path: &Path, path_config: &ManifestPathConfig) -> PathBuf {
    if !path_config.package_root_prefix.as_os_str().is_empty()
        && path.starts_with(&path_config.package_root_prefix)
        && let Ok(stripped) = path.strip_prefix(&path_config.package_root_prefix)
        && !stripped.as_os_str().is_empty()
    {
        return stripped.to_path_buf();
    }
    path.to_path_buf()
}

/// Strip one configured resource-root prefix from a manifest `path`.
///
/// Converts `models/marts/orders.sql` to `marts/orders.sql`, while leaving
/// unmatched or already root-level paths unchanged.
fn strip_resource_path(path: &Path, resource_paths: &[String]) -> PathBuf {
    for resource_path in resource_paths {
        let resource_path = Path::new(resource_path);
        if path.starts_with(resource_path)
            && let Ok(stripped) = path.strip_prefix(resource_path)
            && !stripped.as_os_str().is_empty()
        {
            return stripped.to_path_buf();
        }
    }
    path.to_path_buf()
}

fn build_disabled_map(resolver_state: &ResolverState) -> BTreeMap<String, Vec<YmlValue>> {
    resolver_state
        .disabled_nodes
        .models
        .iter()
        .map(|(id, model)| {
            let mut manifest_model = ManifestModel::from((**model).clone());
            let path_config = path_config_for_package(
                resolver_state,
                &manifest_model.__common_attr__.package_name,
            );
            normalize_manifest_common_path(
                &mut manifest_model.__common_attr__,
                path_config,
                &path_config.model_paths,
            );
            normalize_manifest_patch_path(&mut manifest_model.__common_attr__, path_config);
            (
                id.clone(),
                vec![serialize_with_resource_type(
                    dbt_yaml::to_value(manifest_model).unwrap_or_default(),
                    "model",
                )],
            )
        })
        .chain(
            resolver_state
                .disabled_nodes
                .tests
                .iter()
                .map(|(id, test)| {
                    let mut manifest_test = ManifestDataTest::from((**test).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_test.__common_attr__.package_name,
                    );
                    normalize_manifest_test_path(&mut manifest_test, path_config);
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_test).unwrap_or_default(),
                            "test",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .snapshots
                .iter()
                .map(|(id, snapshot)| {
                    let mut manifest_snapshot = ManifestSnapshot::from((**snapshot).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_snapshot.__common_attr__.package_name,
                    );
                    normalize_manifest_common_path(
                        &mut manifest_snapshot.__common_attr__,
                        path_config,
                        &path_config.snapshot_paths,
                    );
                    normalize_manifest_patch_path(
                        &mut manifest_snapshot.__common_attr__,
                        path_config,
                    );
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_snapshot).unwrap_or_default(),
                            "snapshot",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .seeds
                .iter()
                .map(|(id, seed)| {
                    let mut manifest_seed = ManifestSeed::from((**seed).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_seed.__common_attr__.package_name,
                    );
                    normalize_manifest_common_path(
                        &mut manifest_seed.__common_attr__,
                        path_config,
                        &path_config.seed_paths,
                    );
                    normalize_manifest_patch_path(&mut manifest_seed.__common_attr__, path_config);
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_seed).unwrap_or_default(),
                            "seed",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .analyses
                .iter()
                .map(|(id, analysis)| {
                    let mut manifest_analysis = ManifestAnalysis::from((**analysis).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_analysis.__common_attr__.package_name,
                    );
                    normalize_manifest_analysis_path(
                        &mut manifest_analysis.__common_attr__,
                        path_config,
                    );
                    normalize_manifest_patch_path(
                        &mut manifest_analysis.__common_attr__,
                        path_config,
                    );
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_analysis).unwrap_or_default(),
                            "analysis",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .functions
                .iter()
                .map(|(id, function)| {
                    let mut manifest_function = ManifestFunction::from((**function).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_function.__common_attr__.package_name,
                    );
                    normalize_manifest_common_path(
                        &mut manifest_function.__common_attr__,
                        path_config,
                        &path_config.function_paths,
                    );
                    normalize_manifest_patch_path(
                        &mut manifest_function.__common_attr__,
                        path_config,
                    );
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_function).unwrap_or_default(),
                            "function",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .exposures
                .iter()
                .map(|(id, exposure)| {
                    let mut manifest_exposure = ManifestExposure::from((**exposure).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_exposure.__common_attr__.package_name,
                    );
                    normalize_manifest_common_attrs_property_path(
                        &mut manifest_exposure.__common_attr__,
                        path_config,
                    );
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_exposure).unwrap_or_default(),
                            "exposure",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .saved_queries
                .iter()
                .map(|(id, saved_query)| {
                    let mut manifest_saved_query =
                        ManifestSavedQuery::from((**saved_query).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_saved_query.__common_attr__.package_name,
                    );
                    normalize_manifest_common_attrs_property_path(
                        &mut manifest_saved_query.__common_attr__,
                        path_config,
                    );
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_saved_query).unwrap_or_default(),
                            "saved_query",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .unit_tests
                .iter()
                .map(|(id, unit_test)| {
                    let mut manifest_unit_test = ManifestUnitTest::from((**unit_test).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_unit_test.__common_attr__.package_name,
                    );
                    normalize_manifest_materializable_property_path(
                        &mut manifest_unit_test.__common_attr__,
                        path_config,
                    );
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_unit_test).unwrap_or_default(),
                            "unit_test",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .groups
                .iter()
                .map(|(id, group)| {
                    let mut manifest_group = ManifestGroup::from((**group).clone());
                    let path_config =
                        path_config_for_package(resolver_state, &manifest_group.package_name);
                    manifest_group.path = DbtPath::from(strip_resource_path(
                        &manifest_group.path,
                        &path_config.model_paths,
                    ));
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_group).unwrap_or_default(),
                            "group",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .sources
                .iter()
                .map(|(id, source)| {
                    let mut manifest_source = ManifestSource::from((**source).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_source.__common_attr__.package_name,
                    );
                    normalize_manifest_source_path(&mut manifest_source, path_config);
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_source).unwrap_or_default(),
                            "source",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .metrics
                .iter()
                .map(|(id, metric)| {
                    let mut manifest_metric = ManifestMetric::from((**metric).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_metric.__common_attr__.package_name,
                    );
                    normalize_manifest_common_attrs_property_path(
                        &mut manifest_metric.__common_attr__,
                        path_config,
                    );
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_metric).unwrap_or_default(),
                            "metric",
                        )],
                    )
                }),
        )
        .chain(
            resolver_state
                .disabled_nodes
                .semantic_models
                .iter()
                .map(|(id, semantic_model)| {
                    let mut manifest_semantic_model =
                        ManifestSemanticModel::from((**semantic_model).clone());
                    let path_config = path_config_for_package(
                        resolver_state,
                        &manifest_semantic_model.__common_attr__.package_name,
                    );
                    normalize_manifest_common_attrs_property_path(
                        &mut manifest_semantic_model.__common_attr__,
                        path_config,
                    );
                    (
                        id.clone(),
                        vec![serialize_with_resource_type(
                            dbt_yaml::to_value(manifest_semantic_model).unwrap_or_default(),
                            "semantic_model",
                        )],
                    )
                }),
        )
        .collect()
}

// Build map of group names to nodes in the group
fn build_group_map(nodes: &Nodes) -> BTreeMap<String, Vec<String>> {
    let mut group_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, model) in &nodes.models {
        if let Some(group) = &model.__model_attr__.group {
            group_map.entry(group.clone()).or_default().push(id.clone());
        }
    }
    for (id, semantic_model) in &nodes.semantic_models {
        if let Some(group) = &semantic_model.__semantic_model_attr__.group {
            group_map.entry(group.clone()).or_default().push(id.clone());
        }
    }
    for (id, metric) in &nodes.metrics {
        if let Some(group) = &metric.__metric_attr__.group {
            group_map.entry(group.clone()).or_default().push(id.clone());
        }
    }
    for (id, saved_query) in &nodes.saved_queries {
        if let Some(group) = &saved_query.__saved_query_attr__.group {
            group_map.entry(group.clone()).or_default().push(id.clone());
        }
    }
    group_map
}

/// Build parent and child dependency maps from the nodes.
/// Returns a tuple of (parent_map, child_map) where:
/// - parent_map: maps each node ID to a list of node IDs it depends on
/// - child_map: maps each node ID to a list of node IDs that depend on it
///
/// Mirrors dbt-core's `build_node_edges` invariant: every iterated node receives
/// a key in BOTH maps, even when its list is empty (leaf / root nodes).
fn build_parent_and_child_maps(
    nodes: &Nodes,
    operations: &Operations,
) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>) {
    let mut parent_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut child_map: BTreeMap<String, Vec<String>> = BTreeMap::new();

    // Collect all nodes with their dependencies
    let mut all_nodes: Vec<(String, NodeDependsOn)> = Vec::new();

    for (id, model) in &nodes.models {
        all_nodes.push((id.clone(), model.__base_attr__.depends_on.clone()));
    }

    for (id, test) in &nodes.tests {
        all_nodes.push((id.clone(), test.__base_attr__.depends_on.clone()));
    }

    for (id, seed) in &nodes.seeds {
        all_nodes.push((id.clone(), seed.__base_attr__.depends_on.clone()));
    }

    for (id, snapshot) in &nodes.snapshots {
        all_nodes.push((id.clone(), snapshot.__base_attr__.depends_on.clone()));
    }

    for (id, analysis) in &nodes.analyses {
        all_nodes.push((id.clone(), analysis.__base_attr__.depends_on.clone()));
    }

    for (id, exposure) in &nodes.exposures {
        all_nodes.push((id.clone(), exposure.__base_attr__.depends_on.clone()));
    }

    for (id, unit_test) in &nodes.unit_tests {
        all_nodes.push((id.clone(), unit_test.__base_attr__.depends_on.clone()));
    }

    for (id, semantic_model) in &nodes.semantic_models {
        all_nodes.push((id.clone(), semantic_model.__base_attr__.depends_on.clone()));
    }

    for (id, metric) in &nodes.metrics {
        all_nodes.push((id.clone(), metric.__base_attr__.depends_on.clone()));
    }

    for (id, saved_query) in &nodes.saved_queries {
        all_nodes.push((id.clone(), saved_query.__base_attr__.depends_on.clone()));
    }

    for (id, function) in &nodes.functions {
        all_nodes.push((id.clone(), function.__base_attr__.depends_on.clone()));
    }

    // on_run_start / on_run_end hooks land in the manifest as `operation.*` nodes
    // but live on resolver_state.operations rather than resolver_state.nodes.
    // Include them here so they receive entries in both maps too.
    for op in operations
        .on_run_start
        .iter()
        .chain(operations.on_run_end.iter())
    {
        all_nodes.push((
            op.__common_attr__.unique_id.clone(),
            op.__base_attr__.depends_on.clone(),
        ));
    }

    // Process all collected nodes
    for (node_id, depends_on) in all_nodes {
        // Initialize both maps for this node so leaf / root nodes end up with `[]`.
        parent_map.entry(node_id.clone()).or_default();
        child_map.entry(node_id.clone()).or_default();

        // Add parents and update child map
        for parent_id in &depends_on.nodes {
            // Add parent to this node's parent list
            parent_map
                .entry(node_id.clone())
                .or_default()
                .push(parent_id.clone());

            // Add this node as a child of the parent
            child_map
                .entry(parent_id.clone())
                .or_default()
                .push(node_id.clone());
        }
    }

    // Process sources (they typically don't have dependencies but can have children)
    for id in nodes.sources.keys() {
        // Sources usually don't depend on anything, but we ensure they exist in maps
        parent_map.entry(id.clone()).or_default();
        child_map.entry(id.clone()).or_default();
    }

    // Ensure all nodes that are referenced but don't have their own entry exist in the maps
    // This handles cases where a node is referenced as a parent but isn't in our nodes
    let all_parent_ids: Vec<String> = parent_map
        .values()
        .flat_map(|parents| parents.clone())
        .collect();

    for parent_id in all_parent_ids {
        parent_map.entry(parent_id.clone()).or_default();
        child_map.entry(parent_id).or_default();
    }

    // Match dbt-core's `_sort_values`: deterministic, sorted, dedup'd values.
    for v in parent_map.values_mut() {
        v.sort();
        v.dedup();
    }
    for v in child_map.values_mut() {
        v.sort();
        v.dedup();
    }

    (parent_map, child_map)
}

pub fn nodes_from_dbt_manifest(manifest: DbtManifest, dbt_quoting: DbtQuoting) -> Nodes {
    let mut nodes = Nodes::default();

    let adapter_type =
        AdapterType::from_str(&manifest.metadata.adapter_type).unwrap_or_else(|_| {
            panic!(
                "Invalid adapter_type in manifest {}",
                &manifest.metadata.adapter_type
            )
        });

    let source_default_quoting = default_dbt_quoting_for(adapter_type);

    // A previous manifest carries its own `adapter_type` (just parsed above), which is the
    // spelling authority for the alias-authored keys in that file -- so canonicalize
    // `unrendered_config` the same way the parser does on the write side, or a manifest written
    // by pre-fix Fusion (authored spelling) looks like a `state:modified` false positive against
    // freshly parsed (canonical) config. Suppress-only and idempotent (see the callee's doc
    // comment); on a duplicate the canonical spelling wins rather than erroring, since this is
    // data we did not write this run.
    let canonicalize_unrendered_config = |cfg: BTreeMap<String, YmlValue>| {
        dbt_adapter_core::config_aliases::canonicalize_previous_manifest_config_keys(
            adapter_type,
            cfg,
        )
    };

    // Do not put disabled nodes into the nodes, because all things in Nodes object should be enabled.
    for (unique_id, node) in manifest.nodes.clone() {
        match node {
            DbtNode::Model(model) => {
                nodes.models.insert(
                    unique_id,
                    Arc::new(manifest_model_to_dbt_model(model, &manifest, dbt_quoting)),
                );
            }
            DbtNode::Test(test) => {
                nodes.tests.insert(
                    unique_id,
                    Arc::new(DbtTest {
                        // TODO: persist the line/column info through the manifest as well
                        defined_at: Some(
                            test.__common_attr__.original_file_path.to_path_buf().into(),
                        ),

                        manifest_original_file_path: test
                            .__common_attr__
                            .original_file_path
                            .clone(),

                        __common_attr__: CommonAttributes {
                            unique_id: test.__common_attr__.unique_id,
                            name: test.__common_attr__.name,
                            package_name: test.__common_attr__.package_name,
                            path: test.__common_attr__.path,
                            name_span: Span::default(),

                            original_file_path: test.generated_sql_file.map_or_else(
                                // Note: for fusion generated manifests, the
                                // `generated_sql_file` field should really never be
                                // None (see [ManifestDataTest])
                                || test.__common_attr__.original_file_path.clone(),
                                DbtPath::from,
                            ),
                            patch_path: test.__common_attr__.patch_path,

                            fqn: test.__common_attr__.fqn,
                            description: test.__common_attr__.description,
                            raw_code: test.__base_attr__.raw_code,
                            checksum: test.__base_attr__.checksum,
                            language: test.__base_attr__.language,
                            tags: test
                                .config
                                .tags
                                .inner()
                                .clone()
                                .map(Into::into)
                                .unwrap_or_default(),
                            classifiers: Default::default(),
                            meta: test.config.meta.clone().unwrap_or_default(),
                        },
                        __base_attr__: NodeBaseAttributes {
                            adapter: adapter_type,
                            // `propagate`, like `adapter`, never reaches the manifest, so a node
                            // read back from one has no selection to recover -- the same reason
                            // `adapter` falls back to the manifest's global `adapter_type` here.
                            propagate: Vec::new(),
                            database: test.__common_attr__.database,
                            schema: test.__common_attr__.schema,
                            alias: test.__base_attr__.alias,
                            relation_name: test.__base_attr__.relation_name,
                            materialized: DataTestConfig::default_materialized(),
                            static_analysis: Default::default(),
                            static_analysis_off_reason: None,
                            compute: test.config.compute,
                            enabled: test.config.get_enabled_with_default(),
                            extended_model: false,
                            quoting: test
                                .config
                                .quoting
                                .map(|mut quoting| {
                                    quoting.default_to(&dbt_quoting);
                                    quoting
                                })
                                .unwrap_or(dbt_quoting)
                                .try_into()
                                .expect("DbtQuoting should be set"),
                            quoting_ignore_case: false,
                            persist_docs: None,
                            columns: test.__base_attr__.columns,
                            depends_on: test.__base_attr__.depends_on,
                            refs: test.__base_attr__.refs,
                            sources: test.__base_attr__.sources,
                            functions: test.__base_attr__.functions,
                            metrics: test.__base_attr__.metrics,
                            unrendered_config: canonicalize_unrendered_config(
                                test.__base_attr__.unrendered_config,
                            ),
                        },
                        __test_attr__: DbtTestAttr {
                            column_name: test.column_name,
                            attached_node: test.attached_node,
                            test_metadata: test.test_metadata,
                            file_key_name: test.file_key_name,
                            introspection: IntrospectionKind::None,
                            original_name: None,
                            group: None,
                            state: test.config.state.clone(),
                        },
                        __adapter_attr__: AdapterAttr::from_config_and_dialect(
                            &test.config.__warehouse_specific_config__,
                            AdapterType::from_str(&manifest.metadata.adapter_type)
                                .expect("Unknown or unsupported adapter type"),
                        ),
                        deprecated_config: test.config,
                        __other__: test.__other__,
                    }),
                );
            }
            DbtNode::Snapshot(snapshot) => {
                let recalculated_checksum = match snapshot.__base_attr__.raw_code.clone() {
                    Some(raw_code) => {
                        // Recalculate checksum that eliminates whitespace and case differences.
                        let normalized_raw_code = normalize_sql(&raw_code);
                        let normalized_mantle_conforming_raw_code =
                            conform_normalized_snapshot_raw_code_to_mantle_format(
                                normalized_raw_code.as_str(),
                            );
                        recalculate_checksum(
                            Some(normalized_mantle_conforming_raw_code.as_str()),
                            snapshot.__base_attr__.checksum.clone(),
                        )
                    }
                    None => snapshot.__base_attr__.checksum.clone(),
                };

                nodes.snapshots.insert(
                    unique_id,
                    Arc::new(DbtSnapshot {
                        __common_attr__: CommonAttributes {
                            unique_id: snapshot.__common_attr__.unique_id,
                            name: snapshot.__common_attr__.name,
                            package_name: snapshot.__common_attr__.package_name,
                            path: snapshot.__common_attr__.path,
                            name_span: Span::default(),
                            original_file_path: snapshot.__common_attr__.original_file_path,
                            patch_path: snapshot.__common_attr__.patch_path,
                            fqn: snapshot.__common_attr__.fqn,
                            description: snapshot.__common_attr__.description,
                            raw_code: snapshot.__base_attr__.raw_code,
                            checksum: recalculated_checksum,
                            language: snapshot.__base_attr__.language,
                            tags: snapshot
                                .config
                                .tags
                                .clone()
                                .map(Into::into)
                                .unwrap_or_default(),
                            classifiers: Default::default(),
                            meta: snapshot.config.meta.clone().unwrap_or_default(),
                        },
                        __base_attr__: NodeBaseAttributes {
                            adapter: adapter_type,
                            propagate: Vec::new(),
                            database: snapshot.__common_attr__.database,
                            schema: snapshot.__common_attr__.schema,
                            alias: snapshot.__base_attr__.alias,
                            relation_name: snapshot.__base_attr__.relation_name,
                            compute: snapshot.config.compute,
                            enabled: snapshot.config.enabled.unwrap_or(true),
                            extended_model: false,
                            materialized: snapshot
                                .config
                                .materialized
                                .clone()
                                .unwrap_or_else(SnapshotConfig::default_materialized),
                            static_analysis: Default::default(),
                            static_analysis_off_reason: None,
                            quoting: snapshot
                                .config
                                .quoting
                                .map(|mut quoting| {
                                    quoting.default_to(&dbt_quoting);
                                    quoting
                                })
                                .unwrap_or(dbt_quoting)
                                .try_into()
                                .expect("DbtQuoting should be set"),
                            quoting_ignore_case: false,
                            persist_docs: snapshot.config.persist_docs.clone(),
                            columns: snapshot.__base_attr__.columns,
                            depends_on: snapshot.__base_attr__.depends_on,
                            refs: snapshot.__base_attr__.refs,
                            sources: snapshot.__base_attr__.sources,
                            functions: snapshot.__base_attr__.functions,
                            metrics: snapshot.__base_attr__.metrics,
                            unrendered_config: canonicalize_unrendered_config(
                                snapshot.__base_attr__.unrendered_config,
                            ),
                        },
                        __snapshot_attr__: DbtSnapshotAttr {
                            snapshot_meta_column_names: snapshot
                                .config
                                .snapshot_meta_column_names
                                .clone()
                                .unwrap_or_default(),
                            introspection: IntrospectionKind::None,
                            sync: snapshot.config.sync.clone(),
                            state: snapshot.config.state.clone(),
                        },
                        __adapter_attr__: AdapterAttr::from_config_and_dialect(
                            &snapshot.config.__warehouse_specific_config__,
                            AdapterType::from_str(&manifest.metadata.adapter_type)
                                .expect("Unknown or unsupported adapter type"),
                        ),
                        deprecated_config: snapshot.config.into(),
                        compiled: snapshot.__base_attr__.compiled,
                        compiled_code: snapshot.__base_attr__.compiled_code,
                        __other__: snapshot.__other__,
                    }),
                );
            }
            DbtNode::Seed(seed) => {
                nodes.seeds.insert(
                    unique_id,
                    Arc::new(DbtSeed {
                        __common_attr__: CommonAttributes {
                            unique_id: seed.__common_attr__.unique_id,
                            name: seed.__common_attr__.name,
                            package_name: seed.__common_attr__.package_name,
                            path: seed.__common_attr__.path,
                            name_span: Span::default(),
                            original_file_path: seed.__common_attr__.original_file_path,
                            patch_path: seed.__common_attr__.patch_path,
                            fqn: seed.__common_attr__.fqn,
                            description: seed.__common_attr__.description,
                            raw_code: seed.__base_attr__.raw_code,
                            checksum: seed.__base_attr__.checksum,
                            language: seed.__base_attr__.language,
                            tags: seed.config.tags.clone().map(Into::into).unwrap_or_default(),
                            classifiers: Default::default(),
                            meta: seed.config.meta.clone().unwrap_or_default(),
                        },
                        __base_attr__: NodeBaseAttributes {
                            adapter: adapter_type,
                            propagate: Vec::new(),
                            database: seed.__common_attr__.database,
                            schema: seed.__common_attr__.schema,
                            alias: seed.__base_attr__.alias,
                            relation_name: seed.__base_attr__.relation_name,
                            materialized: DbtMaterialization::Table,
                            static_analysis: Default::default(),
                            static_analysis_off_reason: None,
                            compute: None,
                            enabled: seed.config.enabled.unwrap_or(true),
                            quoting: seed
                                .config
                                .quoting
                                .map(|mut quoting| {
                                    quoting.default_to(&dbt_quoting);
                                    quoting
                                })
                                .unwrap_or(dbt_quoting)
                                .try_into()
                                .expect("DbtQuoting should be set"),
                            quoting_ignore_case: false,
                            extended_model: false,
                            persist_docs: seed.config.persist_docs.clone(),
                            columns: seed.__base_attr__.columns,
                            depends_on: seed.__base_attr__.depends_on,
                            refs: seed.__base_attr__.refs,
                            sources: seed.__base_attr__.sources,
                            functions: seed.__base_attr__.functions,
                            metrics: seed.__base_attr__.metrics,
                            unrendered_config: canonicalize_unrendered_config(
                                seed.__base_attr__.unrendered_config,
                            ),
                        },
                        __seed_attr__: DbtSeedAttr {
                            quote_columns: seed.config.quote_columns.unwrap_or_default(),
                            column_types: seed.config.column_types.clone(),
                            delimiter: seed.config.delimiter.clone().map(|d| d.into_inner()),
                            root_path: seed.root_path,
                            catalog_name: seed.config.catalog_name.clone(),
                        },
                        deprecated_config: seed.config.into(),
                        __other__: seed.__other__,
                    }),
                );
            }
            DbtNode::Operation(_) => {}
            DbtNode::Function(function) => {
                nodes.functions.insert(
                    unique_id,
                    Arc::new(manifest_function_to_dbt_function(
                        function,
                        dbt_quoting,
                        adapter_type,
                    )),
                );
            }
            DbtNode::Analysis(analysis) => {
                let config = analysis.config;
                let tags = config
                    .tags
                    .inner()
                    .clone()
                    .map(Into::into)
                    .unwrap_or_default();
                let meta = config.meta.clone().unwrap_or_default();

                let recalculated_checksum = match analysis.__base_attr__.raw_code.clone() {
                    Some(raw_code) => {
                        let normalized_raw_code = normalize_sql(&raw_code);
                        recalculate_checksum(
                            Some(normalized_raw_code.as_str()),
                            analysis.__base_attr__.checksum.clone(),
                        )
                    }
                    None => analysis.__base_attr__.checksum.clone(),
                };
                nodes.analyses.insert(
                    unique_id,
                    Arc::new(DbtAnalysis {
                        __common_attr__: CommonAttributes {
                            unique_id: analysis.__common_attr__.unique_id,
                            name: analysis.__common_attr__.name,
                            package_name: analysis.__common_attr__.package_name,
                            path: analysis.__common_attr__.path,
                            name_span: Span::default(),
                            original_file_path: analysis.__common_attr__.original_file_path,
                            patch_path: analysis.__common_attr__.patch_path,
                            fqn: analysis.__common_attr__.fqn,
                            description: analysis.__common_attr__.description,
                            raw_code: analysis.__base_attr__.raw_code,
                            checksum: recalculated_checksum,
                            language: analysis.__base_attr__.language,
                            tags,
                            classifiers: Default::default(),
                            meta,
                        },
                        __base_attr__: NodeBaseAttributes {
                            adapter: adapter_type,
                            propagate: Vec::new(),
                            database: analysis.__common_attr__.database,
                            schema: analysis.__common_attr__.schema,
                            alias: analysis.__base_attr__.alias,
                            relation_name: analysis.__base_attr__.relation_name,
                            materialized: analysis.materialized,
                            static_analysis: Spanned::new(analysis.static_analysis),
                            enabled: analysis.enabled,
                            static_analysis_off_reason: None,
                            compute: None,
                            extended_model: false,
                            quoting: analysis
                                .quoting
                                .map(|mut quoting| {
                                    quoting.default_to(&dbt_quoting);
                                    quoting
                                })
                                .unwrap_or(dbt_quoting)
                                .try_into()
                                .expect("DbtQuoting should be set"),
                            quoting_ignore_case: analysis.quoting_ignore_case,
                            persist_docs: analysis.persist_docs.clone(),
                            columns: analysis.__base_attr__.columns,
                            depends_on: analysis.__base_attr__.depends_on,
                            refs: analysis.__base_attr__.refs,
                            sources: analysis.__base_attr__.sources,
                            metrics: analysis.__base_attr__.metrics,
                            functions: analysis.__base_attr__.functions,
                            unrendered_config: analysis.__base_attr__.unrendered_config,
                        },
                        __analysis_attr__: DbtAnalysisAttr::default(),
                        deprecated_config: config,
                        __other__: analysis.__other__,
                    }),
                );
            }
            DbtNode::Check(check) => {
                let config = check.config;
                let tags = config
                    .tags
                    .inner()
                    .clone()
                    .map(Into::into)
                    .unwrap_or_default();
                let meta = config.meta.clone().unwrap_or_default();

                let recalculated_checksum = match check.__base_attr__.raw_code.clone() {
                    Some(raw_code) => {
                        let normalized_raw_code = normalize_sql(&raw_code);
                        recalculate_checksum(
                            Some(normalized_raw_code.as_str()),
                            check.__base_attr__.checksum.clone(),
                        )
                    }
                    None => check.__base_attr__.checksum.clone(),
                };
                nodes.checks.insert(
                    unique_id,
                    Arc::new(DbtCheck {
                        __common_attr__: CommonAttributes {
                            unique_id: check.__common_attr__.unique_id,
                            name: check.__common_attr__.name,
                            package_name: check.__common_attr__.package_name,
                            path: check.__common_attr__.path,
                            name_span: Span::default(),
                            original_file_path: check.__common_attr__.original_file_path,
                            patch_path: check.__common_attr__.patch_path,
                            fqn: check.__common_attr__.fqn,
                            description: check.__common_attr__.description,
                            raw_code: check.__base_attr__.raw_code,
                            checksum: recalculated_checksum,
                            language: check.__base_attr__.language,
                            tags,
                            classifiers: Default::default(),
                            meta,
                        },
                        __base_attr__: NodeBaseAttributes {
                            adapter: adapter_type,
                            propagate: Vec::new(),
                            // A check has no relation, so these stay as written (empty) rather
                            // than being defaulted from the target.
                            database: check.__common_attr__.database,
                            schema: check.__common_attr__.schema,
                            alias: check.__base_attr__.alias,
                            relation_name: check.__base_attr__.relation_name,
                            materialized: DbtMaterialization::Analysis,
                            static_analysis: Default::default(),
                            enabled: check.enabled,
                            static_analysis_off_reason: None,
                            compute: None,
                            extended_model: false,
                            quoting: dbt_quoting.try_into().expect("DbtQuoting should be set"),
                            quoting_ignore_case: false,
                            persist_docs: None,
                            columns: check.__base_attr__.columns,
                            depends_on: check.__base_attr__.depends_on,
                            refs: check.__base_attr__.refs,
                            sources: check.__base_attr__.sources,
                            metrics: check.__base_attr__.metrics,
                            functions: check.__base_attr__.functions,
                            unrendered_config: check.__base_attr__.unrendered_config,
                        },
                        __check_attr__: DbtCheckAttr {
                            // Neither field is carried by `ManifestCheck`. A node reconstructed from
                            // a manifest is only ever *compared* (`--state`), never executed or
                            // scheduled — execution reads the rendered SQL and scheduling reads the
                            // table list, and both of those paths come from a fresh resolve or the
                            // parse cache, which do round-trip them.
                            compiled_sql: None,
                        },
                        deprecated_config: config,
                    }),
                );
            }
        }
    }
    for (unique_id, source) in manifest.sources {
        let user_quoting = source.quoting;
        nodes.sources.insert(
            unique_id,
            Arc::new(DbtSource {
                __common_attr__: CommonAttributes {
                    unique_id: source.__common_attr__.unique_id,
                    name: source.__common_attr__.name,
                    package_name: source.__common_attr__.package_name,
                    path: source.__common_attr__.path,
                    name_span: Span::default(),
                    original_file_path: source.__common_attr__.original_file_path,
                    patch_path: source.__common_attr__.patch_path,
                    fqn: source.__common_attr__.fqn,
                    description: source.__common_attr__.description,
                    raw_code: None,
                    checksum: DbtChecksum::default(),
                    language: None,
                    tags: source
                        .config
                        .tags
                        .inner()
                        .clone()
                        .map(Into::into)
                        .unwrap_or_default(),
                    classifiers: Default::default(),
                    meta: source.config.meta.clone().unwrap_or_default(),
                },
                __base_attr__: NodeBaseAttributes {
                    adapter: adapter_type,
                    propagate: Vec::new(),
                    database: source.__common_attr__.database,
                    schema: source.__common_attr__.schema,
                    alias: source.identifier.clone(),
                    relation_name: source.relation_name,
                    materialized: DbtMaterialization::Table,
                    static_analysis: Default::default(),
                    static_analysis_off_reason: None,
                    compute: None,
                    enabled: source.config.enabled.unwrap_or(true),
                    extended_model: false,
                    quoting: source
                        .quoting
                        .map(|mut quoting| {
                            quoting.default_to(&source_default_quoting);
                            quoting
                        })
                        .unwrap_or(source_default_quoting)
                        .try_into()
                        .expect("DbtQuoting should be set"),
                    quoting_ignore_case: false,
                    persist_docs: None,
                    columns: source.columns,
                    depends_on: NodeDependsOn::default(),
                    refs: vec![],
                    sources: vec![],
                    functions: vec![],
                    metrics: vec![],
                    unrendered_config: canonicalize_unrendered_config(source.unrendered_config),
                },
                __source_attr__: DbtSourceAttr {
                    identifier: source.identifier,
                    source_name: source.source_name,
                    source_description: source.source_description,
                    loader: source.loader,
                    loaded_at_field: source.loaded_at_field,
                    loaded_at_query: source.loaded_at_query,
                    user_quoting,
                    freshness: source.freshness,
                    schema_origin: source.config.schema_origin.unwrap_or_default(),
                    sync: source.config.sync.clone(),
                    unrendered_database: source.unrendered_database,
                    unrendered_schema: source.unrendered_schema,
                    external: source.external,
                    catalog_name: source
                        .config
                        .__warehouse_specific_config__
                        .catalog_name
                        .clone(),
                },
                deprecated_config: source.config,
                __other__: source.__other__,
            }),
        );
    }
    for (unique_id, exposure) in manifest.exposures {
        nodes.exposures.insert(
            unique_id,
            Arc::new(crate::schemas::nodes::DbtExposure {
                __common_attr__: CommonAttributes {
                    name: exposure.__common_attr__.name,
                    package_name: exposure.__common_attr__.package_name,
                    path: exposure.__common_attr__.path,
                    name_span: Span::default(),
                    original_file_path: exposure.__common_attr__.original_file_path,
                    patch_path: None,
                    unique_id: exposure.__common_attr__.unique_id,
                    fqn: exposure.__common_attr__.fqn,
                    description: exposure.__common_attr__.description,
                    checksum: Default::default(),
                    language: None,
                    raw_code: None,
                    tags: vec![],
                    classifiers: Default::default(),
                    meta: IndexMap::new(),
                },
                __base_attr__: NodeBaseAttributes {
                    adapter: adapter_type,
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
                    refs: exposure.__base_attr__.refs,
                    sources: exposure.__base_attr__.sources,
                    functions: vec![],
                    metrics: exposure.__base_attr__.metrics,
                    depends_on: exposure.__base_attr__.depends_on,
                    quoting_ignore_case: false,
                    unrendered_config: Default::default(),
                },
                __exposure_attr__: crate::schemas::nodes::DbtExposureAttr {
                    owner: exposure.owner,
                    label: exposure.label,
                    maturity: exposure.maturity,
                    type_: exposure.type_,
                    url: exposure.url,
                    unrendered_config: canonicalize_unrendered_config(
                        exposure.__base_attr__.unrendered_config,
                    ),
                    created_at: exposure.__base_attr__.created_at,
                },
                deprecated_config: exposure.config,
            }),
        );
    }
    for (unique_id, unit_test) in manifest.unit_tests {
        nodes.unit_tests.insert(
            unique_id,
            Arc::new(DbtUnitTest {
                __common_attr__: CommonAttributes {
                    unique_id: unit_test.__common_attr__.unique_id,
                    name: unit_test.__common_attr__.name,
                    package_name: unit_test.__common_attr__.package_name,
                    path: unit_test.__common_attr__.path,
                    name_span: Span::default(),
                    original_file_path: unit_test.__common_attr__.original_file_path,
                    patch_path: unit_test.__common_attr__.patch_path,
                    fqn: unit_test.__common_attr__.fqn,
                    description: unit_test.__common_attr__.description,
                    raw_code: unit_test.__base_attr__.raw_code,
                    checksum: unit_test.__base_attr__.checksum,
                    language: unit_test.__base_attr__.language,
                    tags: unit_test
                        .config
                        .tags
                        .inner()
                        .clone()
                        .map(Into::into)
                        .unwrap_or_default(),
                    classifiers: Default::default(),
                    meta: unit_test.config.meta.clone().unwrap_or_default(),
                },
                __base_attr__: NodeBaseAttributes {
                    adapter: adapter_type,
                    propagate: Vec::new(),
                    database: unit_test.__common_attr__.database,
                    schema: unit_test.__common_attr__.schema,
                    alias: unit_test.__base_attr__.alias,
                    relation_name: unit_test.__base_attr__.relation_name,
                    materialized: DbtMaterialization::Table,
                    static_analysis: Default::default(),
                    static_analysis_off_reason: None,
                    compute: unit_test.config.compute,
                    quoting: dbt_quoting.try_into().expect("DbtQuoting should be set"),
                    quoting_ignore_case: false,
                    enabled: unit_test.config.enabled.unwrap_or(true),
                    extended_model: false,
                    persist_docs: None,
                    columns: unit_test.__base_attr__.columns,
                    depends_on: unit_test.__base_attr__.depends_on,
                    refs: unit_test.__base_attr__.refs,
                    sources: unit_test.__base_attr__.sources,
                    functions: unit_test.__base_attr__.functions,
                    metrics: unit_test.__base_attr__.metrics,
                    unrendered_config: unit_test.__base_attr__.unrendered_config,
                },
                __unit_test_attr__: DbtUnitTestAttr {
                    model: unit_test.model,
                    given: unit_test.given,
                    expect: unit_test.expect,
                    versions: unit_test.versions,
                    version: unit_test.version,
                    overrides: unit_test.overrides,
                },
                field_event_status: unit_test.field_event_status,
                field_pre_injected_sql: unit_test.field_pre_injected_sql,
                tested_node_unique_id: unit_test.tested_node_unique_id,
                this_input_node_unique_id: unit_test.this_input_node_unique_id,
                defined_at: None,
                deprecated_config: unit_test.config,
            }),
        );
    }
    for (unique_id, semantic_model) in manifest.semantic_models {
        // TODO: I don't like the inconsistency of using From trait here,
        // although it seems everything should be refactored to use that instead
        let mut dbt_semantic_model: crate::schemas::manifest::DbtSemanticModel =
            semantic_model.into();
        dbt_semantic_model.__semantic_model_attr__.unrendered_config =
            canonicalize_unrendered_config(
                dbt_semantic_model.__semantic_model_attr__.unrendered_config,
            );
        nodes
            .semantic_models
            .insert(unique_id, Arc::new(dbt_semantic_model));
    }
    for (unique_id, metric) in manifest.metrics {
        // Load previous-state metrics so `state:modified` can compare them. Without this,
        // `previous_node_for` never finds a metric and every metric is unconditionally
        // reported as modified (dbt-core#15513).
        let mut dbt_metric: crate::schemas::manifest::DbtMetric = metric.into();
        dbt_metric.__metric_attr__.unrendered_config =
            canonicalize_unrendered_config(dbt_metric.__metric_attr__.unrendered_config);
        nodes.metrics.insert(unique_id, Arc::new(dbt_metric));
    }
    for (unique_id, saved_query) in manifest.saved_queries {
        nodes.saved_queries.insert(
            unique_id,
            Arc::new(crate::schemas::manifest::DbtSavedQuery {
                __common_attr__: CommonAttributes {
                    unique_id: saved_query.__common_attr__.unique_id,
                    name: saved_query.__common_attr__.name,
                    package_name: saved_query.__common_attr__.package_name,
                    path: saved_query.__common_attr__.path,
                    original_file_path: saved_query.__common_attr__.original_file_path,
                    patch_path: None, // TODO: Add to ManifestSavedQueryCommonAttributes if needed
                    fqn: saved_query.__common_attr__.fqn,
                    description: saved_query.__common_attr__.description,
                    raw_code: None,
                    checksum: DbtChecksum::default(),
                    name_span: Span::default(),
                    language: None,
                    tags: saved_query
                        .config
                        .tags
                        .inner()
                        .clone()
                        .map(Into::into)
                        .unwrap_or_default(),
                    classifiers: Default::default(),
                    meta: saved_query.config.meta.clone().unwrap_or_default(),
                },
                __base_attr__: NodeBaseAttributes {
                    adapter: adapter_type,
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
                    columns: Default::default(),
                    refs: saved_query.__base_attr__.refs,
                    sources: vec![],
                    functions: vec![],
                    metrics: vec![],
                    depends_on: saved_query.__base_attr__.depends_on,
                    quoting_ignore_case: false,
                    unrendered_config: Default::default(),
                },
                __saved_query_attr__: DbtSavedQueryAttr {
                    query_params: saved_query.query_params,
                    exports: saved_query.exports,
                    label: saved_query.label,
                    metadata: saved_query.metadata,
                    unrendered_config: canonicalize_unrendered_config(
                        saved_query.__base_attr__.unrendered_config,
                    ),
                    created_at: saved_query.__base_attr__.created_at,
                    group: saved_query.group,
                    cache: saved_query.config.cache.clone(),
                },
                deprecated_config: saved_query.config,
                __other__: saved_query.__other__,
            }),
        );
    }
    for (unique_id, group) in manifest.groups {
        nodes.groups.insert(
            unique_id.clone(),
            Arc::new(DbtGroup {
                __common_attr__: CommonAttributes {
                    name: group.name.to_string(),
                    package_name: group.package_name.to_string(),
                    path: group.path.clone(),
                    name_span: Span::default(),
                    original_file_path: group.original_file_path.clone(),
                    unique_id: unique_id.clone(),
                    fqn: vec![],
                    description: Some(group.description.unwrap_or_default()),
                    patch_path: None,
                    checksum: Default::default(),
                    language: None,
                    raw_code: None,
                    tags: vec![],
                    classifiers: Default::default(),
                    meta: IndexMap::new(),
                },
                __base_attr__: NodeBaseAttributes {
                    adapter: adapter_type,
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
                    depends_on: NodeDependsOn::default(),
                    quoting_ignore_case: false,
                    refs: vec![],
                    sources: vec![],
                    functions: vec![],
                    metrics: vec![],
                    unrendered_config: Default::default(),
                },
                __group_attr__: DbtGroupAttr { owner: group.owner },
            }),
        );
    }

    // Process functions from the separate manifest.functions field
    for (unique_id, function) in manifest.functions {
        nodes.functions.insert(
            unique_id,
            Arc::new(manifest_function_to_dbt_function(
                function,
                dbt_quoting,
                adapter_type,
            )),
        );
    }

    for (unique_id, macro_node) in manifest.macros {
        nodes.macros.insert(unique_id, Arc::new(macro_node.into()));
    }

    nodes
}

/// Convert a ManifestModel to a DbtModel.
/// Inverse of From<DbtModel> for ManifestModel.
pub fn manifest_model_to_dbt_model(
    model: ManifestModel,
    manifest: &DbtManifest,
    dbt_quoting: DbtQuoting,
) -> DbtModel {
    // A manifest records no per-node adapter, so a loaded node runs where the run
    // that produced the manifest ran.
    let adapter_type =
        AdapterType::from_str(&manifest.metadata.adapter_type).unwrap_or_else(|_| {
            panic!(
                "Invalid adapter_type in manifest {}",
                &manifest.metadata.adapter_type
            )
        });
    let database = model.__common_attr__.database;
    let schema = model.__common_attr__.schema;
    let alias = model.__base_attr__.alias;
    let relation_name = model.__base_attr__.relation_name;

    let node_relation = NodeRelation {
        database: Some(database.clone()),
        schema_name: schema.clone(),
        alias: alias.clone(),
        relation_name: relation_name.clone(),
    };

    let time_spine = model.time_spine.map(|ts| TimeSpine {
        node_relation,
        primary_column: TimeSpinePrimaryColumn {
            name: ts.standard_granularity_column,
            time_granularity: Default::default(), // TODO: hydrate time_granularity by looking up the column's granularity, not sure if available in manifest.
        },
        custom_granularities: ts.custom_granularities.unwrap_or_default(),
    });

    let recalculated_checksum = match model.__base_attr__.raw_code.clone() {
        Some(raw_code) => {
            let normalized_raw_code = normalize_sql(&raw_code);
            recalculate_checksum(
                Some(normalized_raw_code.as_str()),
                model.__base_attr__.checksum.clone(),
            )
        }
        None => model.__base_attr__.checksum.clone(),
    };

    DbtModel {
        __common_attr__: CommonAttributes {
            unique_id: model.__common_attr__.unique_id,
            name: model.__common_attr__.name,
            package_name: model.__common_attr__.package_name,
            path: model.__common_attr__.path,
            name_span: Span::default(),
            original_file_path: model.__common_attr__.original_file_path,
            patch_path: model.__common_attr__.patch_path,
            fqn: model.__common_attr__.fqn,
            description: model.__common_attr__.description,
            raw_code: model.__base_attr__.raw_code,
            checksum: recalculated_checksum,
            language: model.__base_attr__.language,
            tags: model.config.tags.clone().map(Vec::from).unwrap_or_default(),
            classifiers: model
                .config
                .classifiers
                .clone()
                .map(Into::into)
                .unwrap_or_default(),
            meta: model.config.meta.clone().unwrap_or_default(),
        },
        __base_attr__: NodeBaseAttributes {
            adapter: adapter_type,
            propagate: Vec::new(),
            database,
            schema,
            alias,
            relation_name,
            materialized: model
                .config
                .materialized
                .clone()
                .unwrap_or_else(ModelConfig::default_materialized),
            static_analysis: Default::default(),
            static_analysis_off_reason: None,
            compute: model.config.compute,
            enabled: model.config.enabled.unwrap_or(true),
            extended_model: false,
            quoting: {
                let mut quoting = model.config.quoting.unwrap_or_default();
                quoting.default_to(&dbt_quoting);
                quoting.try_into().expect("DbtQuoting should be set")
            },
            quoting_ignore_case: false,
            persist_docs: model.config.persist_docs.clone(),
            columns: model.__base_attr__.columns,
            depends_on: model.__base_attr__.depends_on,
            refs: model.__base_attr__.refs,
            sources: model.__base_attr__.sources,
            functions: model.__base_attr__.functions,
            metrics: model.__base_attr__.metrics,
            // See `nodes_from_dbt_manifest`'s `canonicalize_unrendered_config` for why: this
            // model's own `adapter_type` (just computed above) is the spelling authority for the
            // previous manifest's alias-authored keys.
            unrendered_config:
                dbt_adapter_core::config_aliases::canonicalize_previous_manifest_config_keys(
                    adapter_type,
                    model.__base_attr__.unrendered_config,
                ),
        },
        __model_attr__: DbtModelAttr {
            // `config.access` first, node-level `access` as a fallback: dbt-core mirrors the two
            // on write but lets the config key win, and `same_ref_representation` compares this
            // field against its `self.access` — the node attribute, which a manifest may carry
            // alone.
            access: model
                .config
                .access
                .clone()
                .or_else(|| model.access.clone())
                .unwrap_or_default(),
            group: model.config.group.clone(),
            contract: model.config.contract.clone(),
            incremental_strategy: model.config.incremental_strategy.clone(),
            freshness: model.config.freshness.clone(),
            state: model.config.state.clone(),
            introspection: IntrospectionKind::None,
            version: model.version,
            latest_version: model.latest_version,
            constraints: model.constraints.unwrap_or_default(),
            deprecation_date: model.deprecation_date,
            primary_key: model.primary_key.unwrap_or_default(),
            time_spine,
            event_time: model.config.event_time.clone(),
            catalog_name: model.config.catalog_name.clone(),
            table_format: model.config.table_format.clone(),
            sync: model.config.sync.clone(),
            compiled_code: None,
        },
        __adapter_attr__: AdapterAttr::from_config_and_dialect(
            &model.config.__warehouse_specific_config__,
            AdapterType::from_str(&manifest.metadata.adapter_type)
                .expect("Unknown or unsupported adapter type"),
        ),
        deprecated_config: model.config.into(),
        __other__: model.__other__,
    }
}

/// Convert a ManifestFunction to a DbtFunction.
/// Inverse of From<DbtFunction> for ManifestFunction.
pub fn manifest_function_to_dbt_function(
    function: ManifestFunction,
    dbt_quoting: DbtQuoting,
    adapter_type: AdapterType,
) -> DbtFunction {
    let recalculated_checksum = match function.__base_attr__.raw_code.clone() {
        Some(raw_code) => {
            // Recalculate checksum that eliminates whitespace and case differences.
            let normalized_raw_code = normalize_sql(&raw_code);
            recalculate_checksum(
                Some(normalized_raw_code.as_str()),
                function.__base_attr__.checksum.clone(),
            )
        }
        None => function.__base_attr__.checksum.clone(),
    };

    DbtFunction {
        __common_attr__: CommonAttributes {
            unique_id: function.__common_attr__.unique_id,
            name: function.__common_attr__.name,
            package_name: function.__common_attr__.package_name,
            path: function.__common_attr__.path,
            name_span: Span::default(),
            original_file_path: function.__common_attr__.original_file_path,
            patch_path: function.__common_attr__.patch_path,
            fqn: function.__common_attr__.fqn,
            description: function.__common_attr__.description,
            raw_code: function.__base_attr__.raw_code,
            checksum: recalculated_checksum,
            language: function.language.clone(),
            tags: function
                .config
                .tags
                .inner()
                .clone()
                .map(Into::into)
                .unwrap_or_default(),
            classifiers: Default::default(),
            meta: function.config.meta.clone().unwrap_or_default(),
        },
        __base_attr__: NodeBaseAttributes {
            adapter: adapter_type,
            propagate: Vec::new(),
            database: function.__common_attr__.database,
            schema: function.__common_attr__.schema,
            alias: function.__base_attr__.alias,
            relation_name: function.__base_attr__.relation_name,
            materialized: DbtMaterialization::Function,
            static_analysis: Default::default(),
            static_analysis_off_reason: None,
            compute: None,
            quoting: function
                .config
                .quoting
                .map(|mut quoting| {
                    quoting.default_to(&dbt_quoting);
                    quoting
                })
                .unwrap_or(dbt_quoting)
                .try_into()
                .expect("DbtQuoting should be set"),
            quoting_ignore_case: false,
            enabled: function.config.enabled.unwrap_or(true),
            extended_model: false,
            persist_docs: None,
            columns: function.__base_attr__.columns,
            depends_on: function.__base_attr__.depends_on,
            refs: function.__base_attr__.refs,
            sources: function.__base_attr__.sources,
            functions: function.__base_attr__.functions,
            metrics: function.__base_attr__.metrics,
            // See `nodes_from_dbt_manifest`'s `canonicalize_unrendered_config` for why.
            unrendered_config:
                dbt_adapter_core::config_aliases::canonicalize_previous_manifest_config_keys(
                    adapter_type,
                    function.__base_attr__.unrendered_config,
                ),
        },
        __function_attr__: DbtFunctionAttr {
            access: function.access,
            group: function.group,
            language: function.language,
            on_configuration_change: function.on_configuration_change,
            returns: function.returns,
            arguments: function.arguments,
            overloads: function.overloads,
        },
        deprecated_config: function.config,
        __other__: function.__other__,
    }
}

/// Recalculate checksum for a snapshot/model based on normalized raw code.
///
/// Callers normalize `raw_code` the same way for SQL and Python models
/// (matching dbt-core 1.12+), so deferred-manifest checksums are comparable
/// regardless of which dbt-core/Fusion version produced the manifest.
///
/// If the normalized code is missing, use the original checksum.
/// If the normalized code is the legacy `--placeholder--` sentinel (older Fusion
/// versions serialized this instead of the verbatim body, e.g. in deferred/
/// previous-state manifests), use the original stored checksum rather than hashing
/// the sentinel — otherwise such nodes are always flagged `state:modified`.
/// Otherwise, hash the normalized code.
pub fn recalculate_checksum(
    normalized_raw_code: Option<&str>,
    original_checksum: DbtChecksum,
) -> DbtChecksum {
    match normalized_raw_code {
        Some("--placeholder--") => original_checksum,
        Some(code) => DbtChecksum::hash(code.as_bytes()),
        None => original_checksum,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::manifest::ManifestModelConfig;
    use crate::schemas::manifest::operation::DbtOperation;
    use crate::schemas::{CommonAttributes, Nodes};
    use crate::state::Operations;
    use dbt_yaml::Spanned;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn create_test_nodes() -> Nodes {
        Nodes {
            models: BTreeMap::new(),
            tests: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            analyses: BTreeMap::new(),
            seeds: BTreeMap::new(),
            exposures: BTreeMap::new(),
            sources: BTreeMap::new(),
            unit_tests: BTreeMap::new(),
            semantic_models: BTreeMap::new(),
            metrics: BTreeMap::new(),
            saved_queries: BTreeMap::new(),
            groups: BTreeMap::new(),
            functions: BTreeMap::new(),
            checks: BTreeMap::new(),
            macros: BTreeMap::new(),
            project_name: None,
        }
    }

    fn create_test_model(id: &str, depends_on: Vec<String>) -> Arc<DbtModel> {
        Arc::new(DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: id.to_string(),
                name: id.split('.').next_back().unwrap_or(id).to_string(),
                package_name: "test".to_string(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                adapter: AdapterType::Snowflake,
                database: "db".to_string(),
                schema: "schema".to_string(),
                depends_on: NodeDependsOn {
                    nodes: depends_on,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        })
    }

    fn create_test_operation(id: &str, depends_on: Vec<String>) -> Spanned<DbtOperation> {
        Spanned::new(DbtOperation {
            __common_attr__: CommonAttributes {
                unique_id: id.to_string(),
                name: id.split('.').next_back().unwrap_or(id).to_string(),
                package_name: "test".to_string(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                depends_on: NodeDependsOn {
                    nodes: depends_on,
                    ..Default::default()
                },
                ..Default::default()
            },
            __other__: BTreeMap::new(),
        })
    }

    /// dbt-core's `ModelNode.same_ref_representation` (dbt-mantle
    /// `core/dbt/contracts/graph/nodes.py:684-691`) compares `self.access` — the NODE attribute —
    /// so a manifest carrying only that one must not read back as `Protected`, which
    /// `DbtModel::same_ref_representation` would report as modified against an unchanged project.
    #[test]
    fn manifest_model_access_falls_back_to_node_attribute() {
        let quoting = DbtQuoting {
            database: Some(false),
            identifier: Some(false),
            schema: Some(false),
            snowflake_ignore_case: Some(false),
        };
        let manifest = DbtManifest {
            metadata: ManifestMetadata {
                adapter_type: "snowflake".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let to_dbt_model = |model: ManifestModel| {
            manifest_model_to_dbt_model(model, &manifest, quoting)
                .__model_attr__
                .access
        };

        // Node-level only: the fallback carries it through.
        let node_only = ManifestModel {
            access: Some(Access::Public),
            ..Default::default()
        };
        assert_eq!(to_dbt_model(node_only), Access::Public);

        // Both present: `config.access` wins, matching dbt-core, where
        // `update_parsed_node_config` overwrites `node.access` from the config dict.
        let both = ManifestModel {
            access: Some(Access::Public),
            config: ManifestModelConfig {
                access: Some(Access::Private),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(to_dbt_model(both), Access::Private);

        // Neither: dbt-core's `protected` default.
        assert_eq!(to_dbt_model(ManifestModel::default()), Access::Protected);
    }

    /// A previous manifest is data this run did not write, so a node carrying both an alias and
    /// its canonical spelling in `unrendered_config` (e.g. a hand-edited or foreign manifest) must
    /// merge to exactly one key on read, deterministically, rather than erroring the way parsing a
    /// project we control does (`canonicalize_config_keys`'s `DuplicateAliasKey`).
    #[test]
    fn manifest_model_unrendered_config_merges_duplicate_alias_deterministically() {
        let quoting = DbtQuoting {
            database: Some(false),
            identifier: Some(false),
            schema: Some(false),
            snowflake_ignore_case: Some(false),
        };
        let manifest = DbtManifest {
            metadata: ManifestMetadata {
                adapter_type: "databricks".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut unrendered_config = BTreeMap::new();
        unrendered_config.insert(
            "catalog".to_string(),
            YmlValue::string("alias_value".to_string()),
        );
        unrendered_config.insert(
            "database".to_string(),
            YmlValue::string("canonical_value".to_string()),
        );
        let model = ManifestModel {
            __base_attr__: crate::schemas::manifest::manifest_nodes::ManifestNodeBaseAttributes {
                unrendered_config,
                ..Default::default()
            },
            ..Default::default()
        };

        let dbt_model = manifest_model_to_dbt_model(model, &manifest, quoting);

        assert_eq!(dbt_model.__base_attr__.unrendered_config.len(), 1);
        assert_eq!(
            dbt_model.__base_attr__.unrendered_config.get("database"),
            Some(&YmlValue::string("canonical_value".to_string()))
        );
    }

    #[test]
    fn test_build_parent_and_child_maps_empty_nodes() {
        let nodes = create_test_nodes();
        let operations = Operations::default();
        let (parent_map, child_map) = build_parent_and_child_maps(&nodes, &operations);

        assert!(parent_map.is_empty());
        assert!(child_map.is_empty());
    }

    /// Regression for fs#10382: every iterated node gets a key in BOTH maps,
    /// even when it has zero parents AND zero children. A single leaf model
    /// must produce `child_map = {id: []}`, not an empty `child_map`.
    #[test]
    fn test_build_parent_and_child_maps_single_model_no_deps() {
        let mut nodes = create_test_nodes();
        nodes.models.insert(
            "model.test.model_a".to_string(),
            create_test_model("model.test.model_a", vec![]),
        );

        let operations = Operations::default();
        let (parent_map, child_map) = build_parent_and_child_maps(&nodes, &operations);

        assert_eq!(parent_map.len(), 1);
        assert_eq!(parent_map.get("model.test.model_a").unwrap().len(), 0);

        // child_map must contain the model with an empty list (was empty pre-fix).
        assert_eq!(child_map.len(), 1);
        assert_eq!(child_map.get("model.test.model_a").unwrap().len(), 0);
    }

    #[test]
    fn test_build_parent_and_child_maps_simple_dependency() {
        let mut nodes = create_test_nodes();

        nodes.models.insert(
            "model.test.model_a".to_string(),
            create_test_model("model.test.model_a", vec![]),
        );
        nodes.models.insert(
            "model.test.model_b".to_string(),
            create_test_model("model.test.model_b", vec!["model.test.model_a".to_string()]),
        );

        let operations = Operations::default();
        let (parent_map, child_map) = build_parent_and_child_maps(&nodes, &operations);

        // Check parent_map
        assert_eq!(parent_map.len(), 2);
        assert_eq!(parent_map.get("model.test.model_a").unwrap().len(), 0);
        assert_eq!(
            parent_map.get("model.test.model_b").unwrap(),
            &vec!["model.test.model_a".to_string()]
        );

        // child_map: model_a -> [model_b], plus model_b -> [] (leaf invariant).
        assert_eq!(child_map.len(), 2);
        assert_eq!(
            child_map.get("model.test.model_a").unwrap(),
            &vec!["model.test.model_b".to_string()]
        );
        assert_eq!(child_map.get("model.test.model_b").unwrap().len(), 0);
    }

    #[test]
    fn test_build_parent_and_child_maps_multiple_dependencies() {
        let mut nodes = create_test_nodes();

        nodes.models.insert(
            "model.test.model_a".to_string(),
            create_test_model("model.test.model_a", vec![]),
        );
        nodes.models.insert(
            "model.test.model_b".to_string(),
            create_test_model("model.test.model_b", vec![]),
        );
        nodes.models.insert(
            "model.test.model_c".to_string(),
            create_test_model(
                "model.test.model_c",
                vec![
                    "model.test.model_a".to_string(),
                    "model.test.model_b".to_string(),
                ],
            ),
        );

        let operations = Operations::default();
        let (parent_map, child_map) = build_parent_and_child_maps(&nodes, &operations);

        // Check parent_map
        assert_eq!(parent_map.len(), 3);
        assert_eq!(parent_map.get("model.test.model_a").unwrap().len(), 0);
        assert_eq!(parent_map.get("model.test.model_b").unwrap().len(), 0);
        assert_eq!(
            parent_map.get("model.test.model_c").unwrap(),
            &vec![
                "model.test.model_a".to_string(),
                "model.test.model_b".to_string()
            ]
        );

        // child_map: every node now appears; the leaf model_c has an empty list.
        assert_eq!(child_map.len(), 3);
        assert_eq!(
            child_map.get("model.test.model_a").unwrap(),
            &vec!["model.test.model_c".to_string()]
        );
        assert_eq!(
            child_map.get("model.test.model_b").unwrap(),
            &vec!["model.test.model_c".to_string()]
        );
        assert_eq!(child_map.get("model.test.model_c").unwrap().len(), 0);
    }

    #[test]
    fn test_build_parent_and_child_maps_chain_dependency() {
        let mut nodes = create_test_nodes();

        nodes.models.insert(
            "model.test.model_a".to_string(),
            create_test_model("model.test.model_a", vec![]),
        );
        nodes.models.insert(
            "model.test.model_b".to_string(),
            create_test_model("model.test.model_b", vec!["model.test.model_a".to_string()]),
        );
        nodes.models.insert(
            "model.test.model_c".to_string(),
            create_test_model("model.test.model_c", vec!["model.test.model_b".to_string()]),
        );

        let operations = Operations::default();
        let (parent_map, child_map) = build_parent_and_child_maps(&nodes, &operations);

        // Check parent_map
        assert_eq!(parent_map.len(), 3);
        assert_eq!(parent_map.get("model.test.model_a").unwrap().len(), 0);
        assert_eq!(
            parent_map.get("model.test.model_b").unwrap(),
            &vec!["model.test.model_a".to_string()]
        );
        assert_eq!(
            parent_map.get("model.test.model_c").unwrap(),
            &vec!["model.test.model_b".to_string()]
        );

        // child_map: a -> [b], b -> [c], and the leaf c -> [].
        assert_eq!(child_map.len(), 3);
        assert_eq!(
            child_map.get("model.test.model_a").unwrap(),
            &vec!["model.test.model_b".to_string()]
        );
        assert_eq!(
            child_map.get("model.test.model_b").unwrap(),
            &vec!["model.test.model_c".to_string()]
        );
        assert_eq!(child_map.get("model.test.model_c").unwrap().len(), 0);
    }

    #[test]
    fn test_build_parent_and_child_maps_with_source() {
        let mut nodes = create_test_nodes();

        nodes.sources.insert(
            "source.test.my_source.table1".to_string(),
            Arc::new(DbtSource {
                __common_attr__: CommonAttributes {
                    unique_id: "source.test.my_source.table1".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        );

        nodes.models.insert(
            "model.test.model_a".to_string(),
            create_test_model(
                "model.test.model_a",
                vec!["source.test.my_source.table1".to_string()],
            ),
        );

        let operations = Operations::default();
        let (parent_map, child_map) = build_parent_and_child_maps(&nodes, &operations);

        // Check parent_map
        assert_eq!(parent_map.len(), 2);
        assert_eq!(
            parent_map.get("model.test.model_a").unwrap(),
            &vec!["source.test.my_source.table1".to_string()]
        );
        assert_eq!(
            parent_map
                .get("source.test.my_source.table1")
                .unwrap()
                .len(),
            0
        );

        // child_map: source -> [model_a]; the leaf model_a -> [].
        assert_eq!(child_map.len(), 2);
        assert_eq!(
            child_map.get("source.test.my_source.table1").unwrap(),
            &vec!["model.test.model_a".to_string()]
        );
        assert_eq!(child_map.get("model.test.model_a").unwrap().len(), 0);
    }

    #[test]
    fn test_build_parent_and_child_maps_missing_dependency() {
        let mut nodes = create_test_nodes();

        nodes.models.insert(
            "model.test.model_b".to_string(),
            create_test_model("model.test.model_b", vec!["model.test.model_a".to_string()]),
        );

        let operations = Operations::default();
        let (parent_map, child_map) = build_parent_and_child_maps(&nodes, &operations);

        // Both the existing model and the missing dependency should have entries
        assert_eq!(parent_map.len(), 2);
        assert_eq!(
            parent_map.get("model.test.model_b").unwrap(),
            &vec!["model.test.model_a".to_string()]
        );
        assert_eq!(parent_map.get("model.test.model_a").unwrap().len(), 0); // Missing node gets empty entry

        // child_map: a -> [b], plus the leaf b -> [].
        assert_eq!(child_map.len(), 2);
        assert_eq!(
            child_map.get("model.test.model_a").unwrap(),
            &vec!["model.test.model_b".to_string()]
        );
        assert_eq!(child_map.get("model.test.model_b").unwrap().len(), 0);
    }

    /// Regression for fs#10382: on_run_start / on_run_end hooks live on
    /// `ResolverState.operations`, not `ResolverState.nodes`, but their
    /// unique_ids surface in the manifest. They must appear in both maps.
    #[test]
    fn test_build_parent_and_child_maps_includes_operations() {
        let mut nodes = create_test_nodes();
        nodes.models.insert(
            "model.test.upstream".to_string(),
            create_test_model("model.test.upstream", vec![]),
        );

        let mut operations = Operations::default();
        operations.on_run_start.push(create_test_operation(
            "operation.test.hook-on-run-start-0",
            vec![],
        ));
        operations.on_run_end.push(create_test_operation(
            "operation.test.hook-on-run-end-0",
            vec!["model.test.upstream".to_string()],
        ));

        let (parent_map, child_map) = build_parent_and_child_maps(&nodes, &operations);

        // Both hooks appear in parent_map (the 3 CSV violations under "parent_map")
        assert!(parent_map.contains_key("operation.test.hook-on-run-start-0"));
        assert_eq!(
            parent_map
                .get("operation.test.hook-on-run-start-0")
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            parent_map.get("operation.test.hook-on-run-end-0").unwrap(),
            &vec!["model.test.upstream".to_string()]
        );

        // Both hooks appear in child_map too (no children, so empty lists).
        assert!(child_map.contains_key("operation.test.hook-on-run-start-0"));
        assert_eq!(
            child_map
                .get("operation.test.hook-on-run-start-0")
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            child_map
                .get("operation.test.hook-on-run-end-0")
                .unwrap()
                .len(),
            0
        );

        // The model the on_run_end hook depends on now sees the hook as a child.
        assert_eq!(
            child_map.get("model.test.upstream").unwrap(),
            &vec!["operation.test.hook-on-run-end-0".to_string()]
        );
    }

    /// Child lists must be deterministic and sorted, matching dbt-core's
    /// `_sort_values`. Insertion-order BTreeMap iteration alone is not enough
    /// because depends_on lists can carry arbitrary user-defined order.
    #[test]
    fn test_build_parent_and_child_maps_values_are_sorted() {
        let mut nodes = create_test_nodes();
        // Three children of one upstream model, inserted in non-sorted order.
        nodes.models.insert(
            "model.test.upstream".to_string(),
            create_test_model("model.test.upstream", vec![]),
        );
        nodes.models.insert(
            "model.test.z_child".to_string(),
            create_test_model(
                "model.test.z_child",
                vec!["model.test.upstream".to_string()],
            ),
        );
        nodes.models.insert(
            "model.test.a_child".to_string(),
            create_test_model(
                "model.test.a_child",
                vec!["model.test.upstream".to_string()],
            ),
        );
        nodes.models.insert(
            "model.test.m_child".to_string(),
            create_test_model(
                "model.test.m_child",
                vec!["model.test.upstream".to_string()],
            ),
        );

        let operations = Operations::default();
        let (_parent_map, child_map) = build_parent_and_child_maps(&nodes, &operations);

        // Children of upstream must come out alphabetically sorted.
        assert_eq!(
            child_map.get("model.test.upstream").unwrap(),
            &vec![
                "model.test.a_child".to_string(),
                "model.test.m_child".to_string(),
                "model.test.z_child".to_string(),
            ]
        );
    }
}
