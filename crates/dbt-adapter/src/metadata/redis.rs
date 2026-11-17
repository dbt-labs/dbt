//! Redis declarations supplement the types omitted by Flight SQL metadata.
use crate::AdapterResult;
use crate::errors::{AdapterError, AdapterErrorKind, adbc_error_to_adapter_error};
use arrow_array::{Array, StringArray};
use arrow_schema::Schema;
use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::ident::escape_string_literal;
use dbt_adbc::Connection;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) fn supplement_schema(
    connection: &mut dyn Connection,
    schema: Schema,
    namespace: &str,
    table: &str,
) -> AdapterResult<Schema> {
    let namespace = escape_string_literal(namespace, AdapterType::RedisAdbc);
    let table = escape_string_literal(table, AdapterType::RedisAdbc);
    let sql = format!(
        "select column_name, driver_data_type from information_schema.columns where table_schema = '{namespace}' and table_name = '{table}'"
    );
    let mut statement = connection
        .new_statement()
        .map_err(adbc_error_to_adapter_error)?;
    statement
        .set_sql_query(&sql)
        .map_err(adbc_error_to_adapter_error)?;
    let reader = statement.execute().map_err(adbc_error_to_adapter_error)?;
    let mut declarations = HashMap::new();
    for batch in reader {
        let batch = batch
            .map_err(|error| AdapterError::new(AdapterErrorKind::Driver, error.to_string()))?;
        let strings = |index| {
            batch
                .column(index)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorKind::UnexpectedResult,
                        "Invalid Redis column metadata",
                    )
                })
        };
        let names = strings(0)?;
        let types = strings(1)?;
        for row in 0..batch.num_rows() {
            if !names.is_null(row) && !types.is_null(row) {
                declarations.insert(names.value(row).to_owned(), types.value(row).to_owned());
            }
        }
    }
    let fields = schema
        .fields()
        .iter()
        .map(|field| {
            let declaration = declarations.get(field.name()).ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::UnexpectedResult,
                    "Missing exact Redis column declaration",
                )
            })?;
            let mut metadata = field.metadata().clone();
            metadata.insert("ARROW:FLIGHT:SQL:TYPE_NAME".into(), declaration.clone());
            Ok(Arc::new(field.as_ref().clone().with_metadata(metadata)))
        })
        .collect::<AdapterResult<Vec<_>>>()?;
    Ok(Schema::new_with_metadata(fields, schema.metadata().clone()))
}
