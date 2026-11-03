use dbt_adapter_core::AdapterType;

use crate::schemas::serde::AdapterTypeOrArray;
use dbt_common::path::DbtPath;
use indexmap::IndexMap;
use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize, de::Deserializer};
use serde_with::skip_serializing_none;

use crate::schemas::common::ExternalTable;
use crate::schemas::serde::OmissibleGrantConfig;
use dbt_common::io_args::ComputeArg;
use dbt_common::serde_utils::Omissible;
use dbt_yaml::DbtSchema;
use dbt_yaml::Spanned;
use dbt_yaml::Verbatim;

use crate::schemas::common::DbtBatchSize;
use crate::schemas::common::DbtIncrementalStrategy;
use crate::schemas::common::DbtUniqueKey;
use crate::schemas::common::{DocsConfig, HardDeletes, OnConfigurationChange, OnError};
use crate::schemas::common::{HookConfig, Hooks, OnSchemaChange};
use crate::schemas::dbt_column::Granularity;
use crate::schemas::project::configs::common::WarehouseSpecificNodeConfig;
use crate::schemas::project::configs::model_config::LatestVersionPointer;
use crate::schemas::properties::{ModelFreshness, ModelState};

use crate::schemas::serde::{bool_or_string_bool, string_or_number_to_string};

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use crate::schemas::{
    AbsorbedOverload, CommonAttributes, DbtAnalysis, DbtCheck, DbtExposure, DbtFunction, DbtModel,
    DbtSeed, DbtSnapshot, DbtSource, DbtTest, DbtUnitTest, NodeBaseAttributes,
    common::{
        Access, DbtChecksum, DbtContract, DbtMaterialization, DbtQuoting, Expect,
        FreshnessDefinition, Given, IncludeExclude, NodeDependsOn, PersistDocsConfig, SyncConfig,
    },
    dbt_column::{DbtColumnRef, deserialize_dbt_columns, serialize_dbt_columns},
    macros::{DbtMacro, MacroArgument, MacroConfig, MacroDependsOn},
    manifest::{
        DbtMetric, DbtOperation, DbtSavedQuery, DbtSemanticModel,
        common::{DbtOwner, SourceFileMetadata, WhereFilterIntersection},
        metric::{
            DbtMetricAttr, MeasureAggregationParameters, MetricTypeParams, NonAdditiveDimension,
        },
        semantic_model::{
            DbtSemanticModelAttr, NodeRelation, SemanticEntity, SemanticMeasure,
            SemanticModelDefaults,
        },
    },
    nodes::{ExposureType, TestMetadata},
    project::{
        AnalysesConfig, CheckConfig, DataTestConfig, ExposureConfig, FunctionConfig, MetricConfig,
        ModelConfig, SavedQueryConfig, SeedConfig, SemanticModelConfig, SnapshotConfig,
        SnapshotMetaColumnNames, SourceConfig, UnitTestConfig,
    },
    properties::{
        ModelConstraint, UnitTestOverrides,
        metrics_properties::{AggregationType, MetricType},
        model_properties::ModelPropertiesTimeSpine,
    },
    ref_and_source::{
        DbtRef, DbtSourceWrapper, deserialize_dbt_function_refs, serialize_dbt_function_refs,
    },
    semantic_layer::semantic_manifest::SemanticLayerElementConfig,
    serde::{StringOrArrayOfStrings, StringOrInteger},
};

use dbt_common::io_args::{StaticAnalysisKind, StaticAnalysisOffReason};

fn default_analysis_materialized() -> DbtMaterialization {
    DbtMaterialization::Analysis
}

fn default_analysis_static_analysis() -> StaticAnalysisKind {
    StaticAnalysisKind::Off
}

fn default_analysis_enabled() -> bool {
    true
}

/// Common attributes for all manifest nodes, materializable or not.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestCommonAttributes {
    // Identifiers
    pub unique_id: String,
    pub name: String,
    pub package_name: String,
    pub fqn: Vec<String>,

    // Paths
    pub path: DbtPath,
    pub original_file_path: DbtPath,

    // Meta
    pub description: Option<String>,

    #[serde(default)]
    pub tags: Vec<String>,

    #[serde(default)]
    pub classifiers: Vec<String>,

    #[serde(default)]
    pub meta: IndexMap<String, YmlValue>,
}

/// Common attributes for materializable nodes, i.e. models, sources, snapshots, tests, etc.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestMaterializableCommonAttributes {
    // Identifiers
    pub unique_id: String,
    #[serde(default)]
    pub database: String,
    pub schema: String,
    pub name: String,
    pub package_name: String,
    pub fqn: Vec<String>,

    // Paths
    pub path: DbtPath,
    pub original_file_path: DbtPath,
    pub patch_path: Option<DbtPath>,

    // Meta
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub classifiers: Vec<String>,
    #[serde(default)]
    pub meta: IndexMap<String, YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestNodeBaseAttributes {
    // Identifiers
    #[serde(default)]
    pub alias: String,
    pub relation_name: Option<String>,
    /// Marker for Databricks canonical relation rendering. Legacy manifests omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_relation_adapter: Option<AdapterType>,

    // Paths
    pub compiled_path: Option<String>,
    pub build_path: Option<String>,

    // Derived
    #[serde(
        default,
        serialize_with = "serialize_dbt_columns",
        deserialize_with = "deserialize_dbt_columns"
    )]
    pub columns: Vec<DbtColumnRef>,
    #[serde(default)]
    pub depends_on: NodeDependsOn,
    #[serde(default)]
    pub refs: Vec<DbtRef>,
    #[serde(default)]
    pub sources: Vec<DbtSourceWrapper>,
    #[serde(
        default,
        serialize_with = "serialize_dbt_function_refs",
        deserialize_with = "deserialize_dbt_function_refs"
    )]
    pub functions: Vec<DbtRef>,

    // Code
    pub raw_code: Option<String>,
    pub compiled: Option<bool>,
    pub compiled_code: Option<String>,
    #[serde(default)]
    pub unrendered_config: BTreeMap<String, YmlValue>,

    // Static analysis
    #[serde(skip_deserializing, default)]
    pub static_analysis_off_reason: Option<StaticAnalysisOffReason>,

    // Metadata
    pub doc_blocks: Option<Vec<YmlValue>>,
    pub extra_ctes_injected: Option<bool>,
    pub extra_ctes: Option<Vec<YmlValue>>,
    #[serde(default)]
    pub metrics: Vec<Vec<String>>,
    pub checksum: DbtChecksum,
    pub language: Option<String>,
    #[serde(default)]
    pub contract: DbtContract,
    pub created_at: Option<f64>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManifestSeed {
    pub __common_attr__: ManifestMaterializableCommonAttributes,

    pub __base_attr__: ManifestNodeBaseAttributes,

    // Test Specific Attributes
    #[serde(default)]
    pub config: ManifestSeedConfig,
    pub root_path: Option<PathBuf>,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtSeed> for ManifestSeed {
    fn from(seed: DbtSeed) -> Self {
        Self {
            __common_attr__: ManifestMaterializableCommonAttributes {
                unique_id: seed.__common_attr__.unique_id,
                database: seed.__base_attr__.database,
                schema: seed.__base_attr__.schema,
                name: seed.__common_attr__.name,
                package_name: seed.__common_attr__.package_name,
                fqn: seed.__common_attr__.fqn,
                path: seed.__common_attr__.path,
                original_file_path: seed.__common_attr__.original_file_path,
                patch_path: seed.__common_attr__.patch_path,
                description: seed.__common_attr__.description,
                tags: seed.__common_attr__.tags,
                classifiers: seed.__common_attr__.classifiers,
                meta: seed.__common_attr__.meta,
            },
            __base_attr__: ManifestNodeBaseAttributes {
                alias: seed.__base_attr__.alias,
                relation_name: seed.__base_attr__.relation_name,
                canonical_relation_adapter: (seed.__base_attr__.effective_propagation_target
                    == Some(AdapterType::Databricks))
                .then_some(AdapterType::Databricks),
                columns: seed.__base_attr__.columns,
                depends_on: seed.__base_attr__.depends_on,
                refs: seed.__base_attr__.refs,
                sources: seed.__base_attr__.sources,
                functions: seed.__base_attr__.functions,
                metrics: seed.__base_attr__.metrics,
                raw_code: seed.__common_attr__.raw_code,
                compiled: None,
                compiled_code: None,
                checksum: seed.__common_attr__.checksum,
                language: seed.__common_attr__.language,
                unrendered_config: seed.__base_attr__.unrendered_config,
                doc_blocks: Default::default(),
                extra_ctes_injected: Default::default(),
                extra_ctes: Default::default(),
                created_at: Default::default(),
                compiled_path: Default::default(),
                build_path: Default::default(),
                contract: Default::default(),
                static_analysis_off_reason: seed.__base_attr__.static_analysis_off_reason,
            },
            config: seed.deprecated_config.into(),
            root_path: seed.__seed_attr__.root_path,
            __other__: seed.__other__,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManifestUnitTest {
    pub __common_attr__: ManifestMaterializableCommonAttributes,

    pub __base_attr__: ManifestNodeBaseAttributes,
    /// Unit Test Specific Attributes
    #[serde(default)]
    pub config: UnitTestConfig,
    pub model: String,
    pub given: Vec<Given>,
    pub expect: Expect,
    pub versions: Option<IncludeExclude>,
    pub version: Option<StringOrInteger>,
    pub overrides: Option<UnitTestOverrides>,
    #[serde(rename = "_event_status")]
    pub field_event_status: Option<BTreeMap<String, YmlValue>>,
    #[serde(rename = "_pre_injected_sql")]
    pub field_pre_injected_sql: Option<String>,
    pub tested_node_unique_id: Option<String>,
    pub this_input_node_unique_id: Option<String>,
}

impl From<DbtUnitTest> for ManifestUnitTest {
    fn from(unit_test: DbtUnitTest) -> Self {
        Self {
            __common_attr__: ManifestMaterializableCommonAttributes {
                unique_id: unit_test.__common_attr__.unique_id,
                database: unit_test.__base_attr__.database,
                schema: unit_test.__base_attr__.schema,
                name: unit_test.__common_attr__.name,
                package_name: unit_test.__common_attr__.package_name,
                fqn: unit_test.__common_attr__.fqn,
                path: unit_test.__common_attr__.path,
                original_file_path: unit_test.__common_attr__.original_file_path,
                patch_path: unit_test.__common_attr__.patch_path,
                description: unit_test.__common_attr__.description,
                tags: unit_test.__common_attr__.tags,
                classifiers: unit_test.__common_attr__.classifiers,
                meta: unit_test.__common_attr__.meta,
            },
            __base_attr__: ManifestNodeBaseAttributes {
                alias: unit_test.__base_attr__.alias,
                relation_name: unit_test.__base_attr__.relation_name,
                canonical_relation_adapter: None,
                columns: unit_test.__base_attr__.columns,
                depends_on: unit_test.__base_attr__.depends_on,
                refs: unit_test.__base_attr__.refs,
                sources: unit_test.__base_attr__.sources,
                functions: unit_test.__base_attr__.functions,
                metrics: unit_test.__base_attr__.metrics,
                raw_code: unit_test.__common_attr__.raw_code,
                compiled: None,
                compiled_code: None,
                checksum: unit_test.__common_attr__.checksum,
                language: unit_test.__common_attr__.language,
                unrendered_config: Default::default(),
                doc_blocks: Default::default(),
                extra_ctes_injected: Default::default(),
                extra_ctes: Default::default(),
                created_at: Default::default(),
                compiled_path: Default::default(),
                build_path: Default::default(),
                contract: Default::default(),
                static_analysis_off_reason: unit_test.__base_attr__.static_analysis_off_reason,
            },
            config: unit_test.deprecated_config,
            model: unit_test.__unit_test_attr__.model,
            given: unit_test.__unit_test_attr__.given,
            expect: unit_test.__unit_test_attr__.expect,
            versions: unit_test.__unit_test_attr__.versions,
            version: unit_test.__unit_test_attr__.version,
            overrides: unit_test.__unit_test_attr__.overrides,
            field_event_status: unit_test.field_event_status,
            field_pre_injected_sql: unit_test.field_pre_injected_sql,
            tested_node_unique_id: unit_test.tested_node_unique_id,
            this_input_node_unique_id: unit_test.this_input_node_unique_id,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestDataTest {
    pub __common_attr__: ManifestMaterializableCommonAttributes,
    pub __base_attr__: ManifestNodeBaseAttributes,

    /// Test Specific Attributes
    #[serde(default)]
    pub config: DataTestConfig,
    pub column_name: Option<String>,
    pub attached_node: Option<String>,
    pub test_metadata: Option<TestMetadata>,
    pub file_key_name: Option<String>,
    pub group: Option<String>,
    pub generated_sql_file: Option<String>,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtTest> for ManifestDataTest {
    fn from(test: DbtTest) -> Self {
        Self {
            __common_attr__: ManifestMaterializableCommonAttributes {
                unique_id: test.__common_attr__.unique_id,
                database: test.__base_attr__.database,
                schema: test.__base_attr__.schema,
                name: test.__common_attr__.name,
                package_name: test.__common_attr__.package_name,
                fqn: test.__common_attr__.fqn,
                path: test.__common_attr__.path,

                // NOTE: `test.__common_attr__.original_file_path` is the path
                // to the generated SQL file, which is *not* what we want here
                // -- in the manifest, `original_file_path` should be the path
                // to the YAML file where the test was defined
                original_file_path: test.manifest_original_file_path,

                patch_path: test.__common_attr__.patch_path,

                description: test.__common_attr__.description,
                tags: test.__common_attr__.tags,
                classifiers: test.__common_attr__.classifiers,
                meta: test.__common_attr__.meta,
            },
            __base_attr__: ManifestNodeBaseAttributes {
                alias: test.__base_attr__.alias,
                relation_name: test.__base_attr__.relation_name,
                canonical_relation_adapter: None,
                columns: test.__base_attr__.columns,
                depends_on: test.__base_attr__.depends_on,
                refs: test.__base_attr__.refs,
                sources: test.__base_attr__.sources,
                functions: test.__base_attr__.functions,
                metrics: test.__base_attr__.metrics,
                raw_code: test.__common_attr__.raw_code,
                compiled: None,
                compiled_code: None,
                checksum: test.__common_attr__.checksum,
                language: test.__common_attr__.language,
                unrendered_config: test.__base_attr__.unrendered_config,
                doc_blocks: Default::default(),
                extra_ctes_injected: Default::default(),
                extra_ctes: Default::default(),
                created_at: Default::default(),
                compiled_path: Default::default(),
                build_path: Default::default(),
                contract: Default::default(),
                static_analysis_off_reason: test.__base_attr__.static_analysis_off_reason,
            },
            config: test.deprecated_config,
            column_name: test.__test_attr__.column_name,
            attached_node: test.__test_attr__.attached_node,
            test_metadata: test.__test_attr__.test_metadata,
            file_key_name: test.__test_attr__.file_key_name,
            group: test.__test_attr__.group,
            generated_sql_file: Some(
                test.__common_attr__
                    .original_file_path
                    .to_string_lossy()
                    .to_string(),
            ),
            __other__: test.__other__,
        }
    }
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
pub struct ManifestSnapshotConfig {
    #[serde(default)]
    pub compute: Option<ComputeArg>,
    #[serde(alias = "project", alias = "data_space")]
    pub database: Option<String>,
    #[serde(alias = "dataset")]
    pub schema: Option<String>,
    pub alias: Option<String>,
    pub materialized: Option<DbtMaterialization>,
    pub strategy: Option<String>,
    pub unique_key: Option<StringOrArrayOfStrings>,
    pub check_cols: Option<StringOrArrayOfStrings>,
    pub updated_at: Option<String>,
    pub dbt_valid_to_current: Option<String>,
    pub snapshot_meta_column_names: Option<SnapshotMetaColumnNames>,
    pub hard_deletes: Option<HardDeletes>,
    pub target_database: Option<String>,
    pub target_schema: Option<String>,
    // Internal-only placement hint; never written to the manifest.
    #[serde(skip_serializing, default)]
    pub adapter: Option<AdapterType>,
    // Internal-only placement hint; never written to the manifest.
    #[serde(skip_serializing, default)]
    pub propagate: Option<AdapterTypeOrArray>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub full_refresh: Option<bool>,
    #[serde(
        default,
        serialize_with = "crate::schemas::nodes::serialize_none_as_empty_list"
    )]
    pub tags: Option<StringOrArrayOfStrings>,
    #[serde(rename = "pre-hook", default)]
    pub pre_hook: Vec<HookConfig>,
    #[serde(rename = "post-hook", default)]
    pub post_hook: Vec<HookConfig>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default"
    )]
    pub persist_docs: Option<PersistDocsConfig>,
    #[serde(default)]
    pub grants: OmissibleGrantConfig,
    pub event_time: Option<String>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default"
    )]
    pub quoting: Option<DbtQuoting>,
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    #[serde(
        default,
        deserialize_with = "crate::schemas::serde::default_type",
        serialize_with = "crate::schemas::serde::serialize_none_as_empty_map"
    )]
    pub meta: Option<IndexMap<String, YmlValue>>,
    pub group: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub quote_columns: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub invalidate_hard_deletes: Option<bool>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_option_docs_with_nulls"
    )]
    pub docs: Option<DocsConfig>,
    pub sync: Option<SyncConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ModelState>,
    pub __warehouse_specific_config__: WarehouseSpecificNodeConfig,
}

impl From<SnapshotConfig> for ManifestSnapshotConfig {
    fn from(config: SnapshotConfig) -> Self {
        Self {
            compute: config.compute,
            database: config.database,
            schema: config.schema,
            alias: config.alias,
            materialized: config.materialized,
            strategy: config.strategy,
            unique_key: config.unique_key,
            check_cols: config.check_cols,
            updated_at: config.updated_at,
            dbt_valid_to_current: config.dbt_valid_to_current,
            snapshot_meta_column_names: config.snapshot_meta_column_names,
            hard_deletes: config.hard_deletes,
            target_database: config.target_database,
            target_schema: config.target_schema,
            adapter: config.adapter,
            propagate: config.propagate,
            enabled: config.enabled,
            full_refresh: config.full_refresh,
            tags: config.tags.into_inner(),
            pre_hook: (*config.pre_hook)
                .as_ref()
                .map(|h| h.to_hook_config_array())
                .unwrap_or_default(),
            post_hook: (*config.post_hook)
                .as_ref()
                .map(|h| h.to_hook_config_array())
                .unwrap_or_default(),
            persist_docs: config.persist_docs,
            grants: config.grants,
            event_time: config.event_time,
            quoting: config.quoting,
            static_analysis: config.static_analysis,
            meta: config.meta,
            group: config.group,
            quote_columns: config.quote_columns,
            invalidate_hard_deletes: config.invalidate_hard_deletes,
            docs: config.docs,
            sync: config.sync,
            state: config.state,
            __warehouse_specific_config__: config.__warehouse_specific_config__,
        }
    }
}

impl From<ManifestSnapshotConfig> for SnapshotConfig {
    fn from(config: ManifestSnapshotConfig) -> Self {
        Self {
            compute: config.compute,
            database: config.database,
            schema: config.schema,
            alias: config.alias,
            materialized: config.materialized,
            strategy: config.strategy,
            unique_key: config.unique_key,
            check_cols: config.check_cols,
            updated_at: config.updated_at,
            dbt_valid_to_current: config.dbt_valid_to_current,
            snapshot_meta_column_names: config.snapshot_meta_column_names,
            hard_deletes: config.hard_deletes,
            target_database: config.target_database,
            target_schema: config.target_schema,
            adapter: config.adapter,
            propagate: config.propagate,
            enabled: config.enabled,
            full_refresh: config.full_refresh,
            tags: crate::schemas::project::configs::config_merge::Tags(config.tags),
            pre_hook: Verbatim::from(if config.pre_hook.is_empty() {
                None
            } else {
                Some(Hooks::HookConfigArray(config.pre_hook))
            }),
            post_hook: Verbatim::from(if config.post_hook.is_empty() {
                None
            } else {
                Some(Hooks::HookConfigArray(config.post_hook))
            }),
            persist_docs: config.persist_docs,
            grants: config.grants,
            event_time: config.event_time,
            quoting: config.quoting,
            static_analysis: config.static_analysis,
            meta: config.meta,
            group: config.group,
            quote_columns: config.quote_columns,
            invalidate_hard_deletes: config.invalidate_hard_deletes,
            docs: config.docs,
            sync: config.sync,
            state: config.state,
            __warehouse_specific_config__: config.__warehouse_specific_config__,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestSnapshot {
    pub __common_attr__: ManifestMaterializableCommonAttributes,
    pub __base_attr__: ManifestNodeBaseAttributes,

    /// Snapshot Specific Attributes
    #[serde(default)]
    pub config: ManifestSnapshotConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtSnapshot> for ManifestSnapshot {
    fn from(snapshot: DbtSnapshot) -> Self {
        Self {
            __common_attr__: ManifestMaterializableCommonAttributes {
                unique_id: snapshot.__common_attr__.unique_id,
                database: snapshot.__base_attr__.database,
                schema: snapshot.__base_attr__.schema,
                name: snapshot.__common_attr__.name,
                package_name: snapshot.__common_attr__.package_name,
                fqn: snapshot.__common_attr__.fqn,
                path: snapshot.__common_attr__.path,
                original_file_path: snapshot.__common_attr__.original_file_path,
                patch_path: snapshot.__common_attr__.patch_path,
                description: snapshot.__common_attr__.description,
                tags: snapshot.__common_attr__.tags,
                classifiers: snapshot.__common_attr__.classifiers,
                meta: snapshot.__common_attr__.meta,
            },
            __base_attr__: ManifestNodeBaseAttributes {
                alias: snapshot.__base_attr__.alias,
                relation_name: snapshot.__base_attr__.relation_name,
                canonical_relation_adapter: None,
                columns: snapshot.__base_attr__.columns,
                depends_on: snapshot.__base_attr__.depends_on,
                refs: snapshot.__base_attr__.refs,
                sources: snapshot.__base_attr__.sources,
                functions: snapshot.__base_attr__.functions,
                metrics: snapshot.__base_attr__.metrics,
                raw_code: snapshot.__common_attr__.raw_code,
                compiled: None,
                compiled_code: None,
                checksum: snapshot.__common_attr__.checksum,
                language: snapshot.__common_attr__.language,
                unrendered_config: snapshot.__base_attr__.unrendered_config,
                doc_blocks: Default::default(),
                extra_ctes_injected: Default::default(),
                extra_ctes: Default::default(),
                created_at: Default::default(),
                compiled_path: Default::default(),
                build_path: Default::default(),
                contract: Default::default(),
                static_analysis_off_reason: snapshot.__base_attr__.static_analysis_off_reason,
            },
            config: snapshot.deprecated_config.into(),
            __other__: snapshot.__other__,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestSource {
    pub __common_attr__: ManifestMaterializableCommonAttributes,

    // Source Specific Attributes
    pub relation_name: Option<String>,
    pub identifier: String,
    pub source_name: String,
    #[serde(
        default,
        serialize_with = "serialize_dbt_columns",
        deserialize_with = "deserialize_dbt_columns"
    )]
    pub columns: Vec<DbtColumnRef>,
    #[serde(default)]
    pub config: SourceConfig,
    pub quoting: Option<DbtQuoting>,
    pub source_description: String,
    pub unrendered_config: BTreeMap<String, YmlValue>,
    pub unrendered_database: Option<String>,
    pub unrendered_schema: Option<String>,
    #[serde(default)]
    pub loader: String,
    pub loaded_at_field: Option<String>,
    pub loaded_at_query: Option<String>,

    #[serialize_always]
    pub freshness: Option<FreshnessDefinition>,

    #[serialize_always]
    pub external: Option<ExternalTable>,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtSource> for ManifestSource {
    fn from(source: DbtSource) -> Self {
        Self {
            __common_attr__: ManifestMaterializableCommonAttributes {
                unique_id: source.__common_attr__.unique_id,
                database: source.__base_attr__.database,
                schema: source.__base_attr__.schema,
                name: source.__common_attr__.name,
                package_name: source.__common_attr__.package_name,
                fqn: source.__common_attr__.fqn,
                path: source.__common_attr__.path,
                original_file_path: source.__common_attr__.original_file_path,
                patch_path: source.__common_attr__.patch_path,
                description: source.__common_attr__.description,
                tags: source.__common_attr__.tags,
                classifiers: source.__common_attr__.classifiers,
                meta: source.__common_attr__.meta,
            },
            relation_name: source.__base_attr__.relation_name,
            identifier: source.__source_attr__.identifier,
            source_name: source.__source_attr__.source_name,
            columns: source.__base_attr__.columns,
            config: source.deprecated_config,
            quoting: source.__source_attr__.user_quoting,
            source_description: source.__source_attr__.source_description,
            unrendered_config: source.__base_attr__.unrendered_config,
            unrendered_database: source.__source_attr__.unrendered_database,
            unrendered_schema: source.__source_attr__.unrendered_schema,
            loader: source.__source_attr__.loader,
            loaded_at_field: source.__source_attr__.loaded_at_field,
            loaded_at_query: source.__source_attr__.loaded_at_query,
            freshness: source.__source_attr__.freshness,
            external: source.__source_attr__.external,
            __other__: source.__other__,
        }
    }
}

/// Macro as represented in the v12 manifest (has arguments, not args)
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestMacro {
    pub name: String,
    pub package_name: String,
    pub path: DbtPath,
    pub original_file_path: DbtPath,
    pub unique_id: String,
    pub macro_sql: String,
    #[serde(default)]
    pub depends_on: MacroDependsOn,
    pub description: String,
    #[serde(default)]
    pub meta: BTreeMap<String, YmlValue>,
    pub docs: Option<DocsConfig>,
    #[serde(default)]
    pub config: MacroConfig,
    pub patch_path: Option<PathBuf>,
    pub supported_languages: Option<Vec<String>>,
    #[serde(default)]
    pub arguments: Vec<MacroArgument>,
    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtMacro> for ManifestMacro {
    fn from(macro_: DbtMacro) -> Self {
        // dbt-core: macro.arguments in manifest only come from YAML definitions.
        // If no YAML, arguments is an empty list.
        Self {
            name: macro_.name,
            package_name: macro_.package_name,
            path: macro_.path,
            original_file_path: macro_.original_file_path,
            unique_id: macro_.unique_id,
            macro_sql: macro_.macro_sql,
            depends_on: macro_.depends_on,
            description: macro_.description,
            meta: macro_.meta,
            docs: macro_.docs,
            config: macro_.config,
            patch_path: macro_.patch_path,
            supported_languages: macro_.supported_languages,
            arguments: macro_.arguments,
            __other__: macro_.__other__,
        }
    }
}

impl From<ManifestMacro> for DbtMacro {
    fn from(macro_: ManifestMacro) -> Self {
        Self {
            name: macro_.name,
            package_name: macro_.package_name,
            path: macro_.path,
            original_file_path: macro_.original_file_path,
            absolute_path: DbtPath::default(),
            span: None,
            unique_id: macro_.unique_id,
            macro_sql: macro_.macro_sql,
            depends_on: macro_.depends_on,
            description: macro_.description,
            meta: macro_.meta,
            docs: macro_.docs,
            config: macro_.config,
            patch_path: macro_.patch_path,
            funcsign: None,
            supported_languages: macro_.supported_languages,
            args: vec![],
            arguments: macro_.arguments,
            macro_name_span: None,
            __other__: macro_.__other__,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestModel {
    pub __common_attr__: ManifestMaterializableCommonAttributes,

    pub __base_attr__: ManifestNodeBaseAttributes,

    // Model Specific Attributes
    pub access: Option<Access>,
    pub group: Option<String>,
    #[serde(default)]
    pub config: ManifestModelConfig,
    #[serialize_always]
    pub version: Option<StringOrInteger>,
    #[serialize_always]
    pub latest_version: Option<StringOrInteger>,
    pub constraints: Option<Vec<ModelConstraint>>,
    pub deprecation_date: Option<String>,
    pub primary_key: Option<Vec<String>>,
    pub time_spine: Option<ModelPropertiesTimeSpine>,

    pub __other__: BTreeMap<String, YmlValue>,
}
#[derive(Deserialize, Serialize, Debug, Default, Clone, PartialEq, DbtSchema)]
pub struct ManifestModelConfig {
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute: Option<ComputeArg>,
    pub alias: Option<String>,
    #[serde(alias = "project", alias = "data_space")]
    pub database: Omissible<Option<String>>,
    #[serde(alias = "dataset")]
    pub schema: Omissible<Option<String>>,
    // TODO: dbt-core's NodeConfig declares many of these as non-Optional with `default_factory`.
    // We keep `Option<T>` here for now (with `serialize_with` to emit defaults when None) to
    // minimize downstream churn. Fields that should eventually be made non-Option to match core:
    // tags, meta, persist_docs, quoting, column_types, on_configuration_change, packages, docs,
    // contract, access (mirrored in ManifestSnapshotConfig / ManifestSeedConfig).
    #[serde(
        default,
        serialize_with = "crate::schemas::nodes::serialize_none_as_empty_list"
    )]
    pub tags: Option<StringOrArrayOfStrings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classifiers: Option<StringOrArrayOfStrings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_name: Option<String>,
    // Internal-only placement hint; never written to the manifest.
    #[serde(skip_serializing, default)]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    // Internal-only placement hint; never written to the manifest.
    #[serde(skip_serializing, default)]
    #[schemars(with = "Option<StringOrArrayOfStrings>")]
    pub propagate: Option<AdapterTypeOrArray>,
    #[serde(
        default,
        deserialize_with = "crate::schemas::serde::default_type",
        serialize_with = "crate::schemas::serde::serialize_none_as_empty_map"
    )]
    pub meta: Option<IndexMap<String, YmlValue>>,
    pub group: Option<String>,
    pub materialized: Option<DbtMaterialization>,
    pub incremental_strategy: Option<DbtIncrementalStrategy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incremental_predicates: Option<StringOrArrayOfStrings>,
    pub batch_size: Option<DbtBatchSize>,
    pub lookback: Option<i32>,
    pub begin: Option<String>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default"
    )]
    pub persist_docs: Option<PersistDocsConfig>,
    #[serde(rename = "post-hook", default)]
    pub post_hook: Vec<HookConfig>,
    #[serde(rename = "pre-hook", default)]
    pub pre_hook: Vec<HookConfig>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default"
    )]
    pub quoting: Option<DbtQuoting>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_empty_map"
    )]
    pub column_types: Option<BTreeMap<Spanned<String>, String>>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub full_refresh: Option<bool>,
    pub unique_key: Option<DbtUniqueKey>,
    pub on_schema_change: Option<OnSchemaChange>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default"
    )]
    pub on_configuration_change: Option<OnConfigurationChange>,
    pub on_error: Option<OnError>,
    #[serde(rename = "grants", alias = "+grants")]
    pub grants: OmissibleGrantConfig,
    #[serde(
        default,
        serialize_with = "crate::schemas::nodes::serialize_none_as_empty_list"
    )]
    pub packages: Option<StringOrArrayOfStrings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default, deserialize_with = "string_or_number_to_string")]
    pub python_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imports: Option<StringOrArrayOfStrings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, YmlValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_access_integrations: Option<StringOrArrayOfStrings>,
    #[serde(
        default,
        deserialize_with = "bool_or_string_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub use_anonymous_sproc: Option<bool>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_option_docs_with_nulls"
    )]
    pub docs: Option<DocsConfig>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default"
    )]
    pub contract: Option<DbtContract>,
    pub event_time: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub concurrent_batches: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_update_columns: Option<StringOrArrayOfStrings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_exclude_columns: Option<StringOrArrayOfStrings>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default"
    )]
    pub access: Option<Access>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_format: Option<String>,
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    pub freshness: Option<ModelFreshness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded_at_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded_at_query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ModelState>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default"
    )]
    pub latest_version_pointer: Option<LatestVersionPointer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_header: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicates: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_cluster_config: Option<BTreeMap<String, YmlValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python_job_config: Option<BTreeMap<String, YmlValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_notebook: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_libs: Option<Vec<YmlValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_folder_for_python: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_dependencies: Option<Vec<String>>,
    /// Schema synchronization configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncConfig>,
    // Adapter specific configs
    pub __warehouse_specific_config__: WarehouseSpecificNodeConfig,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Default, PartialEq, Clone, DbtSchema)]
pub struct ManifestSeedConfig {
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_empty_map"
    )]
    pub column_types: Option<BTreeMap<Spanned<String>, String>>,
    #[serde(alias = "project", alias = "data_space")]
    pub database: Option<String>,
    #[serde(alias = "dataset")]
    pub schema: Option<String>,
    pub alias: Option<String>,
    pub catalog_name: Option<String>,
    // Internal-only placement hint; never written to the manifest.
    #[serde(skip_serializing, default)]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    // Internal-only placement hint; never written to the manifest.
    #[serde(skip_serializing, default)]
    #[schemars(with = "Option<StringOrArrayOfStrings>")]
    pub propagate: Option<AdapterTypeOrArray>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_option_docs_with_nulls"
    )]
    pub docs: Option<DocsConfig>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub grants: OmissibleGrantConfig,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub quote_columns: Option<bool>,
    pub delimiter: Option<Spanned<String>>,
    pub event_time: Option<String>,
    pub full_refresh: Option<bool>,
    pub group: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::schemas::serde::default_type",
        serialize_with = "crate::schemas::serde::serialize_none_as_empty_map"
    )]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default"
    )]
    pub persist_docs: Option<PersistDocsConfig>,
    #[serde(rename = "post-hook", default)]
    pub post_hook: Vec<HookConfig>,
    #[serde(rename = "pre-hook", default)]
    pub pre_hook: Vec<HookConfig>,
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    #[serde(
        default,
        serialize_with = "crate::schemas::nodes::serialize_none_as_empty_list"
    )]
    pub tags: Option<StringOrArrayOfStrings>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_default"
    )]
    pub quoting: Option<DbtQuoting>,
    pub materialized: Option<DbtMaterialization>,
    // Adapter specific configs
    pub __warehouse_specific_config__: WarehouseSpecificNodeConfig,
}

impl From<SeedConfig> for ManifestSeedConfig {
    fn from(config: SeedConfig) -> Self {
        Self {
            column_types: config.column_types,
            enabled: config.enabled,
            alias: config.alias,
            database: config.database,
            schema: config.schema,
            catalog_name: config.catalog_name,
            adapter: config.adapter,
            propagate: config.propagate,
            docs: config.docs,
            grants: config.grants,
            quote_columns: config.quote_columns,
            delimiter: config.delimiter,
            event_time: config.event_time,
            full_refresh: config.full_refresh,
            group: config.group,
            meta: config.meta,
            persist_docs: config.persist_docs,
            post_hook: (*config.post_hook)
                .as_ref()
                .map(|h| h.to_hook_config_array())
                .unwrap_or_default(),
            pre_hook: (*config.pre_hook)
                .as_ref()
                .map(|h| h.to_hook_config_array())
                .unwrap_or_default(),
            static_analysis: config.static_analysis,
            tags: config.tags.into_inner(),
            quoting: config.quoting,
            materialized: config.materialized,
            __warehouse_specific_config__: config.__warehouse_specific_config__,
        }
    }
}

impl From<ManifestSeedConfig> for SeedConfig {
    fn from(config: ManifestSeedConfig) -> Self {
        Self {
            column_types: config.column_types,
            enabled: config.enabled,
            alias: config.alias,
            database: config.database,
            schema: config.schema,
            catalog_name: config.catalog_name,
            adapter: config.adapter,
            propagate: config.propagate,
            docs: config.docs,
            grants: config.grants,
            quote_columns: config.quote_columns,
            delimiter: config.delimiter,
            event_time: config.event_time,
            full_refresh: config.full_refresh,
            group: config.group,
            meta: config.meta,
            persist_docs: config.persist_docs,
            post_hook: Verbatim::from(if config.post_hook.is_empty() {
                None
            } else {
                Some(Hooks::HookConfigArray(config.post_hook))
            }),
            pre_hook: Verbatim::from(if config.pre_hook.is_empty() {
                None
            } else {
                Some(Hooks::HookConfigArray(config.pre_hook))
            }),
            static_analysis: config.static_analysis,
            tags: crate::schemas::project::configs::config_merge::Tags(config.tags),
            quoting: config.quoting,
            materialized: config.materialized,
            __warehouse_specific_config__: config.__warehouse_specific_config__,
        }
    }
}

impl From<ModelConfig> for ManifestModelConfig {
    fn from(config: ModelConfig) -> Self {
        Self {
            enabled: config.enabled,
            compute: config.compute,
            alias: config.alias,
            database: config.database,
            schema: config.schema,
            tags: config.tags.into_inner(),
            classifiers: config.classifiers.into_inner(),
            catalog_name: config.catalog_name,
            adapter: config.adapter,
            propagate: config.propagate,
            meta: config.meta,
            group: config.group,
            materialized: config.materialized,
            incremental_strategy: config.incremental_strategy,
            incremental_predicates: config.incremental_predicates,
            batch_size: config.batch_size,
            lookback: config.lookback,
            begin: config.begin,
            persist_docs: config.persist_docs,
            post_hook: (*config.post_hook)
                .as_ref()
                .map(|h| h.to_hook_config_array())
                .unwrap_or_default(),
            pre_hook: (*config.pre_hook)
                .as_ref()
                .map(|h| h.to_hook_config_array())
                .unwrap_or_default(),
            quoting: config.quoting,
            column_types: config.column_types,
            full_refresh: config.full_refresh,
            unique_key: config.unique_key,
            on_schema_change: config.on_schema_change,
            on_configuration_change: config.on_configuration_change,
            on_error: config.on_error,
            grants: config.grants,
            packages: config.packages.into_inner(),
            python_version: config.python_version,
            imports: config.imports,
            secrets: config.secrets,
            external_access_integrations: config.external_access_integrations,
            use_anonymous_sproc: config.use_anonymous_sproc,
            docs: config.docs,
            contract: config.contract,
            event_time: config.event_time,
            concurrent_batches: config.concurrent_batches,
            merge_update_columns: config.merge_update_columns,
            merge_exclude_columns: config.merge_exclude_columns,
            access: config.access,
            table_format: config.table_format,
            static_analysis: config.static_analysis,
            freshness: config.freshness,
            loaded_at_field: config.loaded_at_field,
            loaded_at_query: (*config.loaded_at_query).clone(),
            state: config.state,
            latest_version_pointer: config.latest_version_pointer,
            sql_header: config.sql_header,
            location: config.location,
            predicates: config.predicates,
            submission_method: config.submission_method.clone(),
            job_cluster_config: config.job_cluster_config.clone(),
            python_job_config: config.python_job_config.clone(),
            cluster_id: config.cluster_id.clone(),
            http_path: config.http_path.clone(),
            create_notebook: config.create_notebook,
            index_url: config.index_url.clone(),
            additional_libs: config.additional_libs.clone(),
            user_folder_for_python: config.user_folder_for_python,
            environment_key: config.environment_key.clone(),
            environment_dependencies: config.environment_dependencies.clone(),
            sync: config.sync,
            __warehouse_specific_config__: config.__warehouse_specific_config__,
        }
    }
}

impl From<ManifestModelConfig> for ModelConfig {
    fn from(config: ManifestModelConfig) -> Self {
        Self {
            enabled: config.enabled,
            alias: config.alias,
            database: config.database,
            schema: config.schema,
            tags: crate::schemas::project::configs::config_merge::Tags(config.tags),
            classifiers: crate::schemas::project::configs::config_merge::Classifiers(
                config.classifiers,
            ),
            catalog_name: config.catalog_name,
            adapter: config.adapter,
            propagate: config.propagate,
            compute: config.compute,
            meta: config.meta,
            group: config.group,
            materialized: config.materialized,
            incremental_strategy: config.incremental_strategy,
            incremental_predicates: config.incremental_predicates,
            // ManifestModelConfig has no constraints field: dbt-core keeps constraints on
            // the node itself (`__model_attr__.constraints`), not under `config`.
            constraints: None,
            batch_size: config.batch_size,
            lookback: config.lookback,
            begin: config.begin,
            persist_docs: config.persist_docs,
            post_hook: Verbatim::from(if config.post_hook.is_empty() {
                None
            } else {
                Some(Hooks::HookConfigArray(config.post_hook))
            }),
            pre_hook: Verbatim::from(if config.pre_hook.is_empty() {
                None
            } else {
                Some(Hooks::HookConfigArray(config.pre_hook))
            }),
            quoting: config.quoting,
            column_types: config.column_types,
            full_refresh: config.full_refresh,
            unique_key: config.unique_key,
            on_schema_change: config.on_schema_change,
            on_configuration_change: config.on_configuration_change,
            on_error: config.on_error,
            grants: config.grants,
            python_version: config.python_version,
            imports: config.imports,
            secrets: config.secrets,
            external_access_integrations: config.external_access_integrations,
            use_anonymous_sproc: config.use_anonymous_sproc,
            docs: config.docs,
            contract: config.contract,
            event_time: config.event_time,
            concurrent_batches: config.concurrent_batches,
            merge_update_columns: config.merge_update_columns,
            merge_exclude_columns: config.merge_exclude_columns,
            access: config.access,
            table_format: config.table_format,
            static_analysis: config.static_analysis,
            freshness: config.freshness,
            loaded_at_field: config.loaded_at_field,
            loaded_at_query: Verbatim::from(config.loaded_at_query),
            state: config.state,
            latest_version_pointer: config.latest_version_pointer,
            sql_header: config.sql_header,
            location: config.location,
            predicates: config.predicates,
            submission_method: config.submission_method.clone(),
            job_cluster_config: config.job_cluster_config.clone(),
            python_job_config: config.python_job_config.clone(),
            cluster_id: config.cluster_id.clone(),
            http_path: config.http_path.clone(),
            create_notebook: config.create_notebook,
            index_url: config.index_url.clone(),
            additional_libs: config.additional_libs.clone(),
            user_folder_for_python: config.user_folder_for_python,
            environment_key: config.environment_key.clone(),
            environment_dependencies: config.environment_dependencies.clone(),
            sync: config.sync,
            __warehouse_specific_config__: config.__warehouse_specific_config__,
            // config_keys_used and config_keys_defaults are not in ManifestModelConfig
            // They're only in ModelConfig and not persisted to the manifest
            config_keys_used: None,
            config_keys_defaults: None,
            meta_keys_used: None,
            meta_keys_defaults: None,
            packages: crate::schemas::project::configs::config_merge::Packages(config.packages),
        }
    }
}

impl From<DbtModel> for ManifestModel {
    fn from(model: DbtModel) -> Self {
        Self {
            __common_attr__: ManifestMaterializableCommonAttributes {
                unique_id: model.__common_attr__.unique_id,
                database: model.__base_attr__.database,
                schema: model.__base_attr__.schema,
                name: model.__common_attr__.name,
                package_name: model.__common_attr__.package_name,
                fqn: model.__common_attr__.fqn,
                path: model.__common_attr__.path,
                original_file_path: model.__common_attr__.original_file_path,
                patch_path: model.__common_attr__.patch_path,
                description: model.__common_attr__.description,
                tags: model.__common_attr__.tags,
                classifiers: model.__common_attr__.classifiers,
                meta: model.__common_attr__.meta,
            },
            __base_attr__: ManifestNodeBaseAttributes {
                alias: model.__base_attr__.alias,
                relation_name: model.__base_attr__.relation_name,
                canonical_relation_adapter: (model.__base_attr__.effective_propagation_target
                    == Some(AdapterType::Databricks))
                .then_some(AdapterType::Databricks),
                columns: model.__base_attr__.columns,
                depends_on: model.__base_attr__.depends_on,
                refs: model.__base_attr__.refs,
                sources: model.__base_attr__.sources,
                functions: model.__base_attr__.functions,
                metrics: model.__base_attr__.metrics,
                raw_code: model.__common_attr__.raw_code,
                compiled: None,
                compiled_code: None,
                checksum: model.__common_attr__.checksum,
                language: model.__common_attr__.language,
                unrendered_config: model.__base_attr__.unrendered_config,
                doc_blocks: Default::default(),
                extra_ctes_injected: Default::default(),
                extra_ctes: Default::default(),
                created_at: Default::default(),
                compiled_path: Default::default(),
                build_path: Default::default(),
                contract: model.__model_attr__.contract.unwrap_or_default(),
                static_analysis_off_reason: model.__base_attr__.static_analysis_off_reason,
            },
            access: Some(model.__model_attr__.access),
            group: model.__model_attr__.group,
            config: model.deprecated_config.into(),
            version: model.__model_attr__.version,
            latest_version: model.__model_attr__.latest_version,
            constraints: Some(model.__model_attr__.constraints),
            deprecation_date: model.__model_attr__.deprecation_date,
            primary_key: Some(model.__model_attr__.primary_key),
            time_spine: model
                .__model_attr__
                .time_spine
                .map(|ts| ModelPropertiesTimeSpine {
                    custom_granularities: Some(ts.custom_granularities),
                    standard_granularity_column: ts.primary_column.name,
                }),
            __other__: model.__other__,
        }
    }
}
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManifestAnalysis {
    pub __common_attr__: ManifestMaterializableCommonAttributes,

    pub __base_attr__: ManifestNodeBaseAttributes,
    // TODO: remove top level materialized, static_analysis, and enabled and move to config along with defaults?
    #[serde(default = "default_analysis_materialized")]
    pub materialized: DbtMaterialization,
    #[serde(default = "default_analysis_static_analysis")]
    pub static_analysis: StaticAnalysisKind,
    #[serde(default = "default_analysis_enabled")]
    pub enabled: bool,
    pub quoting: Option<DbtQuoting>,
    #[serde(default)]
    pub quoting_ignore_case: bool,
    pub persist_docs: Option<PersistDocsConfig>,
    #[serde(default)]
    pub config: AnalysesConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtAnalysis> for ManifestAnalysis {
    fn from(analysis: DbtAnalysis) -> Self {
        Self {
            __common_attr__: ManifestMaterializableCommonAttributes {
                unique_id: analysis.__common_attr__.unique_id,
                database: analysis.__base_attr__.database,
                schema: analysis.__base_attr__.schema,
                name: analysis.__common_attr__.name,
                package_name: analysis.__common_attr__.package_name,
                fqn: analysis.__common_attr__.fqn,
                path: analysis.__common_attr__.path,
                original_file_path: analysis.__common_attr__.original_file_path,
                patch_path: analysis.__common_attr__.patch_path,
                description: analysis.__common_attr__.description,
                tags: analysis.__common_attr__.tags,
                classifiers: analysis.__common_attr__.classifiers,
                meta: analysis.__common_attr__.meta,
            },
            __base_attr__: ManifestNodeBaseAttributes {
                alias: analysis.__base_attr__.alias,
                relation_name: analysis.__base_attr__.relation_name,
                canonical_relation_adapter: None,
                columns: analysis.__base_attr__.columns,
                depends_on: analysis.__base_attr__.depends_on,
                refs: analysis.__base_attr__.refs,
                sources: analysis.__base_attr__.sources,
                metrics: analysis.__base_attr__.metrics,
                raw_code: analysis.__common_attr__.raw_code,
                compiled: None,
                compiled_code: None,
                checksum: analysis.__common_attr__.checksum,
                language: analysis.__common_attr__.language,
                unrendered_config: Default::default(),
                doc_blocks: Default::default(),
                extra_ctes_injected: Default::default(),
                extra_ctes: Default::default(),
                created_at: Default::default(),
                compiled_path: Default::default(),
                build_path: Default::default(),
                contract: Default::default(),
                functions: analysis.__base_attr__.functions,
                static_analysis_off_reason: analysis.__base_attr__.static_analysis_off_reason,
            },
            materialized: analysis.__base_attr__.materialized,
            static_analysis: analysis.__base_attr__.static_analysis.into_inner(),
            enabled: analysis.__base_attr__.enabled,
            quoting: Some(DbtQuoting {
                database: Some(analysis.__base_attr__.quoting.database),
                identifier: Some(analysis.__base_attr__.quoting.identifier),
                schema: Some(analysis.__base_attr__.quoting.schema),
                snowflake_ignore_case: None,
            }),
            quoting_ignore_case: analysis.__base_attr__.quoting_ignore_case,
            persist_docs: analysis.__base_attr__.persist_docs.clone(),
            config: analysis.deprecated_config,
            __other__: analysis.__other__,
        }
    }
}

/// A check as it appears in `manifest.json`.
///
/// Deliberately narrower than the other node types: a check is never materialized and has no
/// relation, so there is no `materialized`, `quoting`, `alias` or `relation_name` to record. `phase`
/// is the resolved phase (configured or inferred), which is what a consumer needs to know when the
/// check runs.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManifestCheck {
    pub __common_attr__: ManifestMaterializableCommonAttributes,

    pub __base_attr__: ManifestNodeBaseAttributes,

    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub config: CheckConfig,
}

impl From<DbtCheck> for ManifestCheck {
    fn from(check: DbtCheck) -> Self {
        Self {
            __common_attr__: ManifestMaterializableCommonAttributes {
                unique_id: check.__common_attr__.unique_id,
                database: check.__base_attr__.database,
                schema: check.__base_attr__.schema,
                name: check.__common_attr__.name,
                package_name: check.__common_attr__.package_name,
                fqn: check.__common_attr__.fqn,
                path: check.__common_attr__.path,
                original_file_path: check.__common_attr__.original_file_path,
                patch_path: check.__common_attr__.patch_path,
                description: check.__common_attr__.description,
                tags: check.__common_attr__.tags,
                classifiers: check.__common_attr__.classifiers,
                meta: check.__common_attr__.meta,
            },
            __base_attr__: ManifestNodeBaseAttributes {
                alias: check.__base_attr__.alias,
                relation_name: check.__base_attr__.relation_name,
                canonical_relation_adapter: None,
                columns: check.__base_attr__.columns,
                depends_on: check.__base_attr__.depends_on,
                refs: check.__base_attr__.refs,
                sources: check.__base_attr__.sources,
                metrics: check.__base_attr__.metrics,
                raw_code: check.__common_attr__.raw_code,
                compiled: None,
                compiled_code: None,
                checksum: check.__common_attr__.checksum,
                language: check.__common_attr__.language,
                unrendered_config: Default::default(),
                doc_blocks: Default::default(),
                extra_ctes_injected: Default::default(),
                extra_ctes: Default::default(),
                created_at: Default::default(),
                compiled_path: Default::default(),
                build_path: Default::default(),
                contract: Default::default(),
                functions: check.__base_attr__.functions,
                static_analysis_off_reason: check.__base_attr__.static_analysis_off_reason,
            },
            enabled: check.__base_attr__.enabled,
            config: check.deprecated_config,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestOperation {
    pub __common_attr__: ManifestMaterializableCommonAttributes,

    pub __base_attr__: ManifestNodeBaseAttributes,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtOperation> for ManifestOperation {
    fn from(operation: DbtOperation) -> Self {
        Self {
            __common_attr__: ManifestMaterializableCommonAttributes {
                unique_id: operation.__common_attr__.unique_id,
                database: operation.__base_attr__.database,
                schema: operation.__base_attr__.schema,
                name: operation.__common_attr__.name,
                package_name: operation.__common_attr__.package_name,
                fqn: operation.__common_attr__.fqn,
                path: operation.__common_attr__.path,
                original_file_path: operation.__common_attr__.original_file_path,
                patch_path: operation.__common_attr__.patch_path,
                description: operation.__common_attr__.description,
                tags: operation.__common_attr__.tags,
                classifiers: Default::default(),
                meta: operation.__common_attr__.meta,
            },
            __base_attr__: ManifestNodeBaseAttributes {
                alias: operation.__base_attr__.alias,
                relation_name: operation.__base_attr__.relation_name,
                canonical_relation_adapter: None,
                columns: operation.__base_attr__.columns,
                depends_on: operation.__base_attr__.depends_on,
                refs: operation.__base_attr__.refs,
                sources: operation.__base_attr__.sources,
                functions: operation.__base_attr__.functions,
                metrics: operation.__base_attr__.metrics,
                raw_code: operation.__common_attr__.raw_code,
                compiled: None,
                compiled_code: None,
                checksum: operation.__common_attr__.checksum,
                language: operation.__common_attr__.language,
                static_analysis_off_reason: operation.__base_attr__.static_analysis_off_reason,
                ..Default::default()
            },
            __other__: operation.__other__,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestFunction {
    pub __common_attr__: ManifestMaterializableCommonAttributes,

    pub __base_attr__: ManifestNodeBaseAttributes,

    // Function Specific Attributes
    #[serde(default)]
    pub config: FunctionConfig,
    pub access: Access,
    pub group: Option<String>,
    pub language: Option<String>,
    pub on_configuration_change: Option<String>,
    pub returns: Option<crate::schemas::properties::FunctionReturnType>,
    pub arguments: Option<Vec<crate::schemas::properties::FunctionArgument>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overloads: Vec<AbsorbedOverload>,

    pub __other__: BTreeMap<String, YmlValue>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
struct ManifestFunctionCompat {
    pub __common_attr__: ManifestMaterializableCommonAttributes,

    pub __base_attr__: ManifestNodeBaseAttributes,

    #[serde(default)]
    pub config: FunctionConfig,
    pub access: Option<Access>,
    pub group: Option<String>,
    pub language: Option<String>,
    pub on_configuration_change: Option<String>,
    pub returns: Option<crate::schemas::properties::FunctionReturnType>,
    pub arguments: Option<Vec<crate::schemas::properties::FunctionArgument>>,
    #[serde(default)]
    pub overloads: Vec<AbsorbedOverload>,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl<'de> Deserialize<'de> for ManifestFunction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let function = ManifestFunctionCompat::deserialize(deserializer)?;
        let access = function
            .access
            .clone()
            .or_else(|| function.config.access.clone())
            .unwrap_or(Access::Private);

        Ok(Self {
            __common_attr__: function.__common_attr__,
            __base_attr__: function.__base_attr__,
            config: function.config,
            access,
            group: function.group,
            language: function.language,
            on_configuration_change: function.on_configuration_change,
            returns: function.returns,
            arguments: function.arguments,
            overloads: function.overloads,
            __other__: function.__other__,
        })
    }
}

impl From<DbtFunction> for ManifestFunction {
    fn from(function: DbtFunction) -> Self {
        Self {
            __common_attr__: ManifestMaterializableCommonAttributes {
                database: function.__base_attr__.database,
                schema: function.__base_attr__.schema,
                unique_id: function.__common_attr__.unique_id,
                name: function.__common_attr__.name,
                package_name: function.__common_attr__.package_name,
                fqn: function.__common_attr__.fqn,
                path: function.__common_attr__.original_file_path.clone(),
                patch_path: function.__common_attr__.patch_path,
                original_file_path: function.__common_attr__.original_file_path,
                description: function.__common_attr__.description,
                tags: function.__common_attr__.tags,
                classifiers: function.__common_attr__.classifiers,
                meta: function.__common_attr__.meta,
            },
            __base_attr__: ManifestNodeBaseAttributes {
                alias: function.__base_attr__.alias,
                relation_name: function.__base_attr__.relation_name,
                canonical_relation_adapter: None,
                compiled_path: None,
                build_path: None,
                columns: Vec::new(),
                depends_on: function.__base_attr__.depends_on,
                refs: function.__base_attr__.refs,
                sources: function.__base_attr__.sources,
                raw_code: function.__common_attr__.raw_code,
                checksum: function.__common_attr__.checksum,
                compiled: None,
                static_analysis_off_reason: function.__base_attr__.static_analysis_off_reason,
                unrendered_config: function.__base_attr__.unrendered_config,
                ..Default::default()
            },
            config: function.deprecated_config,
            access: function.__function_attr__.access,
            group: function.__function_attr__.group,
            language: function.__function_attr__.language,
            on_configuration_change: function.__function_attr__.on_configuration_change,
            returns: function.__function_attr__.returns,
            arguments: function.__function_attr__.arguments,
            overloads: function.__function_attr__.overloads,
            __other__: function.__other__,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestExposureNodeBaseAttributes {
    // Derived
    #[serde(default)]
    pub depends_on: NodeDependsOn,
    #[serde(default)]
    pub refs: Vec<DbtRef>,
    #[serde(default)]
    pub sources: Vec<DbtSourceWrapper>,

    #[serde(default)]
    pub unrendered_config: BTreeMap<String, YmlValue>,

    // Metadata
    #[serde(default)]
    pub metrics: Vec<Vec<String>>,
    #[serde(default)]
    pub created_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManifestExposure {
    pub __common_attr__: ManifestCommonAttributes,

    pub __base_attr__: ManifestExposureNodeBaseAttributes,

    // Exposure Specific Attributes
    pub owner: DbtOwner,
    pub label: Option<String>,
    pub maturity: Option<String>,
    #[serde(rename = "type")]
    pub type_: ExposureType,
    pub url: Option<String>,
    #[serde(default)]
    pub config: ExposureConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtExposure> for ManifestExposure {
    fn from(exposure: DbtExposure) -> Self {
        Self {
            __common_attr__: ManifestCommonAttributes {
                unique_id: exposure.__common_attr__.unique_id,
                name: exposure.__common_attr__.name,
                package_name: exposure.__common_attr__.package_name,
                fqn: exposure.__common_attr__.fqn,
                path: exposure.__common_attr__.path,
                original_file_path: exposure.__common_attr__.original_file_path,
                description: exposure.__common_attr__.description,
                tags: exposure.__common_attr__.tags,
                classifiers: exposure.__common_attr__.classifiers,
                meta: exposure.__common_attr__.meta,
            },
            __base_attr__: ManifestExposureNodeBaseAttributes {
                depends_on: exposure.__base_attr__.depends_on,
                refs: exposure.__base_attr__.refs,
                sources: exposure.__base_attr__.sources,
                metrics: exposure.__base_attr__.metrics,
                unrendered_config: exposure.__exposure_attr__.unrendered_config,
                created_at: exposure.__exposure_attr__.created_at,
            },
            owner: exposure.__exposure_attr__.owner,
            label: exposure.__exposure_attr__.label,
            maturity: exposure.__exposure_attr__.maturity,
            type_: exposure.__exposure_attr__.type_,
            url: exposure.__exposure_attr__.url,
            config: exposure.deprecated_config,
            __other__: Default::default(),
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestMetricNodeBaseAttributes {
    // Derived
    #[serde(default)]
    pub depends_on: NodeDependsOn,

    #[serde(default)]
    pub refs: Vec<DbtRef>,

    #[serde(default)]
    pub sources: Vec<DbtSourceWrapper>,

    #[serde(default)]
    pub unrendered_config: BTreeMap<String, YmlValue>,

    #[serde(default)]
    pub created_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManifestMetric {
    pub __common_attr__: ManifestCommonAttributes,
    pub __base_attr__: ManifestMetricNodeBaseAttributes,

    // Metric Specific Attributes
    pub label: String,
    #[serde(rename = "type")]
    pub metric_type: MetricType,
    pub type_params: MetricTypeParams,
    pub filter: Option<WhereFilterIntersection>,
    pub metadata: Option<SourceFileMetadata>,
    pub time_granularity: Option<Granularity>,
    pub group: Option<String>,

    #[serde(default)]
    pub config: ManifestMetricConfig,

    #[serde(default)]
    pub metrics: Vec<Vec<String>>, // TODO: seems unused, can this be removed?

    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtMetric> for ManifestMetric {
    fn from(metric: DbtMetric) -> Self {
        Self {
            __common_attr__: ManifestCommonAttributes {
                unique_id: metric.__common_attr__.unique_id,
                name: metric.__common_attr__.name,
                package_name: metric.__common_attr__.package_name,
                fqn: metric.__common_attr__.fqn,
                path: metric.__common_attr__.path,
                original_file_path: metric.__common_attr__.original_file_path,
                description: Some(metric.__common_attr__.description.unwrap_or_default()),
                tags: metric.__common_attr__.tags,
                classifiers: metric.__common_attr__.classifiers,
                meta: metric.__common_attr__.meta,
            },
            __base_attr__: ManifestMetricNodeBaseAttributes {
                depends_on: metric.__base_attr__.depends_on,
                refs: metric.__base_attr__.refs,
                sources: metric.__base_attr__.sources,
                unrendered_config: metric.__metric_attr__.unrendered_config,
                created_at: metric.__metric_attr__.created_at,
            },
            label: metric.__metric_attr__.label.unwrap_or_default(),
            metric_type: metric.__metric_attr__.metric_type,
            type_params: metric.__metric_attr__.type_params,
            filter: metric.__metric_attr__.filter,
            metadata: metric.__metric_attr__.metadata,
            time_granularity: metric.__metric_attr__.time_granularity.clone(),
            group: metric.__metric_attr__.group.clone(),
            config: metric.deprecated_config.into(),
            __other__: metric.__other__,
            metrics: vec![],
        }
    }
}

impl From<ManifestMetric> for DbtMetric {
    fn from(manifest_metric: ManifestMetric) -> Self {
        DbtMetric {
            __common_attr__: CommonAttributes {
                unique_id: manifest_metric.__common_attr__.unique_id,
                name: manifest_metric.__common_attr__.name,
                package_name: manifest_metric.__common_attr__.package_name,
                fqn: manifest_metric.__common_attr__.fqn,
                path: manifest_metric.__common_attr__.path,
                original_file_path: manifest_metric.__common_attr__.original_file_path,
                // dbt-core writes an absent description as "" for byte-parity; map it back to
                // `None` so it round-trips against a freshly-parsed metric (dbt-core#15513).
                description: manifest_metric
                    .__common_attr__
                    .description
                    .filter(|s| !s.is_empty()),
                tags: manifest_metric.__common_attr__.tags,
                classifiers: manifest_metric.__common_attr__.classifiers,
                meta: manifest_metric.__common_attr__.meta,
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                depends_on: manifest_metric.__base_attr__.depends_on,
                refs: manifest_metric.__base_attr__.refs,
                sources: manifest_metric.__base_attr__.sources,
                ..Default::default()
            },
            __metric_attr__: DbtMetricAttr {
                label: Some(manifest_metric.label),
                metric_type: manifest_metric.metric_type,
                type_params: manifest_metric.type_params,
                filter: manifest_metric.filter,
                metadata: manifest_metric.metadata,
                time_granularity: manifest_metric.time_granularity,
                unrendered_config: manifest_metric.__base_attr__.unrendered_config,
                metrics: vec![],
                created_at: manifest_metric.__base_attr__.created_at,
                group: manifest_metric.group,
            },
            deprecated_config: manifest_metric.config.into(),
            __other__: manifest_metric.__other__,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct ManifestMetricConfig {
    pub enabled: bool,

    #[serde(serialize_with = "crate::schemas::serde::serialize_none_as_empty_map")]
    pub meta: Option<IndexMap<String, YmlValue>>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tags: Vec<String>,

    pub group: Option<String>,
}

impl Default for ManifestMetricConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            meta: None,
            tags: vec![],
            group: None,
        }
    }
}

impl From<MetricConfig> for ManifestMetricConfig {
    fn from(config: MetricConfig) -> Self {
        Self {
            enabled: config.enabled.unwrap_or(true),
            meta: config.meta,
            tags: match config.tags.into_inner() {
                Some(StringOrArrayOfStrings::ArrayOfStrings(ref tags)) => tags.clone(),
                Some(StringOrArrayOfStrings::String(ref tag)) => vec![tag.clone()],
                None => vec![],
            },
            group: config.group,
        }
    }
}

impl From<ManifestMetricConfig> for MetricConfig {
    fn from(config: ManifestMetricConfig) -> Self {
        MetricConfig {
            enabled: Some(config.enabled),
            meta: config.meta,
            tags: if config.tags.is_empty() {
                crate::schemas::project::configs::config_merge::Tags(None)
            } else {
                crate::schemas::project::configs::config_merge::Tags(Some(
                    StringOrArrayOfStrings::ArrayOfStrings(config.tags),
                ))
            },
            group: config.group,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestSemanticModelNodeBaseAttributes {
    // Derived
    #[serde(default)]
    pub depends_on: NodeDependsOn,

    #[serde(default)]
    pub refs: Vec<DbtRef>,

    #[serde(default)]
    pub unrendered_config: BTreeMap<String, YmlValue>,

    #[serde(default)]
    pub created_at: f64,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct ManifestSemanticModelConfig {
    pub enabled: bool,

    // `meta` is written as `{}` when `None` for dbt-core byte-parity; pair the
    // serializer with the empty-map->None deserializer so the config round-trips.
    // Without the inverse, a semantic model with no `meta` reads back as
    // `Some({})` and `DbtSemanticModel::has_same_content` flags a phantom
    // `deprecated_config` diff under `state:modified` (dbt-core#15513).
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_none_as_empty_map",
        deserialize_with = "crate::schemas::serde::deserialize_empty_map_as_none"
    )]
    pub meta: Option<IndexMap<String, YmlValue>>,

    pub group: Option<String>,
}

impl Default for ManifestSemanticModelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            meta: None,
            group: None,
        }
    }
}

impl From<SemanticModelConfig> for ManifestSemanticModelConfig {
    fn from(config: SemanticModelConfig) -> Self {
        Self {
            enabled: config.enabled.unwrap_or(true),
            meta: config.meta,
            group: config.group,
        }
    }
}

impl From<ManifestSemanticModelConfig> for SemanticModelConfig {
    fn from(config: ManifestSemanticModelConfig) -> Self {
        SemanticModelConfig {
            enabled: Some(config.enabled),
            meta: config.meta,
            group: config.group,
            tags: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManifestSemanticModel {
    pub __common_attr__: ManifestCommonAttributes,
    pub __base_attr__: ManifestSemanticModelNodeBaseAttributes,

    // Semantic Model Specific Attributes
    pub model: String,
    pub node_relation: Option<NodeRelation>,
    pub label: Option<String>,
    pub defaults: Option<SemanticModelDefaults>,
    #[serde(default)]
    pub entities: Vec<SemanticEntity>,
    #[serde(default)]
    pub measures: Vec<ManifestSemanticModelMeasure>,
    #[serde(default)]
    pub dimensions: Vec<crate::schemas::common::Dimension>,
    pub metadata: Option<SourceFileMetadata>,
    pub primary_entity: Option<String>,
    pub group: Option<String>,

    #[serde(default)]
    pub config: ManifestSemanticModelConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtSemanticModel> for ManifestSemanticModel {
    fn from(semantic_model: DbtSemanticModel) -> Self {
        Self {
            __common_attr__: ManifestCommonAttributes {
                unique_id: semantic_model.__common_attr__.unique_id,
                name: semantic_model.__common_attr__.name,
                package_name: semantic_model.__common_attr__.package_name,
                fqn: semantic_model.__common_attr__.fqn,
                path: semantic_model.__common_attr__.path,
                original_file_path: semantic_model.__common_attr__.original_file_path,
                description: semantic_model.__common_attr__.description,
                tags: semantic_model.__common_attr__.tags,
                classifiers: semantic_model.__common_attr__.classifiers,
                meta: semantic_model.__common_attr__.meta,
            },
            __base_attr__: ManifestSemanticModelNodeBaseAttributes {
                depends_on: semantic_model.__base_attr__.depends_on,
                refs: semantic_model.__base_attr__.refs,
                unrendered_config: semantic_model.__semantic_model_attr__.unrendered_config,
                created_at: semantic_model.__semantic_model_attr__.created_at,
            },
            label: semantic_model.__semantic_model_attr__.label,
            metadata: semantic_model.__semantic_model_attr__.metadata,
            group: semantic_model.__semantic_model_attr__.group,
            config: semantic_model.deprecated_config.into(),
            __other__: semantic_model.__other__,
            model: semantic_model.__semantic_model_attr__.model,
            node_relation: semantic_model.__semantic_model_attr__.node_relation,
            defaults: semantic_model.__semantic_model_attr__.defaults,
            entities: semantic_model.__semantic_model_attr__.entities,
            measures: semantic_model
                .__semantic_model_attr__
                .measures
                .into_iter()
                .map(ManifestSemanticModelMeasure::from)
                .collect(),
            dimensions: semantic_model.__semantic_model_attr__.dimensions,
            primary_entity: semantic_model.__semantic_model_attr__.primary_entity,
        }
    }
}

impl From<ManifestSemanticModel> for DbtSemanticModel {
    fn from(manifest_model: ManifestSemanticModel) -> Self {
        DbtSemanticModel {
            __common_attr__: CommonAttributes {
                unique_id: manifest_model.__common_attr__.unique_id,
                name: manifest_model.__common_attr__.name,
                package_name: manifest_model.__common_attr__.package_name,
                fqn: manifest_model.__common_attr__.fqn,
                path: manifest_model.__common_attr__.path,
                original_file_path: manifest_model.__common_attr__.original_file_path,
                description: manifest_model.__common_attr__.description,
                tags: manifest_model.__common_attr__.tags,
                classifiers: manifest_model.__common_attr__.classifiers,
                meta: manifest_model.__common_attr__.meta,
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                depends_on: manifest_model.__base_attr__.depends_on,
                refs: manifest_model.__base_attr__.refs,
                ..Default::default()
            },
            __semantic_model_attr__: DbtSemanticModelAttr {
                model: manifest_model.model,
                node_relation: manifest_model.node_relation,
                label: manifest_model.label,
                defaults: manifest_model.defaults,
                entities: manifest_model.entities,
                dimensions: manifest_model.dimensions,
                metadata: manifest_model.metadata,
                primary_entity: manifest_model.primary_entity,
                measures: manifest_model
                    .measures
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                created_at: manifest_model.__base_attr__.created_at,
                unrendered_config: manifest_model.__base_attr__.unrendered_config,
                group: manifest_model.group,
            },
            deprecated_config: manifest_model.config.into(),
            __other__: manifest_model.__other__,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestSemanticModelMeasure {
    pub name: String,
    pub agg: AggregationType,
    pub description: Option<String>,
    pub label: Option<String>,
    pub create_metric: Option<bool>,
    pub expr: Option<String>,
    pub agg_params: Option<MeasureAggregationParameters>,
    pub non_additive_dimension: Option<NonAdditiveDimension>,
    pub agg_time_dimension: Option<String>,
    pub config: Option<SemanticLayerElementConfig>,
}

impl From<SemanticMeasure> for ManifestSemanticModelMeasure {
    fn from(measure: SemanticMeasure) -> Self {
        Self {
            name: measure.name,
            agg: measure.agg,
            description: measure.description,
            label: measure.label,
            create_metric: measure.create_metric,
            expr: measure.expr,
            agg_params: measure.agg_params,
            non_additive_dimension: measure.non_additive_dimension,
            agg_time_dimension: measure.agg_time_dimension,
            config: measure.config,
        }
    }
}

impl From<ManifestSemanticModelMeasure> for SemanticMeasure {
    fn from(measure: ManifestSemanticModelMeasure) -> Self {
        SemanticMeasure {
            name: measure.name,
            agg: measure.agg,
            description: measure.description,
            label: measure.label,
            create_metric: measure.create_metric,
            expr: measure.expr,
            agg_params: measure.agg_params,
            non_additive_dimension: measure.non_additive_dimension,
            agg_time_dimension: measure.agg_time_dimension,
            config: measure.config,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ManifestSavedQueryNodeBaseAttributes {
    // Derived
    #[serde(default)]
    pub depends_on: NodeDependsOn,

    #[serde(default)]
    pub refs: Vec<DbtRef>,

    #[serde(default)]
    pub unrendered_config: BTreeMap<String, YmlValue>,

    #[serde(default)]
    pub created_at: f64,
}

// #[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManifestSavedQuery {
    pub __common_attr__: ManifestCommonAttributes,
    pub __base_attr__: ManifestSavedQueryNodeBaseAttributes,

    // Saved Query Specific Attributes
    pub query_params: crate::schemas::manifest::saved_query::SavedQueryParams,
    pub exports: Vec<crate::schemas::manifest::saved_query::SavedQueryExport>,
    pub label: Option<String>,
    pub metadata: Option<SourceFileMetadata>,
    pub group: Option<String>,

    #[serde(default)]
    pub config: SavedQueryConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

impl From<DbtSavedQuery> for ManifestSavedQuery {
    fn from(saved_query: DbtSavedQuery) -> Self {
        Self {
            __common_attr__: ManifestCommonAttributes {
                unique_id: saved_query.__common_attr__.unique_id,
                name: saved_query.__common_attr__.name,
                package_name: saved_query.__common_attr__.package_name,
                fqn: saved_query.__common_attr__.fqn,
                path: saved_query.__common_attr__.path,
                original_file_path: saved_query.__common_attr__.original_file_path,
                description: saved_query.__common_attr__.description,
                tags: saved_query.__common_attr__.tags,
                classifiers: saved_query.__common_attr__.classifiers,
                meta: saved_query.__common_attr__.meta,
            },
            __base_attr__: ManifestSavedQueryNodeBaseAttributes {
                depends_on: saved_query.__base_attr__.depends_on,
                refs: saved_query.__base_attr__.refs,
                unrendered_config: saved_query.__saved_query_attr__.unrendered_config,
                created_at: saved_query.__saved_query_attr__.created_at,
            },
            query_params: saved_query.__saved_query_attr__.query_params,
            exports: saved_query.__saved_query_attr__.exports,
            label: saved_query.__saved_query_attr__.label,
            metadata: saved_query.__saved_query_attr__.metadata,
            group: saved_query.__saved_query_attr__.group,
            config: saved_query.deprecated_config,
            __other__: saved_query.__other__,
        }
    }
}

#[cfg(test)]
mod manifest_model_config_null_omission_tests {
    use super::ManifestModelConfig;

    /// dbt-core macros like `default__create_indexes` call
    /// `config.get('indexes', default=[])` with no null-guard, so an explicit JSON
    /// `null` (instead of an omitted key) breaks under `--use-v2-parser`
    /// (dbt-core#15913). These fields must be omitted from the manifest when unset.
    #[test]
    fn unset_fields_are_omitted_not_serialized_as_null() {
        let config = ManifestModelConfig::default();
        let json = serde_json::to_value(&config).unwrap();

        for key in ["classifiers", "incremental_predicates", "sql_header"] {
            assert!(
                json.get(key).is_none(),
                "expected top-level key `{key}` to be omitted when unset, got: {json}"
            );
        }

        let warehouse_specific = json
            .get("__warehouse_specific_config__")
            .expect("__warehouse_specific_config__ should be present");
        for key in ["indexes", "primary_key"] {
            assert!(
                warehouse_specific.get(key).is_none(),
                "expected `{key}` to be omitted when unset, got: {warehouse_specific}"
            );
        }
    }
}

#[cfg(test)]
mod node_adapter_manifest_round_trip_tests {
    use super::{
        AdapterType, DbtModel, DbtSeed, ManifestModel, ManifestSeedConfig, ManifestSnapshotConfig,
        NodeBaseAttributes, SeedConfig, SnapshotConfig,
    };

    #[test]
    fn databricks_canonical_relation_marker_round_trips_for_models_and_seeds() {
        let model = DbtModel {
            __base_attr__: NodeBaseAttributes {
                effective_propagation_target: Some(AdapterType::Databricks),
                ..Default::default()
            },
            ..Default::default()
        };
        let manifest_model: ManifestModel = model.into();
        assert_eq!(
            manifest_model.__base_attr__.canonical_relation_adapter,
            Some(AdapterType::Databricks)
        );

        let seed = DbtSeed {
            __base_attr__: NodeBaseAttributes {
                effective_propagation_target: Some(AdapterType::Databricks),
                ..Default::default()
            },
            ..Default::default()
        };
        let manifest_seed: super::ManifestSeed = seed.into();
        assert_eq!(
            manifest_seed.__base_attr__.canonical_relation_adapter,
            Some(AdapterType::Databricks)
        );
    }

    #[test]
    fn seed_config_adapter_round_trips_through_manifest_seed_config() {
        let seed_config = SeedConfig {
            adapter: Some(AdapterType::LakeCompute),
            ..Default::default()
        };

        let manifest_config: ManifestSeedConfig = seed_config.into();
        assert_eq!(manifest_config.adapter, Some(AdapterType::LakeCompute));

        let round_tripped: SeedConfig = manifest_config.into();
        assert_eq!(round_tripped.adapter, Some(AdapterType::LakeCompute));
    }

    #[test]
    fn snapshot_config_adapter_round_trips_through_manifest_snapshot_config() {
        let snapshot_config = SnapshotConfig {
            adapter: Some(AdapterType::Bigquery),
            ..Default::default()
        };

        let manifest_config: ManifestSnapshotConfig = snapshot_config.into();
        assert_eq!(manifest_config.adapter, Some(AdapterType::Bigquery));

        let round_tripped: SnapshotConfig = manifest_config.into();
        assert_eq!(round_tripped.adapter, Some(AdapterType::Bigquery));
    }

    #[test]
    fn manifest_snapshot_config_never_serializes_adapter() {
        let manifest_config = ManifestSnapshotConfig {
            adapter: Some(AdapterType::Bigquery),
            ..Default::default()
        };

        let json = serde_json::to_value(&manifest_config).unwrap();
        assert!(
            json.get("adapter").is_none(),
            "adapter is an internal placement hint and must not appear in the manifest: {json}"
        );
    }

    #[test]
    fn manifest_seed_config_never_serializes_adapter() {
        let manifest_config = ManifestSeedConfig {
            adapter: Some(AdapterType::LakeCompute),
            ..Default::default()
        };

        let json = serde_json::to_value(&manifest_config).unwrap();
        assert!(
            json.get("adapter").is_none(),
            "adapter is an internal placement hint and must not appear in the manifest: {json}"
        );
    }
}
