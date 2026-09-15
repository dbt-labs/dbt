use crate::adapter::adapter_impl::*;
use crate::connection::AdapterConnectionFactory;
use crate::metadata::FreshnessOverride;
use crate::metadata::freshness_overrides::{
    FreshnessTask, FreshnessTaskResult, apply_freshness_task_result, freshness_override_sql,
    run_override_sql,
};
use crate::metadata::{CatalogAndSchema, *};
use crate::record_batch::RecordBatchExt;
use crate::relation::Relation;
use crate::sql_types::{TypeOps, make_arrow_field};
use crate::time_machine::{
    args_freshness, args_freshness_with_overrides, args_relations_exist,
    with_time_machine_metadata_wrapper,
};
use crate::{AdapterEngine, AdapterResult, AdapterType};
use dbt_adapter_sql::ident::{escape_string_literal, quote_identifier};

use arrow_array::{
    Array, BooleanArray, Decimal128Array, RecordBatch, StringArray, TimestampMillisecondArray,
};
use arrow_schema::Schema;
use dbt_adapter_core::ExecutionPhase;
use dbt_adapter_engine::{ConnectionFactory, MapReduce};
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::AsyncAdapterResult;
use dbt_common::ErrorCode;
use dbt_common::cancellation::Cancellable;
use dbt_common::cancellation::CancellationToken;
use dbt_common::tracing::dbt_emit::{emit_debug_log_message, emit_warn_log_message};
use dbt_frontend_common::Dialect;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::legacy_catalog::*;
use dbt_schemas::schemas::relations::base::*;
use futures::StreamExt;
use indexmap::IndexMap;
use minijinja::State;
use once_cell::sync::Lazy;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Display;
use std::sync::Arc;

const SNOWFLAKE_METADATA_NODE_ID: &str = "snowflake-metadata";

fn metadata_warehouse_error(err: impl Display) -> AdapterError {
    AdapterError::new(AdapterErrorKind::Configuration, err.to_string())
}

/// Wraps a [ConnectionFactory] so a dedicated metadata warehouse is switched
/// to once per *physical connection* rather than once per query.
///
/// `MapReduce` workers reuse a single connection across many tasks in a batch
/// before handing it back, so switching the warehouse inside the per-task closure
/// re-issues `use warehouse` before and after every task on a reused
/// connection. Wrapping the factory instead moves
/// the switch to `new_connection` (once, when the connection is first obtained
/// for the batch) and the restore to `recycle_connection` (once, right before
/// the connection is handed back).
type UseWarehouseFn = Box<dyn Fn(&mut dyn Connection, &str) -> AdapterResult<()> + Send + Sync>;
type RestoreWarehouseFn = Box<dyn Fn(&mut dyn Connection) -> AdapterResult<()> + Send + Sync>;

struct MetadataWarehouseConnectionFactory {
    metadata_warehouse: Option<String>,
    use_warehouse: UseWarehouseFn,
    restore_warehouse: RestoreWarehouseFn,
    inner: Box<dyn ConnectionFactory<Error = Cancellable<AdapterError>>>,
}

impl MetadataWarehouseConnectionFactory {
    fn new(
        adapter: AdapterImpl,
        metadata_warehouse: Option<String>,
        token: CancellationToken,
        inner: Box<dyn ConnectionFactory<Error = Cancellable<AdapterError>>>,
    ) -> Self {
        let use_warehouse = {
            let adapter = adapter.clone();
            let token = token.clone();
            Box::new(move |conn: &mut dyn Connection, warehouse: &str| {
                adapter
                    .use_warehouse(
                        conn,
                        warehouse.to_string(),
                        SNOWFLAKE_METADATA_NODE_ID,
                        token.clone(),
                    )
                    .map(|_| ())
                    .map_err(metadata_warehouse_error)
            }) as UseWarehouseFn
        };
        let restore_warehouse = Box::new(move |conn: &mut dyn Connection| {
            adapter
                .restore_warehouse(conn, SNOWFLAKE_METADATA_NODE_ID, token.clone())
                .map_err(metadata_warehouse_error)
        }) as RestoreWarehouseFn;
        Self::from_hooks(metadata_warehouse, use_warehouse, restore_warehouse, inner)
    }

    fn from_hooks(
        metadata_warehouse: Option<String>,
        use_warehouse: UseWarehouseFn,
        restore_warehouse: RestoreWarehouseFn,
        inner: Box<dyn ConnectionFactory<Error = Cancellable<AdapterError>>>,
    ) -> Self {
        Self {
            metadata_warehouse,
            use_warehouse,
            restore_warehouse,
            inner,
        }
    }

    fn active_warehouse(&self) -> Option<&str> {
        self.metadata_warehouse
            .as_deref()
            .filter(|warehouse| !warehouse.is_empty())
    }
}

impl ConnectionFactory for MetadataWarehouseConnectionFactory {
    type Error = Cancellable<AdapterError>;

    fn new_connection(&self, node_id: Option<&str>) -> Result<Box<dyn Connection>, Self::Error> {
        let mut conn = self.inner.new_connection(node_id)?;
        if let Some(warehouse) = self.active_warehouse() {
            (self.use_warehouse)(conn.as_mut(), warehouse).map_err(Cancellable::Error)?;
        }
        Ok(conn)
    }

    fn recycle_connection(&self, mut conn: Box<dyn Connection>) {
        // These connections go back into the worker thread's slot, where the
        // next node to run there picks them up. A failed restore leaves the
        // connection stuck on the metadata warehouse, so drop it instead —
        // otherwise an unrelated node could silently inherit the metadata
        // warehouse. Mirrors `reset_node_overrides` in
        // dbt-tasks-sa/src/materialize.rs.
        if self.active_warehouse().is_some() {
            if let Err(e) = (self.restore_warehouse)(conn.as_mut()) {
                tracing::warn!(
                    "failed to restore warehouse before recycling Snowflake metadata connection, dropping it instead: {e}"
                );
                return;
            }
        }
        self.inner.recycle_connection(conn);
    }
}

fn require_snowflake_metadata_component<'a>(
    component: &'static str,
    value: &'a str,
) -> AdapterResult<&'a str> {
    let value = value.trim();
    if !value.is_empty() {
        return Ok(value);
    }

    Err(AdapterError::new(
        AdapterErrorKind::Configuration,
        format!(
            "Snowflake metadata query requires a non-empty {component}. \
             Configure a {component} value on the Snowflake target, model, or source."
        ),
    ))
}

/// Render a schema name as a single-quoted SQL string literal, escaping embedded
/// single quotes so a schema containing `'` stays well-formed.
fn snowflake_schema_literal(schema: &str) -> String {
    format!(
        "'{}'",
        escape_string_literal(schema, AdapterType::Snowflake)
    )
}

/// Build the constant-cost schema-count probe for a database. The database is
/// rendered as a quoted identifier (embedded quotes escaped) so a name
/// containing `"` stays a single well-formed identifier.
fn snowflake_schema_count_sql(database: &str, limit: usize) -> String {
    format!(
        "SHOW TERSE SCHEMAS IN DATABASE {} LIMIT {limit}",
        quote_identifier(database, AdapterType::Snowflake)
    )
}

/// Concurrency to use for the per-schema freshness fan-out when a dedicated
/// metadata warehouse is configured but the engine does not expose a thread
/// count (e.g. mock / sidecar engines). Keeps the fan-out bounded so a project
/// with many schemas does not open an unbounded number of connections.
const DEFAULT_SCHEMA_PREFETCH_FANOUT: usize = 4;

// When prefetching last-modified metadata across several schemas we can either issue one broad
// `table_schema IN (...)` scan or one pruned `table_schema = 'S'` point query per schema. A
// `table_schema IN (...)` predicate loses single-schema pruning and forces a full scan of the whole
// database, so its cost grows with the DB's *schema count* (not the
// number of schemas we asked for), while each single-eq point query prunes and stays flat regardless
// of DB size. The two measured curves are:
//   - POINT_QUERY_SECONDS: one pruned `table_schema = 'S'` query is ~0.71s, flat at any DB size.
//   - IN_SCAN_SECONDS_PER_1000_SCHEMAS: the broad `IN (...)` full scan grows ~2.5s per 1,000 schemas
//     present in the database.
// Fetching D schemas one-by-one costs ~D * POINT_QUERY_SECONDS, so it beats the single `IN` scan once
// the database holds more than ~(POINT_QUERY_SECONDS / IN_SCAN_SECONDS_PER_SCHEMA) schemas per fetched
// schema. That per-fetched-schema crossover is CROSSOVER_N_PER_FETCHED_SCHEMA (~284); the probe simply
// asks "does this database have at least CROSSOVER_N_PER_FETCHED_SCHEMA * D schemas?".
const POINT_QUERY_SECONDS: f64 = 0.71; // measured: one `table_schema = 'S'` query, flat
const IN_SCAN_SECONDS_PER_1000_SCHEMAS: f64 = 2.5; // measured: broad `IN (...)` scan slope, per 1,000 schemas
const IN_SCAN_SECONDS_PER_SCHEMA: f64 = IN_SCAN_SECONDS_PER_1000_SCHEMAS / 1000.0;
const CROSSOVER_N_PER_FETCHED_SCHEMA: usize =
    (POINT_QUERY_SECONDS / IN_SCAN_SECONDS_PER_SCHEMA + 0.5) as usize; // 284
const MAX_SHOW_LIMIT: usize = 10000; // Snowflake hard cap on SHOW ... LIMIT
// At or below this many fetched schemas, D point queries cost at most ~D * POINT_QUERY_SECONDS (a
// few seconds) — cheap and bounded — so we always fetch sequentially and skip the schema-count probe
// entirely, sidestepping both the probe round-trip and any risk of a broad IN scan on a large DB.
const ALWAYS_SEQUENTIAL_MAX_SCHEMAS: usize = 4;

fn schema_probe_limit(num_schemas: usize) -> usize {
    (CROSSOVER_N_PER_FETCHED_SCHEMA * num_schemas).min(MAX_SHOW_LIMIT)
}

/// Pure strategy decision from a probe result, split out from
/// `should_fetch_schemas_sequentially` so the edge cases are unit-testable
/// without a live adapter. `observed` is the row count the
/// `SHOW TERSE SCHEMAS ... LIMIT T` probe returned, or `None` if the probe
/// failed. Returns `true` to fetch per-schema sequentially, `false` to use one
/// broad `IN (...)` scan.
///
/// Cap behavior: when `CROSSOVER_N_PER_FETCHED_SCHEMA * num_schemas` exceeds
/// `MAX_SHOW_LIMIT` (num_schemas above ~35), `threshold` is clamped to
/// `MAX_SHOW_LIMIT`, so a saturated probe only proves the database has
/// `>= MAX_SHOW_LIMIT` schemas — not the full
/// `CROSSOVER_N_PER_FETCHED_SCHEMA * num_schemas` crossover. We deliberately
/// still choose sequential there: the database is very large and its true schema
/// count is unknown, so the broad IN scan's cost grows without bound in that
/// count while sequential stays bounded at `~POINT_QUERY_SECONDS * num_schemas`.
/// In the narrow band where N sits between `MAX_SHOW_LIMIT` and the true
/// crossover this can pick sequential when a scan would have been marginally
/// cheaper, but that overpay is bounded and small next to the unbounded cost of
/// scanning a genuinely huge database.
fn schema_probe_decision(num_schemas: usize, observed: Option<usize>) -> bool {
    if num_schemas <= ALWAYS_SEQUENTIAL_MAX_SCHEMAS {
        return true;
    }
    match observed {
        // Probe failed -> fall back to the broad IN scan.
        None => false,
        Some(observed) => observed == schema_probe_limit(num_schemas),
    }
}

fn snowflake_freshness_sql(database: &str, where_clauses: &[String]) -> AdapterResult<String> {
    let database = require_snowflake_metadata_component("database", database)?;
    Ok(format!(
        "SELECT
                table_schema,
                table_name,
                last_altered,
                (table_type = 'VIEW' OR table_type = 'MATERIALIZED VIEW') AS is_view
             FROM {}.INFORMATION_SCHEMA.TABLES
             WHERE {}",
        database,
        where_clauses.join(" OR ")
    ))
}

fn is_table_ddl(ddl: &str) -> bool {
    static TABLE_REGEX: Lazy<fancy_regex::Regex> = Lazy::new(|| {
        fancy_regex::Regex::new(
            r"(?ix)
                ^\s*create\b
                (?:\s+(?!table\b)\w+)*
                \s+table\b
                ",
        )
        .expect("valid regex")
    });

    if ddl.trim().is_empty() {
        return false;
    }
    // `is_match` returns Result because fancy-regex's backtracking engine
    // can fail on pathological inputs; treat any engine error as "not a
    // table" so a malformed DDL doesn't get cached as a view by mistake.
    TABLE_REGEX.is_match(ddl).unwrap_or(false)
}

/// Render the anonymous block (the body that goes inside `EXECUTE IMMEDIATE $$...$$`)
/// that calls `GET_DDL` over a list of FQNs and captures per-object errors as
/// part of the result set.
///
/// The caller is responsible for wrapping the returned string in
/// `EXECUTE IMMEDIATE $$ ... $$`. The rendered block accepts no parameters
/// and returns a result set with columns (fqn, view_definition, error).
fn build_view_definition_script(fqns: &[String]) -> String {
    let array_literals = fqns
        .iter()
        .map(|fqn| format!("'{}'", fqn.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        r#"
begin
    let objects array := array_construct(
        {array_literals}
    );

    let i integer := 0;
    let results array := array_construct();

    while (i < array_size(objects)) do
        let obj_name string := objects[i]::string;

        begin
            let ddl_text string := (select get_ddl('VIEW', :obj_name));

            results := array_append(results, object_construct(
                'OBJECT_NAME', :obj_name,
                'DEFINITION', :ddl_text,
                'ERROR', null
            ));

        exception
            when other then
                results := array_append(results, object_construct(
                    'OBJECT_NAME', :obj_name,
                    'DEFINITION', null,
                    'ERROR', :sqlerrm
                ));
        end;

        i := i + 1;
    end while;

    let rs resultset := (
        select
            f.value['OBJECT_NAME']::string as fqn,
            f.value['DEFINITION']::string as view_definition,
            f.value['ERROR']::string as error
        from table(flatten(input => :results)) f
        order by 1
    );

    return table(rs);
end;"#
    )
}

pub const ARROW_FIELD_SNOWFLAKE_FIELD_WIDTH_METADATA_KEY: &str = "SNOWFLAKE:field_width";

/// Normalize all column names in a RecordBatch to lowercase.
///
/// Snowflake may uppercase column aliases (e.g. `table_catalog as "table_database"`) depending
/// on account-level settings, even when the alias is double-quoted. Lowercasing the schema up
/// front lets all downstream `get_column_values` calls use their expected lowercase names without
/// needing per-call case-insensitive logic.
fn lowercase_column_names(batch: &RecordBatch) -> RecordBatch {
    let schema = batch.schema();
    let fields: Vec<_> = schema
        .fields()
        .iter()
        .map(|f| Arc::new(f.as_ref().clone().with_name(f.name().to_lowercase())))
        .collect();
    let new_schema = Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone()));
    RecordBatch::try_new(new_schema, batch.columns().to_vec())
        .expect("column name normalization preserves schema compatibility")
}

fn accumulate_view_definition_fetch_result(
    acc: &mut ViewDefinitionFetchResult,
    batch: &RecordBatch,
) -> AdapterResult<()> {
    let batch = lowercase_column_names(batch);
    // Result schema: (fqn STRING, view_definition STRING, error STRING)
    let fqns_arr = batch.column_values::<StringArray>("fqn")?;
    let defs_arr = batch.column_values::<StringArray>("view_definition")?;

    for i in 0..batch.num_rows() {
        let fqn = fqns_arr.value(i).to_string();
        if defs_arr.is_null(i) {
            // A NULL definition means Snowflake could not return DDL for
            // this relation. Treat it as unresolvable regardless of the
            // exact GET_DDL error text so the query cache can fall back
            // to freshness metadata for secure/data-share views.
            acc.unresolvable.insert(fqn);
            continue;
        }
        let definition = defs_arr.value(i);

        if is_table_ddl(definition) {
            continue;
        }

        let parsed = match Dialect::Snowflake.parse_fqn(&fqn) {
            Ok(p) => p,
            Err(_) => continue, // unparseable — skip
        };

        acc.definitions.push(ViewDefinition {
            fqn,
            definition: definition.to_string(),
            dialect: AdapterType::Snowflake,
            default_catalog: parsed.catalog().name().to_string(),
            default_schema: parsed.schema().name().to_string(),
        });
    }

    Ok(())
}

/// TODO: When we implement iceberg tables, we might want to pass in the is_iceberg flag here.
///
/// `is_interactive` must be checked before `is_dynamic`: a dynamic interactive table reports
/// both `is_dynamic='Y'` and `is_interactive='Y'`, so checking `is_dynamic` first would
/// misclassify it as a plain dynamic table.
///
/// Callers pass `is_interactive` tolerantly defaulted to `"n"`: the column is absent on
/// accounts and versions without interactive table support, and its cell can be NULL or empty
/// even when the column is present, neither of which may fail the surrounding query.
pub fn relation_type_from_table_flags(
    is_dynamic: &str,
    is_interactive: &str,
) -> Result<RelationType, AdapterError> {
    if try_canonicalize_bool_column_field(is_interactive)? {
        Ok(RelationType::InteractiveTable)
    } else if is_dynamic.eq_ignore_ascii_case("y") {
        Ok(RelationType::DynamicTable)
    } else if is_dynamic.eq_ignore_ascii_case("n") {
        Ok(RelationType::Table)
    } else {
        Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!("Unexpected `is_dynamic` value {is_dynamic}"),
        ))
    }
}

// Helper for serializing query results within `list_relations`
fn build_relations_from_show_objects(
    show_objects_result: &RecordBatch,
    quoting: ResolvedQuoting,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let mut relations = Vec::new();

    let name = show_objects_result.column_values::<StringArray>("name")?;
    let database_name = show_objects_result.column_values::<StringArray>("database_name")?;
    let schema_name = show_objects_result.column_values::<StringArray>("schema_name")?;
    let table_kind = show_objects_result.column_values::<StringArray>("kind")?;
    let is_dynamic = show_objects_result.column_values::<StringArray>("is_dynamic")?;
    let is_iceberg = show_objects_result.column_values::<StringArray>("is_iceberg")?;
    // See `relation_type_from_table_flags`'s doc.
    let is_interactive = show_objects_result
        .column_values::<StringArray>("is_interactive")
        .ok();

    for i in 0..show_objects_result.num_rows() {
        let name = name.value(i);
        let database_name = database_name.value(i);
        let schema_name = schema_name.value(i);
        let table_kind = table_kind.value(i);
        let is_dynamic = is_dynamic.value(i);
        let is_iceberg = is_iceberg.value(i);
        let is_interactive = is_interactive
            .as_ref()
            .map(|col| col.value(i))
            .filter(|value| !value.is_empty())
            .unwrap_or("n");

        let relation_type = if table_kind.eq_ignore_ascii_case("table") {
            Some(relation_type_from_table_flags(is_dynamic, is_interactive)?)
        } else if table_kind.eq_ignore_ascii_case("view") {
            Some(RelationType::View)
        } else if table_kind.eq_ignore_ascii_case("temporary") {
            // Session-scoped temporary tables/views behave like regular tables for
            // all supported dbt operations (SELECT *, DROP TABLE, etc.).
            Some(RelationType::Table)
        } else {
            Some(RelationType::from(table_kind))
        };

        let table_format = if try_canonicalize_bool_column_field(is_iceberg)? {
            TableFormat::Iceberg
        } else {
            TableFormat::Default
        };

        let relation = Relation::new(
            AdapterType::Snowflake,
            database_name.to_string(),
            schema_name.to_string(),
            name.to_string(),
        )
        .with_relation_type(relation_type)
        .with_quoting(quoting)
        .with_table_format(table_format);
        relations.push(Arc::new(relation) as Arc<dyn BaseRelation>);
    }

    Ok(relations)
}

pub fn list_relations(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    // Paginate through the results
    let limit_size = 10000;
    let mut from_name = None;
    let mut batches = Vec::new();
    loop {
        let sql = format!(
            "SHOW OBJECTS IN SCHEMA {} LIMIT {}{}",
            db_schema,
            limit_size,
            from_name
                .map(|name| format!(" FROM '{name}'"))
                .unwrap_or_default()
        );
        let batch = engine.execute(None, conn, ctx, &sql, token.clone())?;

        // From the RecordBatch, get the last row of the vector of name 'name'
        let names = batch.column_values::<StringArray>("name")?;

        let last_name = match batch.num_rows().checked_sub(1) {
            Some(idx) => names.value(idx).to_string(),
            None => break,
        };

        from_name = Some(last_name);
        batches.push(batch);
        if names.len() < limit_size {
            break;
        }
    }
    // Create Relations from the batches
    let mut relations = Vec::new();
    for batch in batches {
        relations.extend(build_relations_from_show_objects(
            &batch,
            ResolvedQuoting::trues(),
        )?);
    }
    Ok(relations)
}

pub struct SnowflakeMetadataAdapter {
    pub adapter: AdapterImpl,
}

impl SnowflakeMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }

    fn freshness_inner_with_options(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        options: &MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, BTreeMap<String, MetadataFreshness>> {
        // Build the where clause for all relations grouped by databases
        let (where_clauses_by_database, relations_by_database) =
            match build_relation_clauses(relations) {
                Ok(result) => result,
                Err(e) => {
                    let future = async move { Err(Cancellable::Error(e)) };
                    return Box::pin(future);
                }
            };

        type Acc = BTreeMap<String, MetadataFreshness>;

        let metadata_warehouse = options.warehouse.clone();
        let factory = Box::new(MetadataWarehouseConnectionFactory::new(
            self.adapter.clone(),
            metadata_warehouse,
            token.clone(),
            Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone())),
        ));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          database_and_where_clauses: &(String, Vec<String>)|
              -> AdapterResult<Arc<RecordBatch>> {
            let (database, where_clauses) = &database_and_where_clauses;
            let sql = snowflake_freshness_sql(database, where_clauses)?;

            let ctx = QueryCtx::default().with_desc("Extracting freshness from information schema");
            let (_adapter_response, agate_table) =
                adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            let batch = agate_table.original_record_batch();
            Ok(batch)
        };

        let reduce_f = move |acc: &mut Acc,
                             database_and_where_clauses: (String, Vec<String>),
                             batch_res: AdapterResult<Arc<RecordBatch>>|
              -> Result<(), Cancellable<AdapterError>> {
            let Ok(batch) = batch_res else {
                // Keep successful database batches; missing relations fall back downstream.
                return Ok(());
            };
            let schemas = batch.column_values::<StringArray>("TABLE_SCHEMA")?;
            let tables = batch.column_values::<StringArray>("TABLE_NAME")?;
            let timestamps = batch.column_values::<TimestampMillisecondArray>("LAST_ALTERED")?;
            let is_views = batch.column_values::<BooleanArray>("IS_VIEW")?;

            let (database, _where_clauses) = &database_and_where_clauses;
            for i in 0..batch.num_rows() {
                let schema = schemas.value(i);
                let table = tables.value(i);
                let timestamp = timestamps.value(i);
                let relations = &relations_by_database[database];
                let is_view = is_views.value(i);

                for table_name in find_matching_relation(schema, table, relations)? {
                    acc.insert(
                        table_name,
                        MetadataFreshness::from_millis(timestamp, is_view)?,
                    );
                }
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        let keys = where_clauses_by_database.into_iter().collect::<Vec<_>>();
        map_reduce.run(Arc::new(keys), token)
    }

    fn freshness_with_overrides_inner_with_options(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        overrides: &BTreeMap<String, FreshnessOverride>,
        options: &MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, BTreeMap<String, MetadataFreshness>> {
        if overrides.is_empty() {
            return self.freshness_inner_with_options(relations, options, token);
        }

        // Partition relations: those with overrides run their own targeted query;
        // the rest go through the existing bulk INFORMATION_SCHEMA path.
        let mut override_targets = Vec::new();
        let mut bulk_relations = Vec::new();
        for relation in relations {
            if let Some(ovr) = overrides.get(&relation.semantic_fqn()) {
                override_targets.push((Arc::clone(relation), ovr.clone()));
            } else {
                bulk_relations.push(Arc::clone(relation));
            }
        }

        let engine = self.adapter.engine().clone();

        // Run the bulk and per-override queries through one MapReduce pass so
        // they share the same connection-factory threadpool — same parallelism
        // model as the plugin.
        let metadata_warehouse = options.warehouse.clone();
        let factory = Box::new(MetadataWarehouseConnectionFactory::new(
            self.adapter.clone(),
            metadata_warehouse,
            token.clone(),
            Box::new(AdapterConnectionFactory::new(engine)),
        ));
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
        let map_f = move |conn: &'_ mut dyn Connection,
                          task: &FreshnessTask|
              -> AdapterResult<FreshnessTaskResult> {
            match task {
                FreshnessTask::Bulk(bulk) => {
                    let (where_clauses_by_database, relations_by_database) =
                        build_relation_clauses(bulk)?;
                    let mut acc: Acc = BTreeMap::new();
                    for (database, where_clauses) in where_clauses_by_database {
                        let sql = snowflake_freshness_sql(&database, &where_clauses)?;
                        let ctx = QueryCtx::default()
                            .with_desc("Extracting freshness from information schema");
                        let Ok((_resp, agate_table)) =
                            adapter_for_map.query(&ctx, conn, &sql, None, token_clone.clone())
                        else {
                            // Keep successful database batches; missing relations fall back downstream.
                            continue;
                        };
                        let batch = agate_table.original_record_batch();
                        let schemas = batch.column_values::<StringArray>("TABLE_SCHEMA")?;
                        let tables = batch.column_values::<StringArray>("TABLE_NAME")?;
                        let timestamps =
                            batch.column_values::<TimestampMillisecondArray>("LAST_ALTERED")?;
                        let is_views = batch.column_values::<BooleanArray>("IS_VIEW")?;
                        let relations = &relations_by_database[&database];
                        for i in 0..batch.num_rows() {
                            let schema = schemas.value(i);
                            let table = tables.value(i);
                            let timestamp = timestamps.value(i);
                            let is_view = is_views.value(i);
                            for table_name in find_matching_relation(schema, table, relations)? {
                                acc.insert(
                                    table_name,
                                    MetadataFreshness::from_millis(timestamp, is_view)?,
                                );
                            }
                        }
                    }
                    Ok(FreshnessTaskResult::Bulk(acc))
                }
                FreshnessTask::Override(relation, ovr) => {
                    let semantic_fqn = relation.semantic_fqn();
                    let sql = freshness_override_sql(relation, ovr);
                    run_override_sql(
                        &adapter_for_map,
                        conn,
                        semantic_fqn,
                        &sql,
                        token_clone.clone(),
                    )
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

    fn list_relations_in_parallel_inner_with_options(
        &self,
        db_schemas: &[CatalogAndSchema],
        options: &MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        type Acc = BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>;
        let metadata_warehouse = options.warehouse.clone();
        let factory = Box::new(MetadataWarehouseConnectionFactory::new(
            self.adapter.clone(),
            metadata_warehouse,
            token.clone(),
            Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone())),
        ));

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
                Err(e) => {
                    // Empty schema error code - no relations in this schema
                    // XXX: The AdapterError struct is not properly being built at the moment, rely on string search for now
                    if e.message().contains("Object does not exist") {
                        acc.insert(db_schema, Ok(Vec::new()));
                        Ok(())
                    } else {
                        // Other errors should be propagated
                        Err(Cancellable::Error(e))
                    }
                }
            }
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(db_schemas.to_vec()), token)
    }
}

impl SnowflakeMetadataAdapter {
    /// Count the schemas in `database`, capped at `limit` rows.
    ///
    /// A cheap, constant-cost probe (`SHOW TERSE SCHEMAS IN DATABASE ... LIMIT`)
    /// used by the adaptive freshness prefetch to choose between one broad
    /// `table_schema IN (...)` scan and per-schema point queries, without listing
    /// the whole database. Returns `min(actual_schema_count, limit)`; callers treat
    /// `observed == limit` as "the database has at least `limit` schemas".
    pub fn count_schemas_up_to<'a>(
        &'a self,
        database: &'a str,
        limit: usize,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, usize> {
        let sql = snowflake_schema_count_sql(database, limit);
        let adapter = self.adapter.clone();
        let token_clone = token.clone();

        // Runs on the default connection (no metadata warehouse): the adaptive
        // prefetch only probes on the no-metadata-warehouse path.
        let factory = Box::new(MetadataWarehouseConnectionFactory::new(
            adapter.clone(),
            None,
            token_clone.clone(),
            Box::new(AdapterConnectionFactory::new(adapter.engine().clone())),
        ));

        let map_f = move |conn: &'_ mut dyn Connection, _: &()| -> AdapterResult<usize> {
            let ctx = QueryCtx::default().with_desc("dbt State schema-count probe");
            let (_resp, agate_table) =
                adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            Ok(agate_table.original_record_batch().num_rows())
        };

        let reduce_f = move |acc: &mut usize, _: (), res: AdapterResult<usize>| {
            *acc = res?;
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(vec![()]), token)
    }

    /// Decide whether to run per-schema point queries sequentially instead of one
    /// broad `IN (...)` scan for `database`.
    ///
    /// Runs `SHOW TERSE SCHEMAS IN DATABASE "<database>" LIMIT T`
    /// (T = `schema_probe_limit`) via `count_schemas_up_to`. Returns `true` when
    /// the probe saturates at exactly `T` rows (the database has `N >= T` schemas,
    /// so sequential point queries are expected to be cheaper); returns `false`
    /// otherwise. On any probe error, returns `false` (fall back to the IN scan).
    /// Returns `true` without probing when `num_schemas <= ALWAYS_SEQUENTIAL_MAX_SCHEMAS`.
    async fn should_fetch_schemas_sequentially(
        &self,
        database: &str,
        num_schemas: usize,
        token: CancellationToken,
    ) -> bool {
        if num_schemas <= ALWAYS_SEQUENTIAL_MAX_SCHEMAS {
            return true;
        }

        let threshold = schema_probe_limit(num_schemas);
        let observed = self
            .count_schemas_up_to(database, threshold, token)
            .await
            .ok();
        let sequential = schema_probe_decision(num_schemas, observed);
        match observed {
            None => emit_debug_log_message(format!(
                "Schema-count probe failed for catalog {database} (fetching {num_schemas} schemas, \
                 threshold {threshold}); falling back to a single IN scan"
            )),
            Some(observed) => emit_debug_log_message(format!(
                "Schema-count probe for catalog {database} (fetching {num_schemas} schemas, \
                 threshold {threshold}) observed {observed} schemas; using {} strategy",
                if sequential {
                    "sequential point-query"
                } else {
                    "single IN scan"
                }
            )),
        }
        sequential
    }

    /// Sequentially fetch per-schema dumps for a database, each fail-open (a
    /// failed dump omits that schema).
    async fn freshness_by_schema_sequential(
        &self,
        database: &str,
        schemas: &BTreeMap<String, Vec<Arc<dyn BaseRelation>>>,
        options: &MetadataQueryOptions,
        token: CancellationToken,
    ) -> Result<BTreeMap<String, MetadataFreshness>, Cancellable<AdapterError>> {
        let mut result = BTreeMap::new();
        for (schema, relations) in schemas {
            result.extend(
                freshness_group_dump(self, database, schema, relations, options, token.clone())
                    .await?,
            );
        }
        Ok(result)
    }

    /// Fetch per-schema dumps for a database concurrently on the dedicated
    /// metadata warehouse, bounded by the engine's thread count. Each schema is
    /// fail-open (a failed dump omits that schema).
    async fn freshness_by_schema_fanout(
        &self,
        database: &str,
        schemas: &BTreeMap<String, Vec<Arc<dyn BaseRelation>>>,
        options: &MetadataQueryOptions,
        token: CancellationToken,
    ) -> Result<BTreeMap<String, MetadataFreshness>, Cancellable<AdapterError>> {
        let fan_out = self
            .adapter
            .engine()
            .threads()
            .unwrap_or(DEFAULT_SCHEMA_PREFETCH_FANOUT)
            .max(1);
        // Build the per-schema futures in an explicit loop rather than
        // `schemas.iter().map(...)`: a closure that returns a future borrowing its
        // argument trips the higher-ranked-lifetime inference, whereas each call
        // here binds concrete lifetimes.
        let mut group_futures = Vec::with_capacity(schemas.len());
        for (schema, relations) in schemas {
            group_futures.push(freshness_group_dump(
                self,
                database,
                schema,
                relations,
                options,
                token.clone(),
            ));
        }
        let groups: Vec<Result<BTreeMap<String, MetadataFreshness>, Cancellable<AdapterError>>> =
            futures::stream::iter(group_futures)
                .buffer_unordered(fan_out)
                .collect()
                .await;
        let mut result = BTreeMap::new();
        for group in groups {
            result.extend(group?);
        }
        Ok(result)
    }

    /// One broad `table_schema IN (...)` scan for a database. Errors propagate so
    /// replay callers can handle missing data; outer callers decide when to fail
    /// open. An empty scan leaves those relations' freshness unknown.
    async fn freshness_all_in_schemas_broad(
        &self,
        database: &str,
        relations: &[Arc<dyn BaseRelation>],
        options: &MetadataQueryOptions,
        token: CancellationToken,
    ) -> Result<BTreeMap<String, MetadataFreshness>, Cancellable<AdapterError>> {
        self.freshness_all_in_schemas_broad_raw(database, relations, options, token)
            .await
    }

    /// Raw broad multi-schema `table_schema IN (...)` scan for one database — one
    /// query covering every schema of `relations` (no fail-open handling; callers
    /// add it).
    ///
    /// The set of schemas to scan is derived from `relations` themselves (via the
    /// same `schema_as_resolved_str` resolution `find_matching_relation` uses to
    /// key results), deduplicated and validated non-empty, so the predicate can
    /// never scope a different set of schemas than the results are matched
    /// against.
    fn freshness_all_in_schemas_broad_raw<'a>(
        &'a self,
        database: &'a str,
        relations: &'a [Arc<dyn BaseRelation>],
        options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        // This scan reads a single `{database}.INFORMATION_SCHEMA.TABLES`, and
        // `find_matching_relation` keys results by schema + table only (it does
        // not compare database). Restrict to relations that actually resolve to
        // this database so a same-`schema.table` relation in another database can
        // never be keyed to a row from this one. Callers group by database before
        // building the broad group, so in practice this keeps every relation;
        // it just makes the method correct regardless of how it is called.
        let relations: Vec<Arc<dyn BaseRelation>> = relations
            .iter()
            .filter(|relation| {
                relation.database_as_resolved_str().ok().as_deref() == Some(database)
            })
            .cloned()
            .collect();
        if relations.is_empty() {
            return Box::pin(async move { Ok(BTreeMap::new()) });
        }

        let quoted_schemas: Result<BTreeSet<String>, AdapterError> = relations
            .iter()
            .map(|relation| {
                let schema = relation.schema_as_resolved_str().map_err(|_| {
                    AdapterError::new(
                        AdapterErrorKind::UnexpectedResult,
                        "relation schema should not be None",
                    )
                })?;
                require_snowflake_metadata_component("schema", &schema)
                    .map(snowflake_schema_literal)
            })
            .collect();
        let quoted_schemas = match quoted_schemas {
            Ok(quoted_schemas) => quoted_schemas,
            Err(e) => {
                let future = async move { Err(Cancellable::Error(e)) };
                return Box::pin(future);
            }
        };
        let where_clause = format!(
            "table_schema IN ({})",
            quoted_schemas.into_iter().collect::<Vec<_>>().join(", ")
        );
        let sql = match snowflake_freshness_sql(database, &[where_clause]) {
            Ok(sql) => sql,
            Err(e) => {
                let future = async move { Err(Cancellable::Error(e)) };
                return Box::pin(future);
            }
        };
        let adapter = self.adapter.clone();
        let metadata_warehouse = options.warehouse.clone();
        let token_clone = token.clone();

        let factory = Box::new(MetadataWarehouseConnectionFactory::new(
            adapter.clone(),
            metadata_warehouse,
            token_clone.clone(),
            Box::new(AdapterConnectionFactory::new(adapter.engine().clone())),
        ));
        type Acc = BTreeMap<String, MetadataFreshness>;

        let map_f = move |conn: &'_ mut dyn Connection,
                          _: &()|
              -> AdapterResult<Arc<RecordBatch>> {
            let ctx = QueryCtx::default().with_desc("Extracting freshness from information schema");
            let (_resp, agate_table) =
                adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            Ok(agate_table.original_record_batch())
        };

        let reduce_f = move |acc: &mut Acc, _: (), batch_res: AdapterResult<Arc<RecordBatch>>| {
            let batch = batch_res?;
            let schemas = batch.column_values::<StringArray>("TABLE_SCHEMA")?;
            let tables = batch.column_values::<StringArray>("TABLE_NAME")?;
            let timestamps = batch.column_values::<TimestampMillisecondArray>("LAST_ALTERED")?;
            let is_views = batch.column_values::<BooleanArray>("IS_VIEW")?;
            for i in 0..batch.num_rows() {
                for fqn in find_matching_relation(schemas.value(i), tables.value(i), &relations)? {
                    acc.insert(
                        fqn,
                        MetadataFreshness::from_millis(timestamps.value(i), is_views.value(i))?,
                    );
                }
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(vec![()]), token)
    }
}

impl MetadataAdapter for SnowflakeMetadataAdapter {
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

        let stats_sql_result = lowercase_column_names(&stats_sql_result);

        let table_catalogs = stats_sql_result.column_values::<StringArray>("table_database")?;
        let table_schemas = stats_sql_result.column_values::<StringArray>("table_schema")?;
        let table_names = stats_sql_result.column_values::<StringArray>("table_name")?;
        let data_types = stats_sql_result.column_values::<StringArray>("table_type")?;
        let comments = stats_sql_result.column_values::<StringArray>("table_comment")?;
        let table_owners = stats_sql_result.column_values::<StringArray>("table_owner")?;

        let clustering_key_label =
            stats_sql_result.column_values::<StringArray>("stats:clustering_key:label")?;
        let clustering_key_value =
            stats_sql_result.column_values::<StringArray>("stats:clustering_key:value")?;
        let clustering_key_description =
            stats_sql_result.column_values::<StringArray>("stats:clustering_key:description")?;
        let clustering_key_include =
            stats_sql_result.column_values::<BooleanArray>("stats:clustering_key:include")?;

        let row_count_label =
            stats_sql_result.column_values::<StringArray>("stats:row_count:label")?;
        let row_count_value =
            stats_sql_result.column_values::<Decimal128Array>("stats:row_count:value")?;
        let row_count_description =
            stats_sql_result.column_values::<StringArray>("stats:row_count:description")?;
        let row_count_include =
            stats_sql_result.column_values::<BooleanArray>("stats:row_count:include")?;

        let bytes_label = stats_sql_result.column_values::<StringArray>("stats:bytes:label")?;
        let bytes_value = stats_sql_result.column_values::<Decimal128Array>("stats:bytes:value")?;
        let bytes_description =
            stats_sql_result.column_values::<StringArray>("stats:bytes:description")?;
        let bytes_include =
            stats_sql_result.column_values::<BooleanArray>("stats:bytes:include")?;

        let last_modified_label =
            stats_sql_result.column_values::<StringArray>("stats:last_modified:label")?;
        let last_modified_value =
            stats_sql_result.column_values::<StringArray>("stats:last_modified:value")?;
        let last_modified_description =
            stats_sql_result.column_values::<StringArray>("stats:last_modified:description")?;
        let last_modified_include =
            stats_sql_result.column_values::<BooleanArray>("stats:last_modified:include")?;

        let mut result = BTreeMap::<String, CatalogTable>::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i).to_string();
            let schema = table_schemas.value(i).to_string();
            let table = table_names.value(i).to_string();
            let data_type = data_types.value(i).to_string();
            let comment = comments.value(i);
            let owner = table_owners.value(i).to_string();

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            if !result.contains_key(&fully_qualified_name) {
                let clustering_key_label_i = clustering_key_label.value(i);
                let clustering_key_value_i = clustering_key_value.value(i);
                let clustering_key_description_i = clustering_key_description.value(i);
                let clustering_key_include_i = clustering_key_include.value(i);

                let row_count_label_i = row_count_label.value(i);
                let row_count_value_i = row_count_value.value(i);
                let row_count_description_i = row_count_description.value(i);
                let row_count_include_i = row_count_include.value(i);

                let bytes_label_i = bytes_label.value(i);
                let bytes_value_i = bytes_value.value(i);
                let bytes_description_i = bytes_description.value(i);
                let bytes_include_i = bytes_include.value(i);

                let last_modified_label_i = last_modified_label.value(i);
                let last_modified_value_i = last_modified_value.value(i);
                let last_modified_description_i = last_modified_description.value(i);
                let last_modified_include_i = last_modified_include.value(i);

                let mut stats = BTreeMap::new();
                if clustering_key_include_i {
                    stats.insert(
                        "clustering_key".to_string(),
                        CatalogNodeStats {
                            id: "clustering_key".to_string(),
                            label: clustering_key_label_i.to_string(),
                            value: serde_json::Value::String(clustering_key_value_i.to_string()),
                            description: Some(clustering_key_description_i.to_string()),
                            include: clustering_key_include_i,
                        },
                    );
                }
                if bytes_include_i {
                    stats.insert(
                        "bytes".to_string(),
                        CatalogNodeStats {
                            id: "bytes".to_string(),
                            label: bytes_label_i.to_string(),
                            value: serde_json::Number::from_i128(bytes_value_i).into(),
                            description: Some(bytes_description_i.to_string()),
                            include: bytes_include_i,
                        },
                    );
                }
                if row_count_include_i {
                    stats.insert(
                        "row_count".to_string(),
                        CatalogNodeStats {
                            id: "row_count".to_string(),
                            label: row_count_label_i.to_string(),
                            value: serde_json::Number::from_i128(row_count_value_i).into(),
                            description: Some(row_count_description_i.to_string()),
                            include: row_count_include_i,
                        },
                    );
                }
                if last_modified_include_i {
                    stats.insert(
                        "last_modified".to_string(),
                        CatalogNodeStats {
                            id: "last_modified".to_string(),
                            label: last_modified_label_i.to_string(),
                            value: serde_json::Value::String(last_modified_value_i.to_string()),
                            description: Some(last_modified_description_i.to_string()),
                            include: last_modified_include_i,
                        },
                    );
                }

                stats.insert(
                    "has_stats".to_string(),
                    CatalogNodeStats {
                        id: "has_stats".to_string(),
                        label: "Has Stats?".to_string(),
                        value: serde_json::Value::Bool(!stats.is_empty()),
                        description: Some(
                            "Indicates whether there are statistics for this table".to_string(),
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

        let stats_sql_result = lowercase_column_names(&stats_sql_result);

        // Can probably zip these into a table metadata tuple array
        let table_catalogs = stats_sql_result.column_values::<StringArray>("table_database")?;
        let table_schemas = stats_sql_result.column_values::<StringArray>("table_schema")?;
        let table_names = stats_sql_result.column_values::<StringArray>("table_name")?;

        let column_names = stats_sql_result.column_values::<StringArray>("column_name")?;
        let column_indices = stats_sql_result.column_values::<Decimal128Array>("column_index")?;
        let column_types = stats_sql_result.column_values::<StringArray>("column_type")?;
        let column_comments = stats_sql_result.column_values::<StringArray>("column_comment")?;

        let mut columns_by_relation = BTreeMap::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            let column_name_i = column_names.value(i);
            let column_index_i = column_indices.value(i);
            let column_type_i = column_types.value(i);
            let column_comment_i = column_comments.value(i);

            let column = ColumnMetadata {
                name: column_name_i.to_string(),
                index: column_index_i,
                data_type: column_type_i.to_string(),
                comment: match column_comment_i {
                    "" => None,
                    _ => Some(column_comment_i.to_string()),
                },
            };

            columns_by_relation
                .entry(fully_qualified_name.clone())
                .or_insert(BTreeMap::new())
                .insert(column_name_i.to_string(), column);
        }
        Ok(columns_by_relation)
    }

    fn list_user_defined_functions_inner(
        &self,
        catalog_schemas: &BTreeMap<String, BTreeSet<String>>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<UDF>> {
        type Acc = Vec<UDF>;

        // https://docs.snowflake.com/en/sql-reference/sql/show-user-functions
        // this is chosen over `information_schema.views` because the latter takes tens of seconds to complete
        // when running against the `ska67070` account
        let queries = catalog_schemas
            .iter()
            .flat_map(|(catalog, schemas)| {
                schemas
                    .iter()
                    .map(move |schema| format!("SHOW USER FUNCTIONS IN SCHEMA {catalog}.{schema}"))
            })
            .collect::<Vec<_>>();

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f =
            move |conn: &'_ mut dyn Connection, sql: &String| -> AdapterResult<Arc<RecordBatch>> {
                let ctx = QueryCtx::default().with_desc("List user functions");
                let (_, table) = adapter.query(&ctx, conn, sql, None, token_clone.clone())?;
                let batch = table.original_record_batch();
                Ok(batch)
            };

        let reduce_f = |acc: &mut Acc,
                        _sql: String,
                        batch_res: AdapterResult<Arc<RecordBatch>>|
         -> Result<(), Cancellable<AdapterError>> {
            let batch = batch_res?;

            if batch.num_rows() == 0 {
                return Ok(());
            }

            let catalog_names = batch.column_values::<StringArray>("catalog_name")?;
            let schema_names = batch.column_values::<StringArray>("schema_name")?;
            let names = batch.column_values::<StringArray>("name")?;
            let descriptions = batch.column_values::<StringArray>("description")?;
            let is_table = batch.column_values::<StringArray>("is_table_function")?;
            let is_aggregate = batch.column_values::<StringArray>("is_aggregate")?;
            let language = batch.column_values::<StringArray>("language")?;
            let arguments = batch.column_values::<StringArray>("arguments")?;

            // possible values are either "Y" or "N"
            let is_true = |s: &str| s.to_uppercase() == "Y";
            for i in 0..batch.num_rows() {
                let language = language.value(i).to_string();
                if language.to_uppercase() != "SQL" {
                    continue;
                }

                let catalog = catalog_names.value(i).to_string();
                let schema = schema_names.value(i).to_string();
                let name = names.value(i).to_string();

                let description = descriptions.value(i).to_string();
                let is_table = is_true(is_table.value(i));
                let is_aggregate = is_true(is_aggregate.value(i));
                // Data types of the arguments and return value.
                let signature = arguments.value(i).to_string();

                // Snowflake doesn't tell if a function is a window function or not in either
                // `show functions` or `information_schema.functions view`
                let kind = if is_aggregate {
                    UDFKind::Aggregate
                } else if is_table {
                    UDFKind::Table
                } else {
                    UDFKind::Scalar
                };

                let fqn = format!("{catalog}.{schema}.{name}");
                acc.push(UDF {
                    name: fqn,
                    description,
                    signature,
                    adapter_type: AdapterType::Snowflake,
                    kind,
                });
            }

            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(queries), token)
    }

    fn list_relations_schemas_inner(
        &self,
        unique_id: Option<String>,
        phase: Option<ExecutionPhase>,
        relations: &[Arc<dyn BaseRelation>],
        item_span_operation_id: Option<&str>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, HashMap<String, AdapterResult<Arc<Schema>>>> {
        // All results are accumulated in an unordered map
        type Acc = HashMap<String, AdapterResult<Arc<Schema>>>;

        let keys: Vec<(String, String)> = relations
            .iter()
            .map(|relation| (relation.semantic_fqn(), relation.render_self_as_str()))
            .collect();

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          key: &(String, String)|
              -> AdapterResult<Arc<Schema>> {
            let (_, rendered) = key;
            let sql = format!("describe table {};", rendered);
            let mut ctx = QueryCtx::new_metadata().with_desc("Get table schema");
            if let Some(node_id) = unique_id.clone() {
                ctx = ctx.with_node_id(&node_id);
            }
            if let Some(phase) = phase {
                ctx = ctx.with_phase(phase.as_str());
            }
            let (_, table) = adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            let batch = table.original_record_batch();
            let schema = build_schema_from_desc_table(batch, adapter.engine().type_ops().as_ref())?;
            Ok(schema)
        };
        let reduce_f = |acc: &mut Acc,
                        key: (String, String),
                        schema: AdapterResult<Arc<Schema>>|
         -> Result<(), Cancellable<AdapterError>> {
            let (semantic_fqn, _) = key;
            acc.insert(semantic_fqn, schema);
            Ok(())
        };
        run_schema_cache_map_reduce(
            factory,
            keys,
            item_span_operation_id,
            map_f,
            reduce_f,
            None,
            token,
        )
    }

    /// List relations schemas by patterns (use information schema query)
    fn list_relations_schemas_by_patterns_inner(
        &self,
        relations_pattern: &[RelationPattern],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        // All results are accumulated in a Vec of pairs
        type Acc = Vec<(String, AdapterResult<RelationSchemaPair>)>;

        // Group patterns by database to minimize queries needed
        let mut patterns_by_database = BTreeMap::new();
        for pat in relations_pattern {
            patterns_by_database
                .entry(pat.database.clone())
                .or_insert_with(Vec::new)
                .push(pat);
        }

        let queries = patterns_by_database
            .into_iter()
            .map(|(database, patterns)| {
                // Build the query for all relations in this database
                let predicates = patterns
                    .iter()
                    .map(|pat| {
                        format!(
                            "(TABLE_SCHEMA ILIKE '{}' AND TABLE_NAME ILIKE '{}')",
                            pat.schema_pattern, pat.table_pattern
                        )
                    })
                    .collect::<Vec<_>>();
                let predicates_union = predicates.join(" OR ");
                format!(
                    "SELECT
    TABLE_CATALOG,
    TABLE_SCHEMA,
    TABLE_NAME,
    COLUMN_NAME,
    DATA_TYPE,
    IS_NULLABLE,
    CHARACTER_MAXIMUM_LENGTH,
    NUMERIC_PRECISION,
    NUMERIC_SCALE,
    COMMENT
FROM {database}.INFORMATION_SCHEMA.COLUMNS
WHERE {predicates_union}
ORDER BY TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION"
                )
            });

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        // map_f runs the queries, reduce_f decodes the result set and builds the schemas
        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f =
            move |conn: &'_ mut dyn Connection, sql: &String| -> AdapterResult<Arc<RecordBatch>> {
                let ctx = QueryCtx::default().with_desc("Get schema by pattern");
                let (_, table) = adapter.query(&ctx, conn, sql, None, token_clone.clone())?;
                let batch = table.original_record_batch();
                Ok(batch)
            };

        let quoting = self.adapter.quoting();

        let adapter = self.adapter.clone();
        let reduce_f = move |acc: &mut Acc,
                             _sql: String,
                             batch_res: AdapterResult<Arc<RecordBatch>>|
              -> Result<(), Cancellable<AdapterError>> {
            let batch = batch_res?;
            let mut schemas_from_batch = build_schemas_from_information_schema(
                batch,
                quoting,
                adapter.engine().type_ops().as_ref(),
            )?;
            acc.append(&mut schemas_from_batch);
            Ok(())
        };
        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        let keys = queries.collect::<Vec<_>>();
        map_reduce.run(Arc::new(keys), token)
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
        self.freshness_inner_with_options(relations, &MetadataQueryOptions::default(), token)
    }

    fn freshness_with_options<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        with_time_machine_metadata_wrapper(
            "global",
            "freshness",
            args_freshness(
                relations.iter().map(|r| r.semantic_fqn()),
                options.warehouse.clone(),
            ),
            self.freshness_inner_with_options(relations, options, token),
        )
    }

    /// Honors per-source `loaded_at_field` / `loaded_at_query` config. Mirrors the
    /// dbt-core run-cache plugin: relations without overrides go through the bulk
    /// INFORMATION_SCHEMA path; each override runs as one targeted query in
    /// parallel. Net call count: 1 bulk (over the non-override subset) + N
    /// override queries — same shape as the plugin.
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
            self.freshness_with_overrides_inner_with_options(
                relations,
                overrides,
                &MetadataQueryOptions::default(),
                token,
            ),
        )
    }

    fn freshness_with_overrides_and_options<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        overrides: &'a BTreeMap<String, FreshnessOverride>,
        options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        with_time_machine_metadata_wrapper(
            "global",
            "freshness_with_overrides",
            args_freshness_with_overrides(
                relations.iter().map(|r| r.semantic_fqn()),
                overrides,
                options.warehouse.clone(),
            ),
            self.freshness_with_overrides_inner_with_options(relations, overrides, options, token),
        )
    }

    fn supports_bulk_freshness_dump(&self) -> bool {
        true
    }

    fn freshness_all_in_schema_inner<'a>(
        &'a self,
        database: &'a str,
        schema: &'a str,
        relations: &'a [Arc<dyn BaseRelation>],
        options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        let schema = match require_snowflake_metadata_component("schema", schema) {
            Ok(schema) => schema,
            Err(e) => {
                let future = async move { Err(Cancellable::Error(e)) };
                return Box::pin(future);
            }
        };

        // Schema-only WHERE clause — no per-table filtering.
        let where_clause = format!("table_schema = {}", snowflake_schema_literal(schema));
        let sql = match snowflake_freshness_sql(database, &[where_clause]) {
            Ok(sql) => sql,
            Err(e) => {
                let future = async move { Err(Cancellable::Error(e)) };
                return Box::pin(future);
            }
        };
        let relations = relations.to_vec();
        let adapter = self.adapter.clone();
        let metadata_warehouse = options.warehouse.clone();
        let token_clone = token.clone();

        let factory = Box::new(MetadataWarehouseConnectionFactory::new(
            adapter.clone(),
            metadata_warehouse,
            token_clone.clone(),
            Box::new(AdapterConnectionFactory::new(adapter.engine().clone())),
        ));
        type Acc = BTreeMap<String, MetadataFreshness>;

        let map_f = move |conn: &'_ mut dyn Connection,
                          _: &()|
              -> AdapterResult<Arc<RecordBatch>> {
            let ctx = QueryCtx::default().with_desc("Extracting freshness from information schema");
            let (_resp, agate_table) =
                adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            Ok(agate_table.original_record_batch())
        };

        let reduce_f = move |acc: &mut Acc, _: (), batch_res: AdapterResult<Arc<RecordBatch>>| {
            let batch = batch_res?;
            let schemas = batch.column_values::<StringArray>("TABLE_SCHEMA")?;
            let tables = batch.column_values::<StringArray>("TABLE_NAME")?;
            let timestamps = batch.column_values::<TimestampMillisecondArray>("LAST_ALTERED")?;
            let is_views = batch.column_values::<BooleanArray>("IS_VIEW")?;
            for i in 0..batch.num_rows() {
                for fqn in find_matching_relation(schemas.value(i), tables.value(i), &relations)? {
                    acc.insert(
                        fqn,
                        MetadataFreshness::from_millis(timestamps.value(i), is_views.value(i))?,
                    );
                }
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(vec![()]), token)
    }

    /// Owns the Snowflake freshness-prefetch strategy: group by database and, per
    /// database, pick the cheapest way to dump last-modified metadata.
    ///
    /// - `metadata_warehouse` set → parallel per-schema fan-out on the isolated
    ///   warehouse (bounded by the engine's thread count).
    /// - no warehouse + adaptive → schema-count probe chooses a broad
    ///   `table_schema IN (...)` scan (small DB) or sequential per-schema point
    ///   queries (large DB).
    /// - no warehouse + non-adaptive → always the broad `IN (...)` scan.
    ///
    /// Every path is fail-open, so a single failing schema or database never
    /// aborts the run.
    fn freshness_all_in_schemas<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        with_time_machine_metadata_wrapper(
            "global",
            "freshness_all_in_schemas",
            args_freshness(
                relations.iter().map(|r| r.semantic_fqn()),
                options.warehouse.clone(),
            ),
            async move {
                // Group by resolved database, then resolved schema, so the strategy
                // decision (per database) and the `table_schema` predicates line up
                // with `find_matching_relation`'s schema resolution.
                let mut by_database: BTreeMap<
                    String,
                    BTreeMap<String, Vec<Arc<dyn BaseRelation>>>,
                > = BTreeMap::new();
                for relation in relations {
                    let database = relation.database_as_resolved_str().unwrap_or_default();
                    let schema = relation.schema_as_resolved_str().unwrap_or_default();
                    by_database
                        .entry(database)
                        .or_default()
                        .entry(schema)
                        .or_default()
                        .push(Arc::clone(relation));
                }

                // With a dedicated metadata warehouse the per-schema dumps run on the
                // isolated warehouse, so fanning them out is a clear win. Without one
                // they contend on the main warehouse, so the sequential/broad choice
                // (below) keeps concurrency at one query at a time.
                let has_metadata_warehouse = options
                    .warehouse
                    .as_deref()
                    .is_some_and(|warehouse| !warehouse.is_empty());

                let mut result: BTreeMap<String, MetadataFreshness> = BTreeMap::new();
                for (database, schemas) in by_database {
                    let db_result = if has_metadata_warehouse {
                        self.freshness_by_schema_fanout(&database, &schemas, options, token.clone())
                            .await
                    } else {
                        let use_broad = if options.adaptive_metadata_fetch {
                            !self
                                .should_fetch_schemas_sequentially(
                                    &database,
                                    schemas.len(),
                                    token.clone(),
                                )
                                .await
                        } else {
                            true
                        };
                        if use_broad {
                            let db_relations: Vec<Arc<dyn BaseRelation>> =
                                schemas.values().flatten().cloned().collect();
                            self.freshness_all_in_schemas_broad(
                                &database,
                                &db_relations,
                                options,
                                token.clone(),
                            )
                            .await
                        } else {
                            self.freshness_by_schema_sequential(
                                &database,
                                &schemas,
                                options,
                                token.clone(),
                            )
                            .await
                        }
                    };
                    match db_result {
                        Ok(values) => result.extend(values),
                        Err(Cancellable::Error(err))
                            if err.kind() == AdapterErrorKind::ReplayDataMissing =>
                        {
                            return Err(Cancellable::Error(err));
                        }
                        // A cancelled join/query can surface as `Cancellable::Error`
                        // with kind `Cancelled` instead of `Cancellable::Cancelled` —
                        // treat it the same way so cancellation always stops the run
                        // instead of fail-opening.
                        Err(Cancellable::Error(err))
                            if err.kind() == AdapterErrorKind::Cancelled =>
                        {
                            return Err(Cancellable::Error(err));
                        }
                        Err(Cancellable::Error(err)) => emit_warn_log_message(
                            ErrorCode::StateServiceWarn,
                            format!(
                                "dbt State database-level freshness dump failed for {database}: {err}; \
                                 omitting freshness for this database"
                            ),
                        ),
                        Err(Cancellable::Cancelled) => {
                            return Err(Cancellable::Cancelled);
                        }
                    }
                }
                Ok(result)
            },
        )
    }

    /// Reference: https://github.com/dbt-labs/dbt-adapters/blob/f492c919d3bd415bf5065b3cd8cd1af23562feb0/dbt-snowflake/src/dbt/include/snowflake/macros/metadata/list_relations_without_caching.sql
    fn list_relations_in_parallel_inner(
        &self,
        db_schemas: &[CatalogAndSchema],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        self.list_relations_in_parallel_inner_with_options(
            db_schemas,
            &MetadataQueryOptions::default(),
            token,
        )
    }

    fn relations_exist_with_options<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, bool>> {
        let db_schemas = relations
            .iter()
            .map(CatalogAndSchema::from)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let future = async move {
            let listed = self
                .list_relations_in_parallel_inner_with_options(&db_schemas, options, token)
                .await?;
            let mut result = BTreeMap::new();

            for relation in relations {
                let semantic_fqn = relation.semantic_fqn();
                let catalog_schema = CatalogAndSchema::from(relation);
                let Some(schema_relations) = listed.get(&catalog_schema) else {
                    result.insert(semantic_fqn, false);
                    continue;
                };

                let schema_relations = schema_relations.as_ref().map_err(|err| {
                    Cancellable::Error(AdapterError::new(err.kind(), err.message().to_string()))
                })?;
                let exists = schema_relations
                    .iter()
                    .any(|candidate| candidate.semantic_fqn() == semantic_fqn);
                result.insert(semantic_fqn, exists);
            }

            Ok(result)
        };
        with_time_machine_metadata_wrapper(
            "global",
            "relations_exist",
            args_relations_exist(
                relations.iter().map(|r| r.semantic_fqn()),
                options.warehouse.clone(),
            ),
            future,
        )
    }

    fn is_permission_error(&self, e: &AdapterError) -> bool {
        // this is supposed to be using/extended from ANSI SQL standard but I didn't find any Snowflake documentation
        // the magic strings here are from inspecting the results from fs run on a project with a new database,
        // and a weak role that lack permissions to create a database

        // 42501: insufficient privileges
        // 02000: does not exist or not authorized error
        e.sqlstate() == "42501" || e.sqlstate() == "02000"
    }

    fn fetch_view_definitions_inner<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, ViewDefinitionFetchResult> {
        type Acc = ViewDefinitionFetchResult;

        if relations.is_empty() {
            return Box::pin(async { Ok(ViewDefinitionFetchResult::default()) });
        }

        // Dedupe FQNs while preserving an order; Snowflake fans them out itself.
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

        let script = build_view_definition_script(&fqns);

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          script: &String|
              -> AdapterResult<Arc<RecordBatch>> {
            let ctx = QueryCtx::default().with_desc("Fetch view definitions");
            let sql = format!("EXECUTE IMMEDIATE $${script}$$");
            let (_, table) = adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            Ok(table.original_record_batch())
        };

        let reduce_f = |acc: &mut Acc,
                        _key: String,
                        batch_res: AdapterResult<Arc<RecordBatch>>|
         -> Result<(), Cancellable<AdapterError>> {
            let batch = batch_res?;
            accumulate_view_definition_fetch_result(acc, &batch).map_err(Cancellable::Error)
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(vec![script]), token)
    }
}

/// reference: https://github.com/sdf-labs/sdf/blob/main/crates/sdf-cli/src/providers/database/snowflake.rs#L177-L178
fn build_schema_from_desc_table(
    show_columns_result: Arc<RecordBatch>,
    type_ops: &dyn TypeOps,
) -> AdapterResult<Arc<Schema>> {
    let column_names = show_columns_result.column_values::<StringArray>("name")?;
    let data_types = show_columns_result.column_values::<StringArray>("type")?;
    let comments = show_columns_result.column_values::<StringArray>("comment")?;
    let nullability = show_columns_result.column_values::<StringArray>("null?")?;

    let mut fields = vec![];
    for i in 0..show_columns_result.num_rows() {
        let name = column_names.value(i);
        let nullable = nullability.value(i).to_uppercase() == "Y";
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

#[allow(clippy::type_complexity)]
fn build_schemas_from_information_schema(
    information_schema_result: Arc<RecordBatch>,
    quoting: ResolvedQuoting,
    type_ops: &dyn TypeOps,
) -> AdapterResult<Vec<(String, AdapterResult<RelationSchemaPair>)>> {
    if information_schema_result.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let table_catalogs = information_schema_result.column_values::<StringArray>("TABLE_CATALOG")?;
    let table_schemas = information_schema_result.column_values::<StringArray>("TABLE_SCHEMA")?;
    let table_names = information_schema_result.column_values::<StringArray>("TABLE_NAME")?;
    let column_names = information_schema_result.column_values::<StringArray>("COLUMN_NAME")?;
    let data_types = information_schema_result.column_values::<StringArray>("DATA_TYPE")?;
    let is_nullable = information_schema_result.column_values::<StringArray>("IS_NULLABLE")?;
    let numeric_precision =
        information_schema_result.column_values::<Decimal128Array>("NUMERIC_PRECISION")?;
    let numeric_scale =
        information_schema_result.column_values::<Decimal128Array>("NUMERIC_SCALE")?;
    let comments = information_schema_result.column_values::<StringArray>("COMMENT")?;

    let mut result = Vec::<(String, AdapterResult<RelationSchemaPair>)>::new();
    let mut current_table = String::new();
    let mut current_fields = Vec::new();
    let mut current_relation: Option<Arc<dyn BaseRelation>> = None;

    for i in 0..information_schema_result.num_rows() {
        let catalog = table_catalogs.value(i);
        let schema = table_schemas.value(i);
        let table = table_names.value(i);
        let fully_qualified_name = format!("{catalog}.{schema}.{table}");

        // If we're starting a new table, save the previous one and start fresh
        if fully_qualified_name != current_table {
            if !current_table.is_empty() {
                let relation_schema: RelationSchemaPair = (
                    current_relation.expect("current_relation should not be None"),
                    Arc::new(Schema::new(current_fields.clone())),
                );
                result.push((current_table.clone(), Ok(relation_schema)));
            }
            current_table = fully_qualified_name;
            current_fields = Vec::new();

            let relation = match crate::relation::do_create_relation(
                type_ops.adapter_type(),
                catalog.to_string(),
                schema.to_string(),
                Some(table.to_string()),
                None,
                quoting,
            ) {
                Ok(relation) => relation,
                Err(e) => {
                    result.push((current_table, Err(e.into())));
                    return Ok(result);
                }
            };
            current_relation = Some(relation.into());
        }

        let name = column_names.value(i);
        let data_type = data_types.value(i);
        let nullable = is_nullable.value(i).to_uppercase() == "YES";
        let comment = match comments.value(i) {
            "" => None,
            c => Some(c.to_string()),
        };

        // Handle numeric types
        let data_type = if data_type == "NUMBER" || data_type == "DECIMAL" {
            let (precision, scale) = (
                numeric_precision.value(i).to_string(),
                numeric_scale.value(i).to_string(),
            );
            format!("decimal({precision},{scale})")
        } else {
            data_type.to_string()
        };

        // Add a Schema Field
        let field = match make_arrow_field(
            type_ops,
            name.to_string(),
            &data_type,
            Some(nullable),
            comment,
        ) {
            Ok(field) => field,
            Err(e) => {
                // Place the error in the accumulator output and return immediately
                // instead of trying to read more tables. Progress on the previously
                // read tables is not lost.
                result.push((current_table, Err(e)));
                return Ok(result);
            }
        };
        current_fields.push(field);
    }

    // If there is only 1 table in the query result set, it won't be captured in the loop, so save it at the end
    if !current_table.is_empty() {
        let relation_schema = (
            current_relation.expect("current_relation should not be None"),
            Arc::new(Schema::new(current_fields.clone())),
        );
        result.push((current_table, Ok(relation_schema)));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::NoopConnection;
    use arrow_schema::{DataType, Field};
    use std::sync::Mutex;

    struct FakeConnectionFactory {
        recycled: Arc<Mutex<u32>>,
    }

    impl ConnectionFactory for FakeConnectionFactory {
        type Error = Cancellable<AdapterError>;

        fn new_connection(
            &self,
            _node_id: Option<&str>,
        ) -> Result<Box<dyn Connection>, Self::Error> {
            Ok(Box::new(NoopConnection))
        }

        fn recycle_connection(&self, _conn: Box<dyn Connection>) {
            *self.recycled.lock().unwrap() += 1;
        }
    }

    /// Builds a use/restore hook pair that records calls into `log`, so tests can
    /// assert switch/restore ordering and frequency without a real connection.
    fn recording_hooks(
        log: Arc<Mutex<Vec<&'static str>>>,
        fail_restore: bool,
    ) -> (UseWarehouseFn, RestoreWarehouseFn) {
        let use_log = log.clone();
        let restore_log = log;
        (
            Box::new(move |_conn: &mut dyn Connection, _warehouse: &str| {
                use_log.lock().unwrap().push("use");
                Ok(())
            }),
            Box::new(move |_conn: &mut dyn Connection| {
                restore_log.lock().unwrap().push("restore");
                if fail_restore {
                    Err(AdapterError::new(
                        AdapterErrorKind::UnexpectedResult,
                        "restore failed",
                    ))
                } else {
                    Ok(())
                }
            }),
        )
    }

    #[test]
    fn metadata_warehouse_factory_switches_once_per_connection() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (use_warehouse, restore_warehouse) = recording_hooks(log.clone(), false);
        let recycled = Arc::new(Mutex::new(0));
        let inner = Box::new(FakeConnectionFactory {
            recycled: recycled.clone(),
        });
        let factory = MetadataWarehouseConnectionFactory::from_hooks(
            Some("metadata_wh".to_string()),
            use_warehouse,
            restore_warehouse,
            inner,
        );

        let conn = factory.new_connection(None).expect("connection");
        assert_eq!(*log.lock().unwrap(), vec!["use"]);

        factory.recycle_connection(conn);
        assert_eq!(*log.lock().unwrap(), vec!["use", "restore"]);
        assert_eq!(*recycled.lock().unwrap(), 1);
    }

    #[test]
    fn metadata_warehouse_factory_drops_connection_when_restore_fails() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (use_warehouse, restore_warehouse) = recording_hooks(log.clone(), true);
        let recycled = Arc::new(Mutex::new(0));
        let inner = Box::new(FakeConnectionFactory {
            recycled: recycled.clone(),
        });
        let factory = MetadataWarehouseConnectionFactory::from_hooks(
            Some("metadata_wh".to_string()),
            use_warehouse,
            restore_warehouse,
            inner,
        );

        let conn = factory.new_connection(None).expect("connection");
        factory.recycle_connection(conn);

        assert_eq!(*log.lock().unwrap(), vec!["use", "restore"]);
        assert_eq!(
            *recycled.lock().unwrap(),
            0,
            "connection must not be recycled after a failed restore"
        );
    }

    #[test]
    fn metadata_warehouse_factory_is_passthrough_without_metadata_warehouse() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (use_warehouse, restore_warehouse) = recording_hooks(log.clone(), false);
        let recycled = Arc::new(Mutex::new(0));
        let inner = Box::new(FakeConnectionFactory {
            recycled: recycled.clone(),
        });
        let factory = MetadataWarehouseConnectionFactory::from_hooks(
            None,
            use_warehouse,
            restore_warehouse,
            inner,
        );

        let conn = factory.new_connection(None).expect("connection");
        factory.recycle_connection(conn);

        assert!(
            log.lock().unwrap().is_empty(),
            "no warehouse switch expected when metadata warehouse is unset"
        );
        assert_eq!(*recycled.lock().unwrap(), 1);
    }

    #[test]
    fn metadata_warehouse_factory_treats_empty_warehouse_as_unset() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (use_warehouse, restore_warehouse) = recording_hooks(log.clone(), false);
        let recycled = Arc::new(Mutex::new(0));
        let inner = Box::new(FakeConnectionFactory {
            recycled: recycled.clone(),
        });
        let factory = MetadataWarehouseConnectionFactory::from_hooks(
            Some(String::new()),
            use_warehouse,
            restore_warehouse,
            inner,
        );

        let conn = factory.new_connection(None).expect("connection");
        factory.recycle_connection(conn);

        assert!(log.lock().unwrap().is_empty());
        assert_eq!(*recycled.lock().unwrap(), 1);
    }

    fn view_definition_batch(rows: Vec<(&str, Option<&str>, Option<&str>)>) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("FQN", DataType::Utf8, false),
            Field::new("VIEW_DEFINITION", DataType::Utf8, true),
            Field::new("ERROR", DataType::Utf8, true),
        ]));
        let mut fqns = Vec::with_capacity(rows.len());
        let mut definitions = Vec::with_capacity(rows.len());
        let mut errors = Vec::with_capacity(rows.len());
        for (fqn, definition, error) in rows {
            fqns.push(fqn);
            definitions.push(definition);
            errors.push(error);
        }
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(fqns)),
                Arc::new(StringArray::from(definitions)),
                Arc::new(StringArray::from(errors)),
            ],
        )
        .expect("valid view-definition batch")
    }

    /// Build a `SHOW OBJECTS`-shaped `RecordBatch`. `is_interactive` is `None` to
    /// simulate accounts/versions where that column doesn't exist yet; when present,
    /// individual cells may themselves be `None` to simulate a NULL value.
    fn show_objects_batch(
        rows: Vec<(&str, &str, &str, &str, &str, &str)>,
        is_interactive: Option<Vec<Option<&str>>>,
    ) -> RecordBatch {
        let mut fields = vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("database_name", DataType::Utf8, false),
            Field::new("schema_name", DataType::Utf8, false),
            Field::new("kind", DataType::Utf8, false),
            Field::new("is_dynamic", DataType::Utf8, false),
            Field::new("is_iceberg", DataType::Utf8, false),
        ];
        let mut names = Vec::with_capacity(rows.len());
        let mut database_names = Vec::with_capacity(rows.len());
        let mut schema_names = Vec::with_capacity(rows.len());
        let mut kinds = Vec::with_capacity(rows.len());
        let mut is_dynamics = Vec::with_capacity(rows.len());
        let mut is_icebergs = Vec::with_capacity(rows.len());
        for (name, database_name, schema_name, kind, is_dynamic, is_iceberg) in rows {
            names.push(name);
            database_names.push(database_name);
            schema_names.push(schema_name);
            kinds.push(kind);
            is_dynamics.push(is_dynamic);
            is_icebergs.push(is_iceberg);
        }
        let mut columns: Vec<Arc<dyn Array>> = vec![
            Arc::new(StringArray::from(names)),
            Arc::new(StringArray::from(database_names)),
            Arc::new(StringArray::from(schema_names)),
            Arc::new(StringArray::from(kinds)),
            Arc::new(StringArray::from(is_dynamics)),
            Arc::new(StringArray::from(is_icebergs)),
        ];
        if let Some(is_interactive) = is_interactive {
            fields.push(Field::new("is_interactive", DataType::Utf8, true));
            columns.push(Arc::new(StringArray::from(is_interactive)));
        }
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
            .expect("valid show-objects batch")
    }

    #[test]
    fn build_relations_from_show_objects_tolerates_missing_is_interactive_column() {
        let batch = show_objects_batch(vec![("MY_DT", "DB", "SCHEMA", "TABLE", "y", "n")], None);

        let relations = build_relations_from_show_objects(&batch, ResolvedQuoting::trues())
            .expect("missing is_interactive column should degrade gracefully, not error");

        assert_eq!(relations.len(), 1);
        assert_eq!(
            relations[0].relation_type(),
            Some(RelationType::DynamicTable)
        );
    }

    #[test]
    fn build_relations_from_show_objects_classifies_interactive_table_when_column_present() {
        let batch = show_objects_batch(
            vec![("MY_IT", "DB", "SCHEMA", "TABLE", "n", "n")],
            Some(vec![Some("y")]),
        );

        let relations = build_relations_from_show_objects(&batch, ResolvedQuoting::trues())
            .expect("valid show-objects batch");

        assert_eq!(relations.len(), 1);
        assert_eq!(
            relations[0].relation_type(),
            Some(RelationType::InteractiveTable)
        );
    }

    #[test]
    fn build_relations_from_show_objects_classifies_interactive_table_for_canonical_truthy_value() {
        let batch = show_objects_batch(
            vec![("MY_IT", "DB", "SCHEMA", "TABLE", "n", "n")],
            Some(vec![Some("yes")]),
        );

        let relations = build_relations_from_show_objects(&batch, ResolvedQuoting::trues())
            .expect("valid show-objects batch");

        assert_eq!(relations.len(), 1);
        assert_eq!(
            relations[0].relation_type(),
            Some(RelationType::InteractiveTable)
        );
    }

    #[test]
    fn build_relations_from_show_objects_tolerates_null_is_interactive_value() {
        let batch = show_objects_batch(
            vec![("MY_DT", "DB", "SCHEMA", "TABLE", "y", "n")],
            Some(vec![None]),
        );

        let relations = build_relations_from_show_objects(&batch, ResolvedQuoting::trues())
            .expect("NULL is_interactive cell should degrade gracefully, not error");

        assert_eq!(relations.len(), 1);
        assert_eq!(
            relations[0].relation_type(),
            Some(RelationType::DynamicTable)
        );
    }

    #[test]
    fn relation_type_from_table_flags_errors_on_malformed_is_interactive_value() {
        let err = relation_type_from_table_flags("n", "maybe").unwrap_err();
        assert!(err.message().contains("maybe"));
    }

    #[test]
    fn snowflake_freshness_sql_rejects_blank_database() {
        let err =
            snowflake_freshness_sql("  ", &["table_schema = 'PUBLIC'".to_string()]).unwrap_err();

        assert_eq!(
            err.message(),
            "Snowflake metadata query requires a non-empty database. Configure a database value on the Snowflake target, model, or source."
        );
    }

    #[test]
    fn require_snowflake_metadata_component_rejects_blank_schema() {
        let err = require_snowflake_metadata_component("schema", "\t ").unwrap_err();

        assert_eq!(
            err.message(),
            "Snowflake metadata query requires a non-empty schema. Configure a schema value on the Snowflake target, model, or source."
        );
    }

    #[test]
    fn snowflake_freshness_sql_trims_database() {
        let sql =
            snowflake_freshness_sql(" RAW ", &["table_schema = 'PUBLIC'".to_string()]).unwrap();

        assert!(sql.contains("FROM RAW.INFORMATION_SCHEMA.TABLES"));
    }

    #[test]
    fn snowflake_freshness_sql_builds_broad_in_scan() {
        // Mirrors how `freshness_all_in_schemas` composes the broad multi-schema
        // predicate for a small database.
        let sql =
            snowflake_freshness_sql("RAW", &["table_schema IN ('S0', 'S1', 'S2')".to_string()])
                .unwrap();

        assert!(sql.contains("FROM RAW.INFORMATION_SCHEMA.TABLES"));
        assert!(sql.contains("WHERE table_schema IN ('S0', 'S1', 'S2')"));
    }

    #[test]
    fn snowflake_schema_literal_escapes_single_quotes() {
        assert_eq!(snowflake_schema_literal("PUBLIC"), "'PUBLIC'");
        // A schema name containing `'` is escaped by doubling so the string
        // literal stays well-formed.
        let schema = snowflake_schema_literal("o'brien");
        assert_eq!(schema, "'o''brien'");

        let sql = snowflake_freshness_sql("RAW", &[format!("table_schema = {schema}")]).unwrap();
        assert!(sql.contains("WHERE table_schema = 'o''brien'"));
    }

    #[test]
    fn snowflake_schema_count_sql_escapes_double_quotes() {
        assert_eq!(
            snowflake_schema_count_sql("DB", 1420),
            r#"SHOW TERSE SCHEMAS IN DATABASE "DB" LIMIT 1420"#
        );
        // A database name containing `"` is escaped by doubling so the identifier
        // stays well-formed.
        assert_eq!(
            snowflake_schema_count_sql(r#"a"b"#, 100),
            r#"SHOW TERSE SCHEMAS IN DATABASE "a""b" LIMIT 100"#
        );
    }

    #[test]
    fn schema_probe_limit_scales_and_clamps() {
        // D=5 -> 284 * 5 = 1420, below the 10k cap.
        assert_eq!(schema_probe_limit(5), 1420);
        // D=36 -> 284 * 36 = 10224, clamped to Snowflake's SHOW LIMIT cap.
        assert_eq!(schema_probe_limit(36), MAX_SHOW_LIMIT);
        assert_eq!(schema_probe_limit(36), 10000);
    }

    #[test]
    fn crossover_coefficient_matches_measured_rates() {
        // Derived in code from the two measured curves; documented as ~284.
        assert_eq!(CROSSOVER_N_PER_FETCHED_SCHEMA, 284);
    }

    #[test]
    fn schema_probe_decision_small_db_uses_sequential_without_probe() {
        // At or below ALWAYS_SEQUENTIAL_MAX_SCHEMAS the probe is skipped: the
        // decision is sequential regardless of any observed count.
        assert!(schema_probe_decision(1, None));
        assert!(schema_probe_decision(ALWAYS_SEQUENTIAL_MAX_SCHEMAS, None));
        assert!(schema_probe_decision(
            ALWAYS_SEQUENTIAL_MAX_SCHEMAS,
            Some(0)
        ));
    }

    #[test]
    fn schema_probe_decision_probe_failure_falls_back_to_broad_scan() {
        // D above the short-circuit and no observed count (probe error) -> broad IN scan.
        assert!(!schema_probe_decision(
            ALWAYS_SEQUENTIAL_MAX_SCHEMAS + 1,
            None
        ));
    }

    #[test]
    fn schema_probe_decision_saturated_probe_uses_sequential() {
        // D=5 -> threshold 284*5 = 1420. Saturated (observed == threshold) -> sequential.
        assert!(schema_probe_decision(5, Some(1420)));
        // Just short of saturation -> the DB is small enough for one broad scan.
        assert!(!schema_probe_decision(5, Some(1419)));
    }

    #[test]
    fn schema_probe_decision_clamped_threshold_saturation() {
        // D=36 -> 284*36 = 10224, clamped to MAX_SHOW_LIMIT (10000). Saturation is
        // measured against the clamped threshold.
        assert!(schema_probe_decision(36, Some(MAX_SHOW_LIMIT)));
        assert!(!schema_probe_decision(36, Some(MAX_SHOW_LIMIT - 1)));
    }

    #[test]
    fn require_snowflake_metadata_component_returns_trimmed_value() {
        let schema = require_snowflake_metadata_component("schema", " PUBLIC ").unwrap();

        assert_eq!(schema, "PUBLIC");
    }

    #[test]
    fn is_table_ddl_recognizes_create_table() {
        assert!(is_table_ddl("CREATE TABLE foo (x INT)"));
    }
    #[test]
    fn is_table_ddl_recognizes_transient_table() {
        assert!(is_table_ddl("CREATE TRANSIENT TABLE foo (x INT)"));
    }
    #[test]
    fn is_table_ddl_recognizes_or_replace_table() {
        assert!(is_table_ddl("CREATE OR REPLACE TABLE foo (x INT)"));
    }
    #[test]
    fn is_table_ddl_rejects_create_view() {
        assert!(!is_table_ddl("CREATE VIEW foo AS SELECT 1"));
    }
    #[test]
    fn is_table_ddl_rejects_or_replace_view() {
        assert!(!is_table_ddl("CREATE OR REPLACE VIEW foo AS SELECT 1"));
    }
    #[test]
    fn is_table_ddl_rejects_empty() {
        assert!(!is_table_ddl(""));
        assert!(!is_table_ddl("   "));
    }
    #[test]
    fn is_table_ddl_rejects_view_with_table_function_in_body() {
        // The TABLE keyword appears in the body of the view's SELECT — the
        // header still says VIEW, so this must not be misclassified as a table.
        assert!(!is_table_ddl(
            "CREATE VIEW foo AS SELECT * FROM TABLE(generator(rowcount => 10))"
        ));
        assert!(!is_table_ddl(
            "CREATE OR REPLACE VIEW foo AS SELECT * FROM TABLE(generator(rowcount => 10))"
        ));
    }
    #[test]
    fn is_table_ddl_rejects_view_referencing_table_in_from() {
        // Same idea but with a plain FROM clause — `tables` here is part of
        // an INFORMATION_SCHEMA reference, not a header keyword.
        assert!(!is_table_ddl(
            "CREATE VIEW foo AS SELECT * FROM information_schema.tables"
        ));
    }
    #[test]
    fn is_table_ddl_recognizes_dynamic_table() {
        assert!(is_table_ddl(
            "CREATE OR REPLACE DYNAMIC TABLE foo TARGET_LAG = '1 minute' WAREHOUSE = w AS SELECT 1"
        ));
    }
    #[test]
    fn is_table_ddl_recognizes_external_table() {
        assert!(is_table_ddl(
            "CREATE EXTERNAL TABLE foo WITH LOCATION = '@stage' FILE_FORMAT = (TYPE = CSV)"
        ));
    }
    #[test]
    fn is_table_ddl_rejects_materialized_view() {
        assert!(!is_table_ddl(
            "CREATE MATERIALIZED VIEW foo AS SELECT * FROM bar"
        ));
    }

    #[test]
    fn view_definition_fetch_result_marks_null_definition_unresolvable() {
        let batch = view_definition_batch(vec![
            (
                r#""DB1"."SCHEMA1"."SECURE_VIEW""#,
                None,
                Some("Object does not exist or not authorized."),
            ),
            (
                r#""DB1"."SCHEMA1"."TABLE1""#,
                Some("CREATE TABLE table1 (id INT)"),
                None,
            ),
        ]);
        let mut result = ViewDefinitionFetchResult::default();

        accumulate_view_definition_fetch_result(&mut result, &batch).expect("parse batch");

        assert!(result.definitions.is_empty());
        assert_eq!(
            result.unresolvable,
            BTreeSet::from([r#""DB1"."SCHEMA1"."SECURE_VIEW""#.to_string()])
        );
    }

    #[test]
    fn view_definition_fetch_result_accumulates_unresolvable_across_batches() {
        let batch1 = view_definition_batch(vec![
            (
                r#""DB1"."SCHEMA1"."SECURE_VIEW""#,
                None,
                Some("not authorized"),
            ),
            (
                r#""DB1"."SCHEMA1"."READABLE_VIEW""#,
                Some(r#"CREATE VIEW readable_view AS SELECT * FROM "DB1"."SCHEMA1"."BASE_T""#),
                None,
            ),
        ]);
        let batch2 = view_definition_batch(vec![(
            r#""DB1"."SCHEMA1"."BASE_T""#,
            Some("CREATE TABLE base_t (id INT)"),
            None,
        )]);
        let mut result = ViewDefinitionFetchResult::default();

        accumulate_view_definition_fetch_result(&mut result, &batch1).expect("parse first batch");
        accumulate_view_definition_fetch_result(&mut result, &batch2).expect("parse second batch");

        assert_eq!(
            result.unresolvable,
            BTreeSet::from([r#""DB1"."SCHEMA1"."SECURE_VIEW""#.to_string()])
        );
        assert_eq!(result.definitions.len(), 1);
        assert_eq!(
            result.definitions[0].fqn,
            r#""DB1"."SCHEMA1"."READABLE_VIEW""#
        );
    }

    #[test]
    fn interactive_flag_takes_precedence_over_dynamic() {
        assert_eq!(
            relation_type_from_table_flags("y", "y").unwrap(),
            RelationType::InteractiveTable
        );
        // static IT: is_dynamic=n, is_interactive=y
        assert_eq!(
            relation_type_from_table_flags("n", "y").unwrap(),
            RelationType::InteractiveTable
        );
        assert_eq!(
            relation_type_from_table_flags("y", "n").unwrap(),
            RelationType::DynamicTable
        );
    }

    #[test]
    fn build_view_definition_script_default_get_ddl() {
        let fqns = vec![
            r#""DB"."S"."V1""#.to_string(),
            r#""DB"."S"."V2""#.to_string(),
        ];
        let script = build_view_definition_script(&fqns);
        assert!(
            script.contains("get_ddl('VIEW', :obj_name)"),
            "got: {script}"
        );
        assert!(script.contains(r#""DB"."S"."V1""#));
        assert!(script.contains(r#""DB"."S"."V2""#));
        assert!(script.contains("array_construct"));
        assert!(script.contains("OBJECT_NAME"));
        assert!(script.contains("DEFINITION"));
        assert!(script.contains("ERROR"));
    }
}
