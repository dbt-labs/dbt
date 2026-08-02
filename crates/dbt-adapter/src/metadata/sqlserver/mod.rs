use crate::adapter::adapter_impl::*;
use crate::connection::AdapterConnectionFactory;
use crate::formatter::SqlLiteralFormatter;
use crate::metadata::{
    CatalogAndSchema, MetadataAdapter, MetadataFreshness, RELATION_CACHE_OP_ID, RelationSchemaPair,
    RelationVec, create_schemas_if_not_exists, run_schema_cache_map_reduce,
    with_relation_list_item_span,
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
use dbt_common::{AdapterError, AdapterErrorKind, Cancellable};
use dbt_common::{AdapterResult, AsyncAdapterResult};
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::legacy_catalog::{CatalogNodeStats, TableMetadata};
use dbt_schemas::schemas::{
    legacy_catalog::{CatalogTable, ColumnMetadata},
    relations::base::{BaseRelation, RelationPattern},
};
use minijinja::State;
use std::collections::HashMap;
use std::collections::btree_map::Entry;
use std::{collections::BTreeMap, sync::Arc};

/// `sp_tables` and `sp_columns` are not usable here, for two reasons measured against
/// SQL Server 2022:
///
/// - They read the connection's current database only. Any other `@table_qualifier`
///   is error 15250, "The database name component of the object qualifier must be the
///   name of the current database".
/// - `@table_name` is a `LIKE` pattern unless `@fUsePattern = 0` is passed, so
///   `stg_orders` also matches `stgXorders`.
///
/// The catalog views take a three-part name, which resolves cross-database without
/// changing the connection's current database the way `USE` does.
///
/// TODO: v1 makes `with (nolock)` overridable by dispatching
/// `sqlserver__information_schema_hints`; this has no equivalent.
pub(crate) fn build_list_relations_sql(quoted_database: &str, schema: &str) -> String {
    let lit_fmt = SqlLiteralFormatter::new(AdapterType::SqlServer);
    format!(
        "select \
            s.name as table_schema, \
            o.name as table_name, \
            case o.type when 'V' then 'VIEW' else 'TABLE' end as table_type \
         from {quoted_database}.sys.objects o with (nolock) \
         join {quoted_database}.sys.schemas s with (nolock) on o.schema_id = s.schema_id \
         where o.type in ('U', 'V') and s.name = {} \
         order by o.name",
        lit_fmt.format_str(schema),
    )
}

/// Relation type of a single object, or no rows when it does not exist.
pub(crate) fn build_get_relation_sql(
    quoted_database: &str,
    schema: &str,
    identifier: &str,
) -> String {
    let lit_fmt = SqlLiteralFormatter::new(AdapterType::SqlServer);
    format!(
        "select case o.type when 'V' then 'VIEW' else 'TABLE' end as table_type \
         from {quoted_database}.sys.objects o with (nolock) \
         join {quoted_database}.sys.schemas s with (nolock) on o.schema_id = s.schema_id \
         where s.name = {} and o.name = {} and o.type in ('U', 'V')",
        lit_fmt.format_str(schema),
        lit_fmt.format_str(identifier),
    )
}

/// Columns of a single relation, in ordinal order.
///
/// `max_length`, `precision` and `scale` come back as `smallint` and `tinyint`; the
/// casts make them a single Arrow type to read.
pub(crate) fn build_columns_sql(quoted_database: &str, schema: &str, identifier: &str) -> String {
    let lit_fmt = SqlLiteralFormatter::new(AdapterType::SqlServer);
    format!(
        "select \
            c.name collate database_default as column_name, \
            t.name collate database_default as data_type, \
            cast(c.max_length as int) as max_length, \
            cast(c.precision as int) as numeric_precision, \
            cast(c.scale as int) as numeric_scale, \
            case when c.is_nullable = 1 then 'YES' else 'NO' end as is_nullable \
         from {quoted_database}.sys.columns c with (nolock) \
         join {quoted_database}.sys.objects o with (nolock) on c.object_id = o.object_id \
         join {quoted_database}.sys.schemas s with (nolock) on o.schema_id = s.schema_id \
         join {quoted_database}.sys.types t with (nolock) on c.user_type_id = t.user_type_id \
         where s.name = {} and o.name = {} \
         order by c.column_id",
        lit_fmt.format_str(schema),
        lit_fmt.format_str(identifier),
    )
}

/// Map the `table_type` column produced by [build_list_relations_sql] and
/// [build_get_relation_sql].
pub(crate) fn relation_type_from_table_type(table_type: &str) -> Option<RelationType> {
    match table_type {
        "TABLE" => Some(RelationType::Table),
        "VIEW" => Some(RelationType::View),
        _ => None,
    }
}

/// Rebuild the declared type text from the catalog columns.
///
/// `sys.types.name` is the bare type name, so `decimal(18,4)` arrives as `decimal`
/// with the precision and scale in separate columns.
pub(crate) fn compose_type_text(
    data_type: &str,
    max_length: i32,
    precision: i32,
    scale: i32,
) -> String {
    match data_type {
        // `max_length` is in bytes, and -1 is the `(max)` spelling.
        "char" | "varchar" | "binary" | "varbinary" => {
            if max_length < 0 {
                format!("{data_type}(max)")
            } else {
                format!("{data_type}({max_length})")
            }
        }
        "nchar" | "nvarchar" => {
            if max_length < 0 {
                format!("{data_type}(max)")
            } else {
                format!("{data_type}({})", max_length / 2)
            }
        }
        "decimal" | "numeric" => format!("{data_type}({precision},{scale})"),
        // `datetime` also reports a scale, but does not accept one.
        "datetime2" | "datetimeoffset" | "time" => format!("{data_type}({scale})"),
        _ => data_type.to_string(),
    }
}

fn quoted_database(relation: &dyn BaseRelation) -> AdapterResult<String> {
    let database = relation.database_as_str()?;
    if database.is_empty() {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "SQL Server relations require a database to build a three-part name",
        ));
    }
    Ok(relation.quoted(&database))
}

pub fn list_relations(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    if db_schema.rendered_catalog.is_empty() {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "SQL Server relations require a database to build a three-part name",
        ));
    }

    let sql = build_list_relations_sql(&db_schema.rendered_catalog, &db_schema.resolved_schema);
    let batch = engine.execute(None, conn, ctx, &sql, token)?;

    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let schema_name = batch.column_values::<StringArray>("table_schema")?;
    let table_name = batch.column_values::<StringArray>("table_name")?;
    let table_type = batch.column_values::<StringArray>("table_type")?;

    let mut relations = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        let relation = Arc::new(Relation::new_sqlserver(
            Some(db_schema.resolved_catalog.clone()),
            Some(schema_name.value(i).to_string()),
            Some(table_name.value(i).to_string()),
            relation_type_from_table_type(table_type.value(i)),
            engine.quoting(),
        )) as Arc<dyn BaseRelation>;

        relations.push(relation);
    }

    Ok(relations)
}

pub struct SqlServerMetadataAdapter {
    pub adapter: AdapterImpl,
}

impl SqlServerMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }
}

impl MetadataAdapter for SqlServerMetadataAdapter {
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

        let mut result = BTreeMap::<String, CatalogTable>::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);
            let data_type = data_types.value(i);
            let comment = comments.value(i);
            let owner = table_owners.value(i);

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            let entry = result.entry(fully_qualified_name.clone());

            if matches!(entry, Entry::Vacant(_)) {
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
        let column_comments = stats_sql_result.column_values::<StringArray>("column_comment")?;

        let mut columns_by_relation = BTreeMap::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            let column_name = column_names.value(i);
            let column_comment = column_comments.value(i);

            let column = ColumnMetadata {
                name: column_name.to_string(),
                index: column_indices.value(i).into(),
                data_type: column_types.value(i).to_string(),
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
            let sql = build_columns_sql(
                &quoted_database(relation.as_ref())?,
                &relation.schema_as_str()?,
                &relation.identifier_as_str()?,
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
            let schema = build_schema_from_columns(batch, adapter.engine().type_ops().as_ref())?;

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
        report_progress: bool,
    ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        type Acc = BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>;
        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          db_schema: &CatalogAndSchema|
              -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
            let query_ctx = QueryCtx::default().with_desc("list_relations_in_parallel");
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

fn build_schema_from_columns(
    columns_result: Arc<RecordBatch>,
    type_ops: &dyn TypeOps,
) -> AdapterResult<Arc<Schema>> {
    let column_names = columns_result.column_values::<StringArray>("column_name")?;
    let data_types = columns_result.column_values::<StringArray>("data_type")?;
    let max_lengths = columns_result.column_values::<Int32Array>("max_length")?;
    let precisions = columns_result.column_values::<Int32Array>("numeric_precision")?;
    let scales = columns_result.column_values::<Int32Array>("numeric_scale")?;
    let nullability = columns_result.column_values::<StringArray>("is_nullable")?;

    let mut fields = vec![];
    for i in 0..columns_result.num_rows() {
        let text_data_type = compose_type_text(
            data_types.value(i),
            max_lengths.value(i),
            precisions.value(i),
            scales.value(i),
        );
        let nullable = nullability.value(i) == "YES";

        let field = make_arrow_field(
            type_ops,
            column_names.value(i).to_string(),
            &text_data_type,
            Some(nullable),
            None,
        )?;
        fields.push(field);
    }

    Ok(Arc::new(Schema::new(fields)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_relations_sql_is_three_part_and_quotes_the_schema() {
        let sql = build_list_relations_sql("\"my_db\"", "my_schema");

        assert!(sql.contains("from \"my_db\".sys.objects o"));
        assert!(sql.contains("join \"my_db\".sys.schemas s"));
        assert!(sql.contains("s.name = 'my_schema'"));
        assert!(sql.contains("o.type in ('U', 'V')"));
    }

    #[test]
    fn test_get_relation_sql_matches_the_identifier_exactly() {
        let sql = build_get_relation_sql("\"my_db\"", "my_schema", "stg_orders");

        // `=`, not `like`: sp_tables' pattern matching also returns `stgXorders`.
        assert!(sql.contains("o.name = 'stg_orders'"));
        assert!(!sql.to_lowercase().contains("like"));
    }

    #[test]
    fn test_single_quotes_in_a_name_are_escaped() {
        let sql = build_get_relation_sql("\"my_db\"", "o'brien", "x");

        assert!(sql.contains("s.name = 'o''brien'"));
    }

    #[test]
    fn test_columns_sql_casts_the_narrow_integer_columns() {
        let sql = build_columns_sql("\"my_db\"", "my_schema", "my_table");

        assert!(sql.contains("cast(c.max_length as int) as max_length"));
        assert!(sql.contains("cast(c.precision as int) as numeric_precision"));
        assert!(sql.contains("cast(c.scale as int) as numeric_scale"));
        assert!(sql.contains("order by c.column_id"));
    }

    #[test]
    fn test_relation_type_from_table_type() {
        assert_eq!(
            relation_type_from_table_type("TABLE"),
            Some(RelationType::Table)
        );
        assert_eq!(
            relation_type_from_table_type("VIEW"),
            Some(RelationType::View)
        );
        assert_eq!(relation_type_from_table_type("SYNONYM"), None);
    }

    #[test]
    fn test_compose_type_text_lengths() {
        assert_eq!(compose_type_text("varchar", 50, 0, 0), "varchar(50)");
        assert_eq!(compose_type_text("varchar", -1, 0, 0), "varchar(max)");
        assert_eq!(compose_type_text("binary", 8, 0, 0), "binary(8)");
        assert_eq!(compose_type_text("varbinary", -1, 0, 0), "varbinary(max)");
    }

    #[test]
    fn test_compose_type_text_halves_the_national_lengths() {
        // sys.columns.max_length is bytes, and nvarchar stores two per character.
        assert_eq!(compose_type_text("nvarchar", 100, 0, 0), "nvarchar(50)");
        assert_eq!(compose_type_text("nchar", 20, 0, 0), "nchar(10)");
        assert_eq!(compose_type_text("nvarchar", -1, 0, 0), "nvarchar(max)");
    }

    #[test]
    fn test_compose_type_text_precision_and_scale() {
        assert_eq!(compose_type_text("decimal", 9, 18, 4), "decimal(18,4)");
        assert_eq!(compose_type_text("numeric", 5, 9, 3), "numeric(9,3)");
    }

    #[test]
    fn test_compose_type_text_scale_only_types() {
        assert_eq!(compose_type_text("datetime2", 8, 27, 7), "datetime2(7)");
        assert_eq!(compose_type_text("time", 4, 12, 3), "time(3)");
        assert_eq!(
            compose_type_text("datetimeoffset", 8, 29, 2),
            "datetimeoffset(2)"
        );
    }

    #[test]
    fn test_compose_type_text_leaves_unparameterized_types_alone() {
        // `datetime` reports scale 3 but rejects `datetime(3)`.
        assert_eq!(compose_type_text("datetime", 8, 23, 3), "datetime");
        assert_eq!(compose_type_text("int", 4, 10, 0), "int");
        assert_eq!(compose_type_text("float", 8, 53, 0), "float");
        assert_eq!(compose_type_text("real", 4, 24, 0), "real");
        assert_eq!(compose_type_text("money", 8, 19, 4), "money");
        assert_eq!(
            compose_type_text("uniqueidentifier", 16, 0, 0),
            "uniqueidentifier"
        );
    }
}
