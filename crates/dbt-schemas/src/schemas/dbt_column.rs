use indexmap::IndexMap;
use std::sync::Arc;

use dbt_common::FsResult;
use dbt_yaml::{DbtSchema, UntaggedEnumDeserialize};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::skip_serializing_none;
use strum::Display;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use crate::schemas::{
    common::DimensionValidityParams,
    semantic_layer::semantic_manifest::SemanticLayerElementConfig,
    serde::{StringOrArrayOfStrings, StringOrMap, policy_tags_from_scalar_or_list},
};

use super::{common::Constraint, data_tests::DataTests};

/// The BaseColumn as implemented by dbt Core.
///
/// This is used to deserialize columns from Jinja that produces them, for example
/// the public API macros for `get_columns_in_relation()`
#[derive(Deserialize, Debug)]
pub struct DbtCoreBaseColumn {
    pub name: String,
    pub dtype: String,
    pub char_size: Option<u32>,
    pub numeric_precision: Option<u64>,
    pub numeric_scale: Option<u64>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Default, Clone)]
#[serde(rename_all = "snake_case")]
pub struct DbtColumn {
    pub name: String,
    pub data_type: Option<String>,
    #[serialize_always]
    #[serde(serialize_with = "serialize_dbt_column_desc")]
    pub description: Option<String>,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    #[serde(default)]
    pub meta: IndexMap<String, YmlValue>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub policy_tags: Option<Vec<StringOrMap>>,
    pub classifiers: Option<Vec<String>>,
    pub databricks_tags: Option<IndexMap<String, YmlValue>>,
    pub column_mask: Option<ColumnMask>,
    pub quote: Option<bool>,
    pub codec: Option<String>,
    pub ttl: Option<String>,
    #[serde(default, rename = "config")]
    pub deprecated_config: ColumnConfig,
    pub dimension: Option<ColumnPropertiesDimension>,
    pub entity: Option<Entity>,
    pub granularity: Option<Granularity>,
}

fn serialize_dbt_column_desc<S>(description: &Option<String>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(description.as_deref().unwrap_or(""))
}

pub type DbtColumnRef = Arc<DbtColumn>;

/// Serialize and deserialize as a map to maintain Jinja behavior
pub fn serialize_dbt_columns<S>(columns: &Vec<DbtColumnRef>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut map = s.serialize_map(Some(columns.len()))?;
    for col in columns {
        map.serialize_entry(&col.name.clone(), col)?;
    }
    map.end()
}

pub fn deserialize_dbt_columns<'de, D>(deserializer: D) -> Result<Vec<DbtColumnRef>, D::Error>
where
    D: Deserializer<'de>,
{
    struct DbtColumnVisitor;

    impl<'de> Visitor<'de> for DbtColumnVisitor {
        type Value = Vec<DbtColumnRef>;

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut columns = Vec::new();
            while let Some((_key, value)) =
                map.next_entry::<serde::de::IgnoredAny, DbtColumnRef>()?
            {
                columns.push(value)
            }
            Ok(columns)
        }

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "a map of column names to columns")
        }
    }

    deserializer.deserialize_map(DbtColumnVisitor)
}

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ColumnProperties {
    pub name: String,
    pub data_type: Option<String>,
    pub description: Option<String>,
    pub constraints: Option<Vec<Constraint>>,
    pub tests: Option<Vec<DataTests>>,
    pub data_tests: Option<Vec<DataTests>>,
    pub granularity: Option<Granularity>,
    #[serde(default, deserialize_with = "policy_tags_from_scalar_or_list")]
    pub policy_tags: Option<Vec<StringOrMap>>,
    pub classifiers: Option<Vec<String>>,
    pub databricks_tags: Option<IndexMap<String, YmlValue>>,
    pub column_mask: Option<ColumnMask>,
    pub quote: Option<bool>,
    pub codec: Option<String>,
    pub ttl: Option<String>,
    pub config: Option<ColumnConfig>,

    pub entity: Option<Entity>,
    pub dimension: Option<ColumnPropertiesDimension>,
}

/// Column entry inside a model version block.
///
/// Unlike `ColumnProperties`, `name` is optional here because version column lists can contain
/// include/exclude directives (e.g. `include: all, exclude: [col]`) that have no name.
#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct VersionColumnProperties {
    pub name: Option<String>,
    pub include: Option<StringOrArrayOfStrings>,
    pub exclude: Option<Vec<String>>,
    pub data_type: Option<String>,
    pub description: Option<String>,
    pub constraints: Option<Vec<Constraint>>,
    pub tests: Option<Vec<DataTests>>,
    pub data_tests: Option<Vec<DataTests>>,
    pub granularity: Option<Granularity>,
    #[serde(default, deserialize_with = "policy_tags_from_scalar_or_list")]
    pub policy_tags: Option<Vec<StringOrMap>>,
    pub classifiers: Option<Vec<String>>,
    pub databricks_tags: Option<IndexMap<String, YmlValue>>,
    pub column_mask: Option<ColumnMask>,
    pub quote: Option<bool>,
    pub codec: Option<String>,
    pub ttl: Option<String>,
    pub config: Option<ColumnConfig>,
    pub entity: Option<Entity>,
    pub dimension: Option<ColumnPropertiesDimension>,
}

impl VersionColumnProperties {
    /// A version column entry is a real column definition iff it carries a `name`. Entries without
    /// one are the `include`/`exclude` inheritance directive, which is consumed by
    /// [`ColumnInheritanceRules::from_version_column_props`] instead.
    ///
    /// Mirrors dbt-core's `UnparsedVersion.__post_init__` split of `columns` into
    /// `_include_exclude` and `_unparsed_columns` (`dbt/contracts/graph/unparsed.py`).
    pub fn to_column_properties(&self) -> Option<ColumnProperties> {
        Some(ColumnProperties {
            name: self.name.clone()?,
            data_type: self.data_type.clone(),
            description: self.description.clone(),
            constraints: self.constraints.clone(),
            tests: self.tests.clone(),
            data_tests: self.data_tests.clone(),
            granularity: self.granularity.clone(),
            policy_tags: self.policy_tags.clone(),
            classifiers: self.classifiers.clone(),
            databricks_tags: self.databricks_tags.clone(),
            column_mask: self.column_mask.clone(),
            quote: self.quote,
            codec: self.codec.clone(),
            ttl: self.ttl.clone(),
            config: self.config.clone(),
            entity: self.entity.clone(),
            dimension: self.dimension.clone(),
        })
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, DbtSchema, Eq, PartialEq)]
pub struct ColumnMask {
    pub function: String,
    pub using_columns: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default, DbtSchema, Eq, PartialEq, Display)]
#[allow(non_camel_case_types)]
pub enum Granularity {
    #[default]
    nanosecond,
    microsecond,
    millisecond,
    second,
    minute,
    hour,
    day,
    week,
    month,
    quarter,
    year,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, Default, PartialEq, Eq)]
pub struct ColumnConfig {
    #[serde(default)]
    pub tags: Option<StringOrArrayOfStrings>,
    pub meta: Option<IndexMap<String, YmlValue>>,
    pub databricks_tags: Option<IndexMap<String, YmlValue>>,
    #[serde(default, deserialize_with = "policy_tags_from_scalar_or_list")]
    pub policy_tags: Option<Vec<StringOrMap>>,
}

/// Represents column inheritance rules for a model version
#[derive(Debug, Clone)]
pub struct ColumnInheritanceRules {
    includes: Vec<String>, // Empty vec means include all
    excludes: Vec<String>,
}

/// dbt-core's default when a version supplies no `include`/`exclude` directive is
/// `IncludeExclude(include="*")` — inherit every model-level column (see
/// `UnparsedVersion.__post_init__` in `dbt/contracts/graph/unparsed.py`).
///
/// Making that dbt-core default the `Default` impl lets callers write
/// `from_version_column_props(..).unwrap_or_default()` instead of hand-rolling the `None` arm,
/// which is how dbt-labs/fs#13334 (every inherited column silently dropped) happened.
impl Default for ColumnInheritanceRules {
    fn default() -> Self {
        Self {
            includes: Vec::new(), // empty == include all
            excludes: Vec::new(),
        }
    }
}

impl ColumnInheritanceRules {
    /// Given the `columns:` list of a versioned model, return the include/exclude directive it
    /// carries, or `None` when it carries none (in which case callers must use [`Default`], i.e.
    /// inherit all).
    ///
    /// dbt-core reads the *first* directive entry and errors on a second one; Fusion takes the
    /// first and ignores the rest.
    pub fn from_version_column_props(columns: &[VersionColumnProperties]) -> Option<Self> {
        let directive = columns
            .iter()
            .find(|col| col.include.is_some() || col.exclude.is_some())?;

        let includes = match directive.include.as_ref() {
            // `all` / `*` means include everything, represented as an empty `includes`.
            Some(StringOrArrayOfStrings::String(s)) if s == "*" || s == "all" => Vec::new(),
            Some(StringOrArrayOfStrings::String(s)) => vec![s.clone()],
            Some(StringOrArrayOfStrings::ArrayOfStrings(names)) => names.clone(),
            // `exclude` without `include` behaves as include-all-except.
            None => Vec::new(),
        };
        let excludes = directive.exclude.clone().unwrap_or_default();

        Some(ColumnInheritanceRules { includes, excludes })
    }

    /// given a column name, return true if it should be included in the tests based on the includes and excludes and inheritance rules
    pub fn should_include_column(&self, column_name: &str) -> bool {
        if self.includes.is_empty() {
            // Empty includes means include all except excluded
            !self.excludes.contains(&column_name.to_string())
        } else {
            // Specific includes: must be in includes and not in excludes
            self.includes.contains(&column_name.to_string())
                && !self.excludes.contains(&column_name.to_string())
        }
    }
}

/// Hydrate a column's `dimension` so its writable-manifest shape matches what
/// dbt-core expects for `ColumnDimension`. The dict form requires `name`
/// (dbt-core has no default), so when YAML omits it we fall back to the
/// column name — same behaviour as dbt-core's `ParserRef._add`. The bare-string
/// form passes through unchanged.
fn normalize_dimension(
    dimension: Option<ColumnPropertiesDimension>,
    column_name: &str,
    column_description: Option<&str>,
) -> Option<ColumnPropertiesDimension> {
    match dimension? {
        d @ ColumnPropertiesDimension::DimensionType(_) => Some(d),
        ColumnPropertiesDimension::DimensionConfig(mut config) => {
            if config.name.is_none() {
                config.name = Some(column_name.to_string());
            }
            if config.description.is_none() {
                config.description = column_description.map(str::to_string);
            }
            Some(ColumnPropertiesDimension::DimensionConfig(config))
        }
    }
}

/// Same shape constraint as `normalize_dimension` but for `entity`: dbt-core's
/// `ColumnEntity` requires `name: str`.
fn normalize_entity(
    entity: Option<Entity>,
    column_name: &str,
    column_description: Option<&str>,
) -> Option<Entity> {
    match entity? {
        e @ Entity::EntityType(_) => Some(e),
        Entity::EntityConfig(mut config) => {
            if config.name.is_none() {
                config.name = Some(column_name.to_string());
            }
            if config.description.is_none() {
                config.description = column_description.map(str::to_string);
            }
            Some(Entity::EntityConfig(config))
        }
    }
}

/// Process column metadata locally and inherit resource tags when column tags are unset.
/// Returns a Vec of DbtColumn references.
pub fn process_columns(
    columns: Option<&Vec<ColumnProperties>>,
    tags: Option<Vec<String>>,
) -> FsResult<Vec<DbtColumnRef>> {
    Ok(columns
        .map(|cols| {
            // Deduplicate by column name, keeping the last definition for each name.
            // This matches dbt-core/Mantle behaviour where columns are stored in a dict
            // and a later definition silently overwrites an earlier one.
            let mut by_name: IndexMap<String, DbtColumnRef> = IndexMap::new();
            for cp in cols.iter() {
                let (cp_meta, cp_tags, cp_databricks_tags, cp_policy_tags) = cp
                    .config
                    .clone()
                    .map(|c| (c.meta, c.tags, c.databricks_tags, c.policy_tags))
                    .unwrap_or_default();
                let column_meta = cp_meta.unwrap_or_default();

                let col = Arc::new(DbtColumn {
                    name: cp.name.clone(),
                    data_type: cp.data_type.clone(),
                    description: cp.description.clone(),
                    constraints: cp.constraints.clone().unwrap_or_default(),
                    meta: column_meta,
                    tags: cp_tags
                        .map(|t| t.into())
                        .or_else(|| tags.clone())
                        .unwrap_or_default(),
                    // Top-level policy_tags takes precedence over config.policy_tags
                    policy_tags: cp.policy_tags.clone().or(cp_policy_tags),
                    classifiers: cp.classifiers.clone(),
                    databricks_tags: cp.databricks_tags.clone().or(cp_databricks_tags),
                    column_mask: cp.column_mask.clone(),
                    quote: cp.quote,
                    codec: cp.codec.clone(),
                    ttl: cp.ttl.clone(),
                    deprecated_config: cp.config.clone().unwrap_or_default(),
                    dimension: normalize_dimension(
                        cp.dimension.clone(),
                        &cp.name,
                        cp.description.as_deref(),
                    ),
                    entity: normalize_entity(
                        cp.entity.clone(),
                        &cp.name,
                        cp.description.as_deref(),
                    ),
                    granularity: cp.granularity.clone(),
                });
                by_name.insert(cp.name.clone(), col);
            }
            Ok::<Vec<DbtColumnRef>, Box<dyn std::error::Error>>(by_name.into_values().collect())
        })
        .transpose()?
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_col(name: &str, description: &str) -> ColumnProperties {
        ColumnProperties {
            name: name.to_string(),
            description: Some(description.to_string()),
            data_type: None,
            constraints: None,
            tests: None,
            data_tests: None,
            granularity: None,
            policy_tags: None,
            classifiers: None,
            databricks_tags: None,
            column_mask: None,
            quote: None,
            codec: None,
            ttl: None,
            config: None,
            entity: None,
            dimension: None,
        }
    }

    /// Regression: when the same column name appears multiple times in a YAML schema,
    /// process_columns must deduplicate by name keeping the last definition (matching
    /// dbt-core/Mantle dict semantics). Previously Fusion kept all occurrences, producing
    /// a Vec with duplicate names that caused false state:modified detections.
    #[test]
    fn test_process_columns_deduplicates_by_name_last_wins() {
        let cols = vec![
            make_col("id", "First definition."),
            make_col("name", "The name."),
            make_col("id", "Second definition (last wins)."),
        ];

        let result = process_columns(Some(&cols), None).unwrap();

        assert_eq!(result.len(), 2, "duplicate 'id' should be collapsed to one");

        let id_col = result.iter().find(|c| c.name == "id").unwrap();
        assert_eq!(
            id_col.description.as_deref(),
            Some("Second definition (last wins)."),
            "last definition should win"
        );
    }

    /// Regression: `dimension: { type: time }` in YAML must produce a manifest
    /// payload that dbt-core's `ColumnDimension` (which requires non-null `name`)
    /// can deserialize. Previously fusion emitted `name: null`, which crashed
    /// `parse_with_fusion` with `mashumaro.InvalidFieldValue`.
    #[test]
    fn test_process_columns_hydrates_dimension_config_name() {
        let mut col = make_col("date_day", "Day grain.");
        col.dimension = Some(ColumnPropertiesDimension::DimensionConfig(
            ColumnPropertiesDimensionConfig {
                type_: ColumnPropertiesDimensionType::time,
                is_partition: None,
                label: None,
                name: None,
                description: None,
                config: None,
                validity_params: None,
            },
        ));

        let result = process_columns(Some(&vec![col]), None).unwrap();
        let dimension = result[0].dimension.as_ref().expect("dimension preserved");
        match dimension {
            ColumnPropertiesDimension::DimensionConfig(c) => {
                assert_eq!(c.name.as_deref(), Some("date_day"));
                assert_eq!(c.description.as_deref(), Some("Day grain."));
            }
            other => panic!("expected DimensionConfig, got {other:?}"),
        }
    }

    /// ClickHouse `codec:`/`ttl:` on a schema.yml column must survive into
    /// `DbtColumn` — contract DDL rendering and schema_changes read them there.
    #[test]
    fn test_process_columns_preserves_codec_and_ttl() {
        let mut col = make_col("col_3", "Compressed column.");
        col.codec = Some("ZSTD".to_string());
        col.ttl = Some("created_at + INTERVAL 1 DAY".to_string());

        let result = process_columns(Some(&vec![col]), None).unwrap();
        assert_eq!(result[0].codec.as_deref(), Some("ZSTD"));
        assert_eq!(
            result[0].ttl.as_deref(),
            Some("created_at + INTERVAL 1 DAY")
        );
    }

    /// Bare-string `dimension: time` must pass through untouched — dbt-core
    /// accepts it via the `DimensionType` arm of its Union.
    #[test]
    fn test_process_columns_preserves_bare_dimension_type() {
        let mut col = make_col("ts", "");
        col.dimension = Some(ColumnPropertiesDimension::DimensionType(
            ColumnPropertiesDimensionType::time,
        ));

        let result = process_columns(Some(&vec![col]), None).unwrap();
        assert!(matches!(
            result[0].dimension,
            Some(ColumnPropertiesDimension::DimensionType(
                ColumnPropertiesDimensionType::time
            ))
        ));
    }

    /// Regression for fs#13281: mapping-valued `policy_tags` entries must deserialize, not be rejected.
    #[test]
    fn test_column_properties_policy_tags_accepts_mixed_string_and_map_entries() {
        let yaml = "\
name: ssn
policy_tags:
  - projects/my-project/locations/us/taxonomies/1/policyTags/2
  - masking_policy: mask_ssn
    using_columns: [ssn]
";
        let parsed: ColumnProperties = dbt_yaml::from_str(yaml).unwrap();
        let tags = parsed.policy_tags.expect("policy_tags preserved");
        assert_eq!(tags.len(), 2);
        match &tags[0] {
            StringOrMap::StringValue(s) => {
                assert_eq!(
                    s,
                    "projects/my-project/locations/us/taxonomies/1/policyTags/2"
                );
            }
            StringOrMap::MapValue(_) => panic!("expected StringValue for first entry"),
        }
        match &tags[1] {
            StringOrMap::MapValue(m) => {
                assert_eq!(
                    m.get("masking_policy").and_then(|v| v.as_str()),
                    Some("mask_ssn")
                );
            }
            StringOrMap::StringValue(_) => panic!("expected MapValue for second entry"),
        }
    }

    #[test]
    fn test_process_columns_keeps_config_meta_local() {
        let yaml = r#"
name: id
config:
  meta:
    constraint: local
    local_only: retained
"#;
        let column: ColumnProperties = dbt_yaml::from_str(yaml).unwrap();
        let unconfigured = make_col("name", "No column metadata.");

        let result = process_columns(Some(&vec![column, unconfigured]), None).unwrap();
        let meta = &result[0].meta;
        assert_eq!(
            meta.get("constraint").and_then(|value| value.as_str()),
            Some("local")
        );
        assert_eq!(
            meta.get("local_only").and_then(|value| value.as_str()),
            Some("retained")
        );
        assert_eq!(
            result[0].deprecated_config.meta.as_ref(),
            Some(meta),
            "column.meta must expose its own config.meta"
        );
        assert!(
            result[1].meta.is_empty(),
            "a column without local metadata must not inherit model metadata"
        );
        assert!(result[1].deprecated_config.meta.is_none());
    }

    /// Regression for fs#13343: a scalar (non-list) `policy_tags` value must deserialize
    /// as a single-element list, not be rejected.
    #[test]
    fn test_column_properties_policy_tags_accepts_scalar_string() {
        let yaml = "\
name: id
policy_tags: governance.tags.PII
";
        let parsed: ColumnProperties = dbt_yaml::from_str(yaml).unwrap();
        let tags = parsed.policy_tags.expect("policy_tags preserved");
        assert_eq!(tags.len(), 1);
        match &tags[0] {
            StringOrMap::StringValue(s) => assert_eq!(s, "governance.tags.PII"),
            StringOrMap::MapValue(_) => panic!("expected StringValue"),
        }
    }
}

#[derive(UntaggedEnumDeserialize, Serialize, Debug, Clone, DbtSchema, Eq, PartialEq)]
#[serde(untagged)]
pub enum ColumnPropertiesDimension {
    DimensionConfig(ColumnPropertiesDimensionConfig),
    DimensionType(ColumnPropertiesDimensionType),
}

#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, Eq, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ColumnPropertiesDimensionType {
    categorical,
    time,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, Eq, PartialEq)]
pub struct ColumnPropertiesDimensionConfig {
    #[serde(rename = "type")]
    pub type_: ColumnPropertiesDimensionType,
    pub is_partition: Option<bool>,
    pub label: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub config: Option<SemanticLayerElementConfig>,
    pub validity_params: Option<DimensionValidityParams>,
}

#[derive(UntaggedEnumDeserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum Entity {
    EntityConfig(EntityConfig),
    EntityType(ColumnPropertiesEntityType),
}

#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, Eq, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ColumnPropertiesEntityType {
    foreign,
    natural,
    primary,
    unique,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
pub struct EntityConfig {
    #[serde(rename = "type")]
    pub type_: ColumnPropertiesEntityType,
    pub name: Option<String>,
    pub description: Option<String>,
    pub label: Option<String>,
    pub config: Option<SemanticLayerElementConfig>,
}
