use crate::schemas::dbt_column::Granularity;
use crate::schemas::project::MetricConfig;
use dbt_yaml::DbtSchema;
use dbt_yaml::UntaggedEnumDeserialize;
use dbt_yaml::Verbatim;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, Default)]
pub struct MetricsProperties {
    pub name: String,
    #[serde(default = "default_hidden")]
    pub hidden: Option<bool>,
    pub description: Option<String>,
    pub label: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<MetricType>,
    pub agg: Option<AggregationType>,
    pub percentile: Option<f32>,
    pub percentile_type: Option<PercentileType>,
    pub join_to_timespine: Option<bool>,
    pub fill_nulls_with: Option<i32>,
    pub expr: Option<MetricExpr>,
    pub filter: Verbatim<Option<String>>,
    pub config: Option<MetricConfig>,
    pub non_additive_dimension: Option<MetricPropertiesNonAdditiveDimension>,
    pub agg_time_dimension: Option<String>,
    pub window: Option<String>,
    pub grain_to_date: Option<Granularity>,
    pub period_agg: Option<PeriodAggregationType>,
    pub input_metric: Option<StringOrMetricPropertiesMetricInput>,
    pub numerator: Option<StringOrMetricPropertiesMetricInput>,
    pub denominator: Option<StringOrMetricPropertiesMetricInput>,
    pub input_metrics: Option<Vec<MetricPropertiesMetricInput>>,
    pub entity: Option<String>,
    pub calculation: Option<ConversionCalculationType>,
    pub base_metric: Option<StringOrMetricPropertiesMetricInput>,
    pub conversion_metric: Option<StringOrMetricPropertiesMetricInput>,
    pub constant_properties: Option<Vec<ConstantProperty>>,
    // Used to specify a default time granularity for the metric
    pub time_granularity: Option<Granularity>,
}

pub fn default_hidden() -> Option<bool> {
    Some(false)
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, DbtSchema)]
#[serde(rename_all = "lowercase")]
pub enum MetricType {
    #[default]
    Simple,
    Ratio,
    Cumulative,
    Derived,
    Conversion,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum AggregationType {
    Sum,
    Min,
    Max,
    CountDistinct,
    SumBoolean,
    Average,
    Percentile,
    Median,
    Count,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum PercentileType {
    Discrete,
    Continuous,
}

#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize, PartialEq, Eq, DbtSchema)]
#[serde(untagged)]
pub enum MetricExpr {
    String(String),
    Integer(i32),
}

impl From<MetricExpr> for String {
    fn from(val: MetricExpr) -> Self {
        match val {
            MetricExpr::String(s) => s,
            MetricExpr::Integer(i) => i.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct MetricPropertiesNonAdditiveDimension {
    pub name: String,
    pub window_agg: WindowChoice,
    pub group_by: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowChoice {
    Min,
    Max,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct MetricPropertiesMetricInput {
    pub name: String,
    pub filter: Verbatim<Option<String>>,
    pub alias: Option<String>,
    pub offset_window: Option<String>,
    pub offset_to_grain: Option<String>,
}

#[derive(UntaggedEnumDeserialize, Serialize, Debug, Clone, DbtSchema)]
#[serde(untagged)]
pub enum StringOrMetricPropertiesMetricInput {
    String(String),
    MetricPropertiesMetricInput(MetricPropertiesMetricInput),
}

#[derive(Serialize, Deserialize, Default, Debug, Clone, DbtSchema, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ConversionCalculationType {
    conversions,
    #[default]
    conversion_rate,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct ConstantProperty {
    pub base_property: String,
    pub conversion_property: String,
}

#[derive(Default, Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PeriodAggregationType {
    #[default]
    First,
    Last,
    Average,
}
