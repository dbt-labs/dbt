use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use crate::schemas::{
    CommonAttributes, NodeBaseAttributes,
    dbt_column::Granularity,
    manifest::common::{SourceFileMetadata, WhereFilter, WhereFilterIntersection},
    project::MetricConfig,
    properties::{
        MetricsProperties,
        metrics_properties::{
            AggregationType, ConstantProperty, ConversionCalculationType,
            MetricPropertiesMetricInput, MetricPropertiesNonAdditiveDimension, MetricType,
            PeriodAggregationType, StringOrMetricPropertiesMetricInput, WindowChoice,
        },
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtMetric {
    pub __common_attr__: CommonAttributes,
    pub __base_attr__: NodeBaseAttributes,
    pub __metric_attr__: DbtMetricAttr,

    // To be deprecated
    pub deprecated_config: MetricConfig,

    pub __other__: BTreeMap<String, YmlValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DbtMetricAttr {
    pub label: Option<String>,
    pub metric_type: MetricType,
    pub type_params: MetricTypeParams,
    pub filter: Option<WhereFilterIntersection>,
    pub metadata: Option<SourceFileMetadata>,
    pub time_granularity: Option<Granularity>,
    pub unrendered_config: BTreeMap<String, YmlValue>,
    pub metrics: Vec<MetricInput>, // TODO: seems unused, can this be removed?
    pub created_at: f64,
    pub group: Option<String>,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricTypeParams {
    pub measure: Option<MetricInputMeasure>,
    pub input_measures: Option<Vec<MetricInputMeasure>>,
    pub numerator: Option<MetricInput>,
    pub denominator: Option<MetricInput>,
    pub expr: Option<String>,
    pub window: Option<MetricTimeWindow>,
    pub grain_to_date: Option<Granularity>,
    pub metrics: Option<Vec<MetricInput>>,
    pub conversion_type_params: Option<ConversionTypeParams>,
    pub cumulative_type_params: Option<CumulativeTypeParams>,
    #[serde(default = "default_join_to_timespine")]
    pub join_to_timespine: Option<bool>,
    pub fill_nulls_with: Option<i32>,
    #[serde(skip_deserializing)]
    pub is_private: bool,
    pub metric_aggregation_params: Option<MetricAggregationParameters>,
}

/// Default join_to_timespine to false if not provided
fn default_join_to_timespine() -> Option<bool> {
    Some(false)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricAggregationParameters {
    pub semantic_model: String,
    pub agg: Option<AggregationType>,
    pub agg_params: Option<MeasureAggregationParameters>,
    pub agg_time_dimension: Option<String>,
    pub non_additive_dimension: Option<NonAdditiveDimension>,
    pub expr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NonAdditiveDimension {
    pub name: String,
    pub window_choice: WindowChoice,
    #[serde(default)]
    pub window_groupings: Vec<String>,
}

impl From<MetricPropertiesNonAdditiveDimension> for NonAdditiveDimension {
    fn from(source: MetricPropertiesNonAdditiveDimension) -> Self {
        Self {
            name: source.name,
            window_choice: source.window_agg,
            window_groupings: source.group_by.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MeasureAggregationParameters {
    pub percentile: Option<f32>,
    pub use_discrete_percentile: Option<bool>,
    pub use_approximate_percentile: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricInputMeasure {
    pub name: String,
    pub filter: Option<WhereFilterIntersection>,
    pub alias: Option<String>,
    pub join_to_timespine: Option<bool>,
    pub fill_nulls_with: Option<i32>,
}

impl Default for MetricInputMeasure {
    fn default() -> Self {
        Self {
            name: String::new(),
            filter: None,
            alias: None,
            join_to_timespine: Some(false),
            fill_nulls_with: None,
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricInput {
    pub name: String,
    pub filter: Option<WhereFilterIntersection>,
    pub alias: Option<String>,
    pub offset_window: Option<MetricTimeWindow>,
    pub offset_to_grain: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricTimeWindow {
    pub count: i32,
    pub granularity: String,
}

impl MetricTimeWindow {
    pub fn from_string(str: String) -> Result<Self, String> {
        let parts: Vec<&str> = str.split_whitespace().collect();

        // Check if we have exactly 2 parts
        if parts.len() != 2 {
            return Err(format!(
                "Invalid window ({}). Should be of the form `<count> <granularity>`, e.g., `28 days`",
                str
            ));
        }

        // Parse count - must be a digit
        let count_str = parts[0];
        if !count_str.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!(
                "Invalid count ({}) in window string: ({})",
                count_str, str
            ));
        }

        let count: i32 = count_str
            .parse()
            .map_err(|_| format!("Invalid count ({}) in window string: ({})", count_str, str))?;

        // Parse granularity
        let mut granularity = parts[1].to_lowercase();

        // Valid granularities from the Granularity enum
        let valid_granularities = [
            "nanosecond",
            "microsecond",
            "millisecond",
            "second",
            "minute",
            "hour",
            "day",
            "week",
            "month",
            "quarter",
            "year",
        ];

        // Check if granularity ends with 's' and the base form is valid
        if granularity.ends_with('s') {
            let singular_form = &granularity[..granularity.len() - 1];
            if valid_granularities.contains(&singular_form) {
                granularity = singular_form.to_string();
            }
        }

        // Validate final granularity
        if !valid_granularities.contains(&granularity.as_str()) {
            return Err(format!(
                "Invalid granularity ({}) in window string: ({})",
                parts[1], str
            ));
        }

        Ok(Self { count, granularity })
    }
}

impl Default for MetricTimeWindow {
    fn default() -> Self {
        Self {
            count: 1,
            granularity: String::from("day"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ConversionTypeParams {
    pub base_measure: Option<MetricInputMeasure>,
    pub base_metric: Option<MetricInput>,
    pub conversion_measure: Option<MetricInputMeasure>,
    pub conversion_metric: Option<MetricInput>,
    pub entity: String,
    pub calculation: Option<ConversionCalculationType>,
    pub window: Option<MetricTimeWindow>,
    pub constant_properties: Option<Vec<ConstantProperty>>,
}

impl From<MetricsProperties> for ConversionTypeParams {
    fn from(props: MetricsProperties) -> Self {
        Self {
            entity: props.entity.unwrap_or_default(),
            window: props
                .window
                .and_then(|w| MetricTimeWindow::from_string(w).ok()),
            base_measure: None,
            base_metric: props.base_metric.map(MetricInput::from),
            conversion_measure: None,
            conversion_metric: props.conversion_metric.map(MetricInput::from),
            calculation: Some(props.calculation.unwrap_or_default()),
            constant_properties: props.constant_properties,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CumulativeTypeParams {
    pub window: Option<MetricTimeWindow>,
    pub grain_to_date: Option<String>,
    pub period_agg: PeriodAggregationType,
    pub metric: Option<MetricInput>,
}

impl From<MetricsProperties> for CumulativeTypeParams {
    fn from(props: MetricsProperties) -> Self {
        Self {
            window: props
                .window
                .and_then(|w| MetricTimeWindow::from_string(w).ok()),
            grain_to_date: props.grain_to_date.map(|value| value.to_string()),
            period_agg: props.period_agg.unwrap_or_default(),
            metric: props.input_metric.map(MetricInput::from),
        }
    }
}

// From implementations for converting from properties to manifest types

impl From<StringOrMetricPropertiesMetricInput> for MetricInput {
    fn from(source: StringOrMetricPropertiesMetricInput) -> Self {
        match source {
            StringOrMetricPropertiesMetricInput::String(name) => Self {
                name,
                ..Default::default()
            },
            StringOrMetricPropertiesMetricInput::MetricPropertiesMetricInput(input) => {
                Self::from(input)
            }
        }
    }
}

impl From<MetricPropertiesMetricInput> for MetricInput {
    fn from(metric_input: MetricPropertiesMetricInput) -> Self {
        Self {
            name: metric_input.name.clone(),
            filter: metric_input.filter.0.map(|filter| WhereFilterIntersection {
                where_filters: vec![WhereFilter {
                    where_sql_template: filter,
                }],
            }),
            alias: metric_input.alias,
            offset_window: metric_input
                .offset_window
                .and_then(|w| MetricTimeWindow::from_string(w).ok()),
            offset_to_grain: metric_input.offset_to_grain,
        }
    }
}

impl From<MetricsProperties> for MetricTypeParams {
    /// Create MetricTypeParams from MetricsProperties.
    ///
    /// Note that this doesn't hydrate all fields.
    /// For example, `metric_aggregation_params` needs to be hydrated with a semantic_model name, a field that does not exist in MetricsProperties.
    #[allow(clippy::manual_map)]
    fn from(props: MetricsProperties) -> Self {
        let numerator = props.numerator.clone().map(MetricInput::from);
        let denominator = props.denominator.clone().map(MetricInput::from);
        let cumulative_type_params = if matches!(&props.type_, Some(MetricType::Cumulative)) {
            Some(props.clone().into())
        } else {
            None
        };
        let expr = props.expr.clone().map(String::from);

        let input_metrics: Option<Vec<MetricInput>> = props
            .input_metrics
            .clone()
            .map(|input_metrics| input_metrics.into_iter().map(MetricInput::from).collect());

        // we infer conversion type from other fields, since type is optional
        let conversion_type_params: Option<ConversionTypeParams> =
            if props.conversion_metric.is_some() {
                Some(props.clone().into())
            } else {
                None
            };

        let metric_type = props.type_.unwrap_or_default();
        let window: Option<MetricTimeWindow> = props
            .window
            .and_then(|w| MetricTimeWindow::from_string(w).ok());

        let mut type_params = MetricTypeParams {
            numerator: numerator.clone(),
            denominator: denominator.clone(),
            cumulative_type_params,
            conversion_type_params,
            expr,
            window,
            metrics: input_metrics,
            input_measures: Some(vec![]),
            is_private: props.hidden.unwrap_or(false),
            ..Default::default()
        };

        let input_measures = match metric_type {
            // For ratio metrics, create input measures from numerator and denominator
            MetricType::Ratio => match (numerator, denominator) {
                (None, _) | (_, None) => Some(vec![]),
                (Some(num), Some(den)) => Some(vec![
                    MetricInputMeasure {
                        name: num.name.clone(),
                        filter: num.filter.clone(),
                        alias: num.alias,
                        join_to_timespine: Some(false),
                        fill_nulls_with: None,
                    },
                    MetricInputMeasure {
                        name: den.name.clone(),
                        filter: den.filter.clone(),
                        alias: den.alias,
                        join_to_timespine: Some(false),
                        fill_nulls_with: None,
                    },
                ]),
            },

            // If we have metrics, convert them to input measures
            MetricType::Derived => Some(
                type_params
                    .metrics
                    .clone()
                    .unwrap_or_default()
                    .iter()
                    .map(|metric_input| MetricInputMeasure {
                        name: metric_input.name.clone(),
                        filter: metric_input.filter.clone(),
                        alias: metric_input.alias.clone(),
                        join_to_timespine: Some(false),
                        fill_nulls_with: None,
                    })
                    .collect(),
            ),

            _ => Some(vec![]),
        };

        type_params.input_measures = input_measures;

        // dbt-semantic-interfaces says to hydrate these fields for simple metrics only
        let mut join_to_timespine = Some(false);
        let mut fill_nulls_with = None;
        if matches!(&metric_type, MetricType::Simple) {
            if props.join_to_timespine.is_some() {
                join_to_timespine = props.join_to_timespine;
            }
            fill_nulls_with = props.fill_nulls_with;
        }

        type_params.join_to_timespine = join_to_timespine;
        type_params.fill_nulls_with = fill_nulls_with;

        type_params
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::properties::metrics_properties::MetricType;

    // Previously window was forced to None for cumulative and conversion metrics; these unit tests guard against future regressions
    fn props_with_window(metric_type: MetricType, window: Option<&str>) -> MetricsProperties {
        MetricsProperties {
            name: "test".to_string(),
            type_: Some(metric_type),
            window: window.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_window_populated_for_simple_metric() {
        let params = MetricTypeParams::from(props_with_window(MetricType::Simple, Some("28 days")));
        assert_eq!(
            params.window,
            Some(MetricTimeWindow {
                count: 28,
                granularity: "day".to_string()
            })
        );
    }

    #[test]
    fn test_window_populated_for_cumulative_metric() {
        let params =
            MetricTypeParams::from(props_with_window(MetricType::Cumulative, Some("7 days")));
        assert_eq!(
            params.window,
            Some(MetricTimeWindow {
                count: 7,
                granularity: "day".to_string()
            })
        );
    }

    #[test]
    fn test_window_populated_for_conversion_metric() {
        let params =
            MetricTypeParams::from(props_with_window(MetricType::Conversion, Some("14 days")));
        assert_eq!(
            params.window,
            Some(MetricTimeWindow {
                count: 14,
                granularity: "day".to_string()
            })
        );
    }

    #[test]
    fn test_window_populated_for_ratio_metric() {
        let params = MetricTypeParams::from(props_with_window(MetricType::Ratio, Some("30 days")));
        assert_eq!(
            params.window,
            Some(MetricTimeWindow {
                count: 30,
                granularity: "day".to_string()
            })
        );
    }

    #[test]
    fn test_window_populated_for_derived_metric() {
        let params =
            MetricTypeParams::from(props_with_window(MetricType::Derived, Some("90 days")));
        assert_eq!(
            params.window,
            Some(MetricTimeWindow {
                count: 90,
                granularity: "day".to_string()
            })
        );
    }

    #[test]
    fn test_window_none_when_not_set() {
        let params = MetricTypeParams::from(props_with_window(MetricType::Simple, None));
        assert_eq!(params.window, None);
    }

    #[test]
    fn test_window_none_for_invalid_format() {
        let params = MetricTypeParams::from(props_with_window(MetricType::Simple, Some("invalid")));
        assert_eq!(params.window, None);
    }
}
