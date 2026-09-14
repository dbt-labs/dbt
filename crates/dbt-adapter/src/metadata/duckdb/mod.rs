use crate::AdapterEngine;
use crate::adapter::adapter_impl::AdapterImpl;
use crate::connection::AdapterConnectionFactory;
use crate::load_catalogs;
use crate::relation::do_create_relation;
use crate::sql_types::{TypeOps, make_arrow_field_v2};
use crate::{AdapterResult, errors::AsyncAdapterResult, metadata::*, record_batch::RecordBatchExt};
use arrow_schema::Schema;

use arrow_array::{Array, Int32Array, RecordBatch, StringArray};

use dbt_adapter_core::ExecutionPhase;
use dbt_adapter_engine::MapReduce;
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::cancellation::Cancellable;
use dbt_common::cancellation::CancellationToken;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::dbt_catalogs_v2::{
    CatalogSpecV2View, CatalogType, DbtCatalogsV2View, PhysicalFormatResolver,
};
use dbt_schemas::schemas::{
    common::ResolvedQuoting,
    legacy_catalog::{CatalogNodeStats, CatalogTable, ColumnMetadata, TableMetadata},
    profiles::DuckDBPathInfo,
    relations::base::{BaseRelation, RelationPattern},
};
use indexmap::IndexMap;
use minijinja::State;
use std::collections::btree_map::Entry;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub struct DuckDBMetadataAdapter {
    adapter: AdapterImpl,
}

impl DuckDBMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }
}

impl MetadataAdapter for DuckDBMetadataAdapter {
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
                    columns: IndexMap::new(),
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
            // Use DESCRIBE to get table schema
            // DuckDB's DESCRIBE returns: column_name, column_type, null, key, default, extra
            let sql = format!("DESCRIBE {};", rendered);
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
                build_schema_from_duckdb_describe(batch, adapter.engine().type_ops().as_ref())?;
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

    fn list_relations_schemas_by_patterns_inner(
        &self,
        _patterns: &[RelationPattern],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        todo!("DuckDBAdapter::list_relations_schemas_by_patterns")
    }

    fn freshness_inner(
        &self,
        _relations: &[Arc<dyn BaseRelation>],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        todo!("DuckDBAdapter::freshness")
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
                    // If the schema doesn't exist, treat as empty (no relations).
                    // DuckDB raises "Catalog Error: Schema with name <x> does not exist"
                    if e.message().contains("does not exist") {
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

/// Build the `information_schema.tables` query used to list relations in a
/// schema.
///
/// `information_schema.tables` unions every attached database, so the query is
/// constrained to the target catalog. This keeps the result correct (no rows
/// leak in from other attached catalogs, including misreported Iceberg REST
/// ones) and matches the per-catalog scope dbt expects when warming the relation
/// cache. When the catalog is unknown (empty), the catalog predicate is omitted.
fn list_relations_sql(quoting: ResolvedQuoting, db_schema: &CatalogAndSchema) -> String {
    let query_schema = if quoting.schema {
        db_schema.resolved_schema.clone()
    } else {
        db_schema.resolved_schema.to_lowercase()
    };

    let catalog_predicate = if db_schema.resolved_catalog.is_empty() {
        String::new()
    } else {
        // lower() on both sides makes the catalog match case-insensitive
        // regardless of quoting config, so no quoting-dependent case folding
        // is needed here (unlike the schema predicate).
        format!(
            " AND lower(table_catalog) = lower('{}')",
            dbt_adapter_sql::ident::escape_string_literal(
                &db_schema.resolved_catalog,
                AdapterType::DuckDB
            ),
        )
    };

    format!(
        "SELECT table_catalog, table_schema, table_name, table_type \
         FROM information_schema.tables \
         WHERE table_schema = '{}'{}",
        dbt_adapter_sql::ident::escape_string_literal(&query_schema, AdapterType::DuckDB),
        catalog_predicate,
    )
}

/// List all relations (tables, views) in a given schema.
///
/// Queries DuckDB's `information_schema.tables` and maps the results to
/// `BaseRelation` objects suitable for populating the adapter relation cache.
pub fn list_relations(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    // Schema-wide listing is only unreliable for external Iceberg REST catalogs
    // (their information_schema coverage is incomplete), so bail to the targeted
    // DESCRIBE-based get_relation path only when the *target* catalog is one of
    // them. A regular DuckDB catalog is still listed even while Iceberg catalogs
    // are attached — see the catalog scoping below.
    if is_duckdb_v2_external_iceberg_catalog_database(&db_schema.resolved_catalog) {
        return Err(AdapterError::new(
            AdapterErrorKind::NotSupported,
            format!(
                "DuckDB schema-wide relation listing is disabled while v2 Iceberg REST catalogs are attached; use targeted get_relation introspection instead for '{}'",
                db_schema.resolved_catalog
            ),
        ));
    }

    let sql = list_relations_sql(engine.quoting(), db_schema);

    let batch = engine.execute(None, conn, ctx, &sql, token)?;

    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let table_catalogs = batch.column_values::<StringArray>("table_catalog")?;
    let table_schemas = batch.column_values::<StringArray>("table_schema")?;
    let table_names = batch.column_values::<StringArray>("table_name")?;
    let table_types = batch.column_values::<StringArray>("table_type")?;

    let mut relations = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        let database = table_catalogs.value(i);
        let schema = table_schemas.value(i);
        let name = table_names.value(i);
        // DuckDB table_type values: "BASE TABLE", "VIEW", "LOCAL TEMPORARY"
        let relation_type = match table_types.value(i) {
            "BASE TABLE" => RelationType::Table,
            "VIEW" => RelationType::View,
            "LOCAL TEMPORARY" => RelationType::Table,
            other => RelationType::from_adapter_type(engine.adapter_type(), other),
        };

        let relation = do_create_relation(
            engine.adapter_type(),
            database.to_string(),
            schema.to_string(),
            Some(name.to_string()),
            Some(relation_type),
            engine.quoting(),
        )
        .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))?;

        relations.push(Arc::from(relation));
    }

    Ok(relations)
}

/// An external Iceberg REST catalog that DuckDB reaches through an Iceberg REST
/// `ATTACH`. Returned instead of a bare bool so callers see *which* catalog
/// matched (its sanitized attach alias) rather than only whether one did.
pub(crate) struct ExternalIcebergAttach {
    pub(crate) attach_alias: String,
}

/// Catalog types DuckDB reaches through an Iceberg REST `ATTACH ... (TYPE
/// ICEBERG)`. Horizon, Unity, and Glue are Iceberg REST services under the
/// hood. Single answer to "is this an Iceberg REST-style attachment?" so the
/// ATTACH composer (`engine::duckdb_attach`) and the metadata routing below
/// can never disagree about which catalogs need REST-attachment treatment.
pub(crate) fn attaches_via_iceberg_rest(catalog_type: CatalogType) -> bool {
    matches!(
        catalog_type,
        CatalogType::IcebergRest | CatalogType::Horizon | CatalogType::Unity | CatalogType::Glue
    )
}

/// DuckDB-specific classification of a single v2 catalog spec.
pub(crate) trait CatalogSpecDuckDbExt {
    /// `Some(..)` iff this catalog is an external Iceberg REST-attached catalog
    /// (IcebergRest/Horizon/Unity/Glue with `table_format: iceberg` and a `duckdb`
    /// config block), carrying its sanitized `ATTACH` alias.
    fn external_iceberg_attach(&self) -> Option<ExternalIcebergAttach>;

    fn resolved_attach_alias(&self) -> Option<String>;
}

impl CatalogSpecDuckDbExt for CatalogSpecV2View<'_> {
    fn external_iceberg_attach(&self) -> Option<ExternalIcebergAttach> {
        // DuckDB exposes these catalogs through Iceberg REST-style attachments.
        // Their information_schema coverage is incomplete, so schema-wide listing
        // is avoided while targeted DESCRIBE-based get_relation remains available.
        let is_external_iceberg =
            attaches_via_iceberg_rest(self.catalog_type) && self.table_format.is_iceberg();
        if !is_external_iceberg {
            return None;
        }

        Some(ExternalIcebergAttach {
            attach_alias: self.resolved_attach_alias()?,
        })
    }

    /// Single source of truth for alias resolution, so routing and attachment
    /// can never drift apart.
    fn resolved_attach_alias(&self) -> Option<String> {
        let duckdb_block = self
            .config_block(AdapterType::DuckDB.as_ref())
            .or_else(|| self.config_block(AdapterType::LakeCompute.as_ref()))?;
        let alias = duckdb_block
            .get(dbt_yaml::Value::from("catalog_database"))
            .and_then(|value| value.as_str())
            .unwrap_or(self.name);
        Some(dbt_adapter_sql::ident::sanitize_identifier(
            alias,
            AdapterType::DuckDB,
        ))
    }
}

/// DuckDB-specific lookups over the whole v2 catalogs view.
trait CatalogsViewDuckDbExt {
    /// The external Iceberg attach whose sanitized alias matches `database`, if any.
    fn external_iceberg_attach_for_database(&self, database: &str)
    -> Option<ExternalIcebergAttach>;

    /// The egress table format string for the catalog whose attached alias
    /// matches `database`, if any.
    fn table_format_for_database(&self, database: &str) -> Option<&'static str>;
}

impl CatalogsViewDuckDbExt for DbtCatalogsV2View<'_> {
    fn external_iceberg_attach_for_database(
        &self,
        database: &str,
    ) -> Option<ExternalIcebergAttach> {
        if database.is_empty() {
            return None;
        }
        self.catalogs.iter().find_map(|catalog| {
            let attach = catalog.external_iceberg_attach()?;
            attach
                .attach_alias
                .eq_ignore_ascii_case(database)
                .then_some(attach)
        })
    }

    fn table_format_for_database(&self, database: &str) -> Option<&'static str> {
        self.catalogs.iter().find_map(|catalog| {
            let alias = catalog.resolved_attach_alias()?;
            if !alias.eq_ignore_ascii_case(database) {
                return None;
            }
            Some(catalog.physical_table_format().as_str())
        })
    }
}

/// Load the active v2 catalogs view and run `f`, returning `None` when
/// catalogs.yml v2 is not in use. One gate replaces the scattered
/// fetch-use / fetch / view_v2 guard boilerplate.
fn with_duckdb_v2_catalogs_view<R>(
    f: impl FnOnce(&DbtCatalogsV2View<'_>) -> Option<R>,
) -> Option<R> {
    if !load_catalogs::fetch_use_catalogs_v2() {
        return None;
    }
    let catalogs = load_catalogs::fetch_catalogs()?;
    let view = catalogs.view_v2().ok()?;
    f(&view)
}

/// The external Iceberg attach DuckDB uses for `database`, when catalogs.yml v2
/// routes it to one.
pub(crate) fn duckdb_external_iceberg_attach_for_database(
    database: &str,
) -> Option<ExternalIcebergAttach> {
    if database.is_empty() {
        return None;
    }
    with_duckdb_v2_catalogs_view(|view| view.external_iceberg_attach_for_database(database))
}

/// Whether `database` resolves to a DuckDB external Iceberg REST catalog. Thin
/// boolean view over [`duckdb_external_iceberg_attach_for_database`] for the
/// schema-listing / get_relation decision points.
pub(crate) fn is_duckdb_v2_external_iceberg_catalog_database(database: &str) -> bool {
    duckdb_external_iceberg_attach_for_database(database).is_some()
}

/// The egress table format string for a DuckDB attached-database alias from catalogs.yml v2.
/// Used by `adapter.table_format(relation)` when a Jinja relation only gives
/// us its database/catalog: the attached alias is the bridge back to
/// catalogs.yml, which tells macros whether DuckDB needs Iceberg/DuckLake DDL
/// behavior for that relation.
pub(crate) fn duckdb_table_format_for_database(database: &str) -> Option<&'static str> {
    with_duckdb_v2_catalogs_view(|view| view.table_format_for_database(database))
}

/// Resolve one profile-level `attach:` entry to its `(alias, format_str)`, or
/// `None` when the entry is not an Iceberg/DuckLake attachment we route on.
/// Each entry yields a single outcome — no cross-entry sentinel state.
pub(crate) fn classify_attach_entry(item: &dbt_yaml::Value) -> Option<(String, &'static str)> {
    let dbt_yaml::Value::Mapping(map, _) = item else {
        return None;
    };
    let path = map.get("path").and_then(|v| v.as_str())?;
    let explicit_type = map
        .get("type")
        .and_then(|v| v.as_str())
        .map(str::to_ascii_lowercase);
    let attach_info = DuckDBPathInfo::parse_path(Some(path));

    // Iceberg takes precedence over DuckLake when an entry is tagged both ways
    // (matches the prior explicit_iceberg-first branch order).
    let format = if explicit_type.as_deref() == Some("iceberg") {
        "iceberg"
    } else if explicit_type.as_deref() == Some("ducklake")
        || map
            .get("is_ducklake")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        || attach_info.is_ducklake
    {
        "ducklake"
    } else {
        return None;
    };

    let alias = map
        .get("alias")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| attach_info.database.to_owned());
    Some((alias, format))
}

/// Whether a DuckDB error from the targeted DESCRIBE probe means "relation absent"
/// (so `get_relation` can report `None`) rather than a real failure.
pub(crate) fn is_missing_relation_error(err: &AdapterError) -> bool {
    let msg = err.to_string().to_lowercase();
    let missing =
        msg.contains("does not exist") || msg.contains("not found") || msg.contains("no such");
    // A remote Iceberg REST server may report a missing relation without echoing
    // the table name (e.g. "Namespace does not exist", "Catalog entry not found"),
    // so match on the relation-ish nouns instead of the identifier — a bare
    // identifier substring also shows up in non-relation failures (a missing
    // secret named after the table, say) which must not be swallowed as None.
    let relationish = msg.contains("table")
        || msg.contains("view")
        || msg.contains("relation")
        || msg.contains("namespace")
        || msg.contains("catalog entry");
    missing && relationish
}

/// Build an Arrow Schema from DuckDB's DESCRIBE output.
///
/// DuckDB's DESCRIBE returns columns: column_name, column_type, null, key, default, extra
fn build_schema_from_duckdb_describe(
    describe_result: Arc<RecordBatch>,
    type_ops: &dyn TypeOps,
) -> AdapterResult<Arc<Schema>> {
    let column_names = describe_result.column_values::<StringArray>("column_name")?;
    let data_types = describe_result.column_values::<StringArray>("column_type")?;
    let nullability = describe_result.column_values::<StringArray>("null")?;

    let mut fields = vec![];
    for i in 0..describe_result.num_rows() {
        let name = column_names.value(i);
        // DuckDB returns "YES" or "NO" for nullability
        let nullable = nullability.value(i).to_uppercase() == "YES";
        let text_data_type = data_types.value(i);

        let field = make_arrow_field_v2(
            type_ops,
            name.to_string(),
            text_data_type,
            Some(nullable),
            None, // No comment from DESCRIBE
        )?;
        fields.push(field);
    }

    let schema = Schema::new(fields);
    Ok(Arc::new(schema))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::dbt_catalogs::DbtCatalogs;

    fn catalog_and_schema(catalog: &str, schema: &str) -> CatalogAndSchema {
        CatalogAndSchema {
            rendered_catalog: catalog.to_string(),
            rendered_schema: schema.to_string(),
            resolved_catalog: catalog.to_string(),
            resolved_schema: schema.to_string(),
        }
    }

    #[test]
    fn is_missing_relation_error_handles_rest_errors_without_identifier() {
        let mk = |m: &str| AdapterError::new(AdapterErrorKind::Internal, m.to_string());
        // identifier echoed in the message (the common DuckDB case)
        assert!(is_missing_relation_error(&mk(
            "Table with name foo does not exist"
        )));
        // remote Iceberg REST servers may omit the table name entirely
        assert!(is_missing_relation_error(&mk("Namespace does not exist")));
        assert!(is_missing_relation_error(&mk("Catalog entry not found")));
        assert!(is_missing_relation_error(&mk("No such table")));
        // unrelated failures must not be swallowed as "missing relation"
        assert!(!is_missing_relation_error(&mk("permission denied")));
        assert!(!is_missing_relation_error(&mk(
            "connection refused by host"
        )));
        // "missing" wording without a relation-ish noun stays a real error: a
        // failure mentioning only the identifier (e.g. a missing secret named
        // after the table) must not be classified as relation-absent.
        assert!(!is_missing_relation_error(&mk("Secret 'orders' not found")));
    }

    #[test]
    fn list_relations_sql_scopes_to_target_catalog() {
        // With a catalog present the listing must be constrained to it, so rows
        // from other attached databases (e.g. remote Iceberg REST catalogs that
        // `information_schema.tables` would otherwise union in) cannot leak.
        let sql = list_relations_sql(
            ResolvedQuoting::trues(),
            &catalog_and_schema("aws_cloud_cost_demo", "aws_cloud_cost"),
        );
        assert_eq!(
            sql,
            "SELECT table_catalog, table_schema, table_name, table_type \
             FROM information_schema.tables \
             WHERE table_schema = 'aws_cloud_cost' \
             AND lower(table_catalog) = lower('aws_cloud_cost_demo')",
        );
    }

    #[test]
    fn list_relations_sql_lowercases_unquoted_identifiers() {
        // Unquoted schema identifiers fold to lowercase in DuckDB, so the schema
        // predicate must be lowercased to match; the catalog predicate is
        // case-insensitive on both sides via lower() and needs no folding.
        let sql = list_relations_sql(
            ResolvedQuoting::falses(),
            &catalog_and_schema("My_Catalog", "My_Schema"),
        );
        assert!(sql.contains("table_schema = 'my_schema'"));
        assert!(sql.contains("lower(table_catalog) = lower('My_Catalog')"));
    }

    #[test]
    fn list_relations_sql_omits_catalog_predicate_when_unknown() {
        // No catalog means we cannot scope; fall back to the schema-only filter
        // rather than emitting an empty/incorrect catalog predicate.
        let sql = list_relations_sql(ResolvedQuoting::trues(), &catalog_and_schema("", "main"));
        assert!(sql.ends_with("WHERE table_schema = 'main'"));
        assert!(!sql.contains("lower(table_catalog)"));
        assert!(!sql.contains(" AND "));
    }

    fn with_v2_view(yaml: &str, test: impl FnOnce(&DbtCatalogsV2View<'_>)) {
        let parsed: dbt_yaml::Value = dbt_yaml::from_str(yaml).expect("valid YAML");
        let dbt_yaml::Value::Mapping(repr, span) = parsed else {
            panic!("expected YAML mapping");
        };
        let catalogs = DbtCatalogs::new(repr, span);
        let view = catalogs.view_v2().expect("valid catalogs v2 view");
        test(&view);
    }

    #[test]
    fn duckdb_v2_external_iceberg_catalogs_disable_schema_listing() {
        with_v2_view(
            r#"
catalogs:
  - name: lakekeeper
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: http://localhost:8181/catalog
        warehouse: demo
        catalog_database: iceberg_demo
  - name: horizon_demo
    type: horizon
    table_format: iceberg
    config:
      duckdb:
        endpoint: https://horizon.example.com/catalog
        warehouse: horizon_wh
  - name: unity_demo
    type: unity
    table_format: iceberg
    config:
      duckdb:
        endpoint: https://dbc.example.com/api/2.1/unity-catalog/iceberg
        catalog_database: unity_db
  - name: files
    type: local_filesystem
    table_format: default
    config:
      duckdb:
        root_path: local_files
        file_format: csv
"#,
            |view| {
                assert!(
                    view.catalogs
                        .iter()
                        .any(|catalog| catalog.external_iceberg_attach().is_some())
                );
                // Horizon/Unity attach via Iceberg REST too, so they get the
                // same introspection routing as a generic iceberg_rest catalog.
                assert!(
                    view.external_iceberg_attach_for_database("horizon_demo")
                        .is_some()
                );
                assert!(
                    view.external_iceberg_attach_for_database("unity_db")
                        .is_some()
                );
                assert!(
                    view.external_iceberg_attach_for_database("iceberg_demo")
                        .is_some()
                );
                assert!(
                    view.external_iceberg_attach_for_database("local_files")
                        .is_none()
                );
                assert!(view.external_iceberg_attach_for_database("").is_none());
            },
        );
    }

    #[test]
    fn table_format_for_database_resolves_aliases() {
        with_v2_view(
            r#"
catalogs:
  - name: lake
    type: ducklake
    table_format: default
    config:
      duckdb:
        metadata_path: meta.ducklake
  - name: rest
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: http://localhost:8181/catalog
        warehouse: demo
  - name: remote_catalog
    type: local_filesystem
    table_format: default
    config:
      duckdb:
        root_path: /tmp/remote
        catalog_database: remote_db
"#,
            |view| {
                assert_eq!(view.table_format_for_database("lake"), Some("ducklake"));
                assert_eq!(view.table_format_for_database("rest"), Some("iceberg"));
                assert_eq!(view.table_format_for_database("remote_db"), Some("default"));
                assert_eq!(view.table_format_for_database("missing"), None);
            },
        );
    }

    #[test]
    fn classify_attach_entry_resolves_format_and_alias() {
        let parse =
            |yaml: &str| -> dbt_yaml::Value { dbt_yaml::from_str(yaml).expect("valid YAML") };

        // Explicit type wins and the alias is taken verbatim.
        assert_eq!(
            classify_attach_entry(&parse("path: /data/lake\ntype: iceberg\nalias: ice")),
            Some(("ice".to_string(), "iceberg"))
        );
        assert_eq!(
            classify_attach_entry(&parse("path: /data/dl\ntype: ducklake\nalias: dl")),
            Some(("dl".to_string(), "ducklake"))
        );
        // The is_ducklake flag also selects ducklake.
        assert_eq!(
            classify_attach_entry(&parse("path: /data/dl\nis_ducklake: true\nalias: dl2")),
            Some(("dl2".to_string(), "ducklake"))
        );
        // Iceberg takes precedence when an entry is tagged both ways.
        assert_eq!(
            classify_attach_entry(&parse(
                "path: /data/x\ntype: iceberg\nis_ducklake: true\nalias: both"
            )),
            Some(("both".to_string(), "iceberg"))
        );
        // Neither iceberg nor ducklake -> not routed.
        assert_eq!(
            classify_attach_entry(&parse("path: /data/plain\nalias: plain")),
            None
        );
        // Missing path / non-mapping -> not routed.
        assert_eq!(
            classify_attach_entry(&parse("type: iceberg\nalias: x")),
            None
        );
        assert_eq!(classify_attach_entry(&parse("- a\n- b")), None);
        // Alias falls back to the database parsed from the path when absent.
        assert!(matches!(
            classify_attach_entry(&parse("path: /data/warehouse.duckdb\ntype: iceberg")),
            Some((alias, "iceberg")) if !alias.is_empty()
        ));
    }
}
