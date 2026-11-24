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
use crate::schema_only_table::SchemaOnlyTableProvider;

/// Catalog list that lazily provisions entries using the schema store.
/// Makes the [`TableProvider`] for a table in the schema store. A function,
/// not a branch, so a caller that never builds the data-reading provider does
/// not link it.
type MakeTable =
    dyn Fn(CanonicalFqn, Arc<dyn SchemaStoreTrait>) -> Arc<dyn TableProvider> + Send + Sync;

#[derive(Clone)]
struct TableFactory(Arc<MakeTable>);

impl TableFactory {
    fn new(
        make: impl Fn(CanonicalFqn, Arc<dyn SchemaStoreTrait>) -> Arc<dyn TableProvider>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(make))
    }

    fn make(&self, fqn: CanonicalFqn, store: Arc<dyn SchemaStoreTrait>) -> Arc<dyn TableProvider> {
        (self.0)(fqn, store)
    }
}

impl std::fmt::Debug for TableFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TableFactory")
    }
}

fn delayed_table_factory(data_store: Arc<dyn DataStoreTrait>) -> TableFactory {
    TableFactory::new(move |fqn, store| {
        Arc::new(DelayedDataTableProvider::new(
            fqn,
            store,
            data_store.clone(),
        ))
    })
}

#[derive(Debug)]
pub struct SchemaStoreCatalogProviderList {
    /// Collection of catalogs containing schemas and ultimately TableProviders
    catalog_cache: SccHashMap<Identifier, Arc<dyn CatalogProvider>>,
    store: Arc<dyn SchemaStoreTrait>,
    make_table: TableFactory,
}

impl SchemaStoreCatalogProviderList {
    /// Creates a new catalog list backed by the given schema store.
    pub fn new(store: Arc<dyn SchemaStoreTrait>, data_store: Arc<dyn DataStoreTrait>) -> Self {
        Self {
            catalog_cache: SccHashMap::new(),
            store,
            make_table: delayed_table_factory(data_store),
        }
    }

    /// Creates a catalog list whose tables carry only their schema: scanning
    /// one fails. For callers that bind SQL but never execute it, this keeps
    /// the data-reading code (listing tables, parquet) out of the binary.
    pub fn schema_only(store: Arc<dyn SchemaStoreTrait>) -> Self {
        Self {
            catalog_cache: SccHashMap::new(),
            store,
            make_table: TableFactory::new(|fqn, store| {
                Arc::new(SchemaOnlyTableProvider::new(fqn, store))
            }),
        }
    }

    /// Resolves the catalog for `id`, materializing it if necessary.
    pub fn resolve_catalog(&self, id: &Identifier) -> InternalResult<Arc<dyn CatalogProvider>> {
        match self.catalog_cache.read_sync(id, |_, c| Arc::clone(c)) {
            Some(catalog) => Ok(catalog),
            None => {
                let catalog = Arc::new(SchemaStoreCatalogProvider::with_table_factory(
                    id.clone(),
                    self.store.clone(),
                    self.make_table.clone(),
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
    make_table: TableFactory,
}

impl SchemaStoreCatalogProvider {
    /// Creates a catalog provider for the given canonical catalog identifier.
    pub fn new(
        catalog: Identifier,
        cache: Arc<dyn SchemaStoreTrait>,
        data_store: Arc<dyn DataStoreTrait>,
    ) -> Self {
        Self::with_table_factory(catalog, cache, delayed_table_factory(data_store))
    }

    fn with_table_factory(
        catalog: Identifier,
        store: Arc<dyn SchemaStoreTrait>,
        make_table: TableFactory,
    ) -> Self {
        Self {
            catalog,
            schema_cache: SccHashMap::new(),
            store,
            make_table,
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
                let schema = Arc::new(SchemaStoreSchemaProvider::with_table_factory(
                    self.catalog.clone(),
                    Identifier::new(name),
                    self.store.clone(),
                    self.make_table.clone(),
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
    make_table: TableFactory,
}

impl SchemaStoreSchemaProvider {
    /// Creates a schema provider bound to a catalog and schema identifier.
    pub fn new(
        catalog: Identifier,
        schema: Identifier,
        store: Arc<dyn SchemaStoreTrait>,
        data_store: Arc<dyn DataStoreTrait>,
    ) -> Self {
        Self::with_table_factory(catalog, schema, store, delayed_table_factory(data_store))
    }

    fn with_table_factory(
        catalog: Identifier,
        schema: Identifier,
        store: Arc<dyn SchemaStoreTrait>,
        make_table: TableFactory,
    ) -> Self {
        Self {
            catalog,
            schema,
            store,
            make_table,
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
            let table_provider = self.make_table.make(canonical_fqn, self.store.clone());
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
