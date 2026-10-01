use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{dbt_utils::get_dbt_schema_version, state::ResolverState};

fn default_dbt_version() -> String {
    "1.10.0a1".to_string()
}

// TODO: a lot of these are nullable, need to confirm against schema

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogMetadata {
    pub dbt_schema_version: String,
    #[serde(default = "default_dbt_version")]
    pub dbt_version: String,
    pub generated_at: DateTime<Utc>,
    pub invocation_id: Option<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogTable {
    pub metadata: TableMetadata,
    pub columns: IndexMap<String, ColumnMetadata>,
    pub stats: BTreeMap<String, CatalogNodeStats>,
    pub unique_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TableMetadata {
    #[serde(rename = "type")]
    pub materialization_type: String,
    pub schema: String,
    pub name: String,
    pub database: Option<String>,
    pub comment: Option<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogNodeStats {
    pub id: String,
    pub label: String,
    pub value: Value,
    pub include: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ColumnMetadata {
    #[serde(rename = "type")]
    pub data_type: String,
    pub index: i128,
    pub name: String,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DbtCatalog {
    pub metadata: CatalogMetadata,
    pub nodes: BTreeMap<String, CatalogTable>,
    pub sources: BTreeMap<String, CatalogTable>,
    pub errors: Option<Vec<String>>,
}

fn columns_sorted_by_index(
    columns: &BTreeMap<String, ColumnMetadata>,
) -> IndexMap<String, ColumnMetadata> {
    let mut entries: Vec<_> = columns.iter().collect();
    entries.sort_by_key(|(_, col)| col.index);
    entries
        .into_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

// TODO: dedupe code below
pub fn build_catalog(
    invocation_id: &str,
    resolver_state: &ResolverState,
    node_stats_and_stuff: BTreeMap<String, CatalogTable>,
    node_columns: BTreeMap<String, BTreeMap<String, ColumnMetadata>>,
) -> DbtCatalog {
    DbtCatalog {
        metadata: CatalogMetadata {
            dbt_schema_version: get_dbt_schema_version("catalog", 1),
            dbt_version: env!("CARGO_PKG_VERSION").to_string(),
            generated_at: Utc::now(),
            invocation_id: Some(invocation_id.to_string()),
            env: dbt_common::constants::collect_dbt_custom_envs(),
        },
        nodes: resolver_state
            .nodes
            .models
            .iter()
            .filter_map(|(id, node)| {
                let fully_qualified_name = format!(
                    "{}.{}.{}",
                    node.__base_attr__.database.clone(),
                    node.__base_attr__.schema.clone(),
                    node.__base_attr__.alias.clone()
                )
                .to_lowercase();
                match node_stats_and_stuff.get(&fully_qualified_name) {
                    Some(node) => {
                        let mut result = node.to_owned();
                        result.unique_id = Some(id.clone());
                        result.columns = columns_sorted_by_index(
                            node_columns
                                .get(&fully_qualified_name)
                                .unwrap_or(&BTreeMap::new()),
                        );
                        Some((id.clone(), result))
                    }
                    None => None,
                }
            })
            .chain(
                resolver_state
                    .nodes
                    .snapshots
                    .iter()
                    .filter_map(|(id, node)| {
                        let fully_qualified_name = format!(
                            "{}.{}.{}",
                            node.__base_attr__.database.clone(),
                            node.__base_attr__.schema.clone(),
                            node.__base_attr__.alias.clone()
                        )
                        .to_lowercase();
                        match node_stats_and_stuff.get(&fully_qualified_name) {
                            Some(node) => {
                                let mut result = node.to_owned();
                                result.unique_id = Some(id.clone());
                                result.columns = columns_sorted_by_index(
                                    node_columns
                                        .get(&fully_qualified_name)
                                        .unwrap_or(&BTreeMap::new()),
                                );
                                Some((id.clone(), result))
                            }
                            None => None,
                        }
                    }),
            )
            .chain(resolver_state.nodes.seeds.iter().filter_map(|(id, node)| {
                let fully_qualified_name = format!(
                    "{}.{}.{}",
                    node.__base_attr__.database.clone(),
                    node.__base_attr__.schema.clone(),
                    node.__base_attr__.alias.clone()
                )
                .to_lowercase();
                match node_stats_and_stuff.get(&fully_qualified_name) {
                    Some(node) => {
                        let mut result = node.to_owned();
                        result.unique_id = Some(id.clone());
                        result.columns = columns_sorted_by_index(
                            node_columns
                                .get(&fully_qualified_name)
                                .unwrap_or(&BTreeMap::new()),
                        );
                        Some((id.clone(), result))
                    }
                    None => None,
                }
            }))
            .chain(resolver_state.nodes.tests.iter().filter_map(|(id, node)| {
                let fully_qualified_name = format!(
                    "{}.{}.{}",
                    node.__base_attr__.database.clone(),
                    node.__base_attr__.schema.clone(),
                    node.__base_attr__.alias.clone()
                )
                .to_lowercase();
                match node_stats_and_stuff.get(&fully_qualified_name) {
                    Some(node) => {
                        let mut result = node.to_owned();
                        result.unique_id = Some(id.clone());
                        result.columns = columns_sorted_by_index(
                            node_columns
                                .get(&fully_qualified_name)
                                .unwrap_or(&BTreeMap::new()),
                        );
                        Some((id.clone(), result))
                    }
                    None => None,
                }
            }))
            .collect(),
        sources: resolver_state
            .nodes
            .sources
            .iter()
            .filter_map(|(id, source)| {
                let fully_qualified_name = format!(
                    "{}.{}.{}",
                    source.__base_attr__.database.clone(),
                    source.__base_attr__.schema.clone(),
                    source.__base_attr__.alias.clone()
                )
                .to_lowercase();
                match node_stats_and_stuff.get(&fully_qualified_name) {
                    Some(node) => {
                        let mut result = node.to_owned();
                        result.unique_id = Some(id.clone());
                        result.columns = columns_sorted_by_index(
                            node_columns
                                .get(&fully_qualified_name)
                                .unwrap_or(&BTreeMap::new()),
                        );
                        Some((id.clone(), result))
                    }
                    None => None,
                }
            })
            .collect(),
        errors: None, // TODO: look into errors and what this should look like
    }
}
