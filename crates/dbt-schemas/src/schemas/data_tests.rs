use dbt_yaml::{DbtSchema, Spanned, UntaggedEnumDeserialize, Verbatim};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::collections::BTreeMap;

use crate::schemas::project::DataTestConfig;
use crate::schemas::serde::StringOrArrayOfStrings;

#[derive(UntaggedEnumDeserialize, Serialize, Debug, Clone, DbtSchema)]
#[serde(untagged)]
pub enum DataTests {
    String(Spanned<String>),
    CustomTest(Spanned<CustomTest>),
}

impl DataTests {
    pub fn span(&self) -> &dbt_yaml::Span {
        match self {
            DataTests::String(spanned) => spanned.span(),
            DataTests::CustomTest(spanned) => spanned.span(),
        }
    }
}

#[derive(Debug, Clone, UntaggedEnumDeserialize, Serialize, DbtSchema)]
#[serde(untagged)]
pub enum CustomTest {
    MultiKey(Box<CustomTestMultiKey>),
    SimpleKeyValue(BTreeMap<String, CustomTestInner>),
}

#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize, Serialize, DbtSchema)]
pub struct CustomTestInner {
    pub name: Option<String>,
    pub description: Option<String>,
    pub config: Option<DataTestConfig>,
    pub column_name: Option<StringOrArrayOfStrings>,
    pub arguments: Verbatim<Option<dbt_yaml::Value>>,
    pub __deprecated_args_and_configs__: Verbatim<BTreeMap<String, dbt_yaml::Value>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize, Serialize, DbtSchema)]
pub struct CustomTestMultiKey {
    pub test_name: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub config: Option<DataTestConfig>,
    pub column_name: Option<StringOrArrayOfStrings>,
    pub arguments: Verbatim<Option<dbt_yaml::Value>>,
    pub __deprecated_args_and_configs__: Verbatim<BTreeMap<String, dbt_yaml::Value>>,
}

// Helper to extract column name from DataTests
impl DataTests {
    pub fn column_name(&self) -> Option<StringOrArrayOfStrings> {
        match self {
            DataTests::String(_) => None,
            DataTests::CustomTest(test) => match test.as_ref() {
                CustomTest::MultiKey(test) => test.column_name.clone(),
                CustomTest::SimpleKeyValue(test) => {
                    test.values().next().and_then(|v| v.column_name.clone())
                }
            },
        }
    }

    pub fn test_name(&self) -> Option<&str> {
        match self {
            DataTests::String(test) => Some(test),
            DataTests::CustomTest(test) => match test.as_ref() {
                CustomTest::MultiKey(test) => test.name.as_deref(),
                CustomTest::SimpleKeyValue(test) => {
                    test.values().next().and_then(|v| v.name.as_deref())
                }
            },
        }
    }

    /// The user-authored `description:` on a generic test, if any. The bare
    /// `String` form (e.g. `- unique`) has no description.
    pub fn description(&self) -> Option<&str> {
        match self {
            DataTests::String(_) => None,
            DataTests::CustomTest(test) => match test.as_ref() {
                CustomTest::MultiKey(test) => test.description.as_deref(),
                CustomTest::SimpleKeyValue(test) => {
                    test.values().next().and_then(|v| v.description.as_deref())
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // dbt Core accepts a sequence-valued `column_name` on a generic test (e.g.
    // `unique: {column_name: [a, b]}`) and forwards it as an arbitrary macro kwarg.
    // `column_name` must accept both a scalar string and a sequence in every YAML shape
    // a generic test can take, or YAML loading fails before the test ever runs (#13607).

    #[test]
    fn multi_key_test_accepts_sequence_column_name() {
        let yaml = r#"
unique:
    column_name: [customer_id, event_date]
"#;
        let test: DataTests = dbt_yaml::from_str(yaml).expect("multi-key form should deserialize");
        assert_eq!(
            test.column_name(),
            Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "customer_id".to_string(),
                "event_date".to_string(),
            ]))
        );
    }

    #[test]
    fn multi_key_test_accepts_scalar_column_name() {
        let yaml = r#"
unique:
    column_name: customer_id
"#;
        let test: DataTests = dbt_yaml::from_str(yaml).expect("multi-key form should deserialize");
        assert_eq!(
            test.column_name(),
            Some(StringOrArrayOfStrings::String("customer_id".to_string()))
        );
    }

    #[test]
    fn simple_key_value_test_accepts_sequence_column_name() {
        // The shape used for a custom (non-builtin) test invoked as `namespace.test_name: {...}`,
        // which is also legal directly under a column's `data_tests:` list.
        let yaml = r#"
dbt_utils.equal_rowcount:
    column_name: [customer_id, event_date]
"#;
        let test: DataTests =
            dbt_yaml::from_str(yaml).expect("simple key-value form should deserialize");
        assert_eq!(
            test.column_name(),
            Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "customer_id".to_string(),
                "event_date".to_string(),
            ]))
        );
    }

    #[test]
    fn bare_string_test_has_no_description() {
        let yaml = "unique";
        let test: DataTests =
            dbt_yaml::from_str(yaml).expect("bare string form should deserialize");
        assert_eq!(test.description(), None);
    }

    #[test]
    fn multi_key_test_carries_description() {
        // The `{test_name: ..., ...}` shape (as opposed to `{<test-name>: {...}}`).
        let yaml = r#"
test_name: unique
description: Order id must be unique across all orders.
"#;
        let test: DataTests = dbt_yaml::from_str(yaml).expect("multi-key form should deserialize");
        assert_eq!(
            test.description(),
            Some("Order id must be unique across all orders.")
        );
    }

    #[test]
    fn multi_key_test_without_description_is_none() {
        let yaml = r#"
test_name: unique
column_name: id
"#;
        let test: DataTests = dbt_yaml::from_str(yaml).expect("multi-key form should deserialize");
        assert_eq!(test.description(), None);
    }

    #[test]
    fn simple_key_value_test_carries_description() {
        let yaml = r#"
unique:
    description: Order id must be unique across all orders.
"#;
        let test: DataTests =
            dbt_yaml::from_str(yaml).expect("simple key-value form should deserialize");
        assert_eq!(
            test.description(),
            Some("Order id must be unique across all orders.")
        );
    }
}
