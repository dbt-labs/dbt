use crate::schemas::serde::OmissibleGrantConfig;
use dbt_adapter_core::AdapterType;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::serde_utils::Omissible;
use dbt_yaml::DbtSchema;
use dbt_yaml::ShouldBe;
use dbt_yaml::Spanned;
use dbt_yaml::Verbatim;
use serde::{Deserialize, Serialize};
// Type aliases for clarity
type YmlValue = dbt_yaml::Value;
use indexmap::IndexMap;
use serde_with::skip_serializing_none;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::collections::btree_map::Iter;

use super::config_keys::ConfigKeys;

use dbt_proc_macros::{DefaultTo, Resolvable};

use crate::schemas::common::DocsConfig;
use crate::schemas::common::{Access, DbtQuoting, Hooks, hooks_equal, serialize_hooks_as_list};
use crate::schemas::project::configs::common::log_state_mod_diff;
// Import comparison helpers from common
use super::common::{
    access_eq, array_of_strings_eq, docs_eq, grants_equal, meta_eq, omissible_option_eq,
    same_warehouse_config, take_databricks_catalog_alias,
};
use crate::schemas::project::configs::common::WarehouseSpecificNodeConfig;
use crate::schemas::project::configs::config_merge::{Packages, Tags};
use crate::schemas::project::dbt_project::{
    ResolvableConfig, ResolvedConfig, TypedRecursiveConfig,
};
use crate::schemas::properties::{FunctionKind, Volatility};
use crate::schemas::serde::{bool_or_string_bool, default_type};

fn default_function_kind() -> Option<FunctionKind> {
    Some(FunctionKind::Scalar)
}

/// Snowflake-specific configuration for functions. Nested under the `snowflake`
/// key inside `config:` in schema YAML (or `+snowflake:` under `functions:` in
/// `dbt_project.yml`).
#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq, Eq, DbtSchema)]
pub struct FunctionSnowflakeConfig {
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub quote_args: Option<bool>,
}

#[skip_serializing_none]
#[derive(DefaultTo, Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ProjectFunctionConfig {
    #[serde(rename = "+access")]
    pub access: Option<Access>,
    #[serde(rename = "+adapter")]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    #[serde(rename = "+alias")]
    pub alias: Option<String>,
    #[serde(rename = "+database", alias = "+project")]
    pub database: Omissible<Option<String>>,
    #[serde(rename = "+description")]
    pub description: Option<String>,
    #[serde(rename = "+docs")]
    pub docs: Option<DocsConfig>,
    #[serde(default, rename = "+enabled", deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    #[serde(rename = "+grants")]
    pub grants: OmissibleGrantConfig,
    #[serde(rename = "+group")]
    pub group: Option<String>,
    #[serde(rename = "+meta")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    #[serde(rename = "+on_configuration_change")]
    pub on_configuration_change: Option<String>,
    // dbt-core's `FunctionConfig` extends `NodeConfig`, so functions carry hooks like any
    // other node; both spellings are accepted (see `translate_hook_names`).
    #[serde(rename = "+post-hook", alias = "+post_hook")]
    pub post_hook: Verbatim<Option<Hooks>>,
    #[serde(rename = "+pre-hook", alias = "+pre_hook")]
    pub pre_hook: Verbatim<Option<Hooks>>,
    #[serde(rename = "+quoting")]
    pub quoting: Option<DbtQuoting>,
    #[serde(rename = "+schema")]
    pub schema: Omissible<Option<String>>,
    #[serde(rename = "+static_analysis")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    #[serde(rename = "+tags")]
    pub tags: Tags,
    #[serde(rename = "+type")]
    pub function_kind: Option<FunctionKind>,
    #[serde(rename = "+volatility")]
    pub volatility: Option<Volatility>,
    #[serde(rename = "+runtime_version")]
    pub runtime_version: Option<String>,
    #[serde(rename = "+entry_point")]
    pub entry_point: Option<String>,
    #[serde(rename = "+packages")]
    pub packages: Packages,
    #[serde(rename = "+snowflake")]
    pub snowflake: Option<FunctionSnowflakeConfig>,

    // Additional properties for directory structure
    #[default_to(skip)]
    pub __additional_properties__: BTreeMap<String, ShouldBe<ProjectFunctionConfig>>,
}

impl Default for ProjectFunctionConfig {
    fn default() -> Self {
        Self {
            access: None,
            adapter: None,
            alias: None,
            database: Omissible::Omitted,
            description: None,
            docs: None,
            enabled: None,
            grants: OmissibleGrantConfig::default(),
            group: None,
            meta: None,
            on_configuration_change: None,
            post_hook: Verbatim::from(None),
            pre_hook: Verbatim::from(None),
            quoting: None,
            schema: Omissible::Omitted,
            static_analysis: None,
            tags: Tags::default(),
            function_kind: None,
            volatility: None,
            runtime_version: None,
            entry_point: None,
            packages: Packages::default(),
            snowflake: None,
            __additional_properties__: BTreeMap::new(),
        }
    }
}

impl ResolvedConfig for ProjectFunctionConfig {
    fn enabled(&self) -> bool {
        true
    }
}

impl ResolvableConfig<ProjectFunctionConfig> for ProjectFunctionConfig {
    type Resolved = Self;
    type PackageDefaults = ();
    type ResolveDefaults = ();

    fn get_enabled_with_default(&self) -> bool {
        true
    }

    fn disable(&mut self) {}

    fn apply_package_defaults(&mut self, _: ()) {}

    fn finalize(self) -> Self {
        self
    }

    fn default_to(&mut self, parent: &ProjectFunctionConfig) {
        self.default_to_fields(parent);
    }
}

impl TypedRecursiveConfig for ProjectFunctionConfig {
    fn type_name() -> &'static str {
        "function"
    }

    fn iter_children(&self) -> Iter<'_, String, ShouldBe<Self>> {
        self.__additional_properties__.iter()
    }

    fn has_set_fields(&self) -> bool {
        self.access.is_some()
            || self.adapter.is_some()
            || self.alias.is_some()
            || self.database.is_present()
            || self.description.is_some()
            || self.docs.is_some()
            || self.enabled.is_some()
            || self.grants.0.is_present()
            || self.group.is_some()
            || self.meta.is_some()
            || self.on_configuration_change.is_some()
            || self.post_hook.is_some()
            || self.pre_hook.is_some()
            || self.quoting.is_some()
            || self.schema.is_present()
            || self.static_analysis.is_some()
            || self.tags.is_some()
            || self.function_kind.is_some()
            || self.volatility.is_some()
            || self.runtime_version.is_some()
            || self.entry_point.is_some()
            || self.packages.is_some()
            || self.snowflake.is_some()
    }
}

#[skip_serializing_none]
#[derive(
    Resolvable, DefaultTo, Debug, Clone, Serialize, Deserialize, Default, PartialEq, DbtSchema,
)]
#[serde(rename_all = "snake_case")]
pub struct FunctionConfig {
    pub access: Option<Access>,
    // Internal placement hint; kept out of serialized config/telemetry output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub adapter: Option<AdapterType>,
    #[resolved(promote, method = get_enabled_with_default)]
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub enabled: Option<bool>,
    pub alias: Option<String>,
    pub database: Omissible<Option<String>>,
    pub schema: Omissible<Option<String>>,
    #[serde(default)]
    pub tags: Tags,
    // need default to ensure None if field is not set
    #[serde(default, deserialize_with = "default_type")]
    pub meta: Option<IndexMap<String, YmlValue>>,
    pub group: Option<String>,
    pub docs: Option<DocsConfig>,
    pub grants: OmissibleGrantConfig,
    // Unlike models/seeds/snapshots, functions have no separate `Manifest*Config`: this
    // struct is what `ManifestFunction` serializes. So the field carries the manifest key
    // (`pre-hook`) with the authored spelling as an alias, and normalizes to `List[Hook]`
    // on the way out — `Verbatim` keeps the flexible authored input shapes.
    #[serde(
        rename = "post-hook",
        alias = "post_hook",
        default,
        serialize_with = "serialize_hooks_as_list"
    )]
    pub post_hook: Verbatim<Option<Hooks>>,
    #[serde(
        rename = "pre-hook",
        alias = "pre_hook",
        default,
        serialize_with = "serialize_hooks_as_list"
    )]
    pub pre_hook: Verbatim<Option<Hooks>>,
    #[resolved(promote, expect = "quoting set by apply_package_defaults")]
    pub quoting: Option<DbtQuoting>,
    pub on_configuration_change: Option<String>,
    #[resolved(promote, expect = "static_analysis set by apply_resolve_defaults")]
    pub static_analysis: Option<Spanned<StaticAnalysisKind>>,
    #[serde(default = "default_function_kind", rename = "type")]
    pub function_kind: Option<FunctionKind>,
    pub volatility: Option<Volatility>,
    pub runtime_version: Option<String>,
    pub entry_point: Option<String>,
    pub packages: Packages,
    pub snowflake: Option<FunctionSnowflakeConfig>,

    // Warehouse-specific configurations
    pub __warehouse_specific_config__: WarehouseSpecificNodeConfig,
}

impl ResolvableConfig<FunctionConfig> for FunctionConfig {
    type Resolved = ResolvedFunctionConfig;
    type PackageDefaults = DbtQuoting;
    type ResolveDefaults = StaticAnalysisKind;

    fn get_enabled_with_default(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn get_enabled(&self) -> Option<bool> {
        self.enabled
    }

    fn disable(&mut self) {
        self.enabled = Some(false);
    }

    fn apply_package_defaults(&mut self, quoting: DbtQuoting) {
        if self.quoting.is_none() {
            self.quoting = Some(quoting);
        }
    }

    fn apply_resolve_defaults(&mut self, static_analysis: StaticAnalysisKind) {
        if self.static_analysis.is_none() {
            self.static_analysis = Some(Spanned::new(static_analysis));
        }
    }

    fn finalize(self) -> ResolvedFunctionConfig {
        self.finalize_resolved()
    }

    fn default_to(&mut self, parent: &FunctionConfig) {
        self.default_to_fields(parent);
    }

    fn canonicalize_adapter_aliases(&mut self, default_adapter: AdapterType) {
        if let Some(catalog) = take_databricks_catalog_alias(
            default_adapter,
            &mut self.__warehouse_specific_config__,
            !self.database.is_omitted(),
        ) {
            self.database = Omissible::Present(Some(catalog));
        }
        // BigQuery's `project`/`dataset` aliases are already routed to `database`/`schema` by
        // the pre-existing, ungated serde `alias`es on those fields (D1); nothing to do here.
    }
}

impl From<ProjectFunctionConfig> for FunctionConfig {
    fn from(config: ProjectFunctionConfig) -> Self {
        Self {
            access: config.access,
            adapter: config.adapter,
            enabled: config.enabled,
            alias: config.alias,
            database: config.database,
            schema: config.schema,
            tags: config.tags,
            meta: config.meta,
            group: config.group,
            docs: config.docs,
            grants: config.grants,
            post_hook: config.post_hook,
            pre_hook: config.pre_hook,
            quoting: config.quoting,
            on_configuration_change: config.on_configuration_change,
            static_analysis: config.static_analysis,
            function_kind: config.function_kind,
            volatility: config.volatility,
            runtime_version: config.runtime_version,
            entry_point: config.entry_point,
            packages: config.packages,
            snowflake: config.snowflake,
            __warehouse_specific_config__: WarehouseSpecificNodeConfig::default(),
        }
    }
}

impl FunctionConfig {
    /// Custom comparison that treats Omitted and Present(None) as equivalent for schema/database fields
    ///
    pub fn same_config(&self, other: &FunctionConfig) -> bool {
        // Compare all fields individually
        let enabled_eq = self.enabled == other.enabled;
        let adapter_eq = self.adapter == other.adapter;
        let alias_eq = self.alias == other.alias;
        let schema_eq = omissible_option_eq(&self.schema, &other.schema); // Custom comparison for Omissible
        let meta_eq_result = meta_eq(&self.meta, &other.meta); // Custom comparison for meta
        let docs_eq_result = docs_eq(&self.docs, &other.docs); // Custom comparison for docs
        let grants_eq = grants_equal(&self.grants, &other.grants); // Custom comparison for grants
        let quoting_eq = self.quoting == other.quoting;
        let on_configuration_change_eq =
            self.on_configuration_change == other.on_configuration_change;
        let post_hook_eq = hooks_equal(&self.post_hook, &other.post_hook);
        let pre_hook_eq = hooks_equal(&self.pre_hook, &other.pre_hook);
        // `static_analysis` is a Fusion-only, invocation-driven value (e.g. set by
        // `--static-analysis`) with no dbt-core equivalent, so it can never be a
        // legitimate dbt-core `state:modified` trigger and is deliberately excluded
        // from this comparison (see `base_config_excluded_keys`, parity-exclude).
        let function_kind_eq = self.function_kind == other.function_kind;
        let volatility_eq = self.volatility == other.volatility;
        let access_eq_result = access_eq(&self.access, &other.access); // Custom comparison for access
        let packages_eq = array_of_strings_eq(self.packages.inner(), other.packages.inner());
        let snowflake_eq = self.snowflake == other.snowflake;
        let warehouse_config_eq = same_warehouse_config(
            &self.__warehouse_specific_config__,
            &other.__warehouse_specific_config__,
        );
        // `tags` and `group` are intentionally NOT compared here: they are dbt-core `CompareBehavior.Exclude`
        // fields (see `base_config_excluded_keys` in prev_state/mod.rs) and dbt-core does not treat them
        // as a config modification anywhere, so this rendered fallback comparator must not either.

        let result = enabled_eq
            && adapter_eq
            && alias_eq
            && schema_eq
            && meta_eq_result
            && docs_eq_result
            && grants_eq
            && quoting_eq
            && on_configuration_change_eq
            && post_hook_eq
            && pre_hook_eq
            && function_kind_eq
            && volatility_eq
            && access_eq_result
            && packages_eq
            && snowflake_eq
            && warehouse_config_eq;

        if !result {
            log_state_mod_diff(
                "unique_id in next function_config log",
                "function_config",
                [
                    (
                        "enabled",
                        enabled_eq,
                        Some((
                            format!("{:?}", &self.enabled),
                            format!("{:?}", &other.enabled),
                        )),
                    ),
                    (
                        "adapter",
                        adapter_eq,
                        Some((
                            format!("{:?}", &self.adapter),
                            format!("{:?}", &other.adapter),
                        )),
                    ),
                    (
                        "alias",
                        alias_eq,
                        Some((format!("{:?}", &self.alias), format!("{:?}", &other.alias))),
                    ),
                    (
                        "schema",
                        schema_eq,
                        Some((
                            format!("{:?}", &self.schema),
                            format!("{:?}", &other.schema),
                        )),
                    ),
                    (
                        "meta",
                        meta_eq_result,
                        Some((format!("{:?}", &self.meta), format!("{:?}", &other.meta))),
                    ),
                    ("docs", docs_eq_result, None),
                    (
                        "grants",
                        grants_eq,
                        Some((
                            format!("{:?}", &self.grants),
                            format!("{:?}", &other.grants),
                        )),
                    ),
                    (
                        "quoting",
                        quoting_eq,
                        Some((
                            format!("{:?}", &self.quoting),
                            format!("{:?}", &other.quoting),
                        )),
                    ),
                    (
                        "on_configuration_change",
                        on_configuration_change_eq,
                        Some((
                            format!("{:?}", &self.on_configuration_change),
                            format!("{:?}", &other.on_configuration_change),
                        )),
                    ),
                    (
                        "post_hook",
                        post_hook_eq,
                        Some((
                            format!("{:?}", &self.post_hook),
                            format!("{:?}", &other.post_hook),
                        )),
                    ),
                    (
                        "pre_hook",
                        pre_hook_eq,
                        Some((
                            format!("{:?}", &self.pre_hook),
                            format!("{:?}", &other.pre_hook),
                        )),
                    ),
                    (
                        "function_kind",
                        function_kind_eq,
                        Some((
                            format!("{:?}", &self.function_kind),
                            format!("{:?}", &other.function_kind),
                        )),
                    ),
                    (
                        "volatility",
                        volatility_eq,
                        Some((
                            format!("{:?}", &self.volatility),
                            format!("{:?}", &other.volatility),
                        )),
                    ),
                    (
                        "access",
                        access_eq_result,
                        Some((
                            format!("{:?}", &self.access),
                            format!("{:?}", &other.access),
                        )),
                    ),
                    (
                        "packages",
                        packages_eq,
                        Some((
                            format!("{:?}", &self.packages),
                            format!("{:?}", &other.packages),
                        )),
                    ),
                    (
                        "snowflake",
                        snowflake_eq,
                        Some((
                            format!("{:?}", &self.snowflake),
                            format!("{:?}", &other.snowflake),
                        )),
                    ),
                    ("warehouse_config", warehouse_config_eq, None),
                ],
            );
        }

        result
    }
}

impl ConfigKeys for FunctionConfig {
    fn valid_field_names() -> HashSet<String> {
        let serialized = dbt_yaml::to_value(Self::default())
            .expect("Failed to serialize FunctionConfig for field extraction");

        let mut field_names = HashSet::new();
        if let YmlValue::Mapping(map, _) = serialized {
            for (key, _) in map {
                if let YmlValue::String(key_str, _) = key {
                    field_names.insert(key_str);
                }
            }
        }

        // Serializing a default instance only yields the canonical spellings
        // (`pre-hook`/`post-hook`); the aliases have to be listed explicitly.
        field_names.insert("pre_hook".to_string());
        field_names.insert("post_hook".to_string());

        field_names
    }
}

impl crate::schemas::project::configs::warehouse_scope::WarehouseConfigResource for FunctionConfig {
    const NODE_TYPE: dbt_telemetry::NodeType = dbt_telemetry::NodeType::Function;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::project::configs::config_merge::Packages;
    use crate::schemas::project::dbt_project::ResolvableConfig;
    use crate::schemas::serde::StringOrArrayOfStrings;

    #[test]
    fn test_function_config_packages_append() {
        let parent = FunctionConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
                "pandas".to_string(),
            ]))),
            ..Default::default()
        };

        let mut child = FunctionConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "matplotlib".to_string(),
            ]))),
            ..Default::default()
        };

        child.default_to(&parent);

        assert_eq!(
            child.packages,
            Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
                "pandas".to_string(),
                "matplotlib".to_string(),
            ])))
        );
    }

    #[test]
    fn test_function_config_packages_none_child_inherits_parent() {
        let parent = FunctionConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
            ]))),
            ..Default::default()
        };

        let mut child = FunctionConfig {
            packages: Packages::default(),
            ..Default::default()
        };

        child.default_to(&parent);

        assert_eq!(
            child.packages,
            Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
            ])))
        );
    }

    #[test]
    fn test_function_config_packages_same_config() {
        let a = FunctionConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
            ]))),
            ..Default::default()
        };

        let b = FunctionConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "numpy".to_string(),
            ]))),
            ..Default::default()
        };

        assert!(a.same_config(&b));

        let c = FunctionConfig {
            packages: Packages(Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "pandas".to_string(),
            ]))),
            ..Default::default()
        };

        assert!(!a.same_config(&c));
    }

    /// `+adapter` names an adapter *type*, so the value is typed rather than a
    /// free string -- anything that is not a supported adapter fails here, at
    /// deserialization. Mirrors the seed and model cases.
    #[test]
    fn test_project_function_config_adapter_parses_and_round_trips() {
        let project_config: ProjectFunctionConfig = dbt_yaml::from_str(
            r#"
+adapter: bigquery
__additional_properties__: {}
"#,
        )
        .unwrap();
        assert_eq!(project_config.adapter, Some(AdapterType::Bigquery));

        let config: FunctionConfig = project_config.into();
        assert_eq!(config.adapter, Some(AdapterType::Bigquery));
    }

    #[test]
    fn test_project_function_config_rejects_a_value_that_is_not_an_adapter() {
        let err = dbt_yaml::from_str::<ProjectFunctionConfig>(
            r#"
+adapter: compute
__additional_properties__: {}
"#,
        )
        .expect_err("`compute` is not an adapter type");
        assert!(
            format!("{err}").contains("compute"),
            "error should name the offending value: {err}"
        );
    }
}
