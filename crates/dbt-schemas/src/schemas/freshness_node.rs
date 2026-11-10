use dbt_telemetry::NodeType;

use super::nodes::{DbtModel, DbtSource, InternalDbtNode, InternalDbtNodeAttributes};

/// A source, or a model carrying a freshness SLA.
///
/// Accessors return `""` rather than `Option<&str>`: the empty string is the
/// established "unset" sentinel across the freshness code path.
pub trait FreshnessNodeRef: InternalDbtNodeAttributes {
    fn get_loaded_at_field(&self) -> &str;
    fn get_loaded_at_query(&self) -> &str;
    fn get_freshness_filter(&self) -> Option<&str>;
    /// Sentence-start node kind for diagnostics.
    fn kind_label(&self) -> &'static str;
}

impl FreshnessNodeRef for DbtSource {
    fn kind_label(&self) -> &'static str {
        "Source"
    }

    fn get_loaded_at_field(&self) -> &str {
        self.__source_attr__
            .loaded_at_field
            .as_deref()
            .unwrap_or("")
    }

    fn get_loaded_at_query(&self) -> &str {
        self.__source_attr__
            .loaded_at_query
            .as_deref()
            .unwrap_or("")
    }

    fn get_freshness_filter(&self) -> Option<&str> {
        self.__source_attr__.freshness.as_ref()?.filter.as_deref()
    }
}

impl FreshnessNodeRef for DbtModel {
    fn kind_label(&self) -> &'static str {
        "Model"
    }

    fn get_loaded_at_field(&self) -> &str {
        self.__model_attr__
            .freshness
            .as_ref()
            .and_then(|f| f.loaded_at_field.as_deref())
            .unwrap_or("")
    }

    fn get_loaded_at_query(&self) -> &str {
        self.__model_attr__
            .freshness
            .as_ref()
            .and_then(|f| f.loaded_at_query.as_deref())
            .unwrap_or("")
    }

    fn get_freshness_filter(&self) -> Option<&str> {
        self.__model_attr__.freshness.as_ref()?.filter.as_deref()
    }
}

/// Every source, plus models with an SLA. `build_after`-only models are excluded.
pub fn is_freshness_node(node: &dyn InternalDbtNode) -> bool {
    node.resource_type() == NodeType::Source || node.has_freshness()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::common::{FreshnessPeriod, FreshnessRules, ModelFreshnessRules};
    use crate::schemas::properties::ModelFreshness;

    fn sla_rules() -> FreshnessRules {
        FreshnessRules {
            count: Some(12),
            period: Some(FreshnessPeriod::hour),
        }
    }

    #[test]
    fn sources_are_always_freshness_nodes() {
        let source = DbtSource::default();
        assert!(is_freshness_node(&source));
    }

    #[test]
    fn models_with_an_sla_are_freshness_nodes() {
        let mut model = DbtModel::default();
        model.__model_attr__.freshness = Some(ModelFreshness {
            warn_after: Some(sla_rules()),
            ..Default::default()
        });
        assert!(is_freshness_node(&model));
    }

    #[test]
    fn plain_models_are_not_freshness_nodes() {
        assert!(!is_freshness_node(&DbtModel::default()));
    }

    #[test]
    fn build_after_only_models_are_not_freshness_nodes() {
        let mut model = DbtModel::default();
        model.__model_attr__.freshness = Some(ModelFreshness {
            build_after: Some(ModelFreshnessRules {
                count: Some(1),
                period: Some(FreshnessPeriod::day),
                updates_on: None,
            }),
            ..Default::default()
        });
        assert!(!is_freshness_node(&model));
    }

    #[test]
    fn selection_yields_sources_and_sla_models_only() {
        let source = DbtSource::default();
        let mut sla_model = DbtModel::default();
        sla_model.__common_attr__.unique_id = "model.pkg.stg_orders".to_string();
        sla_model.__model_attr__.freshness = Some(ModelFreshness {
            error_after: Some(sla_rules()),
            ..Default::default()
        });
        let mut plain_model = DbtModel::default();
        plain_model.__common_attr__.unique_id = "model.pkg.plain".to_string();

        let all: Vec<&dyn InternalDbtNode> = vec![&source, &sla_model, &plain_model];
        let selected: Vec<String> = all
            .into_iter()
            .filter(|node| is_freshness_node(*node))
            .map(|node| node.common().unique_id.clone())
            .collect();

        assert_eq!(
            selected,
            vec![String::new(), "model.pkg.stg_orders".to_string()]
        );
    }
}
