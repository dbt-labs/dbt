use crate::schemas::common::DocsConfig;
use crate::schemas::manifest::postgres::PostgresIndex;
use crate::schemas::properties::model_properties::ModelConstraint;
use dbt_adapter_core::AdapterType;
use dbt_common::serde_utils::Omissible;
use dbt_common::{CodeLocationWithFile, ErrorCode, FsError, FsResult, stdfs};
use dbt_proc_macros::StringOrArrayNewtype;
use dbt_yaml::{DbtSchema, Spanned, UntaggedEnumDeserialize};
use indexmap::IndexMap;
use minijinja::value::ValueKind;
use serde::{
    self, Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeOwned},
};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

pub use dbt_jinja_vars::yml_value_to_minijinja;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;
type MinijinjaValue = minijinja::Value;

/// Deserializes a JSON file into a `T`, using the file's absolute path for error reporting.
pub fn typed_struct_from_json_file<T>(path: &Path) -> FsResult<T>
where
    T: DeserializeOwned,
{
    // Note: Do **NOT** open the file and parse as JSON directly using
    // `serde_json::from_reader`! That will be ~30x slower.
    let json_str = stdfs::read_to_string(path)?;

    typed_struct_from_json_str(&json_str, Some(path))
}

pub fn typed_struct_to_pretty_json_file<T>(path: &Path, value: &T) -> FsResult<()>
where
    T: Serialize,
{
    let yml_val = dbt_yaml::to_value(value).map_err(|e| {
        FsError::new(
            ErrorCode::SerializationError,
            format!("Failed to convert to YAML: {e}"),
        )
    })?;
    let file = std::fs::File::create(path).map_err(|e| {
        FsError::new(
            ErrorCode::SerializationError,
            format!("Failed to create file: {e}"),
        )
    })?;
    serde_json::to_writer_pretty(file, &yml_val).map_err(|e| {
        FsError::new(
            ErrorCode::SerializationError,
            format!("Failed to write to file: {e}"),
        )
    })?;
    Ok(())
}

/// Deserializes a JSON string into a `T`.
pub fn typed_struct_from_json_str<T>(json_str: &str, source: Option<&Path>) -> FsResult<T>
where
    T: DeserializeOwned,
{
    let yml_val: YmlValue = serde_json::from_str(json_str).map_err(|e| {
        FsError::new(
            ErrorCode::SerializationError,
            format!("Failed to parse JSON: {e}"),
        )
    })?;

    T::deserialize(yml_val).map_err(|e| yaml_to_fs_error(e, source))
}

/// Converts a `dbt_yaml::Error` into a `FsError`, attaching the error location
pub fn yaml_to_fs_error(err: dbt_yaml::Error, filename: Option<&Path>) -> Box<FsError> {
    let msg = err.display_no_mark().to_string();
    let location = err
        .span()
        .map_or_else(CodeLocationWithFile::default, CodeLocationWithFile::from);
    let location = if let Some(filename) = filename {
        location.with_file(Arc::new(filename.into()))
    } else {
        location
    };

    if let Some(err) = err.into_external()
        && let Ok(err) = err.downcast::<FsError>()
    {
        // These are errors raised from our own callbacks:
        return err;
    }
    FsError::new(ErrorCode::SerializationError, format!("YAML error: {msg}"))
        .with_location(location)
        .into()
}

/// Serialize an `Option<T>` as an empty map `{}` when `None`.
pub fn serialize_none_as_empty_map<S, T>(val: &Option<T>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize,
{
    use serde::ser::SerializeMap;
    match val {
        Some(v) => v.serialize(serializer),
        None => serializer.serialize_map(Some(0))?.end(),
    }
}

/// Serialize an `Option<T>` as `T::default()` when `None`. Use for fields where
/// dbt-core always emits a default value (e.g. `{}`, `[]`, default enum variant)
/// instead of omitting/null.
pub fn serialize_none_as_default<S, T>(val: &Option<T>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Default + Serialize,
{
    match val {
        Some(v) => v.serialize(serializer),
        None => T::default().serialize(serializer),
    }
}

/// Deserialize inverse of [`serialize_none_as_default`]: deserialize a `T` and
/// collapse it back to `None` when it equals `T::default()`.
///
/// Pair this with `serialize_none_as_default` on `Option<T>` fields so the value
/// round-trips (`None -> default -> None`). Without it, a field serialized as its
/// default (e.g. `""`, `{}`) reads back as `Some(default)` instead of `None`,
/// which breaks typed `state:modified` comparisons. See dbt-core#15513.
///
/// Deserializes via `Option<T>` so an explicit `null` (produced by dbt-core and older
/// Fusion manifests) also collapses to `None` rather than erroring.
pub fn deserialize_default_as_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Default + PartialEq + Deserialize<'de>,
{
    let value = Option::<T>::deserialize(deserializer)?;
    Ok(match value {
        Some(v) if v == T::default() => None,
        other => other,
    })
}

/// Deserialize inverse of [`serialize_none_as_empty_map`] for `Option<IndexMap>`:
/// collapse an empty map `{}` back to `None`.
pub fn deserialize_empty_map_as_none<'de, D>(
    deserializer: D,
) -> Result<Option<IndexMap<String, YmlValue>>, D::Error>
where
    D: Deserializer<'de>,
{
    let map = Option::<IndexMap<String, YmlValue>>::deserialize(deserializer)?;
    Ok(match map {
        Some(m) if m.is_empty() => None,
        other => other,
    })
}

/// Serialize a `DocsConfig` always including `node_color` as an explicit null when absent.
/// dbt-core writes manifests with omit_none=False, so node_color must be present.
pub fn serialize_docs_with_nulls<S>(docs: &DocsConfig, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(Some(2))?;
    map.serialize_entry("show", &docs.show)?;
    map.serialize_entry("node_color", &docs.node_color)?;
    map.end()
}

/// Serialize an `Option<DocsConfig>` — `None` becomes the default docs object with
/// explicit nulls; `Some(v)` also serializes with explicit nulls.
pub fn serialize_option_docs_with_nulls<S>(
    docs: &Option<DocsConfig>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let docs = docs.as_ref().cloned().unwrap_or_default();
    serialize_docs_with_nulls(&docs, serializer)
}

/// Serialize an `Option<Vec<T>>` as an empty array `[]` when `None`.
pub fn serialize_none_as_empty_vec<S, T>(
    val: &Option<Vec<T>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: Serialize,
{
    match val {
        Some(v) => v.serialize(serializer),
        None => <[T]>::serialize(&[], serializer),
    }
}

pub fn default_type<'de, D>(deserializer: D) -> Result<Option<IndexMap<String, YmlValue>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    match value {
        dbt_yaml::Value::Mapping(map, _) => Ok(Some(
            map.into_iter()
                .map(|(k, v)| {
                    let yml_val = dbt_yaml::from_value::<YmlValue>(v).unwrap_or(YmlValue::null());
                    (
                        k.as_scalar_string()
                            .expect("mapping key is not a scalar")
                            .into_owned(),
                        yml_val,
                    )
                })
                .collect(),
        )),
        dbt_yaml::Value::Null(_) => Ok(None),
        _ => Err(de::Error::custom("expected an object or null")),
    }
}

/// Deserialize a string or an array of strings into a vector of strings
pub fn string_or_array<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    match value {
        dbt_yaml::Value::Sequence(arr, _) => arr
            .iter()
            .map(|v| v.as_scalar_string().map(|s| s.into_owned()))
            .collect::<Option<Vec<_>>>()
            .map(Some)
            .ok_or_else(|| de::Error::custom("expected a string, an array of strings, or null")),
        dbt_yaml::Value::Null(_) => Ok(None),
        other => other
            .as_scalar_string()
            .map(|s| Some(vec![s.into_owned()]))
            .ok_or_else(|| de::Error::custom("expected a string, an array of strings, or null")),
    }
}

pub fn bool_or_string_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    Ok(value
        .as_bool()
        .or_else(|| value.as_str().map(|s| s.to_lowercase() == "true")))
}

/// Deserialize a boolean flag without silently treating unknown values as false.
pub fn strict_bool_or_string_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    match value {
        dbt_yaml::Value::Bool(value, _) => Ok(Some(value)),
        dbt_yaml::Value::Null(_) => Ok(None),
        dbt_yaml::Value::String(value, _) if value.eq_ignore_ascii_case("true") => Ok(Some(true)),
        dbt_yaml::Value::String(value, _) if value.eq_ignore_ascii_case("false") => Ok(Some(false)),
        _ => Err(de::Error::custom("expected true, false, or null")),
    }
}

pub fn bool_or_string_bool_default<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    Ok(value
        .as_bool()
        .or_else(|| value.as_str().map(|s| s.to_lowercase() == "true"))
        .unwrap_or_default())
}

pub fn u64_or_string_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok())))
}

/// Deserialize `hours_to_expiration`, distinguishing an omitted key (`Omitted`,
/// inherits) from an explicit `null` (`Present(None)`, which clears a value
/// inherited from a higher level of the `models:` hierarchy). Relies on
/// `#[serde(default)]` to produce `Omitted` for absent keys. See dbt-core#15473.
///
/// A present-but-non-numeric value (e.g. the rendered string "null") is kept as a
/// value rather than collapsed: dbt-core's `BigQueryAdapter.get_common_options`
/// emits `expiration_timestamp` whenever the config is present and not Python
/// `None`, interpolating `str(value)`, so "null" must survive to match. See
/// dbt-labs/fs#11681.
pub fn hours_to_expiration_or_string_omissible<'de, D>(
    deserializer: D,
) -> Result<Omissible<Option<StringOrInteger>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    match value {
        dbt_yaml::Value::Null(_) => Ok(Omissible::Present(None)),
        other => {
            if let Some(i) = other.as_i64() {
                Ok(Omissible::Present(Some(StringOrInteger::Integer(i))))
            } else if let Some(s) = other.as_str() {
                // Keeps the rendered "null" string as a value, distinct from null (#11681).
                Ok(Omissible::Present(Some(StringOrInteger::from(
                    s.to_string(),
                ))))
            } else {
                Ok(Omissible::Present(None))
            }
        }
    }
}

pub fn i64_or_string_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    Ok(value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok())))
}

pub fn f64_or_string_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    Ok(value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok())))
}

pub fn string_or_number_to_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    match value {
        dbt_yaml::Value::Null(_) => Ok(None),
        other => other
            .as_scalar_string()
            .map(|s| Some(s.into_owned()))
            .ok_or_else(|| de::Error::custom("expected a scalar or null")),
    }
}

/// Accepts a mapping or sequence for `event_time`, matching Core's lack of validation
/// (`event_time: Any` in dbt-core's `config.py`) so Fusion doesn't reject valid Core
/// projects. The JSON-stringified result is a placeholder, not a real column name — Core
/// doesn't resolve one either. See dbt-labs/fs#13343.
pub fn event_time_or_map_to_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value: Option<YmlValue> = Option::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        YmlValue::Mapping(_, _) | YmlValue::Sequence(_, _) => {
            let json_string = serde_json::to_string(&value)
                .map_err(|e| de::Error::custom(format!("Failed to serialize to JSON: {e}")))?;
            Ok(Some(json_string))
        }
        scalar => scalar
            .as_scalar_string()
            .map(|s| Some(s.into_owned()))
            .ok_or_else(|| de::Error::custom("expected a string, a map, or a sequence")),
    }
}

/// Serializes an optional YAML 1.1 timestamp as a fully-defaulted RFC 3339
/// string: a missing time defaults to midnight and a missing zone to UTC,
/// matching the semantics of dbt-core's manifest output for `deprecation_date`.
pub fn serialize_timestamp_as_rfc3339<S>(
    ts: &Option<dbt_yaml::Timestamp>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match ts {
        Some(ts) => serializer.serialize_some(&ts.with_defaults().to_string()),
        None => serializer.serialize_none(),
    }
}

/// Drops one trailing `\n` from each of `keys` in `value`, preserving spans. Must run before
/// Jinja: dbt-core strips the template source, so an expression's own newline survives.
pub fn strip_one_trailing_newline_at_keys(value: &mut YmlValue, keys: &[&str]) {
    for key in keys {
        let Some(field) = value.get_mut(*key) else {
            continue;
        };
        let Some(stripped) = field
            .as_str()
            .and_then(|s| s.strip_suffix('\n'))
            .map(str::to_string)
        else {
            continue;
        };
        let span = field.span().clone();
        *field = YmlValue::string(stripped).with_span(span);
    }
}

/// Accepts the sequence-valued `column_types` entries Core tolerates, joined into one string.
pub fn column_types_map<'de, D>(
    deserializer: D,
) -> Result<Option<BTreeMap<Spanned<String>, String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value: Option<BTreeMap<Spanned<String>, YmlValue>> = Option::deserialize(deserializer)?;
    let Some(map) = value else {
        return Ok(None);
    };
    map.into_iter()
        .map(|(key, value)| Ok((key, column_type_value_to_string(value)?)))
        .collect::<Result<_, D::Error>>()
        .map(Some)
}

// Like `event_time`/`query_tag` above, Core's `column_types` values are untyped
// (`Dict[str, Any]`) and never validated. Confirmed Core has no join/stringify logic for a
// sequence value either: its one in-repo consumer (seed CSV loading) only reads the dict's
// keys and discards the value, and the DDL-generating macro outside dbt-mantle bare-Jinja-
// interpolates it, producing a broken Python list repr in the SQL. This join is a Fusion-only
// convention to accept the shape without erroring, not a reproduction of any Core behavior.
fn column_type_value_to_string<E: de::Error>(value: YmlValue) -> Result<String, E> {
    match value {
        YmlValue::String(s, _) => Ok(s),
        YmlValue::Sequence(items, _) => items
            .into_iter()
            .map(column_type_value_to_string)
            .collect::<Result<Vec<_>, E>>()
            .map(|parts| parts.join(", ")),
        _ => Err(de::Error::custom(
            "expected a string or a sequence of strings",
        )),
    }
}

/// Accepts the scalar `policy_tags` Core tolerates in place of a single-element list.
pub fn policy_tags_from_scalar_or_list<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<StringOrMap>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value: Option<YmlValue> = Option::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        YmlValue::Sequence(_, _) => Vec::<StringOrMap>::deserialize(value)
            .map(Some)
            .map_err(|e| de::Error::custom(e.to_string())),
        _ => StringOrMap::deserialize(value)
            .map(|entry| Some(vec![entry]))
            .map_err(|e| de::Error::custom(e.to_string())),
    }
}

/// Accepts the mapping-valued `constraints` dbt-core tolerates. dbt-core declares no
/// `constraints` on its `ModelConfig`, so the value lands in the untyped `_extra` dict and never
/// reaches `node.constraints`: a mapping produces no constraint and no DDL. Dropping it matches
/// that outcome, and the authored value still survives in `unrendered_config`.
pub fn model_constraints_or_map<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<ModelConstraint>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value: Option<YmlValue> = Option::deserialize(deserializer)?;
    match value {
        None | Some(YmlValue::Null(_)) => Ok(None),
        // A mapping is not a constraint list on either engine; accept and drop it.
        Some(YmlValue::Mapping(_, _)) => Ok(None),
        Some(other) => Vec::<ModelConstraint>::deserialize(other)
            .map(Some)
            .map_err(|e| de::Error::custom(e.to_string())),
    }
}

/// Resolves the YAML 1.1 boolean token set PyYAML (and so dbt-core) accepts. Fusion's YAML 1.2
/// reader resolves only `true`/`false`, so an unquoted `no` arrives here as a string. Unlike
/// `bool_or_string_bool`, a token outside the set errors instead of silently becoming `false`.
/// Over-accepts a quoted `"no"` that dbt-core rejects: `dbt_yaml::Value` carries no scalar style,
/// so quoting is already lost by the time this runs.
pub fn yaml_11_bool_default<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    if let Some(b) = value.as_bool() {
        return Ok(b);
    }
    match value.as_str() {
        // PyYAML's bool resolver: these three casings only, and no bare `y`/`n`.
        Some("true" | "True" | "TRUE" | "yes" | "Yes" | "YES" | "on" | "On" | "ON") => Ok(true),
        Some("false" | "False" | "FALSE" | "no" | "No" | "NO" | "off" | "Off" | "OFF") => Ok(false),
        Some(other) => Err(de::Error::invalid_value(
            de::Unexpected::Str(other),
            &"a boolean",
        )),
        None => Err(de::Error::custom("expected a boolean")),
    }
}

pub fn default_true() -> Option<bool> {
    Some(true)
}

// =============================================================================
// QueryTag - Wrapper type for query_tag that converts maps/sequences to JSON strings
// =============================================================================
//
// This type matches dbt-core's query_tag behavior:
// - String values are kept as-is
// - Map/dict values are JSON-serialized to strings (Fusion-only; not what Core does)
// - Sequence/list values are JSON-serialized to strings (Fusion-only; not what Core does)
// - Always serializes as a string
// - Use as Option<QueryTag> for optional fields

/// A wrapper type for the `query_tag` config field that handles flexible deserialization.
///
/// Accepts strings, dictionaries, or sequences for query_tag. Maps and sequences
/// are JSON-serialized to strings.
///
/// # Example
///
/// ```ignore
/// // Input: "my-tag"
/// // Stores as: "my-tag"
///
/// // Input: {"project": "foo", "env": "prod"}
/// // Stores as: "{\"project\":\"foo\",\"env\":\"prod\"}"
/// ```
#[derive(Debug, Clone, PartialEq, Eq, DbtSchema)]
pub struct QueryTag(pub String);

impl QueryTag {
    /// Creates a QueryTag from a String
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Consumes self and returns the inner value
    pub fn into_inner(self) -> String {
        self.0
    }

    /// Returns a reference to the inner value
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for QueryTag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for QueryTag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = dbt_yaml::Value::deserialize(deserializer)?;
        match value {
            dbt_yaml::Value::Mapping(_, _) | dbt_yaml::Value::Sequence(_, _) => {
                // Fusion-only stringification (see module doc above) — not what Core does
                let json_string = serde_json::to_string(&value)
                    .map_err(|e| de::Error::custom(format!("Failed to serialize to JSON: {e}")))?;
                Ok(QueryTag(json_string))
            }
            scalar => scalar
                .as_scalar_string()
                .map(|s| QueryTag(s.into_owned()))
                .ok_or_else(|| de::Error::custom("expected a string, a map, or a sequence")),
        }
    }
}

impl From<String> for QueryTag {
    fn from(value: String) -> Self {
        QueryTag(value)
    }
}

impl From<QueryTag> for String {
    fn from(config: QueryTag) -> Self {
        config.0
    }
}

pub fn try_from_value<T: DeserializeOwned>(
    value: Option<YmlValue>,
) -> Result<Option<T>, Box<dyn std::error::Error>> {
    if let Some(value) = value {
        Ok(Some(
            dbt_yaml::from_value(value).map_err(|e| format!("Error parsing value: {e}"))?,
        ))
    } else {
        Ok(None)
    }
}

/// Convert YmlValue to a BTreeMap for minijinja
pub fn yml_value_to_minijinja_map(value: YmlValue) -> BTreeMap<String, MinijinjaValue> {
    match value {
        YmlValue::Mapping(map, _) => map
            .into_iter()
            .filter_map(|(k, v)| {
                k.as_scalar_string()
                    .map(|s| (s.into_owned(), yml_value_to_minijinja(&v)))
            })
            .collect(),
        _ => BTreeMap::new(),
    }
}

pub fn minijinja_value_to_typed_struct<T: DeserializeOwned>(value: MinijinjaValue) -> FsResult<T> {
    let yml_val = dbt_yaml::to_value(value).map_err(|e| {
        FsError::new(
            ErrorCode::SerializationError,
            format!("Failed to convert MinijinjaValue to YmlValue: {e}"),
        )
    })?;

    T::deserialize(yml_val).map_err(|e| yaml_to_fs_error(e, None))
}

pub fn try_string_to_type<T: DeserializeOwned>(
    value: &Option<String>,
) -> Result<Option<T>, Box<dyn std::error::Error>> {
    if let Some(value) = value {
        Ok(Some(dbt_yaml::from_str(&format!("\"{value}\"")).map_err(
            |e| format!("Error parsing from_str '{value}': {e}"),
        )?))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize, PartialEq, Eq, DbtSchema)]
#[serde(untagged)]
pub enum StringOrInteger {
    String(String),
    Integer(i64),
}

impl Default for StringOrInteger {
    fn default() -> Self {
        StringOrInteger::String("".to_string())
    }
}
impl FromStr for StringOrInteger {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            _ if s.parse::<i64>().is_ok() => Ok(StringOrInteger::Integer(s.parse().unwrap())),
            _ => Ok(StringOrInteger::String(s.to_string())),
        }
    }
}

impl std::fmt::Display for StringOrInteger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StringOrInteger::String(s) => write!(f, "{s}"),
            StringOrInteger::Integer(i) => write!(f, "{i}"),
        }
    }
}

impl From<String> for StringOrInteger {
    fn from(value: String) -> Self {
        if let Ok(i) = value.parse::<i64>() {
            StringOrInteger::Integer(i)
        } else {
            StringOrInteger::String(value)
        }
    }
}

impl StringOrInteger {
    pub fn to_i64(&self) -> i64 {
        match self {
            StringOrInteger::String(value) => {
                if let Ok(i) = value.parse::<i64>() {
                    i
                } else {
                    panic!("")
                }
            }
            StringOrInteger::Integer(i) => *i,
        }
    }
}

/// Preserves the original literal type from a Jinja `ref()` call's `version`/`v` kwarg.
/// Matches dbt-core's `NodeVersion = Union[int, float, str]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, DbtSchema)]
#[serde(untagged)]
pub enum NodeVersion {
    Integer(i64),
    Float(f64),
    String(String),
}

impl std::fmt::Display for NodeVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeVersion::Integer(i) => write!(f, "{i}"),
            NodeVersion::Float(v) => write!(f, "{v}"),
            NodeVersion::String(s) => write!(f, "{s}"),
        }
    }
}

impl TryFrom<minijinja::Value> for NodeVersion {
    type Error = minijinja::Error;

    fn try_from(value: minijinja::Value) -> Result<Self, Self::Error> {
        match value.kind() {
            ValueKind::String => Ok(NodeVersion::String(
                value
                    .as_str()
                    .expect("kind is String but as_str returned None")
                    .to_owned(),
            )),
            ValueKind::Number => {
                if value.is_integer() {
                    if let Some(i) = value.as_i64() {
                        return Ok(NodeVersion::Integer(i));
                    }
                }
                if let Ok(f) = f64::try_from(value) {
                    Ok(NodeVersion::Float(f))
                } else {
                    Err(minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        "unsupported numeric type for version",
                    ))
                }
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "version must be a string, integer, or float",
            )),
        }
    }
}

#[derive(Debug, Serialize, UntaggedEnumDeserialize, Clone, PartialEq, Eq, DbtSchema)]
#[serde(untagged)]
pub enum StringOrMap {
    StringValue(String),
    MapValue(HashMap<String, YmlValue>),
}

#[derive(Serialize, UntaggedEnumDeserialize, Debug, Clone, DbtSchema)]
#[serde(untagged)]
pub enum StringOrArrayOfStrings {
    String(String),
    ArrayOfStrings(Vec<String>),
}

impl From<StringOrArrayOfStrings> for Vec<String> {
    fn from(value: StringOrArrayOfStrings) -> Self {
        match value {
            StringOrArrayOfStrings::String(s) => vec![s],
            StringOrArrayOfStrings::ArrayOfStrings(a) => a,
        }
    }
}

/// One or more adapter types, as `propagate: snowflake` or
/// `propagate: [snowflake]`.
///
/// Typed rather than a [`StringOrArrayOfStrings`], so serde rejects a value that
/// is not an adapter type at parse -- the same guarantee
/// `adapter: Option<AdapterType>` already gives for its single-valued sibling.
///
/// The extra typing is about validation, not about the accepted shape, so fields
/// of this type carry `#[schemars(with = "Option<StringOrArrayOfStrings>")]`:
/// `AdapterType` has no `JsonSchema` impl, and the shape a document may actually
/// contain here *is* a string or an array of strings. Describing it as
/// `Option<Vec<String>>` instead would emit an array-only schema and have
/// editors reject the scalar form (`propagate: snowflake`) that this type
/// accepts.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum AdapterTypeOrArray {
    One(AdapterType),
    Many(Vec<AdapterType>),
}

impl<'de> Deserialize<'de> for AdapterTypeOrArray {
    /// Hand-written rather than derived: `UntaggedEnumDeserialize` reports only
    /// "data did not match any variant of untagged enum AdapterTypeOrArray" on a
    /// bad value, while `+adapter`'s error names the offending value. Dispatching
    /// on the shape first, then letting `AdapterType`'s own error surface, keeps
    /// the two diagnostics consistent -- which is the whole reason this type is
    /// typed rather than a `StringOrArrayOfStrings`.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = YmlValue::deserialize(deserializer)?;
        match value {
            YmlValue::Sequence(..) => dbt_yaml::from_value::<Vec<AdapterType>>(value)
                .map(Self::Many)
                .map_err(de::Error::custom),
            scalar => dbt_yaml::from_value::<AdapterType>(scalar)
                .map(Self::One)
                .map_err(de::Error::custom),
        }
    }
}

impl From<AdapterTypeOrArray> for Vec<AdapterType> {
    fn from(value: AdapterTypeOrArray) -> Self {
        match value {
            AdapterTypeOrArray::One(a) => vec![a],
            AdapterTypeOrArray::Many(a) => a,
        }
    }
}

/// Types that can be viewed as `&Option<StringOrArrayOfStrings>`, e.g. the type itself or a
/// newtype wrapping it (like `Tags`/`Classifiers`). A local trait, since `AsRef` can't be
/// implemented for the foreign `Option<StringOrArrayOfStrings>` type (orphan rule).
pub trait AsStringOrArrayOfStrings {
    fn as_string_or_array_of_strings(&self) -> &Option<StringOrArrayOfStrings>;
}

impl AsStringOrArrayOfStrings for Option<StringOrArrayOfStrings> {
    fn as_string_or_array_of_strings(&self) -> &Option<StringOrArrayOfStrings> {
        self
    }
}

pub fn string_or_number_or_array_to_string_array<'de, D>(
    deserializer: D,
) -> Result<Option<StringOrArrayOfStrings>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = dbt_yaml::Value::deserialize(deserializer)?;
    match value {
        dbt_yaml::Value::Sequence(values, _) => values
            .into_iter()
            .map(|value| {
                value
                    .as_scalar_string()
                    .map(|s| s.into_owned())
                    .ok_or_else(|| de::Error::custom("expected a scalar"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|values| Some(StringOrArrayOfStrings::ArrayOfStrings(values))),
        dbt_yaml::Value::Null(_) => Ok(None),
        other => other
            .as_scalar_string()
            .map(|s| Some(StringOrArrayOfStrings::String(s.into_owned())))
            .ok_or_else(|| de::Error::custom("expected a scalar, array, or null")),
    }
}

/// External-table style `partitions` config — a string or list-of-strings (e.g.
/// `['ds=2023-01-01']`), or a map or list-of-maps (e.g. `[{name: foo, data_type: varchar}]`,
/// as used by `dbt-external-tables`).
#[derive(Debug, Serialize, UntaggedEnumDeserialize, Clone, PartialEq, DbtSchema)]
#[serde(untagged)]
pub enum PartitionsConfig {
    String(String),
    Strings(Vec<String>),
    Map(HashMap<String, YmlValue>),
    Maps(Vec<HashMap<String, YmlValue>>),
}

/// DuckDB extension definition — can be a simple string (name) or an object with name and optional repo
#[derive(Serialize, UntaggedEnumDeserialize, Debug, Clone, PartialEq, DbtSchema)]
#[serde(untagged)]
pub enum DuckDbExtension {
    String(String),
    Object(DuckDbExtensionObject),
}

/// DuckDB extension object with name and optional repo metadata
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct DuckDbExtensionObject {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// Wrapper that serializes `StringOrArrayOfStrings` as an array without allocation.
pub(crate) struct AsArray<'a>(pub(crate) &'a StringOrArrayOfStrings);

impl Serialize for AsArray<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeSeq;

        match self.0 {
            StringOrArrayOfStrings::ArrayOfStrings(vec) => vec.serialize(serializer),
            StringOrArrayOfStrings::String(s) => {
                let mut seq = serializer.serialize_seq(Some(1))?;
                seq.serialize_element(s)?;
                seq.end()
            }
        }
    }
}

/// Wrapper type for grant configurations that normalizes values to arrays during serialization.
///
/// This type handles grant configurations with the following behavior:
/// - Normalizes string values to arrays during serialization
/// - Preserves insertion order using IndexMap
#[derive(Debug, Clone, PartialEq, Eq, Default, DbtSchema)]
pub struct GrantConfig(pub IndexMap<String, StringOrArrayOfStrings>);

/// Wrapper around `Omissible<GrantConfig>` with Jinja-compatible serialization.
///
/// Serializes `Omitted` as `{}` instead of `null` for dbt-core compat
/// All other behavior delegates to `Omissible<GrantConfig>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmissibleGrantConfig(pub Omissible<GrantConfig>);

impl Default for OmissibleGrantConfig {
    fn default() -> Self {
        OmissibleGrantConfig(Omissible::Omitted)
    }
}

impl<'de> Deserialize<'de> for OmissibleGrantConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use dbt_common::serde_utils::Omissible;
        Omissible::<GrantConfig>::deserialize(deserializer).map(OmissibleGrantConfig)
    }
}

impl Serialize for OmissibleGrantConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use dbt_common::serde_utils::Omissible;
        match &self.0 {
            Omissible::Present(grant_config) => grant_config.serialize(serializer),
            // Omitted -> {} instead of null (for Jinja compatibility)
            Omissible::Omitted => GrantConfig::default().serialize(serializer),
        }
    }
}

impl OmissibleGrantConfig {
    pub fn is_omitted(&self) -> bool {
        self.0.is_omitted()
    }

    pub fn as_ref(&self) -> Option<&GrantConfig> {
        self.0.as_ref()
    }

    pub fn as_mut(&mut self) -> Option<&mut GrantConfig> {
        self.0.as_mut()
    }
}

impl schemars::JsonSchema for OmissibleGrantConfig {
    fn schema_name() -> String {
        GrantConfig::schema_name()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        GrantConfig::json_schema(generator)
    }

    fn is_referenceable() -> bool {
        GrantConfig::is_referenceable()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        GrantConfig::schema_id()
    }

    #[doc(hidden)]
    fn _schemars_private_non_optional_json_schema(
        generator: &mut schemars::r#gen::SchemaGenerator,
    ) -> schemars::schema::Schema {
        GrantConfig::_schemars_private_non_optional_json_schema(generator)
    }

    #[doc(hidden)]
    fn _schemars_private_is_option() -> bool {
        true
    }
}

impl<'de> Deserialize<'de> for GrantConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = IndexMap::<String, StringOrArrayOfStrings>::deserialize(deserializer)?;
        Ok(GrantConfig(map))
    }
}

impl Serialize for GrantConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;

        // Serialize to {} by default for dbt-core Compat. Used in things like iterators.
        let mut map_ser = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map_ser.serialize_entry(k, &AsArray(v))?;
        }
        map_ser.end()
    }
}

impl GrantConfig {
    /// Get a reference to the value associated with a key
    pub fn get(&self, key: &str) -> Option<&StringOrArrayOfStrings> {
        self.0.get(key)
    }

    /// Check if a key exists in the grants
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl StringOrArrayOfStrings {
    pub fn to_strings(&self) -> Vec<String> {
        match self {
            StringOrArrayOfStrings::String(s) => vec![s.clone()],
            StringOrArrayOfStrings::ArrayOfStrings(a) => a.clone(),
        }
    }

    /// The value as JSON, preserving the authored shape: a string, or an array of strings.
    pub fn to_json_value(&self) -> serde_json::Value {
        match self {
            StringOrArrayOfStrings::String(s) => serde_json::Value::String(s.clone()),
            StringOrArrayOfStrings::ArrayOfStrings(a) => {
                serde_json::Value::Array(a.iter().cloned().map(serde_json::Value::String).collect())
            }
        }
    }

    /// The value as YAML, preserving the authored shape: a string, or a sequence of strings.
    pub fn to_yaml_value(&self) -> dbt_yaml::Value {
        match self {
            StringOrArrayOfStrings::String(s) => dbt_yaml::Value::string(s.clone()),
            StringOrArrayOfStrings::ArrayOfStrings(a) => dbt_yaml::Value::Sequence(
                a.iter().cloned().map(dbt_yaml::Value::string).collect(),
                Default::default(),
            ),
        }
    }

    /// The inverse of [`Self::to_json_value`]. `None` for any other JSON shape, including an
    /// array holding a non-string.
    pub fn from_json_value(value: &serde_json::Value) -> Option<Self> {
        match value {
            serde_json::Value::String(s) => Some(StringOrArrayOfStrings::String(s.clone())),
            serde_json::Value::Array(a) => a
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect::<Option<Vec<String>>>()
                .map(StringOrArrayOfStrings::ArrayOfStrings),
            _ => None,
        }
    }
}

impl PartialEq for StringOrArrayOfStrings {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (StringOrArrayOfStrings::String(s1), StringOrArrayOfStrings::String(s2)) => s1 == s2,
            (
                StringOrArrayOfStrings::ArrayOfStrings(a1),
                StringOrArrayOfStrings::ArrayOfStrings(a2),
            ) => a1 == a2,
            (StringOrArrayOfStrings::String(s), StringOrArrayOfStrings::ArrayOfStrings(a)) => {
                if a.len() == 1 { a[0] == *s } else { false }
            }
            (StringOrArrayOfStrings::ArrayOfStrings(a), StringOrArrayOfStrings::String(s)) => {
                if a.len() == 1 { a[0] == *s } else { false }
            }
        }
    }
}

impl Eq for StringOrArrayOfStrings {}

impl std::hash::Hash for StringOrArrayOfStrings {
    // Consistent with the `PartialEq` impl above, which treats a single-element array as
    // equal to the equivalent scalar string, so both must normalize to the same hash.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.to_strings().hash(state);
    }
}

// =============================================================================
// PrimaryKeyConfig - Wrapper type for primary_key that normalizes to arrays
// =============================================================================
//
// This type implements sadboy's preferred approach:
// - Encodes the serialization/normalization rules in the Rust type itself
// - Accepts both string and array inputs (e.g., "id" or ["id", "tenant_id"])
// - Always serializes values as arrays (matching dbt-core's `listify` behavior)
// - Generates correct JSON schema automatically

/// A wrapper type for the `primary_key` config field that normalizes serialization.
///
/// In dbt-core, primary_key values are "listified" - single strings are converted
/// to single-element arrays. This type accepts either format on input but always
/// serializes as arrays.
///
/// # Example
///
/// ```ignore
/// // Input: "id"
/// // Serializes as: ["id"]
///
/// // Input: ["id", "tenant_id"]
/// // Serializes as: ["id", "tenant_id"]
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, DbtSchema, StringOrArrayNewtype)]
#[string_or_array(none_as_empty_list = false)]
pub struct PrimaryKeyConfig(pub Option<StringOrArrayOfStrings>);

impl PrimaryKeyConfig {
    /// Creates a new empty PrimaryKeyConfig
    pub fn new() -> Self {
        Self(None)
    }

    /// Creates a PrimaryKeyConfig from a StringOrArrayOfStrings
    pub fn from_value(value: StringOrArrayOfStrings) -> Self {
        Self(Some(value))
    }

    /// Returns true if the primary key is empty or unset
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }

    /// Gets the primary key values as a Vec<String>
    pub fn to_strings(&self) -> Option<Vec<String>> {
        self.0.as_ref().map(|v| v.to_strings())
    }
}

// =============================================================================
// IndexesConfig - Wrapper type for indexes that accepts list or dict formats
// =============================================================================
//
// This type implements sadboy's preferred approach:
// - Encodes the deserialization rules in the Rust type itself
// - Accepts both list format `[{...}]` and dictionary format `{'name': {...}}`
// - Always serializes as a list
// - Generates correct JSON schema automatically

/// A ClickHouse data-skipping index: `{name, definition}` items consumed by the
/// ClickHouse macros (`ADD INDEX {{ index.get('name') }} {{ index.get('definition') }}`).
/// Both fields are required, so no `skip_serializing_none` is needed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct ClickHouseIndex {
    name: String,
    definition: String,
}

/// One element of the shared `indexes` config. Adapter shapes are kept as distinct
/// typed variants (like `PartitionConfig` does for BigQuery); the required fields are
/// disjoint (`columns` vs `name`+`definition`), so untagged deserialization is
/// unambiguous, and each variant serializes its own shape back to Jinja.
#[derive(Debug, Clone, PartialEq, UntaggedEnumDeserialize, Serialize, DbtSchema)]
#[serde(untagged)]
pub enum IndexItem {
    /// Postgres: `{columns, unique, type}`
    Postgres(PostgresIndex),
    /// ClickHouse: `{name, definition}`
    ClickHouse(ClickHouseIndex),
}

/// ClickHouse `refreshable` MV config: the settings map or a bare bool, where `false`
/// behaves like omitting the key (dbt-clickhouse parity).
#[derive(Debug, Clone, PartialEq, UntaggedEnumDeserialize, Serialize, DbtSchema)]
#[serde(untagged)]
pub enum RefreshableConfig {
    Bool(bool),
    Config(BTreeMap<String, YmlValue>),
}

/// A wrapper type for the `indexes` config field that handles flexible deserialization.
///
/// dbt-core accepts both list and dictionary formats for indexes. This type accepts
/// either format on input but always serializes as a list.
///
/// # Example
///
/// ```ignore
/// // Input (list): [{columns: ["id"], unique: true}]
/// // Serializes as: [{columns: ["id"], unique: true}]
///
/// // Input (dict): {"my_index": {columns: ["id"], unique: true}}
/// // Serializes as: [{columns: ["id"], unique: true}]  (keys are discarded)
/// ```
#[derive(Debug, Clone, Default, PartialEq, DbtSchema)]
pub struct IndexesConfig(Option<Vec<IndexItem>>);

impl IndexesConfig {
    /// Creates a new empty IndexesConfig
    pub fn new() -> Self {
        Self(None)
    }

    /// Creates an IndexesConfig from a Vec of IndexItem
    pub fn from_vec(indexes: Vec<IndexItem>) -> Self {
        Self(Some(indexes))
    }

    /// Consumes self and returns the inner value
    pub fn into_inner(self) -> Option<Vec<IndexItem>> {
        self.0
    }

    /// Returns true if the indexes are empty or unset
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }

    /// Returns true if the indexes are set
    pub fn is_some(&self) -> bool {
        self.0.is_some()
    }

    /// Returns true if the indexes are empty
    pub fn is_empty(&self) -> bool {
        self.0.as_ref().is_none_or(|v| v.is_empty())
    }

    /// Returns the number of indexes
    pub fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |v| v.len())
    }
}

impl AsRef<Option<Vec<IndexItem>>> for IndexesConfig {
    fn as_ref(&self) -> &Option<Vec<IndexItem>> {
        &self.0
    }
}

impl Serialize for IndexesConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Always serialize as Option<Vec<IndexItem>>
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for IndexesConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{MapAccess, SeqAccess, Visitor};
        use std::fmt;
        use std::marker::PhantomData;

        struct IndexesVisitor(PhantomData<IndexItem>);

        impl<'de> Visitor<'de> for IndexesVisitor {
            type Value = Option<Vec<IndexItem>>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str(
                    "a sequence of index configs, a map of name -> index config, or null",
                )
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(None)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(None)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                while let Some(elem) = seq.next_element()? {
                    vec.push(elem);
                }
                Ok(Some(vec))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut vec = Vec::new();
                // Discard the keys, collect just the values
                while let Some((_key, value)) = map.next_entry::<String, IndexItem>()? {
                    vec.push(value);
                }
                Ok(Some(vec))
            }
        }

        deserializer
            .deserialize_any(IndexesVisitor(PhantomData))
            .map(IndexesConfig)
    }
}

impl From<Option<Vec<IndexItem>>> for IndexesConfig {
    fn from(value: Option<Vec<IndexItem>>) -> Self {
        IndexesConfig(value)
    }
}

impl From<IndexesConfig> for Option<Vec<IndexItem>> {
    fn from(config: IndexesConfig) -> Self {
        config.0
    }
}

#[derive(UntaggedEnumDeserialize, Serialize, Debug, Clone, DbtSchema)]
#[serde(untagged)]
pub enum SpannedStringOrArrayOfStrings {
    String(Spanned<String>),
    ArrayOfStrings(Vec<Spanned<String>>),
}

impl From<SpannedStringOrArrayOfStrings> for Vec<Spanned<String>> {
    fn from(value: SpannedStringOrArrayOfStrings) -> Self {
        match value {
            SpannedStringOrArrayOfStrings::String(s) => vec![s],
            SpannedStringOrArrayOfStrings::ArrayOfStrings(a) => a,
        }
    }
}

impl SpannedStringOrArrayOfStrings {
    pub fn to_strings(&self) -> Vec<Spanned<String>> {
        match self {
            SpannedStringOrArrayOfStrings::String(s) => vec![s.clone()],
            SpannedStringOrArrayOfStrings::ArrayOfStrings(a) => a.clone(),
        }
    }
}

impl PartialEq for SpannedStringOrArrayOfStrings {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                SpannedStringOrArrayOfStrings::String(s1),
                SpannedStringOrArrayOfStrings::String(s2),
            ) => s1 == s2,
            (
                SpannedStringOrArrayOfStrings::ArrayOfStrings(a1),
                SpannedStringOrArrayOfStrings::ArrayOfStrings(a2),
            ) => a1 == a2,
            (
                SpannedStringOrArrayOfStrings::String(s),
                SpannedStringOrArrayOfStrings::ArrayOfStrings(a),
            ) => {
                if a.len() == 1 {
                    a[0] == *s
                } else {
                    false
                }
            }
            (
                SpannedStringOrArrayOfStrings::ArrayOfStrings(a),
                SpannedStringOrArrayOfStrings::String(s),
            ) => {
                if a.len() == 1 {
                    a[0] == *s
                } else {
                    false
                }
            }
        }
    }
}

impl Eq for SpannedStringOrArrayOfStrings {}

#[derive(UntaggedEnumDeserialize, Serialize, Debug, Clone, DbtSchema)]
#[serde(untagged)]
pub enum FloatOrString {
    Number(f32),
    String(String),
}

impl std::fmt::Display for FloatOrString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FloatOrString::Number(n) => write!(f, "{n}"),
            FloatOrString::String(s) => write!(f, "{s}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::common::PartitionConfig;
    use dbt_common::serde_utils::Omissible;

    #[derive(Serialize, Deserialize)]
    struct TestConfig {
        #[serde(default)]
        grants: OmissibleGrantConfig,
    }

    /// `propagate` accepts a bare adapter name or a list of them, and both
    /// collapse to the same `Vec<AdapterType>` for callers.
    #[test]
    fn adapter_type_or_array_accepts_one_or_many() {
        let one: AdapterTypeOrArray = dbt_yaml::from_str("snowflake").unwrap();
        assert_eq!(one, AdapterTypeOrArray::One(AdapterType::Snowflake));
        assert_eq!(Vec::<AdapterType>::from(one), vec![AdapterType::Snowflake]);

        let many: AdapterTypeOrArray = dbt_yaml::from_str("[snowflake, bigquery]").unwrap();
        assert_eq!(
            Vec::<AdapterType>::from(many),
            vec![AdapterType::Snowflake, AdapterType::Bigquery]
        );
    }

    /// The point of typing this rather than reusing `StringOrArrayOfStrings`:
    /// a value that is not an adapter type fails here, at deserialization.
    #[test]
    fn adapter_type_or_array_rejects_a_value_that_is_not_an_adapter() {
        let err = dbt_yaml::from_str::<AdapterTypeOrArray>("compute")
            .expect_err("`compute` is not an adapter type");
        assert!(
            format!("{err}").contains("compute"),
            "error should name the offending value: {err}"
        );
        dbt_yaml::from_str::<AdapterTypeOrArray>("[snowflake, compute]")
            .expect_err("one bad entry rejects the whole list");
    }

    #[test]
    fn partition_by_round_trips_string_and_list() {
        let scalar: PartitionConfig = dbt_yaml::from_str("toYYYYMM(created_at)").unwrap();
        assert_eq!(
            dbt_yaml::to_string(&scalar).unwrap().trim(),
            "toYYYYMM(created_at)"
        );

        let list: PartitionConfig = dbt_yaml::from_str("[a, b]").unwrap();
        let round_tripped: Vec<String> =
            dbt_yaml::from_str(&dbt_yaml::to_string(&list).unwrap()).unwrap();
        assert_eq!(round_tripped, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn indexes_round_trip_clickhouse_and_postgres_shapes() {
        // ClickHouse {name, definition}
        let ch: IndexesConfig =
            dbt_yaml::from_str("[{name: idx1, definition: 'a TYPE minmax GRANULARITY 4'}]")
                .unwrap();
        assert!(matches!(
            ch.as_ref().as_deref(),
            Some([IndexItem::ClickHouse(_)])
        ));
        // Round-trip: re-parse the serialized YAML and compare to the original items
        // (robust against emitter quoting/reflow choices).
        let ch_reparsed: Vec<IndexItem> =
            dbt_yaml::from_str(&dbt_yaml::to_string(&ch).unwrap()).unwrap();
        assert_eq!(Some(ch_reparsed), ch.into_inner());

        // Postgres {columns, unique} — list form
        let pg: IndexesConfig = dbt_yaml::from_str("[{columns: [id], unique: true}]").unwrap();
        assert!(matches!(
            pg.as_ref().as_deref(),
            Some([IndexItem::Postgres(_)])
        ));
        let pg_reparsed: Vec<IndexItem> =
            dbt_yaml::from_str(&dbt_yaml::to_string(&pg).unwrap()).unwrap();
        assert_eq!(Some(pg_reparsed), pg.into_inner());

        // Postgres dict-of-indexes form still works, and serializes back as a list
        let pg_map: IndexesConfig =
            dbt_yaml::from_str("{my_index: {columns: [id], unique: true}}").unwrap();
        assert_eq!(pg_map.len(), 1);
        let pg_map_reparsed: Vec<IndexItem> =
            dbt_yaml::from_str(&dbt_yaml::to_string(&pg_map).unwrap()).unwrap();
        assert_eq!(Some(pg_map_reparsed), pg_map.into_inner());
    }

    #[test]
    fn test_grant_config_normalizes_string_to_array() {
        let mut grants: IndexMap<String, StringOrArrayOfStrings> = IndexMap::new();
        grants.insert(
            "select".to_string(),
            StringOrArrayOfStrings::String("ROLE_A".to_string()),
        );

        let config = TestConfig {
            grants: OmissibleGrantConfig(Omissible::Present(GrantConfig(grants))),
        };
        let json = serde_json::to_string(&config).unwrap();

        // String should be normalized to array
        assert_eq!(json, r#"{"grants":{"select":["ROLE_A"]}}"#);
    }

    #[test]
    fn test_grant_config_preserves_array() {
        let mut grants = IndexMap::new();
        grants.insert(
            "select".to_string(),
            StringOrArrayOfStrings::ArrayOfStrings(vec![
                "ROLE_A".to_string(),
                "ROLE_B".to_string(),
            ]),
        );

        let config = TestConfig {
            grants: OmissibleGrantConfig(Omissible::Present(GrantConfig(grants))),
        };
        let json = serde_json::to_string(&config).unwrap();

        assert_eq!(json, r#"{"grants":{"select":["ROLE_A","ROLE_B"]}}"#);
    }

    #[test]
    fn test_grant_config_omitted_serializes_as_empty_map() {
        let config = TestConfig {
            grants: OmissibleGrantConfig(Omissible::Omitted),
        };
        let json = serde_json::to_string(&config).unwrap();

        // Omitted should serialize as empty map for Jinja compatibility
        assert_eq!(json, r#"{"grants":{}}"#);
    }

    #[test]
    fn test_grant_config_empty_map() {
        let config = TestConfig {
            grants: OmissibleGrantConfig(Omissible::Present(GrantConfig(IndexMap::new()))),
        };
        let json = serde_json::to_string(&config).unwrap();

        assert_eq!(json, r#"{"grants":{}}"#);
    }

    #[test]
    fn test_partitions_config_accepts_string_list() {
        let yaml = "- ds=2023-01-01\n- ds=2023-01-02\n";
        let parsed: PartitionsConfig = dbt_yaml::from_str(yaml).unwrap();
        match parsed {
            PartitionsConfig::Strings(v) => {
                assert_eq!(
                    v,
                    vec!["ds=2023-01-01".to_string(), "ds=2023-01-02".to_string()]
                );
            }
            PartitionsConfig::Maps(_) | PartitionsConfig::String(_) | PartitionsConfig::Map(_) => {
                panic!("expected Strings variant")
            }
        }
    }

    #[test]
    fn test_partitions_config_accepts_string() {
        let yaml = "ds=2023-01-01\n";
        let parsed: PartitionsConfig = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(
            parsed,
            PartitionsConfig::String("ds=2023-01-01".to_string())
        );
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            r#""ds=2023-01-01""#
        );
    }

    #[test]
    fn test_partitions_config_accepts_map() {
        let yaml = "name: extracted_at_ts\ndata_type: timestamptz\n";
        let parsed: PartitionsConfig = dbt_yaml::from_str(yaml).unwrap();
        match &parsed {
            PartitionsConfig::Map(map) => {
                assert_eq!(
                    map.get("name").and_then(|value| value.as_str()),
                    Some("extracted_at_ts")
                );
                assert_eq!(
                    map.get("data_type").and_then(|value| value.as_str()),
                    Some("timestamptz")
                );
            }
            _ => panic!("expected Map variant"),
        }
        let json = serde_json::to_string(&parsed).unwrap();
        assert!(json.contains(r#""name":"extracted_at_ts""#));
        assert!(json.contains(r#""data_type":"timestamptz""#));
    }

    #[test]
    fn test_partitions_config_accepts_map_list() {
        // dbt-external-tables form: list of maps with name/data_type/expression
        let yaml = "- name: extracted_at_ts\n  data_type: timestamptz\n  expression: cast(_etl_ts as timestamptz)\n";
        let parsed: PartitionsConfig = dbt_yaml::from_str(yaml).unwrap();
        match parsed {
            PartitionsConfig::Maps(v) => {
                assert_eq!(v.len(), 1);
                assert_eq!(
                    v[0].get("name").and_then(|y| y.as_str()),
                    Some("extracted_at_ts"),
                );
                assert_eq!(
                    v[0].get("data_type").and_then(|y| y.as_str()),
                    Some("timestamptz"),
                );
            }
            PartitionsConfig::Strings(_)
            | PartitionsConfig::String(_)
            | PartitionsConfig::Map(_) => panic!("expected Maps variant"),
        }
    }

    #[test]
    fn test_partitions_config_roundtrips_map_form() {
        let yaml = "- name: foo\n  data_type: varchar\n";
        let parsed: PartitionsConfig = dbt_yaml::from_str(yaml).unwrap();
        // Serialization must preserve the map shape (untagged enum).
        let json = serde_json::to_string(&parsed).unwrap();
        assert!(
            json.contains(r#""name":"foo""#),
            "expected map form in json, got {json}",
        );
        assert!(
            json.contains(r#""data_type":"varchar""#),
            "expected map form in json, got {json}",
        );
    }

    #[test]
    fn test_node_version_serialization_preserves_type() {
        assert_eq!(
            serde_json::to_string(&NodeVersion::Integer(2)).unwrap(),
            "2"
        );
        assert_eq!(
            serde_json::to_string(&NodeVersion::Float(1.5)).unwrap(),
            "1.5"
        );
        assert_eq!(
            serde_json::to_string(&NodeVersion::String("2".to_string())).unwrap(),
            r#""2""#
        );
    }

    #[test]
    fn test_node_version_try_from_integral_float_stays_float() {
        use minijinja::Value;
        let val = Value::from(2.0f64);
        assert_eq!(
            NodeVersion::try_from(val).unwrap(),
            NodeVersion::Float(2.0),
            "integral float 2.0 must stay Float, not coerce to Integer",
        );
    }

    #[test]
    fn test_node_version_try_from_minijinja_value() {
        use minijinja::Value;

        let int_val = Value::from(2i64);
        assert_eq!(
            NodeVersion::try_from(int_val).unwrap(),
            NodeVersion::Integer(2)
        );

        let float_val = Value::from(1.5f64);
        assert_eq!(
            NodeVersion::try_from(float_val).unwrap(),
            NodeVersion::Float(1.5)
        );

        let str_val = Value::from("2");
        assert_eq!(
            NodeVersion::try_from(str_val).unwrap(),
            NodeVersion::String("2".to_string())
        );
    }

    // dbt-core#15473: an omitted key must stay `Omitted` (inherit) while an
    // explicit `null` becomes `Present(None)` (clear); the "null" string (#11681)
    // stays a value.
    #[test]
    fn hours_to_expiration_or_string_omissible_distinguishes_null_from_omitted() {
        use dbt_common::serde_utils::Omissible;

        #[derive(Deserialize)]
        struct W {
            #[serde(default, deserialize_with = "hours_to_expiration_or_string_omissible")]
            h: Omissible<Option<StringOrInteger>>,
        }

        assert_eq!(
            serde_json::from_str::<W>(r#"{}"#).unwrap().h,
            Omissible::Omitted
        );
        assert_eq!(
            serde_json::from_str::<W>(r#"{"h":null}"#).unwrap().h,
            Omissible::Present(None)
        );
        assert_eq!(
            serde_json::from_str::<W>(r#"{"h":12}"#).unwrap().h,
            Omissible::Present(Some(StringOrInteger::Integer(12)))
        );
        assert_eq!(
            serde_json::from_str::<W>(r#"{"h":"12"}"#).unwrap().h,
            Omissible::Present(Some(StringOrInteger::Integer(12)))
        );
        assert_eq!(
            serde_json::from_str::<W>(r#"{"h":"null"}"#).unwrap().h,
            Omissible::Present(Some(StringOrInteger::String("null".to_string())))
        );
    }

    #[test]
    fn strip_one_trailing_newline_at_keys_matches_core_get_rendered_native() {
        // Verified against dbt-core's `get_rendered(..., native=True)`, which never takes the
        // no-jinja fast path and so always runs the Jinja lexer -- dropping exactly one trailing
        // newline and nothing else. `untouched` stands in for a key not on the allowlist.
        let mut value: YmlValue = dbt_yaml::from_str(
            "one: \"declared_source\\n\"\ntwo: \"declared_source\\n\\n\"\npadded: \"  declared_source  \\n\"\nplain: declared_source\nuntouched: \"declared_source\\n\"\n",
        )
        .unwrap();

        strip_one_trailing_newline_at_keys(&mut value, &["one", "two", "padded", "plain"]);

        assert_eq!(value.get("one").unwrap().as_str(), Some("declared_source"));
        assert_eq!(
            value.get("two").unwrap().as_str(),
            Some("declared_source\n")
        );
        assert_eq!(
            value.get("padded").unwrap().as_str(),
            Some("  declared_source  ")
        );
        assert_eq!(
            value.get("plain").unwrap().as_str(),
            Some("declared_source")
        );
        assert_eq!(
            value.get("untouched").unwrap().as_str(),
            Some("declared_source\n")
        );
    }
}
