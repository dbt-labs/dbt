use std::collections::{BTreeMap, HashMap};
use std::future;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::OnceLock;

use arrow_array::*;
use arrow_schema::{Field, Schema};
use dbt_adapter_core::AdapterType;
use dbt_adapter_core::ExecutionPhase;
use dbt_adapter_engine::MapReduce;
use dbt_adapter_engine::cow::uppercase;
use dbt_adapter_engine::databricks::query_tags::QueryTags;
use dbt_adapter_sql::ident::{Ident, quote_identifier};
use dbt_adbc::QueryCtx;
use dbt_agate::AgateTable;
use dbt_common::ErrorCode;
use dbt_common::cancellation::Cancellable;
use dbt_common::cancellation::CancellationToken;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_frontend_common::{Dialect, FullyQualifiedName};
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::legacy_catalog::{
    CatalogNodeStats, CatalogTable, ColumnMetadata, TableMetadata,
};
use dbt_schemas::schemas::relations::base::{BaseRelation, RelationPattern};
use indexmap::IndexMap;
use minijinja::value::Object;
use minijinja::{State, Value};
use regex::Regex;
use std::sync::LazyLock;

use crate::adapter::adapter_impl::AdapterImpl;
use crate::connection::{AdapterConnectionFactory, ConnectionGuard};
use crate::errors::{AdapterError, AdapterResult, AsyncAdapterResult};
use crate::metadata::CatalogAndSchema;
use crate::metadata::databricks::dbr_capabilities::DbrComputeContext;
use crate::metadata::databricks::describe_json::{
    ColumnMaskRow, DatabricksDescribeJsonMetadata, ForeignKeyConstraintRow, NonNullConstraintRow,
    PrimaryKeyConstraintRow, RowFilterRow, ViewDescriptionRow, fetch_json_metadata,
    is_describe_as_json_supported, parse_row_filter, parse_view_description,
};
use crate::metadata::databricks::describe_table::DatabricksTableMetadata;
use crate::metadata::databricks::version::EngineVersion;
use crate::metadata::freshness_overrides::{
    FreshnessTask, FreshnessTaskResult, apply_freshness_task_result, run_override_query,
};
use crate::metadata::get_relation::table_format_from_provider;
use crate::metadata::*;
use crate::query_ctx::query_ctx_from_state;
use crate::relation::Relation;
use crate::relation::config_v2::RelationConfig;
use crate::relation::databricks::config::{
    DatabricksRelationMetadata, DatabricksRelationMetadataKey, components,
};
use crate::relation::databricks::is_shallow_clone_type;
use crate::sql_types::{TypeOps, make_arrow_field_v2};
use crate::time_machine::{args_freshness_with_overrides, with_time_machine_metadata_wrapper};
use crate::{AdapterEngine, AdapterResponse};
use dbt_adapter_engine::record_batch::RecordBatchExt;

pub mod dbr_capabilities;
pub mod describe_json;
pub mod describe_table;
mod refresh_wait;
pub mod schemas;
pub(crate) mod version;

/// Only this Databricks driver error permits a plain DESCRIBE retry.
pub(crate) fn is_unsupported_json_describe(error: &AdapterError) -> bool {
    error.kind() == AdapterErrorKind::Driver
        && error
            .message()
            .to_ascii_uppercase()
            .contains("UNSUPPORTED_FEATURE.DESC_JSON_TABLE_TYPE")
}

pub(crate) fn describe_fallback_error(
    json_error: AdapterError,
    plain_error: AdapterError,
) -> AdapterError {
    AdapterError::new_with_sqlstate_and_vendor_code(
        json_error.kind(),
        format!(
            "{}; plain DESCRIBE fallback also failed: {plain_error}",
            json_error.message()
        ),
        json_error
            .sqlstate()
            .as_bytes()
            .try_into()
            .unwrap_or([b'0'; 5]),
        json_error.vendor_code(),
    )
}

/// `DESCRIBE TABLE EXTENDED` of a relation, as JSON where the runtime supports it. The flag is
/// `true` when the plain output was used. `None` when the relation does not exist.
#[allow(clippy::too_many_arguments)]
pub(crate) fn describe_relation_batch(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &mut ConnectionGuard<'_>,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<(RecordBatch, bool)>> {
    match describe_relation(
        adapter, state, ctx, conn, database, schema, identifier, token,
    ) {
        Ok(described) => Ok(Some(described)),
        Err(e) if is_relation_missing(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether a describe error means there is nothing to describe: the relation, or its catalog or
/// schema, is missing.
fn is_relation_missing(e: &AdapterError) -> bool {
    e.to_string().contains("cannot be found")
        || e.to_string().contains("TABLE_OR_VIEW_NOT_FOUND")
        || is_databricks_relation_not_found(e)
}

#[allow(clippy::too_many_arguments)]
fn describe_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &mut ConnectionGuard<'_>,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<(RecordBatch, bool)> {
    use crate::relation::databricks::{INFORMATION_SCHEMA_SCHEMA, SYSTEM_DATABASE};

    // This function is called when relation lookup needs full metadata.
    // See https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/impl.py#L418
    let query_catalog = databricks_query_identifier(database, adapter.quoting().database);
    let query_schema = databricks_query_identifier(schema, adapter.quoting().schema);
    let query_identifier = databricks_query_identifier(identifier, adapter.quoting().identifier);

    // Known system tables and older runtimes cannot use JSON describe. Spark
    // has no Databricks runtime version to check.
    let dbr_version = match adapter.adapter_type() {
        AdapterType::Spark => None,
        AdapterType::Databricks => Some(DatabricksMetadataAdapter::get_engine_version(
            adapter,
            ctx,
            conn,
            token.clone(),
        )?),
        _ => unreachable!(),
    };
    let is_external_system = database.eq_ignore_ascii_case(SYSTEM_DATABASE)
        && schema.eq_ignore_ascii_case(INFORMATION_SCHEMA_SCHEMA);
    let as_json_unsupported = is_external_system
        || dbr_version
            .map(|v| v < EngineVersion::Full(16, 2))
            .unwrap_or(false);

    let fqn = if database.is_empty() {
        format!("{query_schema}.{query_identifier}")
    } else {
        format!("{query_catalog}.{query_schema}.{query_identifier}")
    };
    let plain_sql = format!("DESCRIBE TABLE EXTENDED {fqn}");
    if as_json_unsupported {
        return adapter
            .engine()
            .execute(Some(state), conn.as_mut(), ctx, &plain_sql, token)
            .map(|batch| (batch, true));
    }

    let json_sql = format!("{plain_sql} AS JSON");
    if adapter.adapter_type() != AdapterType::Databricks {
        return adapter
            .engine()
            .execute(Some(state), conn.as_mut(), ctx, &json_sql, token)
            .map(|batch| (batch, false));
    }

    match adapter
        .engine()
        .execute(Some(state), conn.as_mut(), ctx, &json_sql, token.clone())
    {
        Ok(batch) => Ok((batch, false)),
        Err(json_error) if is_unsupported_json_describe(&json_error) => adapter
            .engine()
            .execute(Some(state), conn.as_mut(), ctx, &plain_sql, token)
            .map(|batch| (batch, true))
            .map_err(|plain_error| describe_fallback_error(json_error, plain_error)),
        Err(error) => Err(error),
    }
}

fn databricks_string_value(value: &str) -> String {
    if !value
        .bytes()
        .any(|byte| byte == b'\'' || byte == b'\\' || byte.is_ascii_control())
    {
        return format!("'{value}'");
    }

    let hex = value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    format!("decode(unhex('{hex}'), 'UTF-8')")
}

pub(crate) fn databricks_metadata_value(value: &str) -> String {
    databricks_string_value(&value.to_lowercase())
}

fn databricks_metadata_identifier(value: &str) -> String {
    quote_identifier(&value.to_lowercase(), AdapterType::Databricks)
}

pub(crate) fn databricks_query_identifier(value: &str, quote: bool) -> String {
    if quote {
        quote_identifier(value, AdapterType::Databricks)
    } else {
        Ident::new(value, AdapterType::Databricks)
            .display(AdapterType::Databricks)
            .to_string()
    }
}

/// Whether a Databricks error means the relation does not exist: a `NotFound`
/// with SQLSTATE 42P01 (TABLE_OR_VIEW_NOT_FOUND), or that error class in the message.
///
/// `NotFound` alone is not enough, since the driver also reports it for other
/// missing objects (SQLSTATE 42704, 42883), so this fails closed.
pub(crate) fn is_databricks_relation_not_found(e: &AdapterError) -> bool {
    // https://docs.databricks.com/aws/en/error-messages/table-or-view-not-found-error-class
    (e.kind() == AdapterErrorKind::NotFound && e.sqlstate() == "42P01")
        || e.message().contains("[TABLE_OR_VIEW_NOT_FOUND]")
}

// Reference: https://github.com/databricks/dbt-databricks/blob/92f1442faabe0fce6f0375b95e46ebcbfcea4c67/dbt/include/databricks/macros/adapters/metadata.sql
pub fn list_relations(
    engine: &dyn AdapterEngine,
    state: Option<&State>,
    ctx: &QueryCtx,
    conn: &mut ConnectionGuard<'_>,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let catalog = databricks_metadata_value(&db_schema.resolved_catalog);
    let schema = databricks_metadata_value(&db_schema.resolved_schema);
    let sql = format!("
SELECT
    table_name,
    if(table_type IN ('EXTERNAL', 'MANAGED', 'MANAGED_SHALLOW_CLONE', 'EXTERNAL_SHALLOW_CLONE'), 'table', lower(table_type)) AS table_type,
    lower(data_source_format) AS file_format,
    table_schema,
    table_owner,
    table_catalog,
    if(
    table_type IN (
        'EXTERNAL',
        'MANAGED',
        'MANAGED_SHALLOW_CLONE',
        'EXTERNAL_SHALLOW_CLONE'
    ),
    lower(table_type),
    NULL
    ) AS databricks_table_type
FROM `system`.`information_schema`.`tables`
WHERE table_catalog = {catalog}
    AND table_schema = {schema}");

    let options = {
        let query_tags = QueryTags::from_state(state)?;
        query_tags.unwrap_or_default().into_stmt_options()
    };
    let batch =
        engine.execute_with_options(None, ctx, conn.as_mut(), &sql, options, true, token)?;

    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let mut relations = Vec::new();

    let names = batch.column_values::<StringArray>("table_name")?;
    let schemas = batch.column_values::<StringArray>("table_schema")?;
    let catalogs = batch.column_values::<StringArray>("table_catalog")?;
    let table_types = batch.column_values::<StringArray>("table_type")?;
    let file_formats = batch.column_values::<StringArray>("file_format")?;
    let databricks_table_types = batch.column_values::<StringArray>("databricks_table_type")?;

    for i in 0..batch.num_rows() {
        let name = names.value(i);
        let schema = schemas.value(i);
        let catalog = catalogs.value(i);
        let table_type = uppercase(table_types.value(i).into());
        let is_delta = file_formats.value(i) == "delta";
        let table_format = table_format_from_provider(Some(file_formats.value(i)));
        let is_shallow_clone = !databricks_table_types.is_null(i)
            && is_shallow_clone_type(databricks_table_types.value(i));

        let relation = Arc::new(
            Relation::new(
                engine.adapter_type(),
                catalog.to_string(),
                schema.to_string(),
                name.to_string(),
            )
            .with_relation_type(RelationType::from_adapter_type(
                AdapterType::Databricks,
                table_type.as_ref(),
            ))
            .with_quoting(engine.quoting())
            .with_is_delta(is_delta)
            .with_table_format(table_format)
            .with_is_shallow_clone(is_shallow_clone),
        ) as Arc<dyn BaseRelation>;
        relations.push(relation);
    }

    Ok(relations)
}

fn get_relation_with_quote_policy(
    relation: &Arc<dyn BaseRelation>,
) -> AdapterResult<(String, String, String)> {
    let database = relation.database_as_str()?;
    let schema = relation.schema_as_str()?;
    let identifier = relation.identifier_as_str()?;

    let quoted_database = if database.is_empty() {
        String::new()
    } else {
        databricks_query_identifier(&database, relation.quote_policy().database)
    };
    let quoted_schema = databricks_query_identifier(&schema, relation.quote_policy().schema);
    let quoted_identifier =
        databricks_query_identifier(&identifier, relation.quote_policy().identifier);

    Ok((quoted_database, quoted_schema, quoted_identifier))
}

pub(crate) fn databricks_relation_sql_name(
    relation: &Arc<dyn BaseRelation>,
) -> AdapterResult<String> {
    let (database, schema, identifier) = get_relation_with_quote_policy(relation)?;
    let include = relation.include_policy();
    let mut parts = Vec::new();
    if include.database && !database.is_empty() {
        parts.push(database);
    }
    if include.schema {
        parts.push(schema);
    }
    if include.identifier {
        parts.push(identifier);
    }
    Ok(parts.join("."))
}

fn databricks_safely_quoted_relation(
    relation: &Arc<dyn BaseRelation>,
) -> AdapterResult<Arc<dyn BaseRelation>> {
    relation
        .quote(Some(true), Some(true), Some(true))
        .map_err(|error| AdapterError::new(AdapterErrorKind::Internal, error.to_string()))
}

/// Adapts `DESCRIBE TABLE EXTENDED ... AS JSON` parser output into the
/// `AgateTable` shape the existing (info_schema-backed) relation-config
/// components already know how to read, so the AS-JSON path is a drop-in
/// replacement wherever it's supported.
pub(crate) fn agate_table_from_columns(
    fields: Vec<Field>,
    columns: Vec<ArrayRef>,
) -> AdapterResult<AgateTable> {
    let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
        .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))?;
    Ok(AgateTable::from_record_batch(Arc::new(batch)))
}

// Column names must match `constraints.rs::process_primary_key_constraints`.
pub(crate) fn primary_key_constraints_to_agate(
    rows: &[PrimaryKeyConstraintRow],
) -> AdapterResult<AgateTable> {
    agate_table_from_columns(
        vec![
            Field::new("constraint_name", DataType::Utf8, false),
            Field::new("column_name", DataType::Utf8, false),
        ],
        vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.constraint_name.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.column_name.as_str()),
            )) as ArrayRef,
        ],
    )
}

// Column names must match `constraints.rs::process_foreign_key_constraints`.
pub(crate) fn foreign_key_constraints_to_agate(
    rows: &[ForeignKeyConstraintRow],
) -> AdapterResult<AgateTable> {
    agate_table_from_columns(
        vec![
            Field::new("constraint_name", DataType::Utf8, false),
            Field::new("from_column", DataType::Utf8, false),
            Field::new("to_catalog", DataType::Utf8, false),
            Field::new("to_schema", DataType::Utf8, false),
            Field::new("to_table", DataType::Utf8, false),
            Field::new("to_column", DataType::Utf8, false),
        ],
        vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.constraint_name.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.from_column.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.to_catalog.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.to_schema.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.to_table.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.to_column.as_str()),
            )) as ArrayRef,
        ],
    )
}

// Column name must match `constraints.rs::from_remote_state`'s "column_name"/"col_name" lookup.
pub(crate) fn non_null_constraints_to_agate(
    rows: &[NonNullConstraintRow],
) -> AdapterResult<AgateTable> {
    agate_table_from_columns(
        vec![Field::new("column_name", DataType::Utf8, false)],
        vec![Arc::new(StringArray::from_iter_values(
            rows.iter().map(|r| r.column_name.as_str()),
        )) as ArrayRef],
    )
}

// `column_masks.rs::from_remote_state` reads these positionally: [0]=column_name,
// [1]=mask_name, [2]=using_columns (empty string, not null, when absent).
pub(crate) fn column_masks_to_agate(rows: &[ColumnMaskRow]) -> AdapterResult<AgateTable> {
    agate_table_from_columns(
        vec![
            Field::new("column_name", DataType::Utf8, false),
            Field::new("mask_name", DataType::Utf8, false),
            Field::new("using_columns", DataType::Utf8, false),
        ],
        vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.column_name.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.mask_name.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter()
                    .map(|r| r.using_columns.as_deref().unwrap_or("")),
            )) as ArrayRef,
        ],
    )
}

// Column name must match `query.rs::from_remote_state`'s "view_definition" lookup.
// Zero rows when `row` is `None`, matching `get_first_row` on an empty result set.
pub(crate) fn view_description_to_agate(
    row: &Option<ViewDescriptionRow>,
) -> AdapterResult<AgateTable> {
    let view_definitions: Vec<&str> = row.iter().map(|r| r.view_definition.as_str()).collect();
    agate_table_from_columns(
        vec![Field::new("view_definition", DataType::Utf8, true)],
        vec![Arc::new(StringArray::from_iter_values(view_definitions)) as ArrayRef],
    )
}

pub(crate) fn row_filters_metadata_table(rows: &[RowFilterRow]) -> AdapterResult<AgateTable> {
    agate_table_from_columns(
        vec![
            Field::new("table_catalog", DataType::Utf8, false),
            Field::new("table_schema", DataType::Utf8, false),
            Field::new("table_name", DataType::Utf8, false),
            Field::new("filter_name", DataType::Utf8, false),
            Field::new("target_columns", DataType::Utf8, false),
        ],
        vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.table_catalog.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.table_schema.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.table_name.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.filter_name.as_str()),
            )) as ArrayRef,
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.target_columns.as_str()),
            )) as ArrayRef,
        ],
    )
}

/// Cached result of the engine-version query, shared by
/// [`DatabricksMetadataAdapter::engine_version`] and
/// [`DatabricksMetadataAdapter::engine_version_with_conn`].
static CACHED_ENGINE_VERSION: OnceLock<AdapterResult<EngineVersion>> = OnceLock::new();

pub struct DatabricksMetadataAdapter {
    adapter: AdapterImpl,
}

impl DatabricksMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }

    pub fn new_from_adapter(adapter: AdapterImpl) -> Self {
        Self { adapter }
    }

    /// Get the engine runtime version, caching the result for subsequent calls.
    ///
    /// To bypass the cache, use [`get_engine_version()`](Self::get_engine_version) directly.
    pub(crate) fn engine_version(
        &self,
        node_id: Option<String>,
        token: CancellationToken,
    ) -> AdapterResult<EngineVersion> {
        CACHED_ENGINE_VERSION
            .get_or_init(move || {
                let query_ctx = QueryCtx::default().with_desc("get_engine_version adapter call");
                let mut conn = self.adapter.borrow_tlocal_connection(None, node_id)?;
                Self::get_engine_version(&self.adapter, &query_ctx, &mut conn, token)
            })
            .clone()
    }

    /// [`Self::engine_version`] for a caller that already holds a connection.
    ///
    /// A `MapReduce` map function is handed the worker's connection, so
    /// borrowing the thread-local again would open a second one and trip the
    /// nested-guard assertion. It shares the cache with [`Self::engine_version`]:
    /// the query runs once per process, for whichever of the two asks first.
    fn engine_version_with_conn(
        adapter: &AdapterImpl,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<EngineVersion> {
        CACHED_ENGINE_VERSION
            .get_or_init(|| {
                let query_ctx = QueryCtx::default().with_desc("get_engine_version adapter call");
                Self::get_engine_version(adapter, &query_ctx, conn, token)
            })
            .clone()
    }

    /// Get the Databricks/Spark Runtime version without caching.
    ///
    /// Databricks:
    /// This follows the dbt-databricks implementation:
    /// - For clusters: queries `SET spark.databricks.clusterUsageTags.sparkVersion`
    /// - For SQL Warehouses: returns `EngineVersion::Unset` (treated as latest/max version)
    ///
    /// See: https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/handle.py#L129
    ///
    /// Spark:
    /// This runs a `SELECT version()`
    pub fn get_engine_version(
        adapter: &AdapterImpl,
        ctx: &QueryCtx,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<EngineVersion> {
        match adapter.adapter_type() {
            AdapterType::Databricks => {
                let is_cluster = adapter.is_cluster()?;

                if !is_cluster {
                    return Ok(EngineVersion::Unset);
                }

                // For clusters, query the spark version tag
                // Returns a row like: (key, value) = ("spark.databricks.clusterUsageTags.sparkVersion", "15.4.x-scala2.12")
                let sql = "SET spark.databricks.clusterUsageTags.sparkVersion";
                let (_response, table) =
                    adapter.execute(None, conn, Some(ctx), sql, false, true, None, None, token)?;
                let batch = table.original_record_batch();

                // The result has two columns: "key" and "value"
                let values = batch.column_values::<StringArray>("value")?;
                debug_assert_eq!(values.len(), 1);

                let version_str = values.value(0);
                extract_dbr_version(version_str)
            }
            AdapterType::Spark => {
                let sql = "SELECT version() AS version";
                let (_response, table) =
                    adapter.execute(None, conn, Some(ctx), sql, false, true, None, None, token)?;
                let batch = table.original_record_batch();
                let values = batch.column_values::<StringArray>("version")?;
                debug_assert_eq!(values.len(), 1);
                let version_str = values.value(0);
                EngineVersion::from_str(version_str)
            }
            _ => unreachable!(),
        }
    }

    /// Given the relation, fetch its config from the remote data warehouse
    /// reference: https://github.com/databricks/dbt-databricks/blob/7c282cabb518a5e1173222e7901896d31de8401f/dbt/adapters/databricks/impl.py#L1287-L1470
    // TODO: use Arrow RecordBatches for this instead of a hashmap of Agate tables, like BigQuery does
    pub(crate) fn fetch_relation_config_from_remote(
        &self,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        base_relation: &Arc<dyn BaseRelation>,
        model_config: Option<&RelationConfig>,
        token: CancellationToken,
    ) -> AdapterResult<(RelationType, DatabricksRelationMetadata)> {
        let relation_type = base_relation.relation_type().ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("relation_type is required for the input relation of adapter.get_relation_config. Input relation: {}", base_relation.render_self_as_str()),
            )
        })?;

        let database = base_relation.database_as_str()?;
        let schema = base_relation.schema_as_str()?;
        let identifier = base_relation.identifier_as_str()?;
        let rendered_relation = databricks_relation_sql_name(base_relation)?;
        let json_relation = databricks_safely_quoted_relation(base_relation)?;
        // Absent model_config fetches both; empty desired tags skip that query.
        let fetch_relation_tags =
            components::RelationTagsLoader::requires_server_metadata_for_diff(model_config);
        let fetch_column_tags =
            components::ColumnTagsLoader::requires_server_metadata_for_diff(model_config);

        let mut metadata = IndexMap::new();

        // https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L421
        // Computed lazily (only inside branches that might use it) since it costs an
        // engine-version lookup (cached) and a behavior-flag lookup.
        let is_json_supported = |token: CancellationToken| -> AdapterResult<bool> {
            let compute = if self.adapter.is_cluster()? {
                DbrComputeContext::Cluster(self.engine_version(None, token)?)
            } else {
                DbrComputeContext::SqlWarehouse
            };
            let behavior_flag_enabled = self
                .adapter
                .behavior_object()
                .get_value(&Value::from("use_describe_as_json_for_relation_metadata"))
                .is_some_and(|flag| flag.is_true());
            Ok(is_describe_as_json_supported(
                &**base_relation,
                false, // is_foreign_table: relations that reach this point are never RelationType::External
                compute,
                behavior_flag_enabled,
            ))
        };

        // IMPORTANT (Mantle replay): query ordering is observable in replay.
        // Match dbt-databricks v1 `_describe_relation` query order:
        //   MV:          DESCRIBE EXTENDED → optional tags → view SQL → row filters → TBLPROPERTIES
        //   ST:          DESCRIBE EXTENDED → optional tags → TBLPROPERTIES → row filters
        //   View:        optional tags → optional column tags → view SQL → TBLPROPERTIES → DESCRIBE EXTENDED
        //   MetricView:  TBLPROPERTIES → optional tags → DESCRIBE EXTENDED
        //   Table:       UC information_schema (if not HMS) → TBLPROPERTIES → DESCRIBE EXTENDED
        match relation_type {
            RelationType::MetricView => {
                metadata.insert(
                    DatabricksRelationMetadataKey::ShowTblProperties,
                    self.show_tblproperties(&rendered_relation, state, &mut *conn, token.clone())?,
                );

                if fetch_relation_tags {
                    metadata.insert(
                        DatabricksRelationMetadataKey::InfoSchemaRelationTags,
                        self.fetch_tags(
                            &database,
                            &schema,
                            &identifier,
                            state,
                            &mut *conn,
                            token.clone(),
                        )?,
                    );
                }

                metadata.insert(
                    DatabricksRelationMetadataKey::DescribeExtended,
                    self.describe_extended(
                        &database,
                        &schema,
                        &identifier,
                        state,
                        &mut *conn,
                        token,
                    )?,
                );
            }
            RelationType::MaterializedView => {
                metadata.insert(
                    DatabricksRelationMetadataKey::DescribeExtended,
                    self.describe_extended_after_refresh(
                        &**base_relation,
                        state,
                        &mut *conn,
                        token.clone(),
                    )?,
                );
                if fetch_relation_tags {
                    metadata.insert(
                        DatabricksRelationMetadataKey::InfoSchemaRelationTags,
                        self.fetch_tags(
                            &database,
                            &schema,
                            &identifier,
                            state,
                            &mut *conn,
                            token.clone(),
                        )?,
                    );
                }
                let json_metadata = if is_json_supported(token.clone())? {
                    Some(fetch_json_metadata(state, &*json_relation)?)
                } else {
                    None
                };
                let view_table = match &json_metadata {
                    Some(json_metadata) => {
                        view_description_to_agate(&parse_view_description(json_metadata)?)?
                    }
                    None => self.get_view_description(
                        &database,
                        &schema,
                        &identifier,
                        state,
                        &mut *conn,
                        token.clone(),
                    )?,
                };
                metadata.insert(DatabricksRelationMetadataKey::InfoSchemaViews, view_table);
                let row_filters_table = match &json_metadata {
                    Some(json_metadata) => {
                        row_filters_metadata_table(&parse_row_filter(json_metadata)?)?
                    }
                    None => self.fetch_row_filters(
                        &database,
                        &schema,
                        &identifier,
                        state,
                        &mut *conn,
                        token.clone(),
                    )?,
                };
                metadata.insert(DatabricksRelationMetadataKey::RowFilters, row_filters_table);
                metadata.insert(
                    DatabricksRelationMetadataKey::ShowTblProperties,
                    self.show_tblproperties(&rendered_relation, state, &mut *conn, token)?,
                );
            }
            RelationType::View => {
                if fetch_relation_tags {
                    metadata.insert(
                        DatabricksRelationMetadataKey::InfoSchemaRelationTags,
                        self.fetch_tags(
                            &database,
                            &schema,
                            &identifier,
                            state,
                            &mut *conn,
                            token.clone(),
                        )?,
                    );
                }
                if fetch_column_tags {
                    metadata.insert(
                        DatabricksRelationMetadataKey::InfoSchemaColumnTags,
                        self.fetch_column_tags(
                            &database,
                            &schema,
                            &identifier,
                            state,
                            &mut *conn,
                            token.clone(),
                        )?,
                    );
                }
                let view_table = if is_json_supported(token.clone())? {
                    let json_metadata = fetch_json_metadata(state, &*json_relation)?;
                    view_description_to_agate(&parse_view_description(&json_metadata)?)?
                } else {
                    self.get_view_description(
                        &database,
                        &schema,
                        &identifier,
                        state,
                        &mut *conn,
                        token.clone(),
                    )?
                };
                metadata.insert(DatabricksRelationMetadataKey::InfoSchemaViews, view_table);
                metadata.insert(
                    DatabricksRelationMetadataKey::ShowTblProperties,
                    self.show_tblproperties(&rendered_relation, state, &mut *conn, token.clone())?,
                );
                metadata.insert(
                    DatabricksRelationMetadataKey::DescribeExtended,
                    self.describe_extended(
                        &database,
                        &schema,
                        &identifier,
                        state,
                        &mut *conn,
                        token,
                    )?,
                );
            }
            RelationType::StreamingTable => {
                metadata.insert(
                    DatabricksRelationMetadataKey::DescribeExtended,
                    self.describe_extended_after_refresh(
                        &**base_relation,
                        state,
                        &mut *conn,
                        token.clone(),
                    )?,
                );
                if fetch_relation_tags {
                    metadata.insert(
                        DatabricksRelationMetadataKey::InfoSchemaRelationTags,
                        self.fetch_tags(
                            &database,
                            &schema,
                            &identifier,
                            state,
                            &mut *conn,
                            token.clone(),
                        )?,
                    );
                }
                metadata.insert(
                    DatabricksRelationMetadataKey::ShowTblProperties,
                    self.show_tblproperties(&rendered_relation, state, &mut *conn, token.clone())?,
                );
                let row_filters_table = if is_json_supported(token.clone())? {
                    let json_metadata = fetch_json_metadata(state, &*json_relation)?;
                    row_filters_metadata_table(&parse_row_filter(&json_metadata)?)?
                } else {
                    self.fetch_row_filters(
                        &database,
                        &schema,
                        &identifier,
                        state,
                        &mut *conn,
                        token,
                    )?
                };
                metadata.insert(DatabricksRelationMetadataKey::RowFilters, row_filters_table);
            }
            RelationType::Table => {
                // Tags, constraints and column masks all live in `information_schema`, which
                // Hive Metastore does not have.
                if !base_relation.is_hive_metastore() {
                    if fetch_relation_tags {
                        metadata.insert(
                            DatabricksRelationMetadataKey::InfoSchemaRelationTags,
                            self.fetch_tags(
                                &database,
                                &schema,
                                &identifier,
                                state,
                                &mut *conn,
                                token.clone(),
                            )?,
                        );
                    }
                    if fetch_column_tags {
                        metadata.insert(
                            DatabricksRelationMetadataKey::InfoSchemaColumnTags,
                            self.fetch_column_tags(
                                &database,
                                &schema,
                                &identifier,
                                state,
                                &mut *conn,
                                token.clone(),
                            )?,
                        );
                    }
                    if is_json_supported(token.clone())? {
                        let json_metadata = fetch_json_metadata(state, &*json_relation)?;
                        let relation_metadata =
                            DatabricksDescribeJsonMetadata::from_json_metadata(&json_metadata)?;
                        metadata.insert(
                            DatabricksRelationMetadataKey::NonNullConstraints,
                            non_null_constraints_to_agate(&relation_metadata.non_null_constraints)?,
                        );
                        metadata.insert(
                            DatabricksRelationMetadataKey::PrimaryKeyConstraints,
                            primary_key_constraints_to_agate(
                                &relation_metadata.primary_key_constraints,
                            )?,
                        );
                        metadata.insert(
                            DatabricksRelationMetadataKey::ForeignKeyConstraints,
                            foreign_key_constraints_to_agate(
                                &relation_metadata.foreign_key_constraints,
                            )?,
                        );
                        metadata.insert(
                            DatabricksRelationMetadataKey::ColumnMasks,
                            column_masks_to_agate(&relation_metadata.column_masks)?,
                        );
                        metadata.insert(
                            DatabricksRelationMetadataKey::RowFilters,
                            row_filters_metadata_table(&relation_metadata.row_filters)?,
                        );
                    } else {
                        metadata.insert(
                            DatabricksRelationMetadataKey::NonNullConstraints,
                            self.fetch_non_null_constraint_columns(
                                &database,
                                &schema,
                                &identifier,
                                state,
                                &mut *conn,
                                token.clone(),
                            )?,
                        );
                        metadata.insert(
                            DatabricksRelationMetadataKey::PrimaryKeyConstraints,
                            self.fetch_primary_key_constraints(
                                &database,
                                &schema,
                                &identifier,
                                state,
                                &mut *conn,
                                token.clone(),
                            )?,
                        );
                        metadata.insert(
                            DatabricksRelationMetadataKey::ForeignKeyConstraints,
                            self.fetch_foreign_key_constraints(
                                &database,
                                &schema,
                                &identifier,
                                state,
                                &mut *conn,
                                token.clone(),
                            )?,
                        );
                        metadata.insert(
                            DatabricksRelationMetadataKey::ColumnMasks,
                            self.fetch_column_masks(
                                &database,
                                &schema,
                                &identifier,
                                state,
                                &mut *conn,
                                token.clone(),
                            )?,
                        );
                        metadata.insert(
                            DatabricksRelationMetadataKey::RowFilters,
                            self.fetch_row_filters(
                                &database,
                                &schema,
                                &identifier,
                                state,
                                &mut *conn,
                                token.clone(),
                            )?,
                        );
                    }
                }

                metadata.insert(
                    DatabricksRelationMetadataKey::ShowTblProperties,
                    self.show_tblproperties(&rendered_relation, state, &mut *conn, token.clone())?,
                );
                metadata.insert(
                    DatabricksRelationMetadataKey::DescribeExtended,
                    self.describe_extended(
                        &database,
                        &schema,
                        &identifier,
                        state,
                        &mut *conn,
                        token,
                    )?,
                );
            }
            RelationType::CTE
            | RelationType::Ephemeral
            | RelationType::External
            | RelationType::PointerTable
            | RelationType::DynamicTable
            | RelationType::InteractiveTable
            | RelationType::Function
            | RelationType::Dictionary => {
                return Err(AdapterError::new(
                    AdapterErrorKind::NotSupported,
                    format!(
                        "Cannot apply incremental config on relation of type {relation_type}. Relation: `{database}`.`{schema}`.`{identifier}`"
                    ),
                ));
            }
        };

        Ok((relation_type, metadata))
    }

    // convenience for executing SQL
    fn execute_sql_with_context(
        &self,
        sql: &str,
        state: &State,
        desc: &str,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<(AdapterResponse, AgateTable)> {
        let ctx = query_ctx_from_state(state)?.with_desc(desc);
        self.adapter.execute(
            Some(state),
            conn,
            Some(&ctx),
            sql,
            false, // auto_begin
            true,  // fetch
            None,  // limit
            None,  // options
            token,
        )
    }

    /// `describe extended` of a materialized view / streaming table, re-run until its refresh is
    /// no longer running. Same first query as [`Self::describe_extended`].
    fn describe_extended_after_refresh(
        &self,
        relation: &dyn BaseRelation,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        let (database, schema, identifier) = (
            relation.database_as_str()?,
            relation.schema_as_str()?,
            relation.identifier_as_str()?,
        );
        let first =
            self.describe_extended(&database, &schema, &identifier, state, conn, token.clone())?;
        let replay = self.adapter.engine().is_replay() || crate::time_machine::is_replaying();
        let strategy: &dyn refresh_wait::WaitStrategy = if replay {
            &refresh_wait::Replay
        } else {
            &refresh_wait::Live::DEFAULT
        };
        refresh_wait::wait_for_refresh_to_finish(
            first,
            &relation.render_self_as_str(),
            &token,
            &refresh_wait::RefreshWait::DEFAULT,
            strategy,
            || {
                self.describe_extended(
                    &database,
                    &schema,
                    &identifier,
                    state,
                    &mut *conn,
                    token.clone(),
                )
            },
        )
    }

    // https://github.com/dbt-labs/dbt-adapters/blob/6f2aae13e39c5df1c93e5d514678914142d71768/dbt-spark/src/dbt/include/spark/macros/adapters.sql#L314
    fn describe_extended(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        // match Mantle casing
        let sql = format!(
            "describe extended {}.{}.{};",
            quote_identifier(database, AdapterType::Databricks),
            quote_identifier(schema, AdapterType::Databricks),
            quote_identifier(identifier, AdapterType::Databricks)
        );
        let (_, result) =
            self.execute_sql_with_context(&sql, state, "Describe table extended", conn, token)?;
        Ok(result)
    }

    // https://github.com/databricks/dbt-databricks/blob/9e2566fdb56318cb7a59a4492f96c7aaa7af73b0/dbt/include/databricks/macros/adapters/metadata.sql#L78
    fn get_view_description(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        let database = databricks_metadata_value(database);
        let schema = databricks_metadata_value(schema);
        let identifier = databricks_metadata_value(identifier);
        let sql = format!(
            "SELECT *
            FROM `SYSTEM`.`INFORMATION_SCHEMA`.`VIEWS`
            WHERE TABLE_CATALOG = {}
                AND TABLE_SCHEMA = {}
                AND TABLE_NAME = {};",
            database, schema, identifier
        );
        let (_, result) =
            self.execute_sql_with_context(&sql, state, "Query for view", conn, token)?;
        Ok(result)
    }

    // https://github.com/dbt-labs/dbt-adapters/blob/6f2aae13e39c5df1c93e5d514678914142d71768/dbt-spark/src/dbt/include/spark/macros/adapters.sql#L127
    fn show_tblproperties(
        &self,
        relation_str: &str,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        let sql = format!("SHOW TBLPROPERTIES {relation_str}");
        let (_, result) =
            self.execute_sql_with_context(&sql, state, "Show table properties", conn, token)?;
        Ok(result)
    }

    // https://github.com/databricks/dbt-databricks/blob/9e2566fdb56318cb7a59a4492f96c7aaa7af73b0/dbt/include/databricks/macros/relations/components/constraints.sql#L1
    fn fetch_non_null_constraint_columns(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        let database_identifier = databricks_metadata_identifier(database);
        let database = databricks_metadata_value(database);
        let schema = databricks_metadata_value(schema);
        let identifier = databricks_metadata_value(identifier);
        let sql = format!(
            "SELECT column_name
            FROM {database_identifier}.`information_schema`.`columns`
            WHERE table_catalog = {database}
              AND table_schema = {schema}
              AND table_name = {identifier}
              AND is_nullable = 'NO';"
        );
        let (_, result) = self.execute_sql_with_context(
            &sql,
            state,
            "Fetch non null constraint columns",
            conn,
            token,
        )?;
        Ok(result)
    }

    // https://github.com/databricks/dbt-databricks/blob/9e2566fdb56318cb7a59a4492f96c7aaa7af73b0/dbt/include/databricks/macros/relations/components/constraints.sql#L20
    fn fetch_primary_key_constraints(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        let database_identifier = databricks_metadata_identifier(database);
        let database = databricks_metadata_value(database);
        let schema = databricks_metadata_value(schema);
        let identifier = databricks_metadata_value(identifier);
        let sql = format!(
            "SELECT kcu.constraint_name, kcu.column_name
            FROM {database_identifier}.information_schema.key_column_usage kcu
            WHERE kcu.table_catalog = {database}
                AND kcu.table_schema = {schema}
                AND kcu.table_name = {identifier}
                AND kcu.constraint_name = (
                SELECT constraint_name
                FROM {database_identifier}.information_schema.table_constraints
                WHERE table_catalog = {database}
                    AND table_schema = {schema}
                    AND table_name = {identifier}
                    AND constraint_type = 'PRIMARY KEY'
                )
            ORDER BY kcu.ordinal_position;"
        );
        let (_, result) =
            self.execute_sql_with_context(&sql, state, "Fetch PK constraints", conn, token)?;
        Ok(result)
    }

    // https://github.com/databricks/dbt-databricks/blob/9e2566fdb56318cb7a59a4492f96c7aaa7af73b0/dbt/include/databricks/macros/relations/components/column_mask.sql#L11
    fn fetch_column_masks(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        let database = databricks_metadata_value(database);
        let schema = databricks_metadata_value(schema);
        let identifier = databricks_metadata_value(identifier);
        let sql = format!(
            "SELECT
                column_name,
                mask_name,
                using_columns
            FROM `system`.`information_schema`.`column_masks`
            WHERE table_catalog = {database}
                AND table_schema = {schema}
                AND table_name = {identifier};"
        );
        let (_, result) =
            self.execute_sql_with_context(&sql, state, "Fetch column masks", conn, token)?;
        Ok(result)
    }

    // https://github.com/databricks/dbt-databricks/blob/f65356ef59a5996bebf2c86296f32295815a7bb3/dbt/include/databricks/macros/relations/components/row_filter.sql
    fn fetch_row_filters(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        let database_identifier = databricks_metadata_identifier(database);
        let database = databricks_metadata_value(database);
        let schema = databricks_metadata_value(schema);
        let identifier = databricks_metadata_value(identifier);
        let sql = format!(
            "SELECT
                table_catalog,
                table_schema,
                table_name,
                filter_name,
                target_columns
            FROM {database_identifier}.`information_schema`.`row_filters`
            WHERE table_catalog = {database}
                AND table_schema = {schema}
                AND table_name = {identifier};",
        );
        let (_, result) =
            self.execute_sql_with_context(&sql, state, "Fetch row filters", conn, token)?;
        Ok(result)
    }

    // https://github.com/databricks/dbt-databricks/blob/0f7f1396711cc4ff19092dfdb26c7a8afad946cf/dbt/include/databricks/macros/relations/components/constraints.sql#L47
    fn fetch_foreign_key_constraints(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        let database_identifier = databricks_metadata_identifier(database);
        let database = databricks_metadata_value(database);
        let schema = databricks_metadata_value(schema);
        let identifier = databricks_metadata_value(identifier);
        let sql = format!(
            "SELECT
                kcu.constraint_name,
                kcu.column_name AS from_column,
                ukcu.table_catalog AS to_catalog,
                ukcu.table_schema AS to_schema,
                ukcu.table_name AS to_table,
                ukcu.column_name AS to_column
            FROM {database_identifier}.information_schema.key_column_usage kcu
            JOIN {database_identifier}.information_schema.referential_constraints rc
                ON kcu.constraint_catalog = rc.constraint_catalog
                AND kcu.constraint_schema = rc.constraint_schema
                AND kcu.constraint_name = rc.constraint_name
            JOIN {database_identifier}.information_schema.key_column_usage ukcu
                ON rc.unique_constraint_catalog = ukcu.constraint_catalog
                AND rc.unique_constraint_schema = ukcu.constraint_schema
                AND rc.unique_constraint_name = ukcu.constraint_name
                AND kcu.ordinal_position = ukcu.ordinal_position
            WHERE kcu.table_catalog = {database}
                AND kcu.table_schema = {schema}
                AND kcu.table_name = {identifier}
                AND kcu.constraint_name IN (
                SELECT constraint_name
                FROM {database_identifier}.information_schema.table_constraints
                WHERE table_catalog = {database}
                    AND table_schema = {schema}
                    AND table_name = {identifier}
                    AND constraint_type = 'FOREIGN KEY'
                )
            ORDER BY kcu.ordinal_position;"
        );
        let (_, result) =
            self.execute_sql_with_context(&sql, state, "Fetch FK constraints", conn, token)?;
        Ok(result)
    }

    // https://github.com/databricks/dbt-databricks/blob/9e2566fdb56318cb7a59a4492f96c7aaa7af73b0/dbt/include/databricks/macros/relations/tags.sql#L11
    fn fetch_tags(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        let database = databricks_metadata_value(database);
        let schema = databricks_metadata_value(schema);
        let identifier = databricks_metadata_value(identifier);
        let sql = format!(
            "SELECT tag_name, tag_value
            FROM `system`.`information_schema`.`table_tags`
            WHERE catalog_name = {database}
                AND schema_name = {schema}
                AND table_name = {identifier}",
        );
        let (_, result) = self.execute_sql_with_context(&sql, state, "Fetch tags", conn, token)?;
        Ok(result)
    }

    fn fetch_column_tags(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
        state: &State,
        conn: &mut ConnectionGuard<'_>,
        token: CancellationToken,
    ) -> AdapterResult<AgateTable> {
        let database = databricks_metadata_value(database);
        let schema = databricks_metadata_value(schema);
        let identifier = databricks_metadata_value(identifier);
        let sql = format!(
            "SELECT column_name, tag_name, tag_value
            FROM `system`.`information_schema`.`column_tags`
            WHERE catalog_name = {database}
                AND schema_name = {schema}
                AND table_name = {identifier}",
        );
        let (_, result) =
            self.execute_sql_with_context(&sql, state, "Fetch column tags", conn, token)?;
        Ok(result)
    }

    fn freshness_with_overrides_impl<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        overrides: &'a BTreeMap<String, FreshnessOverride>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        if overrides.is_empty() {
            return self.freshness_inner(relations, token);
        }

        // Partition: sources with loaded_at_query / loaded_at_field get their own
        // targeted query; all other relations go through the bulk DESCRIBE HISTORY path.
        let (bulk_relations, override_targets) = partition_override_relations(relations, overrides);

        let engine = self.adapter.engine().clone();
        let factory = Box::new(AdapterConnectionFactory::new(engine));

        type Acc = BTreeMap<String, MetadataFreshness>;
        let mut tasks: Vec<FreshnessTask> = Vec::new();
        if !bulk_relations.is_empty() {
            tasks.push(FreshnessTask::Bulk(bulk_relations));
        }
        for (relation, ovr) in override_targets {
            tasks.push(FreshnessTask::Override(relation, ovr));
        }

        let token_clone = token.clone();
        let adapter_for_map = self.adapter.clone();
        let map_f = move |conn: &mut ConnectionGuard<'_>,
                          task: &FreshnessTask|
              -> AdapterResult<FreshnessTaskResult> {
            match task {
                FreshnessTask::Bulk(bulk) => {
                    let mut acc: Acc = BTreeMap::new();
                    for relation in bulk {
                        if let Ok(Some(freshness)) = databricks_freshness_for_relation(
                            &adapter_for_map,
                            conn,
                            relation,
                            token_clone.clone(),
                        ) {
                            acc.insert(relation.semantic_fqn(), freshness);
                        }
                    }
                    Ok(FreshnessTaskResult::Bulk(acc))
                }
                FreshnessTask::Override(relation, ovr) => {
                    run_override_query(&adapter_for_map, conn, relation, ovr, token_clone.clone())
                }
            }
        };

        let reduce_f = move |acc: &mut Acc,
                             _task: FreshnessTask,
                             res: AdapterResult<FreshnessTaskResult>|
              -> Result<(), Cancellable<AdapterError>> {
            if let Ok(task_result) = res {
                apply_freshness_task_result(acc, task_result)?;
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(tasks), token)
    }
}

impl MetadataAdapter for DatabricksMetadataAdapter {
    fn adapter_type(&self) -> AdapterType {
        self.adapter.adapter_type()
    }

    fn build_schemas_from_stats_sql(
        &self,
        stats_sql_result: Arc<RecordBatch>,
    ) -> AdapterResult<BTreeMap<String, CatalogTable>> {
        if stats_sql_result.num_rows() == 0 {
            return Ok(BTreeMap::new());
        }

        let table_catalogs = stats_sql_result.column_values::<StringArray>("table_database")?;
        let table_schemas = stats_sql_result.column_values::<StringArray>("table_schema")?;
        let table_names = stats_sql_result.column_values::<StringArray>("table_name")?;
        let data_types = stats_sql_result.column_values::<StringArray>("table_type")?;
        let comments = stats_sql_result.column_values::<StringArray>("table_comment")?;
        let table_owners = stats_sql_result.column_values::<StringArray>("table_owner")?;

        let last_modified_label =
            stats_sql_result.column_values::<StringArray>("stats:last_modified:label")?;
        let last_modified_value = stats_sql_result
            .column_values::<TimestampMicrosecondArray>("stats:last_modified:value")?;
        let last_modified_description =
            stats_sql_result.column_values::<StringArray>("stats:last_modified:description")?;
        let last_modified_include =
            stats_sql_result.column_values::<BooleanArray>("stats:last_modified:include")?;
        let mut result = BTreeMap::<String, CatalogTable>::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);
            let data_type = data_types.value(i);
            let comment = comments.value(i);
            let owner = table_owners.value(i);

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            if !result.contains_key(&fully_qualified_name) {
                let mut stats = BTreeMap::new();
                if last_modified_include.value(i) {
                    stats.insert(
                        "last_modified".to_string(),
                        CatalogNodeStats {
                            id: "last_modified".to_string(),
                            label: last_modified_label.value(i).to_string(),
                            value: serde_json::Value::String(
                                last_modified_value.value(i).to_string(),
                            ),
                            description: Some(last_modified_description.value(i).to_string()),
                            include: last_modified_include.value(i),
                        },
                    );
                }

                stats.insert(
                    "has_stats".to_string(),
                    CatalogNodeStats {
                        id: "has_stats".to_string(),
                        label: "has_stats".to_string(),
                        value: serde_json::Value::Bool(stats.is_empty()),
                        description: Some("Has stats".to_string()),
                        include: last_modified_include.value(i),
                    },
                );

                let node_metadata = TableMetadata {
                    materialization_type: data_type.to_string(),
                    schema: schema.to_string(),
                    name: table.to_string(),
                    database: Some(catalog.to_string()),
                    comment: match comment {
                        "" => None,
                        _ => Some(comment.to_string()),
                    },
                    owner: Some(owner.to_string()),
                };

                let node = CatalogTable {
                    metadata: node_metadata,
                    columns: IndexMap::new(),
                    stats,
                    unique_id: None,
                };
                result.insert(fully_qualified_name.clone(), node);
            }
        }
        Ok(result)
    }

    fn build_columns_from_get_columns(
        &self,
        stats_sql_result: Arc<RecordBatch>,
    ) -> AdapterResult<BTreeMap<String, BTreeMap<String, ColumnMetadata>>> {
        if stats_sql_result.num_rows() == 0 {
            return Ok(BTreeMap::new());
        }

        let table_catalogs = stats_sql_result.column_values::<StringArray>("table_database")?;
        let table_schemas = stats_sql_result.column_values::<StringArray>("table_schema")?;
        let table_names = stats_sql_result.column_values::<StringArray>("table_name")?;

        let column_names = stats_sql_result.column_values::<StringArray>("column_name")?;
        let column_indices = stats_sql_result.column_values::<Int32Array>("column_index")?;
        let column_types = stats_sql_result.column_values::<StringArray>("column_type")?;
        let column_comments = stats_sql_result.column_values::<StringArray>("column_comment")?;

        let mut columns_by_relation = BTreeMap::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            let column_name = column_names.value(i);
            let column_index = column_indices.value(i);
            let column_type = column_types.value(i);
            let column_comment = column_comments.value(i);

            let column = ColumnMetadata {
                name: column_name.to_string(),
                index: column_index as i128,
                data_type: column_type.to_string(),
                comment: match column_comment {
                    "" => None,
                    _ => Some(column_comment.to_string()),
                },
            };

            columns_by_relation
                .entry(fully_qualified_name.clone())
                .or_insert(BTreeMap::new())
                .insert(column_name.to_string(), column);
        }
        Ok(columns_by_relation)
    }

    fn list_relations_schemas_inner(
        &self,
        unique_id: Option<String>,
        phase: Option<ExecutionPhase>,
        relations: &[Arc<dyn BaseRelation>],
        item_span_operation_id: Option<&str>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, HashMap<String, AdapterResult<Arc<Schema>>>> {
        type Acc = HashMap<String, AdapterResult<Arc<Schema>>>;

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let node_id = unique_id.clone();
        let map_f = move |conn: &mut ConnectionGuard<'_>,
                          relation: &Arc<dyn BaseRelation>|
              -> AdapterResult<Arc<Schema>> {
            let (database, schema, identifier) = get_relation_with_quote_policy(relation)?;

            // Databricks system tables doesn't support `DESCRIBE TABLE EXTENDED .. AS JSON` :(
            // More details refer to the test adapter::repros::databricks_use_system_relations

            // NB(cwalden):
            //  It appears that specifically EXTERNAL system tables do not support AS JSON
            //  `is_system()` is actually a bit too broad and unnecessarily excludes some system tables
            //  that do support AS JSON.
            //
            //  Checking the relation_type will be sufficient enough to fix bug raised in `dbt-fusion#543`
            //  but we will need to revisit this when we add cluster support (see `fs#5135`).

            let relation_type = relation.relation_type().or_else(|| {
                // system.information_schema tables are known to be External types
                // This fallback is necessary since schema hydration is not guaranteed to be called
                // by fully resolved relations
                let d = relation.database_as_str().ok()?;
                let s = relation.schema_as_str().ok()?;
                (d.to_lowercase() == "system" && s.to_lowercase() == "information_schema")
                    .then_some(RelationType::External)
            });

            let is_external_system =
                relation.is_system() && matches!(relation_type, Some(RelationType::External));

            // The engine version costs a query, so it is only asked for when a
            // branch actually needs it -- and it is asked with `conn`, the
            // connection this worker was handed.
            let as_json_unsupported = match adapter.adapter_type() {
                AdapterType::Databricks => {
                    is_external_system
                        || Self::engine_version_with_conn(&adapter, conn, token_clone.clone())?
                            < EngineVersion::Full(16, 2)
                }
                AdapterType::Spark => {
                    Self::engine_version_with_conn(&adapter, conn, token_clone.clone())?
                        < EngineVersion::Full(4, 0)
                }
                _ => unreachable!(),
            };

            let fqn = match adapter.adapter_type() {
                // Schema-enabled Fabric lakehouses carry the lakehouse name in
                // `database` (3-part naming); classic lakehouses and plain
                // Spark leave it empty.
                AdapterType::Spark if database.is_empty() => {
                    format!("{schema}.{identifier}")
                }
                AdapterType::Spark => format!("{database}.{schema}.{identifier}"),
                AdapterType::Databricks => format!("{database}.{schema}.{identifier}"),
                _ => unreachable!(),
            };

            let sql = if as_json_unsupported {
                format!("DESCRIBE TABLE {fqn};")
            } else {
                format!("DESCRIBE TABLE EXTENDED {fqn} AS JSON;")
            };

            let mut ctx = QueryCtx::new_metadata().with_desc("Get table schema");
            if let Some(ref node_id) = node_id {
                ctx = ctx.with_node_id(node_id);
            }
            if let Some(phase) = phase {
                ctx = ctx.with_phase(phase.as_str());
            }

            let (table, used_plain_describe) = if as_json_unsupported {
                let (_, table) = adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
                (table, true)
            } else if adapter.adapter_type() == AdapterType::Databricks {
                let plain_sql = format!("DESCRIBE TABLE {fqn};");
                match adapter.query(&ctx, conn, &sql, None, token_clone.clone()) {
                    Ok((_, table)) => (table, false),
                    Err(json_error) if is_unsupported_json_describe(&json_error) => {
                        let (_, table) = adapter
                            .query(&ctx, conn, &plain_sql, None, token_clone.clone())
                            .map_err(|plain_error| {
                                describe_fallback_error(json_error, plain_error)
                            })?;
                        (table, true)
                    }
                    Err(error) => return Err(error),
                }
            } else {
                let (_, table) = adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
                (table, false)
            };
            let batch = table.original_record_batch();

            let schema = if used_plain_describe {
                build_schema_from_basic_describe_table(batch, adapter.engine().type_ops().as_ref())?
            } else {
                let json_metadata = DatabricksTableMetadata::from_record_batch(batch)?;
                json_metadata.to_arrow_schema(adapter.engine().type_ops().as_ref())?
            };
            Ok(schema)
        };
        let reduce_f = |acc: &mut Acc,
                        relation: Arc<dyn BaseRelation>,
                        schema: AdapterResult<Arc<Schema>>|
         -> Result<(), Cancellable<AdapterError>> {
            acc.insert(relation.semantic_fqn(), schema);
            Ok(())
        };
        run_schema_cache_map_reduce(
            factory,
            relations.to_vec(),
            item_span_operation_id,
            map_f,
            reduce_f,
            unique_id,
            token,
        )
    }

    fn list_relations_schemas_by_patterns_inner(
        &self,
        _patterns: &[RelationPattern],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        todo!("DatabricksAdapter::list_relations_schemas_by_patterns")
    }

    fn freshness_inner(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        if relations.is_empty() {
            return Box::pin(async { Ok(BTreeMap::new()) });
        }

        type Acc = BTreeMap<String, MetadataFreshness>;

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();

        let map_f = move |conn: &mut ConnectionGuard<'_>,
                          relation: &Arc<dyn BaseRelation>|
              -> AdapterResult<Option<MetadataFreshness>> {
            databricks_freshness_for_relation(&adapter, &mut *conn, relation, token_clone.clone())
        };

        let reduce_f = move |acc: &mut Acc,
                             relation: Arc<dyn BaseRelation>,
                             result: AdapterResult<Option<MetadataFreshness>>|
              -> Result<(), Cancellable<AdapterError>> {
            if let Some(freshness) = result? {
                acc.insert(relation.semantic_fqn(), freshness);
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(relations.to_vec()), token)
    }

    fn freshness_with_overrides<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        overrides: &'a BTreeMap<String, FreshnessOverride>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        with_time_machine_metadata_wrapper(
            "global",
            "freshness_with_overrides",
            args_freshness_with_overrides(
                relations.iter().map(|r| r.semantic_fqn()),
                overrides,
                None,
            ),
            self.freshness_with_overrides_impl(relations, overrides, token),
        )
    }

    fn create_schemas_if_not_exists(
        &self,
        state: &State<'_, '_>,
        catalog_schemas: Vec<(String, String, String)>,
    ) -> AdapterResult<Vec<(String, String, String, AdapterResult<()>)>> {
        create_schemas_if_not_exists(&self.adapter, self, state, catalog_schemas)
    }

    fn list_relations_in_parallel_inner(
        &self,
        db_schemas: &[CatalogAndSchema],
        token: CancellationToken,
        report_progress: bool,
    ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        type Acc = BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>;
        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &mut ConnectionGuard<'_>,
                          db_schema: &CatalogAndSchema|
              -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
            let query_ctx = QueryCtx::default().with_desc("list_relations_in_parallel (UC)");
            with_relation_list_item_span(
                report_progress.then_some(RELATION_CACHE_OP_ID),
                &db_schema.to_string(),
                || adapter.list_relations(None, &query_ctx, conn, db_schema, token_clone.clone()),
            )
        };

        let reduce_f = move |acc: &mut Acc,
                             db_schema: CatalogAndSchema,
                             relations: AdapterResult<Vec<Arc<dyn BaseRelation>>>|
              -> Result<(), Cancellable<AdapterError>> {
            acc.insert(db_schema, relations);
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(db_schemas.to_vec()), token)
    }

    fn is_permission_error(&self, e: &AdapterError) -> bool {
        // 42501: insufficient privileges
        // Databricks doesn't provide an explicit enough SQLSTATE, noticed most of their errors' SQLSTATE is HY000
        // so we have to match on the error message below.
        // By the time of writing down this note, it is a problem from their backend thus not something we can fix on the SDK or driver layer
        // check out data/repros/databricks_create_schema_no_catalog_access on how to repro this error
        e.sqlstate() == "42501" || e.message().contains("PERMISSION_DENIED")
    }

    /// Fetch view definitions for the given relations.
    ///
    /// We use `DESCRIBE EXTENDED <fqn>` rather than `information_schema.views`
    /// because only the former exposes the `View Catalog and Namespace` field,
    /// which records the catalog/schema that were the *session defaults at
    /// view creation time*. In Databricks, unqualified table references inside
    /// a view's body resolve against those session defaults, not against the
    /// view's own catalog/schema, so this is the only authoritative source for
    /// the `default_catalog`/`default_schema` we hand back on `ViewDefinition`.
    /// Re-parsing the view body with the view's own FQN as the default would
    /// silently misqualify any unqualified reference.
    fn fetch_view_definitions_inner<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, ViewDefinitionFetchResult> {
        type Acc = ViewDefinitionFetchResult;

        if self.adapter.adapter_type() != AdapterType::Databricks {
            let err = AdapterError::new(
                AdapterErrorKind::NotSupported,
                "fetch_view_definitions is not supported for Spark",
            );
            return Box::pin(future::ready(Err(Cancellable::Error(err))));
        }

        if relations.is_empty() {
            return Box::pin(async { Ok(ViewDefinitionFetchResult::default()) });
        }

        // Dedupe FQNs while preserving insertion order.
        let fqns: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            let mut out = Vec::with_capacity(relations.len());
            for r in relations {
                let f = r.semantic_fqn();
                if seen.insert(f.clone()) {
                    out.push(f);
                }
            }
            out
        };

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &mut ConnectionGuard<'_>,
                          fqn: &String|
              -> AdapterResult<Arc<RecordBatch>> {
            let sql = format!("DESCRIBE EXTENDED {fqn}");
            let ctx = QueryCtx::default().with_desc("Fetch view definition");
            let (_, table) = adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            Ok(table.original_record_batch())
        };

        let reduce_f = |acc: &mut Acc,
                        fqn: String,
                        batch_res: AdapterResult<Arc<RecordBatch>>|
         -> Result<(), Cancellable<AdapterError>> {
            let batch = batch_res?;
            let (view_text, catalog_and_ns) = parse_describe_extended_view_info(&batch)?;
            let Some(view_text) = view_text else {
                // Not a view (or row-based DESCRIBE EXTENDED omitted the field).
                return Ok(());
            };
            let Some((default_catalog, default_schema)) =
                parse_view_catalog_and_namespace(catalog_and_ns.as_deref(), &fqn)
            else {
                // Both the `View Catalog and Namespace` row and the view's own
                // FQN failed to parse. Skip rather than emit a ViewDefinition
                // with empty defaults that would misqualify unqualified
                // references downstream.
                emit_warn_log_message(
                    ErrorCode::StateServiceWarn,
                    format!(
                        "Skipping view definition: could not parse `View Catalog and Namespace` ({catalog_and_ns:?}) or fqn ({fqn})"
                    ),
                );
                return Ok(());
            };

            acc.definitions.push(ViewDefinition {
                fqn,
                definition: view_text,
                dialect: AdapterType::Databricks,
                default_catalog,
                default_schema,
            });
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(fqns), token)
    }

    fn supports_bulk_freshness_dump(&self) -> bool {
        true
    }

    fn freshness_all_in_schema_inner<'a>(
        &'a self,
        _database: &'a str,
        _schema: &'a str,
        relations: &'a [Arc<dyn BaseRelation>],
        _options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        // Mirrors the plugin's _batch_table_names strategy:
        //
        // - Tables: per-relation DESCRIBE HISTORY LIMIT 1 in parallel.
        //   INFORMATION_SCHEMA.last_altered only reflects DDL (CREATE/ALTER),
        //   not DML writes (INSERT/UPDATE/DELETE/MERGE). Delta tables require
        //   the transaction log for accurate freshness.
        //   Ref: https://kb.databricks.com/unity-catalog/last_altered-column-in-information_schema-not-reflecting-data-modifications
        //
        // - Views: DESCRIBE HISTORY fails (no Delta log); databricks_freshness_for_relation
        //   falls back to a per-relation INFORMATION_SCHEMA.last_altered query, which
        //   IS accurate for views since DDL changes are the only meaningful freshness signal.
        //
        // The relation cache (populated via list_relations earlier in the run) determines
        // the type for each relation without issuing additional warehouse queries —
        // the same approach the plugin uses via adapter.get_relation().
        if relations.is_empty() {
            return Box::pin(async { Ok(BTreeMap::new()) });
        }

        let cache = self.adapter.engine().relation_cache();
        let mut table_relations: Vec<Arc<dyn BaseRelation>> = Vec::new();
        let mut view_relations: Vec<Arc<dyn BaseRelation>> = Vec::new();
        for relation in relations {
            let cached_type = cache
                .get_relation(relation.as_ref())
                .and_then(|e| e.relation().relation_type());
            match cached_type {
                Some(RelationType::View) | Some(RelationType::MaterializedView) => {
                    view_relations.push(Arc::clone(relation));
                }
                _ => {
                    table_relations.push(Arc::clone(relation));
                }
            }
        }

        // Both paths use databricks_freshness_for_relation, which issues
        // DESCRIBE HISTORY for tables and falls back to IS for views.
        // All relations run in parallel via MapReduce.
        let all_relations = relations.to_vec();
        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        type Acc = BTreeMap<String, MetadataFreshness>;
        let factory = Box::new(AdapterConnectionFactory::new(adapter.engine().clone()));
        let map_f = move |conn: &mut ConnectionGuard<'_>,
                          relation: &Arc<dyn BaseRelation>|
              -> AdapterResult<Option<MetadataFreshness>> {
            databricks_freshness_for_relation(&adapter, &mut *conn, relation, token_clone.clone())
        };
        let reduce_f = move |acc: &mut Acc,
                             relation: Arc<dyn BaseRelation>,
                             result: AdapterResult<Option<MetadataFreshness>>|
              -> Result<(), Cancellable<AdapterError>> {
            if let Some(freshness) = result? {
                acc.insert(relation.semantic_fqn(), freshness);
            }
            Ok(())
        };
        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(all_relations), token)
    }
}

/// Resolve the freshness epoch for a single Databricks relation.
///
/// Tries `DESCRIBE HISTORY <fqn> LIMIT 1` first: this reads from the Delta
/// transaction log and reflects all write operations (INSERT, UPDATE, DELETE,
/// MERGE). On failure — non-Delta tables, views, and older runtimes — falls
/// back to a per-relation `INFORMATION_SCHEMA.TABLES` query.
///
/// Shared by `freshness_inner` (bulk path) and the `Bulk` arm of
/// `freshness_with_overrides`.
/// Extract the epoch-millisecond timestamp from the first row of a
/// `DESCRIBE HISTORY … LIMIT 1` result batch.
///
/// Databricks SQL warehouses may return the `timestamp` column in any of the
/// four Arrow timestamp precisions depending on the runtime and driver version.
/// Returns `None` when the batch is empty, the column is absent, or the value
/// is null.
fn epoch_ms_from_history_batch(batch: &Arc<RecordBatch>) -> Option<i64> {
    if batch.num_rows() == 0 {
        return None;
    }
    let col = batch.column_by_name("timestamp")?;
    if col.is_null(0) {
        return None;
    }
    if let Some(ts) = col.as_any().downcast_ref::<TimestampMillisecondArray>() {
        Some(ts.value(0))
    } else if let Some(ts) = col.as_any().downcast_ref::<TimestampMicrosecondArray>() {
        Some(ts.value(0) / 1_000)
    } else if let Some(ts) = col.as_any().downcast_ref::<TimestampNanosecondArray>() {
        Some(ts.value(0) / 1_000_000)
    } else {
        col.as_any()
            .downcast_ref::<TimestampSecondArray>()
            .map(|ts| ts.value(0) * 1_000)
    }
}

/// Partition `relations` into bulk (no override) and per-override buckets.
///
type OverrideTargets = Vec<(Arc<dyn BaseRelation>, FreshnessOverride)>;

/// Relations whose `semantic_fqn` appears in `overrides` are paired with their
/// override and collected into the second return value; all others go into the
/// first (bulk) return value.
fn partition_override_relations(
    relations: &[Arc<dyn BaseRelation>],
    overrides: &BTreeMap<String, FreshnessOverride>,
) -> (Vec<Arc<dyn BaseRelation>>, OverrideTargets) {
    let mut bulk = Vec::new();
    let mut targets = Vec::new();
    for relation in relations {
        if let Some(ovr) = overrides.get(&relation.semantic_fqn()) {
            targets.push((Arc::clone(relation), ovr.clone()));
        } else {
            bulk.push(Arc::clone(relation));
        }
    }
    (bulk, targets)
}

fn databricks_information_schema_freshness_sql(
    database: &str,
    schema: &str,
    table: &str,
) -> String {
    let database = quote_identifier(database, AdapterType::Databricks);
    let schema = databricks_string_value(schema);
    let table = databricks_string_value(table);
    format!(
        "SELECT
        last_altered,
        (table_type = 'VIEW' OR table_type = 'MATERIALIZED_VIEW') AS is_view
     FROM {database}.INFORMATION_SCHEMA.TABLES
     WHERE table_schema = {schema} AND table_name = {table}"
    )
}

fn databricks_freshness_for_relation(
    adapter: &AdapterImpl,
    conn: &mut ConnectionGuard<'_>,
    relation: &Arc<dyn BaseRelation>,
    token: CancellationToken,
) -> AdapterResult<Option<MetadataFreshness>> {
    let fqn = databricks_relation_sql_name(relation)?;

    let history_sql = format!("DESCRIBE HISTORY {fqn} LIMIT 1");
    let ctx = QueryCtx::default().with_desc("Extracting freshness from Delta history");
    if let Ok((_, agate)) = adapter.query(&ctx, conn, &history_sql, None, token.clone()) {
        if let Some(ms) = epoch_ms_from_history_batch(&agate.original_record_batch()) {
            return MetadataFreshness::from_millis(ms, false).map(Some);
        }
    }

    // Fallback: INFORMATION_SCHEMA for views and non-Delta objects.
    let (input_schema, database, input_table) = get_input_schema_database_and_table(relation)?;
    let sql = databricks_information_schema_freshness_sql(&database, &input_schema, &input_table);
    let ctx = QueryCtx::default().with_desc("Extracting freshness from information schema");
    let (_, agate) = adapter.query(&ctx, conn, &sql, None, token)?;
    let batch = agate.original_record_batch();
    if batch.num_rows() == 0 {
        return Ok(None);
    }
    let timestamps = batch.column_values::<TimestampMicrosecondArray>("last_altered")?;
    let is_views = batch.column_values::<BooleanArray>("is_view")?;
    MetadataFreshness::from_micros(timestamps.value(0), is_views.value(0)).map(Some)
}

/// Extract `View Text` and `View Catalog and Namespace` from the row-based
/// `DESCRIBE EXTENDED <fqn>` output (columns: `col_name`, `data_type`, `comment`).
///
/// For non-view relations these rows are absent, in which case both returned
/// options are `None` and the caller should skip the relation.
fn parse_describe_extended_view_info(
    batch: &RecordBatch,
) -> AdapterResult<(Option<String>, Option<String>)> {
    let col_names = batch.column_values::<StringArray>("col_name")?;
    let data_types = batch.column_values::<StringArray>("data_type")?;

    let mut view_text: Option<String> = None;
    let mut catalog_and_ns: Option<String> = None;
    for i in 0..batch.num_rows() {
        match col_names.value(i) {
            "View Text" => view_text = Some(data_types.value(i).to_string()),
            "View Catalog and Namespace" => catalog_and_ns = Some(data_types.value(i).to_string()),
            _ => {}
        }
    }
    Ok((view_text, catalog_and_ns))
}

/// Resolve `(default_catalog, default_schema)` from the `View Catalog and Namespace`
/// row, falling back to the view's own FQN if the row is missing or unparseable.
///
/// The Databricks value comes back as a backtick-quoted two-part name like
/// `` `cat`.`schema` ``. The FQN fallback matches the dbt-databricks behavior of
/// preferring the view's own qualification over connection defaults when the
/// authoritative session-default record is unavailable. Returns `None` only
/// when both sources fail to parse, so the caller can log and skip the view
/// rather than emit a `ViewDefinition` with empty defaults.
fn parse_view_catalog_and_namespace(value: Option<&str>, fqn: &str) -> Option<(String, String)> {
    if let Some(s) = value {
        if let Ok(idents) = Dialect::Databricks.parse_dot_separated_identifiers(s) {
            if idents.len() == 2 {
                return Some((idents[0].name().to_string(), idents[1].name().to_string()));
            }
        }
    }

    let parsed = FullyQualifiedName::parse(fqn, Dialect::Databricks).ok()?;
    Some((
        parsed.catalog().name().to_string(),
        parsed.schema().name().to_string(),
    ))
}

/// Build a schema from `describe table [table]` (without extended ... as json)
fn build_schema_from_basic_describe_table(
    batch: Arc<RecordBatch>,
    type_ops: &dyn TypeOps,
) -> AdapterResult<Arc<Schema>> {
    let col_name = batch.column_values::<StringArray>("col_name")?;
    let data_type = batch.column_values::<StringArray>("data_type")?;
    let comments = batch.column_values::<StringArray>("comment")?;

    let mut fields: Vec<Field> = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        let name = col_name.value(i).to_string();
        let type_str = data_type.value(i).to_string();
        let comment = comments.value(i).to_string();

        let field = make_arrow_field_v2(type_ops, name, &type_str, None, Some(comment))?;
        fields.push(field);
    }

    Ok(Arc::new(Schema::new(fields)))
}

static DBR_VERSION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([1-9][0-9]*)\.(x|0|[1-9][0-9]*)").unwrap());

/// Extract DBR version from a spark version string using regex.
///
/// See: https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/handle.py#L273
fn extract_dbr_version(version_str: &str) -> AdapterResult<EngineVersion> {
    let caps = DBR_VERSION_REGEX.captures(version_str).ok_or_else(|| {
        AdapterError::new(
            AdapterErrorKind::Internal,
            format!("Failed to detect DBR version from: {version_str}"),
        )
    })?;

    let major: i64 = caps[1].parse().map_err(|_| {
        AdapterError::new(AdapterErrorKind::Internal, "Major version is not a number")
    })?;

    let minor_str = &caps[2];
    if minor_str == "x" {
        Ok(EngineVersion::Full(major, i64::MAX))
    } else {
        let minor: i64 = minor_str.parse().map_err(|_| {
            AdapterError::new(AdapterErrorKind::Internal, "Minor version is not a number")
        })?;
        Ok(EngineVersion::Full(major, minor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_describe_unsupported_is_retryable() {
        let error = AdapterError::new(
            AdapterErrorKind::Driver,
            "[UNSUPPORTED_FEATURE.DESC_JSON_TABLE_TYPE] DESCRIBE AS JSON not supported for table type of view",
        );
        assert!(is_unsupported_json_describe(&error));
    }

    #[test]
    fn plain_describe_fallback_hydrates_columns() {
        let batch = Arc::new(
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![
                    Field::new("col_name", DataType::Utf8, false),
                    Field::new("data_type", DataType::Utf8, false),
                    Field::new("comment", DataType::Utf8, true),
                ])),
                vec![
                    Arc::new(StringArray::from(vec!["id", "name"])),
                    Arc::new(StringArray::from(vec!["bigint", "string"])),
                    Arc::new(StringArray::from(vec!["", "customer name"])),
                ],
            )
            .unwrap(),
        );
        let schema = build_schema_from_basic_describe_table(
            batch,
            &crate::sql_types::DefaultTypeOps::new(AdapterType::Databricks),
        )
        .unwrap();
        assert_eq!(schema.fields().len(), 2);
        assert_eq!(schema.field(0).name(), "id");
        assert_eq!(schema.field(1).name(), "name");
    }

    #[test]
    fn json_describe_other_failures_are_not_retryable() {
        for (kind, message) in [
            (AdapterErrorKind::Authentication, "permission denied"),
            (AdapterErrorKind::Driver, "connection refused"),
            (
                AdapterErrorKind::Driver,
                "[UNSUPPORTED_FEATURE] unrelated operation",
            ),
            (AdapterErrorKind::Driver, "[TABLE_OR_VIEW_NOT_FOUND] view"),
        ] {
            let error = AdapterError::new(kind, message);
            assert!(!is_unsupported_json_describe(&error));
        }
    }

    #[test]
    fn json_describe_failed_fallback_preserves_original_error() {
        let json_error = AdapterError::new(
            AdapterErrorKind::Driver,
            "[UNSUPPORTED_FEATURE.DESC_JSON_TABLE_TYPE] DESCRIBE AS JSON not supported for table type of view",
        );
        let plain_error = AdapterError::new(AdapterErrorKind::Authentication, "permission denied");
        let err = describe_fallback_error(json_error, plain_error);
        assert!(err.message().contains("UNSUPPORTED_FEATURE"));
        assert!(err.message().contains("permission denied"));
    }
    use crate::relation::Relation;
    use crate::stmt_splitter::{DefaultStmtSplitter, StmtSplitter};
    use dbt_schemas::dbt_types::RelationType;
    use dbt_schemas::schemas::common::ResolvedQuoting;
    use dbt_schemas::schemas::relations::base::{BaseRelation, Policy};
    use std::sync::Arc;

    // Helper function to create a test relation with specific quoting policies
    fn create_test_relation(
        database: &str,
        schema: &str,
        identifier: &str,
        quote_database: bool,
        quote_schema: bool,
        quote_identifier: bool,
    ) -> Arc<dyn BaseRelation> {
        let quote_policy = ResolvedQuoting {
            database: quote_database,
            schema: quote_schema,
            identifier: quote_identifier,
        };

        Arc::new(
            Relation::new(
                AdapterType::Databricks,
                database.to_string(),
                schema.to_string(),
                identifier.to_string(),
            )
            .with_relation_type(RelationType::Table)
            .with_quoting(quote_policy),
        )
    }

    #[test]
    fn databricks_metadata_values_use_parser_independent_hex() {
        assert_eq!(databricks_metadata_value("DB_NAME"), "'db_name'");
        assert_eq!(
            databricks_metadata_value(r"DB\'; SELECT 1; --"),
            "decode(unhex('64625C273B2073656C65637420313B202D2D'), 'UTF-8')"
        );
        assert_eq!(
            databricks_metadata_identifier(r"DB\`; SELECT 1; --"),
            r"`db\``; select 1; --`"
        );
    }

    #[test]
    fn databricks_freshness_sql_escapes_identifiers_and_literals() {
        let sql = databricks_information_schema_freshness_sql(
            "db`; SELECT 1; --",
            r"schema\'; SELECT 2; --",
            r"table\'; SELECT 3; --",
        );

        assert!(sql.contains("FROM `db``; SELECT 1; --`.INFORMATION_SCHEMA.TABLES"));
        assert!(sql.contains(
            "table_schema = decode(unhex('736368656D615C273B2053454C45435420323B202D2D'), 'UTF-8')"
        ));
        assert!(sql.contains(
            "table_name = decode(unhex('7461626C655C273B2053454C45435420333B202D2D'), 'UTF-8')"
        ));
        assert_eq!(
            DefaultStmtSplitter.split(&sql, AdapterType::Databricks),
            vec![sql.as_str()]
        );
    }

    #[test]
    fn databricks_describe_history_keeps_quoted_relation_in_one_statement() {
        let relation = create_test_relation(
            "db`; SELECT 1; --",
            "schema`; SELECT 2; --",
            "table`; SELECT 3; --",
            true,
            true,
            true,
        );
        let sql = format!(
            "DESCRIBE HISTORY {} LIMIT 1",
            databricks_relation_sql_name(&relation).unwrap()
        );

        assert_eq!(
            DefaultStmtSplitter.split(&sql, AdapterType::Databricks),
            vec![sql.as_str()]
        );
        assert!(
            sql.contains("`db``; SELECT 1; --`.`schema``; SELECT 2; --`.`table``; SELECT 3; --`")
        );
    }

    #[test]
    fn databricks_relation_sql_name_honors_include_policy() {
        let relation = Arc::new(
            Relation::new(
                AdapterType::Databricks,
                "catalog".to_string(),
                "schema".to_string(),
                "table".to_string(),
            )
            .with_quoting(ResolvedQuoting::trues())
            .with_include_policy(Policy::new(false, true, true)),
        ) as Arc<dyn BaseRelation>;

        assert_eq!(
            databricks_relation_sql_name(&relation).unwrap(),
            "`schema`.`table`"
        );
    }

    #[test]
    fn databricks_relation_sql_name_quotes_unsafe_values_when_policy_is_false() {
        let relation = create_test_relation(
            "catalog`; SELECT 1; --",
            "schema`; SELECT 2; --",
            "table`; SELECT 3; --",
            false,
            false,
            false,
        );
        let rendered = databricks_relation_sql_name(&relation).unwrap();
        let sql = format!("SHOW TBLPROPERTIES {rendered}");

        assert_eq!(
            rendered,
            "`catalog``; SELECT 1; --`.`schema``; SELECT 2; --`.`table``; SELECT 3; --`"
        );
        assert_eq!(
            DefaultStmtSplitter.split(&sql, AdapterType::Databricks),
            vec![sql.as_str()]
        );
    }

    #[test]
    fn databricks_json_relation_quotes_unsafe_values_when_policy_is_false() {
        let relation = create_test_relation(
            "catalog`; SELECT 1; --",
            "schema`; SELECT 2; --",
            "table`; SELECT 3; --",
            false,
            false,
            false,
        );
        let rendered = databricks_safely_quoted_relation(&relation)
            .unwrap()
            .render_self_as_str();
        let sql = format!("DESCRIBE TABLE EXTENDED {rendered} AS JSON");

        assert_eq!(
            rendered,
            "`catalog``; select 1; --`.`schema``; select 2; --`.`table``; select 3; --`"
        );
        assert_eq!(
            DefaultStmtSplitter.split(&sql, AdapterType::Databricks),
            vec![sql.as_str()]
        );
    }

    #[test]
    fn databricks_query_identifier_quotes_unsafe_unquoted_values() {
        assert_eq!(databricks_query_identifier("safe_name", false), "safe_name");
        assert_eq!(
            databricks_query_identifier("table`; SELECT 1; --", false),
            "`table``; SELECT 1; --`"
        );
    }

    #[test]
    fn test_primary_key_constraints_to_agate_row_shape() {
        let rows = vec![PrimaryKeyConstraintRow {
            constraint_name: "pk1".to_string(),
            column_name: "id".to_string(),
        }];
        let table = primary_key_constraints_to_agate(&rows).unwrap();
        let table_rows: Vec<_> = table.rows().into_iter().collect();
        assert_eq!(table_rows.len(), 1);
        assert_eq!(
            table_rows[0].get_attr("constraint_name").unwrap().as_str(),
            Some("pk1")
        );
        assert_eq!(
            table_rows[0].get_attr("column_name").unwrap().as_str(),
            Some("id")
        );
    }

    #[test]
    fn test_primary_key_constraints_to_agate_empty() {
        let table = primary_key_constraints_to_agate(&[]).unwrap();
        assert_eq!(table.rows().into_iter().count(), 0);
    }

    #[test]
    fn test_foreign_key_constraints_to_agate_row_shape() {
        let rows = vec![ForeignKeyConstraintRow {
            constraint_name: "fk1".to_string(),
            from_column: "ref_id".to_string(),
            to_catalog: "main".to_string(),
            to_schema: "default".to_string(),
            to_table: "users".to_string(),
            to_column: "user_id".to_string(),
        }];
        let table = foreign_key_constraints_to_agate(&rows).unwrap();
        let table_rows: Vec<_> = table.rows().into_iter().collect();
        assert_eq!(table_rows.len(), 1);
        assert_eq!(
            table_rows[0].get_attr("constraint_name").unwrap().as_str(),
            Some("fk1")
        );
        assert_eq!(
            table_rows[0].get_attr("from_column").unwrap().as_str(),
            Some("ref_id")
        );
        assert_eq!(
            table_rows[0].get_attr("to_catalog").unwrap().as_str(),
            Some("main")
        );
        assert_eq!(
            table_rows[0].get_attr("to_schema").unwrap().as_str(),
            Some("default")
        );
        assert_eq!(
            table_rows[0].get_attr("to_table").unwrap().as_str(),
            Some("users")
        );
        assert_eq!(
            table_rows[0].get_attr("to_column").unwrap().as_str(),
            Some("user_id")
        );
    }

    #[test]
    fn test_non_null_constraints_to_agate_row_shape() {
        let rows = vec![NonNullConstraintRow {
            column_name: "id".to_string(),
        }];
        let table = non_null_constraints_to_agate(&rows).unwrap();
        let table_rows: Vec<_> = table.rows().into_iter().collect();
        assert_eq!(table_rows.len(), 1);
        assert_eq!(
            table_rows[0].get_attr("column_name").unwrap().as_str(),
            Some("id")
        );
    }

    #[test]
    fn test_column_masks_to_agate_row_shape_with_using_columns() {
        let rows = vec![ColumnMaskRow {
            column_name: "phone_number".to_string(),
            mask_name: "main.db.mask_phone".to_string(),
            using_columns: Some("city".to_string()),
        }];
        let table = column_masks_to_agate(&rows).unwrap();
        let table_rows: Vec<_> = table.rows().into_iter().collect();
        assert_eq!(table_rows.len(), 1);
        let indices = (Value::from(0), Value::from(1), Value::from(2));
        assert_eq!(
            table_rows[0].get_item(&indices.0).unwrap().as_str(),
            Some("phone_number")
        );
        assert_eq!(
            table_rows[0].get_item(&indices.1).unwrap().as_str(),
            Some("main.db.mask_phone")
        );
        assert_eq!(
            table_rows[0].get_item(&indices.2).unwrap().as_str(),
            Some("city")
        );
    }

    #[test]
    fn test_column_masks_to_agate_row_shape_without_using_columns() {
        let rows = vec![ColumnMaskRow {
            column_name: "ssn".to_string(),
            mask_name: "main.db.mask_ssn".to_string(),
            using_columns: None,
        }];
        let table = column_masks_to_agate(&rows).unwrap();
        let table_rows: Vec<_> = table.rows().into_iter().collect();
        assert_eq!(
            table_rows[0].get_item(&Value::from(2)).unwrap().as_str(),
            Some("")
        );
    }

    #[test]
    fn test_view_description_to_agate_some() {
        let row = Some(ViewDescriptionRow {
            view_definition: "SELECT 1".to_string(),
        });
        let table = view_description_to_agate(&row).unwrap();
        let table_rows: Vec<_> = table.rows().into_iter().collect();
        assert_eq!(table_rows.len(), 1);
        assert_eq!(
            table_rows[0].get_attr("view_definition").unwrap().as_str(),
            Some("SELECT 1")
        );
    }

    #[test]
    fn test_view_description_to_agate_none() {
        let table = view_description_to_agate(&None).unwrap();
        assert_eq!(table.rows().into_iter().count(), 0);
    }

    #[test]
    fn test_get_relation_with_quote_policy_no_quoting() {
        let relation =
            create_test_relation("test_db", "test_schema", "test_table", false, false, false);

        let (database, schema, identifier) = get_relation_with_quote_policy(&relation).unwrap();

        assert_eq!(database, "test_db");
        assert_eq!(schema, "test_schema");
        assert_eq!(identifier, "test_table");
    }

    #[test]
    fn test_get_relation_with_quote_policy_identifier_only() {
        let relation =
            create_test_relation("test_db", "test_schema", "test_table", false, false, true);

        let (database, schema, identifier) = get_relation_with_quote_policy(&relation).unwrap();

        assert_eq!(database, "test_db");
        assert_eq!(schema, "test_schema");
        assert_eq!(identifier, "`test_table`");
    }

    #[test]
    fn test_get_relation_with_quote_policy_all_parts() {
        let relation =
            create_test_relation("test_db", "test_schema", "test_table", true, true, true);

        let (database, schema, identifier) = get_relation_with_quote_policy(&relation).unwrap();

        assert_eq!(database, "`test_db`");
        assert_eq!(schema, "`test_schema`");
        assert_eq!(identifier, "`test_table`");
    }

    #[test]
    fn test_get_relation_with_quote_policy_all_parts_empty_db() {
        let relation = create_test_relation("", "test_schema", "test_table", true, true, true);

        let (database, schema, identifier) = get_relation_with_quote_policy(&relation).unwrap();

        assert_eq!(database, "");
        assert_eq!(schema, "`test_schema`");
        assert_eq!(identifier, "`test_table`");
    }

    #[test]
    fn test_get_relation_with_quote_policy_mixed_scenario() {
        let relation =
            create_test_relation("test_db", "test_schema", "test_table", true, false, true);

        let (database, schema, identifier) = get_relation_with_quote_policy(&relation).unwrap();

        assert_eq!(database, "`test_db`");
        assert_eq!(schema, "test_schema");
        assert_eq!(identifier, "`test_table`");
    }

    #[test]
    fn test_get_relation_with_quote_policy_with_special_characters() {
        let relation =
            create_test_relation("test-db", "test-schema", "test-table", false, false, true);

        let (database, schema, identifier) = get_relation_with_quote_policy(&relation).unwrap();

        assert_eq!(database, "`test-db`");
        assert_eq!(schema, "`test-schema`");
        assert_eq!(identifier, "`test-table`");
    }

    #[test]
    fn test_get_relation_with_quote_policy_with_reserved_keywords() {
        let relation = create_test_relation("order", "select", "table", true, true, true);

        let (database, schema, identifier) = get_relation_with_quote_policy(&relation).unwrap();

        assert_eq!(database, "`order`");
        assert_eq!(schema, "`select`");
        assert_eq!(identifier, "`table`");
    }

    #[test]
    fn test_get_relation_with_quote_policy_database_schema_only() {
        let relation =
            create_test_relation("test_db", "test_schema", "test_table", true, true, false);

        let (database, schema, identifier) = get_relation_with_quote_policy(&relation).unwrap();

        assert_eq!(database, "`test_db`");
        assert_eq!(schema, "`test_schema`");
        assert_eq!(identifier, "test_table");
    }

    #[test]
    fn test_extract_dbr_version_with_scala() {
        // Format: "15.4.x-scala2.12" → regex finds "15.4" first → (15, 4)
        let result = extract_dbr_version("15.4.x-scala2.12").unwrap();
        assert_eq!(result, EngineVersion::Full(15, 4));
    }

    #[test]
    fn test_extract_dbr_version_with_gpu_ml() {
        // Format: "15.4.x-gpu-ml-scala2.12" → regex finds "15.4" first → (15, 4)
        let result = extract_dbr_version("15.4.x-gpu-ml-scala2.12").unwrap();
        assert_eq!(result, EngineVersion::Full(15, 4));
    }

    #[test]
    fn test_extract_dbr_version_full_version() {
        // Format: "16.2-scala2.12" → (16, 2)
        let result = extract_dbr_version("16.2-scala2.12").unwrap();
        assert_eq!(result, EngineVersion::Full(16, 2));
    }

    #[test]
    fn test_extract_dbr_version_simple() {
        // Simple format without suffix
        let result = extract_dbr_version("16.2").unwrap();
        assert_eq!(result, EngineVersion::Full(16, 2));
    }

    #[test]
    fn test_extract_dbr_version_with_x_minor() {
        // Format: "16.x-scala2.12" → minor is "x" → (16, i64::MAX)
        // This matches Python's (16, sys.maxsize) behavior
        let result = extract_dbr_version("16.x-scala2.12").unwrap();
        assert_eq!(result, EngineVersion::Full(16, i64::MAX));
    }

    #[test]
    fn parse_view_catalog_and_namespace_parses_quoted_pair() {
        let parsed = parse_view_catalog_and_namespace(
            Some("`my_catalog`.`my_schema`"),
            "`my_catalog`.`my_schema`.`my_view`",
        );
        assert_eq!(
            parsed,
            Some(("my_catalog".to_string(), "my_schema".to_string()))
        );
    }

    #[test]
    fn parse_view_catalog_and_namespace_parses_unquoted_pair() {
        let parsed = parse_view_catalog_and_namespace(
            Some("my_catalog.my_schema"),
            "`my_catalog`.`my_schema`.`my_view`",
        );
        assert_eq!(
            parsed,
            Some(("my_catalog".to_string(), "my_schema".to_string()))
        );
    }

    #[test]
    fn parse_view_catalog_and_namespace_falls_back_to_fqn_on_garbage_value() {
        let parsed = parse_view_catalog_and_namespace(
            Some("not a valid name"),
            "`fallback_cat`.`fallback_schema`.`my_view`",
        );
        assert_eq!(
            parsed,
            Some(("fallback_cat".to_string(), "fallback_schema".to_string()))
        );
    }

    #[test]
    fn parse_view_catalog_and_namespace_falls_back_to_fqn_on_missing_value() {
        let parsed =
            parse_view_catalog_and_namespace(None, "`fallback_cat`.`fallback_schema`.`my_view`");
        assert_eq!(
            parsed,
            Some(("fallback_cat".to_string(), "fallback_schema".to_string()))
        );
    }

    #[test]
    fn parse_view_catalog_and_namespace_returns_none_when_both_unparseable() {
        // Value cannot be parsed AND the fqn cannot be parsed either, so the
        // helper has nothing to fall back to.
        assert_eq!(parse_view_catalog_and_namespace(None, ""), None);
    }

    #[test]
    fn parse_describe_extended_view_info_extracts_view_rows() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("col_name", DataType::Utf8, false),
            Field::new("data_type", DataType::Utf8, false),
            Field::new("comment", DataType::Utf8, true),
        ]));
        let col_names = StringArray::from(vec![
            "id",
            "name",
            "# Detailed Table Information",
            "Catalog",
            "View Text",
            "View Catalog and Namespace",
            "View Query Output Columns",
        ]);
        let data_types = StringArray::from(vec![
            "bigint",
            "string",
            "",
            "main",
            "SELECT 1 AS x",
            "`main`.`default`",
            "[\"x\"]",
        ]);
        let comments = StringArray::from(vec!["", "", "", "", "", "", ""]);
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(col_names),
                Arc::new(data_types),
                Arc::new(comments),
            ],
        )
        .unwrap();

        let (view_text, catalog_and_ns) = parse_describe_extended_view_info(&batch).unwrap();
        assert_eq!(view_text.as_deref(), Some("SELECT 1 AS x"));
        assert_eq!(catalog_and_ns.as_deref(), Some("`main`.`default`"));
    }

    #[test]
    fn parse_describe_extended_view_info_returns_none_for_table_output() {
        // Table DESCRIBE EXTENDED has no `View Text` row.
        let schema = Arc::new(Schema::new(vec![
            Field::new("col_name", DataType::Utf8, false),
            Field::new("data_type", DataType::Utf8, false),
            Field::new("comment", DataType::Utf8, true),
        ]));
        let col_names = StringArray::from(vec!["id", "# Detailed Table Information", "Catalog"]);
        let data_types = StringArray::from(vec!["bigint", "", "main"]);
        let comments = StringArray::from(vec!["", "", ""]);
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(col_names),
                Arc::new(data_types),
                Arc::new(comments),
            ],
        )
        .unwrap();

        let (view_text, catalog_and_ns) = parse_describe_extended_view_info(&batch).unwrap();
        assert!(view_text.is_none());
        assert!(catalog_and_ns.is_none());
    }

    // ── epoch_ms_from_history_batch ──────────────────────────────────────────

    fn make_history_batch_ms(epoch_ms: i64) -> Arc<RecordBatch> {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "timestamp",
            DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None),
            true,
        )]));
        Arc::new(
            RecordBatch::try_new(
                schema,
                vec![Arc::new(TimestampMillisecondArray::from(vec![epoch_ms]))],
            )
            .unwrap(),
        )
    }

    fn make_history_batch_us(epoch_us: i64) -> Arc<RecordBatch> {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "timestamp",
            DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
            true,
        )]));
        Arc::new(
            RecordBatch::try_new(
                schema,
                vec![Arc::new(TimestampMicrosecondArray::from(vec![epoch_us]))],
            )
            .unwrap(),
        )
    }

    fn make_history_batch_ns(epoch_ns: i64) -> Arc<RecordBatch> {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "timestamp",
            DataType::Timestamp(arrow_schema::TimeUnit::Nanosecond, None),
            true,
        )]));
        Arc::new(
            RecordBatch::try_new(
                schema,
                vec![Arc::new(TimestampNanosecondArray::from(vec![epoch_ns]))],
            )
            .unwrap(),
        )
    }

    fn make_history_batch_s(epoch_s: i64) -> Arc<RecordBatch> {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "timestamp",
            DataType::Timestamp(arrow_schema::TimeUnit::Second, None),
            true,
        )]));
        Arc::new(
            RecordBatch::try_new(
                schema,
                vec![Arc::new(TimestampSecondArray::from(vec![epoch_s]))],
            )
            .unwrap(),
        )
    }

    #[test]
    fn epoch_ms_from_history_batch_millisecond_precision() {
        let batch = make_history_batch_ms(1_700_000_000_123);
        assert_eq!(epoch_ms_from_history_batch(&batch), Some(1_700_000_000_123));
    }

    #[test]
    fn epoch_ms_from_history_batch_microsecond_precision() {
        let batch = make_history_batch_us(1_700_000_000_123_456);
        assert_eq!(epoch_ms_from_history_batch(&batch), Some(1_700_000_000_123));
    }

    #[test]
    fn epoch_ms_from_history_batch_nanosecond_precision() {
        let batch = make_history_batch_ns(1_700_000_000_123_000_000);
        assert_eq!(epoch_ms_from_history_batch(&batch), Some(1_700_000_000_123));
    }

    #[test]
    fn epoch_ms_from_history_batch_second_precision() {
        let batch = make_history_batch_s(1_700_000_000);
        assert_eq!(epoch_ms_from_history_batch(&batch), Some(1_700_000_000_000));
    }

    #[test]
    fn epoch_ms_from_history_batch_empty_returns_none() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "timestamp",
            DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None),
            true,
        )]));
        let batch = Arc::new(
            RecordBatch::try_new(
                schema,
                vec![Arc::new(TimestampMillisecondArray::from(Vec::<i64>::new()))],
            )
            .unwrap(),
        );
        assert!(epoch_ms_from_history_batch(&batch).is_none());
    }

    #[test]
    fn epoch_ms_from_history_batch_null_value_returns_none() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "timestamp",
            DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None),
            true,
        )]));
        let batch = Arc::new(
            RecordBatch::try_new(
                schema,
                vec![Arc::new(TimestampMillisecondArray::from(vec![None::<i64>]))],
            )
            .unwrap(),
        );
        assert!(epoch_ms_from_history_batch(&batch).is_none());
    }

    // ── partition_override_relations ─────────────────────────────────────────

    fn make_relation(fqn: &str) -> Arc<dyn BaseRelation> {
        // Build a relation whose semantic_fqn() matches `fqn` by using the three
        // parts as database / schema / identifier with no quoting.
        let parts: Vec<&str> = fqn.splitn(3, '.').collect();
        let (db, schema, id) = match parts.as_slice() {
            [db, sc, id] => (*db, *sc, *id),
            _ => panic!("fqn must be db.schema.identifier, got: {fqn}"),
        };
        create_test_relation(db, schema, id, false, false, false)
    }

    #[test]
    fn partition_routes_all_to_bulk_when_no_overrides() {
        let relations: Vec<Arc<dyn BaseRelation>> =
            vec![make_relation("db.sc.a"), make_relation("db.sc.b")];
        let overrides = BTreeMap::new();
        let (bulk, targets) = partition_override_relations(&relations, &overrides);
        assert_eq!(bulk.len(), 2);
        assert!(targets.is_empty());
    }

    #[test]
    fn partition_routes_override_relation_to_override_bucket() {
        let r = make_relation("db.sc.src");
        let fqn = r.semantic_fqn();
        let relations = vec![r];
        let mut overrides = BTreeMap::new();
        overrides.insert(
            fqn.clone(),
            FreshnessOverride::Field {
                field: "updated_at".into(),
                filter: None,
            },
        );
        let (bulk, targets) = partition_override_relations(&relations, &overrides);
        assert!(bulk.is_empty());
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0.semantic_fqn(), fqn);
        assert!(
            matches!(&targets[0].1, FreshnessOverride::Field { field, .. } if field == "updated_at")
        );
    }

    #[test]
    fn partition_mixed_relations_correctly_sorted() {
        let r_bulk1 = make_relation("db.sc.bulk1");
        let r_ovr = make_relation("db.sc.ovr");
        let r_bulk2 = make_relation("db.sc.bulk2");
        let fqn_ovr = r_ovr.semantic_fqn();

        let relations = vec![
            Arc::clone(&r_bulk1),
            Arc::clone(&r_ovr),
            Arc::clone(&r_bulk2),
        ];
        let mut overrides = BTreeMap::new();
        overrides.insert(
            fqn_ovr.clone(),
            FreshnessOverride::Query("SELECT max(ts) FROM {{ this }}".into()),
        );

        let (bulk, targets) = partition_override_relations(&relations, &overrides);

        assert_eq!(bulk.len(), 2);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0.semantic_fqn(), fqn_ovr);
        // bulk order is preserved
        assert_eq!(bulk[0].semantic_fqn(), r_bulk1.semantic_fqn());
        assert_eq!(bulk[1].semantic_fqn(), r_bulk2.semantic_fqn());
    }

    #[test]
    fn relation_not_found_requires_sqlstate_with_typed_kind() {
        assert!(is_databricks_relation_not_found(
            &AdapterError::new_with_sqlstate_and_vendor_code(
                AdapterErrorKind::NotFound,
                "no text".to_string(),
                *b"42P01",
                None
            )
        ));
        // missing catalog or schema: NotFound with another SQLSTATE
        assert!(!is_databricks_relation_not_found(
            &AdapterError::new_with_sqlstate_and_vendor_code(
                AdapterErrorKind::NotFound,
                "The schema `s` cannot be found.".to_string(),
                *b"42704",
                None
            )
        ));
        assert!(!is_databricks_relation_not_found(&AdapterError::new(
            AdapterErrorKind::NotFound,
            "no sqlstate"
        )));
    }

    #[test]
    fn describing_a_missing_relation_catalog_or_schema_is_not_an_error() {
        assert!(is_relation_missing(&AdapterError::new(
            AdapterErrorKind::NotFound,
            "The schema `s` cannot be found."
        )));
        assert!(is_relation_missing(&AdapterError::new(
            AdapterErrorKind::Driver,
            "[TABLE_OR_VIEW_NOT_FOUND] The table or view `t` cannot be found."
        )));
        assert!(!is_relation_missing(&AdapterError::new(
            AdapterErrorKind::Driver,
            "[PARSE_SYNTAX_ERROR] bad"
        )));
    }

    #[test]
    fn relation_not_found_keeps_legacy_text_match() {
        assert!(is_databricks_relation_not_found(&AdapterError::new(
            AdapterErrorKind::Driver,
            "[TABLE_OR_VIEW_NOT_FOUND] The table or view `t` cannot be found."
        )));
        assert!(!is_databricks_relation_not_found(&AdapterError::new(
            AdapterErrorKind::Driver,
            "[PARSE_SYNTAX_ERROR] bad"
        )));
    }
}
