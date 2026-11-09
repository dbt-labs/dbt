use crate::schemas::project::TypedRecursiveConfig;
use dbt_proc_macros::Resolvable;
use dbt_yaml::{DbtSchema, ShouldBe};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::btree_map::Iter;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use crate::schemas::common::Severity;
use crate::schemas::project::configs::config_merge::Tags;
use crate::schemas::{
    project::ResolvableConfig,
    serde::{StringOrArrayOfStrings, bool_or_string_bool},
};
use dbt_proc_macros::DefaultTo;

/// Checks default to failing the run, matching data tests.
pub const DEFAULT_CHECK_SEVERITY: Severity = Severity::Error;

/// The only accepted value today. Required precisely so that a future v2 is an explicit
/// per-package migration (see [`InfoSchemaConfig`]), not a silent behavior change for every
/// check that never mentioned a version.
pub const SUPPORTED_INFO_SCHEMA_VERSIONS: &[u32] = &[1];

/// Project-level `info_schema:` block.
///
/// Package-scoped, not resolved/inherited the way `checks:` is: each package -- root or
/// dependency -- declares its own `version` directly in its own `dbt_project.yml`, so a check in
/// the root project can read a newer `info_schema()` shape while an installed package (e.g.
/// `dbt_project_evaluator`) keeps depending on an older one, and every check within a single
/// package queries one consistent version. See `SUPPORTED_INFO_SCHEMA_VERSIONS`.
#[derive(Deserialize, Serialize, Debug, Clone, Default, DbtSchema, PartialEq)]
pub struct InfoSchemaConfig {
    pub version: Option<u32>,
}

/// Which output column(s) node selection scopes a check's rows by.
///
/// `"none"` opts out of scoping entirely (the check always evaluates the whole project, which is
/// what aggregate checks want). A single name or a list of names scopes on those output columns:
/// a row is kept when the node id in *any* of them is selected. Unset defaults to the `unique_id`
/// column when the check outputs one.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, DbtSchema)]
#[serde(untagged)]
pub enum SelectionFilterOn {
    One(String),
    Many(Vec<String>),
}

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq)]
pub struct ProjectCheckConfig {
    #[serde(rename = "+meta")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+tags")]
    pub tags: Option<StringOrArrayOfStrings>,
    #[serde(default, rename = "+enabled", deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(rename = "+severity")]
    pub severity: Option<Severity>,
    #[serde(rename = "+selection_filter_on")]
    pub selection_filter_on: Option<SelectionFilterOn>,
    pub __additional_properties__: BTreeMap<String, ShouldBe<ProjectCheckConfig>>,
}

impl TypedRecursiveConfig for ProjectCheckConfig {
    fn type_name() -> &'static str {
        "check"
    }

    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>> {
        self.__additional_properties__.iter()
    }

    fn has_set_fields(&self) -> bool {
        self.meta.is_some()
            || self.tags.is_some()
            || self.enabled.is_some()
            || self.severity.is_some()
            || self.selection_filter_on.is_some()
    }
}

// NOTE: No #[skip_serializing_none] - we handle None serialization in serialize_with_mode
#[derive(
    Resolvable, DefaultTo, Deserialize, Serialize, Debug, Clone, Default, DbtSchema, PartialEq,
)]
pub struct CheckConfig {
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(serialize_with = "crate::schemas::serde::serialize_none_as_empty_map")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(default)]
    pub tags: Tags,
    #[resolved(promote, default = DEFAULT_CHECK_SEVERITY.clone())]
    pub severity: Option<Severity>,
    pub selection_filter_on: Option<SelectionFilterOn>,
}

impl From<ProjectCheckConfig> for CheckConfig {
    fn from(config: ProjectCheckConfig) -> Self {
        Self {
            enabled: config.enabled,
            meta: config.meta,
            tags: Tags(config.tags),
            severity: config.severity,
            selection_filter_on: config.selection_filter_on,
        }
    }
}

impl From<CheckConfig> for ProjectCheckConfig {
    fn from(config: CheckConfig) -> Self {
        Self {
            meta: config.meta,
            tags: config.tags.into_inner(),
            enabled: config.enabled,
            severity: config.severity,
            selection_filter_on: config.selection_filter_on,
            __additional_properties__: BTreeMap::new(),
        }
    }
}

impl ResolvableConfig<CheckConfig> for CheckConfig {
    type Resolved = ResolvedCheckConfig;
    type PackageDefaults = ();
    type ResolveDefaults = ();

    fn get_enabled_with_default(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn disable(&mut self) {
        self.enabled = Some(false);
    }

    fn apply_package_defaults(&mut self, _: ()) {}

    fn finalize(self) -> ResolvedCheckConfig {
        self.finalize_resolved()
    }

    fn default_to(&mut self, parent: &CheckConfig) {
        self.default_to_fields(parent);
    }
}
