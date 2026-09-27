//! Relation, column and schema reads from Glue.
//!
//! dbt-athena answers `list_relations_without_caching`, `get_columns_in_relation`,
//! `list_schemas` and `check_schema_exists` with Glue API calls for every Glue catalog
//! (`awsdatacatalog`, S3 Tables catalogs, Glue catalogs registered in Athena), and falls
//! back to SQL only for data catalogs outside Glue. The API calls are neither queued nor
//! billed as Athena queries, and they return the Hive types (`string`, `struct<...>`)
//! the dbt-athena macros are written for. Each read here answers `None` for a catalog
//! outside Glue, and the caller takes its SQL path.

use super::driver_ops::{AthenaOps, DataCatalog};
use crate::adapter::adapter_impl::AdapterImpl;
use crate::column::Column;
use crate::errors::AdapterResult;
use crate::metadata::CatalogAndSchema;
use crate::query_ctx::{node_id_from_state, query_ctx_from_state};
use crate::relation::{Relation, do_create_relation};
use dbt_adapter_core::AdapterType;
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::cancellation::{CancellationToken, never_cancels};
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::relations::base::BaseRelation;
use minijinja::State;
use serde_json::{Map, Value as Json};
use std::sync::Arc;

/// `AthenaAdapter._is_current_column`: Glue keeps the columns an Iceberg schema change
/// dropped, flagged `iceberg.field.current = false`.
fn is_current_column(column: &Json) -> bool {
    column
        .get("Parameters")
        .and_then(|p| p.get("iceberg.field.current"))
        .and_then(Json::as_str)
        != Some("false")
}

/// `list_relations_without_caching`: a Glue `TableType` of `VIRTUAL_VIEW` is a view,
/// anything else a table.
fn relation_type(table: &Json) -> Option<RelationType> {
    match table.get("TableType").and_then(Json::as_str)? {
        "VIRTUAL_VIEW" => Some(RelationType::View),
        _ => Some(RelationType::Table),
    }
}

/// `get_table_type(table) == TableType.ICEBERG`.
fn is_iceberg(table: &Json) -> bool {
    table
        .get("Parameters")
        .and_then(|p| p.get("table_type"))
        .and_then(Json::as_str)
        .is_some_and(|t| t.eq_ignore_ascii_case("iceberg"))
}

/// Name and type of a table's current columns, then its partition keys.
fn columns(table: &Json) -> Vec<(String, String)> {
    let listed = |key: &Json| key.as_array().cloned().unwrap_or_default();
    listed(&table["StorageDescriptor"]["Columns"])
        .into_iter()
        .filter(is_current_column)
        .chain(listed(&table["PartitionKeys"]))
        .filter_map(|column| {
            Some((
                column.get("Name")?.as_str()?.to_string(),
                column.get("Type")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

impl AthenaOps<'_, '_, '_> {
    /// The `CatalogId` of a database's Glue calls, `None` outside Glue.
    fn glue_scope(&self, database: &str) -> AdapterResult<Option<Map<String, Json>>> {
        let DataCatalog::Glue(catalog_id) = self.data_catalog(Some(database))? else {
            return Ok(None);
        };
        let mut scope = Map::new();
        if let Some(catalog_id) = catalog_id {
            scope.insert("CatalogId".into(), Json::String(catalog_id));
        }
        Ok(Some(scope))
    }

    /// The Glue tables of a schema; empty when the schema does not exist.
    fn glue_tables(&self, database: &str, schema: &str) -> AdapterResult<Option<Vec<Json>>> {
        let Some(mut payload) = self.glue_scope(database)? else {
            return Ok(None);
        };
        payload.insert("DatabaseName".into(), Json::String(schema.to_string()));
        let response = self.call("glue.get_tables", Json::Object(payload))?;
        Ok(Some(
            response["TableList"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        ))
    }

    /// One Glue table, `Some(None)` when it does not exist.
    fn glue_table_raw(
        &self,
        database: &str,
        schema: &str,
        identifier: &str,
    ) -> AdapterResult<Option<Option<Json>>> {
        let Some(mut payload) = self.glue_scope(database)? else {
            return Ok(None);
        };
        payload.insert("DatabaseName".into(), Json::String(schema.to_string()));
        payload.insert("Name".into(), Json::String(identifier.to_string()));
        let response = self.call("glue.get_table", Json::Object(payload))?;
        Ok(Some(
            response.get("Table").filter(|t| !t.is_null()).cloned(),
        ))
    }

    /// `AthenaAdapter.list_schemas`, in the database's own catalog.
    fn glue_schemas(&self, database: &str) -> AdapterResult<Option<Vec<String>>> {
        let Some(payload) = self.glue_scope(database)? else {
            return Ok(None);
        };
        let response = self.call("glue.get_databases", Json::Object(payload))?;
        Ok(Some(
            response["DatabaseList"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|db| db.get("Name")?.as_str().map(str::to_string))
                .collect(),
        ))
    }

    /// `AthenaAdapter.check_schema_exists`.
    fn glue_schema_exists(&self, database: &str, schema: &str) -> AdapterResult<Option<bool>> {
        let Some(mut payload) = self.glue_scope(database)? else {
            return Ok(None);
        };
        payload.insert("Name".into(), Json::String(schema.to_string()));
        let response = self.call("glue.get_database", Json::Object(payload))?;
        Ok(Some(response.get("Database").is_some_and(|d| !d.is_null())))
    }
}

impl AdapterImpl {
    /// Run `read` on the node's thread-local connection.
    fn with_athena_ops<T>(
        &self,
        state: &State,
        desc: &str,
        read: impl FnOnce(&AthenaOps) -> AdapterResult<T>,
    ) -> AdapterResult<T> {
        let mut conn = self.borrow_tlocal_connection(Some(state), node_id_from_state(state))?;
        let ctx = query_ctx_from_state(state)?.with_desc(desc);
        let engine = self.engine();
        let ops = AthenaOps::on_connection(
            engine.as_ref(),
            Some(state),
            &ctx,
            &mut **conn,
            never_cancels(),
        );
        read(&ops)
    }

    /// `AthenaAdapter.list_relations_without_caching`: the tables and views of a schema.
    pub(crate) fn athena_list_relations(
        &self,
        state: Option<&State>,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        db_schema: &CatalogAndSchema,
        token: CancellationToken,
    ) -> AdapterResult<Option<Vec<Arc<dyn BaseRelation>>>> {
        let engine = self.engine();
        let ops = AthenaOps::on_connection(engine.as_ref(), state, ctx, conn, token);
        let catalog = &db_schema.resolved_catalog;
        let Some(tables) = ops.glue_tables(catalog, &db_schema.resolved_schema)? else {
            return Ok(None);
        };
        let mut relations = Vec::with_capacity(tables.len());
        for table in &tables {
            let Some(name) = table.get("Name").and_then(Json::as_str) else {
                continue;
            };
            let Some(relation_type) = relation_type(table) else {
                tracing::info!("Table '{name}' has no TableType attribute - Ignoring");
                continue;
            };
            let schema = table
                .get("DatabaseName")
                .and_then(Json::as_str)
                .unwrap_or(&db_schema.resolved_schema);
            let relation = Relation::new(
                AdapterType::Athena,
                Some(catalog.clone()),
                Some(schema.to_string()),
                Some(name.to_string()),
            )
            .with_relation_type(relation_type)
            .with_quoting(self.quoting());
            relations.push(Arc::new(relation) as Arc<dyn BaseRelation>);
        }
        Ok(Some(relations))
    }

    /// `get_relation` from the Glue table, `Some(None)` when it does not exist.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn athena_get_relation(
        &self,
        state: &State,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        database: &str,
        schema: &str,
        identifier: &str,
        token: CancellationToken,
    ) -> AdapterResult<Option<Option<Box<dyn BaseRelation>>>> {
        let engine = self.engine();
        let ops = AthenaOps::on_connection(engine.as_ref(), Some(state), ctx, conn, token);
        let Some(table) = ops.glue_table_raw(database, schema, identifier)? else {
            return Ok(None);
        };
        let Some(table) = table else {
            return Ok(Some(None));
        };
        let relation = do_create_relation(
            AdapterType::Athena,
            database.to_string(),
            schema.to_string(),
            Some(identifier.to_string()),
            relation_type(&table),
            self.quoting(),
        )?;
        Ok(Some(Some(relation)))
    }

    /// `AthenaAdapter.get_columns_in_relation`: no columns when the table does not exist.
    pub(crate) fn athena_get_columns_in_relation(
        &self,
        state: &State,
        relation: &dyn BaseRelation,
    ) -> AdapterResult<Option<Vec<Column>>> {
        let (Some(database), Some(schema), Some(identifier)) = (
            relation.database(),
            relation.schema(),
            relation.identifier(),
        ) else {
            return Ok(None);
        };
        let table = self.with_athena_ops(state, "get_columns_in_relation", |ops| {
            ops.glue_table_raw(database, schema, identifier)
        })?;
        Ok(table.map(|table| {
            let Some(table) = table else {
                return Vec::new();
            };
            let iceberg = is_iceberg(&table);
            columns(&table)
                .into_iter()
                .map(|(name, dtype)| {
                    Column::new(AdapterType::Athena, name, dtype, None, None, None)
                        .with_iceberg(iceberg)
                })
                .collect()
        }))
    }

    /// `AthenaAdapter.list_schemas`.
    pub(crate) fn athena_list_schemas(
        &self,
        state: &State,
        database: &str,
    ) -> AdapterResult<Option<Vec<String>>> {
        self.with_athena_ops(state, "list_schemas", |ops| ops.glue_schemas(database))
    }

    /// `AthenaAdapter.check_schema_exists`.
    pub(crate) fn athena_check_schema_exists(
        &self,
        state: &State,
        database: &str,
        schema: &str,
    ) -> AdapterResult<Option<bool>> {
        self.with_athena_ops(state, "check_schema_exists", |ops| {
            ops.glue_schema_exists(database, schema)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn columns_are_the_current_ones_then_the_partition_keys() {
        let table = json!({
            "StorageDescriptor": {"Columns": [
                {"Name": "id", "Type": "int"},
                {"Name": "dropped", "Type": "string", "Parameters": {"iceberg.field.current": "false"}},
                {"Name": "payload", "Type": "struct<a:string,b:int>",
                 "Parameters": {"iceberg.field.current": "true"}}
            ]},
            "PartitionKeys": [{"Name": "dt", "Type": "string"}]
        });
        assert_eq!(
            columns(&table),
            [
                ("id".to_string(), "int".to_string()),
                ("payload".to_string(), "struct<a:string,b:int>".to_string()),
                ("dt".to_string(), "string".to_string()),
            ]
        );
        assert!(columns(&json!({})).is_empty());
    }

    #[test]
    fn views_are_virtual_views_and_a_missing_type_is_skipped() {
        assert_eq!(
            relation_type(&json!({"TableType": "VIRTUAL_VIEW"})),
            Some(RelationType::View)
        );
        assert_eq!(
            relation_type(&json!({"TableType": "EXTERNAL_TABLE"})),
            Some(RelationType::Table)
        );
        // S3 Tables tables report TableType "customer".
        assert_eq!(
            relation_type(&json!({"TableType": "customer"})),
            Some(RelationType::Table)
        );
        assert_eq!(relation_type(&json!({"Name": "t"})), None);
    }
}
