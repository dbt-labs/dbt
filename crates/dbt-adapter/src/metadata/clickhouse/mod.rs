use crate::AdapterEngine;
use crate::adapter::adapter_impl::{AdapterImpl, InnerAdapter};
use crate::column::Column;
use crate::connection::AdapterConnectionFactory;
use crate::query_ctx::{node_id_from_state, query_ctx_from_state};
use crate::record_batch::RecordBatchExt;
use crate::relation::Relation;
use crate::sql_types::{TypeOps, make_arrow_field_v2};
use crate::{AdapterResult, errors::AsyncAdapterResult, metadata::*};
use arrow_schema::Schema;

use arrow_array::{Array, RecordBatch, StringArray, UInt64Array};

use dbt_adapter_core::ExecutionPhase;
use dbt_adapter_engine::MapReduce;
use dbt_adapter_sql::ident::quote_identifier;
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::cancellation::Cancellable;
use dbt_common::cancellation::CancellationToken;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::{
    legacy_catalog::{CatalogNodeStats, CatalogTable, ColumnMetadata, TableMetadata},
    relations::base::{BaseRelation, RelationPattern},
};
use indexmap::IndexMap;
use minijinja::State;
use minijinja::Value;
use minijinja::value::ValueKind;
use std::collections::btree_map::Entry;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Escape a value to be safely interpolated inside a single-quoted ClickHouse
/// string literal. ClickHouse uses backslash escaping for `\` and `'` within
/// string literals (see <https://clickhouse.com/docs/en/sql-reference/syntax#string>).
pub(crate) fn escape_clickhouse_string_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Mirrors dbt-clickhouse util.py `engine_can_atomic_exchange`.
pub(crate) fn engine_can_atomic_exchange(engine: &str) -> bool {
    matches!(engine, "Atomic" | "Replicated" | "Shared")
}

/// MV target `db.table` regex from the upstream `clickhouse__list_relations_without_caching`
/// macro. The `?` quantifiers are `\x3F` hex escapes: the ADBC driver treats a literal `?`
/// anywhere in the SQL, even inside string literals, as a bind parameter.
const MV_TARGET_FQN_REGEX_SQL: &str =
    r"'.*TO\\s+`\x3F([^`\\s(]+)`\x3F\\.`\x3F([^`\\s(]+)`\x3F.*', '\\1.\\2'";

/// The `is_refreshable`/`refreshable_append` columns of the upstream
/// `clickhouse__list_relations_without_caching` macro: the REFRESH clause sits between
/// the view name and TO (`CREATE MATERIALIZED VIEW db.mv REFRESH EVERY 2 MINUTE [APPEND] TO …`).
fn refreshable_marker_columns(col: &str, agg: &str) -> String {
    let clause = |marker: &str| {
        format!("{agg}(position(substring({col}, 1, position({col}, ' TO ')), '{marker}')) > 0")
    };
    format!(
        "if({}, 'true', 'false') AS is_refreshable, \
         if({}, 'true', 'false') AS refreshable_append",
        clause(" REFRESH "),
        clause(" APPEND")
    )
}

pub(crate) fn build_get_relation_sql(schema: &str, identifier: &str) -> String {
    let escaped_database = escape_clickhouse_string_literal(schema);
    let escaped_identifier = escape_clickhouse_string_literal(identifier);
    let refreshable_columns = refreshable_marker_columns("create_table_query", "");
    format!(
        "SELECT engine, name, \
                (SELECT engine FROM system.databases WHERE name = '{escaped_database}') AS database_engine, \
                (SELECT toJSONString(groupArray(map('schema', database, 'name', name, 'sql', as_select))) \
                 FROM system.tables \
                 WHERE engine = 'MaterializedView' AND create_table_query LIKE '%TO %' \
                   AND replaceRegexpOne(create_table_query, {MV_TARGET_FQN_REGEX_SQL}) = '{escaped_database}.{escaped_identifier}') AS mvs_pointing_to_it, \
                {refreshable_columns} \
         FROM system.tables \
         WHERE database = '{escaped_database}' \
           AND name = '{escaped_identifier}'",
    )
}

/// impl.py `list_relations_without_caching`:
/// `can_exchange = conn_supports_exchange and rel_type == Table and engine_can_atomic_exchange(db_engine)`.
pub(crate) fn relation_can_exchange(
    caps: &ClickHouseCapabilities,
    relation_type: RelationType,
    database_engine: &str,
) -> bool {
    caps.atomic_exchange
        && relation_type == RelationType::Table
        && engine_can_atomic_exchange(database_engine)
}

/// Mirrors impl.py `json.loads(mvs_pointing_to_it) or []`.
pub(crate) fn parse_mvs_pointing_to_it(json: &str) -> Vec<BTreeMap<String, String>> {
    serde_json::from_str::<Vec<BTreeMap<String, String>>>(json).unwrap_or_default()
}

/// Stamps the state columns shared by [`build_get_relation_sql`] and [`list_relations`]
/// (impl.py `list_relations_without_caching`).
pub(crate) fn with_catalog_state(
    relation: Relation,
    caps: &ClickHouseCapabilities,
    batch: &RecordBatch,
    row: usize,
) -> AdapterResult<Relation> {
    let column = |name: &str| batch.column_values::<StringArray>(name);
    let database_engines = column("database_engine")?;
    let can_exchange = relation.relation_type.is_some_and(|relation_type| {
        relation_can_exchange(caps, relation_type, database_engines.value(row))
    });
    relation
        .with_can_exchange(can_exchange)
        .with_mvs_pointing_to_it(parse_mvs_pointing_to_it(
            column("mvs_pointing_to_it")?.value(row),
        ))
        .with_is_refreshable(column("is_refreshable")?.value(row) == "true")
        .with_refreshable_append(column("refreshable_append")?.value(row) == "true")
        .validate()
        .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))
}

pub struct ClickHouseMetadataAdapter {
    adapter: AdapterImpl,
}

impl ClickHouseMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }
}

impl MetadataAdapter for ClickHouseMetadataAdapter {
    fn adapter_type(&self) -> AdapterType {
        self.adapter.adapter_type()
    }

    /// Parse the record batch produced by the `clickhouse__get_catalog*` macros
    /// into per-relation table metadata.
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
                    columns: IndexMap::new(),
                    stats: BTreeMap::from([("has_stats".to_string(), no_stats)]),
                    unique_id: None,
                };
                result.insert(fully_qualified_name.clone(), node);
            }
        }
        Ok(result)
    }

    /// Parse the record batch produced by the `clickhouse__get_catalog*` macros
    /// into per-relation column metadata.
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
        let column_indices = stats_sql_result.column_values::<UInt64Array>("column_index")?;
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

        // ClickHouse is a 2-part name system: dbt `schema` maps to CH `database`,
        // and the dbt `database` field is unused/empty. The DESCRIBE TABLE SQL must
        // use `schema.identifier` (unquoted), but the HashMap key that callers look
        // up via `schemas.get(&semantic_fqn)` must match `relation.semantic_fqn()`.
        // These two are different strings, so we carry both through MapReduce as a
        // tuple `(semantic_fqn, sql_name)`.
        let keys: Vec<(String, String)> = relations
            .iter()
            .map(|relation| {
                let semantic_fqn = relation.semantic_fqn();
                let schema = relation.schema_as_str().unwrap_or_default();
                let identifier = relation.identifier_as_str().unwrap_or_default();
                let sql_name = if schema.is_empty() {
                    identifier
                } else {
                    format!("{schema}.{identifier}")
                };
                (semantic_fqn, sql_name)
            })
            .collect();

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          key: &(String, String)|
              -> AdapterResult<Arc<Schema>> {
            let (_semantic_fqn, sql_name) = key;
            // ClickHouse DESCRIBE TABLE returns: name, type, default_type, default_expression,
            // comment, codec_expression, ttl_expression
            let sql = format!("DESCRIBE TABLE {};", sql_name);
            let mut ctx = QueryCtx::default().with_desc("Get table schema");
            if let Some(node_id) = unique_id.clone() {
                ctx = ctx.with_node_id(&node_id);
            }
            if let Some(phase) = phase {
                ctx = ctx.with_phase(phase.as_str());
            }
            let (_, table) = adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            let batch = table.original_record_batch();
            let schema =
                build_schema_from_clickhouse_describe(batch, adapter.engine().type_ops().as_ref())?;
            Ok(schema)
        };

        let reduce_f = |acc: &mut Acc,
                        key: (String, String),
                        schema: AdapterResult<Arc<Schema>>|
         -> Result<(), Cancellable<AdapterError>> {
            let (semantic_fqn, _sql_name) = key;
            // Insert under the semantic FQN so callers using `schemas.get(&relation.semantic_fqn())`
            // can find the entry.
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

    fn list_relations_schemas_by_patterns_inner(
        &self,
        _patterns: &[RelationPattern],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        todo!("ClickHouseAdapter::list_relations_schemas_by_patterns")
    }

    fn freshness_inner(
        &self,
        _relations: &[Arc<dyn BaseRelation>],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        todo!("ClickHouseAdapter::freshness")
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
    ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        type Acc = BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>;

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          db_schema: &CatalogAndSchema|
              -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
            let ctx = QueryCtx::default().with_desc("list_relations_in_parallel");
            list_relations(
                adapter.engine().as_ref(),
                &ctx,
                conn,
                db_schema,
                token_clone.clone(),
            )
        };

        let reduce_f = move |acc: &mut Acc,
                             db_schema: CatalogAndSchema,
                             relations: AdapterResult<Vec<Arc<dyn BaseRelation>>>|
              -> Result<(), Cancellable<AdapterError>> {
            match &relations {
                Ok(_) => {
                    acc.insert(db_schema, relations);
                }
                Err(e) => {
                    // Treat missing database as empty so callers can hydrate caches
                    // without erroring out on schemas that haven't been created yet.
                    if e.message().contains("doesn't exist")
                        || e.message().contains("does not exist")
                    {
                        acc.insert(db_schema, Ok(Vec::new()));
                    } else {
                        return Err(Cancellable::Error(AdapterError::new(
                            AdapterErrorKind::Internal,
                            e.message(),
                        )));
                    }
                }
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(db_schemas.to_vec()), token)
    }
}

/// List all relations (tables, views, materialized views, dictionaries) in a given database.
///
/// Queries ClickHouse's `system.tables` and maps the engine name to a [`RelationType`].
pub fn list_relations(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    // ClickHouse only has databases, no schemas — we map dbt `schema` to CH `database`.
    let escaped_database = escape_clickhouse_string_literal(&db_schema.resolved_schema);
    let refreshable_columns = refreshable_marker_columns("t.create_table_query", "max");
    let sql = format!(
        "WITH mv_sources AS ( \
            SELECT name AS mv_name, \
                   database AS mv_database, \
                   any(as_select) AS mv_sql, \
                   any(replaceRegexpOne(create_table_query, {MV_TARGET_FQN_REGEX_SQL})) AS target_fqn \
            FROM system.tables \
            WHERE engine = 'MaterializedView' AND create_table_query LIKE '%TO %' \
            GROUP BY mv_name, mv_database \
         ) \
         SELECT t.database AS table_database, \
                t.name AS table_name, \
                t.engine AS table_type, \
                db.engine AS database_engine, \
                toJSONString(groupArrayIf( \
                    map('schema', mv_sources.mv_database, 'name', mv_sources.mv_name, 'sql', mv_sources.mv_sql), \
                    mv_sources.mv_name != '' \
                )) AS mvs_pointing_to_it, \
                {refreshable_columns} \
         FROM system.tables AS t \
         JOIN system.databases AS db ON t.database = db.name \
         LEFT JOIN mv_sources ON mv_sources.target_fqn = concat(t.database, '.', t.name) \
         WHERE t.database = '{escaped_database}' \
         GROUP BY table_database, table_name, table_type, database_engine"
    );

    let caps = server_capabilities_over_connection(engine, ctx, conn, token.clone());

    let batch = engine.execute(None, conn, ctx, &sql, token)?;

    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let table_databases = batch.column_values::<StringArray>("table_database")?;
    let table_names = batch.column_values::<StringArray>("table_name")?;
    let table_types = batch.column_values::<StringArray>("table_type")?;

    let mut relations = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        let relation = Relation::new(
            engine.adapter_type(),
            Some(db_schema.resolved_catalog.clone()),
            Some(table_databases.value(i).to_string()),
            Some(table_names.value(i).to_string()),
        )
        .with_relation_type(relation_type_from_engine(table_types.value(i)))
        .with_quoting(engine.quoting());
        let relation = with_catalog_state(relation, caps, &batch, i)?;

        relations.push(Arc::new(relation) as Arc<dyn BaseRelation>);
    }

    Ok(relations)
}

/// Map a ClickHouse `system.tables.engine` value to a dbt [`RelationType`].
///
/// MergeTree-family engines (and their Replicated/Versioned variants) are tables;
/// `View` and `LiveView` are views; `MaterializedView` and `Dictionary` get their
/// own kinds.
pub fn relation_type_from_engine(engine_name: &str) -> RelationType {
    match engine_name {
        "View" | "LiveView" => RelationType::View,
        "MaterializedView" => RelationType::MaterializedView,
        // Anything else (MergeTree, ReplacingMergeTree, AggregatingMergeTree,
        // SummingMergeTree, CollapsingMergeTree, VersionedCollapsingMergeTree,
        // GraphiteMergeTree, Replicated*, Distributed, Memory, Log, etc.) is a table.
        _ => RelationType::Table,
    }
}

/// The `(name, type)` pairs from a `DESCRIBE TABLE` result batch (which also
/// carries default/comment/codec/ttl columns; only name/type are consumed).
fn describe_name_type_pairs(batch: &RecordBatch) -> AdapterResult<Vec<(String, String)>> {
    let names = batch.column_values::<StringArray>("name")?;
    let types = batch.column_values::<StringArray>("type")?;
    Ok((0..batch.num_rows())
        .map(|i| (names.value(i).to_string(), types.value(i).to_string()))
        .collect())
}

/// Column names and ClickHouse type text of an arbitrary SELECT, via
/// `DESCRIBE TABLE (<sql>)`. DESCRIBE preserves the server's own type text —
/// the ADBC/Arrow result schema marks every column Nullable, which breaks
/// downstream DDL and contract/type comparisons. `settings_clause` (rendered
/// by [`query_settings_clause`]) makes introspected types match runtime
/// settings such as `join_use_nulls`.
pub fn describe_query_columns(
    engine: &dyn AdapterEngine,
    state: Option<&State>,
    conn: &mut dyn Connection,
    ctx: &QueryCtx,
    sql: &str,
    settings_clause: &str,
    token: CancellationToken,
) -> AdapterResult<Vec<(String, String)>> {
    // The closing paren goes on its own line: model SQL routinely ends with a
    // `-- line comment` (no trailing newline), which would otherwise swallow
    // the `)` and break the query.
    let batch = engine.execute(
        state,
        conn,
        ctx,
        &format!("DESCRIBE TABLE ({sql}\n){settings_clause}"),
        token,
    )?;
    describe_name_type_pairs(&batch)
}

/// Build an Arrow Schema from ClickHouse's `DESCRIBE TABLE` output.
fn build_schema_from_clickhouse_describe(
    describe_result: Arc<RecordBatch>,
    type_ops: &dyn TypeOps,
) -> AdapterResult<Arc<Schema>> {
    let mut fields = vec![];
    for (name, text_data_type) in describe_name_type_pairs(&describe_result)? {
        // ClickHouse encodes nullability via the `Nullable(...)` wrapper in the
        // type text; the type parser handles that and the Arrow nullable bit is
        // derived from there.
        let field = make_arrow_field_v2(type_ops, name, &text_data_type, None, None)?;
        fields.push(field);
    }

    let schema = Schema::new(fields);
    Ok(Arc::new(schema))
}

/// ClickHouse server capabilities, probed once per process and reused for every
/// model/thread afterwards (as the Python client does at connection init, and
/// for `atomic_exchange` behind a process-global lock).
#[derive(Debug, Clone)]
pub struct ClickHouseCapabilities {
    /// `SELECT version()` result; empty when the probe failed (callers treat
    /// unknown as "modern server").
    pub server_version: String,
    /// Whether atomic `EXCHANGE TABLES` works on this server/database
    /// (dbclient.py `_check_atomic_exchange`).
    pub atomic_exchange: bool,
    /// Whether lightweight deletes are usable. Mirrors dbclient.py
    /// `_check_lightweight_deletes`.
    pub has_lw_deletes: bool,
    /// `has_lw_deletes` gated by the profile `use_lw_deletes` flag; picks the
    /// default incremental strategy.
    pub use_lw_deletes: bool,
    /// The ND-mutation setting is disabled server-side but overridable; see
    /// [`statement_options`].
    pub nd_mutation_override: bool,
}

impl ClickHouseCapabilities {
    /// Whether the connected server is older than `version`; an unknown
    /// server version is treated as modern (false).
    pub fn is_before(&self, version: &str) -> AdapterResult<bool> {
        if self.server_version.is_empty() {
            return Ok(false);
        }
        Ok(compare_versions(version, &self.server_version)? > 0)
    }

    /// Inclusive counterpart of [`ClickHouseCapabilities::is_before`]; an
    /// unknown server version is treated as modern (true).
    pub fn is_at_or_after(&self, version: &str) -> AdapterResult<bool> {
        if self.server_version.is_empty() {
            return Ok(true);
        }
        Ok(compare_versions(&self.server_version, version)? >= 0)
    }
}

/// Mirrors dbt-clickhouse util.py `compare_versions`: 1/-1/0 comparing dotted
/// numeric versions segment by segment; errors on non-numeric segments.
fn compare_versions(v1: &str, v2: &str) -> AdapterResult<i32> {
    for (part1, part2) in v1.split('.').zip(v2.split('.')) {
        match (part1.parse::<i64>(), part2.parse::<i64>()) {
            (Ok(a), Ok(b)) => {
                if a != b {
                    return Ok(if a > b { 1 } else { -1 });
                }
            }
            _ => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "Version must consist of only numbers separated by '.'",
                ));
            }
        }
    }
    Ok(0)
}

/// Read a dict-valued key (e.g. `settings`, `query_settings`) from
/// `model['config']`, preserving iteration order. Missing/undefined -> empty.
pub(crate) fn model_config_map(model: &Value, key: &str) -> Vec<(String, Value)> {
    let Ok(config) = model.get_attr("config") else {
        return Vec::new();
    };
    let Ok(map) = config.get_attr(key) else {
        return Vec::new();
    };
    if map.is_none() || map.is_undefined() {
        return Vec::new();
    }
    let Ok(keys) = map.try_iter() else {
        return Vec::new();
    };
    keys.filter_map(|k| {
        let name = k.as_str()?.to_string();
        let value = map.get_item(&k).ok()?;
        Some((name, value))
    })
    .collect()
}

/// Mirrors dbt-clickhouse impl.py `_build_settings_str`: string values not
/// already single-quoted get single-quoted, other values are emitted verbatim;
/// '' for no entries, otherwise newline-terminated.
pub(crate) fn build_settings_str(settings: &[(String, Value)]) -> String {
    let res: Vec<String> = settings
        .iter()
        .map(|(key, value)| match value.as_str() {
            Some(s) if !s.starts_with('\'') => format!("{key}='{s}'"),
            _ => format!("{key}={value}"),
        })
        .collect();
    if res.is_empty() {
        String::new()
    } else {
        format!("SETTINGS {}\n", res.join(", "))
    }
}

/// Mirrors ClickHouse impl.py `format_columns`:
/// `[{'name': c.name, 'data_type': c.data_type}, ...]`.
pub(crate) fn format_columns(columns: &Value) -> Value {
    let mut out: Vec<Value> = Vec::new();
    if let Ok(items) = columns.try_iter() {
        for column in items {
            let mut map: BTreeMap<String, Value> = BTreeMap::new();
            map.insert(
                "name".to_string(),
                column.get_attr("name").unwrap_or_default(),
            );
            map.insert(
                "data_type".to_string(),
                column.get_attr("data_type").unwrap_or_default(),
            );
            out.push(Value::from(map));
        }
    }
    Value::from(out)
}

/// impl.py: `s3config = {**self.config.vars.vars.get(config_name, {}), **s3_model_config}`
pub(crate) fn merge_s3_config(vars_config: Option<&Value>, model_config: Option<&Value>) -> Value {
    let mut merged: BTreeMap<String, Value> = BTreeMap::new();
    for config in [vars_config, model_config].into_iter().flatten() {
        if let Ok(keys) = config.try_iter() {
            for key in keys {
                if let (Some(name), Ok(value)) = (key.as_str(), config.get_item(&key)) {
                    merged.insert(name.to_string(), value);
                }
            }
        }
    }
    Value::from(merged)
}

/// Mirrors dbt-clickhouse impl.py `s3source_clause`: build the `s3(...)` table function.
#[allow(clippy::too_many_arguments)]
pub(crate) fn s3source_clause(
    s3config: &Value,
    structure: &Value,
    bucket: &str,
    path: &str,
    fmt: &str,
    aws_access_key_id: &str,
    aws_secret_access_key: &str,
    role_arn: &str,
    compression: &str,
    external_id: &str,
) -> AdapterResult<String> {
    let cfg_str = |key: &str| -> String {
        match s3config.get_attr(key) {
            // Python `s3config.get(key, '')` stringifies non-string values too.
            Ok(v) if !v.is_undefined() && !v.is_none() => match v.as_str() {
                Some(s) => s.to_string(),
                None => v.to_string(),
            },
            _ => String::new(),
        }
    };
    // impl.py: `arg or s3config.get(key, '')`
    let or_cfg = |arg: &str, key: &str| -> String {
        if arg.is_empty() {
            cfg_str(key)
        } else {
            arg.to_string()
        }
    };

    // impl.py-exact shapes: dict -> ", 'name type,...'"; list -> ", 'a,b'";
    // plain string -> ",'...'" (no space).
    let structure = if structure.is_true() {
        structure.clone()
    } else {
        s3config.get_attr("structure").unwrap_or_default()
    };
    let mut struct_clause = if structure.is_true() {
        match structure.kind() {
            ValueKind::Map => {
                let mut cols = Vec::new();
                if let Ok(keys) = structure.try_iter() {
                    for key in keys {
                        let col_type = structure
                            .get_item(&key)
                            .ok()
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        cols.push(format!("{key} {col_type}"));
                    }
                }
                format!(", '{}'", cols.join(","))
            }
            ValueKind::Seq => {
                let mut cols = Vec::new();
                if let Ok(items) = structure.try_iter() {
                    for item in items {
                        cols.push(item.to_string());
                    }
                }
                format!(", '{}'", cols.join(","))
            }
            _ => format!(",'{structure}'"),
        }
    } else {
        String::new()
    };

    let fmt = or_cfg(fmt, "fmt");

    let bucket = or_cfg(bucket, "bucket");
    let mut path = or_cfg(path, "path");
    let mut url = bucket.replace("https://", "");
    if !path.is_empty() {
        if !bucket.is_empty() && !bucket.ends_with('/') && !bucket.starts_with('/') {
            path = format!("/{path}");
        }
        url = format!("{url}{path}").replace("//", "/");
    }
    let url = format!("https://{url}");

    let aws_access_key_id = or_cfg(aws_access_key_id, "aws_access_key_id");
    let aws_secret_access_key = or_cfg(aws_secret_access_key, "aws_secret_access_key");
    if !aws_access_key_id.is_empty() && aws_secret_access_key.is_empty() {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "S3 aws_access_key_id specified without aws_secret_access_key",
        ));
    }
    if !aws_secret_access_key.is_empty() && aws_access_key_id.is_empty() {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "S3 aws_secret_access_key specified without aws_access_key_id",
        ));
    }
    let access = if aws_access_key_id.is_empty() {
        String::new()
    } else {
        format!(", '{aws_access_key_id}', '{aws_secret_access_key}'")
    };

    // impl.py: compression forces an empty structure placeholder (s3() args are positional).
    let comp = or_cfg(compression, "compression");
    let comp = if comp.is_empty() {
        String::new()
    } else {
        if struct_clause.is_empty() {
            struct_clause = ", ''".to_string();
        }
        format!(", '{comp}'")
    };

    let role_arn = or_cfg(role_arn, "role_arn");
    let external_id = or_cfg(external_id, "external_id");
    if !external_id.is_empty() && role_arn.is_empty() {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "S3 external_id specified without role_arn",
        ));
    }
    let extra_credentials = if role_arn.is_empty() {
        String::new()
    } else if external_id.is_empty() {
        format!(", extra_credentials(role_arn='{role_arn}')")
    } else {
        format!(", extra_credentials(role_arn='{role_arn}', external_id='{external_id}')")
    };

    Ok(format!(
        "s3('{url}'{access}, '{fmt}'{struct_clause}{comp}{extra_credentials})"
    ))
}

/// Render a `query_settings` map (from macro kwargs) into a `" SETTINGS k=v, ..."`
/// suffix for introspection queries; empty string when absent or empty.
pub(crate) fn query_settings_clause(query_settings: Option<&Value>) -> String {
    let Some(qs) = query_settings else {
        return String::new();
    };
    let mut pairs: Vec<(String, Value)> = Vec::new();
    if let Ok(keys) = qs.try_iter() {
        for key in keys {
            if let (Some(name), Ok(value)) = (key.as_str(), qs.get_item(&key)) {
                pairs.push((name.to_string(), value));
            }
        }
    }
    let rendered = build_settings_str(&pairs);
    if rendered.is_empty() {
        String::new()
    } else {
        format!(" {}", rendered.trim_end())
    }
}

static CLICKHOUSE_CAPABILITIES: std::sync::OnceLock<ClickHouseCapabilities> =
    std::sync::OnceLock::new();

/// Return the server capabilities, probing only on the very first call
/// process-wide (concurrent callers block until it completes).
pub fn server_capabilities(
    adapter: &AdapterImpl,
    state: &State,
    token: CancellationToken,
) -> &'static ClickHouseCapabilities {
    CLICKHOUSE_CAPABILITIES.get_or_init(|| {
        probe_capabilities(adapter.engine().as_ref(), |sql| {
            probe_query(adapter, state, sql, token.clone())
        })
    })
}

/// [`server_capabilities`] for the catalog paths, which already hold a connection and
/// have no Jinja `State`.
pub fn server_capabilities_over_connection(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &mut dyn Connection,
    token: CancellationToken,
) -> &'static ClickHouseCapabilities {
    CLICKHOUSE_CAPABILITIES.get_or_init(|| {
        probe_capabilities(engine, |sql| {
            engine.execute(None, conn, ctx, sql, token.clone()).ok()
        })
    })
}

fn probe_capabilities(
    engine: &dyn AdapterEngine,
    mut run: impl FnMut(&str) -> Option<RecordBatch>,
) -> ClickHouseCapabilities {
    let flag = |key: &str| {
        engine
            .get_config()
            .get(key)
            .and_then(|value| value.as_bool())
    };
    let lw_deletes = probe_lightweight_deletes(&mut run);
    ClickHouseCapabilities {
        server_version: probe_server_version(&mut run).unwrap_or_default(),
        // dbclient.py: `not check_exchange or _check_atomic_exchange()`
        atomic_exchange: !flag("check_exchange").unwrap_or(true) || probe_atomic_exchange(&mut run),
        has_lw_deletes: lw_deletes.has_lw_deletes,
        use_lw_deletes: lw_deletes.has_lw_deletes && flag("use_lw_deletes").unwrap_or(false),
        nd_mutation_override: lw_deletes.nd_mutation_override,
    }
}

/// The probed capabilities, or None — for callers that must not trigger the
/// probe themselves.
fn capabilities_if_probed() -> Option<&'static ClickHouseCapabilities> {
    CLICKHOUSE_CAPABILITIES.get()
}

/// dbclient.py parity: after the lightweight-deletes probe finds
/// `ND_MUTATION_SETTING` disabled but overridable, every statement carries it
/// as a driver setting (Python adds it to the per-request `_conn_settings`).
pub(crate) fn statement_options() -> Option<(String, adbc_core::options::OptionValue)> {
    capabilities_if_probed()
        .filter(|caps| caps.nd_mutation_override)
        .map(|_| {
            (
                dbt_adbc::clickhouse::setting_key(ND_MUTATION_SETTING),
                adbc_core::options::OptionValue::String("1".to_string()),
            )
        })
}

/// Run a capability-probe query; None when the probe cannot run (replay has no
/// live connection) or the query fails.
fn probe_query(
    adapter: &AdapterImpl,
    state: &State,
    sql: &str,
    token: CancellationToken,
) -> Option<RecordBatch> {
    let InnerAdapter::Impl(_, engine) = adapter.inner_adapter() else {
        return None;
    };
    let engine = Arc::clone(engine);
    let ctx = query_ctx_from_state(state)
        .ok()?
        .with_desc("clickhouse capability probe");
    let mut conn = adapter
        .borrow_tlocal_connection(Some(state), node_id_from_state(state))
        .ok()?;
    engine.execute(None, conn.as_mut(), &ctx, sql, token).ok()
}

fn scalar_probe(run: &mut impl FnMut(&str) -> Option<RecordBatch>, sql: &str) -> Option<String> {
    let batch = run(sql)?;
    if batch.num_rows() == 0 {
        return None;
    }
    let values = batch.column_values::<StringArray>("v").ok()?;
    Some(values.value(0).to_string())
}

/// `SELECT version()`; None leaves the version unknown -> "assume modern server".
fn probe_server_version(run: &mut impl FnMut(&str) -> Option<RecordBatch>) -> Option<String> {
    scalar_probe(run, "SELECT version() AS v")
}

/// Mirrors dbclient.py `_run_exchange_test` (`Shared` = ClickHouse Cloud, exchange
/// guaranteed without probing).
fn probe_atomic_exchange(run: &mut impl FnMut(&str) -> Option<RecordBatch>) -> bool {
    let db_engine = scalar_probe(
        run,
        "SELECT engine AS v FROM system.databases WHERE name = database()",
    )
    .unwrap_or_default();
    if !engine_can_atomic_exchange(&db_engine) {
        return false;
    }
    if db_engine == "Shared" {
        return true;
    }

    let table_id = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let swap_tables = [
        format!("__dbt_exchange_test_0_{table_id}"),
        format!("__dbt_exchange_test_1_{table_id}"),
    ];
    let created = swap_tables.iter().all(|table| {
        run(&format!(
            "CREATE TABLE IF NOT EXISTS `{table}` (test String) ENGINE MergeTree() ORDER BY tuple()"
        ))
        .is_some()
    });
    let exchanged = created
        && run(&format!(
            "EXCHANGE TABLES `{}` AND `{}`",
            swap_tables[0], swap_tables[1]
        ))
        .is_some();
    for table in &swap_tables {
        let _ = run(&format!("DROP TABLE IF EXISTS `{table}`"));
    }
    exchanged
}

/// dbclient.py `ND_MUTATION_SETTING`.
const ND_MUTATION_SETTING: &str = "allow_nondeterministic_mutations";

/// dbclient.py `_ensure_database` parity (incl. its clause spacing).
/// Divergence: identifiers are double-quoted via the shared ident helper
/// instead of query.py's backtick quoting — equivalent for ClickHouse.
pub(crate) fn exists_database_sql(database: &str) -> String {
    format!(
        "EXISTS DATABASE {}",
        quote_identifier(database, AdapterType::ClickHouse)
    )
}

/// See [`exists_database_sql`].
pub(crate) fn create_database_sql(
    database: &str,
    database_engine: Option<&str>,
    cluster: Option<&str>,
) -> String {
    let engine_clause = match database_engine {
        Some(engine) if !engine.is_empty() => format!(" ENGINE {engine} "),
        _ => String::new(),
    };
    let cluster_clause = match cluster {
        Some(cluster) if !cluster.trim().is_empty() => format!(" ON CLUSTER \"{cluster}\" "),
        _ => String::new(),
    };
    format!(
        "CREATE DATABASE IF NOT EXISTS {}{cluster_clause}{engine_clause}",
        quote_identifier(database, AdapterType::ClickHouse)
    )
}

/// See [`probe_lightweight_deletes`].
struct LwDeletesProbe {
    has_lw_deletes: bool,
    nd_mutation_override: bool,
}

impl LwDeletesProbe {
    const UNAVAILABLE: Self = Self {
        has_lw_deletes: false,
        nd_mutation_override: false,
    };
}

/// Mirrors dbclient.py `_check_lightweight_deletes` (only
/// `allow_nondeterministic_mutations` needs checking — lightweight deletes
/// themselves are GA since ClickHouse 23.3). Divergence: Python raises a
/// config error at connection time when the setting is readonly and the
/// profile requested `use_lw_deletes` — here the probe just reports
/// "unavailable" and `validate_incremental_strategy` errors when a
/// lightweight-delete strategy is actually used.
fn probe_lightweight_deletes(run: &mut impl FnMut(&str) -> Option<RecordBatch>) -> LwDeletesProbe {
    let sql = format!(
        "SELECT value, toString(readonly) AS readonly FROM system.settings \
         WHERE name = '{ND_MUTATION_SETTING}'"
    );
    let Some(batch) = run(&sql) else {
        return LwDeletesProbe::UNAVAILABLE;
    };
    if batch.num_rows() == 0 {
        return LwDeletesProbe::UNAVAILABLE;
    }
    let (Ok(values), Ok(readonlys)) = (
        batch.column_values::<StringArray>("value"),
        batch.column_values::<StringArray>("readonly"),
    ) else {
        return LwDeletesProbe::UNAVAILABLE;
    };
    let enabled = values
        .value(0)
        .parse::<i64>()
        .map(|v| v > 0)
        .unwrap_or(false);
    if enabled {
        return LwDeletesProbe {
            has_lw_deletes: true,
            nd_mutation_override: false,
        };
    }
    if readonlys.value(0) != "0" {
        return LwDeletesProbe::UNAVAILABLE;
    }
    // Python's `SET {setting} = 1` try/except: verify the user may override
    // the setting.
    let probe = format!("SELECT 1 SETTINGS {ND_MUTATION_SETTING} = 1");
    let may_override = run(&probe).is_some();
    LwDeletesProbe {
        has_lw_deletes: may_override,
        nd_mutation_override: may_override,
    }
}

/// Mirrors impl.py `calculate_incremental_strategy`.
pub(crate) fn calculate_incremental_strategy(
    strategy: Option<&str>,
    use_lw_deletes: bool,
) -> String {
    let strategy = match strategy {
        None | Some("") | Some("default") => {
            if use_lw_deletes {
                "delete_insert"
            } else {
                "legacy"
            }
        }
        Some(s) => s,
    };
    strategy.replace('+', "_")
}

/// Mirrors impl.py `validate_incremental_strategy`: ClickHouse-specific
/// cross-config checks, with Python-exact messages.
pub(crate) fn validate_incremental_strategy(
    strategy: &str,
    has_predicates: bool,
    has_unique_key: bool,
    has_partition_by: bool,
    has_lw_deletes: bool,
) -> AdapterResult<()> {
    let err = |msg: String| Err(AdapterError::new(AdapterErrorKind::Configuration, msg));
    if !matches!(
        strategy,
        "legacy" | "append" | "delete_insert" | "insert_overwrite" | "microbatch"
    ) {
        return err(format!(
            "The incremental strategy '{strategy}' is not valid for ClickHouse."
        ));
    }
    if matches!(strategy, "delete_insert" | "microbatch") && !has_lw_deletes {
        return err(format!(
            "'{strategy}' strategy requires lightweight deletes, but the required setting \
             '{ND_MUTATION_SETTING}' could not be enabled on this ClickHouse server \
             (see the warnings logged at connection time)."
        ));
    }
    if matches!(strategy, "delete_insert" | "microbatch") && !has_unique_key {
        return err(format!(
            "'{strategy}' strategy requires a non-empty 'unique_key'."
        ));
    }
    if !matches!(strategy, "delete_insert" | "microbatch") && has_predicates {
        return err(format!(
            "Cannot apply incremental predicates with '{strategy}' strategy."
        ));
    }
    if strategy == "insert_overwrite" && !has_partition_by {
        return err(format!(
            "'{strategy}' strategy requires non-empty 'partition_by'. Current partition_by is None."
        ));
    }
    if strategy == "insert_overwrite" && has_unique_key {
        return err(format!(
            "'{strategy}' strategy does not support unique_key."
        ));
    }
    Ok(())
}

/// Mirrors column.py `ClickHouseColumnChanges` as a Jinja value: none when
/// there are no changes (its `__bool__`, so `{% if column_changes %}` skips),
/// else a map; errors when the strategy cannot reconcile the changes.
pub(crate) fn column_changes_value(
    on_schema_change: &str,
    source: Vec<Column>,
    target: Vec<Column>,
    materialization: &str,
) -> AdapterResult<Value> {
    let in_target = |name: &str| target.iter().any(|c| c.name() == name);
    let source_of = |name: &str| source.iter().find(|c| c.name() == name);

    let columns_to_drop: Vec<Column> = source
        .iter()
        .filter(|c| !in_target(c.name()))
        .cloned()
        .collect();
    let columns_to_add: Vec<Column> = target
        .iter()
        .filter(|c| source_of(c.name()).is_none())
        .cloned()
        .collect();
    let columns_to_modify: Vec<Column> = target
        .iter()
        .filter(|c| source_of(c.name()).is_some_and(|s| s.dtype() != c.dtype()))
        .cloned()
        .collect();

    let has_schema_changes =
        !(columns_to_add.is_empty() && columns_to_drop.is_empty() && columns_to_modify.is_empty());
    let has_sync_changes = !(columns_to_drop.is_empty() && columns_to_modify.is_empty());
    let has_conflicting_changes = (on_schema_change == "fail" && has_schema_changes)
        || (on_schema_change != "sync_all_columns" && has_sync_changes);

    if has_conflicting_changes {
        // Python list-repr of `[str(col), ...]` — tests assert on it.
        let fmt = |cols: &[Column]| {
            if cols.is_empty() {
                "[]".to_string()
            } else {
                format!(
                    "['{}']",
                    cols.iter()
                        .map(|c| format!("{} {}", c.name(), c.data_type()))
                        .collect::<Vec<_>>()
                        .join("', '")
                )
            }
        };
        // errors.py schema_change_fail_error
        let config_name = if materialization == "table" {
            "mv_on_schema_change"
        } else {
            "on_schema_change"
        };
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "The source and target schemas on this {materialization} model are out of sync.\n\
                 They can be reconciled in several ways:\n  \
                 - set the `{config_name}` config to `append_new_columns` or `sync_all_columns`.\n  \
                 - Re-run the {materialization} model with `full_refresh: True` to update the target schema.\n  \
                 - update the schema manually and re-run the process.\n\n\
                 Additional troubleshooting context:\n   \
                 Source columns not in target: {}\n   \
                 Target columns not in source: {}\n   \
                 New column types: {}",
                fmt(&columns_to_drop),
                fmt(&columns_to_add),
                fmt(&columns_to_modify),
            ),
        ));
    }

    if !has_schema_changes {
        return Ok(Value::from(()));
    }

    let mut changes = BTreeMap::new();
    changes.insert(
        "on_schema_change".to_string(),
        Value::from(on_schema_change),
    );
    changes.insert("columns_to_add".to_string(), Value::from(columns_to_add));
    changes.insert("columns_to_drop".to_string(), Value::from(columns_to_drop));
    changes.insert(
        "columns_to_modify".to_string(),
        Value::from(columns_to_modify),
    );
    Ok(Value::from(changes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_types::DefaultTypeOps;
    use crate::stmt_splitter::DefaultStmtSplitter;
    use arrow_schema::{DataType, Field};
    use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;

    #[test]
    fn test_build_settings_str_mirrors_python() {
        // Mirrors dbt-clickhouse _build_settings_str: unquoted strings get
        // single-quoted, other values verbatim; trailing newline; '' when empty.
        assert_eq!(build_settings_str(&[]), "");
        assert_eq!(
            build_settings_str(&[
                ("index_granularity".to_string(), Value::from(4096)),
                (
                    "replicated_deduplication_window".to_string(),
                    Value::from("0")
                ),
                ("prequoted".to_string(), Value::from("'x'")),
            ]),
            "SETTINGS index_granularity=4096, replicated_deduplication_window='0', prequoted='x'\n"
        );
    }

    #[test]
    fn test_model_config_map_reads_model_config() {
        let model = Value::from_serialize(serde_json::json!({
            "config": {
                "settings": {"index_granularity": 4096},
                "query_settings": {"join_use_nulls": 1},
            }
        }));
        let settings = model_config_map(&model, "settings");
        assert_eq!(settings.len(), 1);
        assert_eq!(settings[0].0, "index_granularity");

        // missing key -> empty
        assert!(model_config_map(&model, "not_there").is_empty());
        // model without config -> empty
        assert!(model_config_map(&Value::from(()), "settings").is_empty());
    }

    /// Mirrors impl.py rendering, incl. the TestS3* fixture shapes and URL join.
    #[test]
    fn test_s3source_clause_mirrors_python() {
        let s3config = Value::from_serialize(serde_json::json!({
            "bucket": "https://datasets-documentation.s3.eu-west-3.amazonaws.com/nyc-taxi/",
            "fmt": "TabSeparatedWithNames",
        }));
        let structure = Value::from_serialize(serde_json::json!([
            "trip_id UInt32",
            "pickup_datetime DateTime"
        ]));
        let clause = s3source_clause(
            &s3config,
            &structure,
            "",
            "/trips_5.gz",
            "",
            "",
            "",
            "",
            "",
            "",
        )
        .unwrap();
        assert_eq!(
            clause,
            "s3('https://datasets-documentation.s3.eu-west-3.amazonaws.com/nyc-taxi/trips_5.gz', \
             'TabSeparatedWithNames', 'trip_id UInt32,pickup_datetime DateTime')"
        );

        // dict structure -> "name type,..."; plain string -> ",'...'" (no space)
        let dict_structure =
            Value::from_serialize(serde_json::json!({"fare": "Float64", "trip_id": "UInt32"}));
        let clause = s3source_clause(
            &s3config,
            &dict_structure,
            "bucket.com",
            "",
            "CSV",
            "",
            "",
            "",
            "",
            "",
        )
        .unwrap();
        assert_eq!(
            clause,
            "s3('https://bucket.com', 'CSV', 'fare Float64,trip_id UInt32')"
        );
        let string_structure = Value::from("a UInt32, b String");
        let clause = s3source_clause(
            &s3config,
            &string_structure,
            "bucket.com",
            "",
            "CSV",
            "",
            "",
            "",
            "",
            "",
        )
        .unwrap();
        assert_eq!(
            clause,
            "s3('https://bucket.com', 'CSV','a UInt32, b String')"
        );
    }

    /// Access keys come in pairs; compression forces the empty structure
    /// placeholder; external_id requires role_arn.
    #[test]
    fn test_s3source_clause_credentials_and_compression() {
        let empty = Value::from_serialize(serde_json::json!({}));
        let no_structure = Value::from(());

        let clause = s3source_clause(
            &empty,
            &no_structure,
            "bucket.com",
            "",
            "CSV",
            "KEY",
            "SECRET",
            "",
            "",
            "",
        )
        .unwrap();
        assert_eq!(clause, "s3('https://bucket.com', 'KEY', 'SECRET', 'CSV')");

        let err = s3source_clause(
            &empty,
            &no_structure,
            "bucket.com",
            "",
            "CSV",
            "KEY",
            "",
            "",
            "",
            "",
        )
        .unwrap_err();
        assert!(
            err.message()
                .contains("S3 aws_access_key_id specified without aws_secret_access_key")
        );
        let err = s3source_clause(
            &empty,
            &no_structure,
            "bucket.com",
            "",
            "CSV",
            "",
            "SECRET",
            "",
            "",
            "",
        )
        .unwrap_err();
        assert!(
            err.message()
                .contains("S3 aws_secret_access_key specified without aws_access_key_id")
        );

        let clause = s3source_clause(
            &empty,
            &no_structure,
            "bucket.com",
            "",
            "CSV",
            "",
            "",
            "",
            "gzip",
            "",
        )
        .unwrap();
        assert_eq!(clause, "s3('https://bucket.com', 'CSV', '', 'gzip')");

        let clause = s3source_clause(
            &empty,
            &no_structure,
            "bucket.com",
            "",
            "CSV",
            "",
            "",
            "arn:aws:iam::123456789012:role/my-role",
            "",
            "my-external-id",
        )
        .unwrap();
        assert_eq!(
            clause,
            "s3('https://bucket.com', 'CSV', extra_credentials(\
             role_arn='arn:aws:iam::123456789012:role/my-role', external_id='my-external-id'))"
        );

        let err = s3source_clause(
            &empty,
            &no_structure,
            "bucket.com",
            "",
            "CSV",
            "",
            "",
            "",
            "",
            "my-external-id",
        )
        .unwrap_err();
        assert!(
            err.message()
                .contains("S3 external_id specified without role_arn")
        );
    }

    #[test]
    fn test_ensure_database_sql_mirrors_python() {
        assert_eq!(exists_database_sql("my_db"), "EXISTS DATABASE \"my_db\"");
        // '"' doubling in quoted identifiers verified against ClickHouse 26.3
        assert_eq!(
            exists_database_sql("we\"ird"),
            "EXISTS DATABASE \"we\"\"ird\""
        );
        assert_eq!(
            create_database_sql("my_db", None, None),
            "CREATE DATABASE IF NOT EXISTS \"my_db\""
        );
        assert_eq!(
            create_database_sql("my_db", Some(""), Some("  ")),
            "CREATE DATABASE IF NOT EXISTS \"my_db\""
        );
        assert_eq!(
            create_database_sql("my_db", Some("Replicated"), Some("test_shard")),
            "CREATE DATABASE IF NOT EXISTS \"my_db\" ON CLUSTER \"test_shard\"  ENGINE Replicated "
        );
    }

    #[test]
    fn test_compare_versions_mirrors_python() {
        assert_eq!(compare_versions("26.6", "26.3.12.3").unwrap(), 1);
        assert_eq!(compare_versions("22.7.1.2484", "26.3.12.3").unwrap(), -1);
        assert_eq!(compare_versions("26.3", "26.3.12.3").unwrap(), 0); // zip stops at shorter
        assert!(compare_versions("26.x", "26.3").is_err());
    }

    #[test]
    fn test_calculate_incremental_strategy_mirrors_python() {
        for unset in [None, Some(""), Some("default")] {
            assert_eq!(calculate_incremental_strategy(unset, true), "delete_insert");
            assert_eq!(calculate_incremental_strategy(unset, false), "legacy");
        }
        // explicit strategies pass through regardless of the capability
        assert_eq!(
            calculate_incremental_strategy(Some("append"), true),
            "append"
        );
        // '+' -> '_' (dbt-core spells it delete+insert)
        assert_eq!(
            calculate_incremental_strategy(Some("delete+insert"), false),
            "delete_insert"
        );
    }

    #[test]
    fn test_validate_incremental_strategy_mirrors_python() {
        let msg = |result: AdapterResult<()>| result.unwrap_err().to_string();

        assert!(validate_incremental_strategy("legacy", false, true, false, true).is_ok());
        assert!(validate_incremental_strategy("delete_insert", true, true, false, true).is_ok());
        assert!(
            validate_incremental_strategy("insert_overwrite", false, false, true, false).is_ok()
        );

        assert!(
            msg(validate_incremental_strategy(
                "merge", false, false, false, true
            ))
            .contains("The incremental strategy 'merge' is not valid for ClickHouse.")
        );
        assert!(
            msg(validate_incremental_strategy(
                "delete_insert",
                false,
                true,
                false,
                false
            ))
            .contains(
                "'delete_insert' strategy requires lightweight deletes, but the required setting \
                 'allow_nondeterministic_mutations' could not be enabled on this ClickHouse server \
                 (see the warnings logged at connection time)."
            )
        );
        assert!(
            msg(validate_incremental_strategy(
                "microbatch",
                false,
                false,
                false,
                true
            ))
            .contains("'microbatch' strategy requires a non-empty 'unique_key'.")
        );
        assert!(
            msg(validate_incremental_strategy(
                "append", true, false, false, true
            ))
            .contains("Cannot apply incremental predicates with 'append' strategy.")
        );
        assert!(
            msg(validate_incremental_strategy(
                "insert_overwrite",
                false,
                false,
                false,
                true
            ))
            .contains(
                "'insert_overwrite' strategy requires non-empty 'partition_by'. \
                     Current partition_by is None."
            )
        );
        assert!(
            msg(validate_incremental_strategy(
                "insert_overwrite",
                false,
                true,
                true,
                true
            ))
            .contains("'insert_overwrite' strategy does not support unique_key.")
        );
    }

    fn ch_column(name: &str, dtype: &str) -> Column {
        Column::new(
            AdapterType::ClickHouse,
            name.to_string(),
            dtype.to_string(),
            None,
            None,
            None,
        )
    }

    #[test]
    fn test_column_changes_value_mirrors_python() {
        let source = vec![ch_column("id", "UInt64"), ch_column("name", "String")];

        // identical schemas -> none (ClickHouseColumnChanges.__bool__)
        let unchanged = column_changes_value(
            "append_new_columns",
            source.clone(),
            source.clone(),
            "incremental",
        )
        .unwrap();
        assert!(unchanged.is_none());

        let target = vec![
            ch_column("id", "UInt64"),
            ch_column("name", "String"),
            ch_column("age", "UInt8"),
        ];
        let changes = column_changes_value(
            "append_new_columns",
            source.clone(),
            target.clone(),
            "incremental",
        )
        .unwrap();
        assert_eq!(
            changes.get_attr("on_schema_change").unwrap().as_str(),
            Some("append_new_columns")
        );
        let added = changes.get_attr("columns_to_add").unwrap();
        assert_eq!(added.len(), Some(1));
        let first = added.get_item(&Value::from(0)).unwrap();
        assert_eq!(first.get_attr("name").unwrap().as_str(), Some("age"));

        // any change with fail -> Python's schema_change_fail_error, list-repr'd
        let err = column_changes_value("fail", source.clone(), target.clone(), "incremental")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(
                "The source and target schemas on this incremental model are out of sync."
            )
        );
        assert!(err.contains("set the `on_schema_change` config"));
        assert!(err.contains("Source columns not in target: []"));
        assert!(err.contains("Target columns not in source: ['age UInt8']"));
        assert!(err.contains("New column types: []"));

        // dropped column requires sync_all_columns; append_new_columns conflicts
        let narrowed = vec![ch_column("id", "UInt64")];
        let err = column_changes_value(
            "append_new_columns",
            source.clone(),
            narrowed.clone(),
            "incremental",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Source columns not in target: ['name String']"));
        let synced =
            column_changes_value("sync_all_columns", source.clone(), narrowed, "incremental")
                .unwrap();
        assert_eq!(synced.get_attr("columns_to_drop").unwrap().len(), Some(1));

        // table materialization names the MV config key instead
        let err = column_changes_value("fail", source, target, "table")
            .unwrap_err()
            .to_string();
        assert!(err.contains("set the `mv_on_schema_change` config"));
        assert!(err.contains("this table model"));
    }

    /// A ClickHouse metadata adapter backed by a mock engine (no live
    /// connection); the catalog batch parsers never touch the engine.
    fn make_metadata_adapter() -> ClickHouseMetadataAdapter {
        ClickHouseMetadataAdapter {
            adapter: AdapterImpl::new_mock(
                AdapterType::ClickHouse,
                BTreeMap::new(),
                DEFAULT_RESOLVED_QUOTING,
                Arc::new(DefaultTypeOps::new(AdapterType::ClickHouse)),
                Arc::new(DefaultStmtSplitter),
            ),
        }
    }

    /// Build a catalog record batch with the same Arrow types the ClickHouse
    /// ADBC driver produces for the `clickhouse__get_catalog*` macro SQL:
    /// strings everywhere except `column_index`, which is a `UInt64`
    /// (`system.columns.position`). Using other types here (e.g. Decimal128
    /// for `column_index`) must fail, so these tests pin the wire format.
    #[allow(clippy::type_complexity)]
    fn catalog_batch(rows: &[(&str, &str, &str, &str, &str, u64, &str, &str)]) -> Arc<RecordBatch> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("table_database", DataType::Utf8, false),
            Field::new("table_schema", DataType::Utf8, false),
            Field::new("table_name", DataType::Utf8, false),
            Field::new("table_type", DataType::Utf8, false),
            Field::new("table_comment", DataType::Utf8, true),
            Field::new("column_name", DataType::Utf8, false),
            Field::new("column_index", DataType::UInt64, false),
            Field::new("column_type", DataType::Utf8, false),
            Field::new("column_comment", DataType::Utf8, true),
            Field::new("table_owner", DataType::Utf8, true),
        ]));
        Arc::new(
            RecordBatch::try_new(
                schema,
                vec![
                    // table_database is always '' for ClickHouse (2-part naming)
                    Arc::new(StringArray::from(vec![""; rows.len()])),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.0))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.1))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.2))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.3))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.4))),
                    Arc::new(UInt64Array::from_iter_values(rows.iter().map(|r| r.5))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.6))),
                    Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.7))),
                    // table_owner is `cast(null as Nullable(String))` in the macro
                    Arc::new(StringArray::from(vec![None::<&str>; rows.len()])),
                ],
            )
            .expect("catalog_batch test fixture should build a valid RecordBatch"),
        )
    }

    fn sample_batch() -> Arc<RecordBatch> {
        catalog_batch(&[
            (
                "db1",
                "orders",
                "table",
                "fact table",
                "id",
                1,
                "UInt64",
                "",
            ),
            (
                "db1",
                "orders",
                "table",
                "fact table",
                "amount",
                2,
                "Decimal(18, 2)",
                "in cents",
            ),
            ("db1", "orders_v", "view", "", "id", 1, "UInt64", ""),
        ])
    }

    #[test]
    fn build_schemas_from_stats_sql_groups_rows_per_relation() {
        let adapter = make_metadata_adapter();
        let result = adapter
            .build_schemas_from_stats_sql(sample_batch())
            .unwrap();

        assert_eq!(
            result.keys().collect::<Vec<_>>(),
            vec![".db1.orders", ".db1.orders_v"]
        );

        let orders = &result[".db1.orders"].metadata;
        assert_eq!(orders.materialization_type, "table");
        assert_eq!(orders.schema, "db1");
        assert_eq!(orders.name, "orders");
        assert_eq!(orders.database, Some("".to_string()));
        assert_eq!(orders.comment, Some("fact table".to_string()));
        // null table_owner reads back as an empty string
        assert_eq!(orders.owner, Some("".to_string()));

        let view = &result[".db1.orders_v"].metadata;
        assert_eq!(view.materialization_type, "view");
        assert_eq!(view.comment, None);
    }

    #[test]
    fn build_columns_from_get_columns_reads_uint64_positions() {
        let adapter = make_metadata_adapter();
        let result = adapter
            .build_columns_from_get_columns(sample_batch())
            .unwrap();

        let orders = &result[".db1.orders"];
        assert_eq!(orders.len(), 2);
        assert_eq!(orders["id"].index, 1);
        assert_eq!(orders["id"].data_type, "UInt64");
        assert_eq!(orders["id"].comment, None);
        assert_eq!(orders["amount"].index, 2);
        assert_eq!(orders["amount"].data_type, "Decimal(18, 2)");
        assert_eq!(orders["amount"].comment, Some("in cents".to_string()));

        let view = &result[".db1.orders_v"];
        assert_eq!(view.len(), 1);
        assert_eq!(view["id"].index, 1);
    }

    #[test]
    fn catalog_batch_parsers_accept_empty_batches() {
        let adapter = make_metadata_adapter();
        let batch = catalog_batch(&[]);
        assert!(
            adapter
                .build_schemas_from_stats_sql(batch.clone())
                .unwrap()
                .is_empty()
        );
        assert!(
            adapter
                .build_columns_from_get_columns(batch)
                .unwrap()
                .is_empty()
        );
    }

    /// ClickHouse's `system.tables` is case-sensitive on `database` and `name`
    #[test]
    fn build_get_relation_sql_passes_names_through_verbatim() {
        let sql = build_get_relation_sql("Mixed_Case", "Stg_Customers");
        assert!(
            sql.ends_with(
                "FROM system.tables WHERE database = 'Mixed_Case' AND name = 'Stg_Customers'"
            ),
            "got: {sql}"
        );
        assert!(
            sql.contains(
                "(SELECT engine FROM system.databases WHERE name = 'Mixed_Case') AS database_engine"
            ),
            "got: {sql}"
        );
        assert!(
            sql.contains("= 'Mixed_Case.Stg_Customers') AS mvs_pointing_to_it"),
            "got: {sql}"
        );
        assert!(
            sql.contains("' REFRESH ')) > 0, 'true', 'false') AS is_refreshable"),
            "got: {sql}"
        );
        assert!(
            sql.contains("' APPEND')) > 0, 'true', 'false') AS refreshable_append"),
            "got: {sql}"
        );
    }

    /// Embedded `'` and `\` must be backslash-escaped per
    /// <https://clickhouse.com/docs/en/sql-reference/syntax#string>, otherwise
    /// the literal terminates early or trails an unbalanced backslash.
    #[test]
    fn build_get_relation_sql_escapes_quotes_and_backslashes() {
        let sql = build_get_relation_sql("a'b", r"c\d");
        assert!(
            sql.ends_with(r"FROM system.tables WHERE database = 'a\'b' AND name = 'c\\d'"),
            "got: {sql}"
        );
        assert!(
            sql.contains(r"= 'a\'b.c\\d') AS mvs_pointing_to_it"),
            "got: {sql}"
        );
    }

    #[test]
    fn mv_target_regex_has_no_literal_question_mark() {
        assert!(!MV_TARGET_FQN_REGEX_SQL.contains('?'));
        assert!(!build_get_relation_sql("db", "t").contains('?'));
    }

    #[test]
    fn can_exchange_mirrors_python() {
        assert!(engine_can_atomic_exchange("Atomic"));
        assert!(engine_can_atomic_exchange("Replicated"));
        assert!(engine_can_atomic_exchange("Shared"));
        assert!(!engine_can_atomic_exchange("Ordinary"));
        assert!(!engine_can_atomic_exchange("Lazy"));

        let caps = |atomic_exchange| ClickHouseCapabilities {
            server_version: String::new(),
            atomic_exchange,
            has_lw_deletes: false,
            use_lw_deletes: false,
            nd_mutation_override: false,
        };
        assert!(relation_can_exchange(
            &caps(true),
            RelationType::Table,
            "Atomic"
        ));
        assert!(!relation_can_exchange(
            &caps(true),
            RelationType::View,
            "Atomic"
        ));
        assert!(!relation_can_exchange(
            &caps(true),
            RelationType::MaterializedView,
            "Atomic"
        ));
        assert!(!relation_can_exchange(
            &caps(false),
            RelationType::Table,
            "Atomic"
        ));
        assert!(!relation_can_exchange(
            &caps(true),
            RelationType::Table,
            "Ordinary"
        ));
    }

    #[test]
    fn parse_mvs_pointing_to_it_handles_json_and_garbage() {
        let parsed =
            parse_mvs_pointing_to_it(r#"[{"schema":"db1","name":"mv1","sql":"select 1"}]"#);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["schema"], "db1");
        assert_eq!(parsed[0]["name"], "mv1");
        assert_eq!(parsed[0]["sql"], "select 1");
        assert!(parse_mvs_pointing_to_it("[]").is_empty());
        assert!(parse_mvs_pointing_to_it("").is_empty());
        assert!(parse_mvs_pointing_to_it("not json").is_empty());
    }
}
