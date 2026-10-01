use crate::metadata::bigquery as bigquery_metadata;
use crate::relation::RelationChangeSet;
use crate::relation::config_v2::{RelationConfig, RelationConfigLoader, SimpleComponentConfigImpl};
use crate::relation::databricks;
use crate::relation::redshift::materialized_view_config::{
    DescribeMaterializedViewResults, RedshiftMaterializedViewConfig,
    RedshiftMaterializedViewConfigChangeset,
};
use crate::relation::snowflake::config::relation_types::{dynamic_table, interactive_table};
use crate::relation::snowflake::config::{
    DYNAMIC_TABLE_KEY, INTERACTIVE_TABLE_KEY, SnowflakeDescribeResults, components,
};
use crate::relation::{RelationObject, StaticBaseRelation};
use crate::value::none_value;

use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::ident::max_identifier_length;
use dbt_common::{ErrorCode, FsResult, constants::DBT_CTE_PREFIX, fs_err};
use dbt_frontend_common::ident::Identifier;
use dbt_schema_store::CanonicalFqn;
use dbt_schemas::schemas::common::{DbtMaterialization, DbtQuoting};
use dbt_schemas::schemas::relations::base::{
    BaseRelation, BaseRelationProperties, Policy, RelationPath, TableFormat,
};
use dbt_schemas::schemas::relations::default_resolved_quoting_for;
use dbt_schemas::schemas::serde::minijinja_value_to_typed_struct;
use dbt_schemas::schemas::{InternalDbtNodeAttributes, InternalDbtNodeWrapper};

use arrow::array::RecordBatch;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::ResolvedQuoting;
use minijinja::Value;
use minijinja::arg_utils::ArgsIter;
use serde::Deserialize;

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

fn include_policy(adapter_type: AdapterType, path: &RelationPath) -> Policy {
    match adapter_type {
        AdapterType::DuckDB => Policy::new(
            path.database.as_ref().is_some_and(|db| {
                !db.is_empty()
                    && !db.eq_ignore_ascii_case("main")
                    && !db.eq_ignore_ascii_case("memory")
            }),
            true,
            true,
        ),
        AdapterType::ClickHouse | AdapterType::Exasol => Policy::new(false, true, true),
        AdapterType::Salesforce => Policy::new(false, false, true),
        _ => Policy::trues(),
    }
}

/// A struct representing the relation type for use with static methods
#[derive(Clone, Debug)]
pub struct RelationStatic {
    pub adapter_type: AdapterType,
    pub quoting: ResolvedQuoting,
}

impl StaticBaseRelation for RelationStatic {
    fn try_new(
        &self,
        database: Option<String>,
        schema: Option<String>,
        identifier: Option<String>,
        relation_type: Option<RelationType>,
        custom_quoting: Option<ResolvedQuoting>,
        temporary: Option<bool>,
    ) -> Result<Value, minijinja::Error> {
        Relation::new(self.adapter_type, database, schema, identifier)
            .with_relation_type(relation_type)
            .with_quoting(custom_quoting.unwrap_or(self.quoting))
            .with_temporary(temporary.unwrap_or(false))
            .validate()
            .map(|r| RelationObject::new(Arc::new(r)).into_value())
    }

    fn get_adapter_type(&self) -> String {
        self.adapter_type.as_ref().to_string()
    }

    fn get_default_quoting(&self) -> ResolvedQuoting {
        default_resolved_quoting_for(self.adapter_type)
    }

    fn create(&self, args: &[Value]) -> Result<Value, minijinja::Error> {
        match self.adapter_type {
            AdapterType::Snowflake => {
                let iter = ArgsIter::new("Relation.create", &[], args);
                let database: Option<String> = iter.next_kwarg::<Option<String>>("database")?;
                let schema: Option<String> = iter.next_kwarg::<Option<String>>("schema")?;
                let identifier: Option<String> = iter.next_kwarg::<Option<String>>("identifier")?;
                let relation_type: Option<String> = iter.next_kwarg::<Option<String>>("type")?;
                let custom_quoting: Option<Value> =
                    iter.next_kwarg::<Option<Value>>("quote_policy")?;
                let table_format: Option<String> =
                    iter.next_kwarg::<Option<String>>("table_format")?;
                let _ = iter.trailing_kwargs()?;

                let custom_quoting = custom_quoting
                    .and_then(|v| DbtQuoting::deserialize(v).ok())
                    .map(|v| ResolvedQuoting {
                        database: v.database.unwrap_or_default(),
                        identifier: v.identifier.unwrap_or_default(),
                        schema: v.schema.unwrap_or_default(),
                    })
                    .unwrap_or(self.quoting);

                let table_format = match table_format.as_deref() {
                    Some(s) if s.eq_ignore_ascii_case("iceberg") => TableFormat::Iceberg,
                    _ => TableFormat::Default,
                };

                let relation = Relation::new(AdapterType::Snowflake, database, schema, identifier)
                    .with_relation_type(relation_type.map(|s| RelationType::from(s.as_str())))
                    .with_quoting(custom_quoting)
                    .with_table_format(table_format)
                    .validate()?;
                let rel = RelationObject::new(Arc::new(relation));
                Ok(Value::from_object(rel))
            }
            _ => {
                let iter = ArgsIter::new("Relation.create", &[], args);
                let database = iter.next_kwarg::<Option<String>>("database")?;
                let schema = iter.next_kwarg::<Option<String>>("schema")?;
                let identifier = iter.next_kwarg::<Option<String>>("identifier")?;
                let relation_type = iter.next_kwarg::<Option<Value>>("type")?;
                let custom_quoting = iter.next_kwarg::<Option<Value>>("quote_policy")?;
                let temporary = iter.next_kwarg::<Option<bool>>("temporary")?;
                iter.finish()?;

                let custom_quoting = custom_quoting
                    .and_then(|v| DbtQuoting::deserialize(v).ok())
                    .map(|v| ResolvedQuoting {
                        database: v.database.unwrap_or_default(),
                        identifier: v.identifier.unwrap_or_default(),
                        schema: v.schema.unwrap_or_default(),
                    });

                self.try_new(
                    database,
                    schema,
                    identifier,
                    relation_type.and_then(|v: Value| {
                        if v.is_none() || v.is_undefined() {
                            None
                        } else {
                            Some(RelationType::from(v.as_str().unwrap_or_default()))
                        }
                    }),
                    custom_quoting,
                    temporary,
                )
            }
        }
    }
}

/// Generic relation implementation shared by Databricks, Spark, Fabric, and DuckDB.
///
/// The module path is historical; adapter-specific behavior must still be
/// gated on `adapter_type`.
#[derive(Clone, Debug)]
pub struct Relation {
    /// Whether this relation should behave as a parse-time Relation
    ///
    /// NOTE: It is an unfortunate inheritance from dbt Core 1.0 where parse-time
    /// relation does not behave the same as compile-time relation, and this has
    /// consequences on deferral logic, manifest writing etc. So we need to emulate
    /// the same behavior here...
    pub is_parse_time: bool,
    /// Whether this relation is a BigQuery INFORMATION_SCHEMA view.
    ///
    /// TODO(serramatutu): this is a workaround for the fact that RelationPath does not
    /// support 4-part names. once that happens we should delete this flag.
    pub is_information_schema: bool,
    /// The adapter type this relation instance is for.
    pub adapter_type: AdapterType,
    /// The path of the relation
    pub path: RelationPath,
    /// The relation type (default: None)
    pub relation_type: Option<RelationType>,
    /// Include policy
    pub include_policy: Policy,
    /// Quote policy
    pub quote_policy: Policy,
    /// The actual schema of the relation we got from db
    #[allow(dead_code)]
    pub native_schema: Option<RecordBatch>,
    /// Metadata about the relation
    pub metadata: Option<BTreeMap<String, String>>,
    /// Whether the relation is a delta table
    pub is_delta: bool,
    /// Whether the relation is a Databricks shallow clone
    pub is_shallow_clone: bool,
    /// Constraints to be created with the table
    pub create_constraints: Vec<databricks::typed_constraint::TypedConstraint>,
    /// Constraints to be applied during ALTER operations
    pub alter_constraints: Vec<databricks::typed_constraint::TypedConstraint>,
    /// Whether the relation is a temporary view (session-scoped).
    pub temporary: bool,
    /// ClickHouse catalog state; see [`BaseRelation::can_exchange`] and siblings.
    pub can_exchange: bool,
    pub mvs_pointing_to_it: Vec<BTreeMap<String, String>>,
    pub is_refreshable: bool,
    pub refreshable_append: bool,
    /// The location/region for this relation (BigQuery only, e.g., "US", "EU").
    pub location: Option<String>,
    /// DuckDB external source location, rendered in place of schema/table.
    pub external: Option<String>,
    /// The table format of the relation
    pub table_format: TableFormat,
}

impl BaseRelationProperties for Relation {
    fn include_policy(&self) -> Policy {
        self.include_policy
    }

    fn quote_policy(&self) -> Policy {
        self.quote_policy
    }

    fn get_database(&self) -> FsResult<String> {
        use AdapterType::*;
        match self.adapter_type {
            Databricks | Fabric | Postgres | Redshift | Salesforce | Bigquery => {
                self.path.database.clone().ok_or_else(|| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "database is required for {} relation",
                        self.adapter_type.as_ref()
                    )
                })
            }
            Spark => Ok(self.path.database.clone().unwrap_or_default()),
            _ => Ok(self.path.database.clone().unwrap_or_default()),
        }
    }

    fn get_schema(&self) -> FsResult<String> {
        match self.adapter_type {
            // FIXME: this will cause trouble in a few known places
            // In unit_test.rs, where this sed to build SQL literals
            // In schema_cache where we expect 3 part fqn, non-applicable for now since static analysis is unsupported for Salesforce
            AdapterType::Salesforce => Ok(String::new()),
            _ => self.path.schema.clone().ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "schema is required for {} relation",
                    self.adapter_type.as_ref()
                )
            }),
        }
    }

    fn get_identifier(&self) -> FsResult<String> {
        self.path.identifier.clone().ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "identifier is required for {} relation",
                self.adapter_type.as_ref()
            )
        })
    }

    fn get_canonical_fqn(&self) -> FsResult<CanonicalFqn> {
        use AdapterType::*;

        let db_str = self.get_database()?;
        let schema_str = self.get_schema()?;
        let ident_str = self.get_identifier()?;

        let db = if self.quote_policy().database {
            db_str
        } else {
            match self.adapter_type {
                Fabric | Bigquery => db_str,
                Salesforce | Snowflake => db_str.to_ascii_uppercase(),
                _ => db_str.to_ascii_lowercase(),
            }
        };

        let schema = if self.quote_policy().database {
            schema_str
        } else {
            match self.adapter_type {
                Fabric | Bigquery => schema_str,
                Salesforce | Snowflake => schema_str.to_ascii_uppercase(),
                _ => schema_str.to_ascii_lowercase(),
            }
        };

        let ident = if self.quote_policy().database {
            ident_str
        } else {
            match self.adapter_type {
                Fabric | Bigquery => ident_str,
                Salesforce | Snowflake => ident_str.to_ascii_uppercase(),
                _ => ident_str.to_ascii_lowercase(),
            }
        };

        Ok(CanonicalFqn::new(
            &Identifier::new(db),
            &Identifier::new(schema),
            &Identifier::new(ident),
        ))
    }
}

impl Relation {
    pub fn new(
        adapter_type: AdapterType,
        database: impl Into<Option<String>>,
        schema: impl Into<Option<String>>,
        identifier: impl Into<Option<String>>,
    ) -> Self {
        let path = RelationPath {
            database: match adapter_type {
                // ClickHouse adapter does not normalize empty strings to None
                // https://github.com/ClickHouse/dbt-clickhouse/blob/main/dbt/adapters/clickhouse/relation.py
                AdapterType::ClickHouse => Some(String::new()),
                _ => database.into().filter(|s| !s.is_empty()),
            },
            schema: schema.into(),
            identifier: identifier.into(),
        };
        let include_policy = include_policy(adapter_type, &path);
        let location = match adapter_type {
            // People set the BigQuery region as database (i.e BigQuery "project") sometimes.
            // In that case, set the location AND database to have the same value.
            AdapterType::Bigquery => {
                if let Some(db) = &path.database
                    && db.starts_with(bigquery_metadata::BIGQUERY_REGION_PREFIX)
                {
                    Some(
                        db.strip_prefix(bigquery_metadata::BIGQUERY_REGION_PREFIX)
                            .unwrap()
                            .to_string(),
                    )
                } else {
                    None
                }
            }
            _ => None,
        };

        Self {
            is_parse_time: false,
            is_information_schema: false,
            adapter_type,
            include_policy,
            path,
            location,
            relation_type: None,
            quote_policy: ResolvedQuoting::trues(),
            native_schema: None,
            metadata: None,
            is_delta: false,
            is_shallow_clone: false,
            create_constraints: Vec::new(),
            alter_constraints: Vec::new(),
            temporary: false,
            can_exchange: false,
            mvs_pointing_to_it: Vec::new(),
            is_refreshable: false,
            refreshable_append: false,
            external: None,
            table_format: TableFormat::Default,
        }
    }

    pub fn with_relation_type(mut self, relation_type: impl Into<Option<RelationType>>) -> Self {
        self.relation_type = relation_type.into();
        self
    }

    pub fn with_quoting(mut self, quoting: ResolvedQuoting) -> Self {
        self.quote_policy = quoting;
        self
    }

    pub fn with_include_policy(mut self, include_policy: Policy) -> Self {
        self.include_policy = include_policy;
        self
    }

    pub fn with_native_schema(mut self, native_schema: impl Into<Option<RecordBatch>>) -> Self {
        self.native_schema = native_schema.into();
        self
    }

    pub fn with_metadata(mut self, metadata: impl Into<Option<BTreeMap<String, String>>>) -> Self {
        self.metadata = metadata.into();
        self
    }

    pub fn with_is_delta(mut self, is_delta: bool) -> Self {
        self.is_delta = is_delta;
        self
    }

    pub fn with_is_shallow_clone(mut self, is_shallow_clone: bool) -> Self {
        self.is_shallow_clone = is_shallow_clone;
        self
    }

    pub fn with_temporary(mut self, temporary: bool) -> Self {
        self.temporary = temporary;
        self
    }

    pub fn with_can_exchange(mut self, can_exchange: bool) -> Self {
        self.can_exchange = can_exchange;
        self
    }

    pub fn with_mvs_pointing_to_it(mut self, mvs: Vec<BTreeMap<String, String>>) -> Self {
        self.mvs_pointing_to_it = mvs;
        self
    }

    pub fn with_is_refreshable(mut self, is_refreshable: bool) -> Self {
        self.is_refreshable = is_refreshable;
        self
    }

    pub fn with_refreshable_append(mut self, refreshable_append: bool) -> Self {
        self.refreshable_append = refreshable_append;
        self
    }

    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    pub fn with_table_format(mut self, table_format: TableFormat) -> Self {
        self.table_format = table_format;
        self
    }

    pub fn with_external(mut self, external: impl Into<String>) -> Self {
        self.external = Some(external.into());
        self
    }

    pub fn validate(self) -> Result<Self, minijinja::Error> {
        if let (Some(ident), Some(_relation_type), Some(max_len)) = (
            &self.path.identifier,
            &self.relation_type,
            max_identifier_length(self.adapter_type),
        ) {
            if ident.len() > max_len.into() {
                let message = format!(
                    "Relation name '{}' is longer than {} characters",
                    ident, max_len
                );
                return Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    message,
                ));
            }
        }

        Ok(self)
    }

    /// Create a relation that stubs some stuff to be used during `dbt parse`
    pub(crate) fn new_parse_time(adapter_type: AdapterType) -> Self {
        let path = RelationPath {
            database: Some("".to_string()),
            schema: Some("".to_string()),
            identifier: Some("".to_string()),
        };
        Self {
            is_parse_time: true,
            is_information_schema: false,
            adapter_type,
            include_policy: Policy::falses(),
            path,
            relation_type: None,
            quote_policy: Policy::falses(),
            native_schema: None,
            metadata: None,
            is_delta: false,
            is_shallow_clone: false,
            create_constraints: Vec::default(),
            alter_constraints: Vec::default(),
            temporary: false,
            can_exchange: false,
            mvs_pointing_to_it: Vec::new(),
            is_refreshable: false,
            refreshable_append: false,
            location: Some("".to_string()),
            external: None,
            table_format: TableFormat::Default,
        }
    }

    pub fn new_information_schema(
        adapter_type: AdapterType,
        project: Option<&str>,
        dataset: &str,
        view_name: &str,
        location: Option<&str>,
    ) -> Self {
        let mut r = Self::new(
            adapter_type,
            project.map(|p| p.to_string()),
            Some(dataset.to_string()),
            Some(view_name.to_string()),
        )
        .with_include_policy(Policy::trues())
        .with_quoting(Policy::falses());
        r.is_information_schema = true;
        if let Some(loc) = location {
            r = r.with_location(loc);
        }
        r
    }

    pub fn new_fabric(
        database: Option<String>,
        schema: Option<String>,
        identifier: Option<String>,
        relation_type: Option<RelationType>,
        custom_quoting: ResolvedQuoting,
    ) -> Self {
        Self::new(AdapterType::Fabric, database, schema, identifier)
            .with_relation_type(relation_type)
            .with_quoting(custom_quoting)
    }

    /// Add a constraint, routing to create_constraints or alter_constraints based on type
    pub fn add_constraint(&mut self, constraint: databricks::typed_constraint::TypedConstraint) {
        use dbt_schemas::schemas::common::ConstraintType;

        match constraint.constraint_type() {
            ConstraintType::Check => {
                self.alter_constraints.push(constraint);
            }
            _ => {
                self.create_constraints.push(constraint);
            }
        }
    }

    /// Create a copy of the relation with the given constraints added.
    ///
    /// Reference: https://github.com/databricks/dbt-databricks/blob/25caa2a14ed0535f08f6fd92e29b39df1f453e4d/dbt/adapters/databricks/relation.py#L213-L217
    pub fn enrich(&self, constraints: &[databricks::typed_constraint::TypedConstraint]) -> Self {
        let mut relation = self.clone();
        for constraint in constraints {
            relation.add_constraint(constraint.clone());
        }
        relation
    }

    /// Render constraint DDL for CREATE TABLE.
    ///
    /// Reference: https://github.com/databricks/dbt-databricks/blob/25caa2a14ed0535f08f6fd92e29b39df1f453e4d/dbt/adapters/databricks/relation.py#L219-L221
    pub fn render_constraints_for_create(&self) -> String {
        self.create_constraints
            .iter()
            .map(|c| c.render())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The Snowflake relation types whose configuration is loaded and diffed component by
/// component. The dynamic-table and interactive-table Jinja names are both served by the
/// dynamic-table implementations, which select between them on the model's `materialized`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SnowflakeConfigRelationType {
    DynamicTable,
    InteractiveTable,
}

impl SnowflakeConfigRelationType {
    fn of(local_config: &dyn InternalDbtNodeAttributes) -> Self {
        match local_config.materialized() {
            DbtMaterialization::InteractiveTable => Self::InteractiveTable,
            _ => Self::DynamicTable,
        }
    }

    fn loader(self) -> RelationConfigLoader<'static, SnowflakeDescribeResults> {
        match self {
            Self::DynamicTable => dynamic_table::new_loader(),
            Self::InteractiveTable => interactive_table::new_loader(),
        }
    }

    fn describe_result_key(self) -> &'static str {
        match self {
            Self::DynamicTable => DYNAMIC_TABLE_KEY,
            Self::InteractiveTable => INTERACTIVE_TABLE_KEY,
        }
    }
}

fn target_lag_of(config: &RelationConfig) -> Option<&Option<String>> {
    config
        .get(components::target_lag::TYPE_NAME)?
        .as_any()
        .downcast_ref::<SimpleComponentConfigImpl<Option<String>>>()
        .map(|component| &component.value)
}

/// A static interactive table is a plain `TABLE` with no warehouse attached, so it cannot be
/// made refreshing in place: Snowflake rejects `ALTER INTERACTIVE TABLE ... SET TARGET_LAG`
/// with `001420`. The reverse direction is equally unalterable but visible from the changeset
/// alone, so only this direction is decided here.
fn target_lag_newly_set(desired_state: &RelationConfig, current_state: &RelationConfig) -> bool {
    matches!(
        (target_lag_of(current_state), target_lag_of(desired_state)),
        (Some(None), Some(Some(_)))
    )
}

fn dynamic_table_config_changeset_from_local_config(
    local_config: &dyn InternalDbtNodeAttributes,
    relation_results_value: &Value,
) -> Result<Value, minijinja::Error> {
    let config_relation_type = SnowflakeConfigRelationType::of(local_config);
    let relation_results = SnowflakeDescribeResults::from_value_with_key(
        relation_results_value,
        config_relation_type.describe_result_key(),
    )
    .map_err(|e| {
        minijinja::Error::new(
            minijinja::ErrorKind::SerdeDeserializeError,
            format!("from_config: Failed to serialize SnowflakeDescribeResults: {e}"),
        )
    })?;

    let loader = config_relation_type.loader();
    let current_state = loader.from_remote_state(&relation_results)?;
    let desired_state = loader.from_local_config(local_config)?;

    let mut changeset = RelationConfig::diff(&desired_state, &current_state);
    if config_relation_type == SnowflakeConfigRelationType::InteractiveTable
        && target_lag_newly_set(&desired_state, &current_state)
    {
        changeset = changeset.forcing_full_refresh();
    }

    if changeset.is_empty() {
        Ok(none_value())
    } else {
        Ok(Value::from_object(changeset))
    }
}

impl BaseRelation for Relation {
    fn is_system(&self) -> bool {
        use AdapterType::*;
        match self.adapter_type {
            // It might be relation under a `information_schema` schema or a `system` catalog
            // For example, system.billing.list_prices or [database].information_schema.tables
            // are both system tables
            Databricks | Spark => {
                self.path
                    .database
                    .as_ref()
                    .map(|db| db.eq_ignore_ascii_case(databricks::SYSTEM_DATABASE))
                    .unwrap_or(false)
                    || self
                        .path
                        .schema
                        .as_ref()
                        .map(|schema| {
                            schema.eq_ignore_ascii_case(databricks::INFORMATION_SCHEMA_SCHEMA)
                        })
                        .unwrap_or(false)
            }
            Bigquery => self
                .path
                .schema
                .as_ref()
                .map(|schema| {
                    schema.eq_ignore_ascii_case("information_schema") || self.is_information_schema
                })
                .unwrap_or(false),
            _ => false,
        }
    }

    fn has_information(&self) -> bool {
        self.metadata.is_some()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn to_owned(&self) -> Arc<dyn BaseRelation> {
        Arc::new(self.clone())
    }

    fn create_from(&self) -> Result<Arc<dyn BaseRelation>, minijinja::Error> {
        unimplemented!("{} relation creation from Jinja values", self.adapter_type)
    }

    fn database(&self) -> Option<&str> {
        if self.is_parse_time {
            return None;
        }

        self.path.database.as_deref()
    }

    fn schema(&self) -> Option<&str> {
        self.path.schema.as_deref()
    }

    fn identifier(&self) -> Option<&str> {
        self.path.identifier.as_deref()
    }

    fn relation_type(&self) -> Option<RelationType> {
        self.relation_type
    }

    fn adapter_type(&self) -> AdapterType {
        self.adapter_type
    }

    fn include_inner(&self, policy: Policy) -> Result<Arc<dyn BaseRelation>, minijinja::Error> {
        let mut relation = Relation::new(
            self.adapter_type,
            self.path.database.clone(),
            self.path.schema.clone(),
            self.path.identifier.clone(),
        )
        .with_relation_type(self.relation_type)
        .with_include_policy(policy)
        .with_quoting(self.quote_policy)
        .with_metadata(self.metadata.clone())
        .with_is_delta(self.is_delta)
        .with_is_shallow_clone(self.is_shallow_clone)
        .with_temporary(self.temporary)
        .with_can_exchange(self.can_exchange)
        .with_mvs_pointing_to_it(self.mvs_pointing_to_it.clone())
        .with_is_refreshable(self.is_refreshable)
        .with_refreshable_append(self.refreshable_append)
        // FIXME: no need to validate here since the parent relation is already valid.
        .validate()?;

        // Preserve constraints
        relation.create_constraints = self.create_constraints.clone();
        relation.alter_constraints = self.alter_constraints.clone();
        relation.external = self.external.clone();
        relation.is_parse_time = self.is_parse_time;

        Ok(Arc::new(relation))
    }

    fn quote_inner(&self, policy: Policy) -> Result<Arc<dyn BaseRelation>, minijinja::Error> {
        let mut relation = Relation::new(
            self.adapter_type,
            self.path.database.clone(),
            self.path.schema.clone(),
            self.path.identifier.clone(),
        )
        .with_relation_type(self.relation_type)
        .with_include_policy(self.include_policy)
        .with_quoting(policy)
        .with_metadata(self.metadata.clone())
        .with_is_delta(self.is_delta)
        .with_is_shallow_clone(self.is_shallow_clone)
        .with_temporary(self.temporary)
        .with_can_exchange(self.can_exchange)
        .with_mvs_pointing_to_it(self.mvs_pointing_to_it.clone())
        .with_is_refreshable(self.is_refreshable)
        .with_refreshable_append(self.refreshable_append)
        // FIXME: no need to validate here since the parent relation is already valid.
        .validate()?;
        relation.create_constraints = self.create_constraints.clone();
        relation.alter_constraints = self.alter_constraints.clone();
        relation.is_parse_time = self.is_parse_time;
        Ok(Arc::new(relation))
    }

    fn post_incorporate(
        &self,
        location: Option<String>,
    ) -> Result<Arc<dyn BaseRelation>, minijinja::Error> {
        if let Some(loc) = location {
            let mut cloned = self.clone();
            cloned.location = Some(loc);
            return Ok(Arc::new(cloned));
        }
        Ok(Arc::new(self.clone()))
    }

    fn is_hive_metastore(&self) -> bool {
        match self.adapter_type {
            AdapterType::Databricks | AdapterType::Spark => {
                // Match Python dbt-databricks semantics:
                // def is_hive_metastore(database: Optional[str], temporary: Optional[bool] = False) -> bool:
                //     return (database is None or database.lower() == "hive_metastore") and not temporary
                //
                // Note: The `temporary` field only tracks Unity Catalog temporary tables, not Hive Metastore temporary views.
                // Unity Catalog temporary tables are never considered to be in Hive Metastore.
                (self.path.database.is_none()
                    || self.path.database.as_ref().map(|s| s.to_lowercase())
                        == Some(databricks::DEFAULT_DATABRICKS_DATABASE.to_string()))
                    && !self.temporary
            }
            _ => false,
        }
    }

    fn is_temporary(&self) -> bool {
        self.temporary
    }

    fn can_exchange(&self) -> bool {
        self.can_exchange
    }

    fn mvs_pointing_to_it(&self) -> &[BTreeMap<String, String>] {
        &self.mvs_pointing_to_it
    }

    fn is_refreshable(&self) -> bool {
        self.is_refreshable
    }

    fn refreshable_append(&self) -> bool {
        self.refreshable_append
    }

    fn is_delta(&self) -> bool {
        self.is_delta
    }

    fn set_is_delta(&mut self, is_delta: Option<bool>) {
        match self.adapter_type {
            AdapterType::Databricks | AdapterType::Spark => {
                self.is_delta = is_delta.unwrap_or(self.is_delta);
            }
            _ => {}
        }
    }

    fn is_shallow_clone(&self) -> bool {
        self.is_shallow_clone
    }

    #[allow(clippy::single_match)]
    fn set_is_shallow_clone(&mut self, is_shallow_clone: Option<bool>) {
        match self.adapter_type {
            AdapterType::Databricks => {
                self.is_shallow_clone = is_shallow_clone.unwrap_or(self.is_shallow_clone);
            }
            _ => {}
        }
    }

    fn set_table_format(&mut self, table_format: Option<TableFormat>) {
        if self.adapter_type == AdapterType::Snowflake
            && let Some(table_format) = table_format
        {
            self.table_format = table_format;
        }
    }

    fn is_materialized_view(&self) -> bool {
        let result = matches!(self.relation_type, Some(RelationType::MaterializedView));
        result
    }

    fn is_iceberg_format(&self) -> bool {
        matches!(self.table_format, TableFormat::Iceberg)
    }

    /// Returns the DDL prefix: one of "temporary", "iceberg", "transient", or "".
    fn get_ddl_prefix_for_create(
        &self,
        config: Value,
        temporary: bool,
    ) -> Result<String, minijinja::Error> {
        match self.adapter_type {
            AdapterType::Snowflake => {
                if temporary {
                    return Ok("temporary".to_string());
                }

                // Extract legacy Iceberg configuration values found in a model config.
                // https://docs.getdbt.com/docs/mesh/iceberg/snowflake-iceberg-support#example-configuration
                let is_iceberg = config
                    .get_item(&Value::from("table_format"))
                    .is_ok_and(|v| v.as_str().is_some_and(|s| s == "iceberg"));

                let transient_explicitly_set_true = config
                    .get_item(&Value::from("transient"))
                    .map(|v| v.is_true())
                    .unwrap_or(false);

                if is_iceberg {
                    if transient_explicitly_set_true {
                        eprintln!(
                            "Warning: Iceberg format relations cannot be transient. Please remove either \
                                    the transient=true or iceberg config options from {}.{}.{}. If left unmodified, \
                                    dbt will ignore 'transient'.",
                            self.path.database.as_deref().unwrap_or(""),
                            self.path.schema.as_deref().unwrap_or(""),
                            self.path.identifier.as_deref().unwrap_or("")
                        );
                    }
                    return Ok("iceberg".to_string());
                }

                let is_transient = config
                    .get_item(&Value::from("transient"))
                    .map(|v| v.is_true() || v.is_undefined())
                    .unwrap_or(true);

                Ok(if is_transient {
                    "transient".to_string()
                } else {
                    String::new()
                })
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "Only available for snowflake",
            )),
        }
    }

    /// https://github.com/dbt-labs/dbt-adapters/blob/2a94cc75dba1f98fa5caff1f396f5af7ee444598/dbt-snowflake/src/dbt/adapters/snowflake/relation.py#L206
    fn get_iceberg_ddl_options(
        &self,
        runtime_model_config: Value,
    ) -> Result<String, minijinja::Error> {
        match self.adapter_type {
            AdapterType::Snowflake => {
                // If the base_location_root config is supplied, overwrite the default value ("_dbt/")
                let mut base_location = runtime_model_config
                    .get_attr("base_location_root")?
                    .as_str()
                    .unwrap_or("_dbt")
                    .to_string();

                base_location.push_str(&format!(
                    "/{}/{}",
                    self.schema_as_str().unwrap_or_default(),
                    self.identifier_as_str().unwrap_or_default()
                ));

                if let Some(subpath) = runtime_model_config
                    .get_attr("base_location_subpath")?
                    .as_str()
                {
                    base_location.push_str(&format!("/{subpath}"))
                }

                let external_volume = runtime_model_config
                    .get_attr("external_volume")?
                    .as_str()
                    .ok_or_else(|| {
                        minijinja::Error::new(
                            minijinja::ErrorKind::NonKey,
                            "external_volume is required",
                        )
                    })?
                    .to_string();

                let iceberg_ddl_predicates = format!(
                    "\nexternal_volume = '{external_volume}'\ncatalog = 'snowflake'\nbase_location = '{base_location}'\n"
                );

                // Indent each line by 10 spaces
                let result = iceberg_ddl_predicates
                    .lines()
                    // the first argument is an empty string that then get 10 spaces padding
                    .map(|line| format!("{:indent$}{line}", "", indent = 10))
                    .collect::<Vec<String>>()
                    .join("\n");

                Ok(result)
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "Only available for snowflake",
            )),
        }
    }

    fn get_ddl_prefix_for_alter(&self) -> Result<String, minijinja::Error> {
        match self.adapter_type {
            AdapterType::Snowflake => {
                if self.table_format == TableFormat::Iceberg {
                    Ok("iceberg".to_string())
                } else {
                    Ok(String::new())
                }
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "Only available for snowflake",
            )),
        }
    }

    // https://github.com/dbt-labs/dbt-adapters/blob/2a94cc75dba1f98fa5caff1f396f5af7ee444598/dbt-snowflake/src/dbt/adapters/snowflake/relation.py#L223
    fn needs_to_drop(
        &self,
        old_relation: Option<Arc<dyn BaseRelation>>,
    ) -> Result<bool, minijinja::Error> {
        match self.adapter_type {
            AdapterType::Snowflake => {
                let Some(old_relation) = old_relation else {
                    return Ok(false);
                };

                // `CREATE OR REPLACE` can only replace an object with one of the same kind
                // (Snowflake rejects a cross-kind replacement with `001998`), so the kind of
                // object each side is stored as is the granularity this decides at. A static
                // interactive table is stored as a plain `TABLE`, but it is still declared as
                // an interactive table, so the declared relation type is what matters.
                match (
                    old_relation.is_interactive_table(),
                    self.is_interactive_table(),
                ) {
                    (true, true) => Ok(false),
                    // Its own kind — the `table_format` check below would treat it as
                    // replaceable in place.
                    (true, _) | (_, true) => Ok(true),
                    // core does only checks this for table conversions
                    _ if old_relation.is_table() => {
                        // invoke drop for table -> Iceberg or Iceberg -> table
                        let old_relation_table_format = old_relation
                            .as_any()
                            .downcast_ref::<Relation>()
                            .unwrap()
                            .table_format;
                        Ok(self.table_format != old_relation_table_format)
                    }
                    // Views must be dropped; dynamic tables are rebuilt cross-catalog via
                    // full refresh.
                    _ => Ok(true),
                }
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "Only available for snowflake",
            )),
        }
    }

    fn can_be_renamed(&self) -> bool {
        use AdapterType::*;
        use RelationType::*;

        match (self.adapter_type, self.relation_type()) {
            (Bigquery, Some(Table)) => true,
            (Snowflake, Some(Table) | Some(View) | Some(InteractiveTable)) => {
                !self.is_iceberg_format()
            }
            (_, Some(Table) | Some(View)) => true,
            (_, _) => false,
        }
    }

    fn can_be_replaced(&self) -> bool {
        use AdapterType::*;
        use RelationType::*;

        match (self.adapter_type, self.relation_type()) {
            (Redshift, Some(View)) => true,
            (Snowflake, Some(Table) | Some(View) | Some(DynamicTable) | Some(InteractiveTable)) => {
                true
            }
            (_, Some(Table) | Some(View)) => true,
            (_, _) => false,
        }
    }

    // https://github.com/dbt-labs/dbt-adapters/blob/292d17301eff3c8a972fcd57f7deb3aac4c8a3cb/dbt-snowflake/src/dbt/adapters/snowflake/relation.py#L92
    fn dynamic_table_config_changeset(
        &self,
        relation_results_value: &Value,
        relation_config_value: &Value,
    ) -> Result<Value, minijinja::Error> {
        match self.adapter_type {
            AdapterType::Snowflake => {
                // TODO(serramatutu): minijinja_value_to_typed_struct does not work with references, so we
                // have to clone the value here...
                let local_config = minijinja_value_to_typed_struct::<InternalDbtNodeWrapper>(
                    relation_config_value.clone(),
                )
                .map_err(|e| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::SerdeDeserializeError,
                        format!("Failed to deserialize InternalDbtNodeWrapper: {e}"),
                    )
                })?;
                let local_config = match local_config {
                    InternalDbtNodeWrapper::Model(model) => model,
                    _ => {
                        return Err(minijinja::Error::new(
                            minijinja::ErrorKind::InvalidOperation,
                            "Expected a model node",
                        ));
                    }
                };

                dynamic_table_config_changeset_from_local_config(
                    local_config.as_ref(),
                    relation_results_value,
                )
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "Only available for snowflake",
            )),
        }
    }

    fn from_config(&self, config: &Value) -> Result<Value, minijinja::Error> {
        match self.adapter_type {
            AdapterType::Redshift => Ok(Value::from_object(
                node_value_to_redshift_materialized_view(config)?,
            )),
            // https://github.com/dbt-labs/dbt-adapters/blob/816d190c9e31391a48cee979bd049aeb34c89ad3/dbt-snowflake/src/dbt/adapters/snowflake/relation.py#L81
            AdapterType::Snowflake => {
                // TODO(serramatutu): minijinja_value_to_typed_struct does not work with references, so we
                // have to clone the value here...
                let local_config =
                    minijinja_value_to_typed_struct::<InternalDbtNodeWrapper>(config.clone())
                        .map_err(|e| {
                            minijinja::Error::new(
                                minijinja::ErrorKind::SerdeDeserializeError,
                                format!("Failed to deserialize InternalDbtNodeWrapper: {e}"),
                            )
                        })?;
                let local_config = match local_config {
                    InternalDbtNodeWrapper::Model(model) => model,
                    _ => {
                        return Err(minijinja::Error::new(
                            minijinja::ErrorKind::InvalidOperation,
                            "Expected a model node",
                        ));
                    }
                };
                let relation_config = SnowflakeConfigRelationType::of(local_config.as_ref())
                    .loader()
                    .from_local_config(local_config.as_ref())?;

                Ok(Value::from_object(relation_config))
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "from_config: Only available for Snowflake and Redshift",
            )),
        }
    }

    fn normalize_component(&self, component: &str) -> String {
        use AdapterType::*;
        match self.adapter_type {
            Salesforce | Bigquery | ClickHouse => component.to_string(),
            Snowflake => component.to_uppercase(),
            _ => component.to_lowercase(),
        }
    }

    fn render_self_as_str(&self) -> String {
        if self.adapter_type == AdapterType::DuckDB
            && let Some(external) = &self.external
        {
            return external.clone();
        }

        // Render BigQuery INFORMATION_SCHEMA view names using proper qualifiers
        if self.adapter_type == AdapterType::Bigquery && self.is_system() {
            let dataset = self
                .path
                .schema
                .as_deref()
                .expect("INFORMATION_SCHEMA relation needs dataset");
            let view_name = self.path.identifier.as_deref();

            // Detect if user has set `project == location` and normalize
            let project = self
                .path
                .database
                .as_deref()
                .filter(|proj| !proj.starts_with(bigquery_metadata::BIGQUERY_REGION_PREFIX));

            let fqn_no_project = if let Some(view_name) = view_name {
                bigquery_metadata::generate_system_table_fqn(
                    dataset,
                    view_name,
                    self.location.as_deref(),
                )
            } else {
                // NOTE: adapter.check_schema_exists() uses Relation without a
                // view name, so we have to fall back to something that works with that
                // macro. This is not pretty.
                "INFORMATION_SCHEMA".to_string()
            };

            let fqn = if let Some(project) = &project {
                format!("`{project}`.{fqn_no_project}")
            } else {
                fqn_no_project
            };
            return fqn;
        }

        if let Some(RelationType::Ephemeral) = self.relation_type {
            return format!(
                "{}{}",
                DBT_CTE_PREFIX,
                self.path.identifier.as_deref().unwrap_or_default()
            );
        }

        let include_policy = self.include_policy;
        let quote_policy = self.quote_policy;
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

        if matches!(self.adapter_type, AdapterType::Databricks) {
            rendered.to_ascii_lowercase()
        } else {
            rendered
        }
    }

    fn create_relation(
        &self,
        database: Option<String>,
        schema: Option<String>,
        identifier: Option<String>,
        relation_type: Option<RelationType>,
        custom_quoting: Policy,
    ) -> Result<Arc<dyn BaseRelation>, minijinja::Error> {
        let relation = Relation::new(self.adapter_type, database, schema, identifier)
            .with_relation_type(relation_type)
            .with_quoting(custom_quoting)
            .with_metadata(self.metadata.clone())
            .with_is_delta(self.is_delta)
            .with_is_shallow_clone(self.is_shallow_clone)
            .with_temporary(self.temporary)
            .with_table_format(self.table_format)
            .with_can_exchange(self.can_exchange)
            .with_mvs_pointing_to_it(self.mvs_pointing_to_it.clone())
            .with_is_refreshable(self.is_refreshable)
            .with_refreshable_append(self.refreshable_append)
            .validate()?;
        Ok(Arc::new(relation))
    }

    fn information_schema(
        &self,
        view_name: &str,
    ) -> Result<Arc<dyn BaseRelation>, minijinja::Error> {
        let schema = match self.adapter_type {
            // See the workaround related to Relation.is_information_schema and 4-part names
            AdapterType::Bigquery => self.path.schema.as_deref().unwrap_or_default(),
            _ => "INFORMATION_SCHEMA",
        };
        Ok(Arc::new(Self::new_information_schema(
            self.adapter_type,
            self.path.database.as_deref(),
            schema,
            view_name,
            self.location.as_deref(),
        )))
    }

    fn relation_max_name_length(&self) -> Result<u32, minijinja::Error> {
        Ok(max_identifier_length(self.adapter_type)
            .map(|v| v.get().try_into().unwrap_or(u32::MAX))
            .unwrap_or(u32::MAX))
    }

    fn materialized_view_config_changeset(
        &self,
        remote_state_value: &Value,
        local_config_value: &Value,
    ) -> Result<Value, minijinja::Error> {
        use AdapterType::*;
        match self.adapter_type {
            // FIXME(serramatutu): port over to RelationConfig v2
            Redshift => {
                let remote_state = DescribeMaterializedViewResults::try_from(
                    remote_state_value,
                    )
                    .map_err(|e| {
                        minijinja::Error::new(
                            minijinja::ErrorKind::SerdeDeserializeError,
                            format!(
                                "from_config: Failed to serialized DescribeMaterializedViewResults: {e}"
                            )
                        )
                    })?;

                let remote_state = RedshiftMaterializedViewConfig::try_from(remote_state)
                    .map_err(|e| {
                        minijinja::Error::new(
                            minijinja::ErrorKind::SerdeDeserializeError,
                            format!(
                                "materialized_view_config_changeset: Failed to deserialize RedshiftMaterializedViewConfig: {e}"
                            ),
                        )
                    })?;

                let local_config = node_value_to_redshift_materialized_view(local_config_value)?;

                let changeset =
                    RedshiftMaterializedViewConfigChangeset::new(remote_state, local_config);

                if changeset.has_changes() {
                    Ok(Value::from_object(changeset))
                } else {
                    Ok(Value::from(None::<()>))
                }
            }
            Bigquery => {
                if remote_state_value.is_none() {
                    return Err(minijinja::Error::new(
                        minijinja::ErrorKind::InvalidArgument,
                        "remote_state cannot be None",
                    ));
                }
                let current_state = remote_state_value
                    .as_object()
                    .ok_or_else(|| {
                        minijinja::Error::new(
                            minijinja::ErrorKind::InvalidArgument,
                            "remote_state must be Object",
                        )
                    })?
                    .downcast_ref::<RelationConfig>()
                    .ok_or_else(|| {
                        minijinja::Error::new(
                            minijinja::ErrorKind::InvalidArgument,
                            "remote_state must be RelationConfig",
                        )
                    })?;

                // TODO(serramatutu): minijinja_value_to_typed_struct does not work with
                // references, so we have to clone the value here...
                let local_config = minijinja_value_to_typed_struct::<InternalDbtNodeWrapper>(
                    local_config_value.clone(),
                )
                .map_err(|e| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::SerdeDeserializeError,
                        format!("Failed to deserialize InternalDbtNodeWrapper: {e}"),
                    )
                })?;
                let local_config = match local_config {
                    InternalDbtNodeWrapper::Model(model) => model,
                    _ => {
                        return Err(minijinja::Error::new(
                            minijinja::ErrorKind::InvalidOperation,
                            "Expected a model node",
                        ));
                    }
                };
                let desired_state =
                    crate::relation::bigquery::config::relation_types::materialized_view::new_loader()
                        .from_local_config(local_config.as_ref())?;

                let changeset = RelationConfig::diff(&desired_state, current_state);

                if changeset.is_empty() {
                    Ok(none_value())
                } else {
                    Ok(Value::from_object(changeset))
                }
            }
            _ => unimplemented!("Available only for BigQuery and Redshift"),
        }
    }
}

// FIXME(serramatutu): this should be deleted from here once Redshift Materialized
// Views are migrated to RelationConfig v2.
fn node_value_to_redshift_materialized_view(
    node_value: &Value,
) -> Result<RedshiftMaterializedViewConfig, minijinja::Error> {
    let config_wrapper = InternalDbtNodeWrapper::deserialize(node_value).map_err(|e| {
        minijinja::Error::new(
            minijinja::ErrorKind::SerdeDeserializeError,
            format!("Failed to deserialize InternalDbtNodeWrapper: {e}"),
        )
    })?;

    let model = match config_wrapper {
        InternalDbtNodeWrapper::Model(model) => model,
        _ => {
            return Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "Expected a model node",
            ));
        }
    };

    if model.__base_attr__.materialized != DbtMaterialization::MaterializedView {
        return Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!(
                "Unsupported operation for materialization type {}",
                &model.__base_attr__.materialized
            ),
        ));
    }

    RedshiftMaterializedViewConfig::try_from(&*model).map_err(|e| {
        minijinja::Error::new(
            minijinja::ErrorKind::SerdeDeserializeError,
            format!("Failed to deserialize RedshiftMaterializedViewConfig: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::{dbt_types::RelationType, schemas::relations::DEFAULT_RESOLVED_QUOTING};

    mod postgres {
        use super::*;

        #[test]
        fn test_try_new_via_static_base_relation() {
            let relation_type = RelationStatic {
                adapter_type: AdapterType::Postgres,
                quoting: DEFAULT_RESOLVED_QUOTING,
            };
            let relation = relation_type
                .try_new(
                    Some("d".to_string()),
                    Some("s".to_string()),
                    Some("i".to_string()),
                    Some(RelationType::Table),
                    Some(DEFAULT_RESOLVED_QUOTING),
                    None,
                )
                .unwrap();

            let relation = relation.downcast_object::<RelationObject>().unwrap();
            assert_eq!(relation.inner().render_self_as_str(), "\"d\".\"s\".\"i\"");
            assert_eq!(relation.relation_type().unwrap(), RelationType::Table);
        }
    }

    /// This module tests all the fallback behaviors that are not adapter-type specific
    mod generic {
        use super::*;
        use chrono::{DateTime, NaiveDate, Utc};
        use dbt_schemas::filter::{RunFilter, Sample};

        fn relation(quoting: Policy) -> Relation {
            Relation::new(
                AdapterType::Postgres,
                "test_db".to_string(),
                "test_schema".to_string(),
                "test_table".to_string(),
            )
            .with_quoting(quoting)
        }

        #[test]
        fn test_get_components() {
            let relation = relation(Policy::enabled());
            assert_eq!(
                relation.get("database", None).unwrap(),
                Value::from("test_db")
            );
            assert_eq!(
                relation.get("schema", None).unwrap(),
                Value::from("test_schema")
            );
            assert_eq!(
                relation.get("identifier", None).unwrap(),
                Value::from("test_table")
            );
        }

        #[test]
        fn test_get_metadata() {
            let result = relation(Policy::enabled()).get("metadata", None).unwrap();
            let mut expected = BTreeMap::new();
            expected.insert("type", Value::from(std::any::type_name::<Relation>()));
            assert_eq!(result, Value::from(expected));
        }

        #[test]
        fn test_get_nonexistent() {
            let relation = relation(Policy::enabled());
            assert_eq!(
                relation
                    .get("nonexistent", Some(Value::from("default_value")))
                    .unwrap(),
                Value::from("default_value")
            );
            assert_eq!(relation.get("nonexistent", None).unwrap(), Value::UNDEFINED);
        }

        fn fqn_relation(quoting: Policy) -> Relation {
            Relation::new(
                AdapterType::Postgres,
                "MyDB".to_string(),
                "MySchema".to_string(),
                "MyTable".to_string(),
            )
            .with_quoting(quoting)
        }

        #[test]
        fn test_normalized_fqn_all_quoted() {
            let relation = fqn_relation(Policy {
                database: true,
                schema: true,
                identifier: true,
            });
            assert_eq!(relation.semantic_fqn(), "\"MyDB\".\"MySchema\".\"MyTable\"");
        }

        #[test]
        fn test_normalized_fqn_none_quoted() {
            let relation = fqn_relation(Policy {
                database: false,
                schema: false,
                identifier: false,
            });
            assert_eq!(relation.semantic_fqn(), "\"mydb\".\"myschema\".\"mytable\"");
        }

        #[test]
        fn test_normalized_fqn_mixed_quoted() {
            let relation = fqn_relation(Policy {
                database: true,
                schema: false,
                identifier: true,
            });
            assert_eq!(relation.semantic_fqn(), "\"MyDB\".\"myschema\".\"MyTable\"");
        }

        fn filter_relation() -> Relation {
            Relation::new(
                AdapterType::Postgres,
                "my_db".to_string(),
                "my_schema".to_string(),
                "my_table".to_string(),
            )
            .with_quoting(Policy::disabled())
        }

        #[test]
        fn test_render_with_run_filter_empty() {
            let run_filter = RunFilter {
                empty: true,
                sample: None,
            };
            let result = filter_relation().render_with_run_filter(&run_filter, &None);
            assert_eq!(
                result,
                "(select * from my_db.my_schema.my_table where false limit 0)"
            );
        }

        #[test]
        fn test_render_with_run_filter_no_sample() {
            let run_filter = RunFilter {
                empty: false,
                sample: None,
            };
            let event_time = Some("created_at".to_string());
            let result = filter_relation().render_with_run_filter(&run_filter, &event_time);
            assert_eq!(result, "my_db.my_schema.my_table");
        }

        #[test]
        fn test_render_with_run_filter_both_start_and_end() {
            let start = NaiveDate::from_ymd_opt(2024, 7, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap();
            let end = NaiveDate::from_ymd_opt(2024, 7, 8)
                .unwrap()
                .and_hms_opt(18, 0, 0)
                .unwrap();

            let sample = Sample {
                start: Some(DateTime::<Utc>::from_naive_utc_and_offset(start, Utc)),
                end: Some(DateTime::<Utc>::from_naive_utc_and_offset(end, Utc)),
            };

            let run_filter = RunFilter {
                empty: false,
                sample: Some(sample),
            };
            let event_time = Some("created_at".to_string());

            let result = filter_relation().render_with_run_filter(&run_filter, &event_time);
            assert_eq!(
                result,
                "(select * from my_db.my_schema.my_table where created_at >= '2024-07-01T00:00:00+00:00' and created_at < '2024-07-08T18:00:00+00:00')"
            );
        }

        #[test]
        fn test_render_with_run_filter_start_only() {
            let start = NaiveDate::from_ymd_opt(2024, 7, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap();

            let sample = Sample {
                start: Some(DateTime::<Utc>::from_naive_utc_and_offset(start, Utc)),
                end: None,
            };

            let run_filter = RunFilter {
                empty: false,
                sample: Some(sample),
            };
            let event_time = Some("created_at".to_string());

            let result = filter_relation().render_with_run_filter(&run_filter, &event_time);
            assert_eq!(
                result,
                "(select * from my_db.my_schema.my_table where created_at >= '2024-07-01T00:00:00+00:00')"
            );
        }

        #[test]
        fn test_render_with_run_filter_end_only() {
            let end = NaiveDate::from_ymd_opt(2024, 7, 8)
                .unwrap()
                .and_hms_opt(18, 0, 0)
                .unwrap();

            let sample = Sample {
                start: None,
                end: Some(DateTime::<Utc>::from_naive_utc_and_offset(end, Utc)),
            };

            let run_filter = RunFilter {
                empty: false,
                sample: Some(sample),
            };
            let event_time = Some("created_at".to_string());

            let result = filter_relation().render_with_run_filter(&run_filter, &event_time);
            assert_eq!(
                result,
                "(select * from my_db.my_schema.my_table where created_at < '2024-07-08T18:00:00+00:00')"
            );
        }

        #[test]
        fn test_render_with_run_filter_sample_none_values() {
            let sample = Sample {
                start: None,
                end: None,
            };

            let run_filter = RunFilter {
                empty: false,
                sample: Some(sample),
            };
            let event_time = Some("created_at".to_string());

            let result = filter_relation().render_with_run_filter(&run_filter, &event_time);
            assert_eq!(result, "my_db.my_schema.my_table");
        }

        #[test]
        fn test_render_with_run_filter_no_event_time_error() {
            let start = NaiveDate::from_ymd_opt(2024, 7, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap();

            let sample = Sample {
                start: Some(DateTime::<Utc>::from_naive_utc_and_offset(start, Utc)),
                end: None,
            };

            let run_filter = RunFilter {
                empty: false,
                sample: Some(sample),
            };

            let result = filter_relation().render_with_run_filter(&run_filter, &None);
            assert_eq!(result, "my_db.my_schema.my_table");
        }

        #[test]
        fn test_render_with_run_filter_empty_and_sample() {
            let start = NaiveDate::from_ymd_opt(2024, 7, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap();
            let end = NaiveDate::from_ymd_opt(2024, 7, 8)
                .unwrap()
                .and_hms_opt(18, 0, 0)
                .unwrap();

            let sample = Sample {
                start: Some(DateTime::<Utc>::from_naive_utc_and_offset(start, Utc)),
                end: Some(DateTime::<Utc>::from_naive_utc_and_offset(end, Utc)),
            };

            let run_filter = RunFilter {
                empty: true,
                sample: Some(sample),
            };
            let event_time = Some("created_at".to_string());

            let result = filter_relation().render_with_run_filter(&run_filter, &event_time);
            assert_eq!(
                result,
                "(select * from (select * from my_db.my_schema.my_table where false limit 0) where created_at >= '2024-07-01T00:00:00+00:00' and created_at < '2024-07-08T18:00:00+00:00')"
            );
        }

        /// Regression test: when microbatch calls `with_filter` (empty: false + batch window)
        /// on a RelationObject that already has `empty: true` from `--empty`, the empty flag
        /// must be preserved so the relation still renders as a zero-row subquery.
        #[test]
        fn test_with_filter_preserves_empty_flag() {
            let start = DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDate::from_ymd_opt(2026, 7, 13)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
                Utc,
            );
            let end = DateTime::<Utc>::from_naive_utc_and_offset(
                NaiveDate::from_ymd_opt(2026, 7, 14)
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap(),
                Utc,
            );

            let inner: Arc<dyn BaseRelation> = Arc::new(filter_relation());
            let obj = RelationObject::new_with_filter(
                inner,
                RunFilter {
                    empty: true,
                    sample: None,
                },
                Some("event_date".to_string()),
            );

            // Microbatch overwrites the filter with a batch window (empty: false)
            let batch_filter = RunFilter {
                empty: false,
                sample: Some(Sample {
                    start: Some(start),
                    end: Some(end),
                }),
            };
            let filtered = obj.with_filter(batch_filter, Some("event_date".to_string()));

            // The --empty zero-row wrapper must be preserved under the event-time filter
            let rendered = format!("{}", Value::from_object(filtered));
            assert_eq!(
                rendered,
                "(select * from (select * from my_db.my_schema.my_table where false limit 0) where event_date >= '2026-07-13T00:00:00+00:00' and event_date < '2026-07-14T00:00:00+00:00')"
            );
        }
    }

    mod databricks {
        use super::*;

        #[test]
        fn test_try_new_via_static_base_relation() {
            let relation_type = RelationStatic {
                adapter_type: AdapterType::Databricks,
                quoting: DEFAULT_RESOLVED_QUOTING,
            };
            let relation = relation_type
                .try_new(
                    Some("d".to_string()),
                    Some("s".to_string()),
                    Some("i".to_string()),
                    Some(RelationType::Table),
                    Some(DEFAULT_RESOLVED_QUOTING),
                    None,
                )
                .unwrap();

            let relation = relation.downcast_object::<RelationObject>().unwrap();
            assert_eq!(relation.inner().render_self_as_str(), "`d`.`s`.`i`");
            assert_eq!(relation.relation_type().unwrap(), RelationType::Table);
        }

        #[test]
        fn test_try_new_via_static_base_relation_with_default_database() {
            let relation_type = RelationStatic {
                adapter_type: AdapterType::Databricks,
                quoting: DEFAULT_RESOLVED_QUOTING,
            };
            let relation = relation_type
                .try_new(
                    None,
                    Some("s".to_string()),
                    Some("i".to_string()),
                    Some(RelationType::Table),
                    Some(DEFAULT_RESOLVED_QUOTING),
                    None,
                )
                .unwrap();

            let relation = relation.downcast_object::<RelationObject>().unwrap();
            assert_eq!(relation.inner().render_self_as_str(), "`s`.`i`");
        }

        #[test]
        fn test_render_lowercases_identifiers() {
            // Python DatabricksRelation.render() calls super().render().lower(),
            // lowercasing the entire rendered relation string.
            // Databricks backtick-quoted identifiers are case-insensitive, so
            // this is semantically correct and matches Mantle's behavior.
            let relation_type = RelationStatic {
                adapter_type: AdapterType::Databricks,
                quoting: DEFAULT_RESOLVED_QUOTING,
            };
            let relation = relation_type
                .try_new(
                    Some("dbt".to_string()),
                    Some("dbt_staging".to_string()),
                    Some("stg_pinterest_campaign_INT".to_string()),
                    Some(RelationType::Table),
                    Some(DEFAULT_RESOLVED_QUOTING),
                    None,
                )
                .unwrap();

            let relation = relation.downcast_object::<RelationObject>().unwrap();
            assert_eq!(
                relation.inner().render_self_as_str(),
                "`dbt`.`dbt_staging`.`stg_pinterest_campaign_int`"
            );
        }

        #[test]
        fn test_is_system() {
            let make = |db: Option<&str>, schema: Option<&str>| {
                Relation::new(
                    AdapterType::Databricks,
                    db.map(String::from),
                    schema.map(String::from),
                    Some("table".to_string()),
                )
                .with_relation_type(RelationType::Table)
                .with_quoting(DEFAULT_RESOLVED_QUOTING)
            };

            assert!(make(Some("system"), Some("schema")).is_system());
            assert!(make(Some("SYSTEM"), Some("schema")).is_system());
            assert!(make(Some("database"), Some("information_schema")).is_system());
            assert!(make(Some("database"), Some("INFORMATION_SCHEMA")).is_system());
            assert!(!make(Some("regular_database"), Some("regular_schema")).is_system());
            assert!(!make(None, Some("regular_schema")).is_system());
            assert!(!make(Some("regular_database"), None).is_system());
            assert!(make(Some("system"), Some("information_schema")).is_system());
        }

        #[test]
        fn test_constraint_methods() {
            use crate::relation::databricks::typed_constraint::TypedConstraint;

            let mut relation = Relation::new(
                AdapterType::Databricks,
                "test_db".to_string(),
                "test_schema".to_string(),
                "test_table".to_string(),
            )
            .with_relation_type(RelationType::Table)
            .with_quoting(DEFAULT_RESOLVED_QUOTING);

            // Test check constraint goes to alter_constraints
            let check_constraint = TypedConstraint::Check {
                name: Some("positive_id".to_string()),
                expression: "id > 0".to_string(),
                columns: None,
            };
            relation.add_constraint(check_constraint);
            assert_eq!(relation.alter_constraints.len(), 1);
            assert_eq!(relation.create_constraints.len(), 0);

            // Test primary key constraint goes to create_constraints
            let pk_constraint = TypedConstraint::PrimaryKey {
                name: Some("pk_users".to_string()),
                columns: vec!["id".to_string()],
                expression: None,
            };
            relation.add_constraint(pk_constraint);
            assert_eq!(relation.alter_constraints.len(), 1);
            assert_eq!(relation.create_constraints.len(), 1);
        }
    }

    mod bigquery {
        use super::*;

        #[test]
        fn test_try_new_via_static_base_relation() {
            let relation_type = RelationStatic {
                adapter_type: AdapterType::Bigquery,
                quoting: DEFAULT_RESOLVED_QUOTING,
            };
            let relation = relation_type
                .try_new(
                    Some("d".to_string()),
                    Some("s".to_string()),
                    Some("i".to_string()),
                    Some(RelationType::Table),
                    Some(DEFAULT_RESOLVED_QUOTING),
                    None,
                )
                .unwrap();

            let relation = relation.downcast_object::<RelationObject>().unwrap();
            assert_eq!(relation.inner().render_self_as_str(), "`d`.`s`.`i`");
            assert_eq!(relation.relation_type().unwrap(), RelationType::Table);
        }

        #[test]
        fn test_information_schema_with_database() {
            let relation = Relation::new(
                AdapterType::Bigquery,
                "test-db".to_string(),
                "test_schema".to_string(),
                "test_table".to_string(),
            )
            .with_relation_type(RelationType::Table)
            .with_quoting(DEFAULT_RESOLVED_QUOTING);

            let info_schema = relation.information_schema("TABLES").unwrap();
            let rendered = info_schema.render_self_as_str();
            assert_eq!(rendered, "`test-db`.test_schema.INFORMATION_SCHEMA.TABLES");

            let info_schema = relation.information_schema("COLUMNS").unwrap();
            let rendered = info_schema.render_self_as_str();
            assert_eq!(rendered, "`test-db`.test_schema.INFORMATION_SCHEMA.COLUMNS");

            let info_schema = relation.information_schema("TABLES").unwrap();
            let rendered = info_schema.render_self_as_str();
            assert_eq!(rendered, "`test-db`.test_schema.INFORMATION_SCHEMA.TABLES");
        }

        #[test]
        fn test_information_schema_quotes_project_identifier() {
            let relation = Relation::new(
                AdapterType::Bigquery,
                "my-project-1a".to_string(),
                "test_schema".to_string(),
                "test_table".to_string(),
            )
            .with_relation_type(RelationType::Table)
            .with_quoting(DEFAULT_RESOLVED_QUOTING);

            let info_schema = relation.information_schema("TABLES").unwrap();

            let rendered = info_schema.render_self_as_str();
            assert_eq!(
                rendered,
                "`my-project-1a`.test_schema.INFORMATION_SCHEMA.TABLES"
            );
        }

        #[test]
        fn test_object_privileges_quotes_project_identifier() {
            let relation = Relation::new(
                AdapterType::Bigquery,
                "my-project-1a".to_string(),
                "test_schema".to_string(),
                "test_table".to_string(),
            )
            .with_relation_type(RelationType::Table)
            .with_quoting(DEFAULT_RESOLVED_QUOTING)
            .with_location("US");

            let info_schema = relation.information_schema("OBJECT_PRIVILEGES").unwrap();

            let rendered = info_schema.render_self_as_str();
            assert_eq!(
                rendered,
                "`my-project-1a`.`region-US`.INFORMATION_SCHEMA.OBJECT_PRIVILEGES"
            );
        }

        #[test]
        fn test_information_schema_without_database() {
            let relation = Relation::new(
                AdapterType::Bigquery,
                None::<String>,
                "test_schema".to_string(),
                "test_table".to_string(),
            )
            .with_relation_type(RelationType::Table)
            .with_quoting(DEFAULT_RESOLVED_QUOTING);

            // Test TABLES view without database - uses dataset-level INFORMATION_SCHEMA
            let info_schema = relation.information_schema("TABLES").unwrap();

            let rendered = info_schema.render_self_as_str();
            assert_eq!(rendered, "test_schema.INFORMATION_SCHEMA.TABLES");
        }

        #[test]
        fn test_information_schema_with_dataset_and_region_prefer_region() {
            let relation = Relation::new(
                AdapterType::Bigquery,
                "test-project".to_string(),
                "test_dataset".to_string(),
                "test_relation".to_string(),
            )
            .with_relation_type(RelationType::Table)
            .with_quoting(DEFAULT_RESOLVED_QUOTING)
            .with_location("eu");

            // dataset or region view
            let info_schema = relation.information_schema("COLUMNS").unwrap();
            let rendered = info_schema.render_self_as_str();
            assert_eq!(
                rendered,
                "`test-project`.`region-eu`.INFORMATION_SCHEMA.COLUMNS"
            );
        }

        #[test]
        fn test_information_schema_with_dataset_and_region_prefer_dataset() {
            let relation = Relation::new(
                AdapterType::Bigquery,
                "test-project".to_string(),
                "test_dataset".to_string(),
                "test_relation".to_string(),
            )
            .with_relation_type(RelationType::Table)
            .with_quoting(DEFAULT_RESOLVED_QUOTING)
            .with_location("eu");

            // dataset-only view
            let info_schema = relation.information_schema("TABLE_SNAPSHOTS").unwrap();
            let rendered = info_schema.render_self_as_str();
            assert_eq!(
                rendered,
                "`test-project`.test_dataset.INFORMATION_SCHEMA.TABLE_SNAPSHOTS"
            );
        }

        /// See: https://github.com/dbt-labs/dbt-core/issues/15157
        #[test]
        fn test_information_schema_with_region_as_database_regression_15157() {
            let relation = Relation::new(
                AdapterType::Bigquery,
                "region-eu".to_string(),
                "test_schema".to_string(),
                "my_relation".to_string(),
            )
            .with_relation_type(RelationType::Table)
            .with_quoting(DEFAULT_RESOLVED_QUOTING);

            let info_schema = relation.information_schema("SCHEMATA").unwrap();
            let rendered = info_schema.render_self_as_str();
            assert_eq!(rendered, "`region-eu`.INFORMATION_SCHEMA.SCHEMATA");
        }
    }

    mod duckdb {
        use super::*;
        #[test]
        fn test_external_relation_renders_location() {
            let relation = Relation::new(
                AdapterType::DuckDB,
                "main".to_string(),
                "raw".to_string(),
                "orders".to_string(),
            )
            .with_quoting(DEFAULT_RESOLVED_QUOTING)
            .with_external("'data/RawOrders.csv'");

            assert_eq!(relation.render_self_as_str(), "'data/RawOrders.csv'");
        }

        fn path_from_db(db: Option<&str>) -> RelationPath {
            RelationPath {
                database: db.map(|db| db.to_string()),
                schema: Some("my_schema".to_string()),
                identifier: Some("my_table".to_string()),
            }
        }

        #[test]
        fn test_include_policy_for_attached_catalog() {
            assert!(
                include_policy(AdapterType::DuckDB, &path_from_db(Some("stocks_dev"))).database
            );
        }

        #[test]
        fn test_should_not_include_database_for_default_catalog() {
            assert!(!include_policy(AdapterType::DuckDB, &path_from_db(Some("main"))).database);
            assert!(!include_policy(AdapterType::DuckDB, &path_from_db(Some("memory"))).database);
            assert!(!include_policy(AdapterType::DuckDB, &path_from_db(Some("MEMORY"))).database);
            assert!(!include_policy(AdapterType::DuckDB, &path_from_db(Some(""))).database);
            assert!(!include_policy(AdapterType::DuckDB, &path_from_db(None)).database);
        }
    }

    mod snowflake {
        use super::*;

        #[test]
        fn test_create_via_static_base_relation() {
            let values = [
                Value::from("d"),
                Value::from("s"),
                Value::from("i"),
                Value::from("table"),
                Value::from("{database: true, identifier: true, schema: true}"),
            ];

            let relation = RelationStatic {
                adapter_type: AdapterType::Snowflake,
                quoting: DEFAULT_RESOLVED_QUOTING,
            }
            .create(&values)
            .unwrap();

            let relation = relation.downcast_object::<RelationObject>().unwrap();
            assert_eq!(relation.inner().render_self_as_str(), r#""d"."s"."i""#);
            assert_eq!(relation.relation_type().unwrap(), RelationType::Table);
        }

        fn snowflake_relation(relation_type: RelationType, table_format: TableFormat) -> Relation {
            Relation::new(
                AdapterType::Snowflake,
                "d".to_string(),
                "s".to_string(),
                "i".to_string(),
            )
            .with_relation_type(Some(relation_type))
            .with_table_format(table_format)
        }

        fn drop_decision(old: RelationType, target: RelationType) -> bool {
            drop_decision_with_formats((old, TableFormat::Default), (target, TableFormat::Default))
        }

        fn drop_decision_with_formats(
            old: (RelationType, TableFormat),
            target: (RelationType, TableFormat),
        ) -> bool {
            let old_relation: Arc<dyn BaseRelation> = Arc::new(snowflake_relation(old.0, old.1));
            snowflake_relation(target.0, target.1)
                .needs_to_drop(Some(old_relation))
                .unwrap()
        }

        #[test]
        fn needs_to_drop_matrix_interactive() {
            use RelationType::*;

            assert!(!drop_decision(InteractiveTable, InteractiveTable));
            assert!(drop_decision(InteractiveTable, Table));
            // Snowflake rejects `CREATE OR REPLACE INTERACTIVE TABLE` over a plain `TABLE`
            // with `001998`, and the two share a table format, so the format comparison alone
            // would wrongly report that no drop is needed.
            assert!(drop_decision(Table, InteractiveTable));
            assert!(drop_decision(InteractiveTable, DynamicTable));
            assert!(drop_decision(DynamicTable, InteractiveTable));
            assert!(drop_decision(View, InteractiveTable));
            assert!(drop_decision(InteractiveTable, View));
            assert!(!drop_decision(Table, Table));
        }

        /// Pins the decisions that predate interactive tables, none of which this matrix may
        /// change.
        #[test]
        fn needs_to_drop_preserves_non_interactive_decisions() {
            use RelationType::*;

            assert!(drop_decision(DynamicTable, DynamicTable));
            assert!(drop_decision(View, Table));
            assert!(drop_decision(View, View));
            assert!(drop_decision(DynamicTable, Table));
            assert!(!drop_decision(Table, View));
            assert!(!drop_decision(Table, DynamicTable));
            // table format change in either direction still drops
            assert!(drop_decision_with_formats(
                (Table, TableFormat::Default),
                (Table, TableFormat::Iceberg)
            ));
            assert!(drop_decision_with_formats(
                (Table, TableFormat::Iceberg),
                (Table, TableFormat::Default)
            ));
        }

        #[test]
        fn needs_to_drop_without_old_relation_never_drops() {
            assert!(
                !snowflake_relation(RelationType::InteractiveTable, TableFormat::Default)
                    .needs_to_drop(None)
                    .unwrap()
            );
        }

        #[test]
        fn interactive_table_can_be_renamed_and_replaced() {
            let interactive =
                snowflake_relation(RelationType::InteractiveTable, TableFormat::Default);
            assert!(interactive.can_be_renamed());
            assert!(interactive.can_be_replaced());

            let iceberg = snowflake_relation(RelationType::InteractiveTable, TableFormat::Iceberg);
            assert!(!iceberg.can_be_renamed());
            assert!(iceberg.can_be_replaced());
        }

        #[test]
        fn incorporate_preserves_table_format() {
            // `incorporate` (e.g. `target_relation.incorporate(type='table')` in the
            // incremental materialization) rebuilds the relation via `create_relation`,
            // which used to default `table_format` back to `Default` and silently
            // turn Iceberg models into non-Iceberg ones for anything computed from the
            // incorporated relation, e.g. `persist_docs`' `alter_relation_comment`
            // (fs#14268).
            let iceberg: Arc<dyn BaseRelation> = Arc::new(snowflake_relation(
                RelationType::Table,
                TableFormat::Iceberg,
            ));
            let incorporated = iceberg
                .incorporate(None, Some(RelationType::Table), None)
                .unwrap();
            assert!(incorporated.is_iceberg_format());
        }
    }

    mod snowflake_interactive_target_lag {
        use super::*;
        use crate::relation::config_v2::{ComponentConfigChange, RelationComponentConfigChangeSet};
        use crate::relation::snowflake::config::relation_types::interactive_table;
        use crate::relation::snowflake::config::test_helpers::{
            TestDynamicTableConfig, TestRemoteState, make_local_config,
            make_remote_interactive_state, make_remote_state,
        };
        use dbt_agate::AgateTable;
        use minijinja::value::ValueMap;

        fn states(
            local_target_lag: Option<&'static str>,
            remote_target_lag: Option<&str>,
        ) -> (RelationConfig, RelationConfig) {
            let loader = interactive_table::new_loader();
            let local = make_local_config(TestDynamicTableConfig {
                target_lag: local_target_lag,
                snowflake_warehouse: local_target_lag.map(|_| "WH"),
                ..Default::default()
            });
            let remote = make_remote_interactive_state(TestRemoteState {
                target_lag: remote_target_lag.map(|s| s.to_owned()),
                refresh_warehouse: remote_target_lag.map(|_| "WH".to_owned()),
                ..Default::default()
            });
            (
                loader.from_local_config(&local).unwrap(),
                loader.from_remote_state(&remote).unwrap(),
            )
        }

        /// A static interactive table gaining a `target_lag` has to be rebuilt: Snowflake
        /// rejects the in-place `ALTER ... SET TARGET_LAG` with `001420`.
        #[test]
        fn target_lag_added_forces_full_refresh_at_the_changeset_layer() {
            let (desired, current) = states(Some("1 hour"), None);

            let changeset = RelationConfig::diff(&desired, &current);
            assert!(!changeset.requires_full_refresh());

            assert!(target_lag_newly_set(&desired, &current));
            assert!(changeset.forcing_full_refresh().requires_full_refresh());
        }

        #[test]
        fn target_lag_value_change_is_not_newly_set() {
            let (desired, current) = states(Some("2 hours"), Some("1 hour"));
            assert!(!target_lag_newly_set(&desired, &current));
            assert!(!RelationConfig::diff(&desired, &current).requires_full_refresh());
        }

        #[test]
        fn target_lag_unchanged_is_not_newly_set() {
            let (desired, current) = states(Some("1 hour"), Some("1 hour"));
            assert!(!target_lag_newly_set(&desired, &current));
        }

        #[test]
        fn target_lag_removed_is_not_newly_set() {
            let (desired, current) = states(None, Some("1 hour"));
            assert!(!target_lag_newly_set(&desired, &current));
            assert!(RelationConfig::diff(&desired, &current).requires_full_refresh());
        }

        #[test]
        fn both_sides_static_is_not_newly_set() {
            let (desired, current) = states(None, None);
            assert!(!target_lag_newly_set(&desired, &current));
        }

        #[test]
        fn config_relation_type_follows_the_materialization() {
            let mut model = make_local_config(TestDynamicTableConfig::default());

            assert_eq!(model.materialized(), DbtMaterialization::DynamicTable);
            assert_eq!(
                SnowflakeConfigRelationType::of(&model),
                SnowflakeConfigRelationType::DynamicTable
            );

            model.__base_attr__.materialized = DbtMaterialization::InteractiveTable;
            assert_eq!(
                SnowflakeConfigRelationType::of(&model),
                SnowflakeConfigRelationType::InteractiveTable
            );
        }

        #[test]
        fn describe_result_keys_match_the_describe_methods() {
            assert_eq!(
                SnowflakeConfigRelationType::DynamicTable.describe_result_key(),
                DYNAMIC_TABLE_KEY
            );
            assert_eq!(
                SnowflakeConfigRelationType::InteractiveTable.describe_result_key(),
                INTERACTIVE_TABLE_KEY
            );
        }

        /// `minijinja_value_to_typed_struct` round-trips a serialized `DbtModel` through YAML and
        /// drops `config`, so the Jinja entry point can't be driven from a Rust-level `Value`.
        #[test]
        fn interactive_table_config_changeset_uses_the_interactive_loader_and_forces_refresh() {
            let mut local_either_loader_can_read = make_local_config(TestDynamicTableConfig {
                target_lag: Some("1 hour"),
                snowflake_warehouse: Some("WH"),
                refresh_mode: Some("FULL"),
                ..Default::default()
            });
            local_either_loader_can_read.__base_attr__.materialized =
                DbtMaterialization::InteractiveTable;

            let batch_either_loader_can_read = make_remote_state(TestRemoteState {
                target_lag: Some("1 hour".to_owned()),
                refresh_warehouse: Some("WH".to_owned()),
                ..Default::default()
            })
            .record_batch;
            let relation_results_value_either_loader_can_read = Value::from(ValueMap::from([(
                Value::from(INTERACTIVE_TABLE_KEY),
                Value::from_object(AgateTable::from_record_batch(batch_either_loader_can_read)),
            )]));

            let result_either_loader_can_read = dynamic_table_config_changeset_from_local_config(
                &local_either_loader_can_read,
                &relation_results_value_either_loader_can_read,
            )
            .expect("both the interactive and the dynamic loader can read this batch cleanly");
            let changeset_either_loader_can_read = result_either_loader_can_read
                .downcast_object::<RelationComponentConfigChangeSet>()
                .expect(
                    "snowflake_warehouse differs between the desired \"WH\" and the unset \
                     remote reading, which always produces a diff",
                );
            assert!(
                matches!(
                    changeset_either_loader_can_read.get(components::refresh_mode::TYPE_NAME),
                    ComponentConfigChange::None
                ),
                "refresh_mode is a dynamic-table-only component; it must never appear in an \
                 interactive-table changeset. If this fails, the interactive-table branch of \
                 `SnowflakeConfigRelationType::loader()` is selecting the dynamic loader."
            );

            let mut local = make_local_config(TestDynamicTableConfig {
                target_lag: Some("1 hour"),
                snowflake_warehouse: Some("WH"),
                ..Default::default()
            });
            local.__base_attr__.materialized = DbtMaterialization::InteractiveTable;

            // An 8-column `SHOW INTERACTIVE TABLES` batch, with no `target_lag` yet — the
            // static -> dynamic transition this test is pinning.
            let batch = make_remote_interactive_state(TestRemoteState::default()).record_batch;
            let relation_results_value = Value::from(ValueMap::from([(
                Value::from(INTERACTIVE_TABLE_KEY),
                Value::from_object(AgateTable::from_record_batch(batch)),
            )]));

            let result = dynamic_table_config_changeset_from_local_config(
                &local,
                &relation_results_value,
            )
            .expect(
                "the interactive loader must load an 8-column SHOW INTERACTIVE TABLES batch \
                 cleanly; a wrong loader selection here reads columns the batch doesn't have",
            );
            let changeset = result
                .downcast_object::<RelationComponentConfigChangeSet>()
                .expect("target_lag going from None to Some always produces a diff");

            assert!(changeset.requires_full_refresh());
        }
    }

    mod clickhouse {
        use super::*;

        #[test]
        fn test_try_new_via_static_base_relation_normalizes_database_to_empty_string() {
            let relation_type = RelationStatic {
                adapter_type: AdapterType::ClickHouse,
                quoting: DEFAULT_RESOLVED_QUOTING,
            };

            let from_none = relation_type
                .try_new(
                    None,
                    Some("my_schema".to_string()),
                    Some("my_table".to_string()),
                    Some(RelationType::Table),
                    Some(DEFAULT_RESOLVED_QUOTING),
                    None,
                )
                .unwrap();
            let from_none = from_none.downcast_object::<RelationObject>().unwrap();
            assert_eq!(from_none.inner().database(), Some(""));
            assert_eq!(
                from_none.inner().render_self_as_str(),
                "`my_schema`.`my_table`"
            );

            let from_supplied = relation_type
                .try_new(
                    Some("ignored_db".to_string()),
                    Some("my_schema".to_string()),
                    Some("my_table".to_string()),
                    Some(RelationType::Table),
                    Some(DEFAULT_RESOLVED_QUOTING),
                    None,
                )
                .unwrap();
            let from_supplied = from_supplied.downcast_object::<RelationObject>().unwrap();
            assert_eq!(from_supplied.inner().database(), Some(""));
            assert_eq!(
                from_supplied.inner().render_self_as_str(),
                "`my_schema`.`my_table`"
            );
        }
    }
}
