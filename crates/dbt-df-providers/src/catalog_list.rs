//! CatalogProvider implementations backed by the schema store.
//!
//! Unlike the default DataFusion catalog list, these providers guarantee that a
//! catalog/schema lookup always succeeds by delegating table discovery to the
//! schema store at the moment DataFusion asks for it.

use datafusion_catalog::{CatalogProvider, CatalogProviderList, SchemaProvider, TableProvider};
use datafusion_common::error::DataFusionError;
use dbt_frontend_common::error::InternalResult;
use dbt_frontend_common::ident::Identifier;
use dbt_schema_store::{CanonicalFqn, DataStoreTrait, SchemaStoreTrait};
use scc::HashMap as SccHashMap;
use std::any::Any;
use std::sync::Arc;

use crate::delayed_table::DelayedDataTableProvider;

/// Catalog list that lazily provisions entries using the schema store.
#[derive(Debug)]
pub struct SchemaStoreCatalogProviderList {
    /// Collection of catalogs containing schemas and ultimately TableProviders
    catalog_cache: SccHashMap<Identifier, Arc<dyn CatalogProvider>>,
    store: Arc<dyn SchemaStoreTrait>,
    data_store: Arc<dyn DataStoreTrait>,
}

impl SchemaStoreCatalogProviderList {
    /// Creates a new catalog list backed by the given schema store.
    pub fn new(store: Arc<dyn SchemaStoreTrait>, data_store: Arc<dyn DataStoreTrait>) -> Self {
        Self {
            catalog_cache: SccHashMap::new(),
            store,
            data_store,
        }
    }

    /// Resolves the catalog for `id`, materializing it if necessary.
    pub fn resolve_catalog(&self, id: &Identifier) -> InternalResult<Arc<dyn CatalogProvider>> {
        match self.catalog_cache.read_sync(id, |_, c| Arc::clone(c)) {
            Some(catalog) => Ok(catalog),
            None => {
                let catalog = Arc::new(SchemaStoreCatalogProvider::new(
                    id.clone(),
                    self.store.clone(),
                    self.data_store.clone(),
                ));
                let _ = self.catalog_cache.upsert_sync(id.clone(), catalog.clone());
                Ok(catalog)
            }
        }
    }
}

impl CatalogProviderList for SchemaStoreCatalogProviderList {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn register_catalog(
        &self,
        _name: String,
        _catalog: Arc<dyn CatalogProvider>,
    ) -> Option<Arc<dyn CatalogProvider>> {
        unimplemented!()
    }

    fn catalog_names(&self) -> Vec<String> {
        self.store
            .catalog_names()
            .into_iter()
            .map(|c| c.to_string())
            .collect()
    }

    fn catalog(&self, name: &str) -> Option<Arc<dyn CatalogProvider>> {
        self.resolve_catalog(&Identifier::new(name)).ok()
    }
}

/// Simple in-memory implementation of a catalog.
/// Catalog provider that proxies lookups to the schema store.
#[derive(Debug)]
pub struct SchemaStoreCatalogProvider {
    catalog: Identifier,
    schema_cache: SccHashMap<Identifier, Arc<dyn SchemaProvider>>,
    store: Arc<dyn SchemaStoreTrait>,
    data_store: Arc<dyn DataStoreTrait>,
}

impl SchemaStoreCatalogProvider {
    /// Creates a catalog provider for the given canonical catalog identifier.
    pub fn new(
        catalog: Identifier,
        cache: Arc<dyn SchemaStoreTrait>,
        data_store: Arc<dyn DataStoreTrait>,
    ) -> Self {
        Self {
            catalog,
            schema_cache: SccHashMap::new(),
            store: cache,
            data_store,
        }
    }
}

impl CatalogProvider for SchemaStoreCatalogProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema_names(&self) -> Vec<String> {
        self.store
            .schema_names(&self.catalog)
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn schema(&self, name: &str) -> Option<Arc<dyn SchemaProvider>> {
        match self
            .schema_cache
            .read_sync(&Identifier::new(name), |_, s| Arc::clone(s))
        {
            Some(schema) => Some(schema),
            None => {
                let schema = Arc::new(SchemaStoreSchemaProvider::new(
                    self.catalog.clone(),
                    Identifier::new(name),
                    self.store.clone(),
                    self.data_store.clone(),
                ));
                let _ = self
                    .schema_cache
                    .upsert_sync(Identifier::new(name), schema.clone());
                Some(schema)
            }
        }
    }
}

/// Simple in-memory implementation of a schema.
/// Schema provider that provisions [`DelayedDataTableProvider`] instances on-demand.
#[derive(Debug)]
pub struct SchemaStoreSchemaProvider {
    catalog: Identifier,
    schema: Identifier,
    store: Arc<dyn SchemaStoreTrait>,
    data_store: Arc<dyn DataStoreTrait>,
}

impl SchemaStoreSchemaProvider {
    /// Creates a schema provider bound to a catalog and schema identifier.
    pub fn new(
        catalog: Identifier,
        schema: Identifier,
        store: Arc<dyn SchemaStoreTrait>,
        data_store: Arc<dyn DataStoreTrait>,
    ) -> Self {
        Self {
            catalog,
            schema,
            store,
            data_store,
        }
    }
}

#[async_trait::async_trait]
impl SchemaProvider for SchemaStoreSchemaProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        self.store
            .table_names(&self.catalog, &self.schema)
            .into_iter()
            .map(|t| t.to_string())
            .collect()
    }

    async fn table(&self, name: &str) -> Result<Option<Arc<dyn TableProvider>>, DataFusionError> {
        let canonical_fqn = CanonicalFqn::new(&self.catalog, &self.schema, &Identifier::new(name));
        if self.store.exists(&canonical_fqn) {
            // Materialize a table provider that will consult the schema store when scanned.
            let table_provider = Arc::new(DelayedDataTableProvider::new(
                canonical_fqn,
                self.store.clone(),
                self.data_store.clone(),
            ));
            Ok(Some(table_provider))
        } else {
            Ok(None)
        }
    }

    /// Checks whether the canonical FQN exists in the schema store.
    fn table_exist(&self, name: &str) -> bool {
        let cfqn = CanonicalFqn::new(&self.catalog, &self.schema, &Identifier::new(name));
        self.store.exists(&cfqn)
    }
}
