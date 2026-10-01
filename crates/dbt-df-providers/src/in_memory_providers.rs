//! Simple in-memory catalog providers used in tests and fallback scenarios.

use datafusion_catalog::{CatalogProvider, CatalogProviderList, SchemaProvider, TableProvider};
use datafusion_common::error::DataFusionError;
use dbt_frontend_common::error::InternalResult;
use dbt_frontend_common::ident::Identifier;
use scc::HashMap as SccHashMap;
use std::any::Any;
use std::sync::Arc;

/// Minimal catalog list that stores everything in memory.
#[derive(Debug, Default)]
pub struct InMemoryCatalogProviderList {
    /// Collection of catalogs containing schemas and ultimately TableProviders
    catalog_cache: SccHashMap<Identifier, Arc<dyn CatalogProvider>>,
}

impl InMemoryCatalogProviderList {
    /// Looks up the catalog for `id`, creating it on demand.
    pub fn resolve_catalog(&self, id: &Identifier) -> InternalResult<Arc<dyn CatalogProvider>> {
        match self.catalog_cache.read_sync(id, |_, c| Arc::clone(c)) {
            Some(catalog) => Ok(catalog),
            None => {
                let catalog = Arc::new(InMemoryCatalogProvider::default());
                let _ = self.catalog_cache.upsert_sync(id.clone(), catalog.clone());
                Ok(catalog)
            }
        }
    }
}

impl CatalogProviderList for InMemoryCatalogProviderList {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn register_catalog(
        &self,
        _name: String,
        catalog: Arc<dyn CatalogProvider>,
    ) -> Option<Arc<dyn CatalogProvider>> {
        Some(catalog)
    }

    fn catalog_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        self.catalog_cache.iter_sync(|k, _| {
            names.push(k.to_string());
            true
        });
        names
    }

    fn catalog(&self, name: &str) -> Option<Arc<dyn CatalogProvider>> {
        self.resolve_catalog(&Identifier::new(name)).ok()
    }
}

/// Simple in-memory implementation of a catalog.
/// Catalog provider backed by a concurrent in-memory map.
#[derive(Debug, Default)]
pub struct InMemoryCatalogProvider {
    schema_cache: SccHashMap<Identifier, Arc<dyn SchemaProvider>>,
}

impl CatalogProvider for InMemoryCatalogProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        self.schema_cache.iter_sync(|k, _| {
            names.push(k.to_string());
            true
        });
        names
    }

    fn schema(&self, name: &str) -> Option<Arc<dyn SchemaProvider>> {
        match self
            .schema_cache
            .read_sync(&Identifier::new(name), |_, s| Arc::clone(s))
        {
            Some(schema) => Some(schema),
            None => {
                let schema = Arc::new(InMemorySchemaProvider::default());
                let _ = self
                    .schema_cache
                    .upsert_sync(Identifier::new(name), schema.clone());
                Some(schema)
            }
        }
    }
}

/// Simple in-memory implementation of a schema.
/// Schema provider backed by a concurrent in-memory map.
#[derive(Debug, Default)]
pub struct InMemorySchemaProvider {
    tables_cache: SccHashMap<Identifier, Arc<dyn TableProvider>>,
}

#[async_trait::async_trait]
impl SchemaProvider for InMemorySchemaProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Returns the registered table names without acquiring long-lived locks.
    fn table_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        self.tables_cache.iter_sync(|k, _| {
            names.push(k.to_string());
            true
        });
        names
    }

    async fn table(&self, name: &str) -> Result<Option<Arc<dyn TableProvider>>, DataFusionError> {
        Ok(self
            .tables_cache
            .read_async(&Identifier::new(name), |_, t| Arc::clone(t))
            .await)
    }

    /// Registers or replaces a table provider for the given name.
    fn register_table(
        &self,
        name: String,
        table: Arc<dyn TableProvider>,
    ) -> Result<Option<Arc<dyn TableProvider>>, DataFusionError> {
        let _ = self
            .tables_cache
            .upsert_sync(Identifier::new(name), table.clone());
        Ok(Some(table))
    }

    /// Removes and returns the table provider associated with `name`.
    fn deregister_table(
        &self,
        name: &str,
    ) -> Result<Option<Arc<dyn TableProvider>>, DataFusionError> {
        Ok(self
            .tables_cache
            .remove_sync(&Identifier::new(name))
            .map(|(_, table)| table))
    }

    /// Returns true if a table provider is registered for `name`.
    fn table_exist(&self, name: &str) -> bool {
        // First check the tables_cache
        self.tables_cache.contains_sync(&Identifier::new(name))
    }
}
