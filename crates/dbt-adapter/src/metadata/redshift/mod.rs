use crate::adapter::adapter_impl::{AdapterImpl, get_bool_config, quote_ident};
use crate::connection::AdapterConnectionFactory;
use crate::errors::*;
use crate::metadata::*;
use crate::record_batch::RecordBatchExt;
use crate::relation::Relation;
use crate::sql_types::make_arrow_field_v2;
use crate::{AdapterEngine, AdapterResult};
use arrow::array::*;
use arrow::compute::concat_batches;
use arrow::datatypes::GenericStringType;
use arrow_schema::{DataType, Field, Schema};
use dbt_adapter_core::{AdapterType, ExecutionPhase};
use dbt_adapter_engine::MapReduce;
use dbt_adbc::*;
use dbt_common::cancellation::Cancellable;
use dbt_common::cancellation::CancellationToken;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::legacy_catalog::*;
use dbt_schemas::schemas::relations::base::RelationPattern;
use indexmap::IndexMap;

use std::collections::btree_map::Entry;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::metadata::list_objects::*;

/// Inline reproduction of the `redshift__get_relations` macro
/// (`dbt-loader/src/dbt_macro_assets/dbt-redshift/macros/relations.sql`).
/// Inlined because the async hydration path in `dbt-precompile` has no Jinja
/// `&State` to drive `execute_macro`.
const REDSHIFT_GET_RELATIONS_SQL: &str = r#"
with relation as (
    select
        pg_class.oid as relation_id,
        pg_class.relname as relation_name,
        pg_class.relnamespace as schema_id,
        pg_namespace.nspname as schema_name
    from pg_class
    join pg_namespace
      on pg_class.relnamespace = pg_namespace.oid
    where pg_namespace.nspname != 'information_schema'
      and pg_namespace.nspname not like 'pg\_%'
),
dependency as (
    select distinct
        coalesce(pg_rewrite.ev_class, pg_depend.objid) as dep_relation_id,
        pg_depend.refobjid as ref_relation_id
    from pg_depend
    left join pg_rewrite
      on pg_depend.objid = pg_rewrite.oid
    where coalesce(pg_rewrite.ev_class, pg_depend.objid) != pg_depend.refobjid
)
select distinct
    dep.schema_name as dependent_schema,
    dep.relation_name as dependent_name,
    ref.schema_name as referenced_schema,
    ref.relation_name as referenced_name
from dependency
join relation ref
    on dependency.ref_relation_id = ref.relation_id
join relation dep
    on dependency.dep_relation_id = dep.relation_id
"#;

fn parse_relation_dependencies(
    batch: &RecordBatch,
    catalog: &str,
    schema_matches: impl Fn(&str) -> bool,
    quoting: ResolvedQuoting,
) -> AdapterResult<Vec<ParentChildPair>> {
    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }
    let dep_schemas = batch.column_values::<StringArray>("dependent_schema")?;
    let dep_names = batch.column_values::<StringArray>("dependent_name")?;
    let ref_schemas = batch.column_values::<StringArray>("referenced_schema")?;
    let ref_names = batch.column_values::<StringArray>("referenced_name")?;

    let mut links: Vec<ParentChildPair> = Vec::new();
    for i in 0..batch.num_rows() {
        let ref_schema = ref_schemas.value(i).to_string();
        if !schema_matches(&ref_schema) {
            continue;
        }
        links.push((
            build_redshift_relation(
                catalog.to_string(),
                ref_schema,
                ref_names.value(i).to_string(),
                RelationType::View,
                quoting,
            )?,
            build_redshift_relation(
                catalog.to_string(),
                dep_schemas.value(i).to_string(),
                dep_names.value(i).to_string(),
                RelationType::View,
                quoting,
            )?,
        ));
    }
    Ok(links)
}

/// Per-schema variant for the cache-miss path. Scans pg_depend and filters
/// to rows whose referenced schema matches `db_schema`.
pub fn list_relation_dependencies(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<ParentChildPair>> {
    let batch = engine.execute(None, conn, ctx, REDSHIFT_GET_RELATIONS_SQL, token)?;
    let want = db_schema.resolved_schema.to_lowercase();
    parse_relation_dependencies(
        &batch,
        &db_schema.resolved_catalog,
        |schema| schema.to_lowercase() == want,
        engine.quoting(),
    )
}

/// Reference: https://github.com/dbt-labs/dbt-adapters/blob/87e81a47baa11c312003377091a9efc0ab72d88e/dbt-redshift/src/dbt/include/redshift/macros/adapters.sql#L226
pub fn list_relations(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    if get_bool_config(engine, "datasharing")? {
        list_relations_via_show_tables(engine, ctx, conn, db_schema, token)
    } else {
        list_relations_via_information_schema(engine, ctx, conn, db_schema, token)
    }
}

fn build_redshift_relation(
    database: String,
    schema: String,
    name: String,
    relation_type: RelationType,
    quoting: ResolvedQuoting,
) -> AdapterResult<Arc<dyn BaseRelation>> {
    let relation = Relation::new(AdapterType::Redshift, database, schema, name)
        .with_relation_type(relation_type)
        .with_quoting(quoting)
        .validate()?;
    Ok(Arc::new(relation) as Arc<dyn BaseRelation>)
}

fn list_relations_via_information_schema(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let sql = format!(
        "select
    table_catalog as database,
    table_name as name,
    table_schema as schema,
    'table' as type
from information_schema.tables
where table_schema ilike '{}'
and table_type = 'BASE TABLE'
union all
select
    table_catalog as database,
    table_name as name,
    table_schema as schema,
    case
    when view_definition ilike '%create materialized view%'
        then 'materialized_view'
    else 'view'
    end as type
from information_schema.views
where table_schema ilike '{}'",
        &db_schema.resolved_schema, &db_schema.resolved_schema
    );

    let batch = engine.execute(None, conn, ctx, &sql, token)?;

    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let table_name = batch.column_values::<StringArray>("name")?;
    let database_name = batch.column_values::<StringArray>("database")?;
    let schema_name = batch.column_values::<StringArray>("schema")?;
    let table_type = batch.column_values::<StringArray>("type")?;

    let mut relations = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        relations.push(build_redshift_relation(
            database_name.value(i).to_string(),
            schema_name.value(i).to_string(),
            table_name.value(i).to_string(),
            RelationType::from(table_type.value(i)),
            engine.quoting(),
        )?);
    }

    Ok(relations)
}

/// `SHOW TABLES FROM SCHEMA <db>.<schema>` (datasharing path) — required for
/// cross-database listing. Mirrors the upstream macro introduced in
/// dbt-labs/dbt-adapters#1671.
fn list_relations_via_show_tables(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let sql = format!(
        "SHOW TABLES FROM SCHEMA {}.{}",
        db_schema.rendered_catalog, db_schema.rendered_schema
    );

    let batch = engine.execute(None, conn, ctx, &sql, token)?;
    parse_show_tables_batch(&batch, engine.quoting())
}

/// Parse a `SHOW TABLES FROM SCHEMA` result batch into [BaseRelation]s.
///
/// Mirrors `transform_show_tables_for_list_relations` from
/// dbt-labs/dbt-adapters#1671. The defensive fallback for a missing
/// `table_subtype` column was added in dbt-labs/dbt-adapters#1745: that
/// column only exists on Redshift Patch 197+ (rolling out Nov 2025), so on
/// older clusters every VIEW is treated as a plain view.
fn parse_show_tables_batch(
    batch: &RecordBatch,
    quoting: ResolvedQuoting,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let table_name = batch.column_values::<StringArray>("table_name")?;
    let database_name = batch.column_values::<StringArray>("database_name")?;
    let schema_name = batch.column_values::<StringArray>("schema_name")?;
    let table_type = batch.column_values::<StringArray>("table_type")?;
    let table_subtype = batch
        .column_by_name("table_subtype")
        .and_then(|col| col.as_any().downcast_ref::<StringArray>());

    let mut relations = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        let kind = table_type.value(i).trim().to_uppercase();
        let subtype = table_subtype.map(|arr| arr.value(i).trim().to_uppercase());
        let relation_type = match (kind.as_str(), subtype.as_deref()) {
            ("VIEW", Some("MATERIALIZED VIEW")) => RelationType::MaterializedView,
            ("VIEW", _) => RelationType::View,
            _ => RelationType::Table,
        };

        relations.push(build_redshift_relation(
            database_name.value(i).to_string(),
            schema_name.value(i).to_string(),
            table_name.value(i).to_string(),
            relation_type,
            quoting,
        )?);
    }

    Ok(relations)
}

/// `table_type` and `table_subtype` must already be trimmed.
fn show_table_catalog_type(table_type: &str, table_subtype: Option<&str>) -> &'static str {
    fn lookup(value: &str) -> Option<&'static str> {
        match value {
            v if v.eq_ignore_ascii_case("TABLE") => Some("BASE TABLE"),
            v if v.eq_ignore_ascii_case("VIEW") => Some("VIEW"),
            v if v.eq_ignore_ascii_case("MATERIALIZED VIEW") => Some("MATERIALIZED VIEW"),
            v if v.eq_ignore_ascii_case("LATE BINDING VIEW") => Some("LATE BINDING VIEW"),
            _ => None,
        }
    }
    table_subtype
        .and_then(lookup)
        .or_else(|| lookup(table_type))
        .unwrap_or("BASE TABLE")
}

/// Converts a `Decimal128Array` cell to a `serde_json::Value`, dividing out the
/// decimal's scale first (`Decimal128Array::value` returns the raw unscaled i128
/// mantissa, e.g. `4550000000` for `45.5` at scale 8).
fn decimal128_to_json_value(array: &Decimal128Array, i: usize) -> serde_json::Value {
    let scaled = array.value(i) as f64 / 10f64.powi(array.scale() as i32);
    serde_json::Number::from_f64(scaled)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

fn table_hash(database: &str, schema: &str, table: &str) -> u64 {
    const FIELD_SEPARATOR: &str = "|";

    fn hash_lowercase(value: &str, hasher: &mut impl Hasher) {
        for c in value.chars().flat_map(char::to_lowercase) {
            c.hash(hasher);
        }
    }

    let mut h = DefaultHasher::new();
    database.hash(&mut h);
    FIELD_SEPARATOR.hash(&mut h);
    hash_lowercase(schema, &mut h);
    FIELD_SEPARATOR.hash(&mut h);
    hash_lowercase(table, &mut h);
    h.finish()
}

/// Build the base catalog for Redshift datasharing by joining `SHOW TABLES FROM SCHEMA`
/// (table metadata) with `SVV_REDSHIFT_COLUMNS` (column metadata).
///
/// Redshift's SVV views live on the leader node and cannot be joined to `SHOW` results in
/// SQL, so the catalog macro fetches both and we join them here. The output columns match
/// the legacy `pg_catalog` catalog SQL, so [`build_schemas_from_stats_sql`] and
/// [`build_columns_from_get_columns`] consume the batch unchanged.
///
/// Reference: <https://github.com/dbt-labs/dbt-adapters/pull/1718>
/// SHOW TABLES: <https://docs.aws.amazon.com/redshift/latest/dg/r_SHOW_TABLES.html>
/// SVV_REDSHIFT_COLUMNS: <https://docs.aws.amazon.com/redshift/latest/dg/r_SVV_REDSHIFT_COLUMNS.html>
/// TODO (debrin-og): Refactor and move to `AgateTable.join`
pub(crate) fn join_show_tables_and_svv_columns(
    show_tables_results: &[Arc<RecordBatch>],
    svv_columns: &RecordBatch,
) -> AdapterResult<RecordBatch> {
    let catalog_schema = Arc::new(Schema::new(vec![
        Field::new("table_database", DataType::Utf8, false),
        Field::new("table_schema", DataType::Utf8, false),
        Field::new("table_name", DataType::Utf8, false),
        Field::new("table_type", DataType::Utf8, false),
        Field::new("table_comment", DataType::Utf8, false),
        Field::new("table_owner", DataType::Utf8, false),
        Field::new("column_name", DataType::Utf8, false),
        Field::new("column_index", DataType::Int32, false),
        Field::new("column_type", DataType::Utf8, false),
        Field::new("column_comment", DataType::Utf8, false),
    ]));

    let show_tables_batches: Vec<&RecordBatch> = show_tables_results
        .iter()
        .filter(|batch| batch.num_rows() > 0)
        .map(|batch| batch.as_ref())
        .collect();

    if show_tables_batches.is_empty() || svv_columns.num_rows() == 0 {
        return Ok(RecordBatch::new_empty(catalog_schema));
    }

    let show_tables = concat_batches(
        &show_tables_batches[0].schema(),
        show_tables_batches.iter().copied(),
    )
    .map_err(|e| {
        AdapterError::new(
            AdapterErrorKind::Internal,
            format!("failed to flatten redshift SHOW TABLES record batches: {e}"),
        )
    })?;

    let show_database = show_tables.column_values::<StringArray>("database_name")?;
    let show_schema = show_tables.column_values::<StringArray>("schema_name")?;
    let show_table = show_tables.column_values::<StringArray>("table_name")?;
    let show_type = show_tables.column_values::<StringArray>("table_type")?;
    // `table_subtype` is the only column that can be absent (added in
    // dbt-adapters#1745; pre-Patch-197 Redshift omits it), so it stays optional.
    let show_subtype = show_tables
        .column_by_name("table_subtype")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let show_owner = show_tables.column_values::<StringArray>("owner")?;
    let show_remarks = show_tables.column_values::<StringArray>("remarks")?;

    let mut tables = HashMap::with_capacity(show_tables.num_rows());
    for show_index in 0..show_tables.num_rows() {
        tables.insert(
            table_hash(
                show_database.value(show_index),
                show_schema.value(show_index),
                show_table.value(show_index),
            ),
            show_index,
        );
    }

    let db = svv_columns.column_values::<StringArray>("database_name")?;
    let sch = svv_columns.column_values::<StringArray>("schema_name")?;
    let tbl = svv_columns.column_values::<StringArray>("table_name")?;
    let name = svv_columns.column_values::<StringArray>("column_name")?;
    let dtype = svv_columns.column_values::<StringArray>("data_type")?;
    let remarks = svv_columns.column_values::<StringArray>("remarks")?;
    let ordinal = svv_columns.column_values::<Int32Array>("ordinal_position")?;

    // The catalog is column-level: SHOW TABLES rows with no SVV column rows do not emit
    // catalog rows, and SVV rows without a SHOW TABLES parent are omitted.
    let matched: Vec<(usize, usize)> = (0..svv_columns.num_rows())
        .filter_map(|i| {
            tables
                .get(&table_hash(db.value(i), sch.value(i), tbl.value(i)))
                .map(|&show_index| (show_index, i))
        })
        .collect();

    // Column types are pinned to what the downstream `column_values` downcasts expect.
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from_iter_values(
            matched
                .iter()
                .map(|(show_index, _)| show_database.value(*show_index)),
        )),
        Arc::new(StringArray::from_iter_values(
            matched
                .iter()
                .map(|(show_index, _)| show_schema.value(*show_index)),
        )),
        Arc::new(StringArray::from_iter_values(
            matched
                .iter()
                .map(|(show_index, _)| show_table.value(*show_index)),
        )),
        Arc::new(StringArray::from_iter_values(matched.iter().map(
            |(show_index, _)| {
                show_table_catalog_type(
                    show_type.value(*show_index).trim(),
                    show_subtype.map(|a| a.value(*show_index).trim()),
                )
            },
        ))),
        Arc::new(StringArray::from_iter_values(
            matched
                .iter()
                .map(|(show_index, _)| show_remarks.value(*show_index)),
        )),
        Arc::new(StringArray::from_iter_values(
            matched
                .iter()
                .map(|(show_index, _)| show_owner.value(*show_index)),
        )),
        Arc::new(StringArray::from_iter_values(
            matched.iter().map(|(_, i)| name.value(*i)),
        )),
        Arc::new(Int32Array::from_iter_values(
            matched.iter().map(|(_, i)| ordinal.value(*i)),
        )),
        Arc::new(StringArray::from_iter_values(
            matched.iter().map(|(_, i)| dtype.value(*i)),
        )),
        Arc::new(StringArray::from_iter_values(
            matched.iter().map(|(_, i)| remarks.value(*i)),
        )),
    ];

    RecordBatch::try_new(catalog_schema, columns).map_err(|e| {
        AdapterError::new(
            AdapterErrorKind::Internal,
            format!("failed to build redshift catalog record batch: {e}"),
        )
    })
}

pub(crate) struct RedshiftListRelationsSchemasStrategy {
    adapter: AdapterImpl,
}

impl RedshiftListRelationsSchemasStrategy {
    pub(crate) fn new(adapter: AdapterImpl) -> Self {
        Self { adapter }
    }
}

impl ListRelationsSchemasStrategy for RedshiftListRelationsSchemasStrategy {
    fn run(
        &self,
        relations: Arc<Vec<Arc<dyn BaseRelation>>>,
        unique_id: Option<String>,
        phase: Option<ExecutionPhase>,
        item_span_operation_id: Option<&str>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, HashMap<String, AdapterResult<Arc<Schema>>>> {
        type Acc = HashMap<String, AdapterResult<Arc<Schema>>>;

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          relation: &Arc<dyn BaseRelation>|
              -> AdapterResult<Arc<Schema>> {
            let catalog = relation.database_as_str()?;
            let schema = relation.schema_as_str()?;
            let identifier = relation.identifier_as_str()?;

            // Use EXISTS with r_SVV_EXTERNAL_TABLES (presumably small, only external tables compared to SVV_TABLES)
            // https://docs.aws.amazon.com/redshift/latest/dg/r_SVV_EXTERNAL_TABLES.html
            // SUBQUERY is more performant, it's 650 ms vs 3.3 s from JOIN on the redshift_spectrum_path test project
            let sql = format!(
                "SELECT
    column_name,
    data_type,
    is_nullable,
    remarks,
    EXISTS(SELECT 1 FROM SVV_EXTERNAL_TABLES
           WHERE redshift_database_name = '{catalog}'
           AND schemaname = '{schema}'
           AND tablename = '{identifier}') AS is_external
FROM SVV_ALL_COLUMNS
WHERE database_name = '{catalog}'
AND schema_name = '{schema}'
AND table_name = '{identifier}'"
            );

            let mut ctx = QueryCtx::new_metadata().with_desc("Get table schema");
            if let Some(node_id) = unique_id.clone() {
                ctx = ctx.with_node_id(&node_id);
            }
            if let Some(phase) = phase {
                ctx = ctx.with_phase(phase.as_str());
            }
            let (_, table) = adapter.query(&ctx, &mut *conn, &sql, None, token_clone.clone())?;
            let batch = table.original_record_batch();
            // Build fields from the response
            let mut fields = Vec::new();
            let column_names = batch.column_values::<StringArray>("column_name")?;
            let data_types = batch.column_values::<StringArray>("data_type")?;
            let is_nullables = batch.column_values::<StringArray>("is_nullable")?;
            let comments = batch.column_values::<StringArray>("remarks")?;
            let is_external_flags = batch.column_values::<BooleanArray>("is_external")?;

            let is_external = if batch.num_rows() > 0 {
                is_external_flags.value(0)
            } else {
                false
            };

            for i in 0..batch.num_rows() {
                let name = column_names.value(i);
                let data_type = data_types.value(i);
                let is_nullable = is_nullables.value(i) == "YES";
                let comment = match comments.value(i) {
                    "" => None,
                    c => Some(c.to_string()),
                };

                let field = make_arrow_field_v2(
                    adapter.engine().type_ops().as_ref(),
                    String::from(name),
                    data_type,
                    Some(is_nullable),
                    comment,
                )?;

                fields.push(field);
            }

            // Some pg_ tables contain an "invisible" oid column
            // See: https://docs.aws.amazon.com/redshift/latest/dg/c_join_PG.html
            if schema == "pg_catalog" && TABLES_WITH_OID.contains(&identifier.as_str()) {
                let field = make_arrow_field_v2(
                    adapter.engine().type_ops().as_ref(),
                    String::from("oid"),
                    "oid",
                    Some(false),
                    None,
                )?;
                fields.push(field);
            }

            if fields.is_empty() {
                return Err(AdapterError::new(
                    AdapterErrorKind::UnexpectedResult,
                    format!("No schema in SVV_COLUMNS for {catalog}.{schema}.{identifier}"),
                ));
            }

            let table_schema = if is_external {
                // Add Redshift Spectrum pseudo columns for external tables
                // https://docs.aws.amazon.com/redshift/latest/dg/c-spectrum-external-tables.html#r_spectrum_pseudo_columns
                fields.push(Field::new("$path", DataType::Utf8, true));
                fields.push(Field::new("$size", DataType::Int64, true));
                Arc::new(Schema::new(fields))
            } else {
                Arc::new(Schema::new(fields))
            };

            Ok(table_schema)
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

    fn run_by_patterns(
        &self,
        _patterns: Arc<Vec<RelationPattern>>,
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'static, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        todo!("list_relations_schemas_by_patterns for Redshift")
    }
}

// This list was created using brute force since I can't find docs for which tables support it
const TABLES_WITH_OID: [&str; 10] = [
    "pg_cast",
    "pg_opclass",
    "pg_class",
    "pg_constraint",
    "pg_database",
    "pg_language",
    "pg_namespace",
    "pg_operator",
    "pg_proc",
    "pg_type",
];

pub(crate) struct RedshiftFreshnessStrategy {
    adapter: AdapterImpl,
}

impl RedshiftFreshnessStrategy {
    pub(crate) fn new(adapter: AdapterImpl) -> Self {
        Self { adapter }
    }
}

impl FreshnessStrategy for RedshiftFreshnessStrategy {
    fn run(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, BTreeMap<String, MetadataFreshness>> {
        // datasharing sources can span databases; pg_class is single-db, SHOW TABLES isn't.
        match get_bool_config(self.adapter.engine().as_ref(), "datasharing") {
            Ok(true) => self.run_via_show_tables(relations, token),
            Ok(false) => self.run_via_pg_class(relations, token),
            Err(e) => Box::pin(async move { Err(Cancellable::Error(e)) }),
        }
    }
}

impl RedshiftFreshnessStrategy {
    fn run_via_pg_class(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, BTreeMap<String, MetadataFreshness>> {
        // Group relations by (catalog, schema) to batch one query per schema.
        let mut by_schema: BTreeMap<(String, String), Vec<Arc<dyn BaseRelation>>> = BTreeMap::new();
        for relation in relations {
            let catalog = relation.database_as_str().unwrap_or_default().to_string();
            let schema = relation.schema_as_str().unwrap_or_default().to_string();
            by_schema
                .entry((catalog, schema))
                .or_default()
                .push(relation.clone());
        }

        type Acc = BTreeMap<String, MetadataFreshness>;
        // name → (epoch_ms, is_view)
        type MapResult = HashMap<String, (i64, bool)>;

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();

        let map_f = move |conn: &'_ mut dyn Connection,
                          group: &((String, String), Vec<Arc<dyn BaseRelation>>)|
              -> AdapterResult<MapResult> {
            let ((_, schema), _) = group;

            // Primary: stl_insert tracks INSERT/COPY/CTAS, batched by schema.
            // ~1s vs ~10s for sys_query_detail on this cluster.
            // Not available on Redshift Serverless — falls back to pg_class_info only.
            let stl_sql = format!(
                "SELECT trim(trailing from c.relname) AS table_name, \
                        MAX(i.endtime) AS last_modified \
                 FROM stl_insert i \
                 JOIN pg_class c ON c.oid = i.tbl \
                 JOIN pg_namespace ns ON ns.oid = c.relnamespace \
                 WHERE upper(ns.nspname) = upper('{schema}') \
                 GROUP BY c.relname"
            );
            let ctx = QueryCtx::default().with_desc("Extracting freshness via stl_insert");
            let stl_result = adapter.query(&ctx, conn, &stl_sql, None, token_clone.clone());

            let mut epoch_by_name: MapResult = HashMap::new();
            match stl_result {
                Ok((_, agate)) => {
                    let stl_batch = agate.original_record_batch();
                    if stl_batch.num_rows() > 0 {
                        let names = stl_batch.column_values::<StringArray>("table_name")?;
                        let ts = stl_batch
                            .column_values::<TimestampMicrosecondArray>("last_modified")?;
                        for i in 0..stl_batch.num_rows() {
                            if !ts.is_null(i) {
                                let name = names.value(i).trim().to_lowercase();
                                // stl_insert returns microseconds; convert to millis
                                epoch_by_name.insert(name, (ts.value(i) / 1_000, false));
                            }
                        }
                    }
                }
                Err(_) => {
                    // stl_insert is unavailable (e.g. Redshift Serverless permission denied).
                    // pg_class_info below will supply creation_time as the epoch for all relations.
                }
            }

            // Fallback: pg_class_info.relcreationtime for relations with no insert history
            // (newly created empty tables, pure-DELETE tables, views).
            // Also provides is_view detection via relkind.
            let pci_sql = format!(
                "SELECT trim(trailing from c.relname) AS table_name, \
                        pci.relcreationtime AS creation_time, \
                        CASE WHEN c.relkind = 'v' THEN 1 ELSE 0 END AS is_view \
                 FROM pg_class c \
                 JOIN pg_namespace ns ON ns.oid = c.relnamespace \
                 JOIN pg_class_info pci ON pci.reloid = c.oid \
                 WHERE upper(ns.nspname) = upper('{schema}') \
                 AND pci.relcreationtime IS NOT NULL"
            );
            let ctx2 =
                QueryCtx::default().with_desc("Redshift freshness fallback via pg_class_info");
            let (_, agate2) = adapter.query(&ctx2, conn, &pci_sql, None, token_clone.clone())?;
            let pci_batch = agate2.original_record_batch();

            if pci_batch.num_rows() > 0 {
                let names = pci_batch.column_values::<StringArray>("table_name")?;
                let cts = pci_batch.column_values::<TimestampMicrosecondArray>("creation_time")?;
                let is_view_col = pci_batch.column_values::<Int32Array>("is_view")?;
                for i in 0..pci_batch.num_rows() {
                    let name = names.value(i).trim().to_lowercase();
                    let is_view = is_view_col.value(i) != 0;
                    epoch_by_name
                        .entry(name)
                        .and_modify(|(_, v)| *v = is_view) // patch is_view for tables with stl epoch
                        .or_insert_with(|| {
                            // No stl_insert entry — use creation_time as fallback epoch
                            let ms = if cts.is_null(i) {
                                0
                            } else {
                                cts.value(i) / 1_000
                            };
                            (ms, is_view)
                        });
                }
            }

            Ok(epoch_by_name)
        };

        let reduce_f = move |acc: &mut Acc,
                             group: ((String, String), Vec<Arc<dyn BaseRelation>>),
                             result: AdapterResult<MapResult>|
              -> Result<(), Cancellable<AdapterError>> {
            let epoch_by_name = result?;
            let (_, group_relations) = group;
            for relation in &group_relations {
                let identifier = relation
                    .identifier_as_str()
                    .map_err(|e| {
                        Cancellable::Error(AdapterError::new(
                            AdapterErrorKind::UnexpectedResult,
                            e.to_string(),
                        ))
                    })?
                    .trim()
                    .to_lowercase();
                if let Some(&(epoch_ms, is_view)) = epoch_by_name.get(&identifier) {
                    acc.insert(
                        relation.semantic_fqn(),
                        MetadataFreshness::from_millis(epoch_ms, is_view)
                            .map_err(Cancellable::Error)?,
                    );
                }
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        let keys: Vec<_> = by_schema.into_iter().collect();
        map_reduce.run(Arc::new(keys), token)
    }

    /// Datasharing freshness path. Group sources by (database, schema), issue one
    /// `SHOW TABLES FROM SCHEMA "<db>"."<schema>"` per group, and read
    /// `last_modified_time` from the response.
    ///
    /// Note: per the Redshift docs, `last_modified_time` can lag table writes by
    /// up to ~20 minutes, so freshness here is best-effort, not exact.
    fn run_via_show_tables(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, BTreeMap<String, MetadataFreshness>> {
        let mut relations_by_schema: BTreeMap<(String, String), Vec<Arc<dyn BaseRelation>>> =
            BTreeMap::new();
        for relation in relations {
            let database = relation.database_as_str().unwrap_or_default();
            let schema = relation.schema_as_str().unwrap_or_default();
            relations_by_schema
                .entry((database, schema))
                .or_default()
                .push(Arc::clone(relation));
        }
        let keys: Vec<(String, String)> = relations_by_schema.keys().cloned().collect();

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));
        let adapter = self.adapter.clone();
        let token_clone = token.clone();

        let map_f = move |conn: &'_ mut dyn Connection,
                          (database, schema): &(String, String)|
              -> AdapterResult<Arc<RecordBatch>> {
            let sql = format!(
                "SHOW TABLES FROM SCHEMA {}.{}",
                quote_ident(AdapterType::Redshift, database),
                quote_ident(AdapterType::Redshift, schema),
            );
            let ctx = QueryCtx::default().with_desc("Extracting freshness via SHOW TABLES");
            let (_, agate_table) =
                adapter.query(&ctx, &mut *conn, &sql, None, token_clone.clone())?;
            Ok(agate_table.original_record_batch())
        };

        type Acc = BTreeMap<String, MetadataFreshness>;
        let reduce_f = move |acc: &mut Acc,
                             key: (String, String),
                             batch_res: AdapterResult<Arc<RecordBatch>>|
              -> Result<(), Cancellable<AdapterError>> {
            let batch = batch_res?;
            // `key` came from `relations_by_schema.keys()`, so it is always present.
            let relations = relations_by_schema
                .get(&key)
                .expect("reduce key originates from relations_by_schema");
            acc.extend(parse_show_tables_freshness_batch(&batch, relations)?);
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(keys), token)
    }
}

/// Parse a `SHOW TABLES FROM SCHEMA` result batch into a freshness map keyed by
/// each matching relation's semantic FQN. Rows whose table is not in the
/// requested set are ignored.
fn parse_show_tables_freshness_batch(
    batch: &RecordBatch,
    relations: &[Arc<dyn BaseRelation>],
) -> AdapterResult<BTreeMap<String, MetadataFreshness>> {
    let mut out = BTreeMap::new();
    if batch.num_rows() == 0 {
        return Ok(out);
    }

    let schema_name = batch.column_values::<StringArray>("schema_name")?;
    let table_name = batch.column_values::<StringArray>("table_name")?;
    let table_type = batch.column_values::<StringArray>("table_type")?;
    let last_modified = batch.column_values::<TimestampMicrosecondArray>("last_modified_time")?;

    for i in 0..batch.num_rows() {
        if last_modified.is_null(i) {
            continue;
        }
        let schema = schema_name.value(i);
        let table = table_name.value(i);
        let timestamp = last_modified.value(i);
        let is_view = table_type.value(i).trim().eq_ignore_ascii_case("VIEW");
        for fqn in find_matching_relation(schema, table, relations)? {
            out.insert(fqn, MetadataFreshness::from_micros(timestamp, is_view)?);
        }
    }
    Ok(out)
}

pub struct RedshiftMetadataAdapter {
    adapter: AdapterImpl,
}

impl RedshiftMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }
}

impl MetadataAdapter for RedshiftMetadataAdapter {
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

        let schema = stats_sql_result.schema();
        let columns = schema
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect::<Vec<&str>>();
        let contains_stats = columns.contains(&"stats:encoded:label");

        let table_catalogs = stats_sql_result.column_values::<StringArray>("table_database")?;
        let table_schemas = stats_sql_result.column_values::<StringArray>("table_schema")?;
        let table_names = stats_sql_result.column_values::<StringArray>("table_name")?;
        let data_types = stats_sql_result.column_values::<StringArray>("table_type")?;
        let comments = stats_sql_result.column_values::<StringArray>("table_comment")?;
        let table_owners = stats_sql_result.column_values::<StringArray>("table_owner")?;

        if !contains_stats {
            return build_schema_from_stats_sql_without_stats(
                table_catalogs,
                table_schemas,
                table_names,
                data_types,
                comments,
                table_owners,
            );
        }

        let encoded_label = stats_sql_result.column_values::<StringArray>("stats:encoded:label")?;
        let encoded_value = stats_sql_result.column_values::<StringArray>("stats:encoded:value")?;
        let encoded_description =
            stats_sql_result.column_values::<StringArray>("stats:encoded:description")?;
        let encoded_include =
            stats_sql_result.column_values::<BooleanArray>("stats:encoded:include")?;

        let diststyle_label =
            stats_sql_result.column_values::<StringArray>("stats:diststyle:label")?;
        let diststyle_value =
            stats_sql_result.column_values::<StringArray>("stats:diststyle:value")?;
        let diststyle_description =
            stats_sql_result.column_values::<StringArray>("stats:diststyle:description")?;
        let diststyle_include =
            stats_sql_result.column_values::<BooleanArray>("stats:diststyle:include")?;

        let sortkey1_label =
            stats_sql_result.column_values::<StringArray>("stats:sortkey1:label")?;
        let sortkey1_value =
            stats_sql_result.column_values::<StringArray>("stats:sortkey1:value")?;
        let sortkey1_description =
            stats_sql_result.column_values::<StringArray>("stats:sortkey1:description")?;
        let sortkey1_include =
            stats_sql_result.column_values::<BooleanArray>("stats:sortkey1:include")?;

        let max_varchar_label =
            stats_sql_result.column_values::<StringArray>("stats:max_varchar:label")?;
        let max_varchar_value =
            stats_sql_result.column_values::<Int32Array>("stats:max_varchar:value")?;
        let max_varchar_description =
            stats_sql_result.column_values::<StringArray>("stats:max_varchar:description")?;
        let max_varchar_include =
            stats_sql_result.column_values::<BooleanArray>("stats:max_varchar:include")?;

        let sortkey1_enc_label =
            stats_sql_result.column_values::<StringArray>("stats:sortkey1_enc:label")?;
        let sortkey1_enc_value =
            stats_sql_result.column_values::<StringArray>("stats:sortkey1_enc:value")?;
        let sortkey1_enc_description =
            stats_sql_result.column_values::<StringArray>("stats:sortkey1_enc:description")?;
        let sortkey1_enc_include =
            stats_sql_result.column_values::<BooleanArray>("stats:sortkey1_enc:include")?;

        let sortkey_num_label =
            stats_sql_result.column_values::<StringArray>("stats:sortkey_num:label")?;
        let sortkey_num_value =
            stats_sql_result.column_values::<Int32Array>("stats:sortkey_num:value")?;
        let sortkey_num_description =
            stats_sql_result.column_values::<StringArray>("stats:sortkey_num:description")?;
        let sortkey_num_include =
            stats_sql_result.column_values::<BooleanArray>("stats:sortkey_num:include")?;

        let size_label = stats_sql_result.column_values::<StringArray>("stats:size:label")?;
        let size_value = stats_sql_result.column_values::<Int64Array>("stats:size:value")?;
        let size_description =
            stats_sql_result.column_values::<StringArray>("stats:size:description")?;
        let size_include = stats_sql_result.column_values::<BooleanArray>("stats:size:include")?;

        let pct_used_label =
            stats_sql_result.column_values::<StringArray>("stats:pct_used:label")?;
        let pct_used_value =
            stats_sql_result.column_values::<Decimal128Array>("stats:pct_used:value")?;
        let pct_used_description =
            stats_sql_result.column_values::<StringArray>("stats:pct_used:description")?;
        let pct_used_include =
            stats_sql_result.column_values::<BooleanArray>("stats:pct_used:include")?;

        let unsorted_label =
            stats_sql_result.column_values::<StringArray>("stats:unsorted:label")?;
        let unsorted_value =
            stats_sql_result.column_values::<Decimal128Array>("stats:unsorted:value")?;
        let unsorted_description =
            stats_sql_result.column_values::<StringArray>("stats:unsorted:description")?;
        let unsorted_include =
            stats_sql_result.column_values::<BooleanArray>("stats:unsorted:include")?;

        let stats_off_label =
            stats_sql_result.column_values::<StringArray>("stats:stats_off:label")?;
        let stats_off_value =
            stats_sql_result.column_values::<Decimal128Array>("stats:stats_off:value")?;
        let stats_off_description =
            stats_sql_result.column_values::<StringArray>("stats:stats_off:description")?;
        let stats_off_include =
            stats_sql_result.column_values::<BooleanArray>("stats:stats_off:include")?;

        let rows_label = stats_sql_result.column_values::<StringArray>("stats:rows:label")?;
        let rows_value = stats_sql_result.column_values::<Decimal128Array>("stats:rows:value")?;
        let rows_description =
            stats_sql_result.column_values::<StringArray>("stats:rows:description")?;
        let rows_include = stats_sql_result.column_values::<BooleanArray>("stats:rows:include")?;

        let skew_sortkey1_label =
            stats_sql_result.column_values::<StringArray>("stats:skew_sortkey1:label")?;
        let skew_sortkey1_value =
            stats_sql_result.column_values::<Decimal128Array>("stats:skew_sortkey1:value")?;
        let skew_sortkey1_description =
            stats_sql_result.column_values::<StringArray>("stats:skew_sortkey1:description")?;
        let skew_sortkey1_include =
            stats_sql_result.column_values::<BooleanArray>("stats:skew_sortkey1:include")?;

        let skew_rows_label =
            stats_sql_result.column_values::<StringArray>("stats:skew_rows:label")?;
        let skew_rows_value =
            stats_sql_result.column_values::<Decimal128Array>("stats:skew_rows:value")?;
        let skew_rows_description =
            stats_sql_result.column_values::<StringArray>("stats:skew_rows:description")?;
        let skew_rows_include =
            stats_sql_result.column_values::<BooleanArray>("stats:skew_rows:include")?;

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
                let encoded_label_i = encoded_label.value(i);
                let encoded_value_i = encoded_value.value(i);
                let encoded_description_i = encoded_description.value(i);
                let encoded_include_i = encoded_include.value(i);

                let diststyle_label_i = diststyle_label.value(i);
                let diststyle_value_i = diststyle_value.value(i);
                let diststyle_description_i = diststyle_description.value(i);
                let diststyle_include_i = diststyle_include.value(i);

                let sortkey1_label_i = sortkey1_label.value(i);
                let sortkey1_value_i = sortkey1_value.value(i);
                let sortkey1_description_i = sortkey1_description.value(i);
                let sortkey1_include_i = sortkey1_include.value(i);

                let max_varchar_label_i = max_varchar_label.value(i);
                let max_varchar_value_i = max_varchar_value.value(i);
                let max_varchar_description_i = max_varchar_description.value(i);
                let max_varchar_include_i = max_varchar_include.value(i);

                let sortkey1_enc_label_i = sortkey1_enc_label.value(i);
                let sortkey1_enc_value_i = sortkey1_enc_value.value(i);
                let sortkey1_enc_description_i = sortkey1_enc_description.value(i);
                let sortkey1_enc_include_i = sortkey1_enc_include.value(i);

                let sortkey_num_label_i = sortkey_num_label.value(i);
                let sortkey_num_value_i = sortkey_num_value.value(i);
                let sortkey_num_description_i = sortkey_num_description.value(i);
                let sortkey_num_include_i = sortkey_num_include.value(i);

                let size_label_i = size_label.value(i);
                let size_value_i = size_value.value(i);
                let size_description_i = size_description.value(i);
                let size_include_i = size_include.value(i);

                let pct_used_label_i = pct_used_label.value(i);
                let pct_used_description_i = pct_used_description.value(i);
                let pct_used_include_i = pct_used_include.value(i);

                let unsorted_label_i = unsorted_label.value(i);
                let unsorted_description_i = unsorted_description.value(i);
                let unsorted_include_i = unsorted_include.value(i);

                let stats_off_label_i = stats_off_label.value(i);
                let stats_off_description_i = stats_off_description.value(i);
                let stats_off_include_i = stats_off_include.value(i);

                let rows_label_i = rows_label.value(i);
                let rows_description_i = rows_description.value(i);
                let rows_include_i = rows_include.value(i);

                let skew_sortkey1_label_i = skew_sortkey1_label.value(i);
                let skew_sortkey1_description_i = skew_sortkey1_description.value(i);
                let skew_sortkey1_include_i = skew_sortkey1_include.value(i);

                let skew_rows_label_i = skew_rows_label.value(i);
                let skew_rows_description_i = skew_rows_description.value(i);
                let skew_rows_include_i = skew_rows_include.value(i);

                let mut stats = BTreeMap::new();

                if encoded_include_i {
                    stats.insert(
                        "encoded".to_string(),
                        CatalogNodeStats {
                            id: "encoded".to_string(),
                            label: encoded_label_i.to_string(),
                            value: serde_json::Value::String(encoded_value_i.to_string()),
                            description: Some(encoded_description_i.to_string()),
                            include: encoded_include_i,
                        },
                    );
                }

                if diststyle_include_i {
                    stats.insert(
                        "diststyle".to_string(),
                        CatalogNodeStats {
                            id: "diststyle".to_string(),
                            label: diststyle_label_i.to_string(),
                            value: serde_json::Value::String(diststyle_value_i.to_string()),
                            description: Some(diststyle_description_i.to_string()),
                            include: diststyle_include_i,
                        },
                    );
                }

                if sortkey1_include_i {
                    stats.insert(
                        "sortkey1".to_string(),
                        CatalogNodeStats {
                            id: "sortkey1".to_string(),
                            label: sortkey1_label_i.to_string(),
                            value: serde_json::Value::String(sortkey1_value_i.to_string()),
                            description: Some(sortkey1_description_i.to_string()),
                            include: sortkey1_include_i,
                        },
                    );
                }

                if max_varchar_include_i {
                    stats.insert(
                        "max_varchar".to_string(),
                        CatalogNodeStats {
                            id: "max_varchar".to_string(),
                            label: max_varchar_label_i.to_string(),
                            value: serde_json::Value::Number(max_varchar_value_i.into()),
                            description: Some(max_varchar_description_i.to_string()),
                            include: max_varchar_include_i,
                        },
                    );
                }

                if sortkey1_enc_include_i {
                    stats.insert(
                        "sortkey1_enc".to_string(),
                        CatalogNodeStats {
                            id: "sortkey1_enc".to_string(),
                            label: sortkey1_enc_label_i.to_string(),
                            value: serde_json::Value::String(sortkey1_enc_value_i.to_string()),
                            description: Some(sortkey1_enc_description_i.to_string()),
                            include: sortkey1_enc_include_i,
                        },
                    );
                }

                if sortkey_num_include_i {
                    stats.insert(
                        "sortkey_num".to_string(),
                        CatalogNodeStats {
                            id: "sortkey_num".to_string(),
                            label: sortkey_num_label_i.to_string(),
                            value: serde_json::Value::Number(sortkey_num_value_i.into()),
                            description: Some(sortkey_num_description_i.to_string()),
                            include: sortkey_num_include_i,
                        },
                    );
                }

                if size_include_i {
                    stats.insert(
                        "size".to_string(),
                        CatalogNodeStats {
                            id: "size".to_string(),
                            label: size_label_i.to_string(),
                            value: serde_json::Value::Number(size_value_i.into()),
                            description: Some(size_description_i.to_string()),
                            include: size_include_i,
                        },
                    );
                }

                if pct_used_include_i {
                    stats.insert(
                        "pct_used".to_string(),
                        CatalogNodeStats {
                            id: "pct_used".to_string(),
                            label: pct_used_label_i.to_string(),
                            value: decimal128_to_json_value(&pct_used_value, i),
                            description: Some(pct_used_description_i.to_string()),
                            include: pct_used_include_i,
                        },
                    );
                }

                if unsorted_include_i {
                    stats.insert(
                        "unsorted".to_string(),
                        CatalogNodeStats {
                            id: "unsorted".to_string(),
                            label: unsorted_label_i.to_string(),
                            value: decimal128_to_json_value(&unsorted_value, i),
                            description: Some(unsorted_description_i.to_string()),
                            include: unsorted_include_i,
                        },
                    );
                }

                if stats_off_include_i {
                    stats.insert(
                        "stats_off".to_string(),
                        CatalogNodeStats {
                            id: "stats_off".to_string(),
                            label: stats_off_label_i.to_string(),
                            value: decimal128_to_json_value(&stats_off_value, i),
                            description: Some(stats_off_description_i.to_string()),
                            include: stats_off_include_i,
                        },
                    );
                }

                if rows_include_i {
                    stats.insert(
                        "rows".to_string(),
                        CatalogNodeStats {
                            id: "rows".to_string(),
                            label: rows_label_i.to_string(),
                            value: decimal128_to_json_value(&rows_value, i),
                            description: Some(rows_description_i.to_string()),
                            include: rows_include_i,
                        },
                    );
                }

                if skew_sortkey1_include_i {
                    stats.insert(
                        "skew_sortkey1".to_string(),
                        CatalogNodeStats {
                            id: "skew_sortkey1".to_string(),
                            label: skew_sortkey1_label_i.to_string(),
                            value: decimal128_to_json_value(&skew_sortkey1_value, i),
                            description: Some(skew_sortkey1_description_i.to_string()),
                            include: skew_sortkey1_include_i,
                        },
                    );
                }

                if skew_rows_include_i {
                    stats.insert(
                        "skew_rows".to_string(),
                        CatalogNodeStats {
                            id: "skew_rows".to_string(),
                            label: skew_rows_label_i.to_string(),
                            value: decimal128_to_json_value(&skew_rows_value, i),
                            description: Some(skew_rows_description_i.to_string()),
                            include: skew_rows_include_i,
                        },
                    );
                }

                stats.insert(
                    "has_stats".to_string(),
                    CatalogNodeStats {
                        id: "has_stats".to_string(),
                        label: "has_stats".to_string(),
                        value: serde_json::Value::Bool(!stats.is_empty()),
                        description: Some(
                            "Indicates whether there are any statistics for this table".to_string(),
                        ),
                        include: false,
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
                result.insert(fully_qualified_name, node);
            }
        }

        Ok(result)
    }

    fn build_columns_from_get_columns(
        &self,
        catalog_sql_result: Arc<RecordBatch>,
    ) -> AdapterResult<BTreeMap<String, BTreeMap<String, ColumnMetadata>>> {
        if catalog_sql_result.num_rows() == 0 {
            return Ok(BTreeMap::new());
        }

        let table_catalogs = catalog_sql_result.column_values::<StringArray>("table_database")?;
        let table_schemas = catalog_sql_result.column_values::<StringArray>("table_schema")?;
        let table_names = catalog_sql_result.column_values::<StringArray>("table_name")?;

        let column_names = catalog_sql_result.column_values::<StringArray>("column_name")?;
        let column_indices = catalog_sql_result.column_values::<Int32Array>("column_index")?;
        let column_types = catalog_sql_result.column_values::<StringArray>("column_type")?;
        let column_comments = catalog_sql_result.column_values::<StringArray>("column_comment")?;

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
        relations: &[Arc<dyn BaseRelation>], // TODO: change to an Arc<Vec<..>>
        item_span_operation_id: Option<&str>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, HashMap<String, AdapterResult<Arc<Schema>>>> {
        let strategy = RedshiftListRelationsSchemasStrategy::new(self.adapter.clone());
        let relations = Arc::new(relations.to_vec());
        strategy.run(relations, unique_id, phase, item_span_operation_id, token)
    }

    fn list_relations_schemas_by_patterns_inner(
        &self,
        patterns: &[RelationPattern],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        let strategy = RedshiftListRelationsSchemasStrategy::new(self.adapter.clone());
        let patterns = Arc::new(patterns.to_vec());
        strategy.run_by_patterns(patterns, token)
    }

    fn create_schemas_if_not_exists(
        &self,
        state: &State<'_, '_>,
        catalog_schemas: Vec<(String, String, String)>,
    ) -> AdapterResult<Vec<(String, String, String, AdapterResult<()>)>> {
        create_schemas_if_not_exists(&self.adapter, self, state, catalog_schemas)
    }

    fn freshness_inner(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        let strategy = RedshiftFreshnessStrategy::new(self.adapter.clone());
        strategy.run(relations, token)
    }

    fn fetch_view_definitions_inner<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, ViewDefinitionFetchResult> {
        if relations.is_empty() {
            return Box::pin(async { Ok(ViewDefinitionFetchResult::default()) });
        }

        // Build a single WHERE clause covering all requested relations.
        // information_schema.views returns rows only for views; tables are silently absent.
        let mut conditions: Vec<String> = Vec::with_capacity(relations.len());
        for relation in relations {
            let schema = relation.schema_as_str().unwrap_or_default();
            let name = relation.identifier_as_str().unwrap_or_default();
            conditions.push(format!(
                "(upper(table_schema) = upper('{schema}') AND upper(table_name) = upper('{name}'))"
            ));
        }
        let sql = format!(
            "SELECT table_schema, table_name, view_definition \
             FROM information_schema.views \
             WHERE {}",
            conditions.join(" OR ")
        );

        let adapter = self.adapter.clone();
        let relations_owned: Vec<Arc<dyn BaseRelation>> = relations.to_vec();
        let token_clone = token.clone();

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        // Use MapReduce with a single item to get a managed connection.
        let map_f =
            move |conn: &'_ mut dyn Connection, _key: &()| -> AdapterResult<Arc<RecordBatch>> {
                let ctx = QueryCtx::default().with_desc("Fetch Redshift view definitions");
                let (_, table) = adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
                Ok(table.original_record_batch())
            };

        let reduce_f = move |acc: &mut ViewDefinitionFetchResult,
                             _key: (),
                             batch_res: AdapterResult<Arc<RecordBatch>>|
              -> Result<(), Cancellable<AdapterError>> {
            let batch = batch_res?;
            if batch.num_rows() == 0 {
                return Ok(());
            }

            let schemas = batch.column_values::<StringArray>("table_schema")?;
            let names = batch.column_values::<StringArray>("table_name")?;
            let defs = batch.column_values::<StringArray>("view_definition")?;

            for i in 0..batch.num_rows() {
                if defs.is_null(i) {
                    continue;
                }
                let row_schema = schemas.value(i).trim().to_lowercase();
                let row_name = names.value(i).trim().to_lowercase();
                let definition = defs.value(i).to_string();

                let Some(rel) = relations_owned.iter().find(|r| {
                    r.schema_as_str()
                        .map(|s| s.to_lowercase() == row_schema)
                        .unwrap_or(false)
                        && r.identifier_as_str()
                            .map(|n| n.to_lowercase() == row_name)
                            .unwrap_or(false)
                }) else {
                    continue;
                };

                acc.definitions.push(ViewDefinition {
                    fqn: rel.semantic_fqn(),
                    definition,
                    dialect: AdapterType::Redshift,
                    default_catalog: rel.database_as_str().unwrap_or_default().to_string(),
                    default_schema: rel.schema_as_str().unwrap_or_default().to_string(),
                });
            }

            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(vec![()]), token)
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
            acc.insert(db_schema, relations);
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(db_schemas.to_vec()), token)
    }

    /// Groups schemas by catalog and runs the pg_depend query once per catalog
    /// (not per schema) — the SQL is unfiltered, so per-schema runs would
    /// re-scan the same pg_catalog tables N times.
    fn fetch_relation_dependency_links_inner<'a>(
        &'a self,
        db_schemas: &'a [CatalogAndSchema],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, Vec<ParentChildPair>> {
        use std::collections::{BTreeMap, BTreeSet};

        let mut jobs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for s in db_schemas {
            jobs.entry(s.resolved_catalog.clone())
                .or_default()
                .insert(s.resolved_schema.to_lowercase());
        }
        let jobs: Vec<(String, BTreeSet<String>)> = jobs.into_iter().collect();

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));
        let adapter = self.adapter.clone();
        let token_clone = token.clone();

        let map_f = move |conn: &'_ mut dyn Connection,
                          job: &(String, BTreeSet<String>)|
              -> AdapterResult<Vec<ParentChildPair>> {
            let (catalog, schemas) = job;
            let ctx = QueryCtx::default().with_desc("redshift__get_relations");
            let engine = adapter.engine();
            let batch = engine.execute(
                None,
                conn,
                &ctx,
                REDSHIFT_GET_RELATIONS_SQL,
                token_clone.clone(),
            )?;
            parse_relation_dependencies(
                &batch,
                catalog,
                |schema| schemas.contains(&schema.to_lowercase()),
                engine.quoting(),
            )
        };

        let reduce_f = move |acc: &mut Vec<ParentChildPair>,
                             _job: (String, BTreeSet<String>),
                             res: AdapterResult<Vec<ParentChildPair>>|
              -> Result<(), Cancellable<AdapterError>> {
            match res {
                Ok(links) => acc.extend(links),
                Err(e) => {
                    // Best-effort: cascade just won't evict cross-relation
                    // dependents until the next hydration.
                    tracing::warn!("fetch_relation_dependency_links: pg_depend query failed: {e}");
                }
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(jobs), token)
    }
}

fn build_schema_from_stats_sql_without_stats(
    table_catalogs: GenericByteArray<GenericStringType<i32>>,
    table_schemas: GenericByteArray<GenericStringType<i32>>,
    table_names: GenericByteArray<GenericStringType<i32>>,
    data_types: GenericByteArray<GenericStringType<i32>>,
    comments: GenericByteArray<GenericStringType<i32>>,
    table_owners: GenericByteArray<GenericStringType<i32>>,
) -> AdapterResult<BTreeMap<String, CatalogTable>> {
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

            let node = CatalogTable {
                metadata: node_metadata,
                columns: IndexMap::new(),
                stats: BTreeMap::from([(
                    "has_stats".to_string(),
                    CatalogNodeStats {
                        id: "has_stats".to_string(),
                        label: "Has Stats?".to_string(),
                        value: serde_json::Value::Bool(false),
                        description: Some(
                            "Indicates whether there are statistics for this table".to_string(),
                        ),
                        include: false,
                    },
                )]),
                unique_id: None,
            };
            result.insert(fully_qualified_name, node);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::Schema as ArrowSchema;

    fn quoting() -> ResolvedQuoting {
        ResolvedQuoting::default()
    }

    fn show_tables_batch_with_subtype(rows: &[(&str, &str, &str, &str, &str)]) -> Arc<RecordBatch> {
        let database: Vec<&str> = rows.iter().map(|r| r.0).collect();
        let schema: Vec<&str> = rows.iter().map(|r| r.1).collect();
        let name: Vec<&str> = rows.iter().map(|r| r.2).collect();
        let typ: Vec<&str> = rows.iter().map(|r| r.3).collect();
        let subtype: Vec<&str> = rows.iter().map(|r| r.4).collect();

        let arrow_schema = Arc::new(ArrowSchema::new(vec![
            Field::new("database_name", DataType::Utf8, false),
            Field::new("schema_name", DataType::Utf8, false),
            Field::new("table_name", DataType::Utf8, false),
            Field::new("table_type", DataType::Utf8, false),
            Field::new("table_subtype", DataType::Utf8, false),
        ]));
        Arc::new(
            RecordBatch::try_new(
                arrow_schema,
                vec![
                    Arc::new(StringArray::from(database)),
                    Arc::new(StringArray::from(schema)),
                    Arc::new(StringArray::from(name)),
                    Arc::new(StringArray::from(typ)),
                    Arc::new(StringArray::from(subtype)),
                ],
            )
            .expect("show_tables_batch_with_subtype test fixture should build a valid RecordBatch"),
        )
    }

    fn show_tables_batch_without_subtype(rows: &[(&str, &str, &str, &str)]) -> Arc<RecordBatch> {
        let database: Vec<&str> = rows.iter().map(|r| r.0).collect();
        let schema: Vec<&str> = rows.iter().map(|r| r.1).collect();
        let name: Vec<&str> = rows.iter().map(|r| r.2).collect();
        let typ: Vec<&str> = rows.iter().map(|r| r.3).collect();

        let arrow_schema = Arc::new(ArrowSchema::new(vec![
            Field::new("database_name", DataType::Utf8, false),
            Field::new("schema_name", DataType::Utf8, false),
            Field::new("table_name", DataType::Utf8, false),
            Field::new("table_type", DataType::Utf8, false),
        ]));
        Arc::new(
            RecordBatch::try_new(
                arrow_schema,
                vec![
                    Arc::new(StringArray::from(database)),
                    Arc::new(StringArray::from(schema)),
                    Arc::new(StringArray::from(name)),
                    Arc::new(StringArray::from(typ)),
                ],
            )
            .unwrap(),
        )
    }

    #[test]
    fn test_parse_show_tables_mixed_types() {
        let batch = show_tables_batch_with_subtype(&[
            ("dev", "s1", "test_table", "TABLE", "REGULAR TABLE"),
            ("dev", "s1", "regular_view", "VIEW", "REGULAR VIEW"),
            (
                "dev",
                "s1",
                "late_binding_view",
                "VIEW",
                "LATE BINDING VIEW",
            ),
            ("dev", "s1", "manual_mv", "VIEW", "MATERIALIZED VIEW"),
        ]);

        let relations = parse_show_tables_batch(&batch, quoting()).unwrap();
        assert_eq!(relations.len(), 4);
        assert_eq!(relations[0].relation_type(), Some(RelationType::Table));
        assert_eq!(relations[1].relation_type(), Some(RelationType::View));
        assert_eq!(relations[2].relation_type(), Some(RelationType::View));
        assert_eq!(
            relations[3].relation_type(),
            Some(RelationType::MaterializedView)
        );
        assert_eq!(relations[0].identifier_as_str().unwrap(), "test_table");
        assert_eq!(relations[3].identifier_as_str().unwrap(), "manual_mv");
    }

    #[test]
    fn test_parse_show_tables_empty_batch() {
        let batch = show_tables_batch_with_subtype(&[]);
        let relations = parse_show_tables_batch(&batch, quoting()).unwrap();
        assert!(relations.is_empty());
    }

    #[test]
    fn test_parse_show_tables_missing_subtype_treats_views_as_view() {
        // Pre-Patch-197 Redshift omits the table_subtype column.
        // Without it, every VIEW must be treated as a plain view.
        let batch = show_tables_batch_without_subtype(&[
            ("dev", "s1", "t1", "TABLE"),
            ("dev", "s1", "v1", "VIEW"),
            ("dev", "s1", "v2", "VIEW"),
        ]);

        let relations = parse_show_tables_batch(&batch, quoting()).unwrap();
        assert_eq!(relations.len(), 3);
        assert_eq!(relations[0].relation_type(), Some(RelationType::Table));
        assert_eq!(relations[1].relation_type(), Some(RelationType::View));
        assert_eq!(relations[2].relation_type(), Some(RelationType::View));
    }

    #[test]
    fn test_parse_show_tables_handles_lowercase_and_padding() {
        let batch = show_tables_batch_with_subtype(&[
            ("dev", "s1", "padded", " view ", " materialized view "),
            ("dev", "s1", "lower", "view", "regular view"),
        ]);

        let relations = parse_show_tables_batch(&batch, quoting()).unwrap();
        assert_eq!(
            relations[0].relation_type(),
            Some(RelationType::MaterializedView)
        );
        assert_eq!(relations[1].relation_type(), Some(RelationType::View));
    }

    fn show_tables_freshness_batch(rows: &[(&str, &str, &str, Option<i64>)]) -> Arc<RecordBatch> {
        let schema_col: Vec<&str> = rows.iter().map(|r| r.0).collect();
        let table_col: Vec<&str> = rows.iter().map(|r| r.1).collect();
        let type_col: Vec<&str> = rows.iter().map(|r| r.2).collect();
        let lm_col: Vec<Option<i64>> = rows.iter().map(|r| r.3).collect();

        let arrow_schema = Arc::new(ArrowSchema::new(vec![
            Field::new("schema_name", DataType::Utf8, false),
            Field::new("table_name", DataType::Utf8, false),
            Field::new("table_type", DataType::Utf8, false),
            Field::new(
                "last_modified_time",
                DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
                true,
            ),
        ]));
        Arc::new(
            RecordBatch::try_new(
                arrow_schema,
                vec![
                    Arc::new(StringArray::from(schema_col)),
                    Arc::new(StringArray::from(table_col)),
                    Arc::new(StringArray::from(type_col)),
                    Arc::new(TimestampMicrosecondArray::from(lm_col)),
                ],
            )
            .unwrap(),
        )
    }

    fn relation(database: &str, schema: &str, identifier: &str) -> Arc<dyn BaseRelation> {
        build_redshift_relation(
            database.to_string(),
            schema.to_string(),
            identifier.to_string(),
            RelationType::Table,
            quoting(),
        )
        .expect("relation test fixture should build a valid Redshift relation")
    }

    #[test]
    fn test_parse_show_tables_freshness_matches_requested_identifiers_only() {
        // Two requested sources, plus a third unrelated table in the same
        // schema that SHOW TABLES will also return — that one must be ignored.
        let relations = vec![
            relation("dev", "s1", "orders"),
            relation("dev", "s1", "customers"),
        ];
        let batch = show_tables_freshness_batch(&[
            ("s1", "orders", "TABLE", Some(1_700_000_000_000_000)),
            ("s1", "customers", "VIEW", Some(1_700_000_001_000_000)),
            ("s1", "unrelated", "TABLE", Some(1_700_000_002_000_000)),
        ]);

        let freshness = parse_show_tables_freshness_batch(&batch, &relations).unwrap();
        assert_eq!(freshness.len(), 2);
        let orders = &freshness[&relations[0].semantic_fqn()];
        assert_eq!(
            orders.last_altered.timestamp_micros(),
            1_700_000_000_000_000
        );
        assert!(!orders.is_view);
        let customers = &freshness[&relations[1].semantic_fqn()];
        assert_eq!(
            customers.last_altered.timestamp_micros(),
            1_700_000_001_000_000
        );
        assert!(customers.is_view);
    }

    #[test]
    fn test_parse_show_tables_freshness_skips_null_last_modified() {
        let relations = vec![relation("dev", "s1", "orders")];
        let batch = show_tables_freshness_batch(&[("s1", "orders", "TABLE", None)]);

        let freshness = parse_show_tables_freshness_batch(&batch, &relations).unwrap();
        assert!(freshness.is_empty());
    }

    /// (database, schema, table, type, subtype, owner, remarks)
    fn show_tables_batch_full(
        rows: &[(&str, &str, &str, &str, &str, &str, &str)],
    ) -> Arc<RecordBatch> {
        let arrow_schema = Arc::new(ArrowSchema::new(vec![
            Field::new("database_name", DataType::Utf8, false),
            Field::new("schema_name", DataType::Utf8, false),
            Field::new("table_name", DataType::Utf8, false),
            Field::new("table_type", DataType::Utf8, false),
            Field::new("table_subtype", DataType::Utf8, false),
            Field::new("owner", DataType::Utf8, false),
            Field::new("remarks", DataType::Utf8, false),
        ]));
        Arc::new(
            RecordBatch::try_new(
                arrow_schema,
                vec![
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.0))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.1))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.2))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.3))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.4))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.5))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.6))),
                ],
            )
            .expect("show_tables_batch_full test fixture should build a valid RecordBatch"),
        )
    }

    /// (database, schema, table, column, ordinal_position, data_type, remarks)
    fn svv_columns_batch(rows: &[(&str, &str, &str, &str, i32, &str, &str)]) -> RecordBatch {
        let arrow_schema = Arc::new(ArrowSchema::new(vec![
            Field::new("database_name", DataType::Utf8, false),
            Field::new("schema_name", DataType::Utf8, false),
            Field::new("table_name", DataType::Utf8, false),
            Field::new("column_name", DataType::Utf8, false),
            Field::new("ordinal_position", DataType::Int32, false),
            Field::new("data_type", DataType::Utf8, false),
            Field::new("remarks", DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            arrow_schema,
            vec![
                Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.0))),
                Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.1))),
                Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.2))),
                Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.3))),
                Arc::new(Int32Array::from_iter_values(rows.iter().map(|r| r.4))),
                Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.5))),
                Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.6))),
            ],
        )
        .expect("svv_columns_batch test fixture should build a valid RecordBatch")
    }

    #[test]
    fn test_join_show_tables_and_svv_columns() {
        // Two relations in schema `s1`; SVV reports columns for both plus one row for a
        // table that has no SHOW TABLES parent (which must be dropped from the catalog).
        let show_tables = vec![show_tables_batch_full(&[
            ("dev", "s1", "orders", "TABLE", "", "alice", "orders table"),
            (
                "dev",
                "s1",
                "cust_v",
                "VIEW",
                "MATERIALIZED VIEW",
                "bob",
                "",
            ),
        ])];
        let svv = svv_columns_batch(&[
            // Case-insensitive join: SVV reports uppercase schema/table for `orders`.
            ("dev", "S1", "ORDERS", "id", 1, "integer", "pk"),
            ("dev", "S1", "ORDERS", "amount", 2, "numeric", ""),
            ("dev", "s1", "cust_v", "name", 1, "character varying", ""),
            ("dev", "s1", "missing", "x", 1, "integer", ""),
        ]);

        let batch = join_show_tables_and_svv_columns(&show_tables, &svv).unwrap();

        // 3 matched columns; the orphan `missing` row is dropped.
        assert_eq!(batch.num_rows(), 3);

        // All 10 contract columns present, with the types the catalog parsers expect.
        let schema = batch.schema();
        let dtype = |n: &str| schema.field_with_name(n).unwrap().data_type().clone();
        for col in [
            "table_database",
            "table_schema",
            "table_name",
            "table_type",
            "table_comment",
            "table_owner",
            "column_name",
            "column_type",
            "column_comment",
        ] {
            assert_eq!(dtype(col), DataType::Utf8, "{col} should be Utf8");
        }
        assert_eq!(dtype("column_index"), DataType::Int32);

        let table_type = batch.column_values::<StringArray>("table_type").unwrap();
        let table_owner = batch.column_values::<StringArray>("table_owner").unwrap();
        let table_comment = batch.column_values::<StringArray>("table_comment").unwrap();
        let column_index = batch.column_values::<Int32Array>("column_index").unwrap();
        let column_comment = batch
            .column_values::<StringArray>("column_comment")
            .unwrap();

        // Row 0: orders.id — base table, owner + comments populated, index preserved.
        assert_eq!(table_type.value(0), "BASE TABLE");
        assert_eq!(table_owner.value(0), "alice");
        assert_eq!(table_comment.value(0), "orders table");
        assert_eq!(column_index.value(0), 1);
        assert_eq!(column_comment.value(0), "pk");

        // Row 2: cust_v.name — the subtype (MATERIALIZED VIEW) wins over the type (VIEW).
        assert_eq!(table_type.value(2), "MATERIALIZED VIEW");
        assert_eq!(table_owner.value(2), "bob");
    }

    #[test]
    fn test_build_catalog_empty_inputs() {
        let batch = join_show_tables_and_svv_columns(&[], &svv_columns_batch(&[])).unwrap();
        assert_eq!(batch.num_rows(), 0);
        // Schema stays well-formed so the downstream parsers short-circuit on 0 rows.
        assert_eq!(batch.schema().fields().len(), 10);
    }
}
