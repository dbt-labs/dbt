use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    sync::Arc,
};

use crate::{
    AdapterResult, AdapterType,
    errors::{AdapterError, AdapterErrorKind},
    sql_types::TypeOps,
};
use arrow::array::RecordBatch;
use arrow_schema::{DataType, Field, Schema};
use chrono::{DateTime, Utc};
use dbt_adapter_engine::{ConnectionFactory, MapReduce};
use dbt_adbc::Connection;
use dbt_common::AsyncAdapterResult;
use dbt_common::cancellation::{Cancellable, CancellationToken};
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_tracing::emit::create_debug_span;
use dbt_tracing::span_info::SpanStatusRecorder as _;
use minijinja::State;

pub const SCHEMA_CACHE_OP_ID: &str = "hydrate_schema_cache";

/// Run one schema-cache fetch under a DEBUG-level progress item span.
///
/// `operation_id` must match the parent `GenericOpExecuted` span's operation_id so the
/// TUI layer correlates this item with the right progress bar.
fn with_schema_cache_item_span<T>(
    operation_id: &str,
    target: &str,
    fetch: impl FnOnce() -> AdapterResult<T>,
) -> AdapterResult<T> {
    let span = create_debug_span(dbt_telemetry::GenericOpItemProcessed::new(
        operation_id.to_string(),
        "downloading".to_string(),
        "downloaded".to_string(),
        target.to_string(),
    ));
    let _guard = span.enter();
    fetch().record_status(&span)
}

trait SchemaCacheKey {
    fn schema_cache_target(&self) -> String;
}

impl SchemaCacheKey for Arc<dyn BaseRelation> {
    fn schema_cache_target(&self) -> String {
        self.semantic_fqn()
    }
}

impl SchemaCacheKey for (String, String) {
    fn schema_cache_target(&self) -> String {
        self.0.clone()
    }
}

#[allow(clippy::type_complexity)]
fn run_schema_cache_map_reduce<K>(
    factory: Box<dyn ConnectionFactory<Error = Cancellable<AdapterError>>>,
    keys: Vec<K>,
    item_span_operation_id: Option<&str>,
    map_f: impl Fn(&mut dyn Connection, &K) -> AdapterResult<Arc<Schema>> + Send + Sync + 'static,
    reduce_f: impl Fn(
        &mut HashMap<String, AdapterResult<Arc<Schema>>>,
        K,
        AdapterResult<Arc<Schema>>,
    ) -> Result<(), Cancellable<AdapterError>>
    + Send
    + Sync
    + 'static,
    node_id: Option<String>,
    token: CancellationToken,
) -> AsyncAdapterResult<'static, HashMap<String, AdapterResult<Arc<Schema>>>>
where
    K: SchemaCacheKey + Clone + Send + Sync + 'static,
{
    // Callers that don't own a parent `GenericOpExecuted` span (item_span_operation_id ==
    // None) get no item span at all, rather than being silently attributed to the schema
    // hydration bar's operation_id.
    let map_f: Box<dyn Fn(&mut dyn Connection, &K) -> AdapterResult<Arc<Schema>> + Send + Sync> =
        match item_span_operation_id {
            Some(operation_id) => {
                let operation_id = operation_id.to_string();
                Box::new(move |conn: &mut dyn Connection, key: &K| {
                    let target = key.schema_cache_target();
                    with_schema_cache_item_span(&operation_id, &target, || map_f(conn, key))
                })
            }
            None => Box::new(map_f),
        };
    MapReduce::new(factory, map_f, Box::new(reduce_f), node_id).run(Arc::new(keys), token)
}

pub(crate) mod bigquery;
pub(crate) mod clickhouse;
pub mod databricks;
pub(crate) mod duckdb;
pub(crate) mod exasol;
pub(crate) mod fabric;
pub(crate) mod freshness_overrides;
pub(crate) mod metadata_adapter;
pub(crate) mod postgres;
pub(crate) mod redshift;
pub(crate) mod salesforce;
pub mod snowflake; // XXX: temporarily pub before the refactor is complete
pub(crate) mod spark;
pub(crate) mod view_definition;

// Re-export `metadata_adapter` symbols
// NOTE: this is temporary until all the metadata-releated code
// is verticalized and moved to the metadata module.
pub use metadata_adapter::*;
pub use view_definition::{ViewDefinition, ViewDefinitionFetchResult};

/// The canonical list of BigQuery pseudocolumns (queryable columns absent from
/// `INFORMATION_SCHEMA`). Re-exported so other crates can share the source of truth.
pub use bigquery::BIGQUERY_PSEUDOCOLUMNS;

/// Implementation of the `get_relation` function for all adapters.
pub(crate) mod get_relation;
pub(crate) mod list_objects;

pub const ARROW_FIELD_COMMENT_METADATA_KEY: &str = "comment";
// XXX: use original_type_string() instead of querying for this constant
pub const ARROW_FIELD_ORIGINAL_TYPE_METADATA_KEY: &str = "type_text";

pub type WhereClausesByDb = BTreeMap<String, Vec<String>>;
pub type RelationsByDb = BTreeMap<String, Vec<Arc<dyn BaseRelation>>>;

/// The two ways of representing a relation in a pair.
pub type RelationSchemaPair = (Arc<dyn BaseRelation>, Arc<Schema>);

/// A collection of relations
pub type RelationVec = Vec<Arc<dyn BaseRelation>>;

/// A struct representing a catalog and a schema
#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct CatalogAndSchema {
    pub rendered_catalog: String,
    pub rendered_schema: String,
    pub resolved_catalog: String,
    pub resolved_schema: String,
}

impl fmt::Display for CatalogAndSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.resolved_catalog.is_empty() {
            write!(f, "{}", self.rendered_schema)
        } else if self.resolved_schema.is_empty() {
            write!(f, "{}", self.rendered_catalog)
        } else {
            write!(f, "{}.{}", self.rendered_catalog, self.rendered_schema)
        }
    }
}

impl From<&dyn BaseRelation> for CatalogAndSchema {
    fn from(relation: &dyn BaseRelation) -> Self {
        let resolved_catalog = relation.database_as_resolved_str().unwrap_or_default();
        let rendered_catalog = if resolved_catalog.is_empty() {
            "".to_string()
        } else {
            relation.quoted(&resolved_catalog)
        };

        let resolved_schema = relation.schema_as_resolved_str().unwrap_or_default();
        let rendered_schema = if resolved_schema.is_empty() {
            "".to_string()
        } else {
            relation.quoted(&resolved_schema)
        };

        Self {
            rendered_catalog,
            rendered_schema,
            resolved_catalog,
            resolved_schema,
        }
    }
}

impl From<&Box<dyn BaseRelation>> for CatalogAndSchema {
    fn from(relation: &Box<dyn BaseRelation>) -> Self {
        CatalogAndSchema::from(relation.as_ref())
    }
}

impl From<&Arc<dyn BaseRelation>> for CatalogAndSchema {
    fn from(relation: &Arc<dyn BaseRelation>) -> Self {
        CatalogAndSchema::from(relation.as_ref())
    }
}

/// Stores freshness information for a source
pub struct MetadataFreshness {
    pub last_altered: DateTime<Utc>,
    pub is_view: bool,
}

/// Per-source freshness override. Mirrors the dbt-core plugin's `loaded_at_query` /
/// `loaded_at_field` handling. When set on a source, the metadata adapter must
/// execute a targeted query for that source instead of including it in the bulk
/// INFORMATION_SCHEMA freshness scan.
#[derive(Clone, Debug)]
pub enum FreshnessOverride {
    /// Custom SQL returning a single timestamp scalar. `{{ this }}` (or `{{this}}`)
    /// is substituted with the source's rendered FQN before execution.
    Query(String),
    /// Column name. The metadata adapter runs `SELECT max({field}) FROM {relation}`.
    Field(String),
}

impl MetadataFreshness {
    /// Create from seconds
    pub fn from_secs(timestamp: i64, is_view: bool) -> AdapterResult<Self> {
        let last_altered = DateTime::from_timestamp(timestamp, 0).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                format!("Invalid timestamp in seconds: {timestamp}"),
            )
        })?;

        Ok(Self {
            last_altered,
            is_view,
        })
    }

    /// Create from milliseconds
    pub fn from_millis(timestamp: i64, is_view: bool) -> AdapterResult<Self> {
        let last_altered = DateTime::from_timestamp_millis(timestamp).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                format!("Invalid timestamp in milliseconds: {timestamp}"),
            )
        })?;

        Ok(Self {
            last_altered,
            is_view,
        })
    }

    /// Create from microseconds
    pub fn from_micros(timestamp: i64, is_view: bool) -> AdapterResult<Self> {
        let last_altered = DateTime::from_timestamp_micros(timestamp).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                format!("Invalid timestamp in microseconds: {timestamp}"),
            )
        })?;

        Ok(Self {
            last_altered,
            is_view,
        })
    }

    /// Create from nanoseconds
    pub fn from_nanos(timestamp: i64, is_view: bool) -> AdapterResult<Self> {
        let last_altered = DateTime::from_timestamp_nanos(timestamp);

        Ok(Self {
            last_altered,
            is_view,
        })
    }
}

/// Allows serializing record batches into maps and Arrow schemas
pub trait MetadataProcessor {
    // Implementers can choose the map key/value
    type Key: Ord + Clone;
    type Value: Clone;

    fn into_metadata(self) -> BTreeMap<Self::Key, Self::Value>;
    fn from_record_batch(batch: Arc<RecordBatch>) -> AdapterResult<Self>
    where
        Self: Sized;
    fn to_arrow_schema(&self, type_ops: &dyn TypeOps) -> AdapterResult<Arc<Schema>>;
}

/// This represents a UDF downloaded from a remote data warehouse
#[derive(Debug, Clone)]
pub struct UDF {
    pub name: String,
    pub description: String,
    pub signature: String,
    pub adapter_type: AdapterType,
    pub kind: UDFKind,
}

#[derive(Debug, Clone, Copy)]
pub enum UDFKind {
    Scalar,
    Aggregate,
    Table,
}

/// Map a cell from a two-value array of strings into a boolean
///
/// Postcondition: either true or false or an propogate error for unexpected values
pub fn try_canonicalize_bool_column_field(column_value: &str) -> Result<bool, AdapterError> {
    const TRUTH_VALUES: [&str; 3] = ["1", "y", "yes"];
    const FALSE_VALUES: [&str; 3] = ["0", "n", "no"];

    if TRUTH_VALUES
        .iter()
        .any(|s| column_value.eq_ignore_ascii_case(s))
    {
        return Ok(true);
    }
    if FALSE_VALUES
        .iter()
        .any(|s| column_value.eq_ignore_ascii_case(s))
    {
        return Ok(false);
    }

    Err(AdapterError::new(
        AdapterErrorKind::UnexpectedResult,
        format!("Cannot convert unexpected column value '{column_value}' to boolean."),
    ))
}

// NOTE: being deprecated in favor of `make_arrow_field`
pub fn new_arrow_field_with_metadata(
    col_name: &str,
    data_type: DataType,
    nullable: bool,
    original_type_text: Option<String>,
    comment: Option<String>,
) -> Field {
    let field = Field::new(col_name, data_type, nullable);

    let mut metadata = HashMap::new();
    if let Some(original_type_text) = original_type_text {
        metadata.insert(
            ARROW_FIELD_ORIGINAL_TYPE_METADATA_KEY.to_string(),
            original_type_text,
        );
    }
    if let Some(comment) = comment {
        metadata.insert(ARROW_FIELD_COMMENT_METADATA_KEY.to_string(), comment);
    }
    field.with_metadata(metadata)
}

pub fn get_input_schema_database_and_table(
    relation: &Arc<dyn BaseRelation>,
) -> AdapterResult<(String, String, String)> {
    let table_name = relation.semantic_fqn();

    let parts: Vec<&str> = table_name.split('.').collect();
    if parts.len() != 3 {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!("Invalid table name format: {table_name}"),
        ));
    }
    // database will be used as an identifier
    let database = table_name.split('.').next().ok_or_else(|| {
        AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "relation database should not be None",
        )
    })?;

    // schema and table will be used as string literals
    let input_schema = relation.schema_as_resolved_str().map_err(|_| {
        AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "relation schema should not be None",
        )
    })?;
    let input_table = relation.identifier_as_resolved_str().map_err(|e| {
        AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!("relation identifier should not be None: {e}"),
        )
    })?;

    Ok((input_schema, database.to_owned(), input_table))
}

/// Builds and returns ([WhereClausesByDb], [RelationsByDb]) from a list of [BaseRelation]
/// [WhereClausesByDb] maps databases to statements that select their schema+tables in the relation
/// [RelationsByDb] keys the database to the cloned [BaseRelation]
/// We expect a fqn from the relation in format <database>.<schema>.<table>
pub fn build_relation_clauses(
    relations: &[Arc<dyn BaseRelation>],
) -> AdapterResult<(WhereClausesByDb, RelationsByDb)> {
    // Build the where clause for all relations grouped by databases
    let mut where_clauses_by_database = BTreeMap::new();
    let mut relations_by_database = BTreeMap::new();
    for relation in relations {
        let (input_schema, database, input_table) = get_input_schema_database_and_table(relation)?;

        where_clauses_by_database
            .entry(database.to_owned())
            .or_insert_with(Vec::new)
            .push(format!(
                "table_schema = '{input_schema}' and table_name = '{input_table}'"
            ));
        relations_by_database
            .entry(database.to_owned())
            .or_insert_with(Vec::new)
            .push(relation.clone());
    }
    Ok((where_clauses_by_database, relations_by_database))
}

pub fn find_matching_relation(
    schema: &str,
    table: &str,
    relations: &[Arc<dyn BaseRelation>],
) -> AdapterResult<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    // Find the matching relation
    for relation in relations {
        let table_name = relation.semantic_fqn();
        // schema and table will be used as string literals
        let input_schema = relation.schema_as_resolved_str().map_err(|_| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                "relation schema should not be None",
            )
        })?;
        let input_table = relation.identifier_as_resolved_str().map_err(|_| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                "relation identifier should not be None",
            )
        })?;
        if schema == input_schema && table == input_table {
            out.insert(table_name);
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::Relation;
    use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;
    use dbt_test_primitives::assert_contains;

    #[test]
    fn test_build_relation_clauses() {
        let relations = vec![
            Arc::new(
                Relation::new(
                    AdapterType::Snowflake,
                    "db1".to_string(),
                    "schema1".to_string(),
                    "table1".to_string(),
                )
                .with_quoting(DEFAULT_RESOLVED_QUOTING),
            ) as Arc<dyn BaseRelation>,
            Arc::new(
                Relation::new(
                    AdapterType::Snowflake,
                    "db1".to_string(),
                    "schema2".to_string(),
                    "table2".to_string(),
                )
                .with_quoting(DEFAULT_RESOLVED_QUOTING),
            ) as Arc<dyn BaseRelation>,
            Arc::new(
                Relation::new(
                    AdapterType::Snowflake,
                    "db2".to_string(),
                    "schema1".to_string(),
                    "table3".to_string(),
                )
                .with_quoting(DEFAULT_RESOLVED_QUOTING),
            ) as Arc<dyn BaseRelation>,
        ];

        let (where_clauses, relations_by_db) = build_relation_clauses(&relations).unwrap();

        // Test where clauses
        assert_eq!(where_clauses.len(), 2);
        assert_eq!(
            where_clauses.get("\"db1\"").unwrap(),
            &vec![
                "table_schema = 'schema1' and table_name = 'table1'",
                "table_schema = 'schema2' and table_name = 'table2'"
            ]
        );
        assert_eq!(
            where_clauses.get("\"db2\"").unwrap(),
            &vec!["table_schema = 'schema1' and table_name = 'table3'"]
        );

        // Test relations by database
        assert_eq!(relations_by_db.len(), 2);
        assert_eq!(relations_by_db.get("\"db1\"").unwrap().len(), 2);
        assert_eq!(relations_by_db.get("\"db2\"").unwrap().len(), 1);
    }

    #[test]
    fn test_build_relation_clauses_invalid_fqn() {
        let relations = vec![Arc::new(
            Relation::new(
                AdapterType::Snowflake,
                "invalid.fqn".to_string(), // This will cause an error as it contains a dot
                "schema1".to_string(),
                "table1".to_string(),
            )
            .with_quoting(DEFAULT_RESOLVED_QUOTING),
        ) as Arc<dyn BaseRelation>];

        let result = build_relation_clauses(&relations);
        assert!(result.is_err());
        assert_contains!(result.unwrap_err().to_string(), "Invalid table name format");
    }

    #[test]
    fn test_find_matching_relation() {
        let relations = vec![
            Arc::new(
                Relation::new(
                    AdapterType::Snowflake,
                    "db1".to_string(),
                    "schema1".to_string(),
                    "table1".to_string(),
                )
                .with_quoting(DEFAULT_RESOLVED_QUOTING),
            ) as Arc<dyn BaseRelation>,
            Arc::new(
                Relation::new(
                    AdapterType::Snowflake,
                    "db1".to_string(),
                    "schema2".to_string(),
                    "table2".to_string(),
                )
                .with_quoting(DEFAULT_RESOLVED_QUOTING),
            ) as Arc<dyn BaseRelation>,
        ];

        let result = find_matching_relation("schema1", "table1", &relations).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains("\"db1\".\"schema1\".\"table1\""));

        let result = find_matching_relation("schema2", "table2", &relations).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains("\"db1\".\"schema2\".\"table2\""));

        let result = find_matching_relation("nonexistent", "table", &relations).unwrap();
        assert_eq!(result.len(), 0);
    }
}
