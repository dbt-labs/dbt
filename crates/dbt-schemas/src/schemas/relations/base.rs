//! Reference: dbt-adapters/src/dbt/adapters/contracts/relation.py
use crate::dbt_types::RelationType;
use crate::filter::RunFilter;
use crate::schemas::common::ResolvedQuoting;
pub use crate::schemas::dbt_catalogs_v2::TableFormat;

use chrono::format::SecondsFormat;
use dbt_adapter_core::{AdapterType, quote_char};
use dbt_common::FsResult;
use dbt_common::constants::DBT_CTE_PREFIX;
use dbt_schema_store::CanonicalFqn;
use minijinja::{Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, Value};
use minijinja::{invalid_argument, invalid_argument_inner, jinja_err};
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use core::fmt;
use std::any::Any;
use std::collections::BTreeMap;
use std::option::Option;
use std::sync::Arc;

/// A pattern to match relations
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RelationPattern {
    /// The database
    pub database: String,
    /// The schema pattern to match %, _ etc
    pub schema_pattern: String,
    /// The table pattern to match %, _ etc
    pub table_pattern: String,
}

impl RelationPattern {
    pub fn new(database: String, schema_pattern: String, table_pattern: String) -> Self {
        Self {
            database,
            schema_pattern,
            table_pattern,
        }
    }
}

/// dbt-adapters/src/dbt/adapters/contracts/relation.py
pub type Policy = ResolvedQuoting;

impl Policy {
    pub fn new(database: bool, schema: bool, identifier: bool) -> Self {
        Self {
            database,
            schema,
            identifier,
        }
    }

    pub fn disabled() -> Self {
        Self {
            database: false,
            schema: false,
            identifier: false,
        }
    }

    pub fn enabled() -> Self {
        Self {
            database: true,
            schema: true,
            identifier: true,
        }
    }
}

impl Policy {
    pub fn get_part(&self, component: &ComponentName) -> bool {
        match component {
            ComponentName::Database => self.database,
            ComponentName::Schema => self.schema,
            ComponentName::Identifier => self.identifier,
        }
    }
}

/// dbt-adapters/src/dbt/adapters/contracts/relation.py
#[derive(Debug, EnumString, Display)]
#[strum(serialize_all = "lowercase")]
pub enum ComponentName {
    Database,
    Schema,
    Identifier,
}

/// A struct representing the path of a relation
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct RelationPath {
    /// The database name
    pub database: Option<String>,
    /// The schema name
    pub schema: Option<String>,
    /// The identifier name
    pub identifier: Option<String>,
}

pub trait BaseRelationProperties {
    fn is_database_relation(&self) -> bool {
        true
    }

    fn include_policy(&self) -> Policy;

    fn quote_policy(&self) -> Policy;

    fn get_database(&self) -> FsResult<String>;

    fn get_schema(&self) -> FsResult<String>;

    fn get_identifier(&self) -> FsResult<String>;

    fn get_canonical_fqn(&self) -> FsResult<CanonicalFqn>;

    fn get_location(&self) -> FsResult<Option<String>> {
        Ok(None)
    }
}

/// Base trait for all fs adapter objects
pub trait BaseRelation: BaseRelationProperties + Any + Send + Sync + fmt::Debug {
    /// Whether the relation is a system table or not
    fn is_system(&self) -> bool {
        false
    }

    /// Whether the relation has catalog metadata.
    /// Used by Databricks `needs_information` to avoid redundant DESCRIBE EXTENDED calls.
    /// Default true: relations without this concept are always considered to have information.
    fn has_information(&self) -> bool {
        true
    }

    /// as_any
    fn as_any(&self) -> &dyn Any;

    /// A helper for situation where only a [&dyn BaseRelation] is available
    fn to_owned(&self) -> Arc<dyn BaseRelation>;

    /// Create a new relation from the given state and arguments
    fn create_from(&self) -> Result<Arc<dyn BaseRelation>, MinijinjaError>;

    /// Get the database name
    fn database(&self) -> Option<&str>;

    /// Database as string or error
    fn database_as_str(&self) -> Result<String, MinijinjaError> {
        Ok(self.database().unwrap_or_default().to_string())
    }

    /// Get the database name as a string literal
    /// the same as how a database provider resolves and stores a database component for a relation
    /// given how it's quoted
    fn database_as_resolved_str(&self) -> Result<String, MinijinjaError> {
        match self.database() {
            Some(val) => {
                if !self.quote_policy().database {
                    Ok(self.normalize_component(val))
                } else {
                    Ok(val.to_string())
                }
            }
            None => jinja_err!(
                MinijinjaErrorKind::InvalidOperation,
                "expect database as string"
            ),
        }
    }

    fn database_as_quoted_str(&self) -> Result<String, MinijinjaError> {
        match self.database() {
            Some(val) => Ok(self.quoted(val)),
            None => jinja_err!(
                MinijinjaErrorKind::InvalidOperation,
                "expect database as string"
            ),
        }
    }

    /// Get the schema name
    fn schema(&self) -> Option<&str>;

    /// Schema as string or error
    fn schema_as_str(&self) -> Result<String, MinijinjaError> {
        match self.schema() {
            Some(val) => Ok(val.to_string()),
            None => jinja_err!(
                MinijinjaErrorKind::InvalidOperation,
                "expect schema as string"
            ),
        }
    }

    fn schema_as_quoted_str(&self) -> Result<String, MinijinjaError> {
        match self.schema() {
            Some(val) => Ok(self.quoted(val)),
            None => jinja_err!(
                MinijinjaErrorKind::InvalidOperation,
                "expect schema as string"
            ),
        }
    }

    /// Get the schema name as a string literal
    /// the same as how a database provider resolves and stores a schema component for a relation
    /// given how it's quoted
    fn schema_as_resolved_str(&self) -> Result<String, MinijinjaError> {
        match self.schema() {
            Some(val) => {
                if !self.quote_policy().schema {
                    Ok(self.normalize_component(val))
                } else {
                    Ok(val.to_string())
                }
            }
            None => jinja_err!(
                MinijinjaErrorKind::InvalidOperation,
                "expect schema as string"
            ),
        }
    }

    /// Get the identifier
    fn identifier(&self) -> Option<&str>;

    /// Identifiers as string or error
    fn identifier_as_str(&self) -> Result<String, MinijinjaError> {
        match self.identifier() {
            Some(val) => Ok(val.to_string()),
            None => jinja_err!(
                MinijinjaErrorKind::InvalidOperation,
                "expect identifier as string"
            ),
        }
    }

    /// Get the identifier as a string literal
    /// the same as how a database provider resolves and stores an identifier component for a relation
    /// given how it's quoted
    fn identifier_as_resolved_str(&self) -> Result<String, MinijinjaError> {
        match self.identifier() {
            Some(val) => {
                if !self.quote_policy().identifier {
                    Ok(self.normalize_component(val))
                } else {
                    Ok(val.to_string())
                }
            }
            None => jinja_err!(
                MinijinjaErrorKind::InvalidOperation,
                "expect identifier as string"
            ),
        }
    }

    fn location(&self) -> Option<&str> {
        None
    }

    fn location_as_str(&self) -> Result<Option<String>, MinijinjaError> {
        Ok(self.location().map(|s| s.to_string()))
    }

    fn location_as_resolved_str(&self) -> Result<Option<String>, MinijinjaError> {
        match self.location() {
            Some(val) => {
                if !self.quote_policy().database {
                    Ok(Some(self.normalize_component(val)))
                } else {
                    Ok(Some(val.to_string()))
                }
            }
            None => Ok(None),
        }
    }

    /// Return the relation type if available, defaulting to None.
    fn relation_type(&self) -> Option<RelationType> {
        None
    }

    /// Get adapter type
    fn adapter_type(&self) -> AdapterType;

    /// Helper: check if the relation is a table
    fn is_table(&self) -> bool {
        matches!(self.relation_type(), Some(RelationType::Table))
    }

    /// Helper: check if the relation is a delta table
    fn is_delta(&self) -> bool {
        false
    }

    fn set_is_delta(&mut self, is_delta: Option<bool>);

    /// Helper: check if the relation is a Databricks shallow clone
    fn is_shallow_clone(&self) -> bool {
        false
    }

    fn set_is_shallow_clone(&mut self, is_shallow_clone: Option<bool>);

    /// Set the relation's table format, when the adapter tracks one.
    fn set_table_format(&mut self, table_format: Option<TableFormat>);

    /// Helper: check if the relation is a CTE
    fn is_cte(&self) -> bool {
        matches!(
            self.relation_type(),
            Some(RelationType::CTE) | Some(RelationType::Ephemeral)
        )
    }

    /// Helper: check if the relation is a view
    fn is_view(&self) -> bool {
        matches!(self.relation_type(), Some(RelationType::View))
    }

    /// Helper: check if the relation is a materialized view
    fn is_materialized_view(&self) -> bool {
        matches!(self.relation_type(), Some(RelationType::MaterializedView))
    }

    /// Helper: check if the relation is a streaming table
    fn is_streaming_table(&self) -> bool {
        matches!(self.relation_type(), Some(RelationType::StreamingTable))
    }

    /// Helper: check if the relation is a dynamic table
    fn is_dynamic_table(&self) -> bool {
        matches!(self.relation_type(), Some(RelationType::DynamicTable))
    }

    /// Helper: check if the relation is an interactive table
    fn is_interactive_table(&self) -> bool {
        matches!(self.relation_type(), Some(RelationType::InteractiveTable))
    }

    /// Helper: check if the relation is a metric view
    fn is_metric_view(&self) -> bool {
        matches!(self.relation_type(), Some(RelationType::MetricView))
    }

    /// Helper: check if the relation is for a pointer table
    fn is_pointer(&self) -> bool {
        matches!(self.relation_type(), Some(RelationType::PointerTable))
    }

    /// Helper: is this relation renamable?
    fn can_be_renamed(&self) -> bool {
        matches!(
            self.relation_type(),
            Some(RelationType::Table) | Some(RelationType::View)
        )
    }

    /// Helper: is this relation replaceable?
    fn can_be_replaced(&self) -> bool {
        matches!(
            self.relation_type(),
            Some(RelationType::Table) | Some(RelationType::View)
        )
    }

    /// Get a metadata field from the relation
    /// If key is "metadata", returns a map with type information
    /// Otherwise simulate the behavior of a python dataclass
    fn get(&self, key: &str, default: Option<Value>) -> Result<Value, MinijinjaError> {
        if key == "metadata" {
            let mut map = BTreeMap::new();
            map.insert("type", Value::from(std::any::type_name::<Self>()));
            Ok(Value::from(map))
        } else {
            match key {
                "database" => Ok(Value::from(self.database())),
                "schema" => Ok(Value::from(self.schema())),
                "identifier" => Ok(Value::from(self.identifier())),
                _ => Ok(default.unwrap_or(Value::UNDEFINED)),
            }
        }
    }

    /// Replace path
    fn replace_path(
        &self,
        database: Option<String>,
        schema: Option<String>,
        identifier: Option<String>,
    ) -> Result<Arc<dyn BaseRelation>, MinijinjaError> {
        self.create_relation(
            Some(database.unwrap_or_else(|| self.database().unwrap().to_string())),
            Some(schema.unwrap_or_else(|| self.schema().unwrap().to_string())),
            Some(identifier.unwrap_or_else(|| self.identifier().unwrap().to_string())),
            self.relation_type(),
            self.quote_policy(),
        )
    }

    /// quoting character to be used when rendering the relation
    fn quote_character(&self) -> char {
        quote_char(self.adapter_type())
    }

    /// Quote a relation component (database, schema, or identifier)
    fn quoted(&self, s: &str) -> String {
        format!("{}{}{}", self.quote_character(), s, self.quote_character())
    }

    /// Get the semantic name fully qualified for a Relation
    ///
    /// A semantic name is meant to uniquely identify a relation
    /// agnostic of the literal values initially set for a Relation's component
    ///
    /// Implement [BaseRelation::normalize_relation_component] to complete the functionality
    /// ```
    fn semantic_fqn(&self) -> String {
        let mut parts = vec![];

        if let Ok(database) = self.database_as_str() {
            if !database.is_empty() {
                if self.quote_policy().database {
                    parts.push(self.quoted(&database));
                } else {
                    parts.push(self.quoted(&self.normalize_component(&database)));
                }
            }
        }

        if let Ok(schema) = self.schema_as_str() {
            if self.quote_policy().schema {
                parts.push(self.quoted(&schema));
            } else {
                parts.push(self.quoted(&self.normalize_component(&schema)));
            }
        }

        if let Ok(identifier) = self.identifier_as_str() {
            if self.quote_policy().identifier {
                parts.push(self.quoted(&identifier));
            } else {
                parts.push(self.quoted(&self.normalize_component(&identifier)));
            }
        }

        parts.join(".")
    }

    /// Helper for
    ///
    /// * [BaseRelation::semantic_fqn]
    /// * [BaseRelation::schema_as_resolved_str]
    /// * [BaseRelation::identifier_as_resolved_str]
    /// * [BaseRelation::database_as_resolved_str]
    ///
    /// This is how a specific database provider resolve and store an object's name if quoting is not used
    /// For example, they'll be upper case in Snowflake https://docs.snowflake.com/en/sql-reference/identifiers-syntax#unquoted-identifiers
    fn normalize_component(&self, component: &str) -> String;

    /// Render this relation as a string.
    fn render_self_as_str(&self) -> String {
        if let Some(RelationType::Ephemeral) = self.relation_type() {
            return format!(
                "{}{}",
                DBT_CTE_PREFIX,
                self.identifier().unwrap_or_default()
            );
        }

        let include_policy = self.include_policy();
        let quote_policy = self.quote_policy();
        let mut parts: Vec<String> = Vec::new();

        let quote_part = |val: &str, quote_policy: bool| {
            if quote_policy {
                self.quoted(val)
            } else {
                val.to_string()
            }
        };

        if include_policy.database
            && let Some(database) = self.database()
            && !database.is_empty()
        {
            parts.push(quote_part(database, quote_policy.database));
        }

        if include_policy.schema
            && let Some(schema) = self.schema()
        {
            parts.push(quote_part(schema, quote_policy.schema));
        }

        if include_policy.identifier
            && let Some(identifier) = self.identifier()
        {
            parts.push(quote_part(identifier, quote_policy.identifier));
        }

        let rendered = parts.join(".");

        if matches!(self.adapter_type(), AdapterType::Databricks) {
            rendered.to_ascii_lowercase()
        } else {
            rendered
        }
    }

    /// Render this relation with a run filter.
    fn render_with_run_filter(
        &self,
        run_filter: &RunFilter,
        event_time: &Option<String>,
    ) -> String {
        let rendered = self.render_self_as_str();

        let rendered = if run_filter.empty {
            format!("(select * from {rendered} where false limit 0)")
        } else {
            rendered
        };

        // no event_time? no filter
        let Some(event_time) = event_time.as_deref() else {
            return rendered;
        };

        // get start/end times
        let (start, end) = run_filter.sample_times();

        // Render with explicit UTC offset so non-UTC sessions (e.g. Snowflake
        // with a session TIMEZONE other than UTC) interpret the literal as UTC,
        // matching the microbatch DELETE predicate which also uses `to_rfc3339`.
        let (start, end) = match self.adapter_type() {
            AdapterType::Bigquery => (
                start.map(|t| t.to_rfc3339_opts(SecondsFormat::Micros, true)),
                end.map(|t| t.to_rfc3339_opts(SecondsFormat::Micros, true)),
            ),
            _ => (start.map(|t| t.to_rfc3339()), end.map(|t| t.to_rfc3339())),
        };

        // render the filter conditions
        let (start, end) = match self.adapter_type() {
            // See: https://github.com/dbt-labs/dbt-adapters/blob/221923bf60efc6a099681a82be89e86bef587f55/dbt-snowflake/src/dbt/adapters/snowflake/relation.py#L201
            AdapterType::Snowflake => (
                start.map(|start| format!("{event_time} >= to_timestamp_tz('{start}')")),
                end.map(|end| format!("{event_time} < to_timestamp_tz('{end}')")),
            ),

            // See: https://github.com/dbt-labs/dbt-adapters/blob/221923bf60efc6a099681a82be89e86bef587f55/dbt-bigquery/src/dbt/adapters/bigquery/relation.py#L124
            AdapterType::Bigquery => (
                start.map(|start| format!("cast({event_time} as timestamp) >= '{start}'")),
                end.map(|end| format!("cast({event_time} as timestamp) < '{end}'")),
            ),

            AdapterType::Postgres
            | AdapterType::Databricks
            | AdapterType::Redshift
            | AdapterType::Salesforce
            | AdapterType::Spark
            | AdapterType::DuckDB
            | AdapterType::LakeCompute
            | AdapterType::GizmoSQL
            | AdapterType::Fabric => (
                start.map(|start| format!("{event_time} >= '{start}'")),
                end.map(|end| format!("{event_time} < '{end}'")),
            ),
            // ClickHouse: parseDateTime64BestEffort preserves sub-second boundaries
            // for DateTime64 event-time columns.
            AdapterType::ClickHouse => (
                start.map(|start| {
                    format!("{event_time} >= parseDateTime64BestEffort('{start}', 9)")
                }),
                end.map(|end| format!("{event_time} < parseDateTime64BestEffort('{end}', 9)")),
            ),
            // Exasol TIMESTAMP literals take no time-zone offset; strip the
            // (always +00:00) offset and 'T' from the UTC rfc3339 boundary.
            AdapterType::Exasol => {
                let to_exasol_ts = |s: &str| {
                    let s = s
                        .trim_end_matches("+00:00")
                        .trim_end_matches('Z')
                        .replace('T', " ");
                    format!("TIMESTAMP '{s}'")
                };
                (
                    start.map(|start| format!("{event_time} >= {}", to_exasol_ts(&start))),
                    end.map(|end| format!("{event_time} < {}", to_exasol_ts(&end))),
                )
            }
            AdapterType::Starburst => todo!("Starburst"),
            AdapterType::Athena => todo!("Athena"),
            AdapterType::Trino => todo!("Trino"),
            AdapterType::Datafusion => todo!("Datafusion"),
            AdapterType::Dremio => todo!("Dremio"),
            AdapterType::Oracle => todo!("Oracle"),
        };

        // create the filter expression
        let filter = match (start, end) {
            (None, None) => return rendered,
            (Some(start), Some(end)) => format!("{start} and {end}"),
            (Some(start), None) => start,
            (None, Some(end)) => end,
        };

        // FIXME: for Postgres, we need to support _render_subquery_alias for the returned result
        // reference: https://github.com/dbt-labs/dbt-adapters/blob/d2f725651c05be0de07f3152d5b4842feae8a18a/dbt-adapters/src/dbt/adapters/base/relation.py#L222
        format!("(select * from {rendered} where {filter})")
    }

    /// Relation without any identifier
    fn without_identifier(&self) -> Result<Arc<dyn BaseRelation>, MinijinjaError> {
        let database = match self.database() {
            None | Some("") => None,
            Some(v) => Some(v.to_string()),
        };

        let schema = match self.schema() {
            None | Some("") => None,
            Some(v) => Some(v.to_string()),
        };

        self.create_relation(
            database,
            schema,
            None,
            self.relation_type(),
            self.quote_policy(),
        )
    }

    /// Include a relation component (database, schema, or identifier)
    fn include(
        &self,
        database: Option<bool>,
        schema: Option<bool>,
        identifier: Option<bool>,
    ) -> Result<Arc<dyn BaseRelation>, MinijinjaError> {
        let defaults = self.include_policy();
        let include_policy = Policy {
            database: database.unwrap_or(defaults.database),
            schema: schema.unwrap_or(defaults.schema),
            identifier: identifier.unwrap_or(defaults.identifier),
        };
        self.include_inner(include_policy)
    }

    fn quote(
        &self,
        database: Option<bool>,
        schema: Option<bool>,
        identifier: Option<bool>,
    ) -> Result<Arc<dyn BaseRelation>, MinijinjaError> {
        let defaults = self.include_policy();
        let quote_policy = Policy {
            database: database.unwrap_or(defaults.database),
            schema: schema.unwrap_or(defaults.schema),
            identifier: identifier.unwrap_or(defaults.identifier),
        };
        self.quote_inner(quote_policy)
    }

    /// Implement this to support `include`
    /// Replace the `include_policy` field with the input policy, and return that an update relation value
    fn include_inner(&self, _policy: Policy) -> Result<Arc<dyn BaseRelation>, MinijinjaError>;

    fn quote_inner(&self, _policy: Policy) -> Result<Arc<dyn BaseRelation>, MinijinjaError> {
        Err(MinijinjaError::new(
            MinijinjaErrorKind::InvalidOperation,
            "Not implemented",
        ))
    }

    /// Incorporate
    fn incorporate(
        &self,
        path: Option<Value>,
        relation_type: Option<RelationType>,
        location: Option<String>,
    ) -> Result<Arc<dyn BaseRelation>, MinijinjaError> {
        let (database, schema, identifier) = match path {
            Some(val) => match val.as_object() {
                Some(obj) => {
                    let database_value = obj.get_value(&Value::from("database"));
                    let schema_value = obj.get_value(&Value::from("schema"));
                    let identifier_value = obj.get_value(&Value::from("identifier"));

                    // Differentiate between "not provided" vs "provided but none"
                    let database = match database_value {
                        None => {
                            // Case 1: 'database' key was never provided in path
                            Some(self.database_as_str().unwrap())
                        }
                        Some(val) if val.is_none() => {
                            // Case 2: 'database' key was provided but set to none
                            None
                        }
                        Some(val) => {
                            // Case 3: 'database' key was provided with an actual value
                            Some(
                                val.as_str()
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| self.database_as_str().unwrap()),
                            )
                        }
                    };

                    // Similar logic for schema
                    let schema = match schema_value {
                        None => Some(self.schema_as_str().unwrap()), // Key not provided
                        Some(val) if val.is_none() => None,          // Key provided but none
                        Some(val) => Some(
                            val.as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| self.schema_as_str().unwrap()),
                        ),
                    };

                    let identifier = match identifier_value {
                        None => Some(self.identifier_as_str().unwrap()), // Key not provided
                        Some(val) if val.is_none() => Some(self.identifier_as_str().unwrap()), // Key provided but none
                        Some(val) => Some(
                            val.as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| self.identifier_as_str().unwrap()),
                        ),
                    };

                    (database, schema, identifier)
                }
                None => return invalid_argument!("incorrect 'path' value for incorporate"),
            },
            None => (
                Some(self.database_as_str()?),
                Some(self.schema_as_str()?),
                Some(self.identifier_as_str()?),
            ),
        };

        let relation_type = relation_type.or_else(|| self.relation_type());

        self.create_relation(
            database,
            schema,
            identifier,
            relation_type,
            self.quote_policy(),
        )?
        .post_incorporate(location)
    }

    /// Hook for adapter-specific incorporate behavior.
    /// Default implementation just delegates to create_relation.
    fn post_incorporate(
        &self,
        _location: Option<String>,
    ) -> Result<Arc<dyn BaseRelation>, MinijinjaError> {
        Ok(self.to_owned())
    }

    /// Derive a new relation from this one by appending `suffix` to the identifier
    /// (or, when `interpret_suffix_as_full_identifier` is set, using `suffix` as the
    /// whole identifier), keeping the same schema and optionally overriding the type.
    ///
    /// Called from the ClickHouse materialized-view and snapshot macros, e.g.
    /// `target_relation.derivative('_mv', 'materialized_view')`.
    /// Ref: dbt-clickhouse `relation.py::ClickHouseRelation.derivative`.
    fn derivative(
        &self,
        suffix: &str,
        relation_type: Option<RelationType>,
        interpret_suffix_as_full_identifier: bool,
    ) -> Result<Arc<dyn BaseRelation>, MinijinjaError> {
        let new_identifier = if interpret_suffix_as_full_identifier {
            suffix.to_string()
        } else {
            format!("{}{}", self.identifier_as_str()?, suffix)
        };
        let relation_type = relation_type.or_else(|| self.relation_type());
        self.create_relation(
            self.database().map(|s| s.to_string()),
            Some(self.schema_as_str()?),
            Some(new_identifier),
            relation_type,
            self.quote_policy(),
        )
    }

    /// Create a new relation with the specified components and policies.
    ///
    /// This method is used to create a new relation instance with the given database, schema,
    /// identifier, relation type, and quoting policy. It is a core method that enables several
    /// other relation operations:
    ///
    /// * [`BaseRelation::without_identifier`] - Clones a relation by setting its identifier to None
    /// * [`BaseRelation::replace_path`] - Clones a relation with updated path components
    /// * [`BaseRelation::incorporate`] - Clones a relation incorporating new path components
    fn create_relation(
        &self,
        database: Option<String>,
        schema: Option<String>,
        identifier: Option<String>,
        relation_type: Option<RelationType>,
        quote_policy: Policy,
    ) -> Result<Arc<dyn BaseRelation>, MinijinjaError>;

    /// reference: https://github.com/dbt-labs/dbt-adapters/blob/0775dd27929337ea529cad868dd1722812b8e0fb/dbt-adapters/src/dbt/adapters/base/relation.py#L234
    fn information_schema(&self, view_name: &str) -> Result<Arc<dyn BaseRelation>, MinijinjaError>;

    /// needs_to_drop
    fn needs_to_drop(
        &self,
        _old_relation: Option<Arc<dyn BaseRelation>>,
    ) -> Result<bool, MinijinjaError> {
        jinja_err!(
            MinijinjaErrorKind::InvalidOperation,
            "Only available for snowflake"
        )
    }

    /// is_iceberg_format
    fn is_iceberg_format(&self) -> bool {
        false
    }

    /// get_ddl_prefix_for_create
    fn get_ddl_prefix_for_create(
        &self,
        _model_config: Value,
        _temporary: bool,
    ) -> Result<String, MinijinjaError> {
        jinja_err!(
            MinijinjaErrorKind::InvalidOperation,
            "Only available for snowflake"
        )
    }

    /// get_ddl_prefix_for_alter
    fn get_ddl_prefix_for_alter(&self) -> Result<String, MinijinjaError> {
        jinja_err!(
            MinijinjaErrorKind::InvalidOperation,
            "Only available for snowflake"
        )
    }

    /// get_iceberg_ddl_options
    fn get_iceberg_ddl_options(&self, _config: Value) -> Result<String, MinijinjaError> {
        jinja_err!(
            MinijinjaErrorKind::InvalidOperation,
            "Only available for snowflake"
        )
    }

    /// dynamic_table_config_changeset
    fn dynamic_table_config_changeset(
        &self,
        _relation_results: &Value,
        _relation_config: &Value,
    ) -> Result<Value, MinijinjaError> {
        jinja_err!(
            MinijinjaErrorKind::InvalidOperation,
            "Only available for snowflake"
        )
    }

    /// from_config
    #[allow(clippy::wrong_self_convention)]
    fn from_config(&self, _config: &Value) -> Result<Value, MinijinjaError> {
        jinja_err!(
            MinijinjaErrorKind::InvalidOperation,
            "from_config: Only available for Snowflake and Redshift"
        )
    }

    /// Get max name length
    fn relation_max_name_length(&self) -> Result<u32, MinijinjaError> {
        unimplemented!("Available only for postgres and redshift")
    }

    fn is_hive_metastore(&self) -> bool {
        false
    }

    /// Whether the relation is a temporary view (session-scoped).
    fn is_temporary(&self) -> bool {
        false
    }

    /// ClickHouse relation state, stamped from the catalog (dbt-clickhouse
    /// `ClickHouseRelation.can_exchange`).
    fn can_exchange(&self) -> bool {
        false
    }

    /// dbt-managed MVs writing into this relation, `{schema, name, sql}` entries
    /// (`ClickHouseRelation.mvs_pointing_to_it`).
    fn mvs_pointing_to_it(&self) -> &[BTreeMap<String, String>] {
        &[]
    }

    /// `ClickHouseRelation.is_refreshable`
    fn is_refreshable(&self) -> bool {
        false
    }

    /// `ClickHouseRelation.refreshable_append`
    fn refreshable_append(&self) -> bool {
        false
    }

    /// materialized_view_config_changeset
    fn materialized_view_config_changeset(
        &self,
        _relation_results: &Value,
        _relation_config: &Value,
    ) -> Result<Value, MinijinjaError> {
        unimplemented!("Available only for BigQuery and Redshift")
    }
}
