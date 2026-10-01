use crate::{
    CanonicalFqn, CanonicalIdentifier, DataStoreTrait, SchemaEntry, SchemaStoreResult,
    SchemaStoreTrait,
};
use arrow::array::RecordBatch;
use arrow_schema::SchemaRef;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::RwLock;

/// In-memory [`SchemaStoreTrait`] implementation for tests.
#[derive(Debug, Default)]
pub struct MockSchemaStore {
    // Index by CanonicalFqn
    schemas_by_cfqn: RwLock<HashMap<CanonicalFqn, SchemaEntry>>,
    // Index by unique_id (String)
    schemas_by_unique_id: RwLock<HashMap<String, SchemaEntry>>,
}

impl MockSchemaStore {
    /// Creates an empty mock schema store.
    pub fn new() -> Self {
        Self {
            schemas_by_cfqn: RwLock::new(HashMap::new()),
            schemas_by_unique_id: RwLock::new(HashMap::new()),
        }
    }

    /// Helper to register a schema keyed by dbt `unique_id`.
    pub fn register_schema_by_unique_id(
        &self,
        unique_id: String,
        original_schema: Option<SchemaRef>,
        sdf_schema: SchemaRef,
        overwrite: bool,
    ) -> SchemaStoreResult<SchemaEntry> {
        let mut map = self.schemas_by_unique_id.write().unwrap();
        if map.contains_key(&unique_id) && !overwrite {
            return Ok(map.get(&unique_id).cloned().unwrap());
        }
        let entry = SchemaEntry::from_sdf_arrow_schema(original_schema, sdf_schema);
        map.insert(unique_id, entry.clone());
        Ok(entry)
    }
}

#[async_trait::async_trait]
impl SchemaStoreTrait for MockSchemaStore {
    fn exists(&self, cfqn: &CanonicalFqn) -> bool {
        self.schemas_by_cfqn.read().unwrap().contains_key(cfqn)
    }

    async fn exists_async(&self, cfqn: &CanonicalFqn) -> bool {
        self.exists(cfqn)
    }

    fn exists_by_unique_id(&self, unique_id: &str) -> bool {
        self.schemas_by_unique_id
            .read()
            .unwrap()
            .contains_key(unique_id)
    }

    fn get_schema(&self, cfqn: &CanonicalFqn) -> Option<SchemaEntry> {
        self.schemas_by_cfqn.read().unwrap().get(cfqn).cloned()
    }

    async fn get_schema_async(&self, cfqn: &CanonicalFqn) -> Option<SchemaEntry> {
        self.get_schema(cfqn)
    }

    fn get_schema_by_unique_id(&self, unique_id: &str) -> Option<SchemaEntry> {
        self.schemas_by_unique_id
            .read()
            .unwrap()
            .get(unique_id)
            .cloned()
    }

    async fn get_schema_by_unique_id_async(&self, unique_id: &str) -> Option<SchemaEntry> {
        self.get_schema_by_unique_id(unique_id)
    }

    fn register_schema(
        &self,
        cfqn: &CanonicalFqn,
        original_schema: Option<SchemaRef>,
        sdf_schema: SchemaRef,
        overwrite: bool,
    ) -> SchemaStoreResult<SchemaEntry> {
        let mut map = self.schemas_by_cfqn.write().unwrap();
        if map.contains_key(cfqn) && !overwrite {
            return Ok(map.get(cfqn).cloned().unwrap());
        }
        let entry = SchemaEntry::from_sdf_arrow_schema(original_schema, sdf_schema);
        map.insert(cfqn.clone(), entry.clone());
        Ok(entry)
    }

    fn catalog_names(&self) -> Vec<CanonicalIdentifier> {
        let map = self.schemas_by_cfqn.read().unwrap();
        let mut catalogs = BTreeSet::new();
        for cfqn in map.keys() {
            catalogs.insert(cfqn.catalog().clone());
        }
        catalogs.into_iter().collect()
    }

    fn schema_names(&self, catalog: &CanonicalIdentifier) -> Vec<CanonicalIdentifier> {
        let map = self.schemas_by_cfqn.read().unwrap();
        let mut schemas = BTreeSet::new();
        for cfqn in map.keys() {
            if cfqn.catalog() == catalog {
                schemas.insert(cfqn.schema().clone());
            }
        }
        schemas.into_iter().collect()
    }

    fn table_names(
        &self,
        catalog: &CanonicalIdentifier,
        schema: &CanonicalIdentifier,
    ) -> Vec<CanonicalIdentifier> {
        let map = self.schemas_by_cfqn.read().unwrap();
        let mut tables = BTreeSet::new();
        for cfqn in map.keys() {
            if cfqn.catalog() == catalog && cfqn.schema() == schema {
                tables.insert(cfqn.table().clone());
            }
        }
        tables.into_iter().collect()
    }
}

#[derive(Debug, Default)]
pub struct MockDataStore {
    data_by_cfqn: RwLock<HashMap<CanonicalFqn, PathBuf>>,
}

impl MockDataStore {
    pub fn new() -> Self {
        Self {
            data_by_cfqn: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl DataStoreTrait for MockDataStore {
    fn persist_data(
        &self,
        _cfqn: &CanonicalFqn,
        _schema: SchemaRef,
        _batches: Vec<RecordBatch>,
    ) -> SchemaStoreResult<usize> {
        unimplemented!()
    }

    async fn persist_data_async(
        &self,
        _cfqn: &CanonicalFqn,
        _schema: SchemaRef,
        _stream: std::pin::Pin<
            Box<dyn futures::Stream<Item = SchemaStoreResult<RecordBatch>> + Send + 'static>,
        >,
    ) -> SchemaStoreResult<usize> {
        unimplemented!()
    }
    fn get_path_to_data(&self, cfqn: &CanonicalFqn) -> PathBuf {
        self.data_by_cfqn
            .read()
            .unwrap()
            .get(cfqn)
            .cloned()
            .unwrap()
    }
}
