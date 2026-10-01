use std::collections::HashMap;
use std::collections::btree_map::Iter;
use std::fmt::Debug;

use indexmap::IndexMap;

use dbt_adapter_core::AdapterType;
use dbt_yaml::DbtSchema;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;
use dbt_yaml::ShouldBe;
use dbt_yaml::Spanned;
use dbt_yaml::UntaggedEnumDeserialize;
use dbt_yaml::Verbatim;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use strum::{Display, EnumString};

use crate::schemas::common::DbtQuoting;
use crate::schemas::common::SyncConfig;
use crate::schemas::project::InfoSchemaConfig;
use crate::schemas::project::ProjectAnalysisConfig;
use crate::schemas::project::ProjectCheckConfig;
use crate::schemas::project::ProjectSemanticModelConfig;
use crate::schemas::project::configs::saved_query_config::ProjectSavedQueryConfig;
use crate::schemas::serde::FloatOrString;
use crate::schemas::serde::SpannedStringOrArrayOfStrings;
use crate::schemas::serde::StringOrArrayOfStrings;
use crate::schemas::serde::StringOrInteger;

use super::ProjectDataTestConfig;
use super::ProjectExposureConfig;
use super::ProjectFunctionConfig;
use super::ProjectMetricConfigs;
use super::ProjectModelConfig;
use super::ProjectSeedConfig;
use super::ProjectSkillConfig;
use super::ProjectSnapshotConfig;
use super::ProjectSourceConfig;
use super::ProjectUnitTestConfig;

/// Default `skill-paths` when the project does not set one.
pub const DEFAULT_SKILL_PATH: &str = "skills";

#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ProjectDbtCloudConfig {
    #[serde(rename = "project-id")]
    pub project_id: Spanned<Option<StringOrInteger>>,
    #[serde(rename = "defer-env-id")]
    pub defer_env_id: Option<StringOrInteger>,
    #[serde(rename = "state-org-id")]
    pub state_org_id: Option<StringOrInteger>,

    // unsure if any of these other keys are actually used or expected
    pub account_id: Option<StringOrInteger>,
    #[serde(rename = "account-host")]
    pub account_host: Option<String>,
    #[serde(rename = "job-id")]
    pub job_id: Option<StringOrInteger>,
    #[serde(rename = "run-id")]
    pub run_id: Option<StringOrInteger>,
    pub api_key: Option<StringOrInteger>,
    pub application: Option<StringOrInteger>,
    pub environment: Option<StringOrInteger>,
    pub tenant_hostname: Option<String>,
}

impl ProjectDbtCloudConfig {
    pub fn project_id_str(&self) -> Option<String> {
        self.project_id.as_ref().as_ref().map(|v| v.to_string())
    }
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct DbtProjectNameOnly {
    pub name: String,

    pub __ignored__: Verbatim<HashMap<String, dbt_yaml::Value>>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct DbtProjectSimplified {
    #[serde(rename = "packages-install-path")]
    pub packages_install_path: Option<String>,
    pub profile: Spanned<Option<String>>,
    #[serde(rename = "dbt-cloud")]
    pub dbt_cloud: Option<ProjectDbtCloudConfig>,
    pub flags: Option<YmlValue>,

    // Deprecated paths
    // When present in the db_project.yml file we will raise an error
    #[serde(rename = "source-paths")]
    pub source_paths: Verbatim<Option<Vec<String>>>,
    #[serde(rename = "log-path")]
    pub log_path: Verbatim<Option<String>>,
    #[serde(rename = "target-path")]
    pub target_path: Verbatim<Option<String>>,

    pub __ignored__: Verbatim<HashMap<String, dbt_yaml::Value>>,
}

#[derive(
    Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, EnumString, Display, DbtSchema,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum LogPath {
    #[default]
    Logs,
}

#[derive(
    Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, EnumString, Display, DbtSchema,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum TargetPath {
    #[default]
    Target,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct DbtProject {
    pub name: String,
    pub version: Option<FloatOrString>,
    pub profile: Option<String>,
    // Paths
    #[serde(rename = "analysis-paths")]
    pub analysis_paths: Option<Vec<String>>,
    #[serde(rename = "asset-paths")]
    pub asset_paths: Option<Vec<String>>,
    #[serde(rename = "macro-paths")]
    pub macro_paths: Option<Vec<String>>,
    #[serde(rename = "model-paths")]
    pub model_paths: Option<Vec<String>>,
    #[serde(rename = "function-paths")]
    pub function_paths: Option<Vec<String>>,
    #[serde(rename = "seed-paths", alias = "data-paths")]
    pub seed_paths: Option<Vec<String>>,
    #[serde(rename = "snapshot-paths")]
    pub snapshot_paths: Option<Vec<String>>,
    #[serde(rename = "test-paths")]
    pub test_paths: Option<Vec<String>>,
    #[serde(rename = "check-paths")]
    pub check_paths: Option<Vec<String>>,
    #[serde(rename = "docs-paths")]
    pub docs_paths: Option<Vec<String>>,
    #[serde(rename = "skill-paths")]
    pub skill_paths: Option<Vec<String>>,
    #[serde(rename = "target-path")]
    pub target_path: Option<TargetPath>,
    #[serde(rename = "log-path")]
    pub log_path: Option<LogPath>,
    #[serde(rename = "packages-install-path")]
    pub packages_install_path: Option<String>,
    // Configs
    pub metrics: Option<ProjectMetricConfigs>,
    pub models: Option<ProjectModelConfig>,
    pub functions: Option<ProjectFunctionConfig>,
    pub snapshots: Option<ProjectSnapshotConfig>,
    pub seeds: Option<ProjectSeedConfig>,
    pub sources: Option<ProjectSourceConfig>,
    /// Project-level `checks:` config tree (path-scoped, `+`-prefixed), resolved by the standard
    /// project-config machinery like every other node type's subtree.
    pub checks: Option<ProjectCheckConfig>,
    /// Package-scoped `info_schema:` block. Not resolved/inherited like `checks:` -- see
    /// [`InfoSchemaConfig`].
    pub info_schema: Option<InfoSchemaConfig>,
    pub tests: Option<ProjectDataTestConfig>,
    pub unit_tests: Option<ProjectUnitTestConfig>,
    pub data_tests: Option<ProjectDataTestConfig>,
    pub exposures: Option<ProjectExposureConfig>,
    pub analyses: Option<ProjectAnalysisConfig>,
    pub skills: Option<ProjectSkillConfig>,
    #[serde(rename = "saved-queries")]
    pub saved_queries: Option<ProjectSavedQueryConfig>,
    #[serde(rename = "semantic-models")]
    pub semantic_models: Option<ProjectSemanticModelConfig>,
    // Misc
    #[serde(rename = "clean-targets")]
    pub clean_targets: Option<Vec<String>>,
    #[serde(rename = "config-version")]
    pub config_version: Option<i32>,
    #[serde(rename = "dbt-cloud")]
    pub dbt_cloud: Option<ProjectDbtCloudConfig>,
    /// `AdapterType` has no `JsonSchema` impl -- it lives in `dbt-adapter-core`,
    /// which has no `schemars` dependency and must not gain one -- so the schema
    /// describes the key as the string it is in YAML.
    #[schemars(with = "Option<std::collections::BTreeMap<String, AdapterProjectConfig>>")]
    pub adapters: Option<IndexMap<AdapterType, AdapterProjectConfig>>,
    pub dispatch: Option<Vec<_Dispatch>>,
    pub flags: Option<YmlValue>,
    #[serde(rename = "on-run-end")]
    pub on_run_end: Verbatim<Option<SpannedStringOrArrayOfStrings>>,
    #[serde(rename = "on-run-start")]
    pub on_run_start: Verbatim<Option<SpannedStringOrArrayOfStrings>>,
    #[serde(rename = "query-comment")]
    pub query_comment: Verbatim<Option<QueryComment>>,
    pub quoting: Spanned<Option<DbtQuoting>>,
    pub sync: Option<SyncConfig>,
    #[serde(rename = "require-dbt-version")]
    pub require_dbt_version: Option<StringOrArrayOfStrings>,
    #[serde(rename = "restrict-access")]
    pub restrict_access: Option<bool>,
    pub vars: Verbatim<Option<dbt_yaml::Value>>,
}

impl Default for DbtProject {
    fn default() -> Self {
        DbtProject {
            name: String::new(),
            version: None,
            profile: None,
            analysis_paths: None,
            asset_paths: None,
            macro_paths: None,
            model_paths: None,
            function_paths: None,
            seed_paths: None,
            snapshot_paths: None,
            test_paths: None,
            check_paths: None,
            docs_paths: None,
            skill_paths: None,
            target_path: None,
            log_path: None,
            packages_install_path: None,
            metrics: None,
            models: None,
            functions: None,
            snapshots: None,
            seeds: None,
            sources: None,
            checks: None,
            info_schema: None,
            tests: None,
            unit_tests: None,
            data_tests: None,
            exposures: None,
            analyses: None,
            skills: None,
            saved_queries: None,
            semantic_models: None,
            clean_targets: None,
            config_version: None,
            dbt_cloud: None,
            adapters: None,
            dispatch: None,
            flags: None,
            on_run_end: Verbatim::from(None),
            on_run_start: Verbatim::from(None),
            query_comment: Verbatim::from(None),
            quoting: Spanned::new(None),
            sync: None,
            require_dbt_version: None,
            restrict_access: None,
            vars: Verbatim::from(None),
        }
    }
}

impl DbtProject {
    pub fn get_project_id(&self) -> String {
        /*
        Returns the hash of the project name. Can be used for telemetry.
        */
        // TODO: do we really need cryptographic hashing here?
        format!("{:x}", md5::compute(self.name.as_bytes()))
    }

    /// Every configured resource path, in a fixed order.
    ///
    /// Unset path configs contribute nothing. `skill-paths` is deliberately
    /// excluded: skills are installed at `dbt deps` time, not parsed as nodes,
    /// so their directory must not participate in resource FQN computation
    /// (this feeds `strip_resource_paths_from_ref_path`).
    pub fn all_source_paths(&self) -> Vec<String> {
        [
            &self.model_paths,
            &self.function_paths,
            &self.seed_paths,
            &self.snapshot_paths,
            &self.analysis_paths,
            &self.macro_paths,
            &self.test_paths,
        ]
        .into_iter()
        .flatten()
        .flatten()
        .cloned()
        .collect()
    }

    /// `skill-paths`, falling back to the default of `skills/`.
    pub fn skill_paths_or_default(&self) -> Vec<String> {
        self.skill_paths
            .clone()
            .unwrap_or_else(|| vec![DEFAULT_SKILL_PATH.to_string()])
    }
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct _Dispatch {
    pub macro_namespace: String,
    pub search_order: Vec<String>,
}

/// Project-level config scoped to one adapter, keyed by adapter type:
///
/// ```yaml
/// adapters:
///   snowflake:
///     quoting:
///       identifier: false
/// ```
///
/// A map rather than a list because the key *is* the identity — a node selects an
/// adapter by type, so there is no separate name to carry and no way to declare
/// the same adapter twice.
///
/// Root-project only, read from the root project and ignored elsewhere exactly as
/// `dispatch:` is. Settings here belong to the project, not the connection, which
/// is why they are not in `profiles.yml`: that file is per-user and its concern is
/// connectivity, while identifier rendering is a project semantic that has to be
/// versioned and reviewed alongside the code depending on it.
#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, DbtSchema)]
pub struct AdapterProjectConfig {
    /// Identifier quoting for nodes running on this adapter. Overrides the
    /// top-level `quoting:` block; still overridden by `models:` and model-level
    /// `+quoting:`.
    pub quoting: Option<DbtQuoting>,
}

#[derive(UntaggedEnumDeserialize, Serialize, Debug, Clone, DbtSchema)]
#[serde(untagged)]
pub enum QueryComment {
    String(String),
    Object(YmlValue),
}

/// Common interface for configs that have completed the resolution pipeline.
///
/// Implemented by every `Resolved*Config` config type (generated via `#[derive(Resolvable)]`) and by
/// trivially-resolved configs that implement it directly. All required fields are guaranteed
/// to be set — `enabled()` always returns `bool`, never `Option<bool>`.
///
/// The optional capability methods (`get_pre_hook`, `get_post_hook`, `get_static_analysis`)
/// default to `None` and are overridden only by config types that have those fields.
pub trait ResolvedConfig {
    fn enabled(&self) -> bool;
    fn get_pre_hook(&self) -> Option<&crate::schemas::common::Hooks> {
        None
    }
    fn get_post_hook(&self) -> Option<&crate::schemas::common::Hooks> {
        None
    }
    fn get_static_analysis(&self) -> Option<Spanned<dbt_common::io_args::StaticAnalysisKind>> {
        None
    }
}

/// Full config resolution lifecycle protocol.
///
/// Implement this on every config type that participates in the resolution pipeline.
/// The four lifecycle steps, in order:
/// 1. `default_to` — inherit unset fields from a parent config of the same type
/// 2. `apply_package_defaults` — seed package-level values (e.g. quoting) once per package
/// 3. `apply_resolve_defaults` — fill in runtime values (e.g. CLI flags) after all layers merge
/// 4. `finalize` — consume self and produce `Self::Resolved`, the post-resolution type
///
/// For configs with fields that need `Option<T>` → `T` promotion in the resolved type, use
/// `#[derive(Resolvable)]` to generate the `Resolved*Config` struct and `finalize_resolved()` helper.
/// For trivially-resolved configs (no fields to promote), implement `ResolvedConfig` directly
/// on the struct and set `type Resolved = Self`.
pub trait ResolvableConfig<T>:
    Serialize + DeserializeOwned + Default + Debug + Clone + Send + Sync
{
    /// Post-resolution type returned by `finalize()`.
    ///
    /// For configs with promoted fields use the `#[derive(Resolvable)]`-generated `Resolved*Config` struct.
    /// For trivially-resolved configs set `type Resolved = Self` and impl `ResolvedConfig` directly.
    type Resolved: Send + Sync + ResolvedConfig + Clone;

    /// Values seeded into the root config before parent→child resolution within a package
    /// (e.g. quoting, sync). Applied once per package; cross-package defaults belong in `finalize`.
    /// Use `()` for configs that need no package-level seeding.
    type PackageDefaults;

    /// Runtime values applied after all layers are merged and the root overlay is applied, just
    /// before `finalize()`. Supplied via `ProjectConfigResolver::with_resolve_defaults`.
    ///
    /// Use `()` for configs that need no post-resolution defaults.
    /// Use `StaticAnalysisKind` for configs that carry a `static_analysis` field.
    ///
    /// For fields with a fixed compile-time default (i.e. not dependent on runtime inputs),
    /// prefer the `#[resolved(promote, default = expr)]` or `#[resolved(promote)]` macro
    /// attributes on the struct instead of implementing `apply_resolve_defaults`.
    type ResolveDefaults: Default + Clone + Send + Sync;

    fn default_to(&mut self, parent: &T);

    /// Returns whether this node is enabled, defaulting to `true` if unset.
    fn get_enabled_with_default(&self) -> bool;

    /// The explicitly-configured `enabled` value, or `None` when unset. Unlike
    /// `get_enabled_with_default`, this distinguishes unset from an explicit `true`.
    fn get_enabled(&self) -> Option<bool> {
        None
    }

    fn apply_package_defaults(&mut self, defaults: Self::PackageDefaults);

    /// Canonicalizes adapter config aliases for this layer, mirroring dbt-core's
    /// `credentials.translate_aliases` call in `_update_from_config`
    /// (`core/dbt/context/context_config.py:222`). Called once per config layer, before
    /// `default_to` merging, so an alias key in a less specific layer and its canonical key in a
    /// more specific one merge by ordinary precedence.
    ///
    /// Only covers aliases whose canonical field already exists as a typed field on `Self`
    /// (e.g. Databricks' `catalog` -> `database`): this hook runs on an already-typed `Self`, so
    /// it can move a value between two fields but cannot conjure a field for an alias that has
    /// none of its own (e.g. Databricks' `target_catalog`, or postgres/redshift's `dbname`).
    /// Those resolve only where a *raw* config-key rename is still possible before typing --
    /// currently just the inline `{{ config(...) }}` layer
    /// (`dbt_adapter_core::config_aliases::canonicalize_config_keys` in
    /// `ParseConfig::apply_config`) -- so the same alias authored in `dbt_project.yml` or a
    /// schema.yml `config:` block remains an unrecognized key. See
    /// `test_snapshot_target_catalog_in_project_yml_is_unrecognized_key` and
    /// `test_dbname_in_project_yml_is_unrecognized_key` (`dbt-parser/src/dbt_project_config.rs`)
    /// for the pinned, `#[ignore]`d gap.
    ///
    /// `default_adapter` is the *target's default* adapter, not the adapter this node runs on: a
    /// node's own `+adapter:` override is a mergeable field, so it is only readable once every
    /// layer has merged -- which is after this hook, by construction. `apply_resolve_defaults`
    /// below is the seam that can read it (`ModelConfig` does exactly that, as
    /// `self.adapter.or(default_adapter)`), but canonicalizing there would destroy the per-layer
    /// precedence this hook exists for. Pinned by
    /// `test_databricks_catalog_alias_not_canonicalized_for_adapter_overridden_node`
    /// (`dbt-parser/src/tests.rs`, `#[ignore]`d).
    fn canonicalize_adapter_aliases(&mut self, _default_adapter: AdapterType) {}

    /// Called after all config layers (project, properties, inline) are merged and the root
    /// overlay is applied, but before `finalize()`. Use this to fill in fields that must always
    /// have a value but are not set by `apply_package_defaults` for dependency packages.
    fn apply_resolve_defaults(&mut self, _defaults: Self::ResolveDefaults) {}

    /// Forces `enabled` to `false` unconditionally.
    fn disable(&mut self);

    /// Consumes self and returns the resolved type.
    fn finalize(self) -> Self::Resolved
    where
        Self: Sized;
}

/// Yaml configs that can contain nested child configs of the same type.
pub trait TypedRecursiveConfig: Clone {
    /// Returns the type name of the config, e.g., "model", "source", etc.
    fn type_name() -> &'static str;

    /// Returns an iterator over the child configs.
    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>>;

    /// Returns whether this level of the recursive config sets any config fields.
    /// This is just an approximation, since we can't reliably tell at this level if someone
    /// explicitly set a config field to its default.
    fn has_set_fields(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_project_id() {
        let project = DbtProject {
            name: "fishtown_internal_analytics".to_string(),
            version: Some(FloatOrString::String("1.0".to_string())),
            profile: Some("garage-snowflake".to_string()),
            analysis_paths: Some(vec![]),
            asset_paths: Some(vec![]),
            macro_paths: Some(vec![]),
            model_paths: Some(vec![]),
            function_paths: Some(vec![]),
            seed_paths: Some(vec![]),
            snapshot_paths: Some(vec![]),
            test_paths: Some(vec![]),
            check_paths: Some(vec![]),
            docs_paths: Some(vec![]),
            skill_paths: Some(vec![]),
            target_path: Some(TargetPath::Target),
            log_path: Some(LogPath::Logs),
            packages_install_path: Some("packages".to_string()),
            metrics: None,
            models: None,
            functions: None,
            snapshots: None,
            seeds: None,
            sources: None,
            checks: None,
            info_schema: None,
            tests: None,
            unit_tests: None,
            data_tests: None,
            saved_queries: None,
            semantic_models: None,
            exposures: None,
            analyses: None,
            skills: None,
            clean_targets: None,
            config_version: None,
            dbt_cloud: None,
            adapters: None,
            dispatch: None,
            flags: None,
            on_run_end: Verbatim::from(None),
            on_run_start: Verbatim::from(None),
            query_comment: Verbatim::from(None),
            quoting: Spanned::new(None),
            sync: None,
            require_dbt_version: None,
            restrict_access: None,
            vars: Verbatim::from(None),
        };
        assert_eq!(project.get_project_id(), "92c907bdbc0c4f27451b9b9fdb1bc8ec");
    }

    /// `adapters:` is keyed by adapter type, because the key *is* the identity —
    /// a node selects an adapter by type, so there is no separate name to carry.
    #[test]
    fn project_parses_the_adapters_block() {
        let project: DbtProject = dbt_yaml::from_str(
            r#"
name: test
quoting:
  database: false
  schema: false
  identifier: false
adapters:
  lakecompute:
    quoting:
      database: true
      schema: true
      identifier: true
  snowflake: {}
"#,
        )
        .expect("adapters block should parse");

        let adapters = project.adapters.expect("adapters");
        assert_eq!(
            adapters.keys().copied().collect::<Vec<_>>(),
            vec![AdapterType::LakeCompute, AdapterType::Snowflake],
            "declaration order is preserved"
        );
        assert_eq!(
            adapters[&AdapterType::LakeCompute]
                .quoting
                .expect("lakecompute quoting")
                .identifier,
            Some(true)
        );
        assert!(
            adapters[&AdapterType::Snowflake].quoting.is_none(),
            "an entry may omit `quoting:` entirely"
        );
        assert_eq!(
            project.quoting.as_ref().expect("top-level").identifier,
            Some(false),
            "the top-level block is unaffected"
        );
    }

    /// `lakecompute` is the only name for `AdapterType::LakeCompute`. `alt` was the
    /// external name before the rename and is not kept as an alias, so it has to
    /// be rejected here like any other unknown adapter.
    #[test]
    fn the_adapters_block_rejects_the_retired_alt_name() {
        let result: Result<DbtProject, _> = dbt_yaml::from_str(
            r#"
name: test
adapters:
  alt: {}
"#,
        );

        assert!(result.is_err(), "`alt` is not an adapter type any more");
    }

    /// A key that is not an adapter type is rejected at deserialization, against
    /// the full set of supported adapters -- so there is no bespoke validation for
    /// it, and no way to name an adapter that cannot exist.
    #[test]
    fn an_adapters_key_that_is_not_an_adapter_type_is_rejected() {
        let err = dbt_yaml::from_str::<DbtProject>(
            r#"
name: test
adapters:
  warehouse:
    quoting:
      database: true
"#,
        )
        .expect_err("`warehouse` is not an adapter type");
        assert!(
            format!("{err}").contains("warehouse"),
            "error should name the offending key: {err}"
        );
    }

    #[test]
    fn project_dbt_cloud_config_accepts_state_org_id() {
        let project: DbtProject = dbt_yaml::from_str(
            r#"
name: test
dbt-cloud:
  project-id: 123
  defer-env-id: 456
  state-org-id: 789
"#,
        )
        .unwrap();

        let dbt_cloud = project.dbt_cloud.expect("dbt-cloud config");
        assert_eq!(dbt_cloud.state_org_id, Some(StringOrInteger::Integer(789)));
    }

    /// Regression for fs#13343: Core accepts the legacy `data-paths` key as an
    /// alias for `seed-paths`; Fusion must not reject it during YAML load.
    #[test]
    fn project_accepts_legacy_data_paths_as_seed_paths_alias() {
        let project: DbtProject = dbt_yaml::from_str(
            r#"
name: test
data-paths: ["data"]
"#,
        )
        .unwrap();

        assert_eq!(project.seed_paths, Some(vec!["data".to_string()]));
    }

    /// Regression for fs#13343: the pre-profile-load `DbtProjectSimplified` pass
    /// must not reject a `data-paths` project either (it no longer tracks the
    /// key at all — the alias is resolved on the full `DbtProject` above).
    #[test]
    fn simplified_project_ignores_legacy_data_paths() {
        let project: DbtProjectSimplified = dbt_yaml::from_str(
            r#"
name: test
data-paths: ["data"]
__ignored__: {}
"#,
        )
        .unwrap();

        assert!(project.source_paths.is_none());
    }

    #[test]
    fn project_id_span_points_at_the_project_id_line() {
        let project: DbtProject = dbt_yaml::from_str(
            r#"
name: test
dbt-cloud:
  project-id: 123
  defer-env-id: 456
"#,
        )
        .unwrap();

        let dbt_cloud = project.dbt_cloud.expect("dbt-cloud config");
        assert_eq!(dbt_cloud.project_id_str().as_deref(), Some("123"));
        assert_eq!(dbt_cloud.project_id.span().start.line, 4);
    }

    #[test]
    fn project_dbt_cloud_config_without_project_id() {
        let project: DbtProject = dbt_yaml::from_str(
            r#"
name: test
dbt-cloud:
  defer-env-id: 456
"#,
        )
        .unwrap();

        let dbt_cloud = project.dbt_cloud.expect("dbt-cloud config");
        assert!(dbt_cloud.project_id_str().is_none());
    }

    #[test]
    fn project_schema_includes_state_org_id() {
        use crate::man::deny_additional_properties_in_root;

        fn has_property(value: &serde_json::Value, property: &str) -> bool {
            match value {
                serde_json::Value::Object(map) => {
                    map.get("properties")
                        .and_then(serde_json::Value::as_object)
                        .is_some_and(|properties| properties.contains_key(property))
                        || map.values().any(|value| has_property(value, property))
                }
                serde_json::Value::Array(values) => {
                    values.iter().any(|value| has_property(value, property))
                }
                _ => false,
            }
        }

        let generator = schemars::r#gen::SchemaSettings::draft07().into_generator();
        let mut schema = generator.into_root_schema_for::<DbtProject>();
        deny_additional_properties_in_root(&mut schema);
        let schema_json = serde_json::to_value(&schema).unwrap();

        assert!(
            has_property(&schema_json, "state-org-id"),
            "state-org-id should be a property in the dbt-cloud project config schema"
        );
    }
}
