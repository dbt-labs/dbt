use crate::adapter::adapter_impl::AdapterImpl;
use crate::connection::AdapterConnectionFactory;
use crate::errors::*;
use crate::metadata::CatalogAndSchema;
use crate::metadata::freshness_overrides::{
    FreshnessTask, FreshnessTaskResult, apply_freshness_task_result, run_override_query,
};
use crate::metadata::*;
use crate::query_ctx::query_ctx_from_state;
use crate::record_batch::{RecordBatchExt, StructArrayExt};
use crate::relation::Relation;
use crate::time_machine::{args_freshness_with_overrides, with_time_machine_metadata_wrapper};
use crate::{AdapterEngine, AdapterResult};

use arrow_array::*;
use arrow_schema::*;
use dbt_adapter_core::AdapterType;
use dbt_adapter_core::ExecutionPhase;
use dbt_adapter_engine::MapReduce;
use dbt_adbc::*;
use dbt_common::cancellation::Cancellable;
use dbt_common::cancellation::CancellationToken;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::normalize_quote;
use dbt_schemas::schemas::dbt_column::DbtColumn;
use dbt_schemas::schemas::legacy_catalog::*;
use dbt_schemas::schemas::relations::base::*;
use indexmap::IndexMap;
use minijinja::State;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub mod object_options;

pub mod nested_projection;

// See: https://docs.cloud.google.com/bigquery/docs/information-schema-intro#region_qualifier
pub(crate) const BIGQUERY_REGION_PREFIX: &str = "region-";

pub const BIGQUERY_PSEUDOCOLUMNS: [&str; 7] = [
    "_PARTITIONTIME",
    "_PARTITIONDATE",
    "_FILE_NAME",
    "_TABLE_SUFFIX",
    "_CHANGE_TYPE",
    "_CHANGE_TIMESTAMP",
    "_CHANGE_SEQUENCE_NUMBER",
];

pub fn list_relations(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let relations = list_relations_via_adbc(engine, conn, db_schema)?;
    let connection_project = engine
        .config("execution_project")
        .or_else(|| engine.config("project"))
        .or_else(|| engine.config("database"));
    let (target_project, _) =
        normalize_quote(false, AdapterType::Bigquery, &db_schema.rendered_catalog);
    let is_cross_project =
        connection_project.is_some_and(|project| project.as_ref() != target_project);

    verify_empty_adbc_listing(relations, is_cross_project, || {
        list_relations_via_information_schema(engine, ctx, conn, db_schema, token)
    })
}

fn list_relations_via_information_schema(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let sql = format!(
        "SELECT
    table_catalog,
    table_schema,
    table_name,
    table_type
FROM 
    {db_schema}.INFORMATION_SCHEMA.TABLES"
    );

    let batch = engine.execute(None, conn, ctx, &sql, token)?;
    let table_names = batch.column_values::<StringArray>("table_name")?;
    let table_schemas = batch.column_values::<StringArray>("table_schema")?;
    let table_catalogs = batch.column_values::<StringArray>("table_catalog")?;
    let table_types = batch.column_values::<StringArray>("table_type")?;

    let mut result = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        let database = table_catalogs.value(i);
        let schema = table_schemas.value(i);
        let identifier = table_names.value(i);
        let relation_type =
            RelationType::from_adapter_type(AdapterType::Bigquery, table_types.value(i));

        result.push(Arc::new(
            Relation::new(
                AdapterType::Bigquery,
                database.to_string(),
                schema.to_string(),
                identifier.to_string(),
            )
            .with_relation_type(relation_type)
            .with_quoting(engine.quoting()),
        ) as Arc<dyn BaseRelation>);
    }
    Ok(result)
}

pub fn list_routines(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let sql = format!(
        "SELECT
    routine_catalog AS table_catalog,
    routine_schema AS table_schema,
    routine_name AS table_name,
    routine_type AS table_type
FROM
    {db_schema}.INFORMATION_SCHEMA.ROUTINES
WHERE
    routine_type != 'PROCEDURE'"
    );

    let batch = engine.execute(None, conn, ctx, &sql, token)?;
    let table_names = batch.column_values::<StringArray>("table_name")?;
    let table_schemas = batch.column_values::<StringArray>("table_schema")?;
    let table_catalogs = batch.column_values::<StringArray>("table_catalog")?;
    let table_types = batch.column_values::<StringArray>("table_type")?;

    let mut result = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        let database = table_catalogs.value(i);
        let schema = table_schemas.value(i);
        let identifier = table_names.value(i);
        let relation_type =
            RelationType::from_adapter_type(AdapterType::Bigquery, table_types.value(i));

        result.push(Arc::new(
            Relation::new(
                AdapterType::Bigquery,
                database.to_string(),
                schema.to_string(),
                identifier.to_string(),
            )
            .with_relation_type(relation_type)
            .with_quoting(engine.quoting()),
        ) as Arc<dyn BaseRelation>);
    }
    Ok(result)
}

fn verify_empty_adbc_listing<F>(
    relations: Vec<Arc<dyn BaseRelation>>,
    is_cross_project: bool,
    fallback: F,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>>
where
    F: FnOnce() -> AdapterResult<Vec<Arc<dyn BaseRelation>>>,
{
    // BigQuery GetObjects exposes only the connection project as a catalog, so a
    // different target project produces an empty result. Verify emptiness with
    // the target-qualified metadata query before the caller records the schema
    // as complete.
    // https://github.com/dbt-labs/bigquery-adbc/blob/c87c401a934c71783862dface246252d84f9d2e6/go/connection.go#L106-L123
    if is_cross_project && relations.is_empty() {
        fallback()
    } else {
        Ok(relations)
    }
}

fn list_relations_via_adbc(
    engine: &dyn AdapterEngine,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    // The driver expects unquoted values, so regardless of the adapter's config
    // we need to strip quotes.
    let (catalog, _) = normalize_quote(false, AdapterType::Bigquery, &db_schema.rendered_catalog);
    let (schema, _) = normalize_quote(false, AdapterType::Bigquery, &db_schema.rendered_schema);

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

    // Each row of the column `db_schema_tables` is a list of elements
    // with the shape:
    //   - table_name: utf8
    //   - table_type: utf8
    //   - table_columns: list
    //   - table_constraints: list
    let db_schema_tables = schemas_struct.column_as::<ListArray>("db_schema_tables")?;

    let tables_struct = db_schema_tables
        .values()
        .as_any()
        .downcast_ref::<StructArray>()
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                "Missing or invalid 'db_schema_tables' values",
            )
        })?;

    let table_names = tables_struct.column_as::<StringArray>("table_name")?;
    let table_types = tables_struct.column_as::<StringArray>("table_type")?;

    let mut result = Vec::with_capacity(tables_struct.len());
    for j in 0..tables_struct.len() {
        let identifier = table_names.value(j);
        let relation_type =
            RelationType::from_adapter_type(AdapterType::Bigquery, table_types.value(j));

        result.push(Arc::new(
            Relation::new(
                AdapterType::Bigquery,
                catalog.clone(),
                schema.clone(),
                identifier.to_string(),
            )
            .with_relation_type(relation_type)
            .with_quoting(engine.quoting()),
        ) as Arc<dyn BaseRelation>);
    }

    Ok(result)
}

/// Represent nested data types (struct/array) for BigQuery
/// Leaf nodes are primitive types
/// For example column names "a.b", "a.c", "a.c.d" will be
///  a (struct)
///  /\
/// b  c (struct)
///     \
///      d
#[derive(Debug, Default)]
struct NestedColumnDataTypes {
    root: TrieNode,
}

#[derive(Debug, Default)]
struct TrieNode {
    pub children: IndexMap<String, TrieNode>,
    pub data_type: Option<String>,
    pub rendered_constraints: Option<String>,
}

impl NestedColumnDataTypes {
    pub fn insert(
        &mut self,
        column_name: &str,
        column_type: Option<&str>,
        rendered_constraints: Option<&str>,
    ) {
        let names = column_name.split(".");
        let mut node = &mut self.root;
        for name in names {
            node = node.children.entry(name.to_owned()).or_default();
        }
        node.data_type = column_type.map(str::to_owned);
        node.rendered_constraints = match (column_type, rendered_constraints) {
            (Some(data_type), Some(constraints))
                if !data_type.is_empty() && !constraints.is_empty() =>
            {
                Some(constraints.to_owned())
            }
            _ => None,
        };
    }

    pub fn format_top_level_columns_data_types(&self) -> IndexMap<String, String> {
        let mut result = IndexMap::new();
        for (column_name, node) in &self.root.children {
            let data_type = match &node.data_type {
                None => {
                    let inner_data_type = node.format_data_type();
                    format!("struct<{inner_data_type}>")
                }
                Some(data_type) => match data_type.as_str() {
                    "struct" => {
                        let inner_data_type = node.format_data_type();
                        format!("struct<{inner_data_type}>")
                    }
                    "array" => {
                        let inner_data_type = node.format_data_type();
                        format!("array<struct<{inner_data_type}>>")
                    }
                    // assume any struct or array type is a primitive type
                    _ => {
                        // ensure no sub fields
                        if node.children.is_empty() {
                            data_type.to_owned()
                        }
                        // sub fields exist -> it's actually not a primitive type -> default to struct
                        // this is to be consistent with dbt compile behavior
                        else {
                            let inner_data_type = node.format_data_type();
                            format!("struct<{inner_data_type}>")
                        }
                    }
                },
            };
            result.insert(
                column_name.to_owned(),
                node.append_rendered_constraints(data_type),
            );
        }
        result
    }
}

impl TrieNode {
    // TODO: refactor since this method is very much overlapped with `format_top_level_columns_data_types`
    fn format_data_type(&self) -> String {
        let mut result = vec![];
        for (column_name, node) in &self.children {
            let data_type = match &node.data_type {
                None => {
                    let inner_data_type = node.format_data_type();
                    if inner_data_type.is_empty() {
                        column_name.to_owned()
                    } else {
                        format!("{column_name} struct<{inner_data_type}>")
                    }
                }
                Some(data_type) => match data_type.as_str() {
                    "struct" => {
                        let inner_data_type = node.format_data_type();
                        format!("{column_name} struct<{inner_data_type}>")
                    }
                    "array" => {
                        let inner_data_type = node.format_data_type();
                        format!("{column_name} array<struct<{inner_data_type}>>")
                    }
                    _ => {
                        if node.children.is_empty() {
                            format!("{column_name} {data_type}")
                        } else {
                            let inner_data_type = node.format_data_type();
                            format!("{column_name} struct<{inner_data_type}>")
                        }
                    }
                },
            };
            result.push(node.append_rendered_constraints(data_type));
        }
        result.join(", ")
    }

    fn append_rendered_constraints(&self, data_type: String) -> String {
        match &self.rendered_constraints {
            Some(constraints) => format!("{data_type} {constraints}"),
            None => data_type,
        }
    }
}

/// Collapses dotted-path nested columns like `{"b.nested": ..., "b.nested2": ...}` into a
/// single struct column `b: struct<nested ..., nested2 ...>`, arbitrarily deep (see
/// tests/data/nest_column_data_types). Based on pydoc and observed dbt-core behavior
/// (https://github.com/dbt-labs/dbt-core/blob/main/env/lib/python3.12/site-packages/dbt/adapters/bigquery/column.py#L131-L132),
/// not a full spec, so corner cases may not be handled.
///
/// Constraints on nested fields are preserved when dotted column names are collapsed.
pub fn nest_column_data_types(
    columns: IndexMap<String, DbtColumn>,
    constraints: Option<BTreeMap<String, String>>,
) -> AdapterResult<IndexMap<String, DbtColumn>> {
    let constraints = constraints.unwrap_or_default();
    let mut result = NestedColumnDataTypes::default();
    for (column_name, column) in &columns {
        result.insert(
            column_name,
            column.data_type.as_deref(),
            constraints.get(column_name).map(String::as_str),
        )
    }
    let column_to_data_type = result.format_top_level_columns_data_types();
    let mut result = IndexMap::new();
    for (column_name, data_type) in &column_to_data_type {
        match columns.get(column_name) {
            Some(column) => result.insert(
                column_name.clone(),
                DbtColumn {
                    name: column.name.clone(),
                    data_type: Some(data_type.clone()),
                    description: column.description.clone(),
                    constraints: column.constraints.clone(),
                    meta: column.meta.clone(),
                    tags: column.tags.clone(),
                    policy_tags: column.policy_tags.clone(),
                    classifiers: column.classifiers.clone(),
                    databricks_tags: column.databricks_tags.clone(),
                    column_mask: column.column_mask.clone(),
                    quote: column.quote,
                    codec: column.codec.clone(),
                    ttl: column.ttl.clone(),
                    deprecated_config: column.deprecated_config.clone(),
                    dimension: column.dimension.clone(),
                    entity: column.entity.clone(),
                    granularity: column.granularity.clone(),
                },
            ),
            None => result.insert(
                column_name.clone(),
                DbtColumn {
                    name: column_name.to_owned(),
                    data_type: Some(data_type.clone()),
                    description: None,
                    constraints: vec![],
                    meta: IndexMap::new(),
                    tags: vec![],
                    policy_tags: None,
                    classifiers: None,
                    databricks_tags: None,
                    column_mask: None,
                    quote: None,
                    codec: None,
                    ttl: None,
                    deprecated_config: Default::default(),
                    dimension: None,
                    entity: None,
                    granularity: None,
                },
            ),
        };
    }
    Ok(result)
}

#[derive(Debug)]
pub enum QualifierRequirement {
    DatasetOnly,
    RegionOnly,
    DatasetOrRegion,
}

// TODO: currently not using _optional_include_project and _optional_exclude_region, but could be useful so capturing that info here
#[derive(Debug)]
pub struct QualifierOptions {
    // Most views allow passing an optional project qualifier. Will use default project when not specified
    pub _optional_include_project: bool,
    // Some RegionOnly views allow excluding the region and will default to US
    pub _optional_exclude_region: bool,
    pub requirement: QualifierRequirement,
}

impl QualifierOptions {
    pub const fn new(
        optional_include_project: bool,
        optional_exclude_region: bool,
        requirement: QualifierRequirement,
    ) -> Self {
        Self {
            _optional_include_project: optional_include_project,
            _optional_exclude_region: optional_exclude_region,
            requirement,
        }
    }
}

// Shared QualifierOptions singletons for match-based lookup
static QO_REGION_TF: QualifierOptions =
    QualifierOptions::new(true, false, QualifierRequirement::RegionOnly);
static QO_REGION_FF: QualifierOptions =
    QualifierOptions::new(false, false, QualifierRequirement::RegionOnly);
static QO_REGION_TT: QualifierOptions =
    QualifierOptions::new(true, true, QualifierRequirement::RegionOnly);
static QO_DATASET_TF: QualifierOptions =
    QualifierOptions::new(true, false, QualifierRequirement::DatasetOnly);
static QO_DS_OR_REGION_TF: QualifierOptions =
    QualifierOptions::new(true, false, QualifierRequirement::DatasetOrRegion);

/// Find the qualifier options and requirements for a known view in info schema.
///
/// This should be an exhaustive list of all known views in BQ's INFO SCHEMA. They
/// are organized in the same order as the documentation to make it easier to find
/// any new, missing views that need to be accounted for.
///
/// NOTE: BY_PROJECT views have an alias stripping that suffix.
///
/// NOTE: on the `region` qualifier: some `INFORMATION_SCHEMA` views require it [1], others
/// (like `TABLES`) accept region or dataset instead [2], and if omitted the engine defaults
/// to the US macro location, which may route to any US region [3].
///
/// [1] https://cloud.google.com/bigquery/docs/information-schema-intro#syntax
/// [2] https://cloud.google.com/bigquery/docs/information-schema-intro#dataset_qualifier
/// [3] https://cloud.google.com/bigquery/docs/locations#specify_locations
fn qualifier_options_for_info_schema_view(
    sys_identifier: &str,
) -> Option<&'static QualifierOptions> {
    match sys_identifier {
        // Access control
        "OBJECT_PRIVILEGES" => Some(&QO_REGION_TF),

        // BI Engine
        "BI_CAPACITIES" | "BI_CAPACITY_CHANGES" => Some(&QO_REGION_TF),

        // Configurations
        "EFFECTIVE_PROJECT_OPTIONS"
        | "ORGANIZATION_OPTIONS"
        | "ORGANIZATION_OPTIONS_CHANGES"
        | "PROJECT_OPTIONS"
        | "PROJECT_OPTIONS_CHANGES" => Some(&QO_REGION_FF),

        // Datasets
        "SCHEMATA" | "SCHEMATA_LINKS" | "SCHEMATA_OPTIONS" | "SHARED_DATASET_USAGE" => {
            Some(&QO_REGION_TT)
        }
        "SCHEMATA_REPLICAS" | "SCHEMATA_REPLICAS_BY_FAILOVER_RESERVATION" => Some(&QO_REGION_TF),

        // Jobs
        "JOBS" | "JOBS_BY_PROJECT" | "JOBS_BY_USER" | "JOBS_BY_FOLDER" | "JOBS_BY_ORGANIZATION" => {
            Some(&QO_REGION_TF)
        }

        // Jobs by timeslice
        "JOBS_TIMELINE"
        | "JOBS_TIMELINE_BY_PROJECT"
        | "JOBS_TIMELINE_BY_USER"
        | "JOBS_TIMELINE_BY_FOLDER"
        | "JOBS_TIMELINE_BY_ORGANIZATION" => Some(&QO_REGION_TF),

        // Recommendations and insights
        "INSIGHTS"
        | "INSIGHTS_BY_PROJECT"
        | "RECOMMENDATIONS"
        | "RECOMMENDATIONS_BY_PROJECT"
        | "RECOMMENDATIONS_BY_ORGANIZATION" => Some(&QO_REGION_TF),

        // Reservations
        "ASSIGNMENTS"
        | "ASSIGNMENTS_BY_PROJECT"
        | "ASSIGNMENT_CHANGES"
        | "ASSIGNMENT_CHANGES_BY_PROJECT"
        | "CAPACITY_COMMITMENTS"
        | "CAPACITY_COMMITMENTS_BY_PROJECT"
        | "CAPACITY_COMMITMENT_CHANGES"
        | "CAPACITY_COMMITMENT_CHANGES_BY_PROJECT"
        | "RESERVATIONS"
        | "RESERVATIONS_BY_PROJECT"
        | "RESERVATION_CHANGES"
        | "RESERVATION_CHANGES_BY_PROJECT"
        | "RESERVATIONS_TIMELINE"
        | "RESERVATIONS_TIMELINE_BY_PROJECT" => Some(&QO_REGION_TF),

        // Routines
        "PARAMETERS" | "ROUTINES" | "ROUTINE_OPTIONS" => Some(&QO_DS_OR_REGION_TF),

        // Search indexes
        "SEARCH_INDEXES"
        | "SEARCH_INDEX_COLUMNS"
        | "SEARCH_INDEX_COLUMN_OPTIONS"
        | "SEARCH_INDEX_OPTIONS" => Some(&QO_DATASET_TF),
        "SEARCH_INDEXES_BY_ORGANIZATION" => Some(&QO_REGION_TF),

        // Sessions
        "SESSIONS" | "SESSIONS_BY_PROJECT" | "SESSIONS_BY_USER" => Some(&QO_REGION_TF),

        // Streaming
        "STREAMING_TIMELINE"
        | "STREAMING_TIMELINE_BY_PROJECT"
        | "STREAMING_TIMELINE_BY_FOLDER"
        | "STREAMING_TIMELINE_BY_ORGANIZATION" => Some(&QO_REGION_TF),

        // Tables
        "COLUMNS" | "COLUMN_FIELD_PATHS" | "TABLES" | "TABLE_OPTIONS" => Some(&QO_DS_OR_REGION_TF),
        "CONSTRAINT_COLUMN_USAGE"
        | "KEY_COLUMN_USAGE"
        | "PARTITIONS"
        | "TABLE_CONSTRAINTS"
        | "TABLE_SNAPSHOTS" => Some(&QO_DATASET_TF),
        "TABLE_STORAGE"
        | "TABLE_STORAGE_BY_PROJECT"
        | "TABLE_STORAGE_BY_FOLDER"
        | "TABLE_STORAGE_BY_ORGANIZATION"
        | "TABLE_STORAGE_USAGE_TIMELINE"
        | "TABLE_STORAGE_USAGE_TIMELINE_BY_FOLDER"
        | "TABLE_STORAGE_USAGE_TIMELINE_BY_ORGANIZATION" => Some(&QO_REGION_TF),

        // Vector indexes
        "VECTOR_INDEXES" | "VECTOR_INDEX_COLUMNS" | "VECTOR_INDEX_OPTIONS" => Some(&QO_DATASET_TF),

        // Views
        "VIEWS" | "MATERIALIZED_VIEWS" => Some(&QO_DS_OR_REGION_TF),

        // Write API
        "WRITE_API_TIMELINE"
        | "WRITE_API_TIMELINE_BY_PROJECT"
        | "WRITE_API_TIMELINE_BY_FOLDER"
        | "WRITE_API_TIMELINE_BY_ORGANIZATION" => Some(&QO_REGION_TF),

        _ => None,
    }
}

/// Generate the fully qualified name of a BigQuery INFORMATION_SCHEMA table.
///
/// BQ's info schema tables have unique way of handling qualifiers. Instead of a
/// "database", the qualifier is something like [<project_id>.]<region_or_dataset_id>.
/// but in some cases the region is also optional.
///
/// See `qualifier_options_for_info_schema_view` for specific view requirements.
///
/// NOTE: this function is lenient and will return invalid qualifiers if the specific
/// requirements are not met. I.e it will return stuff that might break if ran on BigQuery.
///
/// TODO: We're currently ignoring any differences between the provided qualifier and
///       the specific requirements for the given view name since this is only used to
///       fetch the view schema, so the specific location doesn't really matter.
pub(crate) fn generate_system_table_fqn(
    dataset: &str,
    view_name: &str,
    user_preferred_region: Option<&str>,
) -> String {
    let sys_identifier = view_name.to_uppercase();

    match qualifier_options_for_info_schema_view(&sys_identifier) {
        Some(qualifier_option) => match qualifier_option.requirement {
            QualifierRequirement::RegionOnly => {
                let region = user_preferred_region.unwrap_or("us");
                format!("`region-{region}`.INFORMATION_SCHEMA.{sys_identifier}")
            }
            QualifierRequirement::DatasetOnly => {
                format!("{dataset}.INFORMATION_SCHEMA.{sys_identifier}")
            }
            QualifierRequirement::DatasetOrRegion => {
                // respect user's location preferences by querying the region directly if
                // possible
                match user_preferred_region {
                    None => format!("{dataset}.INFORMATION_SCHEMA.{sys_identifier}"),
                    Some(region) => {
                        format!("`region-{region}`.INFORMATION_SCHEMA.{sys_identifier}")
                    }
                }
            }
        },
        // This is technically an error, but we'll just let it fail when querying BQ
        None => format!("INFORMATION_SCHEMA.{sys_identifier}"),
    }
}

pub fn build_relation_clauses_bigquery(
    relations: &[Arc<dyn BaseRelation>],
) -> AdapterResult<(WhereClausesByDb, RelationsByDb)> {
    let mut where_by_db = BTreeMap::<String, Vec<String>>::new();
    let mut rels_by_db = BTreeMap::<String, Vec<Arc<dyn BaseRelation>>>::new();

    for rel in relations {
        let project = rel.database_as_resolved_str()?;
        let dataset = rel.schema_as_resolved_str()?;
        let table = rel.identifier_as_resolved_str()?;

        // Backtick-quote the project so dotted/dashed project IDs parse as
        // a single identifier in `<db>.__TABLES__`.
        let db_key = format!("`{project}`.{dataset}");

        where_by_db
            .entry(db_key.clone())
            .or_default()
            .push(format!("table_id = '{table}'"));

        rels_by_db.entry(db_key).or_default().push(rel.clone());
    }

    Ok((where_by_db, rels_by_db))
}

/// Build SQL to fetch last-modified timestamps from `{project}.{dataset}.__TABLES__`.
///
/// `where_clauses` is already scoped to this (project, dataset) group.
///
/// __TABLES__ is officially deprecated in favor of TABLES and PARTITIONS, but neither has
/// last_modified_time. Bigquery's API has get_table. But for customers with larger source
/// freshness workloads fanning out over all individual relations can trigger API limiting
/// errors or run up larger bills.
///
/// reference: https://discuss.google.dev/t/information-schema-tables-monitoring-last-modified-time/125698
fn build_tables_freshness_query(database: &str, where_clauses: &[String]) -> String {
    let joined_where_clauses = where_clauses.join(" OR ");
    build_external_table_freshness_query(
        &format!("{database}.__TABLES__"),
        &format!("{database}.INFORMATION_SCHEMA.TABLES"),
        &format!("\n             WHERE {joined_where_clauses}"),
    )
}

fn build_schema_freshness_query(database: &str, schema: &str) -> String {
    build_external_table_freshness_query(
        &format!("`{database}`.`{schema}`.__TABLES__"),
        &format!("`{database}`.`{schema}`.INFORMATION_SCHEMA.TABLES"),
        "",
    )
}

fn build_external_table_freshness_query(
    legacy_tables_fqn: &str,
    info_schema_tables_fqn: &str,
    where_clause: &str,
) -> String {
    format!(
        "SELECT
                 legacy.dataset_id AS table_schema,
                 legacy.table_id AS table_name,
                 IF(info.table_type = 'EXTERNAL', NULL, TIMESTAMP_MILLIS(legacy.last_modified_time)) AS last_altered,
                 (legacy.type = 2) AS is_view
             FROM {legacy_tables_fqn} legacy
             LEFT JOIN {info_schema_tables_fqn} info
               ON info.table_schema = legacy.dataset_id
              AND info.table_name = legacy.table_id{where_clause}",
    )
}

fn query_tables_freshness(
    adapter: &AdapterImpl,
    conn: &mut dyn Connection,
    database: &str,
    where_clauses: &[String],
    token: CancellationToken,
) -> AdapterResult<Arc<RecordBatch>> {
    let sql = build_tables_freshness_query(database, where_clauses);
    let ctx = QueryCtx::default().with_desc("Extracting freshness from information schema");
    let (_, agate_table) = adapter.query(&ctx, conn, &sql, None, token)?;
    Ok(agate_table.original_record_batch())
}

fn accumulate_tables_freshness_from_batch(
    acc: &mut BTreeMap<String, MetadataFreshness>,
    batch: &RecordBatch,
    database: &str,
    relations_by_database: &RelationsByDb,
) -> AdapterResult<()> {
    let schemas = batch.column_values::<StringArray>("table_schema")?;
    let tables = batch.column_values::<StringArray>("table_name")?;
    let timestamps = batch.column_values::<TimestampMicrosecondArray>("last_altered")?;
    let is_views = batch.column_values::<BooleanArray>("is_view")?;
    let relations = &relations_by_database[database];
    for i in 0..batch.num_rows() {
        if timestamps.is_null(i) {
            continue;
        }
        let schema = schemas.value(i);
        let table = tables.value(i);
        let timestamp = timestamps.value(i);
        let is_view = is_views.value(i);
        for table_name in find_matching_relation(schema, table, relations)? {
            acc.insert(
                table_name,
                MetadataFreshness::from_micros(timestamp, is_view)?,
            );
        }
    }
    Ok(())
}

fn run_bulk_tables_freshness(
    adapter: &AdapterImpl,
    conn: &mut dyn Connection,
    relations: &[Arc<dyn BaseRelation>],
    token: CancellationToken,
) -> AdapterResult<BTreeMap<String, MetadataFreshness>> {
    let (where_clauses_by_database, relations_by_database) =
        build_relation_clauses_bigquery(relations)?;
    let mut acc = BTreeMap::new();
    for (database, where_clauses) in where_clauses_by_database {
        let batch =
            match query_tables_freshness(adapter, conn, &database, &where_clauses, token.clone()) {
                Ok(batch) => batch,
                Err(e) if is_bigquery_not_found_error(&e) => continue,
                Err(e) => return Err(e),
            };
        accumulate_tables_freshness_from_batch(
            &mut acc,
            &batch,
            &database,
            &relations_by_database,
        )?;
    }
    Ok(acc)
}

fn bulk_freshness_tasks_from_relations(
    relations: &[Arc<dyn BaseRelation>],
) -> AdapterResult<Vec<FreshnessTask>> {
    let (_, relations_by_database) = build_relation_clauses_bigquery(relations)?;
    Ok(relations_by_database
        .into_values()
        .map(FreshnessTask::Bulk)
        .collect())
}

fn run_freshness_task(
    adapter: &AdapterImpl,
    conn: &mut dyn Connection,
    task: &FreshnessTask,
    token: CancellationToken,
) -> AdapterResult<FreshnessTaskResult> {
    match task {
        FreshnessTask::Bulk(bulk) => {
            let acc = run_bulk_tables_freshness(adapter, conn, bulk, token)?;
            Ok(FreshnessTaskResult::Bulk(acc))
        }
        FreshnessTask::Override(relation, ovr) => {
            run_override_query(adapter, conn, relation, ovr, token)
        }
    }
}

/// Render the SQL for fetching view definitions from
/// `<project>.<dataset>.INFORMATION_SCHEMA.VIEWS` for a list of table identifiers.
///
/// Both `project` and `dataset` are wrapped in backticks unconditionally — BQ
/// requires them for identifiers containing `-`, and unconditional quoting is
/// simpler than detecting the "needs quotes" case.
///
/// Identifiers are interpolated into a `IN ('...', '...')` list with `'`
/// escaped to `''` defensively.
fn build_views_query(project: &str, dataset: &str, identifiers: &[String]) -> String {
    let literals = identifiers
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "SELECT
    table_catalog,
    table_schema,
    table_name,
    view_definition
FROM `{project}`.`{dataset}`.INFORMATION_SCHEMA.VIEWS
WHERE table_name IN ({literals})"
    )
}

pub struct BigqueryMetadataAdapter {
    adapter: AdapterImpl,
}

impl BigqueryMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }

    fn freshness_mapreduce(
        &self,
        tasks: Vec<FreshnessTask>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        type Acc = BTreeMap<String, MetadataFreshness>;

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));
        let adapter_for_map = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &mut dyn Connection, task: &FreshnessTask| {
            run_freshness_task(&adapter_for_map, conn, task, token_clone.clone())
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

    fn freshness_with_overrides_impl<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        overrides: &'a BTreeMap<String, FreshnessOverride>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        if overrides.is_empty() {
            return self.freshness_inner(relations, token);
        }

        let mut override_targets = Vec::new();
        let mut bulk_relations = Vec::new();
        for relation in relations {
            if let Some(ovr) = overrides.get(&relation.semantic_fqn()) {
                override_targets.push((Arc::clone(relation), ovr.clone()));
            } else {
                bulk_relations.push(Arc::clone(relation));
            }
        }

        let mut tasks: Vec<FreshnessTask> = Vec::new();
        if !bulk_relations.is_empty() {
            match bulk_freshness_tasks_from_relations(&bulk_relations) {
                Ok(bulk_tasks) => tasks.extend(bulk_tasks),
                Err(e) => {
                    let future = async move { Err(Cancellable::Error(e)) };
                    return Box::pin(future);
                }
            }
        }
        for (relation, ovr) in override_targets {
            tasks.push(FreshnessTask::Override(relation, ovr));
        }

        self.freshness_mapreduce(tasks, token)
    }
}

impl MetadataAdapter for BigqueryMetadataAdapter {
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

        let date_shards_label =
            stats_sql_result.column_values::<StringArray>("stats__date_shards__label")?;
        let date_shards_value =
            stats_sql_result.column_values::<Int64Array>("stats__date_shards__value")?;
        let date_shards_description =
            stats_sql_result.column_values::<StringArray>("stats__date_shards__description")?;
        let date_shards_include =
            stats_sql_result.column_values::<BooleanArray>("stats__date_shards__include")?;

        let date_shard_min_label =
            stats_sql_result.column_values::<StringArray>("stats__date_shard_min__label")?;
        let date_shard_min_value =
            stats_sql_result.column_values::<StringArray>("stats__date_shard_min__value")?;
        let date_shard_min_description =
            stats_sql_result.column_values::<StringArray>("stats__date_shard_min__description")?;
        let date_shard_min_include =
            stats_sql_result.column_values::<BooleanArray>("stats__date_shard_min__include")?;

        let date_shard_max_label =
            stats_sql_result.column_values::<StringArray>("stats__date_shard_max__label")?;
        let date_shard_max_value =
            stats_sql_result.column_values::<StringArray>("stats__date_shard_max__value")?;
        let date_shard_max_description =
            stats_sql_result.column_values::<StringArray>("stats__date_shard_max__description")?;
        let date_shard_max_include =
            stats_sql_result.column_values::<BooleanArray>("stats__date_shard_max__include")?;

        let num_rows_label =
            stats_sql_result.column_values::<StringArray>("stats__num_rows__label")?;
        let num_rows_value =
            stats_sql_result.column_values::<Int64Array>("stats__num_rows__value")?;
        let num_rows_description =
            stats_sql_result.column_values::<StringArray>("stats__num_rows__description")?;
        let num_rows_include =
            stats_sql_result.column_values::<BooleanArray>("stats__num_rows__include")?;

        let bytes_label =
            stats_sql_result.column_values::<StringArray>("stats__num_bytes__label")?;
        let bytes_value =
            stats_sql_result.column_values::<Int64Array>("stats__num_bytes__value")?;
        let bytes_description =
            stats_sql_result.column_values::<StringArray>("stats__num_bytes__description")?;
        let bytes_include =
            stats_sql_result.column_values::<BooleanArray>("stats__num_bytes__include")?;

        let partition_type_label =
            stats_sql_result.column_values::<StringArray>("stats__partitioning_type__label")?;
        let partition_type_value =
            stats_sql_result.column_values::<StringArray>("stats__partitioning_type__value")?;
        let partition_type_description = stats_sql_result
            .column_values::<StringArray>("stats__partitioning_type__description")?;
        let partition_type_include =
            stats_sql_result.column_values::<BooleanArray>("stats__partitioning_type__include")?;

        let clustering_fields_label =
            stats_sql_result.column_values::<StringArray>("stats__clustering_fields__label")?;
        let clustering_fields_value =
            stats_sql_result.column_values::<StringArray>("stats__clustering_fields__value")?;
        let clustering_fields_description = stats_sql_result
            .column_values::<StringArray>("stats__clustering_fields__description")?;
        let clustering_fields_include =
            stats_sql_result.column_values::<BooleanArray>("stats__clustering_fields__include")?;

        let mut result = BTreeMap::<String, CatalogTable>::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);
            let data_type = data_types.value(i);
            let comment = comments.value(i);

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            let entry = result.entry(fully_qualified_name.clone());

            if matches!(entry, Entry::Vacant(_)) {
                let date_shards_label_i = date_shards_label.value(i);
                let date_shards_value_i = date_shards_value.value(i);
                let date_shards_description_i = date_shards_description.value(i);
                let date_shards_include_i = date_shards_include.value(i);

                let date_shard_min_label_i = date_shard_min_label.value(i);
                let date_shard_min_value_i = date_shard_min_value.value(i);
                let date_shard_min_description_i = date_shard_min_description.value(i);
                let date_shard_min_include_i = date_shard_min_include.value(i);

                let date_shard_max_label_i = date_shard_max_label.value(i);
                let date_shard_max_value_i = date_shard_max_value.value(i);
                let date_shard_max_description_i = date_shard_max_description.value(i);
                let date_shard_max_include_i = date_shard_max_include.value(i);

                let num_rows_label_i = num_rows_label.value(i);
                let num_rows_value_i = num_rows_value.value(i);
                let num_rows_description_i = num_rows_description.value(i);
                let num_rows_include_i = num_rows_include.value(i);

                let bytes_label_i = bytes_label.value(i);
                let bytes_value_i = bytes_value.value(i);
                let bytes_description_i = bytes_description.value(i);
                let bytes_include_i = bytes_include.value(i);

                let partition_type_label_i = partition_type_label.value(i);
                let partition_type_value_i = partition_type_value.value(i);
                let partition_type_description_i = partition_type_description.value(i);
                let partition_type_include_i = partition_type_include.value(i);

                let clustering_fields_label_i = clustering_fields_label.value(i);
                let clustering_fields_value_i = clustering_fields_value.value(i);
                let clustering_fields_description_i = clustering_fields_description.value(i);
                let clustering_fields_include_i = clustering_fields_include.value(i);

                let mut stats = BTreeMap::new();

                if date_shards_include_i {
                    stats.insert(
                        "date_shards".to_string(),
                        CatalogNodeStats {
                            id: "date_shards".to_string(),
                            label: date_shards_label_i.to_string(),
                            value: serde_json::Value::String(date_shards_value_i.to_string()),
                            description: Some(date_shards_description_i.to_string()),
                            include: date_shards_include_i,
                        },
                    );
                }
                if date_shard_min_include_i {
                    stats.insert(
                        "date_shard_min".to_string(),
                        CatalogNodeStats {
                            id: "date_shard_min".to_string(),
                            label: date_shard_min_label_i.to_string(),
                            value: serde_json::Value::String(date_shard_min_value_i.to_string()),
                            description: Some(date_shard_min_description_i.to_string()),
                            include: date_shard_min_include_i,
                        },
                    );
                }
                if date_shard_max_include_i {
                    stats.insert(
                        "date_shard_max".to_string(),
                        CatalogNodeStats {
                            id: "date_shard_max".to_string(),
                            label: date_shard_max_label_i.to_string(),
                            value: serde_json::Value::String(date_shard_max_value_i.to_string()),
                            description: Some(date_shard_max_description_i.to_string()),
                            include: date_shard_max_include_i,
                        },
                    );
                }
                if num_rows_include_i {
                    stats.insert(
                        "num_rows".to_string(),
                        CatalogNodeStats {
                            id: "num_rows".to_string(),
                            label: num_rows_label_i.to_string(),
                            value: serde_json::Value::Number(num_rows_value_i.into()),
                            description: Some(num_rows_description_i.to_string()),
                            include: num_rows_include_i,
                        },
                    );
                }
                if bytes_include_i {
                    stats.insert(
                        "bytes".to_string(),
                        CatalogNodeStats {
                            id: "bytes".to_string(),
                            label: bytes_label_i.to_string(),
                            value: serde_json::Value::Number(bytes_value_i.into()),
                            description: Some(bytes_description_i.to_string()),
                            include: bytes_include_i,
                        },
                    );
                }
                if partition_type_include_i {
                    stats.insert(
                        "partition_type".to_string(),
                        CatalogNodeStats {
                            id: "partition_type".to_string(),
                            label: partition_type_label_i.to_string(),
                            value: serde_json::Value::String(partition_type_value_i.to_string()),
                            description: Some(partition_type_description_i.to_string()),
                            include: partition_type_include_i,
                        },
                    );
                }
                if clustering_fields_include_i {
                    stats.insert(
                        "clustering_fields".to_string(),
                        CatalogNodeStats {
                            id: "clustering_fields".to_string(),
                            label: clustering_fields_label_i.to_string(),
                            value: serde_json::Value::String(clustering_fields_value_i.to_string()),
                            description: Some(clustering_fields_description_i.to_string()),
                            include: clustering_fields_include_i,
                        },
                    );
                }

                stats.insert(
                    "has_stats".to_string(),
                    CatalogNodeStats {
                        id: "has_stats".to_string(),
                        label: "Has Stats?".to_string(),
                        value: serde_json::Value::Bool(stats.is_empty()),
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
                    owner: None,
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
        let column_indices = catalog_sql_result.column_values::<Int64Array>("column_index")?;
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
        _phase: Option<ExecutionPhase>,
        relations: &[Arc<dyn BaseRelation>],
        item_span_operation_id: Option<&str>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, HashMap<String, AdapterResult<Arc<Schema>>>> {
        // All results are accumulated in an unordered map
        type Acc = HashMap<String, AdapterResult<Arc<Schema>>>;

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));
        let node_id = unique_id.or_else(|| Some("sources".to_string()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          relation: &Arc<dyn BaseRelation>|
              -> AdapterResult<Arc<Schema>> {
            let project = relation.database_as_resolved_str()?;
            let dataset = relation.schema_as_resolved_str()?;
            let table = relation.identifier_as_resolved_str()?;

            // We can't use `get_table_schema` for INFORMATION_SCHEMA tables (the adbc
            // connection's googleapi backend doesn't support it) or query the COLUMNS
            // INFORMATION_SCHEMA view directly, so instead issue a query that returns the
            // minimum data and read the Arrow schema off the resulting batch.
            // TODO(jason): This needs to be resolved within the driver itself - querying this way returns IPC directly from the
            // storage API within the driver where it's currently not annotated with the original type text
            if relation.is_system() {
                let qualifier = relation.database_as_quoted_str()?;

                let user_preferred_region = adapter
                    .engine()
                    .config("location")
                    .map(|cfg| cfg.to_lowercase());

                let table_fqn =
                    generate_system_table_fqn(&qualifier, &table, user_preferred_region.as_deref());
                let sql = format!("SELECT * FROM {table_fqn} LIMIT 0");

                let ctx = QueryCtx::default().with_desc("Get table schema");
                let (_, agate_table) =
                    adapter.query(&ctx, &mut *conn, &sql, None, token_clone.clone())?;
                let batch = agate_table.original_record_batch();

                let schema = batch.schema();
                if schema.fields().is_empty() {
                    Err(AdapterError::new(
                        AdapterErrorKind::UnexpectedResult,
                        format!("BigQuery driver returned no schema for {table_fqn}"),
                    ))
                } else {
                    Ok(schema)
                }
            } else {
                let schema = conn
                    .get_table_schema(Some(&project), Some(&dataset), &table)
                    .map_err(adbc_error_to_adapter_error)?;
                let mut schema_builder = SchemaBuilder::from(schema.fields());

                if let Some(time_partitioning_type) = schema.metadata().get("TimePartitioning.Type")
                {
                    schema_builder.push(Field::new(
                        "_PARTITIONTIME",
                        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                        true,
                    ));
                    if time_partitioning_type == "DAY" {
                        schema_builder.push(Field::new("_PARTITIONDATE", DataType::Date32, true));
                    }
                }

                if let Some(schema_type) = schema.metadata().get("Type") {
                    if schema_type == "EXTERNAL" {
                        schema_builder.push(Field::new("_FILE_NAME", DataType::Utf8, true));
                    }
                }

                Ok(Arc::new(schema_builder.finish()))
            }
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
            node_id,
            token,
        )
    }

    fn list_relations_schemas_by_patterns_inner(
        &self,
        _patterns: &[RelationPattern],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        todo!("list_relations_schemas_by_patterns for BigQuery")
    }

    fn freshness_inner(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        let tasks = match bulk_freshness_tasks_from_relations(relations) {
            Ok(tasks) => tasks,
            Err(e) => {
                let future = async move { Err(Cancellable::Error(e)) };
                return Box::pin(future);
            }
        };
        self.freshness_mapreduce(tasks, token)
    }

    /// Honors per-source `loaded_at_field` / `loaded_at_query` config. Mirrors the
    /// dbt-core run-cache plugin: relations without overrides go through the bulk
    /// `__TABLES__` path; each override runs as one targeted query in parallel.
    /// Net call count: 1 bulk (over the non-override subset) + N override
    /// queries — same shape as the plugin.
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
            self.freshness_with_overrides_impl(relations, overrides, token),
        )
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
            // Deviation from core: we cannot use `list_tables` as this is not supported from ADBC
            // Pagination is handled in the ADBC driver
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
                    if is_bigquery_not_found_error(&e) {
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

    fn is_permission_error(&self, e: &AdapterError) -> bool {
        is_bigquery_permission_error(e)
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

        let mut by_triple: HashMap<(String, String, String), Arc<dyn BaseRelation>> =
            HashMap::new();
        let mut by_dataset: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();

        for rel in relations {
            let project = match rel.database_as_resolved_str() {
                Ok(p) => p,
                Err(e) => {
                    let err = AdapterError::from(e);
                    return Box::pin(async move { Err(Cancellable::Error(err)) });
                }
            };
            let dataset = match rel.schema_as_resolved_str() {
                Ok(s) => s,
                Err(e) => {
                    let err = AdapterError::from(e);
                    return Box::pin(async move { Err(Cancellable::Error(err)) });
                }
            };
            let table = match rel.identifier_as_resolved_str() {
                Ok(t) => t,
                Err(e) => {
                    let err = AdapterError::from(e);
                    return Box::pin(async move { Err(Cancellable::Error(err)) });
                }
            };

            by_triple.insert(
                (
                    project.to_lowercase(),
                    dataset.to_lowercase(),
                    table.to_lowercase(),
                ),
                rel.clone(),
            );
            by_dataset
                .entry((project, dataset))
                .or_default()
                .push(table);
        }

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          key: &((String, String), Vec<String>)|
              -> AdapterResult<Arc<RecordBatch>> {
            let ((project, dataset), identifiers) = key;
            let sql = build_views_query(project, dataset, identifiers);
            let ctx = QueryCtx::default().with_desc("Fetch view definitions");
            let (_, table) = adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            Ok(table.original_record_batch())
        };

        let by_triple = Arc::new(by_triple);
        let reduce_f = move |acc: &mut Acc,
                             _key: ((String, String), Vec<String>),
                             batch_res: AdapterResult<Arc<RecordBatch>>|
              -> Result<(), Cancellable<AdapterError>> {
            let batch = batch_res?;
            let catalogs = batch.column_values::<StringArray>("table_catalog")?;
            let schemas = batch.column_values::<StringArray>("table_schema")?;
            let names = batch.column_values::<StringArray>("table_name")?;
            let defs = batch.column_values::<StringArray>("view_definition")?;

            for i in 0..batch.num_rows() {
                if defs.is_null(i) {
                    continue;
                }
                let catalog = catalogs.value(i);
                let schema = schemas.value(i);
                let name = names.value(i);
                let definition = defs.value(i);

                let key = (
                    catalog.to_lowercase(),
                    schema.to_lowercase(),
                    name.to_lowercase(),
                );
                let Some(input_rel) = by_triple.get(&key) else {
                    continue;
                };

                acc.definitions.push(ViewDefinition {
                    fqn: input_rel.semantic_fqn(),
                    definition: definition.to_string(),
                    dialect: AdapterType::Bigquery,
                    default_catalog: catalog.to_string(),
                    default_schema: schema.to_string(),
                });
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        let keys = by_dataset.into_iter().collect::<Vec<_>>();
        map_reduce.run(Arc::new(keys), token)
    }

    fn supports_bulk_freshness_dump(&self) -> bool {
        true
    }

    fn freshness_all_in_schema_inner<'a>(
        &'a self,
        database: &'a str,
        schema: &'a str,
        relations: &'a [Arc<dyn BaseRelation>],
        _options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        // `__TABLES__` is dataset-scoped: FROM `project`.`dataset`.__TABLES__
        // Using the two-part form `project.__TABLES__` is wrong — BigQuery
        // treats it as `current_project.project.__TABLES__` (dataset named
        // "project"), which 404s. Both parts need backtick quoting because
        // project IDs often contain hyphens.
        //
        // `database` and `schema` are raw (unquoted) identifiers from
        // `RelationPath`. Quoting is only applied at render time via
        // `quote_policy`/`quote_part`, so these values are never pre-quoted
        // and the unconditional backticks here cannot double-quote them.
        let sql = build_schema_freshness_query(database, schema);
        let relations = relations.to_vec();
        let adapter = self.adapter.clone();
        let factory = Box::new(AdapterConnectionFactory::new(adapter.engine().clone()));
        type Acc = BTreeMap<String, MetadataFreshness>;

        let token_clone = token.clone();
        let map_f = move |conn: &mut dyn Connection, _: &()| -> AdapterResult<Arc<RecordBatch>> {
            let ctx = QueryCtx::default().with_desc("Extracting freshness from information schema");
            let (_, agate) = adapter.query(&ctx, &mut *conn, &sql, None, token_clone.clone())?;
            Ok(agate.original_record_batch())
        };

        let reduce_f = move |acc: &mut Acc, _: (), batch_res: AdapterResult<Arc<RecordBatch>>| {
            let batch = match batch_res {
                Ok(b) => b,
                Err(e) if is_bigquery_not_found_error(&e) => return Ok(()),
                Err(e) => return Err(Cancellable::Error(e)),
            };
            let schemas = batch.column_values::<StringArray>("table_schema")?;
            let tables = batch.column_values::<StringArray>("table_name")?;
            let timestamps = batch.column_values::<TimestampMicrosecondArray>("last_altered")?;
            let is_views = batch.column_values::<BooleanArray>("is_view")?;
            for i in 0..batch.num_rows() {
                if timestamps.is_null(i) {
                    continue;
                }
                for fqn in find_matching_relation(schemas.value(i), tables.value(i), &relations)? {
                    acc.insert(
                        fqn,
                        MetadataFreshness::from_micros(timestamps.value(i), is_views.value(i))?,
                    );
                }
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(vec![()]), token)
    }
}

/// BigQuery reports a miss through two different APIs, and bigquery-adbc
/// formats each from whichever fields its error type carries — so the two
/// share no common token:
///
/// - **Metadata lookup** — a REST `Tables.Get` that returns HTTP 404. Has a
///   status code, no reason code (for example, `get_table_schema` -> Table.Metadata):
///
///   ```text
///   [bq] Could not get metadata for table `p`.`d`.`t`: 404 Not Found: Not found: Table p:d.t
///   ```
///
/// - **Query job** — the HTTP calls all succeed; the failure arrives inside
///   the job payload. Has a reason code, no status code (`execute` arbitrary SQLs):
///
///   ```text
///   [bq] Could not complete job: notFound: Not found: Dataset p:d was not found in location US ()
///   ```
///
/// See `errToAdbcErr`, which dispatches on the Go error type:
/// <https://github.com/dbt-labs/bigquery-adbc/blob/449ef311c5f2b82d586c97cf36d7c00dc8610851/go/util.go#L153>
///
/// arrow-adbc stringifies the raw SDK error instead, so both of its variants
/// read `googleapi: Error 404: Not found: …`.
///
/// TODO: match on the ADBC status instead — bigquery-adbc already reports
/// `StatusNotFound` for both — once the driver migration is complete.
pub fn is_bigquery_not_found_error(e: &AdapterError) -> bool {
    if e.kind() == AdapterErrorKind::NotFound {
        return true;
    }
    let msg = e.message();
    // arrow-adbc (both sources)
    msg.contains("Error 404: Not found:")
        // bigquery-adbc, metadata lookup
        || msg.contains("404 Not Found:")
        // bigquery-adbc, query job
        || msg.contains("notFound:")
}

/// Fallback when object is not found in `INFORMATION_SCHEMA.TABLES`
#[allow(clippy::too_many_arguments)]
pub fn get_relation_routine_fallback(
    adapter: &AdapterImpl,
    state: &State,
    conn: &'_ mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    let query_database = if adapter.quoting().database {
        adapter.quote(database)
    } else {
        database.to_string()
    };
    let query_schema = if adapter.quoting().schema {
        adapter.quote(schema)
    } else {
        schema.to_string()
    };
    let query_identifier = if adapter.quoting().identifier {
        identifier.to_string()
    } else {
        identifier.to_lowercase()
    };

    let escaped_identifier =
        dbt_adapter_sql::ident::escape_string_literal(&query_identifier, AdapterType::Bigquery);
    let routines_sql = format!(
        "SELECT routine_catalog AS table_catalog,
                    routine_schema AS table_schema,
                    routine_name AS table_name,
                    routine_type AS table_type
                FROM {query_database}.{query_schema}.INFORMATION_SCHEMA.ROUTINES
                 WHERE routine_name = '{escaped_identifier}'
                    AND routine_type != 'PROCEDURE';"
    );

    let ctx = query_ctx_from_state(state)?.with_desc("get_relation routines fallback");
    let result = adapter
        .engine()
        .execute(Some(state), conn, &ctx, &routines_sql, token.clone());
    let batch = match result {
        Ok(batch) => batch,
        Err(err)
            if err.message().contains("Dataset") && err.message().contains("was not found") =>
        {
            return Ok(None);
        }
        Err(err) => return Err(err),
    };

    let Some(mut relation) =
        get_relation::relation_from_routines_batch(adapter, database, schema, identifier, &batch)?
    else {
        return Ok(None);
    };

    // BigQuery-only metadata not covered by the generic fallback.
    let location = adapter.get_dataset_location(state, conn, &relation, token)?;
    relation.location = location;
    Ok(Some(Box::new(relation)))
}

/// BigQuery surfaces access-control failures as googleapi HTTP 403 errors.
/// This covers both plain IAM denials (e.g. the executing identity lacks
/// `bigquery.datasets.create`) and VPC Service Controls policy violations.
fn is_bigquery_permission_error(e: &AdapterError) -> bool {
    let msg = e.message();
    // Match access-control reasons only, not HTTP status: BigQuery also returns
    // 403 for quota failures (`quotaExceeded`, `rateLimitExceeded`,
    // `billingNotEnabled`), which must not be swallowed here.
    // https://cloud.google.com/bigquery/docs/error-messages
    msg.contains("accessDenied")
        || msg.contains("policyViolation")
        || msg.contains("VPC Service Controls")
        || msg.contains("PERMISSION_DENIED")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bq_rel(project: &str, dataset: &str, table: &str) -> Arc<dyn BaseRelation> {
        use crate::relation::Relation;
        use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;

        Arc::new(
            Relation::new(
                AdapterType::Bigquery,
                project.to_string(),
                dataset.to_string(),
                table.to_string(),
            )
            .with_quoting(DEFAULT_RESOLVED_QUOTING),
        )
    }

    #[test]
    fn cross_project_adbc_miss_uses_target_qualified_fallback() {
        let relations = verify_empty_adbc_listing(Vec::new(), true, || {
            Ok(vec![bq_rel(
                "target-project",
                "analytics",
                "existing_table",
            )])
        })
        .unwrap();

        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].database_as_str().unwrap(), "target-project");
        assert_eq!(relations[0].schema_as_str().unwrap(), "analytics");
        assert_eq!(relations[0].identifier_as_str().unwrap(), "existing_table");
    }

    #[test]
    fn same_project_empty_adbc_relation_listing_remains_authoritative() {
        let relations = verify_empty_adbc_listing(Vec::new(), false, || {
            panic!("same-project listing should not use the fallback")
        })
        .unwrap();

        assert!(relations.is_empty());
    }

    #[test]
    fn nonempty_adbc_relation_listing_remains_authoritative() {
        let relations = verify_empty_adbc_listing(
            vec![bq_rel("connection-project", "analytics", "adbc_table")],
            true,
            || {
                Ok(vec![bq_rel(
                    "connection-project",
                    "analytics",
                    "fallback_table",
                )])
            },
        )
        .unwrap();

        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].identifier_as_str().unwrap(), "adbc_table");
    }

    #[test]
    fn empty_adbc_relation_listing_returns_fallback_error() {
        let error = verify_empty_adbc_listing(Vec::new(), true, || {
            Err(AdapterError::new(
                AdapterErrorKind::SqlExecution,
                "target-qualified metadata is unavailable",
            ))
        })
        .expect_err("fallback error should be returned");

        assert_eq!(error.kind(), AdapterErrorKind::SqlExecution);
    }

    fn freshness_batch(rows: &[(&str, &str, Option<i64>, bool)]) -> RecordBatch {
        let schemas = rows.iter().map(|row| row.0).collect::<Vec<_>>();
        let tables = rows.iter().map(|row| row.1).collect::<Vec<_>>();
        let timestamps = rows.iter().map(|row| row.2).collect::<Vec<_>>();
        let is_views = rows.iter().map(|row| row.3).collect::<Vec<_>>();

        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("table_schema", DataType::Utf8, false),
                Field::new("table_name", DataType::Utf8, false),
                Field::new(
                    "last_altered",
                    DataType::Timestamp(TimeUnit::Microsecond, None),
                    true,
                ),
                Field::new("is_view", DataType::Boolean, false),
            ])),
            vec![
                Arc::new(StringArray::from(schemas)),
                Arc::new(StringArray::from(tables)),
                Arc::new(TimestampMicrosecondArray::from(timestamps)),
                Arc::new(BooleanArray::from(is_views)),
            ],
        )
        .unwrap()
    }

    #[test]
    fn test_generate_system_table_fqn_always_dataset_only() {
        let dataset_only_view = "PARTITIONS";
        assert_eq!(
            generate_system_table_fqn("`my-project`", dataset_only_view, None),
            "`my-project`.INFORMATION_SCHEMA.PARTITIONS"
        );
        assert_eq!(
            generate_system_table_fqn("`my-project`", dataset_only_view, Some("eu")),
            "`my-project`.INFORMATION_SCHEMA.PARTITIONS"
        );
    }

    #[test]
    fn test_generate_system_table_fqn_dataset_or_region() {
        // FIXME: sometimes the actual dataset reaches this method as if it were a part of
        // the project due to our upstream relation parsing.
        //
        // See: https://github.com/dbt-labs/fs/issues/4917

        let dataset_or_region_view = "TABLES";

        assert_eq!(
            generate_system_table_fqn("`my_dataset`", dataset_or_region_view, None),
            "`my_dataset`.INFORMATION_SCHEMA.TABLES"
        );
        // prefer user's region settings if specified
        assert_eq!(
            generate_system_table_fqn("`my_dataset`", dataset_or_region_view, Some("eu")),
            "`region-eu`.INFORMATION_SCHEMA.TABLES"
        );
    }

    #[test]
    fn test_generate_system_table_fqn_region_only() {
        // FIXME: sometimes the actual dataset reaches this method as if it were a part of
        // the project due to our upstream relation parsing.
        //
        // See: https://github.com/dbt-labs/fs/issues/4917

        let region_only_view = "JOBS";

        // use US as the default region if the user hasn't specified one
        assert_eq!(
            generate_system_table_fqn("`my_dataset`", region_only_view, None),
            "`region-us`.INFORMATION_SCHEMA.JOBS"
        );
        // prefer user's region settings if specified
        assert_eq!(
            generate_system_table_fqn("`my_dataset`", region_only_view, Some("eu")),
            "`region-eu`.INFORMATION_SCHEMA.JOBS"
        );
    }

    #[test]
    fn test_format_top_level_columns_data_types() {
        // Test case 1: Simple primitive types
        {
            let mut nested = NestedColumnDataTypes::default();
            nested.insert("id", Some("integer"), None);
            nested.insert("name", Some("string"), None);

            let result = nested.format_top_level_columns_data_types();
            assert_eq!(result.get("id").unwrap(), "integer");
            assert_eq!(result.get("name").unwrap(), "string");
        }

        // Test case 2: Nested struct
        {
            let mut nested = NestedColumnDataTypes::default();
            nested.insert("user.id", Some("integer"), None);
            nested.insert("user.name", Some("string"), None);

            let result = nested.format_top_level_columns_data_types();
            assert_eq!(
                result.get("user").unwrap(),
                "struct<id integer, name string>"
            );
        }

        // Test case 3: Array of structs
        {
            let mut nested = NestedColumnDataTypes::default();
            nested.insert("addresses", Some("array"), None);
            nested.insert("addresses.street", Some("string"), None);
            nested.insert("addresses.city", Some("string"), None);

            let result = nested.format_top_level_columns_data_types();
            assert_eq!(
                result.get("addresses").unwrap(),
                "array<struct<street string, city string>>"
            );
        }

        // Test case 4: Mixed types with deep nesting
        {
            let mut nested = NestedColumnDataTypes::default();
            nested.insert("id", Some("integer"), None);
            nested.insert("user.name", Some("string"), None);
            nested.insert("user.contact.email", Some("string"), None);
            nested.insert("user.contact.phone", Some("string"), None);

            let result = nested.format_top_level_columns_data_types();
            assert_eq!(result.get("id").unwrap(), "integer");
            assert_eq!(
                result.get("user").unwrap(),
                "struct<name string, contact struct<email string, phone string>>"
            );
        }

        // Test case 5: Empty struct (no data type)
        {
            let mut nested = NestedColumnDataTypes::default();
            nested.insert("empty_struct", None, None);
            nested.insert("empty_struct.field1", Some("string"), None);
            nested.insert("empty_struct.untyped", None, Some("not null"));

            let result = nested.format_top_level_columns_data_types();
            assert_eq!(
                result.get("empty_struct").unwrap(),
                "struct<field1 string, untyped>"
            );
        }

        // Test case 6: Struct marked as primitive but has children
        {
            let mut nested = NestedColumnDataTypes::default();
            nested.insert("metadata", Some("json"), None);
            nested.insert("metadata.key1", Some("string"), None);
            nested.insert("metadata.key2", Some("integer"), None);

            let result = nested.format_top_level_columns_data_types();
            assert_eq!(
                result.get("metadata").unwrap(),
                "struct<key1 string, key2 integer>"
            );
        }
    }

    #[test]
    fn test_format_top_level_columns_data_types_preserves_type_strings() {
        // Test case 7: Type strings are preserved verbatim
        {
            let mut nested = NestedColumnDataTypes::default();
            nested.insert("float_col", Some("FLOAT"), None);
            nested.insert("integer_col", Some("INTEGER"), None);
            nested.insert("text_col", Some("TEXT"), None);
            nested.insert("string_col", Some("STRING"), None);
            nested.insert("int64_col", Some("INT64"), None);
            nested.insert("numeric_col", Some("NUMERIC"), None);

            let result = nested.format_top_level_columns_data_types();
            assert_eq!(result.get("float_col").unwrap(), "FLOAT");
            assert_eq!(result.get("integer_col").unwrap(), "INTEGER");
            assert_eq!(result.get("text_col").unwrap(), "TEXT");
            assert_eq!(result.get("string_col").unwrap(), "STRING");
            assert_eq!(result.get("int64_col").unwrap(), "INT64");
            assert_eq!(result.get("numeric_col").unwrap(), "NUMERIC");
        }

        // Test case 8: Nested struct leaves preserve provided type strings
        {
            let mut nested = NestedColumnDataTypes::default();
            nested.insert("s.x", Some("FLOAT"), None);

            let result = nested.format_top_level_columns_data_types();
            assert_eq!(result.get("s").unwrap(), "struct<x FLOAT>");
        }
    }

    #[test]
    fn build_views_query_renders_basic_select() {
        let sql = build_views_query(
            "my-project",
            "analytics",
            &["users".to_string(), "orders".to_string()],
        );
        assert!(
            sql.contains("FROM `my-project`.`analytics`.INFORMATION_SCHEMA.VIEWS"),
            "got: {sql}"
        );
        assert!(
            sql.contains("table_name IN ('users', 'orders')"),
            "got: {sql}"
        );
        assert!(sql.contains("table_catalog"));
        assert!(sql.contains("table_schema"));
        assert!(sql.contains("view_definition"));
    }

    #[test]
    fn build_views_query_quotes_hyphenated_project() {
        let sql = build_views_query("my-project-123", "ds", &["t".to_string()]);
        assert!(sql.contains("`my-project-123`.`ds`"), "got: {sql}");
    }

    #[test]
    fn build_views_query_escapes_single_quotes_in_identifiers() {
        let sql = build_views_query("p", "d", &["weird'name".to_string()]);
        assert!(sql.contains("'weird''name'"), "got: {sql}");
    }

    /// https://github.com/dbt-labs/dbt-fusion/issues/1450:
    /// project IDs containing a `.` (domain-style IDs) used to abort source
    /// freshness with `Invalid BigQuery FQN` because the prior implementation
    /// split `semantic_fqn()` on `.`.
    #[test]
    fn build_relation_clauses_bigquery_handles_dotted_project_id() {
        let rel = bq_rel("mycompany.io", "analytics", "orders");

        let (where_by_db, rels_by_db) =
            build_relation_clauses_bigquery(std::slice::from_ref(&rel)).unwrap();

        let db_key = "`mycompany.io`.analytics";
        assert!(
            where_by_db.contains_key(db_key),
            "got keys: {:?}",
            where_by_db.keys().collect::<Vec<_>>()
        );
        assert_eq!(where_by_db[db_key], vec!["table_id = 'orders'"]);
        assert_eq!(rels_by_db[db_key].len(), 1);
    }

    #[test]
    fn tables_freshness_query_nulls_external_last_altered() {
        let sql =
            build_tables_freshness_query("`my-project`.analytics", &["table_id = 'orders'".into()]);

        assert!(sql.contains("INFORMATION_SCHEMA.TABLES"), "got: {sql}");
        assert!(sql.contains("table_type = 'EXTERNAL'"), "got: {sql}");
        assert!(sql.contains("WHERE table_id = 'orders'"), "got: {sql}");
        assert!(
            sql.contains("NULL") && sql.contains("TIMESTAMP_MILLIS(legacy.last_modified_time)"),
            "got: {sql}"
        );
    }

    #[test]
    fn schema_freshness_query_nulls_external_last_altered() {
        let sql = build_schema_freshness_query("my-project", "analytics");

        assert!(
            sql.contains("`my-project`.`analytics`.INFORMATION_SCHEMA.TABLES"),
            "got: {sql}"
        );
        assert!(sql.contains("table_type = 'EXTERNAL'"), "got: {sql}");
        assert!(
            sql.contains("NULL") && sql.contains("TIMESTAMP_MILLIS(legacy.last_modified_time)"),
            "got: {sql}"
        );
    }

    #[test]
    fn accumulate_tables_freshness_skips_null_last_altered() {
        let normal = bq_rel("my-project", "analytics", "orders");
        let external = bq_rel("my-project", "analytics", "sheet_orders");
        let database = "`my-project`.analytics";
        let relations_by_database = BTreeMap::from([(
            database.to_string(),
            vec![Arc::clone(&normal), Arc::clone(&external)],
        )]);
        let batch = freshness_batch(&[
            ("analytics", "orders", Some(1_700_000_000_000_000), false),
            ("analytics", "sheet_orders", None, false),
        ]);

        let mut acc = BTreeMap::new();
        accumulate_tables_freshness_from_batch(&mut acc, &batch, database, &relations_by_database)
            .unwrap();

        assert!(acc.contains_key(&normal.semantic_fqn()));
        assert!(!acc.contains_key(&external.semantic_fqn()));
    }

    /// IAM and VPC Service Controls denials are permission errors; not-found
    /// and quota errors are not.
    #[test]
    fn test_is_bigquery_permission_error() {
        let vpc_sc = AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "[BigQuery] googleapi: Error 403: VPC Service Controls: Request is \
             prohibited by organization's policy. policyViolation",
        );
        assert!(is_bigquery_permission_error(&vpc_sc));

        let iam_denied = AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "googleapi: Error 403: Access Denied: Permission \
             bigquery.datasets.create denied, accessDenied",
        );
        assert!(is_bigquery_permission_error(&iam_denied));

        // Not a permission error.
        let not_found = AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "googleapi: Error 404: Not found: Dataset foo:bar",
        );
        assert!(!is_bigquery_permission_error(&not_found));

        // 403 but not a permission error.
        let quota = AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "googleapi: Error 403: Quota exceeded: Your project exceeded quota \
             for dataset operations, quotaExceeded",
        );
        assert!(!is_bigquery_permission_error(&quota));
    }

    #[test]
    fn test_is_bigquery_not_found_error() {
        // arrow-adbc spelling.
        let legacy = AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "googleapi: Error 404: Not found: Table proj:dataset.tbl, notFound",
        );
        assert!(is_bigquery_not_found_error(&legacy));

        let typed = AdapterError::new(AdapterErrorKind::NotFound, "table missing");
        assert!(is_bigquery_not_found_error(&typed));

        // arrow-adbc, query job.
        let legacy_job = AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "googleapi: Error 404: Not found: Dataset proj:dataset was not found \
             in location US, notFound",
        );
        assert!(is_bigquery_not_found_error(&legacy_job));

        // bigquery-adbc metadata lookup: "<code> <StatusText>", not "Error <code>".
        let foundry = AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "[bq] Could not get metadata for table `proj`.`dataset`.`tbl`: \
             404 Not Found: Not found: Table proj:dataset.tbl",
        );
        assert!(is_bigquery_not_found_error(&foundry));

        // bigquery-adbc query job: no HTTP code at all, only the reason.
        let foundry_job = AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "[bq] Could not complete job: notFound: Not found: Dataset \
             proj:dataset was not found in location US ()",
        );
        assert!(is_bigquery_not_found_error(&foundry_job));

        // Other failures must still propagate.
        let denied = AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "googleapi: Error 403: Access Denied, accessDenied",
        );
        assert!(!is_bigquery_not_found_error(&denied));

        let invalid = AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "[bq] Could not complete job: invalidQuery: Syntax error (query)",
        );
        assert!(!is_bigquery_not_found_error(&invalid));
    }

    fn routine_batch(routine_type: &str) -> RecordBatch {
        let schema = Schema::new(vec![
            Field::new("table_catalog", DataType::Utf8, false),
            Field::new("table_schema", DataType::Utf8, false),
            Field::new("table_name", DataType::Utf8, false),
            Field::new("table_type", DataType::Utf8, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(StringArray::from(vec!["my-proj"])),
                Arc::new(StringArray::from(vec!["my_schema"])),
                Arc::new(StringArray::from(vec!["add_one"])),
                Arc::new(StringArray::from(vec![routine_type])),
            ],
        )
        .unwrap()
    }

    #[test]
    fn routine_batch_maps_function_type_to_relation_type_function() {
        let batch = routine_batch("FUNCTION");
        assert!(batch.num_rows() > 0);

        let column = batch.column_by_name("table_type").unwrap();
        let string_array = column.as_any().downcast_ref::<StringArray>().unwrap();
        let relation_type_name = string_array.value(0).to_uppercase();
        let relation_type =
            RelationType::from_adapter_type(AdapterType::Bigquery, &relation_type_name);

        assert_eq!(relation_type, RelationType::Function);
    }

    #[test]
    fn empty_routine_batch_signals_not_found() {
        let schema = Schema::new(vec![Field::new("table_type", DataType::Utf8, false)]);
        let empty_batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![Arc::new(StringArray::from(Vec::<&str>::new()))],
        )
        .unwrap();
        assert_eq!(empty_batch.num_rows(), 0);
    }
}
