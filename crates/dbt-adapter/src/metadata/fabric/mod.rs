use crate::adapter::adapter_impl::*;
use crate::connection::AdapterConnectionFactory;
use crate::metadata::{
    CatalogAndSchema, MetadataAdapter, MetadataFreshness, RelationSchemaPair, RelationVec,
    create_schemas_if_not_exists, run_schema_cache_map_reduce,
};
use crate::record_batch::RecordBatchExt;
use crate::relation::Relation;
use crate::sql_types::{TypeOps, make_arrow_field};
use crate::{AdapterEngine, AdapterType};
use arrow_array::{Array, Int32Array, RecordBatch, StringArray};
use arrow_schema::Schema;
use dbt_adapter_core::ExecutionPhase;
use dbt_adapter_engine::MapReduce;
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::cancellation::CancellationToken;
use dbt_common::{AdapterError, Cancellable};
use dbt_common::{AdapterResult, AsyncAdapterResult};
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::legacy_catalog::{CatalogNodeStats, TableMetadata};
use dbt_schemas::schemas::{
    legacy_catalog::{CatalogTable, ColumnMetadata},
    relations::base::{BaseRelation, RelationPattern},
};
use minijinja::State;
use std::collections::HashMap;
use std::{collections::BTreeMap, sync::Arc};

pub fn list_relations(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let sql = format!(
        "EXEC sp_tables @table_qualifier={}, @table_owner={}",
        db_schema.resolved_catalog, db_schema.resolved_schema,
    );

    let batch = engine.execute(None, conn, ctx, &sql, token)?;

    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let mut relations = Vec::new();

    let table_name = batch.column_values::<StringArray>("TABLE_NAME")?;
    let database_name = batch.column_values::<StringArray>("TABLE_QUALIFIER")?;
    let schema_name = batch.column_values::<StringArray>("TABLE_OWNER")?;
    let table_type = batch.column_values::<StringArray>("TABLE_TYPE")?;

    for i in 0..batch.num_rows() {
        let table_type_value = table_type.value(i);

        let relation_type = if table_type_value.eq_ignore_ascii_case("TABLE") {
            Some(RelationType::Table)
        } else if table_type_value.eq_ignore_ascii_case("VIEW") {
            Some(RelationType::View)
        } else {
            None
        };
        let relation = Arc::new(Relation::new_fabric(
            Some(database_name.value(i).to_string()),
            Some(schema_name.value(i).to_string()),
            Some(table_name.value(i).to_string()),
            relation_type,
            engine.quoting(),
        )) as Arc<dyn BaseRelation>;

        relations.push(relation);
    }

    Ok(relations)
}

pub struct FabricMetadataAdapter {
    #[allow(dead_code, reason = "TODO implement FabricMetadataAdapter")]
    pub adapter: AdapterImpl,
}

impl FabricMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }
}

impl MetadataAdapter for FabricMetadataAdapter {
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
        let table_owners = stats_sql_result.column_values::<StringArray>("table_owner")?;

        let mut result = BTreeMap::<String, CatalogTable>::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);
            let data_type = data_types.value(i);
            let owner = table_owners.value(i);

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            if !result.contains_key(&fully_qualified_name) {
                let node_metadata = TableMetadata {
                    materialization_type: data_type.to_string(),
                    schema: schema.to_string(),
                    name: table.to_string(),
                    database: Some(catalog.to_string()),
                    comment: None,
                    owner: Some(owner.to_string()),
                };

                let no_stats = CatalogNodeStats {
                    id: "has_stats".to_string(),
                    label: "Has Stats?".to_string(),
                    value: serde_json::Value::Bool(false),
                    description: Some(
                        "Indicates whether there are statistics for this table".to_string(),
                    ),
                    include: false,
                };

                let node = CatalogTable {
                    metadata: node_metadata,
                    columns: Default::default(),
                    stats: BTreeMap::from([("has_stats".to_string(), no_stats)]),
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

        let mut columns_by_relation = BTreeMap::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            let column_name_i = column_names.value(i);
            let column_index_i = column_indices.value(i);
            let column_type_i = column_types.value(i);

            let column = ColumnMetadata {
                name: column_name_i.to_string(),
                index: column_index_i.into(),
                data_type: column_type_i.to_string(),
                comment: None,
            };

            columns_by_relation
                .entry(fully_qualified_name.clone())
                .or_insert(BTreeMap::new())
                .insert(column_name_i.to_string(), column);
        }
        Ok(columns_by_relation)
    }

    fn create_schemas_if_not_exists(
        &self,
        state: &State<'_, '_>,
        catalog_schemas: Vec<(String, String, String)>,
    ) -> AdapterResult<Vec<(String, String, String, AdapterResult<()>)>> {
        create_schemas_if_not_exists(&self.adapter, self, state, catalog_schemas)
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
        let map_f = move |conn: &'_ mut dyn Connection,
                          relation: &Arc<dyn BaseRelation>|
              -> AdapterResult<Arc<Schema>> {
            let sql = format!(
                "EXEC sp_columns @table_qualifier={}, @table_owner={}, @table_name={}",
                relation.database_as_str()?,
                relation.schema_as_str()?,
                relation.identifier_as_str()?,
            );

            let ctx = QueryCtx::new_metadata().with_desc("Get table schema");

            let ctx = unique_id
                .iter()
                .fold(ctx, |ctx, id| ctx.with_node_id(id.clone()));

            let ctx = phase
                .iter()
                .fold(ctx, |ctx, phase| ctx.with_phase(phase.as_str()));

            let (_, table) = adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            let batch = table.original_record_batch();
            let schema = build_schema_from_sp_columns(batch, adapter.engine().type_ops().as_ref())?;

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
            None,
            token,
        )
    }

    fn list_relations_schemas_by_patterns_inner(
        &self,
        patterns: &[RelationPattern],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        let _ = patterns;

        todo!()
    }

    fn freshness_inner(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        let _ = relations;

        todo!()
    }

    fn list_relations_in_parallel_inner(
        &self,
        db_schemas: &[CatalogAndSchema],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        type Acc = BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>;
        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          db_schema: &CatalogAndSchema|
              -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
            let query_ctx = QueryCtx::default().with_desc("list_relations_in_parallel");
            adapter.list_relations(&query_ctx, conn, db_schema, token_clone.clone())
        };

        let reduce_f = move |acc: &mut Acc,
                             db_schema: CatalogAndSchema,
                             relations: AdapterResult<Vec<Arc<dyn BaseRelation>>>|
              -> Result<(), Cancellable<AdapterError>> {
            match relations {
                Ok(relations) => {
                    acc.insert(db_schema, Ok(relations));
                    Ok(())
                }
                Err(e) => Err(Cancellable::Error(e)),
            }
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(db_schemas.to_vec()), token)
    }
}

fn build_schema_from_sp_columns(
    sp_columns_result: Arc<RecordBatch>,
    type_ops: &dyn TypeOps,
) -> AdapterResult<Arc<Schema>> {
    let column_names = sp_columns_result.column_values::<StringArray>("COLUMN_NAME")?;
    let data_types = sp_columns_result.column_values::<StringArray>("TYPE_NAME")?;
    let comments = sp_columns_result.column_values::<StringArray>("REMARKS")?;
    let nullability = sp_columns_result.column_values::<StringArray>("IS_NULLABLE")?;

    let mut fields = vec![];
    for i in 0..sp_columns_result.num_rows() {
        let name = column_names.value(i);
        let nullable = nullability.value(i).to_uppercase() == "YES";
        let text_data_type = data_types.value(i);
        let comment = match comments.value(i) {
            "" => None,
            c => Some(c.to_string()),
        };

        let field = make_arrow_field(
            type_ops,
            name.to_string(),
            text_data_type,
            Some(nullable),
            comment,
        )?;
        fields.push(field);
    }

    let schema = Schema::new(fields);
    Ok(Arc::new(schema))
}
