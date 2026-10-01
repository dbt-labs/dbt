//! Storage APIs for canonicalized dbt schemas and intermediate table data.
//!
//! The schema store centralizes how dbt Fusion persists and retrieves Arrow
//! schemas for every node that may participate in a run.  It understands which
//! schemas must always originate from remote sources (frontier nodes, deferred
//! models, cross-project references) and which ones may be hydrated from the
//! local compilation results (analyzed models).  By baking those guarantees into
//! the storage layer, we avoid accidental cross-run state bleed and ensure that
//! commands see a consistent view of the project.
//!
//! Schemas and data are persisted using the following canonical layout:
//!
//! ```text
//! target/
//!   ├── private/metadata/compile/schemas/{N}.parquet   ← compile-time schemas (no TTL)
//!   ├── private/metadata/warehouse/schemas/{N}.parquet ← warehouse-fetched schemas (TTL)
//!   └── data/...
//! ```
//!
//! The store is responsible for building these paths, canonicalizing the fully
//! qualified identifiers, and serializing schemas (with the original warehouse
//! schema embedded as metadata when available).  Consumers interact with the
//! store exclusively through the [`SchemaStoreTrait`], which is implemented by
//! both production and mock backends.

pub mod mock_store;
pub mod parquet_cache;
pub mod store;

pub use store::read_cached_schema_from_parquet;

use std::path::PathBuf;

use arrow::array::RecordBatch;
use arrow_schema::{ArrowError, SchemaRef};
use dbt_ident::Ident;

/// A local schema entry containing the fully qualified name, unique ID, and Arrow schema.
///
/// Used to pass local source schemas (derived from YAML column definitions) into the
/// schema store for registration.
#[derive(Debug, Clone)]
pub struct LocalSchemaEntry {
    /// The canonical fully qualified name of the source
    pub cfqn: CanonicalFqn,
    /// The dbt unique_id of the source
    pub unique_id: String,
    /// The Arrow schema built from column definitions
    pub schema: SchemaRef,
}

/// Canonical identifier component (catalog, schema, or table) used within the
/// schema store.
///
/// All identifiers are normalized to the target dialect so that filesystem
/// lookups become deterministic regardless of the source system casing rules.
pub type CanonicalIdentifier = Ident<'static>;

/// Canonical fully qualified name (FQN) that uniquely identifies a table within
/// the schema store.
///
/// The canonical form uses the normalized catalog, schema, and table segments
/// produced by dbt's identifier handling.  This eliminates ambiguity between
/// different casing or quoting strategies when mapping warehouse objects onto
/// filesystem paths.
#[derive(Hash, Eq, PartialEq, Clone, Debug, Default)]
pub struct CanonicalFqn {
    catalog: CanonicalIdentifier,
    schema: CanonicalIdentifier,
    table: CanonicalIdentifier,
}

impl std::fmt::Display for CanonicalFqn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.catalog, self.schema, self.table)
    }
}

impl CanonicalFqn {
    /// Builds a canonical FQN from normalized identifier components.
    pub fn new(
        database: &CanonicalIdentifier,
        schema: &CanonicalIdentifier,
        table: &CanonicalIdentifier,
    ) -> Self {
        Self {
            catalog: database.clone(),
            schema: schema.clone(),
            table: table.clone(),
        }
    }
}

impl CanonicalFqn {
    /// Returns the canonical catalog identifier.
    pub fn catalog(&self) -> &CanonicalIdentifier {
        &self.catalog
    }

    /// Returns the canonical schema identifier.
    pub fn schema(&self) -> &CanonicalIdentifier {
        &self.schema
    }

    /// Returns the canonical table identifier.
    pub fn table(&self) -> &CanonicalIdentifier {
        &self.table
    }
}

/// Arrow schema payload stored by the schema store.
///
/// Every entry retains both the schema that dbt uses during compilation (the
/// "SDF" schema) and, when available, the original warehouse schema.  Retaining
/// the original schema allows consumers to inspect server-side metadata without
/// sacrificing the deterministic transforms required by static analysis.
#[derive(Debug, Clone)]
pub struct SchemaEntry {
    original: Option<SchemaRef>,
    schema: SchemaRef,
}

impl SchemaEntry {
    /// Creates a new SchemaEntry from a transformed Arrow schema.
    ///
    /// PRE-CONDITION: the schema must have been transformed to use SDF types.
    /// All types have been converted to types that static analysis expects
    /// and all the canonicalization steps have been applied (e.g. the
    /// `FixedSizeList` hack for Snowflake timestamps)
    pub fn from_sdf_arrow_schema(original: Option<SchemaRef>, schema: SchemaRef) -> Self {
        SchemaEntry { original, schema }
    }

    /// Returns the canonical (SDF) schema.
    pub fn inner(&self) -> &SchemaRef {
        &self.schema
    }

    /// Consumes the entry, yielding the canonical schema.
    pub fn into_inner(self) -> SchemaRef {
        self.schema
    }

    /// Returns the original warehouse schema, when present.
    pub fn original(&self) -> Option<&SchemaRef> {
        self.original.as_ref()
    }
}

/// Fallible operation result used throughout the schema store layer.
pub type SchemaStoreResult<T> = Result<T, ArrowError>;

/// Unified abstraction for reading and writing schemas within the store.
///
/// Implementors manage the backing persistence mechanism (filesystem, mocks,
/// etc.) while exposing consistent runtime guarantees:
///
/// * Frontier and deferred nodes always read from the remote-backed cache.
/// * Analyzed nodes store their compilation results locally.
/// * External nodes can be provisioned lazily when first encountered.
#[async_trait::async_trait]
pub trait SchemaStoreTrait: std::fmt::Debug + Send + Sync {
    /// Returns `true` if the store has an entry for the given canonical FQN.
    fn exists(&self, cfqn: &CanonicalFqn) -> bool;

    /// Asynchronously checks whether the store has an entry for the canonical
    /// FQN.  Use this from async contexts to avoid blocking on locks.
    async fn exists_async(&self, cfqn: &CanonicalFqn) -> bool;

    /// Returns `true` if a schema associated with the dbt `unique_id` exists.
    fn exists_by_unique_id(&self, unique_id: &str) -> bool;

    /// Retrieves a schema entry by its canonical FQN.
    fn get_schema(&self, cfqn: &CanonicalFqn) -> Option<SchemaEntry>;

    /// Retrieves a schema entry by its canonical FQN without blocking the
    /// current task.
    async fn get_schema_async(&self, cfqn: &CanonicalFqn) -> Option<SchemaEntry>;

    /// Returns whether the schema was restored from an earlier invocation.
    ///
    /// In-memory stores default to `false`; persistent stores override this so
    /// callers that require warehouse-current metadata can distinguish a warm
    /// disk-cache hit from a schema produced during the active invocation.
    fn schema_is_from_prior_invocation(&self, _cfqn: &CanonicalFqn) -> bool {
        false
    }

    /// Retrieves a schema entry by its dbt `unique_id`.
    fn get_schema_by_unique_id(&self, unique_id: &str) -> Option<SchemaEntry>;

    /// Retrieves a schema entry by its dbt `unique_id` without blocking the
    /// current task.
    async fn get_schema_by_unique_id_async(&self, unique_id: &str) -> Option<SchemaEntry>;

    /// Registers (or overwrites) the schema for the given canonical FQN.
    ///
    /// * `original_schema` captures the warehouse schema before canonicalization.
    /// * `sdf_schema` is the canonical schema used within Fusion.
    /// * `overwrite` controls whether existing entries should be replaced.
    fn register_schema(
        &self,
        cfqn: &CanonicalFqn,
        original_schema: Option<SchemaRef>,
        sdf_schema: SchemaRef,
        overwrite: bool,
    ) -> SchemaStoreResult<SchemaEntry>;

    /// Re-registers a `Selected` schema as a `Frontier` entry so that downstream
    /// models executed in the same run (Local execution mode) can find it.
    ///
    /// Default implementation is a no-op; overridden by [`SchemaStore`] for the
    /// `ParquetCache` format where per-file copying no longer happens automatically.
    fn promote_to_frontier(&self, _cfqn: &CanonicalFqn) -> SchemaStoreResult<()> {
        Ok(())
    }

    /// Returns the names of all catalogs tracked by this store.
    fn catalog_names(&self) -> Vec<CanonicalIdentifier>;

    /// Returns the schema names registered within the given catalog.
    fn schema_names(&self, catalog: &CanonicalIdentifier) -> Vec<CanonicalIdentifier>;

    /// Returns the table names registered within the given catalog + schema.
    fn table_names(
        &self,
        catalog: &CanonicalIdentifier,
        schema: &CanonicalIdentifier,
    ) -> Vec<CanonicalIdentifier>;
}

/// Abstraction for reading and writing materialized data maintained alongside
/// canonical schemas.
#[async_trait::async_trait]
pub trait DataStoreTrait: std::fmt::Debug + Send + Sync {
    /// Persists materialized data for the given canonical FQN using the provided schema.
    fn persist_data(
        &self,
        cfqn: &CanonicalFqn,
        schema: SchemaRef,
        batches: Vec<RecordBatch>,
    ) -> SchemaStoreResult<usize>;

    /// Asynchronously persists materialized data for the given canonical FQN.
    async fn persist_data_async(
        &self,
        cfqn: &CanonicalFqn,
        schema: SchemaRef,
        stream: std::pin::Pin<
            Box<dyn futures::Stream<Item = SchemaStoreResult<RecordBatch>> + Send + 'static>,
        >,
    ) -> SchemaStoreResult<usize>;

    /// Returns the canonical filesystem location for a table's materialized data.
    /// This normalizes to lowercase in order to ensure consistency between
    /// file systems
    fn get_path_to_data(&self, cfqn: &CanonicalFqn) -> PathBuf;
}
