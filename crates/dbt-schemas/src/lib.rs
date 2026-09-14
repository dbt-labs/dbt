// `DbtProfile::adapters` is a public `IndexMap`, so downstream crates need to be
// able to name it to construct a profile without taking an `indexmap` dependency
// of their own.
pub use indexmap::IndexMap;

pub mod constants;
pub mod dbt_types;
pub mod dbt_utils;
pub mod filter;
pub mod man;
pub mod materialization_resolver;
pub mod state;
pub mod stats;

pub mod schemas {
    pub mod common;
    pub mod data_tests;
    pub mod dbt_catalogs;
    pub mod dbt_catalogs_v2;
    pub mod dbt_column;
    pub mod serialization_utils;

    pub use dbt_catalogs::{
        AdapterPropsView, CatalogSpecView, DatabricksUnityPropsView, DbtCatalogsView, FileFormat,
        SerializationPolicy, SnowflakeBuiltInPropsView, SnowflakeRestPropsView, TargetFileSize,
        WriteIntegrationView, validate_catalogs,
    };
    pub use dbt_catalogs_v2::TableFormat;
    pub mod macros;
    pub mod packages;
    mod prev_state;
    pub mod profiles;
    pub mod ref_and_source;
    pub mod relations;
    mod run_results;
    pub mod selection_override;
    pub mod selectors;
    pub mod serde;
    mod sources;
    pub mod user_settings;
    pub use prev_state::{
        ModificationType, OnManifestLoadFailure, StateArtifacts, config_excluded_keys,
    };
    pub use run_results::{
        BatchResults, ContextRunResult, DbtCommandExecutionArtifacts, RunResultOutput,
        RunResultsArgs, RunResultsArtifact, RunResultsMetadata, TimingInfo,
    };
    pub use user_settings::UserSettings;

    pub mod nodes;
    pub use nodes::{
        AbsorbedOverload, AdapterAttr, CommonAttributes, DbtAnalysis, DbtAnalysisAttr, DbtCheck,
        DbtCheckAttr, DbtExposure, DbtExposureAttr, DbtFunction, DbtFunctionAttr, DbtModel,
        DbtModelAttr, DbtSeed, DbtSeedAttr, DbtSnapshot, DbtSnapshotAttr, DbtSource, DbtSourceAttr,
        DbtTest, DbtTestAttr, DbtUnitTest, DbtUnitTestAttr, ExposureType, InternalDbtNode,
        InternalDbtNodeAttributes, InternalDbtNodeWrapper, IntrospectionKind, NodeBaseAttributes,
        NodePathKind, Nodes, TestMetadata, TimeSpine, TimeSpinePrimaryColumn,
        deserialize_empty_string_as_none, serialize_none_as_empty_string,
    };

    pub use sources::{FreshnessResultsArtifact, FreshnessResultsMetadata, FreshnessResultsNode};

    pub mod freshness_node;
    pub use freshness_node::{FreshnessNodeRef, is_freshness_node};
    pub mod legacy_catalog {
        mod catalog;
        pub use catalog::CatalogNodeStats;
        pub use catalog::CatalogTable;
        pub use catalog::ColumnMetadata;
        pub use catalog::DbtCatalog;
        pub use catalog::TableMetadata;
        pub use catalog::build_catalog;
    }
    pub mod manifest {
        mod bigquery_partition;
        mod defer_relation;
        mod group;
        #[allow(clippy::module_inception)]
        mod manifest;
        mod manifest_nodes;
        pub mod metric;
        mod operation;
        pub mod postgres;
        pub mod saved_query;
        mod selector;
        pub mod semantic_model;

        // Versioned manifest modules
        pub mod v10;
        pub mod v11;
        pub mod v12;

        pub mod common;
        pub use bigquery_partition::{
            BigqueryPartitionConfig, BigqueryPartitionConfigInner, GrantAccessToTarget, Range,
            RangeConfig, TimeConfig,
        };
        pub use defer_relation::{
            DeferRelation, defer_relation_for_model, defer_relation_for_seed,
            defer_relation_for_snapshot,
        };
        pub use group::ManifestGroup;
        pub use manifest::{
            BaseMetadata, DbtManifest, DbtNode, ManifestMetadata, build_manifest,
            nodes_from_dbt_manifest,
        };
        pub use manifest_nodes::{
            ManifestCheck, ManifestDataTest, ManifestExposure, ManifestFunction, ManifestMacro,
            ManifestMetric, ManifestModel, ManifestModelConfig, ManifestSavedQuery, ManifestSeed,
            ManifestSeedConfig, ManifestSemanticModel, ManifestSnapshot, ManifestSnapshotConfig,
            ManifestSource, ManifestUnitTest,
        };
        pub use metric::DbtMetric;
        pub use operation::DbtOperation;
        pub use saved_query::{DbtSavedQuery, DbtSavedQueryAttr};
        pub use selector::DbtSelector;
        pub use semantic_model::DbtSemanticModel;
        pub use v10::DbtManifestV10;
        pub use v11::DbtManifestV11;
        pub use v12::DbtManifestV12;
    }
    pub mod dbt_cloud;
    pub use dbt_cloud::{
        CloudCredentials, DbtCloudConfig, DbtCloudContext, DbtCloudProject, DbtCloudProjectConfig,
        DbtCloudState, ResolvedCloudConfig,
    };

    pub mod semantic_layer {
        pub mod metric;
        pub mod project_configuration;
        pub mod saved_query;
        pub mod semantic_manifest;
        pub mod semantic_model;
    }
    pub mod project {
        mod config_tree;
        mod dbt_project;
        pub(crate) mod configs {
            pub mod analysis_config;
            pub mod check_config;
            pub mod common;
            pub mod config_keys;
            pub mod config_merge;
            pub mod config_merge_tests;
            pub mod data_test_config;
            pub mod exposure_config;
            pub mod function_config;
            pub mod metric_config;
            pub mod model_config;
            pub mod saved_query_config;
            pub mod seed_config;
            pub mod semantic_model_config;
            pub mod skill_config;
            pub mod snapshot_config;
            pub mod source_config;
            pub mod unit_test_config;
        }

        pub use config_tree::{
            DbtProjectConfig, ProjectConfigResolver, RootProjectConfigs,
            authored_quoting_per_adapter, build_root_project_configs,
            disallow_plus_prefix_from_flags, init_project_config, recur_build_dbt_project_config,
            strip_resource_paths_from_ref_path,
        };
        pub use configs::analysis_config::{
            AnalysesConfig, ProjectAnalysisConfig, ResolvedAnalysesConfig,
        };
        pub use configs::check_config::{
            CheckConfig, DEFAULT_CHECK_SEVERITY, InfoSchemaConfig, ProjectCheckConfig,
            ResolvedCheckConfig, SUPPORTED_INFO_SCHEMA_VERSIONS, SelectionFilterOn,
        };
        pub use configs::common::{WarehouseSpecificNodeConfig, same_warehouse_config};
        pub use configs::config_keys::ConfigKeys;
        pub use configs::config_merge::{DefaultTo, Packages, Tags, TblProperties};
        pub use configs::data_test_config::{
            DEFAULT_DATA_TEST_ERROR_IF, DEFAULT_DATA_TEST_FAIL_CALC, DEFAULT_DATA_TEST_SEVERITY,
            DEFAULT_DATA_TEST_WARN_IF, DataTestConfig, ProjectDataTestConfig,
            ResolvedDataTestConfig,
        };
        pub use configs::exposure_config::{
            ExposureConfig, ProjectExposureConfig, ResolvedExposureConfig,
        };
        pub use configs::function_config::{
            FunctionConfig, ProjectFunctionConfig, ResolvedFunctionConfig,
        };
        pub use configs::metric_config::{
            MetricConfig, ProjectMetricConfigs, ResolvedMetricConfig,
        };
        pub use configs::model_config::{
            LatestVersionPointer, ModelConfig, ProjectModelConfig, ResolvedModelConfig,
        };
        pub use configs::saved_query_config::{
            ExportConfigExportAs, ResolvedSavedQueryConfig, SavedQueryCache, SavedQueryConfig,
        };
        pub use configs::seed_config::{ProjectSeedConfig, ResolvedSeedConfig, SeedConfig};
        pub use configs::semantic_model_config::{
            ProjectSemanticModelConfig, ResolvedSemanticModelConfig, SemanticModelConfig,
        };
        pub use configs::skill_config::{ProjectSkillConfig, ResolvedSkillConfig, SkillConfig};
        pub use configs::snapshot_config::{
            ProjectSnapshotConfig, ResolvedSnapshotConfig, SnapshotConfig, SnapshotMetaColumnNames,
        };
        pub use configs::source_config::{ProjectSourceConfig, ResolvedSourceConfig, SourceConfig};
        pub use configs::unit_test_config::{
            ProjectUnitTestConfig, ResolvedUnitTestConfig, UnitTestConfig,
        };
        pub use dbt_project::{
            AdapterProjectConfig, DEFAULT_SKILL_PATH, DbtProject, DbtProjectNameOnly,
            DbtProjectSimplified, ProjectDbtCloudConfig, QueryComment, ResolvableConfig,
            ResolvedConfig, TypedRecursiveConfig,
        };
    }

    pub mod properties {
        mod analysis_properties;
        mod check_properties;
        mod data_test_properties;
        mod exposure_properties;
        mod function_properties;
        pub mod metrics_properties;
        pub mod model_properties;
        #[allow(clippy::module_inception)]
        mod properties;
        mod saved_queries_properties;
        mod seed_properties;
        mod snapshot_properties;
        mod source_properties;
        mod unit_test_properties;

        pub use analysis_properties::AnalysesProperties;
        pub use check_properties::CheckProperties;
        pub use data_test_properties::DataTestProperties;
        pub use exposure_properties::ExposureProperties;
        pub use function_properties::{
            FUNCTION_LANGUAGE_JAVASCRIPT, FUNCTION_LANGUAGE_PYTHON, FUNCTION_LANGUAGE_SQL,
            FunctionArgument, FunctionKind, FunctionOverload, FunctionProperties,
            FunctionReturnType, Volatility,
        };
        pub use metrics_properties::MetricsProperties;
        pub use model_properties::DataTestState;
        pub use model_properties::ModelConstraint;
        pub use model_properties::ModelFreshness;
        pub use model_properties::ModelProperties;
        pub use model_properties::ModelState;
        pub use model_properties::StatePreClone;
        pub use properties::GroupConfig;
        pub use properties::GroupProperties;
        pub use properties::MacrosProperties;
        pub use properties::{
            DbtPropertiesFile, DbtPropertiesFileValues, GetConfig, MinimalSchemaValue,
            MinimalTableValue, MinimalUnitTestValue,
        };
        pub use saved_queries_properties::SavedQueriesProperties;
        pub use seed_properties::SeedProperties;
        pub use snapshot_properties::SnapshotProperties;
        pub use source_properties::{SourceProperties, Tables, TablesConfig};
        pub use unit_test_properties::{UnitTestOverrides, UnitTestProperties};
    }

    // TODO: When dbt-schemas dependency on dbt-common is removed, we should move this to dbt_schemas
    pub use dbt_telemetry as telemetry;
}
