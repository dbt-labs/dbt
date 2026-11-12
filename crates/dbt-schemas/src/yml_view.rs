//! Borrowed, read-only accessors over a YAML mapping.

use std::borrow::Cow;

use dbt_yaml::{Mapping, Value as YmlValue};

pub fn yml_value_to_string<'a>(value: &'a YmlValue) -> Cow<'a, str> {
    // This function exists because `dbt_yaml::to_string` appends
    // a newline to the end of every string. And, less importantly, it
    // also copies values that are strings already.
    match value {
        YmlValue::Null(_) => Cow::Borrowed("null"),
        YmlValue::Bool(b, _) => Cow::Borrowed(if *b { "true" } else { "false" }),
        YmlValue::Number(n, _) => Cow::Owned(n.to_string()),
        YmlValue::String(s, _) => Cow::Borrowed(s),
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
    }
}

/// A borrowed view of a YAML mapping. Cheap to copy.
#[derive(Clone, Copy, Debug)]
pub struct YmlView<'a> {
    inner: &'a Mapping,
}

impl<'a> YmlView<'a> {
    pub fn new(mapping: &'a Mapping) -> Self {
        Self { inner: mapping }
    }

    /// A view of `value` if it is a mapping.
    pub fn from_value(value: &'a YmlValue) -> Option<Self> {
        value.as_mapping().map(Self::new)
    }

    /// Get the underlying YAML representation of the configuration.
    pub fn inner(&self) -> &'a Mapping {
        self.inner
    }

    /// Checks if the config contains the given key.
    pub fn contains_key(&self, field: &str) -> bool {
        self.inner.contains_key(field)
    }

    /// Get a value from a map or return None.
    pub fn get(&self, field: &str) -> Option<&'a YmlValue> {
        self.inner.get(field)
    }

    /// Like `get`, but returns an error if the field is missing.
    pub fn require(&self, field: &str) -> Result<&'a YmlValue, dbt_yaml::Error> {
        use serde::de::Error as _;
        // Re-implementation of [dbt_yaml::Error::missing_field]
        // that doesn't require a &'static str field name.
        let err = || dbt_yaml::Error::custom(format_args!("missing field `{field}`"));
        self.get(field).ok_or_else(err)
    }

    /// Like `get`, but calls `to_string` on the value.
    pub fn get_string(&self, field: &str) -> Option<Cow<'a, str>> {
        self.get(field).map(yml_value_to_string)
    }

    /// Get a direct reference to a string value if it exists and is a string.
    /// This returns a borrow tied to the lifetime of the mapping itself.
    pub fn get_str(&self, field: &str) -> Option<&'a str> {
        self.get(field)?.as_str()
    }

    /// Get a direct reference to a string value if it exists and is a string,
    /// and is non-empty.
    /// This returns a borrow tied to the lifetime of the mapping itself.
    pub fn get_str_non_empty(&self, field: &str) -> Option<&'a str> {
        self.get(field)?.as_str().filter(|s| !s.is_empty())
    }

    /// Like `require`, but returns a borrowed string.
    pub fn require_str(&self, field: &str) -> Result<&'a str, dbt_yaml::Error> {
        use serde::de::Error as _;
        // Re-implementation of [dbt_yaml::Error::missing_field]
        // that doesn't require a &'static str field name.
        let err = || dbt_yaml::Error::custom(format_args!("missing field `{field}`"));
        self.get_str(field).ok_or_else(err)
    }

    /// Like `require`, but calls `to_string` on the value.
    pub fn require_string(&self, field: &str) -> Result<Cow<'a, str>, dbt_yaml::Error> {
        self.require(field).map(yml_value_to_string)
    }

    /// Get a boolean value if the field exists and parses as a YAML bool.
    pub fn get_bool(&self, field: &str) -> Option<bool> {
        let value = self.get(field)?;
        value.as_bool()
    }

    /// A view of a nested mapping, if the field exists and is a mapping.
    pub fn get_mapping(&self, field: &str) -> Option<YmlView<'a>> {
        Self::from_value(self.get(field)?)
    }
}

impl<'a> From<&'a Mapping> for YmlView<'a> {
    fn from(mapping: &'a Mapping) -> Self {
        Self::new(mapping)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> Mapping {
        Mapping::from_iter([
            ("str".into(), YmlValue::string("text".to_string())),
            ("empty".into(), YmlValue::string(String::new())),
            ("bool".into(), YmlValue::bool(true)),
            ("num".into(), YmlValue::number(42i64.into())),
            (
                "nested".into(),
                YmlValue::mapping(Mapping::from_iter([("k".into(), "v".into())])),
            ),
        ])
    }

    #[test]
    fn str_vs_string_accessors() {
        let m = mapping();
        let view = YmlView::new(&m);
        assert_eq!(view.get_str("str"), Some("text"));
        assert_eq!(view.get_str("num"), None);
        assert_eq!(view.get_string("num").as_deref(), Some("42"));
        assert_eq!(view.get_string("bool").as_deref(), Some("true"));
        assert_eq!(view.get_str_non_empty("empty"), None);
        assert_eq!(view.get_bool("bool"), Some(true));
        assert!(view.get_str("missing").is_none());
        assert!(view.require_str("missing").is_err());
    }

    #[test]
    fn values_outlive_the_view() {
        let m = mapping();
        let text = {
            let view = YmlView::new(&m);
            view.get_mapping("nested").unwrap().get_str("k").unwrap()
        };
        assert_eq!(text, "v");
    }
}
