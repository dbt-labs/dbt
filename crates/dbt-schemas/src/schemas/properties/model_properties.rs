use crate::schemas::common::ConstraintType;
use crate::schemas::common::DimensionValidityParams;
use crate::schemas::common::UpdatesOn;
use crate::schemas::common::Versions;
use crate::schemas::common::model_freshness_rules_or_duration;
use crate::schemas::common::{FreshnessRules, ModelFreshnessRules};
use crate::schemas::data_tests::DataTests;
use crate::schemas::dbt_column::ColumnProperties;
use crate::schemas::dbt_column::ColumnPropertiesDimensionType;
use crate::schemas::dbt_column::ColumnPropertiesEntityType;
use crate::schemas::dbt_column::Granularity;
use crate::schemas::project::ModelConfig;
use crate::schemas::project::ResolvableConfig;
use crate::schemas::project::SemanticModelConfig;
use crate::schemas::project::configs::config_merge::DefaultTo;
use crate::schemas::project::configs::semantic_model_config::ResolvedSemanticModelConfig;
use crate::schemas::properties::MetricsProperties;
use crate::schemas::properties::properties::GetConfig;
use crate::schemas::semantic_layer::semantic_manifest::SemanticLayerElementConfig;
use crate::schemas::serde::FloatOrString;
use crate::schemas::serde::{bool_or_string_bool, string_or_array};
use dbt_common::io_args::StaticAnalysisOffReason;
use dbt_yaml::{DbtSchema, Spanned};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// Model level contraint
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct ModelConstraint {
    #[serde(rename = "type")]
    pub type_: ConstraintType,
    pub expression: Option<String>,
    pub name: Option<String>,
    // Only ForeignKey constraints accept: a relation input
    // ref(), source() etc
    pub to: Option<Spanned<String>>,
    /// Only ForeignKey constraints accept: a list columns in that table
    /// containing the corresponding primary or unique key.
    #[serde(
        default,
        deserialize_with = "string_or_array",
        serialize_with = "crate::schemas::serde::serialize_none_as_empty_vec"
    )]
    pub to_columns: Option<Vec<String>>,
    #[serde(default, deserialize_with = "string_or_array")]
    pub columns: Option<Vec<String>>,
    pub warn_unsupported: Option<bool>,
    pub warn_unenforced: Option<bool>,
}
// todo: consider revising this design: warn_unsupported, warn_unenforced are adapter specific constraint. You don't want to specify them on all models!

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ModelProperties {
    pub columns: Option<Vec<ColumnProperties>>,
    pub config: Option<ModelConfig>,
    pub constraints: Option<Vec<ModelConstraint>>,
    pub data_tests: Option<Vec<DataTests>>,
    pub deprecation_date: Option<String>,
    pub description: Option<String>,
    pub identifier: Option<String>,
    pub latest_version: Option<FloatOrString>,
    pub name: String,
    #[serde(skip_deserializing, default)]
    pub static_analysis_off_reason: Option<StaticAnalysisOffReason>,
    pub tests: Option<Vec<DataTests>>,
    pub time_spine: Option<ModelPropertiesTimeSpine>,
    pub versions: Option<Vec<Versions>>,

    pub semantic_model: Option<ModelPropertiesSemanticModelConfig>,
    pub agg_time_dimension: Option<String>,
    pub metrics: Option<Vec<MetricsProperties>>,
    pub derived_semantics: Option<DerivedSemantics>,
    pub primary_entity: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, Default)]
pub struct ModelPropertiesSemanticModelConfig {
    pub enabled: bool,
    pub name: Option<String>,
    pub group: Option<String>,
    pub config: Option<SemanticLayerElementConfig>,
}

impl ResolvableConfig<SemanticModelConfig> for ModelPropertiesSemanticModelConfig {
    type Resolved = ResolvedSemanticModelConfig;
    type PackageDefaults = ();
    type ResolveDefaults = ();

    fn get_enabled_with_default(&self) -> bool {
        self.enabled
    }

    fn disable(&mut self) {
        self.enabled = false;
    }

    fn apply_package_defaults(&mut self, _: ()) {}

    fn finalize(self) -> ResolvedSemanticModelConfig {
        unreachable!("ModelPropertiesSemanticModelConfig is never finalized directly")
    }

    fn default_to(&mut self, parent: &SemanticModelConfig) {
        DefaultTo::inherit_from(&mut self.group, &parent.group);
    }
}

impl ModelProperties {
    pub fn empty(name: String) -> Self {
        Self {
            name,
            columns: None,
            config: None,
            constraints: None,
            data_tests: None,
            deprecation_date: None,
            description: None,
            identifier: None,
            latest_version: None,
            static_analysis_off_reason: None,
            tests: None,
            time_spine: None,
            versions: None,
            semantic_model: None,
            agg_time_dimension: None,
            metrics: None,
            derived_semantics: None,
            primary_entity: None,
        }
    }
}

impl GetConfig<ModelConfig> for ModelProperties {
    fn get_config(&self) -> Option<&ModelConfig> {
        self.config.as_ref()
    }
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ModelPropertiesTimeSpine {
    pub custom_granularities: Option<Vec<TimeSpineCustomGranularity>>,
    pub standard_granularity_column: String,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct TimeSpineCustomGranularity {
    pub column_name: Option<String>,
    pub name: String,
}

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
pub struct ModelFreshness {
    pub build_after: Option<ModelFreshnessRules>,
    pub warn_after: Option<FreshnessRules>,
    pub error_after: Option<FreshnessRules>,
    pub filter: Option<String>,
    pub loaded_at_field: Option<String>,
    pub loaded_at_query: Option<String>,
}

impl ModelFreshness {
    /// True when SLA rules are set. Excludes `build_after`, a scheduling rule.
    ///
    /// An empty rule object (`warn_after: {}`) counts as absent, matching
    /// `FreshnessRules::validate`'s F1 rule that an empty rule is equivalent to
    /// omitting the key.
    pub fn has_sla(&self) -> bool {
        self.warn_after.as_ref().is_some_and(|r| !r.is_empty())
            || self.error_after.as_ref().is_some_and(|r| !r.is_empty())
    }

    /// The subset of this config a downstream project may see, e.g. in a Mesh
    /// publication artifact.
    ///
    /// `None` when there's no SLA to check (`has_sla` is false), so a
    /// `build_after`-only config doesn't travel at all. `build_after` is a
    /// local scheduling rule, not an SLA, and is cleared even when an SLA is
    /// present: it names this project's own build cadence, which is
    /// meaningless — and potentially confusing/leaky — to a consumer that
    /// doesn't run this project's jobs.
    pub fn for_publication(&self) -> Option<Self> {
        if !self.has_sla() {
            return None;
        }
        Some(Self {
            build_after: None,
            ..self.clone()
        })
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatePreClone {
    Never,
    IfMissing,
    Always,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ModelState {
    #[serde(default, deserialize_with = "model_freshness_rules_or_duration")]
    pub lag_tolerance: Option<ModelFreshnessRules>,
    pub require_fresh_data_from: Option<UpdatesOn>,
    pub evaluate_volatile_sql: Option<bool>,
    pub pre_clone: Option<StatePreClone>,
    #[serde(alias = "execute_hooks_on_reuse")]
    pub execute_hooks_on_any_reuse: Option<bool>,
    pub compare_unrendered_code: Option<bool>,
    #[serde(default, deserialize_with = "bool_or_string_bool")]
    pub ignore_external_modifications: Option<bool>,
}

impl PartialEq for ModelState {
    fn eq(&self, other: &Self) -> bool {
        self.lag_tolerance == other.lag_tolerance
            && updates_on_eq(
                &self.require_fresh_data_from,
                &other.require_fresh_data_from,
            )
            && self.evaluate_volatile_sql == other.evaluate_volatile_sql
            && self.pre_clone == other.pre_clone
            && self.execute_hooks_on_any_reuse == other.execute_hooks_on_any_reuse
            && self.compare_unrendered_code == other.compare_unrendered_code
            && self.ignore_external_modifications == other.ignore_external_modifications
    }
}

impl Eq for ModelState {}

/// The dbt State configs supported on data tests: only `require_fresh_data_from`,
/// `evaluate_volatile_sql` and `compare_unrendered_code` (snapshots reuse the full `ModelState`).
/// Other keys are not fields here, so they are flagged as unknown keys at parse time.
#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct DataTestState {
    pub require_fresh_data_from: Option<UpdatesOn>,
    pub evaluate_volatile_sql: Option<bool>,
    pub compare_unrendered_code: Option<bool>,
}

impl PartialEq for DataTestState {
    fn eq(&self, other: &Self) -> bool {
        updates_on_eq(
            &self.require_fresh_data_from,
            &other.require_fresh_data_from,
        ) && self.evaluate_volatile_sql == other.evaluate_volatile_sql
            && self.compare_unrendered_code == other.compare_unrendered_code
    }
}

impl Eq for DataTestState {}

fn updates_on_eq(a: &Option<UpdatesOn>, b: &Option<UpdatesOn>) -> bool {
    match (a.as_ref(), b.as_ref()) {
        (None, None) => true,
        (Some(a_val), Some(b_val)) => a_val == b_val,
        (None, Some(b_val)) => b_val == &UpdatesOn::default(),
        (Some(a_val), None) => a_val == &UpdatesOn::default(),
    }
}

// derived_semantics properties nested in models
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
pub struct DerivedSemantics {
    pub dimensions: Option<Vec<DerivedDimension>>,
    pub entities: Option<Vec<DerivedEntity>>,
}

impl Default for DerivedSemantics {
    fn default() -> Self {
        Self {
            dimensions: Some(vec![]),
            entities: Some(vec![]),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
pub struct DerivedDimension {
    pub name: String,
    pub expr: String,
    #[serde(rename = "type")]
    pub type_: ColumnPropertiesDimensionType,
    pub granularity: Option<Granularity>,
    pub is_partition: Option<bool>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub config: Option<SemanticLayerElementConfig>,
    pub validity_params: Option<DimensionValidityParams>,
}

#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
pub struct DerivedEntity {
    pub name: String,
    pub expr: String,
    #[serde(rename = "type")]
    pub type_: ColumnPropertiesEntityType,
    pub description: Option<String>,
    pub label: Option<String>,
    pub config: Option<SemanticLayerElementConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::common::FreshnessPeriod;
    use dbt_yaml;

    #[test]
    fn model_state_eq_defaults_require_fresh_data_from_to_any() {
        let base = ModelState {
            require_fresh_data_from: None,
            lag_tolerance: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
            ignore_external_modifications: None,
        };
        let other = ModelState {
            require_fresh_data_from: Some(UpdatesOn::Any),
            lag_tolerance: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
            ignore_external_modifications: None,
        };

        assert_eq!(base, other);
    }

    #[test]
    fn model_state_eq_keeps_require_fresh_data_from_all_distinct() {
        let base = ModelState {
            require_fresh_data_from: None,
            lag_tolerance: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
            ignore_external_modifications: None,
        };
        let other = ModelState {
            require_fresh_data_from: Some(UpdatesOn::All),
            lag_tolerance: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
            ignore_external_modifications: None,
        };

        assert_ne!(base, other);
    }

    #[test]
    fn model_state_accepts_legacy_execute_hooks_on_reuse_key() {
        let yaml = r#"
execute_hooks_on_reuse: true
"#;
        let state: ModelState = dbt_yaml::from_str(yaml).unwrap();

        assert_eq!(state.execute_hooks_on_any_reuse, Some(true));
    }

    #[test]
    fn compare_unrendered_code_parses_on_models_and_data_tests() {
        let yaml = "compare_unrendered_code: true\n";

        let model_state: ModelState = dbt_yaml::from_str(yaml).unwrap();
        let data_test_state: DataTestState = dbt_yaml::from_str(yaml).unwrap();

        assert_eq!(model_state.compare_unrendered_code, Some(true));
        assert_eq!(data_test_state.compare_unrendered_code, Some(true));
    }

    #[test]
    fn compare_unrendered_code_participates_in_state_eq() {
        let base = ModelState {
            require_fresh_data_from: None,
            lag_tolerance: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
            ignore_external_modifications: None,
        };
        let other = ModelState {
            compare_unrendered_code: Some(true),
            ..base.clone()
        };

        assert_ne!(base, other);

        let base_test = DataTestState {
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            compare_unrendered_code: None,
        };
        let other_test = DataTestState {
            compare_unrendered_code: Some(true),
            ..base_test.clone()
        };

        assert_ne!(base_test, other_test);
    }

    #[test]
    fn ignore_external_modifications_parses_and_participates_in_state_eq() {
        let state: ModelState =
            dbt_yaml::from_str("ignore_external_modifications: true\n").unwrap();
        assert_eq!(state.ignore_external_modifications, Some(true));

        let base = ModelState {
            require_fresh_data_from: None,
            lag_tolerance: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
            ignore_external_modifications: None,
        };
        let other = ModelState {
            ignore_external_modifications: Some(true),
            ..base.clone()
        };
        assert_ne!(base, other);
    }

    #[test]
    fn model_freshness_parses_sla_fields() {
        let yaml = r#"
warn_after:
  count: 24
  period: hour
error_after:
  count: 48
  period: hour
filter: "region = 'us'"
loaded_at_field: updated_at
loaded_at_query: "select max(updated_at) from {{ this }}"
"#;
        let freshness: ModelFreshness = dbt_yaml::from_str(yaml).unwrap();

        assert!(freshness.build_after.is_none());
        let warn_after = freshness.warn_after.expect("warn_after should parse");
        assert_eq!(warn_after.count, Some(24));
        assert_eq!(warn_after.period, Some(FreshnessPeriod::hour));
        let error_after = freshness.error_after.expect("error_after should parse");
        assert_eq!(error_after.count, Some(48));
        assert_eq!(error_after.period, Some(FreshnessPeriod::hour));
        assert_eq!(freshness.filter.as_deref(), Some("region = 'us'"));
        assert_eq!(freshness.loaded_at_field.as_deref(), Some("updated_at"));
        assert_eq!(
            freshness.loaded_at_query.as_deref(),
            Some("select max(updated_at) from {{ this }}")
        );
    }

    #[test]
    fn model_freshness_build_after_only_serializes_unchanged() {
        let yaml = r#"
build_after:
  count: 1
  period: day
"#;
        let freshness: ModelFreshness = dbt_yaml::from_str(yaml).unwrap();

        assert!(freshness.warn_after.is_none());
        assert!(freshness.error_after.is_none());
        // SLA fields must not appear when unset.
        assert_eq!(
            dbt_yaml::to_string(&freshness).unwrap(),
            "build_after:\n  count: 1\n  period: day\n  updates_on: null\n"
        );
    }

    #[test]
    fn empty_rule_object_is_not_an_sla() {
        let yaml = r#"
warn_after: {}
"#;
        let freshness: ModelFreshness = dbt_yaml::from_str(yaml).unwrap();
        assert!(freshness.warn_after.is_some());
        assert!(
            !freshness.has_sla(),
            "an empty rule object is equivalent to omitting the key"
        );
    }

    #[test]
    fn for_publication_is_none_without_an_sla() {
        let build_after_only = ModelFreshness {
            build_after: Some(ModelFreshnessRules {
                count: Some(1),
                period: Some(FreshnessPeriod::day),
                updates_on: None,
            }),
            ..Default::default()
        };
        assert!(build_after_only.for_publication().is_none());
        assert!(ModelFreshness::default().for_publication().is_none());
    }

    #[test]
    fn for_publication_clears_build_after_but_keeps_the_sla() {
        let freshness = ModelFreshness {
            build_after: Some(ModelFreshnessRules {
                count: Some(1),
                period: Some(FreshnessPeriod::day),
                updates_on: None,
            }),
            warn_after: Some(FreshnessRules {
                count: Some(12),
                period: Some(FreshnessPeriod::hour),
            }),
            loaded_at_field: Some("updated_at".to_string()),
            ..Default::default()
        };

        let published = freshness
            .for_publication()
            .expect("has an SLA, so it should publish");

        assert!(
            published.build_after.is_none(),
            "build_after is this project's own scheduling rule and must not leak to a downstream consumer"
        );
        assert_eq!(published.warn_after, freshness.warn_after);
        assert_eq!(published.loaded_at_field, freshness.loaded_at_field);
    }

    #[test]
    fn test_model_constraint_columns_as_string() {
        let yaml = r#"
type: primary_key
columns: mart_hashkey_order
"#;
        let constraint: ModelConstraint = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(
            constraint.columns,
            Some(vec!["mart_hashkey_order".to_string()])
        );
        assert_eq!(constraint.type_, ConstraintType::PrimaryKey);
    }

    #[test]
    fn test_model_constraint_columns_as_string_array() {
        let yaml = r#"
type: primary_key
columns: ["mart_hashkey_order"]
"#;
        let constraint: ModelConstraint = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(
            constraint.columns,
            Some(vec!["mart_hashkey_order".to_string()])
        );
        assert_eq!(constraint.type_, ConstraintType::PrimaryKey);
    }

    #[test]
    fn test_model_constraint_columns_as_array() {
        let yaml = r#"
type: primary_key
columns:
  - column1
  - column2
"#;
        let constraint: ModelConstraint = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(
            constraint.columns,
            Some(vec!["column1".to_string(), "column2".to_string()])
        );
        assert_eq!(constraint.type_, ConstraintType::PrimaryKey);
    }

    #[test]
    fn test_model_constraint_columns_as_null() {
        let yaml = r#"
type: check
expression: "amount > 0"
"#;
        let constraint: ModelConstraint = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(constraint.columns, None);
        assert_eq!(constraint.type_, ConstraintType::Check);
        assert_eq!(constraint.expression, Some("amount > 0".to_string()));
    }

    #[test]
    fn test_model_constraint_to_columns_as_string() {
        let yaml = r#"
type: foreign_key
columns: order_id
to: ref('orders')
to_columns: id
"#;
        let constraint: ModelConstraint = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(constraint.columns, Some(vec!["order_id".to_string()]));
        assert_eq!(constraint.to_columns, Some(vec!["id".to_string()]));
        assert_eq!(
            constraint.to.as_ref().map(|s| s.as_str()),
            Some("ref('orders')")
        );
        assert_eq!(constraint.type_, ConstraintType::ForeignKey);
    }

    #[test]
    fn test_model_constraint_to_columns_as_array() {
        let yaml = r#"
type: foreign_key
columns:
  - user_id
  - org_id
to: ref('users')
to_columns:
  - id
  - organization_id
"#;
        let constraint: ModelConstraint = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(
            constraint.columns,
            Some(vec!["user_id".to_string(), "org_id".to_string()])
        );
        assert_eq!(
            constraint.to_columns,
            Some(vec!["id".to_string(), "organization_id".to_string()])
        );
        assert_eq!(constraint.type_, ConstraintType::ForeignKey);
    }

    #[test]
    fn test_model_constraint_full_example() {
        let yaml = r#"
type: primary_key
name: pk_orders
columns: order_id
warn_unsupported: true
warn_unenforced: false
"#;
        let constraint: ModelConstraint = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(constraint.type_, ConstraintType::PrimaryKey);
        assert_eq!(constraint.name, Some("pk_orders".to_string()));
        assert_eq!(constraint.columns, Some(vec!["order_id".to_string()]));
        assert_eq!(constraint.warn_unsupported, Some(true));
        assert_eq!(constraint.warn_unenforced, Some(false));
    }

    #[test]
    fn test_model_constraint_to_span_ref_captures_line() {
        let yaml = "type: foreign_key\nto: ref('primary_model')\nto_columns: [id]\n";
        let constraint: ModelConstraint = dbt_yaml::from_str(yaml).unwrap();
        let spanned = constraint.to.as_ref().expect("to should be Some");
        assert_eq!(spanned.as_str(), "ref('primary_model')");
        assert!(spanned.span().is_valid(), "span should be valid");
        assert_eq!(spanned.span().start.line, 2, "to: should be on line 2");
    }

    #[test]
    fn test_model_constraint_to_span_source_captures_line() {
        let yaml = "type: foreign_key\nto: source('raw', 'users')\nto_columns: [id]\n";
        let constraint: ModelConstraint = dbt_yaml::from_str(yaml).unwrap();
        let spanned = constraint.to.as_ref().expect("to should be Some");
        assert_eq!(spanned.as_str(), "source('raw', 'users')");
        assert!(spanned.span().is_valid(), "span should be valid");
        assert_eq!(spanned.span().start.line, 2, "to: should be on line 2");
    }
}
