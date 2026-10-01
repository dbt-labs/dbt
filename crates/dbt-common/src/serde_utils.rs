use std::{fmt, marker::PhantomData};

use crate::dashmap::DashMap;
use indexmap::IndexMap;
use minijinja::value::ValueMap;
use minijinja::value::mutable_vec::MutableVec;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{Visitor, value::UnitDeserializer},
};

type YmlValue = dbt_yaml::Value;

/// Converts a [dbt_yaml::Value] to a [minijinja::Value]
fn convert_yml_value(yml: YmlValue) -> minijinja::Value {
    match yml {
        YmlValue::Mapping(map, _) => {
            let mut value_map = ValueMap::new();
            for (k, v) in map {
                value_map.insert(
                    minijinja::Value::from(
                        k.as_scalar_string()
                            .expect("mapping key is not a scalar")
                            .as_ref(),
                    ),
                    convert_yml_value(v),
                );
            }
            minijinja::Value::from_object(value_map)
        }
        YmlValue::Sequence(arr, _) => {
            let items: MutableVec<minijinja::Value> =
                arr.into_iter().map(convert_yml_value).collect();
            minijinja::Value::from_object(items)
        }
        YmlValue::Null(_) => minijinja::Value::from(None::<()>),
        _ => minijinja::Value::from_serialize(yml),
    }
}

/// Converts a [dbt_yaml::Value] to a [DashMap<String, Value>]
pub fn convert_yml_to_dash_map(yml: YmlValue) -> DashMap<String, minijinja::Value> {
    match yml {
        YmlValue::Mapping(map, _) => {
            let value_map = DashMap::default();
            for (k, v) in map {
                value_map.insert(
                    k.as_scalar_string()
                        .expect("mapping key is not a scalar")
                        .into_owned(),
                    convert_yml_value(v),
                );
            }
            value_map
        }
        _ => {
            let map = DashMap::default();
            map.insert("value".to_string(), convert_yml_value(yml));
            map
        }
    }
}

/// Converts a [dbt_yaml::Value] to a [minijinja::Value], preserving order of keys if the value is a mapping
// TODO(anna): This now converts to an ordered map by using `ValueMap`, with the assertion that the key is a string.
// But as I mentioned below, if we implement Object for IndexMap<String, minijinja::Value>, then we don't have to wrap the
// key in a value.
// TODO: We are losing span info for error reporting
fn convert_yml_value_ordered(yml: YmlValue) -> minijinja::Value {
    match yml {
        YmlValue::Mapping(map, _) => {
            let mut value_map = ValueMap::new();
            for (k, v) in map {
                value_map.insert(
                    minijinja::Value::from(
                        k.as_scalar_string()
                            .expect("mapping key is not a scalar")
                            .as_ref(),
                    ),
                    convert_yml_value(v),
                );
            }
            minijinja::Value::from_object(value_map)
        }
        YmlValue::Sequence(arr, _) => {
            let items: MutableVec<minijinja::Value> =
                arr.into_iter().map(convert_yml_value).collect();
            minijinja::Value::from_object(items)
        }
        YmlValue::Null(_) => minijinja::Value::from(None::<()>),
        _ => minijinja::Value::from_serialize(yml),
    }
}

/// Converts a dbt_yaml::Value to an order-preserving minijinja::ValueMap, only converting the first level to a map
// See: https://docs.rs/minijinja/2.5.0/minijinja/value/trait.Object.html#foreign-impls
pub fn convert_yml_to_value_map(yml: YmlValue) -> IndexMap<String, minijinja::Value> {
    match yml {
        YmlValue::Mapping(map, _) => {
            let mut value_map = IndexMap::with_capacity(map.len());
            for (k, v) in map {
                value_map.insert(
                    k.as_scalar_string()
                        .expect("mapping key is not a scalar")
                        .into_owned(),
                    convert_yml_value_ordered(v),
                );
            }
            value_map
        }
        _ => {
            let mut map = IndexMap::new();
            map.insert("value".to_string(), convert_yml_value_ordered(yml));
            map
        }
    }
}

/// A wrapper around a value that can be either present or omitted.
///
/// This is a counterpart to the `Option` type, intended for use in
/// deserialization scenarios where explicit `null` values should be treated as
/// distinct from omitted values.
#[derive(Default, Debug)]
pub enum Omissible<T> {
    /// The value is present and valid.
    Present(T),

    #[default]
    /// The value is omitted.
    Omitted,
}

impl<T> Clone for Omissible<T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        match self {
            Omissible::Present(value) => Omissible::Present(value.clone()),
            Omissible::Omitted => Omissible::Omitted,
        }
    }
}

impl<T> PartialEq for Omissible<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Omissible::Present(a), Omissible::Present(b)) => a == b,
            (Omissible::Omitted, Omissible::Omitted) => true,
            _ => false,
        }
    }
}

impl<T> Eq for Omissible<T> where T: Eq {}

impl<T> PartialOrd for Omissible<T>
where
    T: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Omissible::Present(a), Omissible::Present(b)) => a.partial_cmp(b),
            (Omissible::Omitted, Omissible::Omitted) => Some(std::cmp::Ordering::Equal),
            _ => None,
        }
    }
}

impl<T> Ord for Omissible<T>
where
    T: Ord,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Omissible::Present(a), Omissible::Present(b)) => a.cmp(b),
            (Omissible::Omitted, Omissible::Omitted) => std::cmp::Ordering::Equal,
            _ => std::cmp::Ordering::Less,
        }
    }
}

impl<T> std::hash::Hash for Omissible<T>
where
    T: std::hash::Hash,
{
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Omissible::Present(value) => value.hash(state),
            Omissible::Omitted => state.write_u8(0),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////

struct OmissibleVisitor<T> {
    marker: PhantomData<T>,
}

impl<'de, T> Visitor<'de> for OmissibleVisitor<T>
where
    T: Deserialize<'de>,
{
    type Value = Omissible<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("omissible value")
    }

    #[inline]
    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        // This function is called by the dbt_yaml deserializers for
        // explicit null values
        T::deserialize(UnitDeserializer::new()).map(Omissible::Present)
    }

    #[inline]
    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        // This function is called by the "missing_field" handler generated by
        // serde_derive
        Ok(Omissible::Omitted)
    }

    #[inline]
    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Omissible::Present)
    }
}

impl<'de, T> Deserialize<'de> for Omissible<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_option(OmissibleVisitor {
            marker: PhantomData,
        })
    }
}

impl<T> Serialize for Omissible<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Omissible::Present(value) => value.serialize(serializer),
            Omissible::Omitted => serializer.serialize_none(),
        }
    }
}

impl<T> schemars::JsonSchema for Omissible<T>
where
    T: schemars::JsonSchema,
{
    fn schema_name() -> String {
        T::schema_name()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        T::json_schema(generator)
    }

    fn is_referenceable() -> bool {
        T::is_referenceable()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        T::schema_id()
    }

    #[doc(hidden)]
    fn _schemars_private_non_optional_json_schema(
        generator: &mut schemars::r#gen::SchemaGenerator,
    ) -> schemars::schema::Schema {
        T::_schemars_private_non_optional_json_schema(generator)
    }

    #[doc(hidden)]
    fn _schemars_private_is_option() -> bool {
        true
    }
}

impl<T> Omissible<T> {
    /// Returns `true` if the value is present.
    pub fn is_present(&self) -> bool {
        matches!(self, Omissible::Present(_))
    }

    /// Returns `true` if the value is omitted.
    pub fn is_omitted(&self) -> bool {
        matches!(self, Omissible::Omitted)
    }

    /// Returns a reference to the value if it is present.
    pub fn as_ref(&self) -> Option<&T> {
        match self {
            Omissible::Present(value) => Some(value),
            Omissible::Omitted => None,
        }
    }

    /// Returns a mutable reference to the value if it is present.
    pub fn as_mut(&mut self) -> Option<&mut T> {
        match self {
            Omissible::Present(value) => Some(value),
            Omissible::Omitted => None,
        }
    }

    /// Returns the value if it is present, or the provided default value if it is omitted.
    pub fn unwrap_or(self, default: T) -> T {
        match self {
            Omissible::Present(value) => value,
            Omissible::Omitted => default,
        }
    }

    /// Returns the value if it is present, or the result of the provided function if it is omitted.
    pub fn unwrap_or_else<F>(self, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        match self {
            Omissible::Present(value) => value,
            Omissible::Omitted => f(),
        }
    }

    /// Converts the `Omissible` value into an `Option<T>`.
    pub fn into_inner(self) -> Option<T> {
        match self {
            Omissible::Present(value) => Some(value),
            Omissible::Omitted => None,
        }
    }
}

impl<T> From<T> for Omissible<T> {
    fn from(value: T) -> Self {
        Omissible::Present(value)
    }
}

impl<T> From<Option<T>> for Omissible<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(v) => Omissible::Present(v),
            None => Omissible::Omitted,
        }
    }
}

/// Read an optional boolean from a YAML mapping.
///
/// Accepts a YAML `Bool` literal or a string parseable by
/// [`crate::string_utils::try_parse_bool_str`]. Missing keys return
/// `Ok(None)`; other YAML shapes or unparseable strings return an
/// `InvalidConfig` error.
pub fn try_get_bool(m: &dbt_yaml::Mapping, k: &str) -> crate::FsResult<Option<bool>> {
    use crate::string_utils::try_parse_bool_str;
    use crate::{ErrorCode, fs_err};
    match m.get(dbt_yaml::Value::from(k)) {
        None => Ok(None),
        Some(v) => match v {
            dbt_yaml::Value::Bool(b, _) => Ok(Some(*b)),
            dbt_yaml::Value::String(s, _) => try_parse_bool_str(Some(s.as_str()), k).map_err(|e| {
                fs_err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(v.span().clone()),
                    "{}",
                    e.message()
                )
            }),
            _ => Err(fs_err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(v.span().clone()),
                "Key '{}' must be a boolean",
                k
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_test_primitives::assert_contains;
    use dbt_yaml::JsonSchema;
    use indoc::indoc;
    use schemars::schema_for;

    #[test]
    fn test_omissible() {
        #[derive(Deserialize, Serialize, Debug, PartialEq, JsonSchema)]
        struct TestStruct {
            field: Omissible<String>,
        }

        let yaml = r#"
            field: "value"
        "#;
        let value: TestStruct = dbt_yaml::from_str(yaml).unwrap();
        let expected = TestStruct {
            field: Omissible::Present("value".to_string()),
        };
        assert_eq!(value, expected);

        let yaml = r#"
        "#;
        let value: TestStruct = dbt_yaml::from_str(yaml).unwrap();
        let expected = TestStruct {
            field: Omissible::Omitted,
        };
        assert_eq!(value, expected);

        let yaml = r#"
            field: 
        "#;
        let err = dbt_yaml::from_str::<TestStruct>(yaml).unwrap_err();
        assert_contains!(
            err.to_string(),
            "invalid type: unit value, expected a string"
        );

        let yaml = r#"
            field: null
        "#;
        let err = dbt_yaml::from_str::<TestStruct>(yaml).unwrap_err();
        assert_contains!(
            err.to_string(),
            "invalid type: unit value, expected a string"
        );
    }

    #[test]
    fn test_omissible_option() {
        #[derive(Deserialize, Serialize, Debug, PartialEq, JsonSchema)]
        struct TestStruct {
            field: Omissible<Option<String>>,
        }
        let yaml = r#"
            field: "value"
        "#;
        let value: TestStruct = dbt_yaml::from_str(yaml).unwrap();
        let expected = TestStruct {
            field: Omissible::Present(Some("value".to_string())),
        };
        assert_eq!(value, expected);

        let yaml = r#"
        "#;
        let value: TestStruct = dbt_yaml::from_str(yaml).unwrap();
        let expected = TestStruct {
            field: Omissible::Omitted,
        };
        assert_eq!(value, expected);

        let yaml = r#"
            field: 
        "#;
        let value: TestStruct = dbt_yaml::from_str(yaml).unwrap();
        let expected = TestStruct {
            field: Omissible::Present(None),
        };
        assert_eq!(value, expected);

        let yaml = r#"
            field: null
        "#;
        let value: TestStruct = dbt_yaml::from_str(yaml).unwrap();
        let expected = TestStruct {
            field: Omissible::Present(None),
        };
        assert_eq!(value, expected);
    }

    #[test]
    fn test_omissible_jsonschema() {
        #[derive(Deserialize, Serialize, Debug, PartialEq, JsonSchema)]
        struct TestStruct {
            required_field: String,
            omissible_field: Omissible<String>,
            omissible_option_field: Omissible<Option<String>>,
        }
        let schema = schema_for!(TestStruct);
        let schema_str = dbt_yaml::to_string(&schema).unwrap();
        println!("{schema_str}");
        assert_eq!(
            schema_str,
            indoc! {"
$schema: http://json-schema.org/draft-07/schema#
title: TestStruct
type: object
required:
- required_field
properties:
  omissible_field:
    type: string
  omissible_option_field:
    type:
    - string
    - 'null'
  required_field:
    type: string
"}
        );
    }

    #[test]
    fn test_convert_yml_mapping_preserves_insertion_order() {
        // Regression test: YAML mappings must render with keys in insertion order,
        // not sorted alphabetically, when embedded in compiled Python models
        // (e.g. lifetime/days/score dicts in config_dict / meta_dict).
        //
        // Keys here are deliberately NOT in alphabetical order:
        //   insertion: ID, TH, PH
        //   alphabetical (BTreeMap): ID, PH, TH  ← P sorts before T
        let yaml = "ID: 30\nTH: 30\nPH: 30\n";
        let yml_value: YmlValue = dbt_yaml::from_str(yaml).unwrap();

        let mj_value = convert_yml_value(yml_value);
        let rendered = mj_value.to_string();

        assert_eq!(
            rendered, "{'ID': 30, 'TH': 30, 'PH': 30}",
            "expected insertion order but got: {rendered}"
        );
    }

    #[test]
    fn test_convert_yml_sequence_renders_as_list_not_tuple() {
        // Regression test: YAML sequences must render as Python lists `[...]`,
        // not as Python tuples `(...)`, when embedded in compiled Python models
        // (e.g. config_dict / meta_dict in the py_script_postfix template).
        let yaml = "- ID\n- TH\n- PH\n- SG\n";
        let yml_value: YmlValue = dbt_yaml::from_str(yaml).unwrap();

        let mj_value = convert_yml_value(yml_value);
        let rendered = mj_value.to_string();

        assert!(
            rendered.starts_with('['),
            "expected list syntax `[...]` but got: {rendered}"
        );
        assert!(
            rendered.ends_with(']'),
            "expected list syntax `[...]` but got: {rendered}"
        );
        assert_eq!(rendered, "['ID', 'TH', 'PH', 'SG']");
    }

    #[test]
    fn try_get_bool_missing_key_yields_none() {
        let m: dbt_yaml::Mapping = dbt_yaml::from_str("other_key: true").unwrap();
        assert_eq!(try_get_bool(&m, "use_uniform").unwrap(), None);
    }

    #[test]
    fn try_get_bool_yaml_bool_literal() {
        let m: dbt_yaml::Mapping = dbt_yaml::from_str("k: true").unwrap();
        assert_eq!(try_get_bool(&m, "k").unwrap(), Some(true));
        let m: dbt_yaml::Mapping = dbt_yaml::from_str("k: false").unwrap();
        assert_eq!(try_get_bool(&m, "k").unwrap(), Some(false));
    }

    #[test]
    fn try_get_bool_yaml_string_with_casing_and_whitespace() {
        let m: dbt_yaml::Mapping = dbt_yaml::from_str(r#"k: "True""#).unwrap();
        assert_eq!(try_get_bool(&m, "k").unwrap(), Some(true));
        let m: dbt_yaml::Mapping = dbt_yaml::from_str(r#"k: "  TRUE  ""#).unwrap();
        assert_eq!(try_get_bool(&m, "k").unwrap(), Some(true));
        let m: dbt_yaml::Mapping = dbt_yaml::from_str(r#"k: "false""#).unwrap();
        assert_eq!(try_get_bool(&m, "k").unwrap(), Some(false));
    }

    #[test]
    fn try_get_bool_unparseable_string_errors() {
        let m: dbt_yaml::Mapping = dbt_yaml::from_str(r#"k: "yes""#).unwrap();
        let err = try_get_bool(&m, "k").unwrap_err();
        assert!(err.to_string().contains(r#"expected "true" or "false""#));
    }

    #[test]
    fn try_get_bool_wrong_yaml_type_errors() {
        let m: dbt_yaml::Mapping = dbt_yaml::from_str("k: 42").unwrap();
        let err = try_get_bool(&m, "k").unwrap_err();
        assert!(err.to_string().contains("must be a boolean"));
    }
}
