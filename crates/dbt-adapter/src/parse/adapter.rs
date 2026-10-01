use crate::AdapterEngine;
use crate::cast_util::downcast_value_to_dyn_base_relation;
use crate::relation::{RelationObject, do_create_relation};
use crate::value::empty_mutable_vec_value;

use dashmap::{DashMap, DashSet};
use dbt_adapter_core::AdapterType;
use dbt_common::FsError;
use dbt_schemas::schemas::dbt_catalogs_deprecated::DbtCatalogs;
use dbt_schemas::schemas::relations::base::{BaseRelation, RelationPattern};
use minijinja::constants::TARGET_UNIQUE_ID;
use minijinja::{State, Value};
use serde::Deserialize;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

type RelationsToFetch = (
    Result<BTreeMap<String, Vec<Arc<dyn BaseRelation>>>, FsError>,
    Result<BTreeMap<String, Vec<Arc<dyn BaseRelation>>>, FsError>,
    BTreeMap<String, Vec<RelationPattern>>,
);

#[derive(Clone)]
pub struct ParseAdapterState {
    pub adapter_type: AdapterType,
    /// The engine for the parse phase
    ///
    /// Not actually used to run SQL queries during parse, but needed since
    /// this object carries useful dependencies.
    pub engine: Arc<dyn AdapterEngine>,
    /// The call_get_relation method calls found during parse
    pub call_get_relation: DashMap<String, Vec<Value>>,
    /// The call_get_columns_in_relation method calls found during parse
    call_get_columns_in_relation: DashMap<String, Vec<Value>>,
    /// A patterned relation may turn to many dangling sources
    patterned_dangling_sources: DashMap<String, Vec<RelationPattern>>,
    /// A list of unsafe nodes detected during parse (unsafe nodes are nodes that have introspection qualities that make them non-deterministic / stateful)
    pub unsafe_nodes: DashSet<String>,
    /// SQLs that are found passed in to adapter.execute in the hidden Parse phase
    pub execute_sqls: DashSet<String>,
    /// catalogs.yml stored when found and loaded
    pub catalogs: Option<Arc<DbtCatalogs>>,
}

impl ParseAdapterState {
    pub fn new(
        adapter_type: AdapterType,
        engine: Arc<dyn AdapterEngine>,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> Self {
        ParseAdapterState {
            adapter_type,
            engine,
            call_get_relation: DashMap::new(),
            call_get_columns_in_relation: DashMap::new(),
            patterned_dangling_sources: DashMap::new(),
            unsafe_nodes: DashSet::new(),
            execute_sqls: DashSet::new(),
            catalogs,
        }
    }

    pub fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParseAdapter")
            .field("adapter_type", &self.adapter_type)
            .field("call_get_relation", &self.call_get_relation)
            .field(
                "call_get_columns_in_relation",
                &self.call_get_columns_in_relation,
            )
            .field(
                "patterned_dangling_sources",
                &self.patterned_dangling_sources,
            )
            .field("unsafe_nodes", &self.unsafe_nodes)
            .field("execute_sqls", &self.execute_sqls)
            .field("quoting", &self.engine.quoting())
            .finish()
    }

    pub fn record_get_relation_call(
        &self,
        state: &State,
        database: &str,
        schema: &str,
        identifier: &str,
    ) -> Result<(), minijinja::Error> {
        let relation = RelationObject::new(Arc::from(do_create_relation(
            self.adapter_type,
            database.to_string(),
            schema.to_string(),
            Some(identifier.to_string()),
            None,
            self.engine.quoting(),
        )?))
        .into_value();

        if state.is_execute() {
            if let Some(unique_id) = state.lookup(TARGET_UNIQUE_ID, &[]) {
                self.call_get_relation
                    .entry(unique_id.to_string())
                    .or_default()
                    .push(relation);
            } else {
                println!("'TARGET_UNIQUE_ID' while get_relation is unset");
            }
        }
        Ok(())
    }

    pub fn record_get_columns_in_relation_call(
        &self,
        state: &State,
        relation: &dyn BaseRelation,
    ) -> Result<(), minijinja::Error> {
        if !relation.is_database_relation() {
            return Ok(());
        }
        if state.is_execute() {
            if let Some(unique_id) = state.lookup(TARGET_UNIQUE_ID, &[]) {
                let relation_value = RelationObject::new(relation.to_owned()).into_value();
                self.call_get_columns_in_relation
                    .entry(unique_id.to_string())
                    .or_default()
                    .push(relation_value);
            } else {
                println!("'TARGET_UNIQUE_ID' while get_columns_in_relation is unset");
            }
        }
        Ok(())
    }

    /// Returns a tuple of (dangling_sources, patterned_dangling_sources)
    /// dangling_sources is a vector of dangling source relations
    /// patterned_dangling_sources is a vector of patterned dangling source relations
    #[allow(clippy::type_complexity)]
    pub fn relations_to_fetch(&self) -> RelationsToFetch {
        let relations_to_fetch = self
            .call_get_relation
            .iter()
            .map(|v| {
                Ok((
                    v.key().to_owned(),
                    v.value()
                        .iter()
                        .map(|v| downcast_value_to_dyn_base_relation(v))
                        .collect::<Result<Vec<Arc<dyn BaseRelation>>, minijinja::Error>>()?,
                ))
            })
            .collect::<Result<BTreeMap<String, Vec<Arc<dyn BaseRelation>>>, minijinja::Error>>()
            .map_err(|e| FsError::from_jinja_err(e, "Failed to collect get_relation"));

        let relations_to_fetch_columns = self
            .call_get_columns_in_relation
            .iter()
            .map(|v| {
                Ok((
                    v.key().to_owned(),
                    v.value()
                        .iter()
                        .map(|v| downcast_value_to_dyn_base_relation(v))
                        .collect::<Result<Vec<Arc<dyn BaseRelation>>, minijinja::Error>>()?,
                ))
            })
            .collect::<Result<BTreeMap<String, Vec<Arc<dyn BaseRelation>>>, minijinja::Error>>()
            .map_err(|e| FsError::from_jinja_err(e, "Failed to collect get_columns_in_relation"));

        let patterned_dangling_sources: BTreeMap<String, Vec<RelationPattern>> = self
            .patterned_dangling_sources
            .iter()
            .map(|r| (r.key().to_owned(), r.value().to_owned()))
            .collect();
        (
            relations_to_fetch,
            relations_to_fetch_columns,
            patterned_dangling_sources,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_relations_by_pattern(
        &self,
        state: &State,
        schema_pattern: &str,
        table_pattern: &str,
        _exclude: Option<&str>,
        database: Option<&str>,
        _quote_table: Option<bool>,
        excluded_schemas: Option<Value>,
    ) -> Result<Value, minijinja::Error> {
        // Validate excluded_schemas if provided
        if let Some(ref schemas) = excluded_schemas {
            let _: Vec<String> = Vec::<String>::deserialize(schemas.clone()).map_err(|e| {
                minijinja::Error::new(minijinja::ErrorKind::SerdeDeserializeError, e.to_string())
            })?;
        }

        let target = state
            .lookup("target", &[])
            .expect("target is set in parse")
            .get_attr("database")
            .unwrap_or_default();
        let default_database = target.as_str().unwrap_or_default();
        let database = database.unwrap_or(default_database);

        let patterned_relation = RelationPattern::new(
            database.to_string(),
            schema_pattern.to_string(),
            table_pattern.to_string(),
        );

        if state.is_execute() {
            if let Some(unique_id) = state.lookup(TARGET_UNIQUE_ID, &[]) {
                self.patterned_dangling_sources
                    .entry(unique_id.to_string())
                    .or_default()
                    .push(patterned_relation);
            } else {
                println!("'TARGET_UNIQUE_ID' while get_relations_by_pattern is unset");
            }
        }

        // Seen methods like 'append' being used on the result in internaly-analytics
        Ok(empty_mutable_vec_value())
    }

    pub fn unsafe_nodes(&self) -> &DashSet<String> {
        &self.unsafe_nodes
    }
}
