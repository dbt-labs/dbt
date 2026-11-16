use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, impl_loader,
};
use crate::relation::databricks::config::{
    DatabricksRelationMetadata, DatabricksRelationMetadataKey,
};
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use minijinja::value::{Value, ValueMap};

pub(crate) const TYPE_NAME: &str = "query";

#[derive(Clone, Debug)]
pub struct MetricViewDefinition(String);

pub type MetricViewQuery = SimpleComponentConfigImpl<MetricViewDefinition>;

// TODO: Revisit this -- is there a way we can just use dbt_yaml to handle this?
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CanonicalYaml {
    Scalar(String),
    Sequence(Vec<Self>),
    Mapping(Vec<(Self, Self)>),
    Tagged(String, Box<Self>),
}

fn scalar_lexeme(source: &str, span: &dbt_yaml::Span) -> Option<String> {
    source
        .get(span.start.index..span.end.index)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn canonicalize(value: dbt_yaml::Value, source: &str) -> CanonicalYaml {
    match value {
        dbt_yaml::Value::Null(span) => CanonicalYaml::Scalar(
            scalar_lexeme(source, &span).unwrap_or_else(|| "null".to_string()),
        ),
        dbt_yaml::Value::Bool(value, span) => {
            CanonicalYaml::Scalar(scalar_lexeme(source, &span).unwrap_or_else(|| value.to_string()))
        }
        dbt_yaml::Value::Number(value, span) => {
            CanonicalYaml::Scalar(scalar_lexeme(source, &span).unwrap_or_else(|| value.to_string()))
        }
        dbt_yaml::Value::String(value, _) => CanonicalYaml::Scalar(value),
        dbt_yaml::Value::Sequence(values, _) => CanonicalYaml::Sequence(
            values
                .into_iter()
                .map(|value| canonicalize(value, source))
                .collect(),
        ),
        dbt_yaml::Value::Mapping(values, _) => {
            let mut values = values
                .into_iter()
                .map(|(key, value)| (canonicalize(key, source), canonicalize(value, source)))
                .collect::<Vec<_>>();
            values.sort();
            CanonicalYaml::Mapping(values)
        }
        dbt_yaml::Value::Tagged(value, _) => CanonicalYaml::Tagged(
            value.tag.to_string(),
            Box::new(canonicalize(value.value, source)),
        ),
    }
}

fn normalized_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Reference: https://github.com/databricks/dbt-databricks/blob/2c3aa9fdddbab30a3c4a660c5e98722e989a592b/dbt/adapters/databricks/relation_configs/metric_view.py#L22
fn diff(
    desired: &MetricViewDefinition,
    current: &MetricViewDefinition,
) -> Option<MetricViewDefinition> {
    let desired_yaml = dbt_yaml::from_str::<dbt_yaml::Value>(&desired.0);
    let current_yaml = dbt_yaml::from_str::<dbt_yaml::Value>(&current.0);

    let equal = match (desired_yaml, current_yaml) {
        (Ok(desired_yaml), Ok(current_yaml)) => {
            canonicalize(desired_yaml, &desired.0) == canonicalize(current_yaml, &current.0)
        }
        _ => normalized_whitespace(&desired.0) == normalized_whitespace(&current.0),
    };

    (!equal).then(|| desired.clone())
}

fn to_jinja(query: &MetricViewDefinition) -> Value {
    Value::from(ValueMap::from([(
        Value::from("query"),
        Value::from(query.0.clone()),
    )]))
}

fn clean_definition(definition: &str) -> String {
    let definition = definition.trim();
    definition
        .strip_suffix(';')
        .unwrap_or(definition)
        .trim_end()
        .to_string()
}

fn new_component(query: &str) -> MetricViewQuery {
    MetricViewQuery {
        type_name: TYPE_NAME,
        diff_fn: diff,
        to_jinja_fn: to_jinja,
        value: MetricViewDefinition(clean_definition(query)),
    }
}

fn from_remote_state(results: &DatabricksRelationMetadata) -> AdapterResult<MetricViewQuery> {
    let describe_extended = results
        .get(&DatabricksRelationMetadataKey::DescribeExtended)
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                "Cannot find metric view description".to_string(),
            )
        })?;

    for row in describe_extended.rows() {
        if let (Ok(key), Ok(value)) = (row.get_item(&Value::from(0)), row.get_item(&Value::from(1)))
            && key.as_str() == Some("View Text")
            && let Some(definition) = value.as_str()
            && !definition.is_empty()
        {
            let definition = definition.trim();
            let definition = definition
                .strip_prefix("$$")
                .and_then(|inner| inner.strip_suffix("$$"))
                .unwrap_or(definition)
                .trim();
            return Ok(new_component(definition));
        }
    }

    Err(AdapterError::new(
        AdapterErrorKind::Configuration,
        "Metric view has no 'View Text' in DESCRIBE EXTENDED output".to_string(),
    ))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<MetricViewQuery> {
    let definition = relation_config
        .compiled_code()
        .filter(|sql| !sql.trim().is_empty());
    let definition = definition.ok_or_else(|| {
        AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Cannot compile metric view {} with no YAML definition",
                relation_config.name()
            ),
        )
    })?;

    Ok(new_component(definition))
}

impl_loader!(MetricViewQuery, DatabricksRelationMetadata);

impl MetricViewQueryLoader {
    pub fn new_component_type_erased(query: &str) -> Box<dyn ComponentConfig> {
        Box::new(new_component(query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::config_v2::ComponentConfig;
    use crate::relation::databricks::config::test_helpers;
    use arrow::array::{ArrayRef, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use dbt_agate::AgateTable;
    use indexmap::IndexMap;
    use std::sync::Arc;

    fn describe_extended(rows: &[(&str, &str)]) -> AgateTable {
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, true),
            Field::new("value", DataType::Utf8, true),
        ]));
        let keys = rows.iter().map(|(key, _)| *key).collect::<Vec<_>>();
        let values = rows.iter().map(|(_, value)| *value).collect::<Vec<_>>();
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(keys)) as ArrayRef,
                Arc::new(StringArray::from(values)) as ArrayRef,
            ],
        )
        .unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    fn has_diff(desired: &str, current: &str) -> bool {
        let desired = new_component(desired);
        let current = new_component(current);
        desired.diff_from(Some(&current)).is_some()
    }

    #[test]
    fn compares_metric_view_yaml_semantically() {
        assert!(!has_diff(
            "version: 1.1\nsource: \"`c`.`s`.`t`\"\nsynonyms: [a, b]",
            "version: 1.1\nsource: '`c`.`s`.`t`'\nsynonyms:\n  - a\n  - b",
        ));
        assert!(has_diff(
            "version: 1.1\nsynonyms: [yes, no]",
            "version: 1.1\nsynonyms: [true, false]",
        ));
    }

    #[test]
    fn preserves_scalar_lexemes_like_yaml_base_loader() {
        for (desired, current) in [
            ("value: True", "value: true"),
            ("value: yes", "value: Yes"),
            ("value: 1.10", "value: 1.1"),
            ("value: 01", "value: 1"),
            ("value: null", "value: ~"),
            ("value: Null", "value: null"),
        ] {
            assert!(has_diff(desired, current), "{desired:?} and {current:?}");
        }

        for (desired, current) in [
            ("value: true", "value: 'true'"),
            ("value: 1.10", "value: \"1.10\""),
            ("value: null", "value: 'null'"),
        ] {
            assert!(!has_diff(desired, current), "{desired:?} and {current:?}");
        }
    }

    #[test]
    fn malformed_yaml_falls_back_to_whitespace_comparison() {
        assert!(!has_diff("a: [1, 2", "a:   [1,   2"));
        assert!(has_diff("a: [1, 2", "a: [1, 3"));
    }

    #[test]
    fn reads_view_text_with_optional_dollar_delimiters() {
        for definition in ["$$ version: 1.1\nsource: t $$", "version: 1.1\nsource: t"] {
            let results = IndexMap::from([(
                DatabricksRelationMetadataKey::DescribeExtended,
                describe_extended(&[("View Text", definition)]),
            )]);
            assert_eq!(
                from_remote_state(&results).unwrap().value.0,
                "version: 1.1\nsource: t"
            );
        }
    }

    #[test]
    fn missing_remote_definition_is_an_error() {
        let missing_description = from_remote_state(&IndexMap::new()).unwrap_err();
        assert!(
            missing_description
                .to_string()
                .contains("Cannot find metric view description")
        );

        let results = IndexMap::from([(
            DatabricksRelationMetadataKey::DescribeExtended,
            describe_extended(&[("Other Field", "value")]),
        )]);
        let missing_view_text = from_remote_state(&results).unwrap_err();
        assert!(missing_view_text.to_string().contains("no 'View Text'"));
    }

    #[test]
    fn missing_local_definition_is_an_error() {
        let model = test_helpers::create_mock_dbt_model(test_helpers::TestModelConfig {
            query: Some("  ".to_string()),
            ..Default::default()
        });
        let error = from_local_config(&model).unwrap_err();
        assert!(error.to_string().contains("no YAML definition"));
    }
}
