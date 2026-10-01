//! Builds Arrow schemas from YAML column definitions for local sources.
//!
//! When a source has `schema_origin: local`, its schema is derived from
//! the column definitions in the YAML file rather than being fetched from
//! the warehouse.

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::Arc,
};

use arrow_schema::{Field, Schema};
use dbt_adapter::{AdapterType, relation::create_relation_from_node};
use dbt_adapter_sql::types::SqlType;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_dag::schedule::Schedule;
use dbt_schema_store::{
    CanonicalFqn, LocalSchemaEntry,
    store::{DataStore, SchemaStore},
};
use dbt_schemas::schemas::{DbtSource, InternalDbtNodeAttributes, Nodes, dbt_column::DbtColumnRef};
use dbt_telemetry::NodeType;

use crate::static_analysis_buckets::build_refresh_intervals;

/// Converts YAML column definitions to an Arrow Schema.
///
/// Each column must have a `data_type` specified (e.g., "VARCHAR(100)", "NUMBER(38,2)").
fn build_arrow_schema_from_columns(
    columns: &[DbtColumnRef],
    adapter_type: AdapterType,
    source_selector: &str,
) -> FsResult<Schema> {
    if columns.is_empty() {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "{} has schema_origin: local but no columns defined. \
             Local sources must have column definitions with data_type specified.",
            source_selector
        ));
    }

    let fields: Vec<Field> = columns
        .iter()
        .map(|col| {
            let data_type = col.data_type.as_ref().ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Column '{}' in {} has schema_origin: local but no data_type defined. \
                     All columns must have data_type specified for local sources.",
                    col.name,
                    source_selector
                )
            })?;

            let (sql_type, nullable) = SqlType::parse(adapter_type, data_type).map_err(|e| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Failed to parse data_type '{}' for column '{}' in {}: {}",
                    data_type,
                    col.name,
                    source_selector,
                    e
                )
            })?;

            // Handle column name casing based on adapter type
            let column_name = match adapter_type {
                // Snowflake uppercases unquoted identifiers
                AdapterType::Snowflake => col.name.to_uppercase(),
                // Other adapters preserve case or have different rules
                _ => col.name.clone(),
            };

            Ok(sql_type.to_field(adapter_type, column_name, nullable))
        })
        .collect::<FsResult<_>>()?;

    Ok(Schema::new(fields))
}

/// Builds an Arrow schema from a source's YAML column definitions.
///
/// # Errors
///
/// Returns an error if:
/// - Any column lacks a `data_type` definition
/// - Any `data_type` cannot be parsed as a valid SQL type
fn build_schema_from_source(
    source: &DbtSource,
    cfqn: CanonicalFqn,
    unique_id: String,
    adapter_type: AdapterType,
) -> FsResult<LocalSchemaEntry> {
    let columns = &source.__base_attr__.columns;
    let err_hint_source_selector = format!(
        "source:{}.{}",
        source.__source_attr__.source_name, source.__common_attr__.name
    );

    let schema = build_arrow_schema_from_columns(columns, adapter_type, &err_hint_source_selector)?;

    Ok(LocalSchemaEntry {
        cfqn,
        unique_id,
        schema: Arc::new(schema),
    })
}

/// Result of partitioning frontier nodes by schema origin.
///
/// - `0`: Remote frontier - sources/nodes with `schema_origin: remote` (fetched from warehouse)
/// - `1`: Local map - sources with `schema_origin: local` (cfqn -> unique_id mapping)
/// - `2`: Local schemas - Arrow schemas built from YAML column definitions
type PartitionedFrontier = (
    HashMap<CanonicalFqn, String>,
    HashMap<CanonicalFqn, String>,
    Vec<LocalSchemaEntry>,
);

/// Partitions frontier nodes by their schema origin.
fn partition_by_schema_origin(
    unique_ids: &BTreeSet<String>,
    nodes: &Nodes,
    adapter_type: AdapterType,
) -> FsResult<PartitionedFrontier> {
    use dbt_schemas::schemas::common::SchemaOrigin;

    let mut remote_frontier = HashMap::new();
    let mut local = HashMap::new();
    let mut local_schemas = Vec::new();

    for unique_id in unique_ids {
        let node: &dyn InternalDbtNodeAttributes =
            nodes.get_node(unique_id).expect("fetched from unique_id");
        let relation = create_relation_from_node(adapter_type, node, None)?;
        let cfqn = relation.get_canonical_fqn()?;

        if node.schema_origin() == SchemaOrigin::Local {
            // For local sources, build the schema from YAML columns
            if let Some(source) = nodes.sources.get(unique_id) {
                local.insert(cfqn.clone(), unique_id.clone());
                let schema =
                    build_schema_from_source(source, cfqn, unique_id.clone(), adapter_type)?;
                local_schemas.push(schema);
            }
        } else {
            remote_frontier.insert(cfqn, unique_id.clone());
        }
    }

    Ok((remote_frontier, local, local_schemas))
}

pub fn init_schema_store(
    schedule: &Schedule<String>,
    nodes: &Nodes,
    cache_dir: &Path,
    adapter_type: AdapterType,
    extra_frontier_unique_ids: Option<&BTreeSet<String>>,
) -> FsResult<SchemaStore> {
    // Filter out unit tests and data tests from selected nodes.
    // Unit tests inherit the database/schema/alias from the model they test, which causes
    // CanonicalFqn collisions. Tests don't represent real tables/views and shouldn't be
    // in the schema store.
    let selected = schedule
        .selected_nodes
        .iter()
        .filter_map(|unique_id| {
            let node: &dyn InternalDbtNodeAttributes =
                nodes.get_node(unique_id).expect("fetched from unique_id");
            // Skip unit tests and data tests - they don't have their own schema
            if matches!(node.resource_type(), NodeType::UnitTest) {
                return None;
            }
            let relation = create_relation_from_node(adapter_type, node, None).ok()?;
            Some(Ok((relation.get_canonical_fqn().ok()?, unique_id.clone())))
        })
        .collect::<FsResult<HashMap<CanonicalFqn, String>>>()?;

    // Collect frontier unique IDs, filtering out unit tests
    let mut all_frontier_ids: BTreeSet<String> = schedule
        .frontier_nodes
        .iter()
        .filter_map(|unique_id| {
            let node = nodes.get_node(unique_id)?;
            // Skip unit tests and data tests - they don't have their own schema
            if matches!(node.resource_type(), NodeType::UnitTest) {
                return None;
            }
            Some(unique_id.clone())
        })
        .collect::<BTreeSet<String>>();

    // Merge in extra frontier IDs with deduplication and filtering
    if let Some(extra_unique_ids) = extra_frontier_unique_ids {
        for unique_id in extra_unique_ids {
            // Skip if already in frontier
            if all_frontier_ids.contains(unique_id) {
                continue;
            }
            // Skip if already selected -- Selected entries take precedence in
            // SchemaStore lookups (store.rs), so adding a redundant Frontier
            // entry only triggers an unnecessary remote schema fetch.
            if schedule.selected_nodes.contains(unique_id) {
                continue;
            }
            let Some(node) = nodes.get_node(unique_id) else {
                continue;
            };
            // Skip unit tests and data tests
            if matches!(node.resource_type(), NodeType::UnitTest) {
                continue;
            }
            all_frontier_ids.insert(unique_id.clone());
        }
    }

    // Partition frontier nodes by schema origin
    let (remote_frontier, local, local_schemas) =
        partition_by_schema_origin(&all_frontier_ids, nodes, adapter_type)?;

    // Build per-source refresh intervals
    let refresh_intervals = build_refresh_intervals(&all_frontier_ids, nodes);

    let store = SchemaStore::new(
        cache_dir.to_path_buf(),
        selected,
        remote_frontier,
        local,
        local_schemas,
        refresh_intervals,
    );

    Ok(store)
}

pub fn init_data_store(cache_dir: &Path) -> DataStore {
    DataStore::new(cache_dir.to_path_buf())
}
