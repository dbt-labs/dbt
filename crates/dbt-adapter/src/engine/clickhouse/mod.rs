//! ClickHouse-specific connection bootstrap for [`super::AdbcEngine`].

use std::borrow::Cow;

use dbt_adbc::Connection;
use dbt_auth::AdapterConfig;
use dbt_common::AdapterResult;

use crate::errors::{AdapterError, AdapterErrorKind, adbc_error_to_adapter_error};
use crate::metadata::clickhouse;

/// The profile `schema` (a ClickHouse database; the driver maps it to the
/// ADBC schema).
pub(crate) fn target_schema(config: &AdapterConfig) -> Option<Cow<'_, str>> {
    config.get_string("schema").filter(|s| !s.is_empty())
}

/// Mirrors dbclient.py `_ensure_database`: the server rejects every request
/// whose default database does not exist, so a fresh target could never
/// bootstrap itself. `conn` must have no current schema set.
pub(crate) fn ensure_database(
    conn: &mut dyn Connection,
    config: &AdapterConfig,
) -> AdapterResult<()> {
    let Some(db_name) = target_schema(config) else {
        return Ok(());
    };

    let db_exists = |conn: &mut dyn Connection| -> AdapterResult<bool> {
        let mut stmt = conn.new_statement().map_err(adbc_error_to_adapter_error)?;
        stmt.set_sql_query(&clickhouse::exists_database_sql(&db_name))
            .map_err(adbc_error_to_adapter_error)?;
        let mut reader = stmt.execute().map_err(adbc_error_to_adapter_error)?;
        let batch = reader
            .next()
            .transpose()
            .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))?;
        // EXISTS DATABASE returns a single UInt8 row.
        let Some(batch) = batch else {
            return Ok(false);
        };
        if batch.num_rows() == 0 {
            return Ok(false);
        }
        let column = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow_array::UInt8Array>()
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Internal,
                    format!(
                        "unexpected EXISTS DATABASE result type: {}",
                        batch.column(0).data_type()
                    ),
                )
            })?;
        Ok(column.value(0) != 0)
    };

    if db_exists(conn)? {
        return Ok(());
    }
    let create_sql = clickhouse::create_database_sql(
        &db_name,
        config.get_str("database_engine"),
        config.get_str("cluster"),
    );
    let mut stmt = conn.new_statement().map_err(adbc_error_to_adapter_error)?;
    stmt.set_sql_query(&create_sql)
        .map_err(adbc_error_to_adapter_error)?;
    stmt.execute_update().map_err(|e| {
        AdapterError::new(
            AdapterErrorKind::Configuration,
            format!("Failed to create {db_name} database due to ClickHouse exception: {e}"),
        )
    })?;
    if !db_exists(conn)? {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!("Failed to create database {db_name} for unknown reason"),
        ));
    }
    Ok(())
}
