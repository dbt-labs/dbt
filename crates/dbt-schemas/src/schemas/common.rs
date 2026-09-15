//! Common Definitions for use in serializing and deserializing dbt nodes

use indexmap::IndexMap;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufReader, Read};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use dbt_adapter_core::{AdapterType, quote_char};
use dbt_common::{CodeLocationWithFile, ErrorCode, FsError, FsResult, err, fs_err};
use dbt_telemetry::NodeMaterialization;
use dbt_yaml::{DbtSchema, Spanned, UntaggedEnumDeserialize, Verbatim};
use hex;
use serde::{Deserialize, Deserializer, Serialize};
// Type alias for clarity
type YmlValue = dbt_yaml::Value;
use serde_with::skip_serializing_none;
use sha2::{Digest, Sha256};
use strum::{Display, EnumIter, EnumString};

use crate::dbt_types::RelationType;
use crate::schemas::dbt_column::{ColumnPropertiesDimensionType, Granularity};
use crate::schemas::manifest::BigqueryPartitionConfig;
use crate::schemas::manifest::common::SourceFileMetadata;
use crate::schemas::semantic_layer::semantic_manifest::SemanticLayerElementConfig;

use super::relations::base::ComponentName;
use super::serde::{
    StringOrArrayOfStrings, bool_or_string_bool, bool_or_string_bool_default, i64_or_string_i64,
    yaml_11_bool_default,
};

/// Indicates where schema metadata originates from.
///
/// - `Remote` (default): Schema is fetched from the remote warehouse
/// - `Local`: Schema is derived from YAML column definitions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, DbtSchema)]
#[serde(rename_all = "lowercase")]
pub enum SchemaOrigin {
    /// Schema metadata comes from the remote warehouse (default)
    #[default]
    Remote,
    /// Schema metadata is derived from YAML column definitions
    Local,
}

impl SchemaOrigin {
    /// Returns the string representation of the schema origin.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Local => "local",
        }
    }
}

impl std::fmt::Display for SchemaOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for SchemaOrigin {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "remote" => Ok(Self::Remote),
            "local" => Ok(Self::Local),
            _ => Err(format!(
                "Invalid schema_origin '{}': expected 'remote' or 'local'",
                s
            )),
        }
    }
}

#[derive(Default, Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
pub struct FreshnessRules {
    // F1: an empty rule object (`error_after: {}` with no children) must
    // deserialize successfully — the validator below treats it as
    // semantically equivalent to omitting the rule. Without `default`, the
    // custom deserializer on `count` causes serde to reject `{}` with a
    // "missing field `count`" error before the validator ever runs.
    #[serde(default, deserialize_with = "i64_or_string_i64")]
    pub count: Option<i64>,
    #[serde(default)]
    pub period: Option<FreshnessPeriod>,
}

impl FreshnessRules {
    pub fn validate(rule: Option<&Self>) -> FsResult<()> {
        let Some(rule) = rule else {
            return Ok(());
        };
        // F1: an empty rule object (`error_after: {}` or all children commented
        // out) is semantically equivalent to omitting the key. Mantle accepts
        // it; reject only when the rule is *partially* populated.
        if rule.is_empty() {
            return Ok(());
        }
        if rule.count.is_none() || rule.period.is_none() {
            return Err(fs_err!(
                ErrorCode::InvalidArgument,
                "count and period are required when freshness is provided, count: {:?}, period: {:?}",
                rule.count,
                rule.period
            ));
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.count.is_none() && self.period.is_none()
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdatesOn {
    #[default]
    Any,
    All,
}

impl std::fmt::Display for UpdatesOn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdatesOn::Any => write!(f, "any"),
            UpdatesOn::All => write!(f, "all"),
        }
    }
}

impl FromStr for UpdatesOn {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "any" => Ok(UpdatesOn::Any),
            "all" => Ok(UpdatesOn::All),
            _ => Err(format!("Unknown UpdatesOn value: {s}")),
        }
    }
}

#[derive(Default, Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ModelFreshnessRules {
    pub count: Option<i64>,
    pub period: Option<FreshnessPeriod>,
    pub updates_on: Option<UpdatesOn>,
}

impl PartialEq for ModelFreshnessRules {
    fn eq(&self, other: &Self) -> bool {
        self.count == other.count
            && self.period == other.period
            && updates_on_eq(&self.updates_on, &other.updates_on)
    }
}

impl Eq for ModelFreshnessRules {}

fn updates_on_eq(a: &Option<UpdatesOn>, b: &Option<UpdatesOn>) -> bool {
    match (a.as_ref(), b.as_ref()) {
        (None, None) => true,
        (Some(a_val), Some(b_val)) => a_val == b_val,
        (None, Some(b_val)) => b_val == &UpdatesOn::default(),
        (Some(a_val), None) => a_val == &UpdatesOn::default(),
    }
}

impl ModelFreshnessRules {
    pub fn validate(rule: Option<&Self>) -> FsResult<()> {
        if rule.is_none() {
            return Ok(());
        }
        let rule = rule.expect("rule should be Some now");
        let count_present = rule.count.is_some();
        let period_present = rule.period.is_some();

        if count_present != period_present {
            return Err(fs_err!(
                ErrorCode::InvalidArgument,
                "count and period are required when freshness is provided, count: {:?}, period: {:?}",
                rule.count,
                rule.period
            ));
        }
        Ok(())
    }

    /// Convert the freshness duration to seconds
    pub fn to_seconds(&self) -> i64 {
        let count = self.count.expect("count is required");
        let period = self.period.as_ref().expect("period is required");
        count
            * match period {
                FreshnessPeriod::second => 1,
                FreshnessPeriod::minute => 60,
                FreshnessPeriod::hour => 60 * 60,
                FreshnessPeriod::day => 60 * 60 * 24,
            }
    }
}

pub fn model_freshness_rules_or_duration<'de, D>(
    deserializer: D,
) -> Result<Option<ModelFreshnessRules>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ModelFreshnessRulesOrDuration {
        Duration(String),
        Rules(ModelFreshnessRules),
    }

    Option::<ModelFreshnessRulesOrDuration>::deserialize(deserializer)?
        .map(|value| match value {
            ModelFreshnessRulesOrDuration::Duration(duration) => {
                model_freshness_rules_from_duration(&duration).map_err(serde::de::Error::custom)
            }
            ModelFreshnessRulesOrDuration::Rules(rules) => Ok(rules),
        })
        .transpose()
}

fn model_freshness_rules_from_duration(duration: &str) -> Result<ModelFreshnessRules, String> {
    let parsed = humantime::parse_duration(duration)
        .map_err(|e| format!("invalid lag_tolerance duration {duration:?}: {e}"))?;
    if parsed.subsec_nanos() != 0 {
        return Err(format!(
            "invalid lag_tolerance duration {duration:?}: expected a whole number of seconds, minutes, hours, or days"
        ));
    }
    let seconds = parsed.as_secs();

    let (count, period) = if seconds == 0 {
        (0, FreshnessPeriod::minute)
    } else if seconds % (60 * 60 * 24) == 0 {
        (seconds / (60 * 60 * 24), FreshnessPeriod::day)
    } else if seconds % (60 * 60) == 0 {
        (seconds / (60 * 60), FreshnessPeriod::hour)
    } else if seconds % 60 == 0 {
        (seconds / 60, FreshnessPeriod::minute)
    } else {
        (seconds, FreshnessPeriod::second)
    };

    let count = i64::try_from(count).map_err(|_| {
        format!("invalid lag_tolerance duration {duration:?}: duration is too large")
    })?;

    Ok(ModelFreshnessRules {
        count: Some(count),
        period: Some(period),
        updates_on: None,
    })
}

#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum FreshnessPeriod {
    second,
    minute,
    hour,
    day,
}
impl FromStr for FreshnessPeriod {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "second" => Ok(FreshnessPeriod::second),
            "minute" => Ok(FreshnessPeriod::minute),
            "hour" => Ok(FreshnessPeriod::hour),
            "day" => Ok(FreshnessPeriod::day),
            _ => Err(()),
        }
    }
}
impl std::fmt::Display for FreshnessPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let period_str = match self {
            FreshnessPeriod::second => "second",
            FreshnessPeriod::minute => "minute",
            FreshnessPeriod::hour => "hour",
            FreshnessPeriod::day => "day",
        };
        write!(f, "{period_str}")
    }
}

/// Reference: https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/artifacts/resources/v1/source_definition.py#L36
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ExternalPartitionConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub data_type: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub meta: IndexMap<String, YmlValue>,

    pub __other__: IndexMap<String, YmlValue>,
}

#[derive(UntaggedEnumDeserialize, Serialize, Debug, Clone, DbtSchema)]
#[serde(untagged)]
pub enum ExternalPartition {
    String(String),
    ExternalPartitionConfig(ExternalPartitionConfig),
}

/// Reference: https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/artifacts/resources/v1/source_definition.py#L48
/// These always get serialized, even if none.
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ExternalTable {
    pub location: Option<String>,
    pub file_format: Option<String>,
    pub row_format: Option<String>,
    pub tbl_properties: Option<String>,
    // TODO: Add external partition validation as seen here:
    // https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/artifacts/resources/v1/source_definition.py#L36
    pub partitions: Option<Vec<ExternalPartition>>,

    // `external` allows arbitrary, externally typed additional properties.
    pub __other__: IndexMap<String, YmlValue>,
}

// We don't skip serializing none here because dbt project evaluator checks for the presence of either error_after or warn_after
// https://github.com/dbt-labs/dbt-project-evaluator/blob/94768b117573705e95a9456273de8e358efadb00/macros/unpack/get_source_values.sql#L27-L28
#[derive(Default, Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
pub struct FreshnessDefinition {
    #[serde(default, serialize_with = "serialize_freshness_rule")]
    pub error_after: Option<FreshnessRules>,
    #[serde(default, serialize_with = "serialize_freshness_rule")]
    pub warn_after: Option<FreshnessRules>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded_at_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded_at_query: Option<String>,
}

/// Custom serializer to ensure FreshnessRules are always objects, never null
fn serialize_freshness_rule<S>(
    rule: &Option<FreshnessRules>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match rule {
        Some(rule) => rule.serialize(serializer),
        None => FreshnessRules::default().serialize(serializer),
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum FreshnessStatus {
    Pass,
    Warn,
    Error,
}

impl std::fmt::Display for FreshnessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Error => "error",
        };
        write!(f, "{s}")
    }
}

/// Trait for types that can be merged, taking the last non-None value
pub trait Merge<T> {
    /// Merge with another instance, where the other's non-None values overwrite self
    fn merge(&self, other: &T) -> Self;
}

// Generic implementation for Option<T> where T: Clone
impl<T: Clone + Merge<T>> Merge<Option<T>> for Option<T> {
    fn merge(&self, other: &Option<T>) -> Self {
        match (self, other) {
            (Some(s), Some(o)) => Some(s.merge(o)),
            (None, Some(o)) => Some(o.clone()),
            (Some(s), None) => Some(s.clone()),
            (None, None) => None,
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, EnumIter, Eq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum DbtMaterialization {
    #[default]
    Snapshot,
    Seed,
    View,
    Table,
    Incremental,
    MaterializedView,
    External,
    Test,
    Ephemeral,
    Unit,
    Analysis,
    Function,
    /// only for databricks
    MetricView,
    /// only for databricks
    StreamingTable,
    /// only for snowflake
    DynamicTable,
    /// only for snowflake
    InteractiveTable,
    /// for inline SQL compilation
    Inline,
    #[serde(untagged)]
    Unknown(String),
}
impl FromStr for DbtMaterialization {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "view" => Ok(DbtMaterialization::View),
            "table" => Ok(DbtMaterialization::Table),
            "incremental" => Ok(DbtMaterialization::Incremental),
            "materialized_view" => Ok(DbtMaterialization::MaterializedView),
            "metric_view" => Ok(DbtMaterialization::MetricView),
            "external" => Ok(DbtMaterialization::External),
            "test" => Ok(DbtMaterialization::Test),
            "ephemeral" => Ok(DbtMaterialization::Ephemeral),
            "unit" => Ok(DbtMaterialization::Unit),
            "analysis" => Ok(DbtMaterialization::Analysis),
            "function" => Ok(DbtMaterialization::Function),
            "streaming_table" => Ok(DbtMaterialization::StreamingTable),
            "dynamic_table" => Ok(DbtMaterialization::DynamicTable),
            "interactive_table" => Ok(DbtMaterialization::InteractiveTable),
            "inline" => Ok(DbtMaterialization::Inline),
            other => Ok(DbtMaterialization::Unknown(other.to_string())),
        }
    }
}
impl From<DbtMaterialization> for String {
    fn from(materialization: DbtMaterialization) -> Self {
        materialization.to_string()
    }
}

impl std::fmt::Display for DbtMaterialization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let materialized_str = match self {
            DbtMaterialization::View => "view",
            DbtMaterialization::Table => "table",
            DbtMaterialization::Incremental => "incremental",
            DbtMaterialization::MaterializedView => "materialized_view",
            DbtMaterialization::MetricView => "metric_view",
            DbtMaterialization::External => "external",
            DbtMaterialization::Test => "test",
            DbtMaterialization::Ephemeral => "ephemeral",
            DbtMaterialization::Unit => "unit",
            DbtMaterialization::StreamingTable => "streaming_table",
            DbtMaterialization::DynamicTable => "dynamic_table",
            DbtMaterialization::InteractiveTable => "interactive_table",
            DbtMaterialization::Analysis => "analysis",
            DbtMaterialization::Function => "function",
            DbtMaterialization::Inline => "inline",
            DbtMaterialization::Unknown(s) => s.as_str(),
            DbtMaterialization::Snapshot => "snapshot",
            DbtMaterialization::Seed => "seed",
        };
        write!(f, "{materialized_str}")
    }
}

// Question (Ani): does this map correctly?
impl From<DbtMaterialization> for RelationType {
    fn from(materialization: DbtMaterialization) -> Self {
        match materialization {
            DbtMaterialization::Table => RelationType::Table,
            DbtMaterialization::View => RelationType::View,
            DbtMaterialization::MaterializedView => RelationType::MaterializedView,
            DbtMaterialization::MetricView => RelationType::MetricView,
            DbtMaterialization::Ephemeral => RelationType::Ephemeral,
            DbtMaterialization::External => RelationType::External,
            DbtMaterialization::Test => RelationType::External, // TODO Validate this
            // Incremental models materialize as tables on the warehouse, and
            // `is_incremental()` checks `relation.type == 'table'`. Mapping to
            // anything else (e.g. External) breaks dev-cloned incrementals —
            // the Jinja body takes the non-incremental branch even though the
            // CLONE has already produced a table.
            DbtMaterialization::Incremental => RelationType::Table,
            DbtMaterialization::Unit => RelationType::External, // TODO Validate this
            DbtMaterialization::StreamingTable => RelationType::StreamingTable,
            DbtMaterialization::DynamicTable => RelationType::DynamicTable,
            DbtMaterialization::InteractiveTable => RelationType::InteractiveTable,
            DbtMaterialization::Analysis => RelationType::External, // TODO Validate this
            DbtMaterialization::Inline => RelationType::Ephemeral, // Inline models don't materialize in DB
            DbtMaterialization::Unknown(_) => RelationType::External, // TODO Validate this
            DbtMaterialization::Snapshot => RelationType::Table,   // TODO Validate this
            DbtMaterialization::Seed => RelationType::Table,       // TODO Validate this
            DbtMaterialization::Function => RelationType::Function,
        }
    }
}

impl From<&DbtMaterialization> for NodeMaterialization {
    fn from(value: &DbtMaterialization) -> Self {
        match value {
            DbtMaterialization::Table => Self::Table,
            DbtMaterialization::View => Self::View,
            DbtMaterialization::MaterializedView => Self::MaterializedView,
            DbtMaterialization::MetricView => Self::MetricView,
            DbtMaterialization::Ephemeral => Self::Ephemeral,
            DbtMaterialization::External => Self::External,
            DbtMaterialization::Test => Self::Test,
            DbtMaterialization::Incremental => Self::Incremental,
            DbtMaterialization::Unit => Self::Unit,
            DbtMaterialization::StreamingTable => Self::StreamingTable,
            DbtMaterialization::DynamicTable => Self::DynamicTable,
            DbtMaterialization::InteractiveTable => Self::InteractiveTable,
            DbtMaterialization::Analysis => Self::Analysis,
            DbtMaterialization::Inline => Self::Ephemeral, // Inline is similar to ephemeral
            DbtMaterialization::Unknown(_) => Self::Custom,
            DbtMaterialization::Snapshot => Self::Snapshot,
            DbtMaterialization::Seed => Self::Seed,
            DbtMaterialization::Function => Self::Function,
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Display, DbtSchema)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum Access {
    Private,
    #[default]
    Protected,
    Public,
}

impl FromStr for Access {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "private" => Ok(Access::Private),
            "protected" => Ok(Access::Protected),
            "public" => Ok(Access::Public),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct NodeDependsOn {
    #[serde(default)]
    pub macros: Vec<String>,
    #[serde(default)]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub nodes_with_ref_location: Vec<(String, CodeLocationWithFile)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Copy, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct ResolvedQuoting {
    #[serde(deserialize_with = "bool_or_string_bool_default")]
    pub database: bool,
    #[serde(deserialize_with = "bool_or_string_bool_default")]
    pub identifier: bool,
    #[serde(deserialize_with = "bool_or_string_bool_default")]
    pub schema: bool,
}

impl Default for ResolvedQuoting {
    fn default() -> Self {
        // dbt rules
        Self::trues()
        // todo: however a much more sensible rule would be
        // Self::falses()
        // ... since SQL is case insensitive -- so let the dialect dictate and not the user...
    }
}

impl ResolvedQuoting {
    pub fn trues() -> Self {
        ResolvedQuoting {
            database: true,
            identifier: true,
            schema: true,
        }
    }
    pub fn falses() -> Self {
        ResolvedQuoting {
            database: false,
            identifier: false,
            schema: false,
        }
    }
    pub fn must_quote(&self, what: ComponentName) -> bool {
        match what {
            ComponentName::Database => self.database,
            ComponentName::Schema => self.schema,
            ComponentName::Identifier => self.identifier,
        }
    }
}

impl TryFrom<DbtQuoting> for ResolvedQuoting {
    type Error = Box<FsError>;

    fn try_from(value: DbtQuoting) -> FsResult<Self> {
        Ok(ResolvedQuoting {
            database: value.database.ok_or_else(|| fs_err!(
                ErrorCode::InvalidArgument,
                "Missing database in dbt quoting config. Failed to convert to ResolvedQuoting."
            ))?,
            identifier: value.identifier.ok_or_else(|| fs_err!(
                ErrorCode::InvalidArgument,
                "Missing identifier in dbt quoting config. Failed to convert to ResolvedQuoting."
            ))?,
            schema: value.schema.ok_or_else(|| fs_err!(
                ErrorCode::InvalidArgument,
                "Missing schema in dbt quoting config. Failed to convert to ResolvedQuoting."
            ))?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Default, Deserialize, PartialEq, Eq, Copy, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct DbtQuoting {
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "bool_or_string_bool"
    )]
    pub database: Option<bool>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "bool_or_string_bool"
    )]
    pub identifier: Option<bool>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "bool_or_string_bool"
    )]
    pub schema: Option<bool>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        deserialize_with = "bool_or_string_bool"
    )]
    pub snowflake_ignore_case: Option<bool>,
}

impl DbtQuoting {
    pub fn is_default(&self) -> bool {
        self.database.is_none() && self.identifier.is_none() && self.schema.is_none()
    }

    pub fn default_to(&mut self, other: &DbtQuoting) {
        self.database = self.database.or(other.database);
        self.identifier = self.identifier.or(other.identifier);
        self.schema = self.schema.or(other.schema);
    }

    /// A copy of `self` with every field it leaves unset taken from `fallback`.
    ///
    /// The layering primitive for quoting precedence: apply it once per layer,
    /// most specific first, and the first layer to set a field wins. Unlike
    /// [`Self::default_to`] this carries `snowflake_ignore_case` too, so a layer
    /// that sets only that field is not silently dropped.
    #[must_use]
    pub fn filled_from(&self, fallback: &DbtQuoting) -> DbtQuoting {
        DbtQuoting {
            database: self.database.or(fallback.database),
            schema: self.schema.or(fallback.schema),
            identifier: self.identifier.or(fallback.identifier),
            snowflake_ignore_case: self
                .snowflake_ignore_case
                .or(fallback.snowflake_ignore_case),
        }
    }

    /// Shallow last-non-None-wins merge of two user-supplied quoting layers.
    /// Returns `None` only when both inputs are `None` so callers can preserve
    /// "user set nothing" on the manifest (no adapter defaults folded in).
    /// Mirrors dbt-core's `source.quoting.merged(table.quoting)`.
    pub fn merge_user(source: Option<&Self>, table: Option<&Self>) -> Option<Self> {
        match (source, table) {
            (None, None) => None,
            _ => {
                let mut q = table.copied().unwrap_or_default();
                if let Some(s) = source {
                    q.default_to(s);
                }
                Some(q)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbtCheckColsSpec {
    /// A list of column names to be used for checking if a snapshot's source data set was updated
    Cols(Vec<String>),
    /// Use all columns to check whether a snapshot's source data set was updated
    All,
}

impl Serialize for DbtCheckColsSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            DbtCheckColsSpec::Cols(cols) => cols.serialize(serializer),
            DbtCheckColsSpec::All => "all".serialize(serializer),
        }
    }
}

impl TryFrom<StringOrArrayOfStrings> for DbtCheckColsSpec {
    type Error = Box<FsError>;
    fn try_from(value: StringOrArrayOfStrings) -> Result<Self, Self::Error> {
        match value {
            StringOrArrayOfStrings::String(all) => {
                if all == "all" {
                    Ok(DbtCheckColsSpec::All)
                } else {
                    err!(
                        ErrorCode::InvalidConfig,
                        "Invalid check_cols value: {}",
                        all
                    )
                }
            }
            StringOrArrayOfStrings::ArrayOfStrings(cols) => Ok(DbtCheckColsSpec::Cols(cols)),
        }
    }
}
impl<'de> Deserialize<'de> for DbtCheckColsSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = YmlValue::deserialize(deserializer)?;
        match value {
            YmlValue::String(all, _) => {
                // Validate that all is 'all'
                if all != "all" {
                    return Err(serde::de::Error::custom("Expected 'all'"));
                }
                Ok(DbtCheckColsSpec::All)
            }
            YmlValue::Sequence(col_list, _) => {
                let cols: Result<Vec<_>, D::Error> = col_list
                    .into_iter()
                    .map(|v| match v {
                        YmlValue::String(s, _) => Ok(s),
                        _ => Err(serde::de::Error::custom("Expected array of strings")),
                    })
                    .collect();
                match cols {
                    Ok(col_names) => Ok(DbtCheckColsSpec::Cols(col_names.into_iter().collect())),
                    Err(_) => Err(serde::de::Error::custom("Expected array of strings")),
                }
            }
            _ => Err(serde::de::Error::custom(format!(
                "Expected a string or array of strings for check_cols, got {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, EnumString, Display, DbtSchema)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum DbtBatchSize {
    Hour,
    Day,
    Month,
    Year,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct DbtContract {
    #[serde(
        default = "default_alias_types",
        deserialize_with = "yaml_11_bool_default"
    )]
    pub alias_types: bool,
    #[serde(default, deserialize_with = "yaml_11_bool_default")]
    pub enforced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<YmlValue>,
}

fn default_alias_types() -> bool {
    true
}

// DO NOT REMOVE. Somehow, `#[derive(Default)]` does not respect `#[serde(default = "default_alias_types")]`, so we need to implement it manually.
impl Default for DbtContract {
    fn default() -> Self {
        Self {
            alias_types: default_alias_types(),
            enforced: false,
            checksum: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, EnumString, Display, DbtSchema)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum DbtIncrementalStrategy {
    Append,
    Merge,
    #[serde(rename = "delete+insert")]
    #[strum(serialize = "delete+insert")]
    DeleteInsert,
    InsertOverwrite,
    Microbatch,
    /// replace_where (Databricks only)
    /// see https://docs.getdbt.com/reference/resource-configs/databricks-configs
    ReplaceWhere,
    /// legacy (ClickHouse only) — intermediate-table + swap approach
    /// https://github.com/ClickHouse/dbt-clickhouse/blob/main/dbt/adapters/clickhouse/impl.py
    Legacy,
    #[strum(default)]
    #[serde(untagged)]
    Custom(String),
}

#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize, PartialEq, Eq, DbtSchema)]
#[serde(untagged)]
pub enum DbtUniqueKey {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnSchemaChange {
    // Matches dbt-core's default: `on_schema_change: Optional[str] = "ignore"`
    // (core/dbt/artifacts/resources/v1/config.py).
    #[default]
    Ignore,
    AppendNewColumns,
    Fail,
    SyncAllColumns,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnConfigurationChange {
    #[default]
    Apply,
    Continue,
    Fail,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnError {
    SkipChildren,
    Continue,
}

impl From<StringOrArrayOfStrings> for DbtUniqueKey {
    fn from(value: StringOrArrayOfStrings) -> Self {
        match value {
            StringOrArrayOfStrings::String(s) => DbtUniqueKey::Single(s),
            StringOrArrayOfStrings::ArrayOfStrings(v) => DbtUniqueKey::Multiple(v),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum HardDeletes {
    Ignore,
    Invalidate,
    NewRecord,
}

// Impl try from string
impl TryFrom<String> for HardDeletes {
    type Error = Box<FsError>;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(match value.as_str() {
            "ignore" => HardDeletes::Ignore,
            "invalidate" => HardDeletes::Invalidate,
            "new_record" => HardDeletes::NewRecord,
            _ => {
                return err!(
                    ErrorCode::InvalidConfig,
                    "Invalid hard_deletes value: {}",
                    value
                );
            }
        })
    }
}

/// Constraints (model level or column level)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct Constraint {
    #[serde(rename = "type")]
    pub type_: ConstraintType,
    pub expression: Option<String>,
    pub name: Option<String>,
    // Only ForeignKey constraints accept: a relation input
    // ref(), source() etc
    pub to: Option<Spanned<String>>,
    /// Only ForeignKey constraints accept: a list columns in that table
    /// containing the corresponding primary or unique key.
    #[serde(
        default,
        deserialize_with = "crate::schemas::serde::string_or_array",
        serialize_with = "crate::schemas::serde::serialize_none_as_empty_vec"
    )]
    pub to_columns: Option<Vec<String>>,
    pub warn_unsupported: Option<bool>,
    pub warn_unenforced: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintSupport {
    Enforced,
    NotEnforced,
    NotSupported,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintType {
    #[default]
    NotNull,
    Unique,
    PrimaryKey,
    ForeignKey,
    Check,
    Custom,
}

impl ConstraintType {
    /// The dbt-core `ConstraintType` enum value, e.g. for use in warning messages.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConstraintType::NotNull => "not_null",
            ConstraintType::Unique => "unique",
            ConstraintType::PrimaryKey => "primary_key",
            ConstraintType::ForeignKey => "foreign_key",
            ConstraintType::Check => "check",
            ConstraintType::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize)]
#[serde(untagged)]
pub enum DbtChecksum {
    String(String),
    Object(DbtChecksumObject),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbtChecksumObject {
    /// The algorithm of the checksum, e.g., "sha256".
    pub name: String,
    /// Hex-encoded string computed by the algorithm.
    pub checksum: String,
}

impl Default for DbtChecksum {
    fn default() -> Self {
        // dbt-core FileHash.empty() → {"name": "none", "checksum": ""}.
        // Generic tests have no source file and use this as their checksum.
        // name:"" diverges from Mantle state manifests and causes every
        // generic test to appear as state:modified on every run.
        Self::Object(DbtChecksumObject {
            name: "none".to_string(),
            checksum: "".to_string(),
        })
    }
}

impl PartialEq for DbtChecksum {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::String(s1), Self::String(s2)) => s1 == s2,
            (Self::Object(o1), Self::Object(o2)) => {
                o1.name.to_lowercase() == o2.name.to_lowercase() && o1.checksum == o2.checksum
            }
            (Self::String(c1), Self::Object(o2)) => *c1 == o2.checksum,
            (Self::Object(o1), Self::String(c2)) => o1.checksum == *c2,
        }
    }
}

impl DbtChecksum {
    /// Returns the checksum string value regardless of variant.
    pub fn as_checksum_string(&self) -> &str {
        match self {
            Self::String(s) => s,
            Self::Object(o) => &o.checksum,
        }
    }

    pub fn to_checksum_string(&self) -> String {
        self.as_checksum_string().to_string()
    }

    pub fn hash(s: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(s);
        let checksum = hasher.finalize();
        Self::Object(DbtChecksumObject {
            name: "sha256".to_string(),
            checksum: hex::encode(checksum),
        })
    }

    pub fn seed_content_checksum<R: Read>(reader: R) -> std::io::Result<Self> {
        const SEED_HASH_CHUNK_SIZE: u64 = 1024 * 1024; // 1 MiB
        Self::seed_content_checksum_chunked(reader, SEED_HASH_CHUNK_SIZE)
    }

    pub(crate) fn seed_content_checksum_chunked<R: Read>(
        mut reader: R,
        chunk_size: u64,
    ) -> std::io::Result<Self> {
        let mut hasher = Sha256::new();
        let mut prev_cr = false; // carry: `\r` seen at the end of the previous chunk

        let mut read_chunk = || -> std::io::Result<Vec<u8>> {
            let mut raw = Vec::new();
            reader.by_ref().take(chunk_size).read_to_end(&mut raw)?; // handle.read(chunk_size)
            let mut out = Vec::with_capacity(raw.len());
            for &b in &raw {
                match b {
                    b'\n' if prev_cr => prev_cr = false, // drop `\n` of a split `\r\n`
                    b'\r' => {
                        // `\r` -> `\n`
                        prev_cr = true;
                        out.push(b'\n');
                    }
                    b => {
                        prev_cr = false;
                        out.push(b);
                    }
                }
            }
            Ok(out)
        };

        let mut chunk = read_chunk()?.trim_ascii_start().to_vec();

        while !chunk.is_empty() {
            let next = read_chunk()?;
            if next.is_empty() {
                chunk = chunk.trim_ascii_end().to_vec();
            }
            hasher.update(&chunk);
            chunk = next;
        }

        Ok(Self::Object(DbtChecksumObject {
            name: "sha256".to_string(),
            checksum: hex::encode(hasher.finalize()),
        }))
    }

    pub fn seed_content_checksum_legacy(s: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        let utf8_string = String::from_utf8_lossy(s);
        hasher.update(utf8_string.trim().as_bytes());
        Self::Object(DbtChecksumObject {
            name: "sha256".to_string(),
            checksum: hex::encode(hasher.finalize()),
        })
    }

    pub fn seed_file_checksum(
        file_path: &Path,
        original_file_path: &str,
        maximum_seed_size_mib: u64,
    ) -> FsResult<Self> {
        // Configured value is in MiB -> convert to bytes. `None` => 1 MiB default,
        // `Some(0)` => no limit.
        const BYTES_PER_MIB: u64 = 1024 * 1024;
        let maximum_seed_size = maximum_seed_size_mib.saturating_mul(BYTES_PER_MIB);

        let size = std::fs::metadata(file_path)
            .map_err(|e| {
                fs_err!(
                    ErrorCode::IoError,
                    "Failed to stat seed file '{}' while computing checksum: {}",
                    file_path.display(),
                    e
                )
            })?
            .len();
        if maximum_seed_size != 0 && size > maximum_seed_size {
            Ok(Self::Object(DbtChecksumObject {
                name: "path".to_string(),
                checksum: original_file_path.to_string(),
            }))
        } else {
            let file = std::fs::File::open(file_path).map_err(|e| {
                fs_err!(
                    ErrorCode::IoError,
                    "Failed to open seed file '{}' for content hashing: {}",
                    file_path.display(),
                    e
                )
            })?;
            Self::seed_content_checksum(BufReader::new(file)).map_err(|e| {
                fs_err!(
                    ErrorCode::IoError,
                    "Failed to hash seed file '{}' contents: {}",
                    file_path.display(),
                    e
                )
            })
        }
    }
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct IncludeExclude {
    #[serde(
        default,
        deserialize_with = "crate::schemas::serde::string_or_number_or_array_to_string_array"
    )]
    pub exclude: Option<StringOrArrayOfStrings>,
    #[serde(
        default,
        deserialize_with = "crate::schemas::serde::string_or_number_or_array_to_string_array"
    )]
    pub include: Option<StringOrArrayOfStrings>,
}

#[derive(Debug, Serialize, Deserialize, Clone, DbtSchema, Default)]
pub struct Expect {
    pub rows: Option<Rows>,
    #[serde(default)]
    pub format: Formats,
    pub fixture: Option<String>,
}

#[derive(
    Debug, Serialize, Default, Deserialize, Clone, EnumString, Display, DbtSchema, PartialEq,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Formats {
    #[default]
    Dict,
    Csv,
    Sql,
}

#[derive(Debug, Serialize, Default, Deserialize, Clone, DbtSchema)]
pub struct Given {
    pub input: Spanned<String>,
    pub rows: Option<Rows>,
    #[serde(default)]
    pub format: Formats,
    pub fixture: Option<String>,
}

#[derive(Debug, Serialize, UntaggedEnumDeserialize, Clone, DbtSchema)]
#[serde(untagged)]
pub enum Rows {
    String(String),
    List(Vec<BTreeMap<String, YmlValue>>),
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, DbtSchema)]
pub struct DocsConfig {
    #[serde(default = "default_show", deserialize_with = "yaml_11_bool_default")]
    pub show: bool,
    pub node_color: Option<String>,
}

impl Default for DocsConfig {
    fn default() -> Self {
        Self {
            show: default_show(),
            node_color: None,
        }
    }
}

fn default_show() -> bool {
    true
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, Default, DbtSchema)]
pub struct PersistDocsConfig {
    #[serde(deserialize_with = "bool_or_string_bool", default)]
    pub columns: Option<bool>,
    #[serde(deserialize_with = "bool_or_string_bool", default)]
    pub relation: Option<bool>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, DbtSchema, Default)]
pub struct ScheduleConfig {
    pub cron: Option<String>,
    pub time_zone_value: Option<String>,
}

/// Schedule configuration that accepts both string and structured formats.
/// This allows users to specify schedule as either:
/// - A string: `schedule: "USING CRON 0,15,30,45 * * * * UTC"`
/// - A structured config: `schedule: { cron: "0 * * * *", time_zone_value: "UTC" }`
#[derive(UntaggedEnumDeserialize, Serialize, Debug, Clone, PartialEq, Eq, DbtSchema)]
#[serde(untagged)]
pub enum Schedule {
    String(String),
    ScheduleConfig(ScheduleConfig),
}

impl Schedule {
    /// Convert Schedule to ScheduleConfig
    pub fn to_schedule_config(&self) -> ScheduleConfig {
        match self {
            Schedule::String(s) => ScheduleConfig {
                cron: Some(s.clone()),
                time_zone_value: None,
            },
            Schedule::ScheduleConfig(config) => config.clone(),
        }
    }
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, DbtSchema)]
pub struct RowFilterConfig {
    pub function: Option<String>,
    pub columns: Option<StringOrArrayOfStrings>,
}

#[derive(UntaggedEnumDeserialize, Serialize, Debug, Clone, PartialEq, Eq, DbtSchema)]
#[serde(untagged)]
pub enum Hooks {
    String(String),
    ArrayOfStrings(Vec<String>),
    HookConfig(HookConfig),
    HookConfigArray(Vec<HookConfig>),
}

impl Hooks {
    pub fn extend(&mut self, other: &Self) {
        let mut new_hooks = vec![];
        match other {
            Hooks::String(s) => {
                new_hooks.push(HookConfig {
                    sql: Some(s.clone()),
                    transaction: Some(true),
                    index: None,
                });
            }
            Hooks::ArrayOfStrings(v) => {
                new_hooks.extend(v.iter().map(|s| HookConfig {
                    sql: Some(s.clone()),
                    transaction: Some(true),
                    index: None,
                }));
            }
            Hooks::HookConfig(hook_config) => {
                new_hooks.push(hook_config.clone());
            }
            Hooks::HookConfigArray(hook_configs) => {
                new_hooks.extend(hook_configs.clone());
            }
        }
        match self {
            Hooks::String(s) => {
                new_hooks.push(HookConfig {
                    sql: Some(s.clone()),
                    transaction: Some(true),
                    index: None,
                });
            }
            Hooks::ArrayOfStrings(v) => {
                new_hooks.extend(v.iter().map(|s| HookConfig {
                    sql: Some(s.clone()),
                    transaction: Some(true),
                    index: None,
                }));
            }
            Hooks::HookConfig(hook_config) => {
                new_hooks.push(hook_config.clone());
            }
            Hooks::HookConfigArray(hook_configs) => {
                new_hooks.extend(hook_configs.clone());
            }
        }
        *self = Hooks::HookConfigArray(new_hooks);
    }

    pub fn to_hook_config_array(&self) -> Vec<HookConfig> {
        match self {
            Hooks::String(s) => vec![HookConfig {
                sql: Some(s.clone()),
                transaction: Some(true),
                index: None,
            }],
            Hooks::ArrayOfStrings(v) => v
                .iter()
                .map(|s| HookConfig {
                    sql: Some(s.clone()),
                    transaction: Some(true),
                    index: None,
                })
                .collect(),
            Hooks::HookConfig(h) => vec![h.clone()],
            Hooks::HookConfigArray(v) => v.clone(),
        }
    }

    /// Compare hooks where different variants can be equal if they contain the same SQL
    /// These checks are needed so that we can conform to dbt-core/dbt-mantle
    /// when comparing hooks.
    pub fn hooks_equal(&self, other: &Self) -> bool {
        // Helper to check if a Hooks value is effectively empty
        let is_empty_hooks = |hooks: &Hooks| -> bool {
            match hooks {
                Hooks::ArrayOfStrings(vec) => vec.is_empty(),
                _ => false,
            }
        };

        // If both are the same variant and equal, return true
        if self == other {
            return true;
        }

        // Check if one is empty array and the other is something that should be considered equal
        if is_empty_hooks(self) && is_empty_hooks(other) {
            return true;
        }

        // Compare different variants by their SQL content
        match (self, other) {
            // String vs HookConfig: compare the string with the SQL field
            (Hooks::String(s), Hooks::HookConfig(config))
            | (Hooks::HookConfig(config), Hooks::String(s)) => config.sql.as_ref() == Some(s),

            // ArrayOfStrings vs HookConfigArray: compare each string with corresponding SQL field
            (Hooks::ArrayOfStrings(strings), Hooks::HookConfigArray(configs))
            | (Hooks::HookConfigArray(configs), Hooks::ArrayOfStrings(strings)) => {
                // Both must have the same length
                if strings.len() != configs.len() {
                    return false;
                }

                // Each string must match the corresponding config's SQL
                strings
                    .iter()
                    .zip(configs.iter())
                    .all(|(s, config)| config.sql.as_ref() == Some(s))
            }

            // String vs ArrayOfStrings with single element
            (Hooks::String(s), Hooks::ArrayOfStrings(vec))
            | (Hooks::ArrayOfStrings(vec), Hooks::String(s)) => {
                vec.len() == 1 && vec.first() == Some(s)
            }

            // HookConfig vs HookConfigArray with single element
            (Hooks::HookConfig(config), Hooks::HookConfigArray(vec))
            | (Hooks::HookConfigArray(vec), Hooks::HookConfig(config)) => {
                vec.len() == 1 && vec.first() == Some(config)
            }

            // String vs HookConfigArray with single element
            (Hooks::String(s), Hooks::HookConfigArray(configs))
            | (Hooks::HookConfigArray(configs), Hooks::String(s)) => {
                configs.len() == 1 && configs.first().and_then(|c| c.sql.as_ref()) == Some(s)
            }

            // ArrayOfStrings vs HookConfig - only equal if array has one element
            (Hooks::ArrayOfStrings(strings), Hooks::HookConfig(config))
            | (Hooks::HookConfig(config), Hooks::ArrayOfStrings(strings)) => {
                strings.len() == 1 && strings.first() == config.sql.as_ref()
            }

            _ => false,
        }
    }
}

// Helper function to compare hooks wrapped in Verbatim<Option<Hooks>> where None equals Some(ArrayOfStrings([]))
pub fn hooks_equal(a: &Verbatim<Option<Hooks>>, b: &Verbatim<Option<Hooks>>) -> bool {
    match (a.as_ref(), b.as_ref()) {
        (None, None) => true,
        (None, Some(b_hooks)) => {
            // Check if b_hooks is effectively empty
            matches!(b_hooks, Hooks::ArrayOfStrings(vec) if vec.is_empty())
        }
        (Some(a_hooks), None) => {
            // Check if a_hooks is effectively empty
            matches!(a_hooks, Hooks::ArrayOfStrings(vec) if vec.is_empty())
        }
        (Some(a_hooks), Some(b_hooks)) => a_hooks.hooks_equal(b_hooks),
    }
}

/// Serializes hooks in dbt-core's manifest shape: always a `List[Hook]`, and `[]` rather
/// than `null` when unset, whatever form the user authored (bare string, list of strings,
/// or `{sql, transaction}` mapping). For config structs that are serialized straight into
/// the manifest; the ones with a dedicated `Manifest*Config` normalize in `From` instead.
pub fn serialize_hooks_as_list<S>(
    hooks: &Verbatim<Option<Hooks>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let normalized = match &**hooks {
        Some(hooks) => hooks.to_hook_config_array(),
        None => Vec::new(),
    };
    normalized.serialize(serializer)
}

// `skip_serializing_none` only rewrites fields whose declared outer type is
// `Option`, so it cannot elide a field through a `Verbatim` wrapper.
pub fn verbatim_option_is_none<T>(value: &Verbatim<Option<T>>) -> bool {
    value.is_none()
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq, DbtSchema)]
pub struct HookConfig {
    pub sql: Option<String>,
    pub transaction: Option<bool>,
    pub index: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema, PartialEq)]
pub struct Dimension {
    pub name: String,
    #[serde(rename = "type")]
    pub dimension_type: ColumnPropertiesDimensionType,
    pub description: Option<String>,
    pub label: Option<String>,
    #[serde(default = "default_false")]
    pub is_partition: bool,
    pub type_params: Option<DimensionTypeParams>,
    pub expr: Option<String>,
    pub metadata: Option<SourceFileMetadata>,
    pub config: Option<SemanticLayerElementConfig>,
}

pub fn default_false() -> bool {
    false
}

pub fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema, PartialEq, Eq, Default)]
pub struct DimensionTypeParams {
    pub time_granularity: Option<Granularity>,
    pub validity_params: Option<DimensionValidityParams>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct DimensionValidityParams {
    #[serde(default = "default_false")]
    pub is_start: bool,
    #[serde(default = "default_false")]
    pub is_end: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct SemanticModelDependsOn {
    pub macros: Vec<String>,
    pub nodes: Vec<String>,
}

#[derive(Default, Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StoreFailuresAs {
    #[default]
    Ephemeral,
    Table,
    View,
}

#[derive(
    Debug, Serialize, Default, Deserialize, Clone, PartialEq, Eq, EnumString, Display, DbtSchema,
)]
#[serde(rename_all = "UPPERCASE")]
#[schemars(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Severity {
    #[default]
    #[serde(alias = "error", alias = "Error")]
    Error,
    #[serde(alias = "warn", alias = "Warn")]
    Warn,
}

/// Parses a `deprecation_date` string in any of the documented input formats
/// (bare date, or a full datetime with or without a UTC offset -- see
/// https://docs.getdbt.com/reference/resource-properties/deprecation_date)
/// into a timezone-aware timestamp. An already offset-aware input keeps its
/// original offset; a naive (offset-less) datetime is assumed to be UTC.
pub fn parse_deprecation_date(raw: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    let raw = raw.trim();

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt);
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f%z", "%Y-%m-%d %H:%M:%S%z"] {
        if let Ok(dt) = chrono::DateTime::parse_from_str(raw, format) {
            return Some(dt);
        }
    }

    let naive_utc_to_fixed = |naive: chrono::NaiveDateTime| {
        chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(naive, chrono::Utc)
            .fixed_offset()
    };
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, format) {
            return Some(naive_utc_to_fixed(naive));
        }
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let naive = date.and_hms_opt(0, 0, 0).unwrap();
        return Some(naive_utc_to_fixed(naive));
    }

    None
}

/// Normalizes a raw `deprecation_date` string (as authored in YAML) to an
/// RFC 3339 string with an explicit UTC offset, matching dbt-core's manifest
/// output. Falls back to the original string if it doesn't match any
/// documented format.
pub fn normalize_deprecation_date(raw: &str) -> String {
    parse_deprecation_date(raw)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| raw.to_string())
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct Versions {
    pub v: YmlValue,
    pub deprecation_date: Option<String>,
    pub defined_in: Option<String>,
    pub description: Option<String>,
    pub access: Option<String>,
    pub config: Verbatim<Option<dbt_yaml::Value>>,
    pub constraints: Option<Vec<crate::schemas::properties::model_properties::ModelConstraint>>,
    pub data_tests: Option<Vec<crate::schemas::data_tests::DataTests>>,
    pub tests: Option<Vec<crate::schemas::data_tests::DataTests>>,
    // NOTE: this doc comment is surfaced as the JSON Schema `description` (editor hover text), so
    // keep it user-facing. Implementation rationale goes in the plain comment below.
    /// Columns for this version. Each entry is either a column definition or the single optional
    /// `include`/`exclude` directive controlling which model-level columns this version inherits.
    /// When no directive is given, every model-level column is inherited.
    //
    // Deliberately a normal field -- neither `skip_deserializing` nor `Verbatim` -- so that
    // `into_typed_with_jinja` walks it and renders `description: '{{ doc(...) }}'` on a version
    // column exactly like it does on a model-level column (dbt-labs/fs#13334). Moving it back
    // into `__additional_properties__` would silently reintroduce that bug.
    #[serde(default)]
    pub columns: Option<Vec<crate::schemas::dbt_column::VersionColumnProperties>>,
    pub __additional_properties__: Verbatim<HashMap<String, YmlValue>>,
}

impl Versions {
    pub fn get_version(&self) -> Option<String> {
        match &self.v {
            dbt_yaml::Value::String(s, _) => Some(s.to_string()),
            dbt_yaml::Value::Number(n, _) => Some(n.to_string()),
            _ => None,
        }
    }
}
/// Get the semantic names for database, schema, and identifier
/// This function will parse the database, schema, and identifier
/// according to the dialect and quoting rules.
pub fn normalize_quoting(
    quoting: &ResolvedQuoting,
    adapter_type: AdapterType,
    database: &str,
    schema: &str,
    identifier: &str,
) -> (String, String, String, ResolvedQuoting) {
    let (database, database_quoting) = _normalize_quote(quoting.database, adapter_type, database);
    let (schema, schema_quoting) = _normalize_quote(quoting.schema, adapter_type, schema);
    let (identifier, identifier_quoting) =
        _normalize_quote(quoting.identifier, adapter_type, identifier);
    (
        database,
        schema,
        identifier,
        ResolvedQuoting {
            database: database_quoting,
            schema: schema_quoting,
            identifier: identifier_quoting,
        },
    )
}

pub fn normalize_quote(quoting: bool, adapter_type: AdapterType, name: &str) -> (String, bool) {
    _normalize_quote(quoting, adapter_type, name)
}

pub fn _normalize_quote(quoting: bool, adapter_type: AdapterType, name: &str) -> (String, bool) {
    let q = quote_char(adapter_type);
    let quoted = name.len() > 1 && name.starts_with(q) && name.ends_with(q);

    // If the name is quoted, but the quote config is false, we need to unquote the name
    if (quoted && !quoting) && !name.is_empty() {
        (name[1..name.len() - 1].to_string(), true)
    } else {
        (name.to_string(), quoting)
    }
}

/// Normalize SQL by compacting multiple spaces and newlines into a single space.
/// This ensures consistent checksums regardless of formatting differences
/// due to multiple spaces and newlines.
pub fn normalize_sql(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len());
    let mut last_was_whitespace = false;

    for c in sql.chars() {
        if c.is_whitespace() {
            if !last_was_whitespace {
                result.push(' ');
                last_was_whitespace = true;
            }
        } else {
            result.push(c);
            last_was_whitespace = false;
        }
    }

    result.trim().to_string()
}

/// Merge two meta maps, with the second map's values taking precedence on key conflicts.
pub fn merge_meta(
    base_meta: Option<IndexMap<String, YmlValue>>,
    update_meta: Option<IndexMap<String, YmlValue>>,
) -> Option<IndexMap<String, YmlValue>> {
    match (base_meta, update_meta) {
        (Some(base_map), Some(update_map)) => {
            let mut merged = base_map;
            merged.extend(update_map);
            Some(merged)
        }
        (None, Some(update_map)) => Some(update_map),
        (Some(base_map), None) => Some(base_map),
        (None, None) => None,
    }
}

/// Merge two string lists, deduplicating and sorting the result.
///
/// Prefer this for unordered set-like configs (e.g. classifiers). For tags,
/// use [`merge_tags`] so inheritance order matches dbt-core.
pub fn merge_vec(
    base_vec: Option<Vec<String>>,
    update_vec: Option<Vec<String>>,
) -> Option<Vec<String>> {
    match (base_vec, update_vec) {
        // If both are None, result is None
        (None, None) => None,
        // If either has a value (even empty), we preserve that semantic meaning
        (Some(mut base), Some(update)) => {
            base.extend(update);
            base.sort();
            base.dedup();
            Some(base)
        }
        // If only one side has a value, use it
        (Some(base), None) => Some(base),
        (None, Some(update)) => Some(update),
    }
}

/// Merge inherited tags in dbt-core order: parent (less specific) first, then
/// child (more specific), with first-seen deduplication and no alphabetical sort.
///
/// dbt-core concatenates additive tags along the config hierarchy. Callers that
/// depend on list position (e.g. custom schema naming from `tags[0]`) require
/// this order for Core/Fusion parity. See dbt-labs/dbt-core#15590.
pub fn merge_tags(
    parent_tags: Option<Vec<String>>,
    child_tags: Option<Vec<String>>,
) -> Option<Vec<String>> {
    match (parent_tags, child_tags) {
        (None, None) => None,
        (Some(mut parent), Some(child)) => {
            parent.extend(child);
            dedup_preserve_order(parent)
        }
        (Some(parent), None) => dedup_preserve_order(parent),
        (None, Some(child)) => dedup_preserve_order(child),
    }
}

fn dedup_preserve_order(tags: Vec<String>) -> Option<Vec<String>> {
    let mut seen = std::collections::HashSet::with_capacity(tags.len());
    let mut out = Vec::with_capacity(tags.len());
    for tag in tags {
        if seen.insert(tag.clone()) {
            out.push(tag);
        }
    }
    Some(out)
}

pub fn conform_normalized_snapshot_raw_code_to_mantle_format(normalized_full: &str) -> String {
    // Strip snapshot tags to match dbt-mantle behavior.
    //
    // This fn runs *after* `normalize_sql`, which collapses every run of
    // whitespace into a single ASCII space. So an on-disk `{%snapshot foo%}`
    // (no spaces) and `{%- snapshot foo -%}` (whitespace control) both arrive
    // here as variants of `{% snapshot foo %}` etc. We must accept all of:
    //   `{%snapshot`, `{% snapshot`, `{%-snapshot`, `{%- snapshot`
    // and the corresponding `endsnapshot` forms — otherwise the strip silently
    // no-ops and Fusion's recalculated checksum diverges from Mantle's
    // pre-stripped raw_code, breaking `state:modified.body` parity.
    let find_opening = |s: &str| -> Option<usize> {
        ["{%-snapshot", "{%- snapshot", "{%snapshot", "{% snapshot"]
            .iter()
            .filter_map(|p| s.find(*p))
            .min()
    };
    let rfind_closing = |s: &str| -> Option<usize> {
        [
            "{%-endsnapshot",
            "{%- endsnapshot",
            "{%endsnapshot",
            "{% endsnapshot",
        ]
        .iter()
        .filter_map(|p| s.rfind(*p))
        .max()
    };

    // Remove everything before and including the opening snapshot tag.
    let sql_without_opening = find_opening(normalized_full)
        .and_then(|start_pos| {
            let after_tag_start = &normalized_full[start_pos..];
            // Scoped to this tag's own boundary: the *nearest* `%}` after
            // `start_pos` always closes this tag, dashed or not, because a
            // snapshot name is a bare identifier that can't itself contain
            // `%}`. Searching for `-%}` first (as before) would skip past
            // this tag's own plain `%}` and match a later, unrelated inner
            // tag's dashed close instead (e.g. `{% snapshot foo %} ...
            // {% for x in y -%}`), stripping real body content.
            after_tag_start
                .find("%}")
                .map(|end_offset| &normalized_full[start_pos + end_offset + 2..])
        })
        .unwrap_or(normalized_full);

    // Strip the closing endsnapshot tag.
    let normalized_sql = sql_without_opening
        .strip_suffix("-%}")
        .or_else(|| sql_without_opening.strip_suffix("%}"))
        .and_then(|s| rfind_closing(s).map(|pos| &s[..pos]))
        .unwrap_or(sql_without_opening);

    // Trim boundary whitespace left behind by tag stripping. Without this, on-disk
    // snapshot SQL (which has a `{% snapshot %}` wrapper to strip) and Mantle's
    // pre-stripped raw_code produce different leading/trailing whitespace and hash
    // to different digests, breaking `state:modified.body` parity.
    normalized_sql.trim().to_string()
}

/// Schema refresh interval configuration.
///
/// This type represents how often to refresh source schemas from remote.
/// Default: 1 hour.
///
/// Examples:
/// - `"30m"`, `"2h"`, `"1d"` - refresh after the specified duration
/// - `"never"` - never automatically refresh (only manual sync)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaRefreshInterval {
    /// Refresh after the specified duration
    Duration(Duration), //todo validate that this deserializes as claimed
    /// Never automatically refresh
    Never,
}

impl schemars::JsonSchema for SchemaRefreshInterval {
    fn schema_name() -> String {
        "SchemaRefreshInterval".to_string()
    }

    fn json_schema(_gen: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        schemars::schema::Schema::Object(schemars::schema::SchemaObject {
            instance_type: Some(schemars::schema::InstanceType::String.into()),
            metadata: Some(Box::new(schemars::schema::Metadata {
                description: Some(
                    "Schema refresh interval: '30m', '2h', '1d', or 'never'".to_string(),
                ),
                ..Default::default()
            })),
            ..Default::default()
        })
    }
}

impl Default for SchemaRefreshInterval {
    fn default() -> Self {
        Self::Duration(Duration::from_hours(1))
    }
}

impl SchemaRefreshInterval {
    /// Creates a new interval from hours.
    pub fn from_hours(hours: u64) -> Self {
        Self::Duration(Duration::from_hours(hours))
    }

    /// Returns the duration as seconds, or None if set to never refresh.
    pub fn as_secs(&self) -> Option<u64> {
        match self {
            Self::Duration(d) => Some(d.as_secs()),
            Self::Never => None,
        }
    }

    /// Returns the duration, or None if set to never refresh.
    pub fn as_duration(&self) -> Option<Duration> {
        match self {
            Self::Duration(d) => Some(*d),
            Self::Never => None,
        }
    }

    /// Returns true if this interval is set to never refresh.
    pub fn is_never(&self) -> bool {
        matches!(self, Self::Never)
    }
}

impl From<Duration> for SchemaRefreshInterval {
    fn from(duration: Duration) -> Self {
        Self::Duration(duration)
    }
}

impl From<SchemaRefreshInterval> for Option<Duration> {
    fn from(interval: SchemaRefreshInterval) -> Self {
        interval.as_duration()
    }
}

impl FromStr for SchemaRefreshInterval {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("never") {
            return Ok(Self::Never);
        }
        // Use humantime to parse duration strings like "30m", "2h", "1d"
        let duration = humantime::parse_duration(s).map_err(|e| {
            format!(
                "Invalid schema_refresh_interval '{}': {}. \
                 Expected format like '30m', '2h', '1d', or 'never'",
                s, e
            )
        })?;

        // Validate duration bounds
        const MIN_DURATION: Duration = Duration::from_secs(0); // 0 seconds (always refresh)
        const MAX_DURATION: Duration = Duration::from_secs(365 * 24 * 60 * 60); // 1 year

        if duration < MIN_DURATION {
            return Err(format!(
                "schema_refresh_interval '{}' is invalid. Must be non-negative",
                s
            ));
        }

        if duration > MAX_DURATION {
            return Err(format!(
                "schema_refresh_interval '{}' is too long. Maximum allowed is 1 year",
                s
            ));
        }

        Ok(Self::Duration(duration))
    }
}

impl<'de> Deserialize<'de> for SchemaRefreshInterval {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl Serialize for SchemaRefreshInterval {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Duration(d) => {
                serializer.serialize_str(&humantime::format_duration(*d).to_string())
            }
            Self::Never => serializer.serialize_str("never"),
        }
    }
}

impl std::fmt::Display for SchemaRefreshInterval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duration(d) => write!(f, "{}", humantime::format_duration(*d)),
            Self::Never => write!(f, "never"),
        }
    }
}

/// Configuration for schema synchronization behavior.
///
/// This can be specified at source level (default for all tables) or
/// at individual table level (overrides source-level settings).
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DbtSchema, Default)]
pub struct SyncConfig {
    /// How often to refresh the schema from the warehouse.
    /// Examples: "30m", "2h", "1d", "never"
    pub schema_refresh_interval: Option<SchemaRefreshInterval>,
}

/// Configuration for cluster by columns.
///
/// dbt-core allows either of the variants for the `cluster_by`
/// to allow cluster on a single column or on multiple columns
#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize, PartialEq, Eq, DbtSchema)]
#[serde(untagged)]
pub enum ClusterConfig {
    String(String),
    List(Vec<String>),
}

impl ClusterConfig {
    /// Normalize the enum as a list of cluster_by fields
    pub fn fields(&self) -> Vec<&str> {
        match self {
            ClusterConfig::String(s) => vec![s.as_ref()],
            ClusterConfig::List(l) => l.iter().map(|s| s.as_ref()).collect(),
        }
    }

    /// Normalize the enum as a list of cluster_by fields
    pub fn into_fields(self) -> Vec<String> {
        match self {
            ClusterConfig::String(s) => vec![s],
            ClusterConfig::List(l) => l,
        }
    }
}

/// Configuration for partition by columns.
///
/// dbt-core allows either of the variants for the `partition_by` in the model config
/// but the bigquery-adapter throws RunTime error
/// the behaviors are tested from the latest dbt-core + bigquery-adapter as this is written
/// we're conformant to this behavior via here and via the `into_bigquery()` method
#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize, PartialEq, Eq, DbtSchema)]
#[serde(untagged)]
pub enum PartitionConfig {
    String(String),
    List(Vec<String>),
    BigqueryPartitionConfig(BigqueryPartitionConfig),
}

impl PartitionConfig {
    pub fn into_bigquery(self) -> Option<BigqueryPartitionConfig> {
        match self {
            PartitionConfig::BigqueryPartitionConfig(bq) => Some(bq),
            _ => None,
        }
    }

    pub fn as_bigquery(&self) -> Option<&BigqueryPartitionConfig> {
        match self {
            PartitionConfig::BigqueryPartitionConfig(bq) => Some(bq),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::schemas::serde::minijinja_value_to_typed_struct;

    use super::*;
    use minijinja::value::Value as MinijinjaValue;
    use std::io::Cursor;

    // ---- Seed file hashing (configurable MAXIMUM_SEED_SIZE_MIB, dbt-core PR 13033) ----

    fn checksum_parts(checksum: &DbtChecksum) -> (&str, &str) {
        match checksum {
            DbtChecksum::Object(o) => (o.name.as_str(), o.checksum.as_str()),
            DbtChecksum::String(_) => panic!("expected object checksum"),
        }
    }

    #[test]
    fn interactive_table_materialization_roundtrip() {
        let m: DbtMaterialization = "interactive_table".parse().unwrap();
        assert_eq!(m, DbtMaterialization::InteractiveTable);
        assert_eq!(m.to_string(), "interactive_table");
        assert_eq!(RelationType::from(m), RelationType::InteractiveTable);
        assert_eq!(
            RelationType::InteractiveTable.to_string(),
            "interactive_table"
        );
    }

    #[test]
    fn interactive_table_maps_to_dedicated_node_materialization() {
        let node_materialization = NodeMaterialization::from(&DbtMaterialization::InteractiveTable);
        assert_eq!(node_materialization, NodeMaterialization::InteractiveTable);
        assert_ne!(node_materialization, NodeMaterialization::Custom);
    }

    #[test]
    fn test_include_exclude_deserializes_number_versions() {
        let config: IncludeExclude = dbt_yaml::from_str(
            r#"
include: [1, "2"]
exclude: 3
"#,
        )
        .unwrap();

        assert_eq!(
            config.include,
            Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                "1".to_string(),
                "2".to_string(),
            ]))
        );
        assert_eq!(
            config.exclude,
            Some(StringOrArrayOfStrings::String("3".to_string()))
        );
    }

    #[test]
    fn test_seed_content_checksum_normalizes_crlf() {
        // New text-mode hashing must treat CRLF and LF identically (Windows == Linux).
        let crlf =
            DbtChecksum::seed_content_checksum(Cursor::new(b"a,b\r\nc,d\r\n".to_vec())).unwrap();
        let lf = DbtChecksum::seed_content_checksum(Cursor::new(b"a,b\nc,d\n".to_vec())).unwrap();
        assert_eq!(crlf, lf);
    }

    #[test]
    fn test_seed_content_checksum_legacy_preserves_crlf() {
        // Legacy hashing preserves interior CRLF, so CRLF != LF (old Fusion behavior).
        let crlf = DbtChecksum::seed_content_checksum_legacy(b"a,b\r\nc,d\r\n");
        let lf = DbtChecksum::seed_content_checksum_legacy(b"a,b\nc,d\n");
        assert_ne!(crlf, lf);
    }

    #[test]
    fn test_seed_content_checksum_lf_matches_legacy() {
        // Backward-compat: on LF-only content (Linux/macOS) the new text-mode hash
        // must equal the legacy hash, so existing seed checksums stay stable.
        let content = b"id,name\n1,Ada\n2,Grace\n";
        let new = DbtChecksum::seed_content_checksum(Cursor::new(content.to_vec())).unwrap();
        let legacy = DbtChecksum::seed_content_checksum_legacy(content);
        assert_eq!(new, legacy);
    }

    #[test]
    fn test_seed_content_checksum_trims_outer_whitespace() {
        let padded =
            DbtChecksum::seed_content_checksum(Cursor::new(b"  \n a,b\nc,d \n  ".to_vec()))
                .unwrap();
        let bare = DbtChecksum::seed_content_checksum(Cursor::new(b"a,b\nc,d".to_vec())).unwrap();
        assert_eq!(padded, bare);
    }

    #[test]
    fn test_seed_content_checksum_chunk_boundary_crlf() {
        // A CRLF straddling a chunk boundary must still collapse to a single LF.
        let data = b"ab\r\ncd\r\nef".to_vec();
        let small =
            DbtChecksum::seed_content_checksum_chunked(Cursor::new(data.clone()), 4).unwrap();
        let big = DbtChecksum::seed_content_checksum_chunked(Cursor::new(data), 64).unwrap();
        let lf = DbtChecksum::seed_content_checksum(Cursor::new(b"ab\ncd\nef".to_vec())).unwrap();
        assert_eq!(small, big);
        assert_eq!(small, lf);
    }

    #[test]
    fn test_seed_content_checksum_incremental_large() {
        // Content larger than a chunk hashes identically regardless of chunk size.
        let mut data = Vec::new();
        for i in 0..5000 {
            data.extend_from_slice(format!("{i},row{i}\r\n").as_bytes());
        }
        let chunked =
            DbtChecksum::seed_content_checksum_chunked(Cursor::new(data.clone()), 7).unwrap();
        let whole = DbtChecksum::seed_content_checksum_chunked(Cursor::new(data), 1 << 20).unwrap();
        assert_eq!(chunked, whole);
    }

    fn write_temp_seed(bytes: &[u8]) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("dbt_seed_hash_{}_{}.csv", std::process::id(), n));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn test_seed_file_checksum_under_limit_hashes_content() {
        let path = write_temp_seed(b"id,name\n1,Ada\n");
        let cs = DbtChecksum::seed_file_checksum(&path, "seeds/x.csv", 1).unwrap();
        assert_eq!(checksum_parts(&cs).0, "sha256");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_seed_file_checksum_over_limit_uses_path() {
        // File larger than the 1 MiB limit -> path-based checksum.
        let big = vec![b'a'; 2 * 1024 * 1024];
        let path = write_temp_seed(&big);
        let cs = DbtChecksum::seed_file_checksum(&path, "seeds/x.csv", 1).unwrap();
        let (name, checksum) = checksum_parts(&cs);
        assert_eq!(name, "path");
        assert_eq!(checksum, "seeds/x.csv");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_seed_file_checksum_zero_disables_limit() {
        let path = write_temp_seed(b"id,name\n1,Ada\n2,Grace\n");
        // limit 0 means "no limit": content is hashed even though the file is large.
        let cs = DbtChecksum::seed_file_checksum(&path, "seeds/x.csv", 0).unwrap();
        assert_eq!(checksum_parts(&cs).0, "sha256");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_seed_file_checksum_matches_content_checksum() {
        let bytes = b"id,name\r\n1,Ada\r\n";
        let path = write_temp_seed(bytes);
        let file_cs = DbtChecksum::seed_file_checksum(&path, "seeds/x.csv", 0).unwrap();
        let reader_cs = DbtChecksum::seed_content_checksum(Cursor::new(bytes.to_vec())).unwrap();
        assert_eq!(file_cs, reader_cs);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_conform_normalized_snapshot_strips_spaced_snapshot_blocks() {
        // Regression: this fn runs *after* `normalize_sql`, which collapses every
        // run of whitespace into a single ASCII space. So even input that was
        // `{%snapshot foo%}` on disk becomes `{% snapshot foo %}` (with spaces)
        // by the time it reaches this fn. The previous implementation only
        // matched `{%snapshot` / `{%-snapshot` (no space) and silently no-op'd
        // on the spaced form, leaving the wrapper in the hashed input. That
        // produced a different checksum than Mantle's pre-stripped raw_code,
        // breaking `state:modified` parity for snapshots.
        //
        // dbt-core's recorded raw_code already has the wrapper removed, so
        // when Fusion recalculates checksums on both sides, the inputs should
        // converge once we correctly strip.

        // Standard form: `{% snapshot name %}` ... `{% endsnapshot %}`
        let normalized = "{% snapshot ip_location %} {{ config(...) }} body {% endsnapshot %}";
        let stripped = conform_normalized_snapshot_raw_code_to_mantle_format(normalized);
        assert!(
            !stripped.contains("snapshot ip_location"),
            "opening `{{% snapshot ip_location %}}` should be stripped, got: {stripped:?}"
        );
        assert!(
            !stripped.contains("endsnapshot"),
            "closing `{{% endsnapshot %}}` should be stripped, got: {stripped:?}"
        );
        assert!(
            stripped.contains("config"),
            "body should be preserved, got: {stripped:?}"
        );

        // Whitespace-control form: `{%- snapshot name -%}` ... `{%- endsnapshot -%}`
        let normalized_ws = "{%- snapshot foo -%} {{ x }} {%- endsnapshot -%}";
        let stripped_ws = conform_normalized_snapshot_raw_code_to_mantle_format(normalized_ws);
        assert!(
            !stripped_ws.contains("snapshot foo"),
            "opening `{{%- snapshot foo -%}}` should be stripped, got: {stripped_ws:?}"
        );
        assert!(
            !stripped_ws.contains("endsnapshot"),
            "closing `{{%- endsnapshot -%}}` should be stripped, got: {stripped_ws:?}"
        );

        // Already-stripped form (e.g. raw_code from Mantle's manifest): no-op.
        let already = "{{ config(...) }} body";
        assert_eq!(
            conform_normalized_snapshot_raw_code_to_mantle_format(already),
            already,
            "already-stripped input should be returned unchanged"
        );
    }

    #[test]
    fn test_conform_normalized_snapshot_dashed_inner_tag_does_not_leak_into_opening_strip() {
        // Regression for dbt-labs/dbt-core#15956 (FUSCSE-58): a plain outer
        // `{% snapshot %}` tag combined with any inner whitespace-controlled tag
        // caused the opening-tag boundary search to skip past this tag's own
        // nearby `%}` and match the inner tag's dashed close instead, silently
        // dropping real body content before hashing and falsely flagging
        // unchanged snapshots as `state:modified.body`.
        let trigger = "{% snapshot repro %} select {% for c in cols -%} {{ c }}{%- if not loop.last %},{%- endif -%} {%- endfor %} from t {% endsnapshot %}";
        let control = "{%- snapshot repro -%} select {% for c in cols -%} {{ c }}{%- if not loop.last %},{%- endif -%} {%- endfor %} from t {%- endsnapshot -%}";

        let stripped_trigger = conform_normalized_snapshot_raw_code_to_mantle_format(trigger);
        let stripped_control = conform_normalized_snapshot_raw_code_to_mantle_format(control);

        assert!(
            stripped_trigger.contains("select") && stripped_trigger.contains("from t"),
            "body content before/after the inner dashed tag must survive stripping, got: {stripped_trigger:?}"
        );
        assert_eq!(
            stripped_trigger, stripped_control,
            "plain and dashed outer tags must normalize to the same stripped body; \
             the previous bug made these diverge, causing a false state:modified.body"
        );
    }

    #[test]
    fn test_conform_normalized_snapshot_idempotent_with_normalize_sql() {
        // End-to-end: a typical on-disk snapshot file run through
        // `normalize_sql` then `conform_...` should produce the same string
        // as Mantle's pre-stripped raw_code run through the same pipeline.
        // This is what makes recalculated-checksum equality work for
        // `state:modified.body` parity.
        let on_disk = "{% snapshot ip_location %}\n\n    {{\n        config(\n            target_schema='utils'\n        )\n    }}\n    select 1 as id\n{% endsnapshot %}";
        let mantle_raw_code = "\n\n    {{\n        config(\n            target_schema='utils'\n        )\n    }}\n    select 1 as id\n";

        let from_disk =
            conform_normalized_snapshot_raw_code_to_mantle_format(&normalize_sql(on_disk));
        let from_manifest =
            conform_normalized_snapshot_raw_code_to_mantle_format(&normalize_sql(mantle_raw_code));
        // Byte-exact, not trim-equal: this output feeds directly into SHA256 for
        // the body checksum. Any boundary-whitespace divergence produces different
        // digests and breaks `state:modified.body` parity.
        assert_eq!(
            from_disk, from_manifest,
            "Conform pipeline must produce byte-equal output for on-disk and Mantle-stored raw_code"
        );
    }

    #[test]
    fn model_freshness_rules_eq_defaults_updates_on_to_any() {
        let base = ModelFreshnessRules {
            count: Some(1),
            period: Some(FreshnessPeriod::hour),
            updates_on: None,
        };
        let other = ModelFreshnessRules {
            count: Some(1),
            period: Some(FreshnessPeriod::hour),
            updates_on: Some(UpdatesOn::Any),
        };

        assert_eq!(base, other);
    }

    #[test]
    fn test_hooks_equal_array_of_strings_vs_hook_config_array() {
        let array_of_strings = Hooks::ArrayOfStrings(vec![
            "{{ dbt_snow_mask.apply_masking_policy('snapshots') }}".to_string(),
        ]);

        let hook_config_array = Hooks::HookConfigArray(vec![HookConfig {
            sql: Some("{{ dbt_snow_mask.apply_masking_policy('snapshots') }}".to_string()),
            transaction: Some(true),
            index: None,
        }]);

        // Test that they are equal
        assert!(array_of_strings.hooks_equal(&hook_config_array));
        assert!(hook_config_array.hooks_equal(&array_of_strings));
    }

    #[test]
    fn test_hooks_equal_string_vs_hook_config() {
        let hook_string =
            Hooks::String("{{ dbt_snow_mask.apply_masking_policy('snapshots') }}".to_string());

        let hook_config = Hooks::HookConfig(HookConfig {
            sql: Some("{{ dbt_snow_mask.apply_masking_policy('snapshots') }}".to_string()),
            transaction: Some(true),
            index: None,
        });

        // Test that they are equal
        assert!(hook_string.hooks_equal(&hook_config));
        assert!(hook_config.hooks_equal(&hook_string));
    }

    #[test]
    fn test_get_semantic_name_snowflake_simple() {
        helper(AdapterType::Snowflake, false, "xyz", false, "xyz");
    }

    #[test]
    fn test_get_semantic_name_snowflake_quoted_identifier() {
        helper(AdapterType::Snowflake, false, r#""xyz""#, true, "xyz");
    }

    #[test]
    fn test_get_semantic_name_snowflake_quoting() {
        helper(AdapterType::Snowflake, true, "xyz", true, "xyz");
    }
    #[test]
    fn test_get_semantic_name_snowflake_quoted_identifier_with_quoting() {
        helper(AdapterType::Snowflake, true, r#""xyz""#, true, r#""xyz""#);
    }

    #[test]
    fn test_get_semantic_name_snowflake_non_identifier() {
        // This will fail because `z.3` should be quoted, but this is a user error
        helper(AdapterType::Snowflake, false, "z.3", false, "z.3");
    }

    #[test]
    fn test_get_semantic_name_snowflake_reserved() {
        // This will fail because `group` is a reserved keyword, but this is a user error
        helper(AdapterType::Snowflake, false, "group", false, "group");
    }

    #[test]
    fn test_get_semantic_name_snowflake_quoted_reserved() {
        helper(AdapterType::Snowflake, false, r#""GROUP""#, true, "GROUP");
    }

    #[test]
    fn test_get_semantic_name_snowflake_quoted_reserved_with_quoting() {
        helper(
            AdapterType::Snowflake,
            true,
            r#""GROUP""#,
            true,
            r#""GROUP""#,
        );
    }

    #[test]
    fn test_get_semantic_name_postgres_simple() {
        helper(AdapterType::Postgres, false, "xyz", false, "xyz");
    }

    #[test]
    fn test_get_semantic_name_postgres_quoted_identifier() {
        helper(AdapterType::Postgres, false, r#""xyz""#, true, "xyz");
    }

    #[test]
    fn test_get_semantic_name_postgres_quoting() {
        helper(AdapterType::Postgres, true, "xyz", true, "xyz");
    }
    #[test]
    fn test_get_semantic_name_postgres_quoted_identifier_with_quoting() {
        helper(AdapterType::Postgres, true, r#""xyz""#, true, r#""xyz""#);
    }

    #[test]
    fn test_get_semantic_name_postgres_non_identifier() {
        // This will fail because `z.3` should be quoted, but this is a user error
        helper(AdapterType::Postgres, false, "z.3", false, "z.3");
    }

    #[test]
    fn test_get_semantic_name_postgres_reserved() {
        // This will fail because `group` is a reserved keyword, but this is a user error
        helper(AdapterType::Postgres, false, "group", false, "group");
    }

    #[test]
    fn test_get_semantic_name_postgres_quoted_reserved() {
        helper(AdapterType::Postgres, false, r#""GROUP""#, true, "GROUP");
    }

    #[test]
    fn test_get_semantic_name_postgres_quoted_reserved_with_quoting() {
        helper(
            AdapterType::Postgres,
            true,
            r#""GROUP""#,
            true,
            r#""GROUP""#,
        );
    }
    fn helper(
        adapter_type: AdapterType,
        quoting: bool,
        identifier: &str,
        expected_quoting: bool,
        expected_identifier: &str,
    ) {
        let result = normalize_quote(quoting, adapter_type, identifier);

        let (actual_identifier, actual_quoting) = result;
        assert_eq!(actual_identifier, expected_identifier);
        assert_eq!(actual_quoting, expected_quoting);
    }

    mod schema_refresh_interval_tests {
        use super::*;

        #[test]
        fn test_parsing() {
            // Duration variants
            assert_eq!(
                "30m".parse::<SchemaRefreshInterval>().unwrap().as_secs(),
                Some(30 * 60)
            );
            assert_eq!(
                "2h".parse::<SchemaRefreshInterval>().unwrap().as_secs(),
                Some(2 * 60 * 60)
            );
            assert_eq!(
                "1d".parse::<SchemaRefreshInterval>().unwrap().as_secs(),
                Some(24 * 60 * 60)
            );

            // Never variant (case insensitive)
            assert!("never".parse::<SchemaRefreshInterval>().unwrap().is_never());
            assert!("NEVER".parse::<SchemaRefreshInterval>().unwrap().is_never());
            assert!("Never".parse::<SchemaRefreshInterval>().unwrap().is_never());

            // Never returns None for duration
            let never: SchemaRefreshInterval = "never".parse().unwrap();
            assert_eq!(never.as_secs(), None);
            assert_eq!(never.as_duration(), None);
        }

        #[test]
        fn test_default_and_constructors() {
            // Default is 1 hour
            let default = SchemaRefreshInterval::default();
            assert_eq!(default.as_secs(), Some(60 * 60));
            assert!(!default.is_never());

            // from_hours constructor
            assert_eq!(
                SchemaRefreshInterval::from_hours(3).as_secs(),
                Some(3 * 60 * 60)
            );
        }

        #[test]
        fn test_yaml_roundtrip() {
            #[derive(Serialize, Deserialize, PartialEq, Debug)]
            struct Config {
                interval: SchemaRefreshInterval,
            }

            // Duration roundtrip
            let duration_config = Config {
                interval: "2h".parse().unwrap(),
            };
            let yaml = dbt_yaml::to_string(&duration_config).unwrap();
            assert_eq!(
                dbt_yaml::from_str::<Config>(&yaml).unwrap(),
                duration_config
            );

            // Never roundtrip
            let never_config = Config {
                interval: SchemaRefreshInterval::Never,
            };
            let yaml = dbt_yaml::to_string(&never_config).unwrap();
            assert_eq!(dbt_yaml::from_str::<Config>(&yaml).unwrap(), never_config);
        }

        #[test]
        fn test_conversions() {
            // Into Option<Duration>
            let duration: Option<Duration> = "1h".parse::<SchemaRefreshInterval>().unwrap().into();
            assert_eq!(duration.unwrap().as_secs(), 60 * 60);

            let none: Option<Duration> = SchemaRefreshInterval::Never.into();
            assert!(none.is_none());

            // Display
            assert_eq!(
                format!("{}", "30m".parse::<SchemaRefreshInterval>().unwrap()),
                "30m"
            );
            assert_eq!(format!("{}", SchemaRefreshInterval::Never), "never");
        }

        #[test]
        fn test_zero_ttl() {
            // 0s TTL means always refresh
            let zero_ttl = "0s".parse::<SchemaRefreshInterval>().unwrap();
            assert_eq!(zero_ttl.as_secs(), Some(0));
            assert!(!zero_ttl.is_never());

            // Verify 0s is different from "never"
            let never = SchemaRefreshInterval::Never;
            assert_ne!(zero_ttl, never);
        }

        #[test]
        fn test_validation_bounds() {
            // Test minimum duration validation (0s and above should succeed)
            assert!("-1s".parse::<SchemaRefreshInterval>().is_err());
            assert!("0s".parse::<SchemaRefreshInterval>().is_ok());
            assert!("30s".parse::<SchemaRefreshInterval>().is_ok());
            assert!("59s".parse::<SchemaRefreshInterval>().is_ok());

            // Test 1 minute (should succeed)
            assert!("1m".parse::<SchemaRefreshInterval>().is_ok());
            assert!("60s".parse::<SchemaRefreshInterval>().is_ok());

            // Test maximum duration validation (> 1 year should fail)
            assert!("366d".parse::<SchemaRefreshInterval>().is_err());
            assert!("2y".parse::<SchemaRefreshInterval>().is_err());

            // Test exactly 1 year (should succeed)
            assert!("365d".parse::<SchemaRefreshInterval>().is_ok());
            assert!("8760h".parse::<SchemaRefreshInterval>().is_ok());
        }

        #[test]
        fn test_error_messages() {
            // Test invalid format error
            let err = "invalid".parse::<SchemaRefreshInterval>().unwrap_err();
            assert!(err.contains("Invalid schema_refresh_interval"));
            assert!(err.contains("Expected format like '30m', '2h', '1d', or 'never'"));

            // Test negative duration error
            let err = "-1s".parse::<SchemaRefreshInterval>().unwrap_err();
            assert!(err.contains("Invalid schema_refresh_interval"));
            assert!(err.contains("Expected format"));

            // Test too long error
            let err = "2y".parse::<SchemaRefreshInterval>().unwrap_err();
            assert!(err.contains("too long"));
            assert!(err.contains("Maximum allowed is 1 year"));
        }
    }

    #[test]
    fn test_schedule_parses_string_format() {
        // Test parsing schedule as a plain string (the format that was failing before)
        let yaml = r#"schedule: "USING CRON 0,15,30,45 * * * * UTC""#;
        #[derive(Deserialize)]
        struct TestConfig {
            schedule: Schedule,
        }
        let config: TestConfig = dbt_yaml::from_str(yaml).unwrap();

        // Verify it parsed as Schedule::String
        assert!(matches!(config.schedule, Schedule::String(_)));

        // Verify to_schedule_config() works correctly
        let schedule_config = config.schedule.to_schedule_config();
        assert_eq!(
            schedule_config.cron,
            Some("USING CRON 0,15,30,45 * * * * UTC".to_string())
        );
        assert_eq!(schedule_config.time_zone_value, None);
    }

    #[test]
    fn test_schedule_parses_struct_format() {
        // Test parsing schedule as a structured config
        let yaml = r#"
schedule:
  cron: "0 */6 * * *"
  time_zone_value: "UTC"
"#;
        #[derive(Deserialize)]
        struct TestConfig {
            schedule: Schedule,
        }
        let config: TestConfig = dbt_yaml::from_str(yaml).unwrap();

        // Verify it parsed as Schedule::ScheduleConfig
        assert!(matches!(config.schedule, Schedule::ScheduleConfig(_)));

        // Verify to_schedule_config() works correctly
        let schedule_config = config.schedule.to_schedule_config();
        assert_eq!(schedule_config.cron, Some("0 */6 * * *".to_string()));
        assert_eq!(schedule_config.time_zone_value, Some("UTC".to_string()));
    }

    #[test]
    fn test_schedule_in_model_config_string_format() {
        // Test the exact YAML format from the bug report:
        // models:
        //   - name: decrypt_drivers_license_number_task
        //     config:
        //       schedule: "USING CRON 0,15,30,45 * * * * UTC"
        let yaml = r#"
models:
  - name: some_scheduled_task
    config:
      schedule: "USING CRON 0,15,30,45 * * * * UTC"
"#;
        #[derive(Deserialize)]
        struct ModelsFile {
            models: Vec<ModelEntry>,
        }
        #[derive(Deserialize)]
        struct ModelEntry {
            name: String,
            config: ModelConfigInner,
        }
        #[derive(Deserialize)]
        struct ModelConfigInner {
            schedule: Schedule,
        }

        let parsed: ModelsFile = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].name, "some_scheduled_task");

        let schedule_config = parsed.models[0].config.schedule.to_schedule_config();
        assert_eq!(
            schedule_config.cron,
            Some("USING CRON 0,15,30,45 * * * * UTC".to_string())
        );
        assert_eq!(schedule_config.time_zone_value, None);
    }

    #[test]
    fn test_freshness_rules_count_as_number() {
        let yaml = r#"
count: 24
period: hour
"#;
        let rules: FreshnessRules = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(rules.count, Some(24));
        assert_eq!(rules.period, Some(FreshnessPeriod::hour));
    }

    #[test]
    fn test_freshness_rules_count_as_string() {
        let yaml = r#"
count: "24"
period: hour
"#;
        let rules: FreshnessRules = dbt_yaml::from_str(yaml).unwrap();
        assert_eq!(rules.count, Some(24));
        assert_eq!(rules.period, Some(FreshnessPeriod::hour));
    }

    // MVT for F1+F2: pin the validator's truth table for the three failure modes
    // reported across multiple projects. See `.agents/...` analysis: a partial
    // freshness rule must still be rejected (negative regression guards), while
    // a fully-empty rule must be accepted (`is_empty()` short-circuit).
    #[test]
    fn test_freshness_rules_validate_empty_ok() {
        let rule = FreshnessRules {
            count: None,
            period: None,
        };
        assert!(rule.is_empty());
        FreshnessRules::validate(Some(&rule))
            .expect("an empty freshness rule must validate as OK (variant 3)");
    }

    #[test]
    fn test_freshness_rules_validate_count_only_err() {
        let rule = FreshnessRules {
            count: Some(25),
            period: None,
        };
        let err = FreshnessRules::validate(Some(&rule))
            .expect_err("a count-only freshness rule must still be rejected (variant 1)");
        let msg = err.to_string();
        assert!(
            msg.contains("count and period are required when freshness is provided"),
            "unexpected error message: {msg}"
        );
        assert!(
            msg.contains("count: Some(25)") && msg.contains("period: None"),
            "expected diagnostic to echo the offending values; got: {msg}"
        );
    }

    #[test]
    fn test_freshness_rules_validate_period_only_err() {
        let rule = FreshnessRules {
            count: None,
            period: Some(FreshnessPeriod::hour),
        };
        let err = FreshnessRules::validate(Some(&rule))
            .expect_err("a period-only freshness rule must still be rejected (variant 2)");
        let msg = err.to_string();
        assert!(
            msg.contains("count and period are required when freshness is provided"),
            "unexpected error message: {msg}"
        );
        assert!(
            msg.contains("count: None") && msg.contains("period: Some(hour)"),
            "expected diagnostic to echo the offending values; got: {msg}"
        );
    }

    #[test]
    fn test_merge_classifiers_unions_and_deduplicates() {
        let base = Some(vec!["finance".to_string(), "pii".to_string()]);
        let update = Some(vec!["pii".to_string(), "gdpr".to_string()]);
        let result = merge_vec(base, update);
        assert_eq!(
            result,
            Some(vec![
                "finance".to_string(),
                "gdpr".to_string(),
                "pii".to_string(),
            ])
        );
    }

    #[test]
    fn test_merge_classifiers_none_child_inherits_parent() {
        let base = Some(vec!["pii".to_string()]);
        let result = merge_vec(base, None);
        assert_eq!(result, Some(vec!["pii".to_string()]));
    }

    #[test]
    fn test_merge_classifiers_none_parent_uses_child() {
        let update = Some(vec!["finance".to_string()]);
        let result = merge_vec(None, update);
        assert_eq!(result, Some(vec!["finance".to_string()]));
    }

    #[test]
    fn test_merge_classifiers_both_none_is_none() {
        assert_eq!(merge_vec(None, None), None);
    }

    #[test]
    fn test_merge_tags_parent_first_preserves_inheritance_order() {
        // Regression for dbt-labs/dbt-core#15590: nested +tags must keep
        // parent-then-child order (not alphabetical). Alphabetical would put
        // DAILY before INTERMEDIATE.
        let parent = Some(vec!["INTERMEDIATE".to_string()]);
        let child = Some(vec!["DAILY".to_string()]);
        assert_eq!(
            merge_tags(parent, child),
            Some(vec!["INTERMEDIATE".to_string(), "DAILY".to_string()])
        );
    }

    #[test]
    fn test_merge_tags_dedupes_preserving_first_seen() {
        let parent = Some(vec!["a".to_string(), "b".to_string()]);
        let child = Some(vec!["b".to_string(), "c".to_string()]);
        assert_eq!(
            merge_tags(parent, child),
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn test_merge_tags_matches_docs_additive_hierarchy() {
        // Docs example: contains_pii (project) + hourly (folder) + finance (model)
        // must stay inheritance order, not alpha (contains_pii, finance, hourly).
        let project = Some(vec!["contains_pii".to_string()]);
        let folder = Some(vec!["hourly".to_string()]);
        let model = Some(vec!["finance".to_string()]);
        let after_folder = merge_tags(project, folder);
        assert_eq!(
            merge_tags(after_folder, model),
            Some(vec![
                "contains_pii".to_string(),
                "hourly".to_string(),
                "finance".to_string(),
            ])
        );
    }

    #[test]
    fn test_bigquery_partition_config_legacy_deserialize_from_jinja_values() {
        // Test String variant
        let string_value = MinijinjaValue::from("partition_field");
        let result = minijinja_value_to_typed_struct::<PartitionConfig>(string_value).unwrap();
        assert!(matches!(result, PartitionConfig::String(s) if s == "partition_field"));

        // Test List variant
        let list_value = MinijinjaValue::from(vec!["field1", "field2"]);
        let result = minijinja_value_to_typed_struct::<PartitionConfig>(list_value).unwrap();
        assert!(
            matches!(result, PartitionConfig::List(ref list) if list == &vec!["field1".to_string(), "field2".to_string()])
        );

        // Test BigqueryPartitionConfig variant with time partitioning
        let config_json: YmlValue = dbt_yaml::from_str(
            r#"
            field: "partition_date"
            data_type: "date"
            granularity: "day"
            time_ingestion_partitioning: true
        "#,
        )
        .unwrap();
        let config_value = MinijinjaValue::from_serialize(&config_json);
        let result = minijinja_value_to_typed_struct::<PartitionConfig>(config_value).unwrap();
        if let PartitionConfig::BigqueryPartitionConfig(config) = result {
            assert_eq!(config.field, "partition_date");
            assert_eq!(config.data_type, "date");
            assert!(config.time_ingestion_partitioning());
            assert_eq!(config.granularity().unwrap(), "day");
        } else {
            panic!("Expected BigqueryPartitionConfig variant");
        }
    }

    #[test]
    fn test_constraint_to_span_ref_captures_line() {
        let yaml = "type: foreign_key\nto: ref('orders')\nto_columns: [id]\n";
        let constraint: Constraint = dbt_yaml::from_str(yaml).unwrap();
        let spanned = constraint.to.as_ref().expect("to should be Some");
        assert_eq!(spanned.as_str(), "ref('orders')");
        assert!(spanned.span().is_valid(), "span should be valid");
        assert_eq!(spanned.span().start.line, 2, "to: should be on line 2");
    }

    #[test]
    fn test_constraint_to_span_source_captures_line() {
        let yaml = "type: foreign_key\nto: source('raw', 'orders')\nto_columns: [id]\n";
        let constraint: Constraint = dbt_yaml::from_str(yaml).unwrap();
        let spanned = constraint.to.as_ref().expect("to should be Some");
        assert_eq!(spanned.as_str(), "source('raw', 'orders')");
        assert!(spanned.span().is_valid(), "span should be valid");
        assert_eq!(spanned.span().start.line, 2, "to: should be on line 2");
    }

    #[test]
    fn test_dbt_checksum_default_matches_dbt_core_file_hash_empty() {
        // dbt-core's FileHash.empty() serializes as {"name": "none", "checksum": ""}.
        // Generic tests have no source file and use DbtChecksum::default() for their
        // checksum. If default() emits name:"" instead of name:"none", every generic
        // test appears as state:modified against a Mantle-recorded state manifest,
        // causing all-test over-selection and Replay Data Missing errors in conformance.
        let default = DbtChecksum::default();
        let json = serde_json::to_value(&default).expect("serializes");
        assert_eq!(
            json["name"], "none",
            "DbtChecksum::default() must serialize name as \"none\" to match dbt-core FileHash.empty()"
        );
        assert_eq!(json["checksum"], "", "checksum must be empty string");

        // Must be equal to an explicitly constructed {name:"none", checksum:""} — the
        // form that appears in Mantle-produced state manifests.
        let mantle_form = DbtChecksum::Object(DbtChecksumObject {
            name: "none".to_string(),
            checksum: "".to_string(),
        });
        assert_eq!(
            default, mantle_form,
            "DbtChecksum::default() must equal the Mantle state-manifest form {{name:\"none\",checksum:\"\"}}"
        );
    }

    // Regression (dbt-labs/fs#13334): `columns:` inside a version block must deserialize into the
    // typed `Versions::columns` field, NOT into the `Verbatim` `__additional_properties__` bag.
    // Only the typed field is walked by `into_typed_with_jinja`, which is what renders
    // `description: '{{ doc(...) }}'` on a version column.
    #[test]
    fn test_versions_columns_deserialize_into_typed_field() {
        let yaml = "v: 2\ncolumns:\n  - name: id\n    description: primary key\n  - include: all\n    exclude:\n      - dropped\n";
        let value: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        // Use into_typed (the same path as into_typed_with_jinja) so dbt_yaml's
        // dunder-flatten mechanism is active.
        let versions: Versions = value
            .into_typed::<Versions, _, _>(
                |_, _, _| {},
                |_| -> Result<
                    Option<dbt_yaml::Value>,
                    Box<dyn std::error::Error + 'static + Send + Sync>,
                > { Ok(None) },
            )
            .unwrap();

        assert!(
            !versions.__additional_properties__.contains_key("columns"),
            "`columns` must not fall through to the Verbatim __additional_properties__ catch-all; \
             values there are never Jinja-rendered (fs#13334)"
        );

        let columns = versions
            .columns
            .as_deref()
            .expect("`columns` must populate the typed Versions::columns field");
        assert_eq!(columns.len(), 2, "both entries must be preserved");
        assert_eq!(columns[0].name.as_deref(), Some("id"));
        assert_eq!(columns[0].description.as_deref(), Some("primary key"));
        // The include/exclude directive entry has no `name`.
        assert!(columns[1].name.is_none());
        assert_eq!(
            columns[1].exclude.as_deref(),
            Some(["dropped".to_string()].as_slice())
        );
    }

    // Regression: the generated JSON Schema for Versions must expose `columns` as a named
    // array property so YAML language servers do not reject `columns: [...]` with
    // "Incorrect type. Expected 'object'".
    #[test]
    fn test_versions_schema_has_columns_as_array_property() {
        use crate::man::deny_additional_properties_in_root;
        use schemars::r#gen::SchemaSettings;

        let generator = SchemaSettings::draft07().into_generator();
        let mut root =
            generator.into_root_schema_for::<crate::schemas::properties::DbtPropertiesFile>();
        deny_additional_properties_in_root(&mut root);

        let schema_json = serde_json::to_value(&root).unwrap();
        let versions_def = &schema_json["definitions"]["Versions"];
        let columns_schema = &versions_def["properties"]["columns"];

        assert!(
            !columns_schema.is_null(),
            "`columns` must be a named property in the Versions schema definition"
        );

        let type_field = &columns_schema["type"];
        let is_array = type_field == "array"
            || type_field
                .as_array()
                .map(|arr| arr.iter().any(|v| v == "array"))
                .unwrap_or(false);
        assert!(
            is_array,
            "`columns` property must have type 'array' (got {type_field})"
        );
    }

    // ---- deprecation_date normalization (dbt-core#14563) ----

    #[test]
    fn test_normalize_deprecation_date_bare_date() {
        assert_eq!(
            normalize_deprecation_date("2025-10-31"),
            "2025-10-31T00:00:00+00:00"
        );
    }

    #[test]
    fn test_normalize_deprecation_date_naive_t_separator() {
        assert_eq!(
            normalize_deprecation_date("2025-10-31T00:00:00"),
            "2025-10-31T00:00:00+00:00"
        );
    }

    #[test]
    fn test_normalize_deprecation_date_naive_space_separator() {
        assert_eq!(
            normalize_deprecation_date("2025-10-31 00:00:00"),
            "2025-10-31T00:00:00+00:00"
        );
    }

    #[test]
    fn test_normalize_deprecation_date_z_suffix() {
        assert_eq!(
            normalize_deprecation_date("2025-10-31T00:00:00Z"),
            "2025-10-31T00:00:00+00:00"
        );
    }

    #[test]
    fn test_normalize_deprecation_date_with_offset_preserved() {
        assert_eq!(
            normalize_deprecation_date("2025-10-31T00:00:00-05:00"),
            "2025-10-31T00:00:00-05:00"
        );
    }

    #[test]
    fn test_normalize_deprecation_date_space_separator_with_offset_and_fraction() {
        // Exact example from the docs page for `deprecation_date`.
        assert_eq!(
            normalize_deprecation_date("1999-01-01 00:00:00.00+00:00"),
            "1999-01-01T00:00:00+00:00"
        );
    }

    #[test]
    fn test_normalize_deprecation_date_unparseable_passes_through() {
        assert_eq!(normalize_deprecation_date("not-a-date"), "not-a-date");
    }

    /// PyYAML (and so dbt-core) resolves the YAML 1.1 boolean tokens that Fusion's YAML 1.2
    /// reader hands to serde as strings. `yes` must resolve to `true`, and a token outside the
    /// set must still error rather than silently become `false`.
    #[test]
    fn test_dbt_contract_resolves_yaml_11_boolean_tokens() {
        for field in ["enforced", "alias_types"] {
            for (token, expected) in [
                ("no", false),
                ("No", false),
                ("off", false),
                ("false", false),
                ("yes", true),
                ("on", true),
                ("true", true),
            ] {
                let contract: DbtContract =
                    dbt_yaml::from_str(&format!("{field}: {token}\n")).unwrap();
                let resolved = match field {
                    "enforced" => contract.enforced,
                    _ => contract.alias_types,
                };
                assert_eq!(resolved, expected, "{field}: {token}");
            }

            for token in ["maybe", "1"] {
                let result: Result<DbtContract, _> =
                    dbt_yaml::from_str(&format!("{field}: {token}\n"));
                assert!(result.is_err(), "{field}: {token}");
            }
        }
    }

    /// `docs: { show: no }` is the same YAML 1.1 boolean divergence as `contract.enforced`;
    /// `show` defaults to `true`, so a silently wrong value would be invisible.
    #[test]
    fn test_docs_config_show_resolves_yaml_11_boolean_tokens() {
        for (token, expected) in [("no", false), ("off", false), ("yes", true), ("on", true)] {
            let docs: DocsConfig = dbt_yaml::from_str(&format!("show: {token}\n")).unwrap();
            assert_eq!(docs.show, expected, "token: {token}");
        }

        let result: Result<DocsConfig, _> = dbt_yaml::from_str("show: maybe\n");
        assert!(result.is_err());
    }
}
