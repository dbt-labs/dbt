use crate::connection::ConnectionGuard;
use crate::errors::{AdapterError, AdapterErrorKind, adbc_error_to_adapter_error};
use crate::metadata::CatalogAndSchema;
use crate::record_batch::StructArrayExt;
use crate::relation::Relation;
use crate::{AdapterEngine, AdapterResult};
use arrow_array::{Array, ListArray, RecordBatch, StringArray, StructArray};
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::{ResolvedQuoting, normalize_quote};
use dbt_schemas::schemas::relations::base::BaseRelation;
use std::sync::Arc;

pub(crate) fn list_relations_via_adbc(
    engine: &dyn AdapterEngine,
    conn: &mut ConnectionGuard<'_>,
    db_schema: &CatalogAndSchema,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    // The driver expects unquoted values, so regardless of the adapter's config
    // we need to strip quotes.
    let (catalog, _) = normalize_quote(false, engine.adapter_type(), &db_schema.rendered_catalog);
    let (schema, _) = normalize_quote(false, engine.adapter_type(), &db_schema.rendered_schema);

    let reader = conn
        .get_objects(
            adbc_core::options::ObjectDepth::Tables,
            Some(&catalog),
            Some(&schema),
            None,
            None,
            None,
        )
        .map_err(adbc_error_to_adapter_error)?;

    let arrow_schema = reader.schema();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AdapterError::new(AdapterErrorKind::Driver, e.to_string()))?;
    let batch = arrow::compute::concat_batches(&arrow_schema, &batches)
        .map_err(|e| AdapterError::new(AdapterErrorKind::Driver, e.to_string()))?;

    relations_from_get_objects(
        &batch,
        &RelationContext {
            adapter_type: engine.adapter_type(),
            quoting: engine.quoting(),
            catalog: &catalog,
            schema: &schema,
        },
    )
}

struct RelationContext<'a> {
    adapter_type: AdapterType,
    quoting: ResolvedQuoting,
    catalog: &'a str,
    schema: &'a str,
}

impl RelationContext<'_> {
    fn relation_type(&self, value: &str) -> AdapterResult<RelationType> {
        match self.adapter_type {
            AdapterType::RedisAdbc => match value.to_ascii_uppercase().as_str() {
                "TABLE" => Ok(RelationType::Table),
                "VIEW" => Ok(RelationType::View),
                other => Err(AdapterError::new(
                    AdapterErrorKind::UnexpectedResult,
                    format!("Unsupported Redis relation type: {other}"),
                )),
            },
            _ => Ok(RelationType::from_adapter_type(self.adapter_type, value)),
        }
    }
}

fn relations_from_get_objects(
    batch: &RecordBatch,
    context: &RelationContext<'_>,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let schemas_struct = schemas_from_get_objects(batch)?;
    let db_schema_tables = schemas_struct.column_as::<ListArray>("db_schema_tables")?;
    let schema_names = schemas_struct.column_as::<StringArray>("db_schema_name")?;
    let mut result = Vec::new();
    for row in 0..schemas_struct.len() {
        if schema_names.is_null(row) {
            continue;
        }
        if schema_names.value(row) != context.schema {
            continue;
        }
        if db_schema_tables.is_null(row) {
            continue;
        }
        let tables = db_schema_tables.value(row);
        result.extend(relations_from_tables(tables.as_ref(), context)?);
    }
    Ok(result)
}

fn schemas_from_get_objects(batch: &RecordBatch) -> AdapterResult<&StructArray> {
    // The schema names are nested in the result of the get object call.
    // The batch has the following shape at the top level:
    // - catalog_name: utf8
    // - catalog_db_schemas: list[struct]
    //
    // Each row of the column `catalog_db_schemas` is a list of elements
    // with the shape:
    //   - db_schema_name: utf8
    //   - db_schema_tables: list
    let catalog_db_schemas = batch
        .column_by_name("catalog_db_schemas")
        .and_then(|c| c.as_any().downcast_ref::<ListArray>())
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                "Missing or invalid 'catalog_db_schemas' column",
            )
        })?;

    let schemas_struct = catalog_db_schemas
        .values()
        .as_any()
        .downcast_ref::<StructArray>()
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                "Missing or invalid 'catalog_db_schemas' values",
            )
        })?;

    Ok(schemas_struct)
}

fn relations_from_tables(
    tables: &dyn Array,
    context: &RelationContext<'_>,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let mut result = Vec::new();
    let tables = tables
        .as_any()
        .downcast_ref::<StructArray>()
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                "Invalid db_schema_tables values",
            )
        })?;
    let names = tables.column_as::<StringArray>("table_name")?;
    let types = tables.column_as::<StringArray>("table_type")?;
    for row in 0..tables.len() {
        let adapter_type = context.adapter_type;
        let relation_type = context.relation_type(types.value(row))?;
        result.push(Arc::new(
            Relation::new(
                adapter_type,
                context.catalog.to_owned(),
                context.schema.to_owned(),
                names.value(row).to_owned(),
            )
            .with_relation_type(relation_type)
            .with_quoting(context.quoting),
        ) as Arc<dyn BaseRelation>);
    }
    Ok(result)
}

pub(crate) fn get_relation_via_adbc(
    engine: &dyn AdapterEngine,
    conn: &mut ConnectionGuard<'_>,
    relation: Relation,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    let database = relation.database().unwrap_or_default();
    let schema = relation.schema().unwrap_or_default();
    let db_schema = CatalogAndSchema {
        rendered_catalog: database.to_owned(),
        rendered_schema: schema.to_owned(),
        resolved_catalog: database.to_owned(),
        resolved_schema: schema.to_owned(),
    };
    let rows = list_relations_via_adbc(engine, conn, &db_schema)?;
    Ok(rows
        .into_iter()
        .find(|row| row.identifier() == relation.identifier())
        .map(|row| {
            Box::new(relation.with_relation_type(row.relation_type())) as Box<dyn BaseRelation>
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::buffer::OffsetBuffer;
    use arrow_array::ArrayRef;
    use arrow_schema::{DataType, Field, Schema};
    use dbt_schemas::schemas::relations::base::Policy;

    #[test]
    fn metadata_keeps_exact_schema_and_adapter_identity() {
        // ADBC schema filters use SQL LIKE: analytics_one can also match
        // analyticsXone. Only relations in the requested schema may be cached.
        let tables = StructArray::from(vec![
            (
                Arc::new(Field::new("table_name", DataType::Utf8, false)),
                Arc::new(StringArray::from(vec!["Trips", "other_schema_table"])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("table_type", DataType::Utf8, false)),
                Arc::new(StringArray::from(vec!["TABLE", "VIEW"])) as ArrayRef,
            ),
        ]);
        let tables = ListArray::new(
            Arc::new(Field::new("item", tables.data_type().clone(), true)),
            OffsetBuffer::new(vec![0i32, 1, 2].into()),
            Arc::new(tables),
            None,
        );
        let schemas = StructArray::from(vec![
            (
                Arc::new(Field::new("db_schema_name", DataType::Utf8, false)),
                Arc::new(StringArray::from(vec!["analytics_one", "analyticsXone"])) as ArrayRef,
            ),
            (
                Arc::new(Field::new(
                    "db_schema_tables",
                    tables.data_type().clone(),
                    true,
                )),
                Arc::new(tables) as ArrayRef,
            ),
        ]);
        let catalogs = ListArray::new(
            Arc::new(Field::new("item", schemas.data_type().clone(), true)),
            OffsetBuffer::new(vec![0i32, 2].into()),
            Arc::new(schemas),
            None,
        );
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                "catalog_db_schemas",
                catalogs.data_type().clone(),
                true,
            )])),
            vec![Arc::new(catalogs)],
        )
        .unwrap();
        for adapter_type in [AdapterType::RedisAdbc, AdapterType::Bigquery] {
            let relations = relations_from_get_objects(
                &batch,
                &RelationContext {
                    adapter_type,
                    quoting: Policy::trues(),
                    catalog: "redis",
                    schema: "analytics_one",
                },
            )
            .unwrap();
            assert_eq!(relations.len(), 1);
            assert_eq!(relations[0].adapter_type(), adapter_type);
            assert_eq!(relations[0].identifier(), Some("Trips"));
            assert_eq!(relations[0].relation_type(), Some(RelationType::Table));
        }
    }
}
