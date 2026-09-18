use std::collections::BTreeMap;
use std::sync::Arc;

use arrow::array::{Array as _, StringArray};
use dbt_adapter_core::AdapterType;
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::normalize_quote;
use dbt_schemas::schemas::relations::base::{BaseRelation, Policy, TableFormat};
use minijinja::State;

use crate::adapter::adapter_impl::AdapterImpl;
use crate::errors::adbc_error_to_adapter_error;
use crate::formatter::SqlLiteralFormatter;
use crate::metadata::bigquery;
use crate::metadata::bigquery::is_bigquery_not_found_error;
use crate::metadata::databricks::describe_table::DatabricksTableMetadata;
use crate::metadata::{snowflake, try_canonicalize_bool_column_field};
use crate::record_batch::RecordBatchExt;
use crate::relation::Relation;
use crate::relation::databricks::is_shallow_clone_type;
use crate::relation::do_create_relation;
use dbt_common::cancellation::CancellationToken;

macro_rules! invalid_value {
    ($msg:expr) => {
        Err(AdapterError::new(AdapterErrorKind::UnexpectedResult, $msg))
    };

    ($($arg:tt)*) => {
        Err(AdapterError::new(AdapterErrorKind::UnexpectedResult, format!($($arg)*)))
    };
}

// TODO: turn this into a struct and collapse all the common code from X_get_relation functions

#[inline(never)]
#[allow(clippy::too_many_arguments)]
pub fn get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    match adapter.adapter_type() {
        AdapterType::Snowflake => snowflake_get_relation(
            adapter, state, ctx, conn, database, schema, identifier, token,
        ),
        AdapterType::Bigquery => bigquery_get_relation_via_adbc(
            adapter, state, conn, database, schema, identifier, token,
        ),
        AdapterType::Databricks => databricks_get_relation(
            adapter, state, ctx, conn, database, schema, identifier, token,
        ),
        AdapterType::Redshift => redshift_get_relation(
            adapter, state, ctx, conn, database, schema, identifier, token,
        ),
        AdapterType::Postgres => postgres_get_relation(
            adapter, state, ctx, conn, database, schema, identifier, token,
        ),
        AdapterType::Salesforce => {
            salesforce_get_relation(adapter, state, ctx, conn, database, schema, identifier)
        }
        AdapterType::Spark => {
            spark_get_relation(adapter, state, ctx, conn, schema, identifier, token)
        }
        AdapterType::DuckDB | AdapterType::LakeCompute | AdapterType::GizmoSQL => {
            duckdb_get_relation(
                adapter, state, ctx, conn, database, schema, identifier, token,
            )
        }
        AdapterType::Fabric => fabric_get_relation(
            adapter, state, ctx, conn, database, schema, identifier, token,
        ),
        AdapterType::ClickHouse => clickhouse_get_relation(
            adapter, state, ctx, conn, database, schema, identifier, token,
        ),
        AdapterType::Exasol => exasol_get_relation(
            adapter, state, ctx, conn, database, schema, identifier, token,
        ),
        AdapterType::Starburst => todo!("Starburst"),
        AdapterType::Athena => todo!("Athena"),
        AdapterType::Trino => todo!("Trino"),
        AdapterType::Dremio => todo!("Dremio"),
        AdapterType::Oracle => todo!("Oracle"),
        AdapterType::Datafusion => todo!("Datafusion"),
    }
}

/// Parses an `INFORMATION_SCHEMA.ROUTINES`-shaped result batch into a
/// [`Relation`].
pub(crate) fn relation_from_routines_batch(
    adapter: &AdapterImpl,
    database: &str,
    schema: &str,
    identifier: &str,
    batch: &arrow::record_batch::RecordBatch,
) -> AdapterResult<Option<Relation>> {
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let column = batch.column_by_name("table_type").unwrap();
    let string_array = column.as_any().downcast_ref::<StringArray>().unwrap();
    let relation_type_name = string_array.value(0).to_uppercase();
    let relation_type =
        RelationType::from_adapter_type(adapter.adapter_type(), &relation_type_name);

    Ok(Some(
        Relation::new(
            adapter.adapter_type(),
            database.to_string(),
            schema.to_string(),
            identifier.to_string(),
        )
        .with_relation_type(relation_type)
        .with_quoting(adapter.quoting()),
    ))
}

// https://github.com/dbt-labs/dbt-adapters/blob/ace1709df001df4232a66f9d5f331a5fda4d3389/dbt-snowflake/src/dbt/include/snowflake/macros/adapters.sql#L138
#[allow(clippy::too_many_arguments)]
fn snowflake_get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    let quoted_database = if adapter.quoting().database {
        adapter.quote(database)
    } else {
        database.to_string()
    };
    let quoted_schema = if adapter.quoting().schema {
        adapter.quote(schema)
    } else {
        schema.to_string()
    };
    let quoted_identifier = if adapter.quoting().identifier {
        identifier.to_string()
    } else {
        identifier.to_uppercase()
    };
    // this is a case-insenstive search
    let lit_fmt = SqlLiteralFormatter::new(adapter.adapter_type());
    let sql = format!(
        "show objects like {} in schema {quoted_database}.{quoted_schema}",
        lit_fmt.format_str(&quoted_identifier)
    );

    let batch = match adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token)
    {
        Ok(b) => b,
        Err(e) => {
            // Previous versions of this code [1] checked the prefix of the error message
            // and looked for "002043 (02000)", but now we can compare the SQLSTATE and
            // vendor code directly.
            //
            // SQLSTATE "02000" means "no data" [1].
            // "002043" is the Snowflake code for "object does not exist or is not found".
            //
            // This error happens specifically when the specified DATABASE.SCHEMA does not exist.
            // If the schema does exist, then the query succeeds and will return zero or more rows.
            //
            // [1] https://github.com/dbt-labs/dbt-adapters/blob/5181389e4d4e2f9649026502bb685741a1c19a8e/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L259
            // [2] https://en.wikipedia.org/wiki/SQLSTATE
            if e.sqlstate() == "02000" && e.vendor_code().is_some_and(|code| code == 2043) {
                return Ok(None);
            } else {
                // Other errors should be propagated
                return Err(e);
            }
        }
    };

    // Handle case where the query succeeds, but no rows are returned.
    // This happens when no objects are LIKE the specified identifier.
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let kind_column = batch.column_values::<StringArray>("kind")?;

    if kind_column.len() != 1 {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "Did not find expected column 'kind' in 'show objects' query result",
        ));
    }

    // Reference: https://github.com/dbt-labs/dbt-adapters/blob/61221f455f5960daf80024febfae6d6fb4b46251/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L309
    // TODO: We'll have to revisit this when iceberg gets implemented.
    let is_dynamic_column = batch.column_values::<StringArray>("is_dynamic")?;

    if is_dynamic_column.len() != 1 {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "Did not find expected column 'is_dynamic' in 'show objects' query result",
        ));
    }
    let is_dynamic = is_dynamic_column.value(0);

    // See `snowflake::relation_type_from_table_flags`'s doc.
    let is_interactive = batch
        .column_values::<StringArray>("is_interactive")
        .ok()
        .filter(|col| col.len() == 1)
        .map(|col| col.value(0).to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "n".to_string());

    let relation_type_name = kind_column.value(0);
    let relation_type = if relation_type_name.eq_ignore_ascii_case("table") {
        Some(snowflake::relation_type_from_table_flags(
            is_dynamic,
            &is_interactive,
        )?)
    } else if relation_type_name.eq_ignore_ascii_case("view") {
        Some(RelationType::View)
    } else {
        None
    };

    let is_iceberg_column = batch.column_values::<StringArray>("is_iceberg")?;

    if is_iceberg_column.len() != 1 {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "Did not find expected column 'is_iceberg' in 'show objects' query result",
        ));
    }

    let is_iceberg = is_iceberg_column.value(0);
    let table_format = if try_canonicalize_bool_column_field(is_iceberg)? {
        TableFormat::Iceberg
    } else {
        TableFormat::Default
    };

    let relation = Relation::new(
        AdapterType::Snowflake,
        database.to_string(),
        schema.to_string(),
        identifier.to_string(),
    )
    .with_relation_type(relation_type)
    .with_quoting(adapter.quoting())
    .with_table_format(table_format);
    Ok(Some(Box::new(relation)))
}

// TODO: This uses GetTableSchema, which does not find routines.
// The Python implementation also returns routines and functions as relations.
// See: https://github.com/dbt-labs/dbt-adapters/blob/a375bd590133e6756952c82ea0652c3e62a6562a/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L455
#[allow(clippy::too_many_arguments)]
fn bigquery_get_relation_via_adbc(
    adapter: &AdapterImpl,
    state: &State,
    conn: &'_ mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    // The driver expects unquoted values, so regardless of the adapter's config
    // we need to strip quotes.
    let (catalog, _) = normalize_quote(false, adapter.adapter_type(), database);
    let (db_schema, _) = normalize_quote(false, adapter.adapter_type(), schema);
    let (table_name, _) = normalize_quote(false, adapter.adapter_type(), identifier);

    // If GetTableSchema fails to find the table, it returns an error like this:
    // [BigQuery] googleapi: Error 404: Not found: Table <table>, notFound
    // TODO: Instead of string-matching on the error, we could have the driver
    // set e.status = StatusNotFound and match on that.
    let relation_schema = match conn.get_table_schema(Some(&catalog), Some(&db_schema), &table_name)
    {
        Ok(relation_schema) => relation_schema,
        Err(e) => {
            let adapter_err = adbc_error_to_adapter_error(e);
            if is_bigquery_not_found_error(&adapter_err) {
                if adapter.engine().relation_cache().project_has_functions() {
                    return bigquery::get_relation_routine_fallback(
                        adapter, state, conn, database, schema, identifier, token,
                    );
                }
                return Ok(None);
            } else {
                return Err(adapter_err);
            }
        }
    };

    let relation_type_name = relation_schema.metadata().get("Type").ok_or_else(|| {
        AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "Expected schema to expose relation type",
        )
    })?;

    let relation_type = RelationType::from_adapter_type(AdapterType::Bigquery, relation_type_name);

    let mut relation = Box::new(
        Relation::new(
            AdapterType::Bigquery,
            database.to_string(),
            schema.to_string(),
            identifier.to_string(),
        )
        .with_relation_type(relation_type)
        .with_quoting(adapter.quoting()),
    );

    // TODO: This is loaded lazily in dbt Core when required by certain macros:
    // Ex: https://github.com/dbt-labs/dbt-adapters/blob/9fce78f44db248ba33832c0f65c884a5139c0169/dbt-bigquery/src/dbt/include/bigquery/macros/adapters/apply_grants.sql#L2-L3
    let location = adapter.get_dataset_location(state, conn, relation.as_ref(), token)?;
    relation.location = location;
    Ok(Some(relation))
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)]
fn bigquery_get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    let query_database = if adapter.quoting().database {
        adapter.quote(database)
    } else {
        database.to_string()
    };
    let query_schema = if adapter.quoting().schema {
        adapter.quote(schema)
    } else {
        schema.to_string()
    };

    let query_identifier = if adapter.quoting().identifier {
        identifier.to_string()
    } else {
        identifier.to_lowercase()
    };

    let sql = format!(
        "SELECT table_catalog,
                    table_schema,
                    table_name,
                    table_type
                FROM {query_database}.{query_schema}.INFORMATION_SCHEMA.TABLES
                WHERE table_name = '{query_identifier}';",
    );

    let result = adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token.clone());
    let batch = match result {
        Ok(batch) => batch,
        Err(err) => {
            let err_msg = err.to_string();
            if err_msg.contains("Dataset") && err_msg.contains("was not found") {
                return Ok(None);
            } else {
                return Err(err);
            }
        }
    };

    if batch.num_rows() == 0 {
        // If there are no rows, then we did not find the object
        return Ok(None);
    }

    let string_array = batch.column_values::<StringArray>("table_type")?;

    let relation_type_name = string_array.value(0).to_uppercase();
    let relation_type = RelationType::from_adapter_type(AdapterType::Bigquery, &relation_type_name);

    let mut relation = Box::new(
        Relation::new(
            AdapterType::Bigquery,
            database.to_string(),
            schema.to_string(),
            identifier.to_string(),
        )
        .with_relation_type(relation_type)
        .with_quoting(adapter.quoting()),
    );
    let location = adapter.get_dataset_location(state, conn, relation.as_ref(), token)?;
    relation.location = location;
    Ok(Some(relation))
}

// Parses non-JSON `DESCRIBE TABLE EXTENDED` output, shared by Spark and older
// Databricks runtimes without `AS JSON`. Output layout (col_name/data_type):
// column defs, empty separator row, then metadata key-value pairs. Some catalogs
// (e.g. hive_metastore) omit 'Type'/'Provider', so we default gracefully.
fn parse_describe_table_extended(
    adapter_type: AdapterType,
    batch: &arrow::record_batch::RecordBatch,
) -> AdapterResult<(RelationType, bool, bool, BTreeMap<String, String>)> {
    let col_names = batch.column_values::<StringArray>("col_name")?;
    let data_types = batch.column_values::<StringArray>("data_type")?;

    let mut metadata_map = BTreeMap::new();
    let mut type_str = None;
    let mut provider_str = None;
    let mut in_metadata_section = false;

    for i in 0..batch.num_rows() {
        let key = col_names.value(i).trim();
        let value = data_types.value(i).trim();

        if key.is_empty() {
            in_metadata_section = true;
            continue;
        }
        if !in_metadata_section || key.starts_with('#') {
            continue;
        }

        metadata_map.insert(key.to_string(), value.to_string());
        match key {
            "Type" => type_str = Some(value.to_string()),
            "Provider" => provider_str = Some(value.to_string()),
            _ => {}
        }
    }

    let relation_type = match type_str.as_deref() {
        Some(t) => RelationType::from_adapter_type(adapter_type, t),
        None => RelationType::Table,
    };
    let is_delta = provider_str.as_deref() == Some("delta");
    let is_shallow_clone = type_str.as_deref().is_some_and(is_shallow_clone_type);

    Ok((relation_type, is_delta, is_shallow_clone, metadata_map))
}

fn spark_get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &mut dyn Connection,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    let query_schema = if adapter.quoting().schema {
        adapter.quote(schema)
    } else {
        schema.to_string()
    };
    let query_identifier = if adapter.quoting().identifier {
        adapter.quote(identifier)
    } else {
        identifier.to_string()
    };

    // Spark 3.5 does not support AS JSON
    let sql = format!("DESCRIBE TABLE EXTENDED {query_schema}.{query_identifier}");
    let batch = adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token);
    if let Err(e) = &batch
        && (e.to_string().contains("cannot be found")
            || e.to_string().contains("TABLE_OR_VIEW_NOT_FOUND")
            || e.to_string().contains("UnresolvedTableOrView"))
    {
        return Ok(None);
    }
    let batch = batch?;
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let (relation_type, is_delta, is_shallow_clone, metadata) =
        parse_describe_table_extended(AdapterType::Spark, &batch)?;

    Ok(Some(Box::new(
        Relation::new(
            AdapterType::Spark,
            None::<String>,
            schema.to_string(),
            identifier.to_string(),
        )
        .with_relation_type(Some(relation_type))
        .with_quoting(adapter.quoting())
        .with_metadata(Some(metadata))
        .with_is_delta(is_delta)
        .with_is_shallow_clone(is_shallow_clone),
    )))
}

#[allow(clippy::too_many_arguments)]
fn databricks_get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    use crate::metadata::MetadataProcessor as _;
    use crate::metadata::databricks::DatabricksMetadataAdapter;
    use crate::metadata::databricks::version::EngineVersion;
    use crate::relation::databricks::{INFORMATION_SCHEMA_SCHEMA, SYSTEM_DATABASE};

    // This function is only called when full metadata is needed. See https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/impl.py#L418

    let query_catalog = if adapter.quoting().database {
        adapter.quote(database)
    } else {
        database.to_string()
    };
    let query_schema = if adapter.quoting().schema {
        adapter.quote(schema)
    } else {
        schema.to_string()
    };
    let query_identifier = if adapter.quoting().identifier {
        adapter.quote(identifier)
    } else {
        identifier.to_string()
    };

    // Determine whether `DESCRIBE TABLE EXTENDED ... AS JSON` is supported.
    // This mirrors the safety checks in list_relations_schemas_inner:
    // - External system tables (system.information_schema.*) don't support AS JSON
    // - DBR versions < 16.2 don't support AS JSON
    // - Spark adapter: skip version check (no DBR version concept)
    // See also: https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/impl.py#L423
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

    let sql = if as_json_unsupported {
        format!("DESCRIBE TABLE EXTENDED {fqn}")
    } else {
        format!("DESCRIBE TABLE EXTENDED {fqn} AS JSON")
    };

    let batch = adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token);
    if let Err(e) = &batch
        && (e.to_string().contains("cannot be found")
            || e.to_string().contains("TABLE_OR_VIEW_NOT_FOUND"))
    {
        return Ok(None);
    }
    let batch = batch?;
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let (relation_type, is_delta, is_shallow_clone, metadata) = if as_json_unsupported {
        let (relation_type, is_delta, is_shallow_clone, metadata_map) =
            parse_describe_table_extended(adapter.adapter_type(), &batch)?;
        (
            Some(relation_type),
            is_delta,
            is_shallow_clone,
            Some(metadata_map),
        )
    } else {
        debug_assert_eq!(batch.num_rows(), 1);
        let json_metadata = DatabricksTableMetadata::from_record_batch(Arc::new(batch))?;
        let is_delta = json_metadata.provider.as_deref() == Some("delta");
        let is_shallow_clone = is_shallow_clone_type(&json_metadata.type_);
        let relation_type = Some(RelationType::from_adapter_type(
            adapter.adapter_type(),
            &json_metadata.type_,
        ));
        (
            relation_type,
            is_delta,
            is_shallow_clone,
            Some(json_metadata.into_metadata()),
        )
    };

    let db = if database.is_empty() {
        None
    } else {
        Some(database.to_string())
    };

    Ok(Some(Box::new(
        Relation::new(
            adapter.adapter_type(),
            db,
            schema.to_string(),
            identifier.to_string(),
        )
        .with_relation_type(relation_type)
        .with_quoting(adapter.quoting())
        .with_metadata(metadata)
        .with_is_delta(is_delta)
        .with_is_shallow_clone(is_shallow_clone),
    )))
}

#[allow(clippy::too_many_arguments)]
fn redshift_get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    let query_schema = if adapter.quoting().schema {
        schema.to_string()
    } else {
        schema.to_lowercase()
    };

    // determine table, view, or materialized view
    let sql = format!(
        "WITH materialized_views AS (
    SELECT TRIM(name) AS object_name, 'materialized_view'::text AS object_type
    FROM svv_mv_info
    WHERE TRIM(schema_name) ILIKE '{query_schema}'
        AND TRIM(name) ILIKE '{identifier}'
),
all_objects AS (
    SELECT table_name AS object_name,
        CASE
            WHEN table_type ILIKE 'BASE TABLE' THEN 'table'::text
            WHEN table_type ILIKE 'VIEW' THEN 'view'::text
            ELSE 'table'
        END AS object_type
    FROM svv_tables
    WHERE table_schema ILIKE '{query_schema}'
        AND table_name ILIKE '{identifier}'
)
SELECT
    COALESCE(mv.object_name, ao.object_name) AS object_name,
    COALESCE(mv.object_type, ao.object_type) AS object_type
FROM all_objects ao
LEFT JOIN materialized_views mv
    ON ao.object_name = mv.object_name"
    );

    let batch = adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token)?;

    if batch.num_rows() == 0 {
        // If there are no rows, then we did not find the object
        return Ok(None);
    }

    let string_array = batch.column_values::<StringArray>("object_type")?;

    if string_array.len() != 1 {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "Did not find 'object_type' for a relation",
        ));
    }

    let relation_type_name = string_array.value(0).to_lowercase();
    let relation_type = match relation_type_name.as_str() {
        "table" => Some(RelationType::Table),
        "view" => Some(RelationType::View),
        "materialized_view" => Some(RelationType::MaterializedView),
        _ => None,
    };

    let relation = Relation::new(
        AdapterType::Redshift,
        database.to_string(),
        schema.to_string(),
        identifier.to_string(),
    )
    .with_relation_type(relation_type)
    .with_quoting(adapter.quoting())
    .validate()?;
    Ok(Some(Box::new(relation)))
}

// reference: https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-postgres/src/dbt/include/postgres/macros/adapters.sql#L85
#[allow(clippy::too_many_arguments)]
fn postgres_get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    let query_schema = if adapter.quoting().schema {
        schema.to_string()
    } else {
        schema.to_lowercase()
    };

    let query_identifier = if adapter.quoting().identifier {
        identifier.to_string()
    } else {
        identifier.to_lowercase()
    };

    let sql = format!(
        r#"
            select 'table' as type
            from pg_tables
            where schemaname = '{query_schema}'
              and tablename = '{query_identifier}'
            union all
            select 'view' as type
            from pg_views
            where schemaname = '{query_schema}'
              and viewname = '{query_identifier}'
            union all
            select 'materialized_view' as type
            from pg_matviews
            where schemaname = '{query_schema}'
              and matviewname = '{query_identifier}'
            "#,
    );

    let batch = adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token)?;
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let string_array = batch.column_values::<StringArray>("type")?;

    if string_array.len() != 1 {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "Did not find 'type' for a relation",
        ));
    }

    let relation_type = match string_array.value(0) {
        "table" => Some(RelationType::Table),
        "view" => Some(RelationType::View),
        "materialized_view" => Some(RelationType::MaterializedView),
        _ => return invalid_value!("Unsupported relation type {}", string_array.value(0)),
    };

    let relation = Relation::new(
        AdapterType::Postgres,
        database.to_string(),
        schema.to_string(),
        identifier.to_string(),
    )
    .with_relation_type(relation_type)
    .with_quoting(adapter.quoting())
    .validate()?;
    Ok(Some(Box::new(relation)))
}

#[allow(clippy::too_many_arguments)]
fn exasol_get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    // Exasol folds unquoted identifiers to uppercase but stores quoted ones
    // verbatim, so resolve to the stored case per the quoting policy and match
    // exactly (as postgres_get_relation does). Single quotes doubled for safety.
    let quoting = adapter.quoting();
    let fold = |s: &str, quoted: bool| {
        let folded = if quoted {
            s.to_string()
        } else {
            s.to_uppercase()
        };
        folded.replace('\'', "''")
    };
    let q_schema = fold(schema, quoting.schema);
    let q_ident = fold(identifier, quoting.identifier);

    let sql = format!(
        "select 'table' as \"type\" from sys.exa_all_tables \
         where table_schema = '{q_schema}' and table_name = '{q_ident}' \
         union all \
         select 'view' from sys.exa_all_views \
         where view_schema = '{q_schema}' and view_name = '{q_ident}'"
    );
    let batch = adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token)?;
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let arr = batch.column_values::<StringArray>("type")?;
    let relation_type = match arr.value(0) {
        "table" => Some(RelationType::Table),
        "view" => Some(RelationType::View),
        other => {
            return Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!("Unexpected relation type: {other}"),
            ));
        }
    };
    let relation = do_create_relation(
        AdapterType::Exasol,
        database.to_string(),
        schema.to_string(),
        Some(identifier.to_string()),
        relation_type,
        adapter.quoting(),
    )?;
    Ok(Some(relation))
}

fn salesforce_get_relation(
    _adapter: &AdapterImpl,
    _state: &State,
    _query_ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    database: &str,
    _schema: &str,
    identifier: &str,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    // TODO: resolves relation_table based on the metadata to be returned in schema
    match conn.get_table_schema(Some(database), None, identifier) {
        Ok(_) => Ok(Some(Box::new(
            Relation::new(
                AdapterType::Salesforce,
                database.to_string(),
                None::<String>,
                identifier.to_string(),
            )
            .with_relation_type(RelationType::Table)
            .with_quoting(Policy::enabled())
            .validate()?,
        ))),
        Err(_) => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
fn duckdb_get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    // DuckDB is case-preserving for quoted identifiers
    // Unquoted identifiers are lowercase by default
    let query_schema = if adapter.quoting().schema {
        schema.to_string()
    } else {
        schema.to_lowercase()
    };

    let query_identifier = if adapter.quoting().identifier {
        identifier.to_string()
    } else {
        identifier.to_lowercase()
    };

    if !schema.is_empty()
        && !identifier.is_empty()
        && crate::metadata::duckdb::is_duckdb_v2_external_iceberg_catalog_database(database)
    {
        // DuckDB's information_schema can omit or misreport Iceberg REST
        // attached-catalog tables. A targeted DESCRIBE is the narrow fallback:
        // it reuses the normal relation construction after proving the table
        // exists, without enabling broad schema listing for these catalogs.
        let quote = |id: &str| dbt_adapter_sql::ident::quote_identifier(id, AdapterType::DuckDB);
        let relation_name = format!(
            "{}.{}.{}",
            quote(database),
            quote(&query_schema),
            quote(&query_identifier),
        );
        let sql = format!("DESCRIBE {relation_name}");
        let result = adapter
            .engine()
            .execute(Some(state), conn, ctx, &sql, token);

        match result {
            Ok(_) => {
                let relation = do_create_relation(
                    adapter.adapter_type(),
                    database.to_string(),
                    schema.to_string(),
                    Some(identifier.to_string()),
                    Some(RelationType::Table),
                    adapter.quoting(),
                )?;
                return Ok(Some(relation));
            }
            Err(err) if crate::metadata::duckdb::is_missing_relation_error(&err) => {
                return Ok(None);
            }
            Err(err) => return Err(err),
        }
    }

    // Query INFORMATION_SCHEMA.TABLES for relation metadata
    // DuckDB's table_type values: BASE TABLE, VIEW, LOCAL TEMPORARY
    let sql = format!(
        r#"
            SELECT table_type as type
            FROM information_schema.tables
            WHERE table_schema = '{}'
              AND table_name = '{}'
        "#,
        dbt_adapter_sql::ident::escape_string_literal(&query_schema, AdapterType::DuckDB),
        dbt_adapter_sql::ident::escape_string_literal(&query_identifier, AdapterType::DuckDB),
    );

    let batch = adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token)?;
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let string_array = batch.column_values::<StringArray>("type")?;

    if string_array.len() != 1 {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "Did not find 'type' for a relation",
        ));
    }

    // Map DuckDB table_type to dbt RelationType
    let relation_type = match string_array.value(0) {
        "BASE TABLE" => Some(RelationType::Table),
        "VIEW" => Some(RelationType::View),
        "LOCAL TEMPORARY" => Some(RelationType::Table), // Treat temp tables as tables
        _ => return invalid_value!("Unsupported relation type {}", string_array.value(0)),
    };

    // Use the logical adapter type to create the appropriate relation
    // This avoids backend-specific limitations (like Postgres 63-char identifier limit)
    // when running in sidecar mode
    let relation = do_create_relation(
        adapter.adapter_type(),
        database.to_string(),
        schema.to_string(),
        Some(identifier.to_string()),
        relation_type,
        adapter.quoting(),
    )?;
    Ok(Some(relation))
}

#[allow(clippy::too_many_arguments)]
fn fabric_get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    // Can't use `conn.get_table_schema()` here because the driver doesn't use the proper identifier casing in its internal query.
    // Same goes for `conn.get_objects()`
    //
    // What we should ideally do is use
    // ```sql
    // EXEC sys.sp_tables @table_qualifier = '<catalog>', @table_owner = '<schema>', @table_name = '<identifier>'
    // ```
    //
    // Which would give back:
    //
    // | TABLE_QUALIFIER | TABLE_OWNER | TABLE_NAME   | TABLE_TYPE       | REMARKS |
    // | --------------- | ----------- | ------------ | ---------------- | ------- |
    // | <catalog>       | <schema>    | <identifier> | 'VIEW' / 'TABLE' | NULL    |
    //
    // > See: https://learn.microsoft.com/en-us/sql/relational-databases/system-stored-procedures/sp-tables-transact-sql?view=fabric#remarks

    let lit_fmt = SqlLiteralFormatter::new(adapter.adapter_type());

    let sql = format!(
        "EXEC sys.sp_tables @table_qualifier = {}, @table_owner = {}, @table_name = {}",
        lit_fmt.format_str(database),
        lit_fmt.format_str(schema),
        lit_fmt.format_str(identifier),
    );

    let batch = adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token)?;

    if batch.num_rows() == 0 {
        // If there are no rows, then we did not find the object
        return Ok(None);
    }

    let string_array = batch.column_values::<StringArray>("TABLE_TYPE")?;

    if string_array.len() != 1 {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "Did not find 'TABLE_TYPE' for a relation",
        ));
    }

    // https://learn.microsoft.com/en-us/sql/relational-databases/system-stored-procedures/sp-tables-transact-sql?view=sql-server-ver17#----table_type
    let relation_type = match string_array.value(0) {
        // "SYSTEMTABLE" => ??? // do we treat this as a table too?
        "TABLE" => Some(RelationType::Table),
        "VIEW" => Some(RelationType::View),
        _ => None,
    };

    Ok(Some(Box::new(Relation::new_fabric(
        Some(database.to_string()),
        Some(schema.to_string()),
        Some(identifier.to_string()),
        relation_type,
        adapter.quoting(),
    ))))
}

#[allow(clippy::too_many_arguments)]
fn clickhouse_get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    use crate::metadata::clickhouse::{
        build_get_relation_sql, relation_type_from_engine, server_capabilities_over_connection,
        with_catalog_state,
    };
    use crate::record_batch::RecordBatchExt;

    // ClickHouse only has databases, not schemas — dbt `schema` maps to CH `database`.
    // dbt `database` is unused here.
    let sql = build_get_relation_sql(schema, identifier);

    // Probe over the connection we already hold: the State-based entry would
    // borrow a second thread-local connection and trip the nested-guard check.
    let caps =
        server_capabilities_over_connection(adapter.engine().as_ref(), ctx, conn, token.clone());

    let batch = adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token)?;
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let engines = batch.column_values::<StringArray>("engine")?;
    if engines.len() != 1 {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "Expected exactly one row for ClickHouse get_relation",
        ));
    }
    let relation = Relation::new(
        adapter.adapter_type(),
        Some(database.to_string()),
        Some(schema.to_string()),
        Some(identifier.to_string()),
    )
    .with_relation_type(relation_type_from_engine(engines.value(0)))
    .with_quoting(adapter.quoting());
    let relation = with_catalog_state(relation, caps, &batch, 0)?;
    Ok(Some(Box::new(relation) as Box<dyn BaseRelation>))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::StringArray;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;

    fn describe_batch(rows: &[(&str, &str)]) -> RecordBatch {
        let schema = Schema::new(vec![
            Field::new("col_name", DataType::Utf8, false),
            Field::new("data_type", DataType::Utf8, false),
        ]);
        let col_names: Vec<&str> = rows.iter().map(|(k, _)| *k).collect();
        let data_types: Vec<&str> = rows.iter().map(|(_, v)| *v).collect();
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(col_names)),
                Arc::new(StringArray::from(data_types)),
            ],
        )
        .unwrap()
    }

    #[test]
    fn spark_describe_classifies_view() {
        let batch = describe_batch(&[
            ("id", "int"),
            ("", ""),
            ("# Detailed Table Information", ""),
            ("Catalog", "spark_catalog"),
            ("Type", "VIEW"),
        ]);
        let (relation_type, is_delta, is_shallow_clone, _) =
            parse_describe_table_extended(AdapterType::Spark, &batch).unwrap();
        assert_eq!(relation_type, RelationType::View);
        assert!(!is_delta);
        assert!(!is_shallow_clone);
    }

    #[test]
    fn spark_describe_classifies_managed_table() {
        let batch = describe_batch(&[
            ("id", "int"),
            ("", ""),
            ("Type", "MANAGED"),
            ("Provider", "delta"),
        ]);
        let (relation_type, is_delta, is_shallow_clone, _) =
            parse_describe_table_extended(AdapterType::Spark, &batch).unwrap();
        assert_eq!(relation_type, RelationType::Table);
        assert!(is_delta);
        assert!(!is_shallow_clone);
    }

    #[test]
    fn spark_describe_defaults_to_table_when_type_missing() {
        let batch = describe_batch(&[("id", "int"), ("", ""), ("Catalog", "spark_catalog")]);
        let (relation_type, _, _, _) =
            parse_describe_table_extended(AdapterType::Spark, &batch).unwrap();
        assert_eq!(relation_type, RelationType::Table);
    }

    #[test]
    fn databricks_describe_classifies_shallow_clone() {
        let batch = describe_batch(&[
            ("id", "int"),
            ("", ""),
            ("Type", "MANAGED_SHALLOW_CLONE"),
            ("Provider", "delta"),
        ]);
        let (relation_type, is_delta, is_shallow_clone, _) =
            parse_describe_table_extended(AdapterType::Databricks, &batch).unwrap();
        assert_eq!(relation_type, RelationType::Table);
        assert!(is_delta);
        assert!(is_shallow_clone);
    }

    fn mock_bigquery_adapter() -> AdapterImpl {
        AdapterImpl::new_mock(
            AdapterType::Bigquery,
            BTreeMap::new(),
            dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING,
            Arc::new(crate::sql_types::DefaultTypeOps::new(AdapterType::Bigquery)),
            Arc::new(crate::stmt_splitter::DefaultStmtSplitter),
        )
    }

    fn routines_batch(routine_type: &str) -> RecordBatch {
        let schema = Schema::new(vec![
            Field::new("table_catalog", DataType::Utf8, false),
            Field::new("table_schema", DataType::Utf8, false),
            Field::new("table_name", DataType::Utf8, false),
            Field::new("table_type", DataType::Utf8, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(vec!["my-proj"])),
                Arc::new(StringArray::from(vec!["my_schema"])),
                Arc::new(StringArray::from(vec!["add_one"])),
                Arc::new(StringArray::from(vec![routine_type])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn relation_from_routines_batch_resolves_function_row() {
        let adapter = mock_bigquery_adapter();
        let batch = routines_batch("FUNCTION");

        let relation =
            relation_from_routines_batch(&adapter, "my-proj", "my_schema", "add_one", &batch)
                .unwrap()
                .expect("a non-empty ROUTINES batch should resolve to Some(relation)");

        assert_eq!(relation.relation_type(), Some(RelationType::Function));
        assert_eq!(relation.database_as_resolved_str().unwrap(), "my-proj");
        assert_eq!(relation.schema_as_resolved_str().unwrap(), "my_schema");
        assert_eq!(relation.identifier_as_resolved_str().unwrap(), "add_one");
    }

    #[test]
    fn relation_from_routines_batch_returns_none_when_batch_is_empty() {
        let adapter = mock_bigquery_adapter();
        let schema = Schema::new(vec![Field::new("table_type", DataType::Utf8, false)]);
        let empty_batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![Arc::new(StringArray::from(Vec::<&str>::new()))],
        )
        .unwrap();

        // No matching row means the object truly doesn't exist (it's not a
        // table/view either, since this is only reached as a fallback after
        // that lookup already failed).
        let result =
            relation_from_routines_batch(&adapter, "my-proj", "my_schema", "add_one", &empty_batch)
                .unwrap();
        assert!(result.is_none());
    }
}
