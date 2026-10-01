use std::borrow::Cow;

use dbt_adbc::{Backend, database};

use crate::{Auth, AuthError};

pub use dbt_yaml::Value as YmlValue;

// TODO(felipecrv): move this struct for generic use as it now has nothing specific to adapters

#[derive(Clone, Debug, Default)]
pub struct AdapterConfig {
    repr: dbt_yaml::Mapping,
}

pub(crate) fn yml_value_to_string<'a>(value: &'a YmlValue) -> Cow<'a, str> {
    // This function exists because `dbt_yaml::to_string` appends
    // a newline to the end of every string. And, less importantly, it
    // also copies values that are strings already.
    match value {
        YmlValue::Sequence(_, _) | YmlValue::Mapping(_, _) => {
            let res = dbt_yaml::to_string(value);
            debug_assert!(
                res.is_ok(),
                "failed to convert sequence/mapping to string: {res:?}",
            );
            let mut s = res.unwrap();
            if s.ends_with('\n') {
                s.pop();
            }
            Cow::Owned(s)
        }
        YmlValue::Tagged(tagged_value, _) => yml_value_to_string(&tagged_value.value),
        other => other.as_scalar_string().expect("is a scalar"),
    }
}

impl AdapterConfig {
    pub fn new(mapping: dbt_yaml::Mapping) -> Self {
        Self { repr: mapping }
    }

    /// Get the underlying YAML representation of the configuration.
    pub fn repr(&self) -> &dbt_yaml::Mapping {
        &self.repr
    }

    /// Checks if the config contains the given key.
    pub fn contains_key(&self, field: &str) -> bool {
        self.repr.contains_key(field)
    }

    /// Get a value from a map or return None.
    pub fn get(&self, field: &str) -> Option<&YmlValue> {
        self.repr.get(field)
    }

    /// Like `get`, but returns an error if the field is missing.
    pub fn require(&self, field: &str) -> Result<&YmlValue, dbt_yaml::Error> {
        use serde::de::Error as _;
        // Re-implementation of [dbt_yaml::Error::missing_field]
        // that doesn't require a &'static str field name.
        let err = || dbt_yaml::Error::custom(format_args!("missing field `{field}`"));
        self.get(field).ok_or_else(err)
    }

    /// Like `get`, but calls `to_string` on the value.
    pub fn get_string(&self, field: &str) -> Option<Cow<'_, str>> {
        self.get(field).map(yml_value_to_string)
    }

    /// Get a direct reference to a string value if it exists and is a string.
    /// This returns a borrow tied to the lifetime of the AdapterConfig itself.
    pub fn get_str(&self, field: &str) -> Option<&str> {
        self.get(field)?.as_str()
    }

    /// Like `require`, but returns a borrowed string.
    pub fn require_str(&self, field: &str) -> Result<&str, dbt_yaml::Error> {
        use serde::de::Error as _;
        // Re-implementation of [dbt_yaml::Error::missing_field]
        // that doesn't require a &'static str field name.
        let err = || dbt_yaml::Error::custom(format_args!("missing field `{field}`"));
        self.get_str(field).ok_or_else(err)
    }

    /// Like `require`, but calls `to_string` on the value.
    pub fn require_string(&self, field: &str) -> Result<Cow<'_, str>, dbt_yaml::Error> {
        self.require(field).map(yml_value_to_string)
    }

    /// Get a boolean value if the field exists and parses as a YAML bool.
    pub fn get_bool(&self, field: &str) -> Option<bool> {
        let value = self.get(field)?;
        value.as_bool()
    }

    /// Whether this config indicates that authentication should be handled by
    /// dbt Cloud credentials (via the flock/remote driver) instead of
    /// adapter-specific auth.
    pub fn use_dbt_cloud_credentials(&self) -> bool {
        self.get("use_dbt_cloud_credentials")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    pub fn build_connection_builder(
        &self,
        auth: &dyn Auth,
        configure_cloud_database: impl FnOnce(Backend) -> Result<database::Builder, AuthError>,
    ) -> Result<database::Builder, AuthError> {
        if self.use_dbt_cloud_credentials() {
            configure_cloud_database(auth.backend())
        } else {
            auth.configure(self)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_ra3_node_config() {
        let mapping = dbt_yaml::Mapping::from_iter([
            ("ra3_node_bool".into(), YmlValue::bool(true)),
            ("ra3_node_str".into(), YmlValue::string("true".to_string())),
        ]);
        let config = AdapterConfig::new(mapping);

        assert!(config.get_string("ra3_node").is_none());

        let ra3_node = config
            .get_string("ra3_node_MISSING")
            .unwrap_or(Cow::Borrowed("false"));
        let ra3_node: bool = FromStr::from_str(&ra3_node).unwrap();
        assert!(!ra3_node);

        let ra3_node = config
            .get_string("ra3_node_bool")
            .unwrap_or(Cow::Borrowed("false"));
        let ra3_node: bool = FromStr::from_str(&ra3_node).unwrap();
        assert!(ra3_node);
    }

    #[test]
    fn test_yaml_value_conversions() {
        let mapping = dbt_yaml::Mapping::from_iter([
            ("null".into(), YmlValue::null()),
            ("bool".into(), YmlValue::bool(true)),
            ("i64".into(), YmlValue::number(42i64.into())),
            ("u64".into(), YmlValue::number(42u64.into())),
            ("f64".into(), YmlValue::number(42.0f64.into())),
            ("string".into(), YmlValue::string("test".to_string())),
            (
                "sequence".into(),
                YmlValue::sequence(vec!["abra".into(), "cadabra".into()]),
            ),
            (
                "mapping".into(),
                YmlValue::mapping(dbt_yaml::Mapping::from_iter([
                    ("key1".into(), "value1".into()),
                    ("key2".into(), "value2".into()),
                ])),
            ),
        ]);
        let config = AdapterConfig::new(mapping);
        let get_to_string = |key: &str| config.get_string(key).unwrap();
        assert_eq!(get_to_string("null"), "null");
        assert_eq!(get_to_string("bool"), "true");
        assert_eq!(get_to_string("i64"), "42");
        assert_eq!(get_to_string("u64"), "42");
        assert_eq!(get_to_string("f64"), "42.0");
        assert_eq!(get_to_string("string"), "test");
        assert_eq!(
            get_to_string("sequence"),
            r#"- abra
- cadabra"#
        );
        assert_eq!(
            get_to_string("mapping"),
            r#"key1: value1
key2: value2"#
        );

        // check with values that came from a YAML document
        let value: YmlValue = dbt_yaml::from_str(
            r#"
test:
  type: snowflake

  account: 'test_account'

  role: INTEGRATION_TEST

"#,
        )
        .unwrap();
        let test = value.get("test").unwrap();
        let ty = test.get("type").unwrap();
        assert_eq!(yml_value_to_string(ty), "snowflake");
        let account = test.get("account").unwrap();
        assert_eq!(yml_value_to_string(account), "test_account");
        let role = test.get("role").unwrap();
        assert_eq!(yml_value_to_string(role), "INTEGRATION_TEST");

        let s = yml_value_to_string(test);
        assert_eq!(
            s,
            r#"type: snowflake
account: test_account
role: INTEGRATION_TEST"#
        );
    }
}
