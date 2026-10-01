use std::sync::Arc;

pub use dbt_tasks_core::metricflow::{MetricflowClient, MetricflowClientFactory};

/// Metricflow feature slot on [`FeatureStack`](super::feature_stack::FeatureStack).
#[derive(Default)]
pub struct MetricflowFeature {
    pub factory: Option<Arc<dyn MetricflowClientFactory>>,
}
